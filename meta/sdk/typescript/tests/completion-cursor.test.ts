/**
 * Turn-completion contract for the TypeScript SDK.
 *
 * The defect this mirrors: a consumer captured the previous turn's output text
 * as a baseline, sent again, and waited for the text to change. Two turns that
 * both answer exactly `ACK` are indistinguishable from no turn at all, so the
 * wait sleeps out its whole timeout. Completion is a cursor —
 * `{epoch, turns}` — never a text comparison.
 */

import { describe, it } from "node:test";
import assert from "node:assert/strict";

import type { CompletionCursor } from "../src/types.js";
import { runtimeConfig } from "./runtime-config.js";

type ScriptedInspection = {
  output_preview: string | null;
  completion_cursor: { epoch: number; turns: number } | null;
};

/**
 * Model the gateway's server-side `meta/wait_for_completion`: walk the
 * scripted cursor states (each a later moment) until one satisfies the wait.
 * A script that ends unsatisfied is the typed `timed_out`.
 */
function serveCompletionWait(
  walk: () => ScriptedInspection,
  atEnd: () => boolean,
  identity: unknown,
  params: Record<string, unknown>,
): Record<string, unknown> {
  const after = params.after as { epoch: number; turns: number } | undefined;
  for (;;) {
    const entry = walk();
    const cursor = entry.completion_cursor;
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
 * Runtime whose `_rpc` answers from a script of inspection payloads, each a
 * later moment, holding the last once exhausted, so "still running, then
 * done" is expressible without racing a clock. `wait_for_completion` models
 * the gateway's server-side wait ({@link serveCompletionWait}). After a wait
 * that resolved, `inspect_identity` answers the entry the wait resolved on
 * (the output a waiter reads once, at completion); otherwise it walks the
 * script itself, as does `completion_cursor`. `legacyGateway` models a
 * gateway predating the server-side wait and the cursor read ("method not
 * found").
 */
async function makeRuntime(script: {
  send?: Record<string, unknown>;
  dispatch?: Record<string, unknown>;
  inspections?: ScriptedInspection[];
  legacyGateway?: boolean;
  /** Answer every `wait_for_completion` with this typed outcome. */
  waitOutcome?: string;
}) {
  const { MetaRuntime } = await import("../src/runtime.js");
  const { RpcError } = await import("../src/errors.js");
  /** Every RPC the fake gateway received, with the wall-clock time it arrived. */
  const calls: { method: string; params: Record<string, unknown>; atMs: number }[] = [];
  const rt = new MetaRuntime(runtimeConfig());
  const inspections = script.inspections ?? [];
  let index = 0;
  let pinned = false;
  const walk = () => {
    const entry = inspections[Math.min(index, inspections.length - 1)];
    index += 1;
    return entry;
  };
  (rt as unknown as Record<string, unknown>)._rpc = async (
    method: string,
    params?: Record<string, unknown>,
  ) => {
    calls.push({ method, params: params ?? {}, atMs: Date.now() });
    if (method === "meta/send") return script.send ?? {};
    if (method === "meta/dispatch") return script.dispatch ?? {};
    if (
      script.legacyGateway &&
      (method === "meta/completion_cursor" ||
        method === "meta/wait_for_completion")
    ) {
      throw new RpcError(-32601, "method not found", "1", method);
    }
    if (method === "meta/wait_for_completion") {
      if (script.waitOutcome !== undefined) {
        return {
          identity: params?.identity ?? "x:1",
          outcome: script.waitOutcome,
          completion_cursor: { epoch: 3, turns: 1 },
        };
      }
      const result = serveCompletionWait(
        walk,
        () => index >= inspections.length,
        params?.identity ?? "x:1",
        params ?? {},
      );
      pinned =
        result.outcome === "completed" ||
        result.outcome === "incarnation_changed";
      if (result.outcome === "timed_out") {
        // The gateway holds a server-side wait until its deadline.
        await new Promise((resolve) =>
          setTimeout(resolve, Number(params?.timeout_ms ?? 0)),
        );
      }
      return result;
    }
    if (method === "meta/completion_cursor") {
      const entry = walk();
      return {
        identity: params?.identity ?? "x:1",
        state: "active",
        completion_cursor: entry.completion_cursor,
      };
    }
    if (method === "meta/inspect_identity") {
      const entry = pinned
        ? inspections[Math.min(index - 1, inspections.length - 1)]
        : walk();
      return { identity: params?.identity ?? "x:1", is_final: false, ...entry };
    }
    return { accepted: true };
  };
  (rt as unknown as Record<string, unknown>)._running = true;
  const count = (method: string) =>
    calls.filter((c) => c.method === method).length;
  const inspectCalls = () => count("meta/inspect_identity");
  const cursorCalls = () => count("meta/completion_cursor");
  const waitCalls = () => count("meta/wait_for_completion");
  return { rt, calls, inspectCalls, cursorCalls, waitCalls };
}

// ---------------------------------------------------------------------------
// The production regression
// ---------------------------------------------------------------------------

describe("identical consecutive output", () => {
  it("detects the second turn when both answer exactly ACK", async () => {
    const { rt, inspectCalls, cursorCalls, waitCalls } = await makeRuntime({
      send: { fencing_token: 7, completion_baseline: { epoch: 7, turns: 1 } },
      inspections: [
        // Turn 2 in flight — the PREVIOUS turn's ACK is still visible.
        { output_preview: "ACK", completion_cursor: { epoch: 7, turns: 1 } },
        // Turn 2 committed. Same text, byte for byte.
        { output_preview: "ACK", completion_cursor: { epoch: 7, turns: 2 } },
      ],
    });

    const output = await rt.sendAndWait("triage:main", "ping", {
      timeoutMs: 5000,
      pollIntervalMs: 1,
    });

    assert.equal(output, "ACK");
    assert.equal(waitCalls(), 1, "one server-side wait, no client polling");
    assert.equal(cursorCalls(), 0);
    assert.equal(inspectCalls(), 1, "output is read once, at completion");
  });

  it("dispatchAndWait threads its own baseline", async () => {
    const { rt, inspectCalls, waitCalls } = await makeRuntime({
      dispatch: {
        fencing_token: 4,
        durable: true,
        completion_baseline: { epoch: 4, turns: 5 },
      },
      inspections: [
        { output_preview: "ACK", completion_cursor: { epoch: 4, turns: 5 } },
        { output_preview: "ACK", completion_cursor: { epoch: 4, turns: 6 } },
      ],
    });

    const output = await rt.dispatchAndWait(
      "internal:main",
      { content: "go", origin: "system" },
      { timeoutMs: 5000, pollIntervalMs: 1 },
    );

    assert.equal(output, "ACK");
    assert.equal(waitCalls(), 1);
    assert.equal(inspectCalls(), 1);
  });
});

// ---------------------------------------------------------------------------
// Waiter semantics
// ---------------------------------------------------------------------------

describe("waitForCompletion", () => {
  it("times out on a genuinely stalled turn", async () => {
    const { rt } = await makeRuntime({
      inspections: [
        { output_preview: "ACK", completion_cursor: { epoch: 3, turns: 1 } },
      ],
    });

    await assert.rejects(
      rt.waitForCompletion(
        "triage:main",
        { epoch: 3, turns: 1 },
        { timeoutMs: 50, pollIntervalMs: 1 },
      ),
      /did not complete a turn/,
    );
  });

  it("reports an incarnation change rather than guessing", async () => {
    const { rt } = await makeRuntime({
      inspections: [
        { output_preview: "ACK", completion_cursor: { epoch: 9, turns: 0 } },
      ],
    });

    await assert.rejects(
      rt.waitForCompletion(
        "triage:main",
        { epoch: 3, turns: 1 },
        { timeoutMs: 5000, pollIntervalMs: 1 },
      ),
      /superseded runtime incarnation/,
    );
  });

  it("fails loudly against a gateway with no cursor", async () => {
    const { rt } = await makeRuntime({
      send: { fencing_token: 7 },
      inspections: [{ output_preview: "ACK", completion_cursor: null }],
    });

    await assert.rejects(
      rt.sendAndWait("triage:main", "ping", {
        timeoutMs: 1000,
        pollIntervalMs: 1,
      }),
      /no completion_baseline/,
    );
  });

  it("a different identity's completion does not satisfy the wait", async () => {
    // The cursor is per-identity: this identity's cursor never moves, so the
    // wait must time out no matter what any other identity did.
    const { rt, calls } = await makeRuntime({
      inspections: [
        { output_preview: "ACK", completion_cursor: { epoch: 4, turns: 2 } },
      ],
    });

    await assert.rejects(
      rt.waitForCompletion(
        "triage:main",
        { epoch: 4, turns: 2 },
        { timeoutMs: 50, pollIntervalMs: 1 },
      ),
      /did not complete a turn/,
    );
    assert.ok(
      calls.every((c) => c.params.identity === "triage:main"),
      "the waiter must only ever poll its own identity",
    );
  });
});

// ---------------------------------------------------------------------------
// Server-side, event-driven waits (#468)
// ---------------------------------------------------------------------------

describe("server-side completion waits", () => {
  it("waitForCompletion is one server wait, then one member read", async () => {
    const { rt, calls } = await makeRuntime({
      inspections: [
        { output_preview: "old", completion_cursor: { epoch: 3, turns: 1 } },
        { output_preview: "old", completion_cursor: { epoch: 3, turns: 1 } },
        { output_preview: "new", completion_cursor: { epoch: 3, turns: 2 } },
      ],
    });

    const startedAtMs = Date.now();
    const output = await rt.waitForCompletion(
      "triage:main",
      { epoch: 3, turns: 1 },
      { timeoutMs: 7000 },
    );

    assert.equal(output, "new");
    assert.deepEqual(
      calls.map((c) => c.method),
      ["meta/wait_for_completion", "meta/inspect_identity"],
    );
    const { timeout_ms: timeoutMs, ...waitParams } = calls[0]?.params ?? {};
    assert.deepEqual(waitParams, {
      identity: "triage:main",
      after: { epoch: 3, turns: 1 },
    });
    // The wait carries the budget left when it is sent, floor(deadline -
    // now): never more than the caller's 7000 ms, and never less than what
    // remained when the gateway received it. Time that passes before the
    // send narrows the budget; it never fails the bound.
    const elapsedBeforeReceiptMs = (calls[0]?.atMs ?? Number.NaN) - startedAtMs;
    assert.ok(
      typeof timeoutMs === "number" &&
        Number.isInteger(timeoutMs) &&
        timeoutMs <= 7000 &&
        timeoutMs >= 7000 - elapsedBeforeReceiptMs,
      `the one server wait must carry the remaining budget (7000 ms less the ${String(elapsedBeforeReceiptMs)} ms before it was received), got ${String(timeoutMs)}`,
    );
  });

  it("a server timeout rejects with the timeout error", async () => {
    const { rt, inspectCalls } = await makeRuntime({
      inspections: [
        { output_preview: "ACK", completion_cursor: { epoch: 3, turns: 1 } },
      ],
    });

    await assert.rejects(
      rt.waitForCompletion(
        "triage:main",
        { epoch: 3, turns: 1 },
        { timeoutMs: 50 },
      ),
      /did not complete a turn/,
    );
    assert.equal(inspectCalls(), 0);
  });

  it("waitUntilReady is one server wait per identity", async () => {
    const { rt, calls, inspectCalls, waitCalls } = await makeRuntime({
      inspections: [
        { output_preview: null, completion_cursor: { epoch: 3, turns: 0 } },
        { output_preview: null, completion_cursor: { epoch: 3, turns: 1 } },
      ],
    });

    await rt.waitUntilReady(["triage:main"], { timeoutMs: 5000 });

    assert.equal(waitCalls(), 1);
    assert.equal("after" in (calls[0]?.params ?? {}), false);
    assert.equal(inspectCalls(), 0);
  });

  it("waitUntilReady names the identities that did not become ready", async () => {
    const { rt } = await makeRuntime({
      inspections: [
        { output_preview: null, completion_cursor: { epoch: 3, turns: 0 } },
      ],
    });

    await assert.rejects(
      rt.waitUntilReady(["b:1", "a:1"], { timeoutMs: 20 }),
      /did not become ready within 20ms: a:1, b:1/,
    );
  });

  it("waitUntilReady falls back to the preview for a live alias", async () => {
    const { rt, inspectCalls, waitCalls } = await makeRuntime({
      inspections: [
        { output_preview: null, completion_cursor: null },
        { output_preview: "hi", completion_cursor: null },
      ],
    });

    await rt.waitUntilReady(["live:alias"], {
      timeoutMs: 5000,
      pollIntervalMs: 1,
    });

    assert.equal(waitCalls(), 1);
    assert.equal(inspectCalls(), 1);
  });

  it("completionCursor reads null as untracked", async () => {
    const { rt, calls } = await makeRuntime({
      inspections: [{ output_preview: null, completion_cursor: null }],
    });

    assert.equal(await rt.completionCursor("live:alias"), null);
    assert.deepEqual(calls.at(-1)?.params, { identity: "live:alias" });
  });

  it("falls back to polling on a gateway without server waits", async () => {
    const { rt } = await makeRuntime({
      legacyGateway: true,
      inspections: [
        { output_preview: "ACK", completion_cursor: { epoch: 7, turns: 1 } },
        { output_preview: "ACK", completion_cursor: { epoch: 7, turns: 2 } },
      ],
    });

    const output = await rt.waitForCompletion(
      "triage:main",
      { epoch: 7, turns: 1 },
      { timeoutMs: 5000, pollIntervalMs: 1 },
    );

    assert.equal(output, "ACK");
    assert.deepEqual(await rt.completionCursor("triage:main"), {
      epoch: 7,
      turns: 2,
    });
  });
});

describe("typed wait outcomes", () => {
  for (const outcome of [
    "run_failed",
    "broken",
    "retiring",
    "identity_gone",
    "shutting_down",
  ]) {
    it(`waitForCompletion rejects with WaitEndedError on ${outcome}`, async () => {
      const { WaitEndedError } = await import("../src/errors.js");
      const { rt, inspectCalls } = await makeRuntime({ waitOutcome: outcome });
      await assert.rejects(
        rt.waitForCompletion("triage:main", { epoch: 3, turns: 1 }),
        (error) => {
          assert.ok(error instanceof WaitEndedError);
          assert.equal(error.outcome, outcome);
          assert.equal(error.identity, "triage:main");
          return true;
        },
      );
      assert.equal(inspectCalls(), 0);
    });
  }

  it("waitUntilReady names why an identity is not ready", async () => {
    const { rt } = await makeRuntime({ waitOutcome: "broken" });
    await assert.rejects(
      rt.waitUntilReady(["a:1"], { timeoutMs: 5000 }),
      /a:1 \(broken\)/,
    );
  });

  it("parses typed restore progress on the bootstrap status", async () => {
    const { parseIdentityBootstrapStatus } = await import("../src/types.js");
    const status = parseIdentityBootstrapStatus({
      mode: { mode: "eager_materialize" },
      complete: false,
      ready: false,
      counts: { active: 1, broken: 1 },
      identities: {
        "agent:resuming": { state: "warming", restore: { stage: "resuming" } },
        "agent:broken": {
          state: "broken",
          error: "resume refused",
          restore: { stage: "refused", code: -32051, kind: "identity_unresumable" },
        },
        "agent:older": { state: "active" },
      },
    });
    assert.deepEqual(status.identities["agent:resuming"]?.restore, {
      stage: "resuming",
      kind: null,
      code: null,
    });
    assert.deepEqual(status.identities["agent:broken"]?.restore, {
      stage: "refused",
      kind: "identity_unresumable",
      code: -32051,
    });
    assert.equal(status.identities["agent:older"]?.restore, null);
    assert.equal(status.complete, false);
    assert.equal(status.counts.broken, 1);
  });
});

// ---------------------------------------------------------------------------
// Cursor value semantics
// ---------------------------------------------------------------------------

describe("CompletionCursor", () => {
  it("classifies progress by cursor, not content", async () => {
    const { completionProgressSince } = await import("../src/types.js");
    const baseline: CompletionCursor = { epoch: 2, turns: 3 };

    assert.equal(completionProgressSince(baseline, baseline), "pending");
    assert.equal(
      completionProgressSince({ epoch: 2, turns: 4 }, baseline),
      "completed",
    );
    assert.equal(
      completionProgressSince({ epoch: 3, turns: 0 }, baseline),
      "incarnation_changed",
    );
  });

  it("round-trips", async () => {
    const { parseCompletionCursor, completionCursorToDict } = await import(
      "../src/types.js"
    );
    const cursor = parseCompletionCursor({ epoch: 12, turns: 34 });
    assert.deepEqual(cursor, { epoch: 12, turns: 34 });
    assert.deepEqual(completionCursorToDict(cursor), { epoch: 12, turns: 34 });
  });
});

// ---------------------------------------------------------------------------
// Wire mirrors: both directions, both fields
// ---------------------------------------------------------------------------

describe("model mirrors carry the cursor in both directions", () => {
  it("IdentityInspection", async () => {
    const { parseIdentityInspection, identityInspectionToDict } = await import(
      "../src/types.js"
    );
    const payload = {
      identity: "triage:main",
      output_preview: "ACK",
      is_final: false,
      peer_reachable_count: 0,
      intent_managed: false,
      completion_cursor: { epoch: 7, turns: 2 },
    };

    const parsed = parseIdentityInspection(payload);
    assert.deepEqual(parsed.completionCursor, { epoch: 7, turns: 2 });
    assert.deepEqual(identityInspectionToDict(parsed), payload);
    assert.deepEqual(
      parseIdentityInspection(identityInspectionToDict(parsed)),
      parsed,
    );
  });

  it("IdentityInspection preview_unavailable", async () => {
    const { parseIdentityInspection, identityInspectionToDict } = await import(
      "../src/types.js"
    );
    const payload = {
      identity: "triage:main",
      is_final: false,
      peer_reachable_count: 0,
      intent_managed: false,
      preview_unavailable: "observation_deadline",
    };

    const parsed = parseIdentityInspection(payload);
    assert.equal(parsed.outputPreview, null);
    assert.equal(parsed.previewUnavailable, "observation_deadline");
    assert.deepEqual(identityInspectionToDict(parsed), payload);
    // An observed preview carries no marker, and a gateway predating the
    // field (or a null marker) reads as observed.
    const observed = parseIdentityInspection({
      identity: "triage:main",
      output_preview: "ACK",
      preview_unavailable: null,
    });
    assert.equal(observed.previewUnavailable, null);
    assert.equal("preview_unavailable" in identityInspectionToDict(observed), false);
    // Unknown future reasons pass through.
    assert.equal(
      parseIdentityInspection({
        identity: "triage:main",
        preview_unavailable: "some_future_reason",
      }).previewUnavailable,
      "some_future_reason",
    );
  });

  it("DispatchResult", async () => {
    const { parseDispatchResult, dispatchResultToDict } = await import(
      "../src/types.js"
    );
    const payload = {
      fencing_token: 4,
      durable: true,
      completion_baseline: { epoch: 4, turns: 5 },
    };

    const parsed = parseDispatchResult(payload);
    assert.deepEqual(parsed.completionBaseline, { epoch: 4, turns: 5 });
    assert.deepEqual(dispatchResultToDict(parsed), payload);
    assert.deepEqual(parseDispatchResult(dispatchResultToDict(parsed)), parsed);
  });

  it("SendResult", async () => {
    const { parseSendResult, sendResultToDict } = await import(
      "../src/types.js"
    );
    const payload = {
      fencing_token: 4,
      completion_baseline: { epoch: 4, turns: 5 },
    };

    const parsed = parseSendResult(payload);
    assert.deepEqual(parsed.completionBaseline, { epoch: 4, turns: 5 });
    assert.deepEqual(sendResultToDict(parsed), payload);
    assert.deepEqual(parseSendResult(sendResultToDict(parsed)), parsed);
  });

  it("payloads without the field still parse, and absence stays null", async () => {
    const { parseIdentityInspection, parseDispatchResult, parseSendResult } =
      await import("../src/types.js");

    const inspection = parseIdentityInspection({
      identity: "triage:main",
      output_preview: "ACK",
      is_final: true,
    });
    assert.equal(inspection.completionCursor, null);
    assert.equal(inspection.outputPreview, "ACK");
    assert.equal(inspection.isFinal, true);

    assert.equal(
      parseDispatchResult({ fencing_token: 2, durable: false })
        .completionBaseline,
      null,
    );
    assert.equal(parseSendResult({ fencing_token: 2 }).completionBaseline, null);
  });

  it("an explicit null cursor reads as absent, not as zero turns", async () => {
    const { parseIdentityInspection } = await import("../src/types.js");
    const inspection = parseIdentityInspection({
      identity: "live:alias",
      completion_cursor: null,
    });
    assert.equal(inspection.completionCursor, null);
  });
});
