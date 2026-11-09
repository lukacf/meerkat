/**
 * Per-admission completion for the TypeScript SDK.
 *
 * The defect: `sendAndWait` waited until the identity-wide completion cursor
 * passed the send's baseline, then returned the session's latest
 * `outputPreview`. A concurrent delivery (a peer message, a scheduled turn,
 * a fork completion wake) completing first satisfied the wait, and the caller
 * got someone else's output.
 *
 * `sendAndWait` / `dispatchAndWait` now send with `trackTurn` and wait on the
 * returned ticket through `meta/turn_result`, which reports THAT turn and
 * its own output.
 */

import { describe, it } from "node:test";
import assert from "node:assert/strict";

import {
  MetaError,
  TurnFailedError,
  TurnNotDeliveredError,
  TurnUnknownError,
} from "../src/errors.js";
import {
  dispatchResultToDict,
  parseDispatchResult,
  parseSendResult,
  parseTurnResult,
  sendResultToDict,
} from "../src/types.js";
import { runtimeConfig } from "./runtime-config.js";

type TurnScript = Record<string, Record<string, unknown>[]>;

const PENDING = { state: "pending" };

function completed(
  output: string | null,
  options: { status?: string; truncated?: boolean } = {},
): Record<string, unknown> {
  return {
    state: "completed",
    output_status: options.status ?? (output === null ? "empty" : "text"),
    output,
    output_truncated: options.truncated ?? false,
  };
}

function sent(
  ticket: string | null,
  extra: Record<string, unknown> = {},
): Record<string, unknown> {
  return {
    fencing_token: 3,
    completion_baseline: { epoch: 3, turns: 0 },
    ...(ticket !== null ? { turn: { ticket } } : {}),
    ...extra,
  };
}

function inspection(preview: string | null, turns: number) {
  return {
    output_preview: preview,
    completion_cursor: { epoch: 3, turns },
  };
}

/**
 * Model the gateway's server-side `meta/wait_for_completion`: walk the
 * scripted cursor states until one satisfies the wait; a script that ends
 * unsatisfied is the typed `timed_out`.
 */
function serveCompletionWait(
  walk: () => ReturnType<typeof inspection> | undefined,
  atEnd: () => boolean,
  identity: unknown,
  params: Record<string, unknown>,
): Record<string, unknown> {
  const after = params.after as { epoch: number; turns: number } | undefined;
  for (;;) {
    const cursor = walk()?.completion_cursor ?? null;
    const base = { identity, completion_cursor: cursor };
    if (cursor === null) return { ...base, outcome: "untracked" };
    if (after !== undefined && cursor.epoch !== after.epoch) {
      return { ...base, outcome: "incarnation_changed" };
    }
    if (cursor.turns > (after?.turns ?? 0)) {
      return { ...base, outcome: "completed" };
    }
    if (atEnd()) return { ...base, outcome: "timed_out" };
  }
}

/**
 * Runtime whose `_rpc` answers from a script: `sends` is consumed one per
 * send/dispatch, `turns` maps a ticket to the states `meta/turn_result`
 * walks through (one per poll, holding the last), and `inspections` models the
 * identity-wide cursor moving for someone else's turn.
 */
async function makeRuntime(script: {
  sends?: Record<string, unknown>[];
  turns?: TurnScript;
  inspections?: ReturnType<typeof inspection>[];
  /** Answer a pending `wait_for_turn` as ended by shutdown. */
  shuttingDown?: boolean;
}) {
  const { MetaRuntime } = await import("../src/runtime.js");
  const calls: { method: string; params: Record<string, unknown> }[] = [];
  const rt = new MetaRuntime(runtimeConfig());
  const sends = [...(script.sends ?? [])];
  const turns = script.turns ?? {};
  const polls = new Map<string, number>();
  const inspections = script.inspections ?? [];
  let inspectIndex = 0;
  let cursorReads = 0;
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
    if (method === "meta/turn_result") {
      const ticket = String(p.ticket);
      const states = turns[ticket] ?? [{ state: "unknown" }];
      const n = polls.get(ticket) ?? 0;
      polls.set(ticket, n + 1);
      return {
        identity: p.identity,
        ticket,
        ...states[Math.min(n, states.length - 1)],
      };
    }
    if (method === "meta/wait_for_turn") {
      // The gateway's server-side wait: answer once the ticket leaves
      // pending, or at the deadline (the script ends still pending).
      const ticket = String(p.ticket);
      const states = turns[ticket] ?? [{ state: "unknown" }];
      let n = Math.min(polls.get(ticket) ?? 0, states.length - 1);
      while (n < states.length - 1 && states[n]?.state === "pending") n += 1;
      polls.set(ticket, n + 1);
      const settled = { identity: p.identity, ticket, ...states[n] };
      if (states[n]?.state === "pending") {
        // The gateway holds a server-side wait until its deadline.
        await new Promise((resolve) =>
          setTimeout(resolve, Number(p.timeout_ms ?? 0)),
        );
        return { ...settled, wait: script.shuttingDown ? "shutting_down" : "timed_out" };
      }
      return { ...settled, wait: "settled" };
    }
    if (method === "meta/wait_for_completion") {
      cursorReads += 1;
      return serveCompletionWait(
        () => inspections[Math.min(inspectIndex++, inspections.length - 1)],
        () => inspectIndex >= inspections.length,
        p.identity,
        p,
      );
    }
    if (method === "meta/completion_cursor") {
      cursorReads += 1;
      const entry = inspections[Math.min(inspectIndex, inspections.length - 1)];
      inspectIndex += 1;
      return {
        identity: p.identity,
        state: "active",
        completion_cursor: entry?.completion_cursor ?? null,
      };
    }
    if (method === "meta/inspect_identity") {
      const entry =
        cursorReads > 0
          ? inspections[Math.min(inspectIndex - 1, inspections.length - 1)]
          : inspections[Math.min(inspectIndex++, inspections.length - 1)];
      return { identity: p.identity, is_final: false, ...entry };
    }
    return {};
  };
  (rt as unknown as Record<string, unknown>)._running = true;
  const paramsOf = (method: string) =>
    calls.filter((c) => c.method === method).map((c) => c.params);
  return { rt, calls, paramsOf };
}

const FAST = { timeoutMs: 5_000, pollIntervalMs: 1 };

/** Capture the typed process warnings a call emits, as "Type: message". */
async function withWarnings<T>(
  run: () => Promise<T>,
): Promise<{ value: T; warnings: string[] }> {
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

describe("sendAndWait waits for its own turn", () => {
  it("is not satisfied by a foreign completion", async () => {
    const { rt, paramsOf } = await makeRuntime({
      sends: [sent("ticket-a")],
      turns: { "ticket-a": [PENDING, PENDING, completed("A's reply")] },
      // The identity-wide cursor already moved for a foreign turn.
      inspections: [inspection("foreign reply", 1)],
    });

    const { value: output, warnings } = await withWarnings(() =>
      rt.sendAndWait("keeper", "alpha", FAST),
    );

    assert.equal(output, "A's reply");
    assert.deepEqual(warnings, []);
    assert.equal(paramsOf("meta/send")[0]?.track_turn, true);
    assert.deepEqual(
      paramsOf("meta/inspect_identity"),
      [],
      "a ticketed wait must not read the member",
    );
    assert.deepEqual(
      paramsOf("meta/completion_cursor"),
      [],
      "a ticketed wait must not read the identity-wide cursor",
    );
    assert.deepEqual(paramsOf("meta/turn_result"), [], "no client polling");
    assert.equal(paramsOf("meta/wait_for_turn").length, 1);
    assert.deepEqual(paramsOf("meta/wait_for_completion"), []);
  });

  it("gives concurrent sends each their own output", async () => {
    const { rt } = await makeRuntime({
      sends: [sent("ticket-a"), sent("ticket-b")],
      turns: {
        "ticket-a": [PENDING, PENDING, PENDING, completed("A's reply")],
        "ticket-b": [PENDING, completed("B's reply")],
      },
    });

    const outputs = await Promise.all([
      rt.sendAndWait("keeper", "alpha", FAST),
      rt.sendAndWait("keeper", "beta", FAST),
    ]);

    assert.deepEqual(outputs, ["A's reply", "B's reply"]);
  });

  it("control: the identity-wide cursor wait returns the foreign preview", async () => {
    const { rt } = await makeRuntime({
      inspections: [inspection("foreign reply", 1)],
    });

    const output = await rt.waitForCompletion(
      "keeper",
      { epoch: 3, turns: 0 },
      FAST,
    );

    assert.equal(output, "foreign reply");
  });

  it("dispatchAndWait waits on its ticket", async () => {
    const { rt, paramsOf } = await makeRuntime({
      sends: [sent("ticket-d", { durable: true })],
      turns: { "ticket-d": [completed("dispatched reply")] },
    });

    const output = await rt.dispatchAndWait(
      "keeper",
      { content: "work", origin: "system" },
      FAST,
    );

    assert.equal(output, "dispatched reply");
    assert.equal(paramsOf("meta/dispatch")[0]?.track_turn, true);
  });
});

describe("waitForTurn", () => {
  it("throws TurnFailedError for a failed turn", async () => {
    const { rt } = await makeRuntime({
      turns: { "ticket-a": [{ state: "failed", error: "the model refused" }] },
    });

    await assert.rejects(rt.waitForTurn("keeper", "ticket-a", FAST), (error) => {
      assert.ok(error instanceof TurnFailedError);
      assert.equal(error.reason, "the model refused");
      return true;
    });
  });

  it("throws TurnUnknownError and never guesses", async () => {
    const { rt } = await makeRuntime({
      turns: { "ticket-a": [{ state: "unknown" }] },
    });

    await assert.rejects(
      rt.waitForTurn("keeper", "ticket-a", FAST),
      TurnUnknownError,
    );
  });

  it("times out on a pending turn", async () => {
    const { rt } = await makeRuntime({ turns: { "ticket-a": [PENDING] } });

    await assert.rejects(
      rt.waitForTurn("keeper", "ticket-a", { timeoutMs: 20, pollIntervalMs: 1 }),
      /did not complete/,
    );
  });

  it("resolves the typed result", async () => {
    const { rt } = await makeRuntime({
      turns: {
        "ticket-a": [completed("A's reply")],
        "ticket-e": [completed(null)],
        "ticket-n": [completed(null, { status: "no_own_result" })],
        "ticket-t": [completed("long[truncated]", { truncated: true })],
      },
    });

    const text = await rt.waitForTurn("keeper", "ticket-a", FAST);
    assert.equal(text.outputStatus, "text");
    assert.equal(text.output, "A's reply");
    const empty = await rt.waitForTurn("keeper", "ticket-e", FAST);
    assert.equal(empty.outputStatus, "empty");
    assert.equal(empty.output, null);
    const folded = await rt.waitForTurn("keeper", "ticket-n", FAST);
    assert.equal(folded.outputStatus, "no_own_result");
    const cut = await rt.waitForTurn("keeper", "ticket-t", FAST);
    assert.equal(cut.outputTruncated, true);
  });

  it("sendAndWait never returns partial or absent output silently", async () => {
    const { rt } = await makeRuntime({
      sends: [sent("ticket-t"), sent("ticket-n"), sent("ticket-u"), sent("ticket-e")],
      turns: {
        "ticket-t": [completed("long[truncated]", { truncated: true })],
        "ticket-n": [completed(null, { status: "no_own_result" })],
        "ticket-u": [completed(null, { status: "unavailable" })],
        "ticket-e": [completed(null)],
      },
    });

    const truncated = await withWarnings(() => rt.sendAndWait("keeper", "t", FAST));
    assert.equal(truncated.value, "long[truncated]");
    assert.match(truncated.warnings[0] ?? "", /^TurnOutputTruncatedWarning: /);
    const folded = await withWarnings(() => rt.sendAndWait("keeper", "n", FAST));
    assert.equal(folded.value, null);
    assert.match(
      folded.warnings[0] ?? "",
      /^TurnOutputUnavailableWarning: .*no_own_result/,
    );
    const unavailable = await withWarnings(() => rt.sendAndWait("keeper", "u", FAST));
    assert.equal(unavailable.value, null);
    assert.match(unavailable.warnings[0] ?? "", /unavailable/);
    const empty = await withWarnings(() => rt.sendAndWait("keeper", "e", FAST));
    assert.equal(empty.value, null);
    assert.deepEqual(empty.warnings, [], "no text committed: nothing to warn about");
  });
});

describe("server-side turn waits", () => {
  it("a shutdown ends a pending turn wait with WaitEndedError", async () => {
    const { WaitEndedError } = await import("../src/errors.js");
    const { rt } = await makeRuntime({
      turns: { "ticket-a": [PENDING] },
      shuttingDown: true,
    });
    await assert.rejects(rt.waitForTurn("keeper", "ticket-a", FAST), (error) => {
      assert.ok(error instanceof WaitEndedError);
      assert.equal(error.outcome, "shutting_down");
      return true;
    });
  });
});

describe("explicit fallback", () => {
  it("falls back to the cursor with a warning on an old gateway", async () => {
    const { rt, paramsOf } = await makeRuntime({
      sends: [sent(null)],
      inspections: [inspection("latest reply", 1)],
    });

    const { value, warnings } = await withWarnings(() =>
      rt.sendAndWait("keeper", "alpha", FAST),
    );

    assert.equal(value, "latest reply");
    assert.equal(warnings.length, 1);
    assert.match(
      warnings[0] ?? "",
      /^TurnTrackingUnavailableWarning: .*predates turn tickets/,
    );
    assert.deepEqual(paramsOf("meta/turn_result"), []);
  });

  it("names why a turn could not be tracked", async () => {
    const { rt } = await makeRuntime({
      sends: [
        sent(null, {
          turn: null,
          turn_unavailable: {
            code: "autonomous_host",
            reason: "the member runs in autonomous_host mode",
          },
        }),
      ],
      inspections: [inspection("latest reply", 1)],
    });

    const { value, warnings } = await withWarnings(() =>
      rt.sendAndWait("keeper", "alpha", FAST),
    );

    assert.equal(value, "latest reply");
    assert.match(warnings[0] ?? "", /autonomous_host/);
  });

  it("an undelivered send throws instead of waiting", async () => {
    const { rt, paramsOf } = await makeRuntime({
      sends: [
        sent(null, {
          turn: null,
          turn_unavailable: {
            code: "not_delivered",
            reason: "no session bridge",
            delivered: false,
          },
        }),
      ],
    });

    await assert.rejects(rt.sendAndWait("keeper", "alpha", FAST), (error) => {
      assert.ok(error instanceof TurnNotDeliveredError);
      assert.ok(error instanceof MetaError);
      assert.equal(error.code, "not_delivered");
      assert.equal(error.reason, "no session bridge");
      return true;
    });
    assert.deepEqual(paramsOf("meta/inspect_identity"), []);
  });

  it("a plain send does not request tracking", async () => {
    const { rt, paramsOf } = await makeRuntime({ sends: [sent(null)] });

    const result = await rt.send("keeper", "alpha");

    assert.equal(paramsOf("meta/send")[0]?.track_turn, undefined);
    assert.equal(result.turnTicket, null);
  });
});

describe("models", () => {
  it("carry the ticket and round-trip", () => {
    const send = parseSendResult(sent("ticket-a"));
    assert.equal(send.turnTicket, "ticket-a");
    assert.deepEqual(parseSendResult(sendResultToDict(send)), send);
    const unavailable = parseDispatchResult(
      sent(null, {
        durable: false,
        turn: null,
        turn_unavailable: { code: "not_delivered", reason: "no bridge" },
      }),
    );
    assert.equal(unavailable.turnTicket, null);
    assert.deepEqual(unavailable.turnUnavailable, {
      code: "not_delivered",
      reason: "no bridge",
      delivered: false,
    });
    assert.deepEqual(
      parseDispatchResult(dispatchResultToDict(unavailable)),
      unavailable,
    );
  });

  it("parse every turn state", () => {
    const done = parseTurnResult({
      identity: "keeper",
      ticket: "t",
      ...completed("hi"),
      completion_cursor: { epoch: 3, turns: 4 },
    });
    assert.equal(done.state, "completed");
    assert.equal(done.outputStatus, "text");
    assert.equal(done.output, "hi");
    assert.deepEqual(done.completionCursor, { epoch: 3, turns: 4 });
    const failed = parseTurnResult({ state: "failed", error: "boom" });
    assert.equal(failed.state, "failed");
    assert.equal(failed.error, "boom");
    assert.equal(parseTurnResult({ state: "surprise" }).state, "unknown");
    const future = parseTurnResult({
      state: "completed",
      output_status: "later",
      output: "x",
    });
    assert.equal(future.outputStatus, "unavailable");
    assert.equal(future.output, null, "text is only read for the text status");
  });
});
