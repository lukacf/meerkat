/**
 * `*AndWait` never attributes another turn and never loses its admission
 * (parity with the Python SDK).
 *
 * The incident (MobKit 0.8.42): an admitted dispatch's wait failed with
 * `observation_lane_saturated`, and the admission result was only a local
 * variable, inviting callers to redispatch business work. An untracked
 * delivery never throws by default: it keeps the identity-wide wait, typed
 * as non-attributed with its `turnUnavailable` code and warned; only
 * `requireAttribution` throws, for every code. Every failure after admission
 * carries the admission.
 */

import { describe, it } from "node:test";
import assert from "node:assert/strict";

import {
  MetaError,
  PostAdmissionObservationError,
  RpcError,
  TurnTrackingUnavailableError,
  TurnUnknownError,
  TurnWaitTimeoutError,
  WaitEndedError,
} from "../src/errors.js";
import { runtimeConfig } from "./runtime-config.js";

const TICKET = "6f1c2a8e-0d4b-4b7e-9a51-3c2f8d7e1a90";
const SATURATED = {
  code: -32000,
  message: "observation_lane_saturated: member_status_observation",
};
const FAST = { timeoutMs: 5_000, pollIntervalMs: 1 };

type Step = { error: { code: number; message: string } } | Record<string, unknown>;

function admitted(
  ticket: string | null,
  unavailable: string | null = null,
): Record<string, unknown> {
  return {
    fencing_token: 3,
    completion_baseline: { epoch: 3, turns: 4 },
    turn: ticket !== null ? { ticket } : null,
    ...(unavailable !== null
      ? { turn_unavailable: { code: unavailable, reason: `because ${unavailable}` } }
      : {}),
  };
}

function own(output: string): Record<string, unknown> {
  return {
    state: "completed",
    wait: "settled",
    output_status: "text",
    output,
    output_truncated: false,
  };
}

/**
 * Runtime whose `_rpc` answers from a script. `waitForTurn` steps are walked
 * one per `meta/wait_for_turn` (holding the last); a step with `error`
 * rejects like a gateway JSON-RPC error. The identity-wide methods report a
 * peer's completed turn, so any fallback is visible.
 */
async function makeRuntime(script: {
  sends: Record<string, unknown>[];
  waitForTurn?: Step[];
  /** Answer for `meta/wait_for_completion`; `raw` throws a plain Error. */
  completion?: Record<string, unknown> | { raw: string };
}) {
  const { MetaRuntime } = await import("../src/runtime.js");
  const calls: { method: string; params: Record<string, unknown> }[] = [];
  const rt = new MetaRuntime(runtimeConfig());
  const sends = [...script.sends];
  const steps = script.waitForTurn ?? [];
  let waitIndex = 0;
  (rt as unknown as Record<string, unknown>)._rpc = async (
    method: string,
    params?: Record<string, unknown>,
  ) => {
    await Promise.resolve();
    const p = params ?? {};
    calls.push({ method, params: p });
    if (method === "meta/send" || method === "meta/dispatch") {
      return sends.shift() ?? {};
    }
    if (method === "meta/wait_for_turn") {
      const step = steps[Math.min(waitIndex, steps.length - 1)] ?? { state: "unknown" };
      waitIndex += 1;
      if ("error" in step && typeof step.error === "object" && step.error !== null) {
        const error = step.error as { code: number; message: string };
        throw new RpcError(error.code, error.message, "rid", method);
      }
      if ((step as Record<string, unknown>).state === "pending") {
        await new Promise((resolve) => setTimeout(resolve, Number(p.timeout_ms ?? 0)));
      }
      return { identity: p.identity, ticket: p.ticket, ...step };
    }
    if (method === "meta/wait_for_completion") {
      const completion = script.completion;
      if (completion !== undefined && "raw" in completion) {
        throw new Error(String(completion.raw));
      }
      return {
        identity: p.identity,
        outcome: "completed",
        completion_cursor: { epoch: 3, turns: 5 },
        ...(completion ?? {}),
      };
    }
    if (method === "meta/inspect_identity") {
      return {
        identity: p.identity,
        output_preview: "peer reply",
        completion_cursor: { epoch: 3, turns: 5 },
      };
    }
    return {};
  };
  await Promise.resolve();
  (rt as unknown as Record<string, unknown>)._running = true;
  const count = (method: string) => calls.filter((c) => c.method === method).length;
  const identityWide = () =>
    calls.filter((c) =>
      ["meta/wait_for_completion", "meta/completion_cursor", "meta/inspect_identity"].includes(
        c.method,
      ),
    ).length;
  return { rt, count, identityWide };
}

async function withWarnings<T>(run: () => Promise<T>): Promise<{ value: T; warnings: string[] }> {
  const warnings: string[] = [];
  const original = process.emitWarning;
  process.emitWarning = ((message: string | Error, options?: unknown) => {
    const type =
      typeof options === "object" && options !== null
        ? String((options as { type?: unknown }).type)
        : String(options);
    warnings.push(`${type}: ${String(message)}`);
  }) as typeof process.emitWarning;
  try {
    return { value: await run(), warnings };
  } finally {
    process.emitWarning = original;
  }
}

describe("attribution", () => {
  it("a tracked outcome is attributed", async () => {
    const { rt } = await makeRuntime({ sends: [admitted(TICKET)], waitForTurn: [own("own reply")] });

    const outcome = await rt.sendAndWaitOutcome("keeper", "alpha", FAST);

    assert.equal(outcome.text, "own reply");
    assert.equal(outcome.attributed, true);
    assert.equal(outcome.ticket, TICKET);
    assert.equal(outcome.outputStatus, "text");
    assert.equal(outcome.untrackedCode, null);
    assert.equal(outcome.admission.turnTicket, TICKET);
  });

  for (const code of [
    "autonomous_host",
    "externally_bound",
    "runtime_refused",
    "session_rotated",
    "a_future_code",
  ]) {
    it(`untracked ${code} waits identity-wide, typed as not attributed`, async () => {
      const { rt, count } = await makeRuntime({ sends: [admitted(null, code)] });

      const { value: outcome, warnings } = await withWarnings(() =>
        rt.dispatchAndWaitOutcome("keeper", { content: "alpha", origin: "system" }, FAST),
      );

      assert.equal(outcome.text, "peer reply");
      assert.equal(outcome.attributed, false);
      assert.equal(outcome.untrackedCode, code);
      assert.equal(outcome.ticket, null);
      assert.match(warnings[0] ?? "", new RegExp(`TurnTrackingUnavailableWarning: .*${code}.*not attributed`));
      assert.equal(count("meta/dispatch"), 1);
    });
  }

  it("an old gateway keeps the plain text API working (non-attributed)", async () => {
    const { rt } = await makeRuntime({ sends: [admitted(null)] });

    const { value, warnings } = await withWarnings(() => rt.sendAndWait("keeper", "alpha", FAST));

    assert.equal(value, "peer reply");
    assert.match(warnings[0] ?? "", /predates turn tickets/);
  });

  for (const code of ["autonomous_host", "runtime_refused", null]) {
    it(`requireAttribution throws when untracked (${code ?? "old gateway"})`, async () => {
      const { rt, count, identityWide } = await makeRuntime({ sends: [admitted(null, code)] });

      await assert.rejects(
        rt.sendAndWait("keeper", "alpha", { ...FAST, requireAttribution: true }),
        (error) => {
          assert.ok(error instanceof TurnTrackingUnavailableError);
          assert.equal(error.code, code);
          return true;
        },
      );
      assert.equal(count("meta/send"), 1);
      assert.equal(identityWide(), 0);
    });
  }

});

describe("post-admission observation", () => {
  it("a tracked wait that ended carries the admission and its ticket", async () => {
    const { rt, count } = await makeRuntime({
      sends: [admitted(TICKET)],
      waitForTurn: [{ state: "pending", wait: "shutting_down" }],
    });

    await assert.rejects(rt.sendAndWait("keeper", "alpha", FAST), (error) => {
      assert.ok(error instanceof WaitEndedError);
      assert.equal((error.admission as { turnTicket: unknown }).turnTicket, TICKET);
      assert.equal(error.ticket, TICKET);
      return true;
    });
    assert.equal(count("meta/send"), 1);
  });

  it("retries only the exact ticket after a read failure and never redispatches", async () => {
    const { rt, count, identityWide } = await makeRuntime({
      sends: [admitted(TICKET)],
      waitForTurn: [{ error: SATURATED }, { error: SATURATED }, own("own reply")],
    });

    const outcome = await rt.dispatchAndWaitOutcome(
      "keeper",
      { content: "process the incident", origin: "system", idempotencyKey: "incident-7" },
      FAST,
    );

    assert.equal(outcome.text, "own reply");
    assert.equal(outcome.attributed, true);
    assert.equal(count("meta/dispatch"), 1);
    assert.equal(count("meta/wait_for_turn"), 3);
    assert.equal(identityWide(), 0);
  });

  it("a persistent read failure throws typed with admission, ticket and cause", async () => {
    const { rt, count } = await makeRuntime({
      sends: [admitted(TICKET)],
      waitForTurn: [{ error: SATURATED }],
    });

    await assert.rejects(
      rt.dispatchAndWait("keeper", { content: "process", origin: "system" }, {
        timeoutMs: 1_200,
        pollIntervalMs: 1,
      }),
      (error) => {
        assert.ok(error instanceof PostAdmissionObservationError);
        assert.equal(error.ticket, TICKET);
        assert.equal((error.admission as { turnTicket: unknown }).turnTicket, TICKET);
        assert.ok(error.attempts >= 2);
        assert.ok(error.cause instanceof RpcError);
        assert.match(String((error.cause as Error).message), /observation_lane_saturated/);
        return true;
      },
    );
    assert.equal(count("meta/dispatch"), 1);
  });

  it("an unknown ticket throws TurnUnknownError carrying the admission", async () => {
    const { rt, count } = await makeRuntime({
      sends: [admitted(TICKET)],
      waitForTurn: [{ state: "unknown", wait: "settled" }],
    });

    await assert.rejects(rt.sendAndWait("keeper", "alpha", FAST), (error) => {
      assert.ok(error instanceof TurnUnknownError);
      assert.equal((error.admission as { turnTicket: unknown }).turnTicket, TICKET);
      return true;
    });
    assert.equal(count("meta/send"), 1);
  });

  it("a turn pending at the deadline throws TurnWaitTimeoutError with the admission", async () => {
    const { rt, count } = await makeRuntime({
      sends: [admitted(TICKET)],
      waitForTurn: [{ state: "pending", wait: "timed_out" }],
    });

    await assert.rejects(
      rt.sendAndWait("keeper", "alpha", { timeoutMs: 200, pollIntervalMs: 1 }),
      (error) => {
        assert.ok(error instanceof TurnWaitTimeoutError);
        assert.equal(error.ticket, TICKET);
        assert.equal((error.admission as { turnTicket: unknown }).turnTicket, TICKET);
        return true;
      },
    );
    assert.equal(count("meta/send"), 1);
  });
});

describe("default-path custody", () => {
  async function failOnDefaultPath(
    sends: Record<string, unknown>[],
    completion: Record<string, unknown> | { raw: string },
  ): Promise<{ error: unknown; dispatches: number }> {
    const { rt, count } = await makeRuntime({ sends, completion });
    let error: unknown;
    await withWarnings(async () => {
      try {
        await rt.dispatchAndWait("keeper", { content: "alpha", origin: "system" }, {
          timeoutMs: 200,
          pollIntervalMs: 1,
        });
      } catch (caught) {
        error = caught;
      }
    });
    return { error, dispatches: count("meta/dispatch") };
  }

  function custody(error: unknown): { admission: unknown; ticket: unknown } {
    return error as { admission: unknown; ticket: unknown };
  }

  it("a wait that ended keeps its type and carries the admission", async () => {
    const { error, dispatches } = await failOnDefaultPath([admitted(null, "autonomous_host")], {
      outcome: "broken",
      completion_cursor: { epoch: 3, turns: 4 },
    });
    assert.ok(error instanceof WaitEndedError);
    assert.equal((custody(error).admission as { turnTicket: unknown }).turnTicket, null);
    assert.equal(custody(error).ticket, null);
    assert.equal(dispatches, 1);
  });

  it("the cursor deadline keeps its type and carries the admission", async () => {
    const { error, dispatches } = await failOnDefaultPath([admitted(null, "autonomous_host")], {
      outcome: "timed_out",
      completion_cursor: { epoch: 3, turns: 4 },
    });
    assert.ok(error instanceof Error);
    assert.match(String((error as Error).message), /did not complete a turn past/);
    assert.notEqual(custody(error).admission, undefined);
    assert.equal(custody(error).ticket, null);
    assert.equal(dispatches, 1);
  });

  it("a missing baseline keeps its type and carries the admission", async () => {
    const sent = admitted(null, "autonomous_host");
    delete sent.completion_baseline;
    const { error, dispatches } = await failOnDefaultPath([sent], {});
    assert.ok(error instanceof Error);
    assert.match(String((error as Error).message), /no completion_baseline/);
    assert.notEqual(custody(error).admission, undefined);
    assert.equal(custody(error).ticket, null);
    assert.equal(dispatches, 1);
  });

  it("an incarnation change keeps its type and carries the admission", async () => {
    const { error, dispatches } = await failOnDefaultPath([admitted(null, "autonomous_host")], {
      outcome: "incarnation_changed",
      completion_cursor: { epoch: 4, turns: 0 },
    });
    assert.ok(error instanceof Error);
    assert.match(String((error as Error).message), /superseded runtime/);
    assert.notEqual(custody(error).admission, undefined);
    assert.equal(dispatches, 1);
  });

  it("a raw transport Error keeps its type and carries the admission", async () => {
    const { error, dispatches } = await failOnDefaultPath([admitted(null, "autonomous_host")], {
      raw: "persistent transport: timeout after 5200ms",
    });
    assert.ok(error instanceof Error);
    assert.equal(Object.getPrototypeOf(error), Error.prototype);
    assert.match(String((error as Error).message), /persistent transport: timeout/);
    assert.equal((custody(error).admission as { turnTicket: unknown }).turnTicket, null);
    assert.equal(custody(error).ticket, null);
    assert.equal(dispatches, 1);
  });
});
