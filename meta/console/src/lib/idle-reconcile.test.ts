import assert from "node:assert/strict";
import test from "node:test";
import { ConsoleActivityProjection, resolvePanelResponsePhase } from "@console-core";
import type { ConsoleAgent, ConsoleFrame } from "../types";
import {
  IDLE_SETTLE_MIN_GAP_MS,
  agentReportsIdle,
  decideIdleSettlement,
  framesAfterIdleWatermark,
  idleSettleClock,
  idleWatermarkFor,
  isAfterIdleWatermark,
} from "./idle-reconcile";

const scope = { runtimeKey: "runtime", identity: "original:main", sessionId: "session" };
let sequence = 0;
function frame(event: string, timestampMs: number, extra: Partial<ConsoleFrame> = {}): ConsoleFrame {
  sequence += 1;
  return {
    ...scope,
    id: `${event}:${sequence}`,
    cursor: `console:${sequence}`,
    event,
    sourceKind: "console_event",
    timestampMs,
    data: {},
    ...extra,
  } as ConsoleFrame;
}

function fold(frames: ConsoleFrame[]): ConsoleActivityProjection {
  const projection = new ConsoleActivityProjection();
  for (const row of frames) projection.fold(row);
  return projection;
}

/// The user's turn stops at a host callback, and a runtime-started
/// continuation finishes the work under its own lineage: the paused
/// interaction never sees a terminal of its own.
function callbackPauseThenContinuation(): ConsoleFrame[] {
  return [
    frame("user_input", 1000, { interactionId: "I1", status: "delivered" }),
    frame("run_started", 1001, { interactionId: "I1", runId: "R1" }),
    frame("tool_call_requested", 1002, { interactionId: "I1", runId: "R1" }),
    frame("turn_completed", 1003, { interactionId: "I1", runId: "R1", data: { stop_reason: "tool_use" } }),
    frame("interaction_callback_pending", 1004, { interactionId: "I1" }),
    frame("run_started", 2000, { runId: "R2" }),
    frame("text_delta", 2001, { runId: "R2" }),
    frame("text_complete", 2002, { runId: "R2" }),
    frame("turn_completed", 2003, { runId: "R2", data: { stop_reason: "end_turn" } }),
    frame("interaction_complete", 2004, { runId: "R2" }),
  ];
}

/// The user's turn settles, then a frame with no lifecycle ids (the approved
/// setup's tool completion) arrives before the continuation runs.
function idlessFrameThenContinuation(): ConsoleFrame[] {
  return [
    frame("user_input", 1000, { interactionId: "I1", status: "delivered" }),
    frame("run_started", 1001, { interactionId: "I1", runId: "R1" }),
    frame("text_complete", 1002, { interactionId: "I1", runId: "R1" }),
    frame("turn_completed", 1003, { interactionId: "I1", runId: "R1", data: { stop_reason: "end_turn" } }),
    frame("interaction_complete", 1004, { interactionId: "I1", runId: "R1" }),
    frame("tool_execution_completed", 1500),
    frame("run_started", 2000, { runId: "R2" }),
    frame("text_delta", 2001, { runId: "R2" }),
    frame("turn_completed", 2003, { runId: "R2", data: { stop_reason: "end_turn" } }),
    frame("interaction_complete", 2004, { runId: "R2" }),
  ];
}

const idleAgent: Pick<ConsoleAgent, "response_phase" | "progress"> = {
  response_phase: null,
  progress: { run_state: "idle", in_flight_work: 0, last_progress_at_ms: 0, last_progress_event: "", health: "ok" },
};

for (const [name, build] of [
  ["a paused interaction a continuation finished", callbackPauseThenContinuation],
  ["an anonymous owner opened by an id-less frame", idlessFrameThenContinuation],
] as const) {
  test(`${name} holds the fold open until the idle watermark settles it`, () => {
    const frames = build();
    const stuck = fold(frames);
    assert.equal(stuck.busy, true, "the frames alone leave an owner open: the stale 'working'");
    assert.notEqual(
      resolvePanelResponsePhase({ frames, serverPhase: null }),
      null,
      "the chat reads 'working' although the server is idle",
    );

    const watermark = idleWatermarkFor(frames);
    assert.equal(watermark?.timestampMs, 2004);
    const settled = framesAfterIdleWatermark(frames, watermark);
    assert.deepEqual(settled, []);
    assert.equal(fold(settled).busy, false);
    assert.equal(resolvePanelResponsePhase({ frames: settled, serverPhase: null }), null);
  });

  test(`${name}: a rebuild of the fold after reconciliation stays idle`, () => {
    const frames = build();
    const watermark = idleWatermarkFor(frames);
    // A full replay reads the whole log again, including a late frame that
    // still belongs to the settled turn: the watermark is applied again.
    const replayed = [...frames, frame("tool_execution_completed", 1600, { interactionId: "I1", runId: "R1" })];
    const projection = fold(framesAfterIdleWatermark(replayed, watermark));
    assert.equal(projection.busy, false);
    assert.equal(projection.phase, null);
  });
}

test("work that starts after the idle watermark is busy again", () => {
  const frames = callbackPauseThenContinuation();
  const watermark = idleWatermarkFor(frames);
  const next = [
    ...frames,
    frame("user_input", 3000, { interactionId: "I2", status: "delivered" }),
    frame("run_started", 3001, { interactionId: "I2", runId: "R3" }),
  ];
  const projection = fold(framesAfterIdleWatermark(next, watermark));
  assert.equal(projection.busy, true);
  assert.equal(projection.phase, "waiting");
  projection.fold(frame("interaction_complete", 3005, { interactionId: "I2", runId: "R3" }));
  assert.equal(projection.busy, false);
});

test("a frame the console had when it settled is settled; a later one is read, even in the same millisecond", () => {
  const frames = callbackPauseThenContinuation();
  const watermark = idleWatermarkFor(frames);
  assert.equal(watermark?.timestampMs, 2004);
  for (const seen of frames) assert.equal(isAfterIdleWatermark(seen, watermark), false);
  assert.equal(isAfterIdleWatermark(frame("run_started", 2004, { runId: "R9" }), watermark), true,
    "a new frame stamped in the watermark's millisecond is read");
  assert.equal(isAfterIdleWatermark(frame("run_started", 2005, { runId: "R9" }), watermark), true);
  assert.equal(isAfterIdleWatermark(frame("tool_execution_completed", 1500), watermark), false,
    "a late frame stamped before the watermark belongs to the settled turn");
  assert.equal(isAfterIdleWatermark({ ...frame("run_started", 0), timestampMs: undefined }, watermark), true,
    "a new frame without a timestamp is read");
  assert.equal(isAfterIdleWatermark(frame("run_started", 1), undefined), true);
});

test("a skewed browser clock does not hide a new server frame", () => {
  // A frame the console mints itself carries the browser's clock, here an
  // hour ahead of the server's.
  const frames = [
    ...callbackPauseThenContinuation(),
    frame("workgraph_card", 2004 + 3_600_000, { sourceKind: "console-local" }),
  ];
  const watermark = idleWatermarkFor(frames);
  assert.equal(watermark?.timestampMs, 2004, "only server timestamps set the watermark");
  const next = frame("run_started", 2100, { interactionId: "I2", runId: "R3" });
  assert.equal(isAfterIdleWatermark(next, watermark), true);
  assert.equal(fold(framesAfterIdleWatermark([...frames, next], watermark)).busy, true);
});

test("no frames, no watermark", () => {
  assert.equal(idleWatermarkFor([]), undefined);
});

test("without a watermark the frames pass through unchanged", () => {
  const frames = callbackPauseThenContinuation();
  assert.equal(framesAfterIdleWatermark(frames, undefined), frames);
});

test("only an idle status poll settles: response phase null and no open run", () => {
  assert.equal(agentReportsIdle(idleAgent), true);
  assert.equal(agentReportsIdle({ ...idleAgent, response_phase: "waiting" }), false);
  assert.equal(agentReportsIdle({ ...idleAgent, response_phase: undefined }), false, "a silent server is not idle");
  assert.equal(agentReportsIdle({ ...idleAgent, progress: { ...idleAgent.progress!, run_state: "run_open" } }), false);
  assert.equal(agentReportsIdle({ response_phase: null }), false, "no run state is not idle");
});

test("two idle polls a poll interval apart settle; every guard resets the count", () => {
  const base = {
    agent: idleAgent,
    logVersionAtRequest: 7,
    logVersionNow: 7,
    sendInFlight: false,
    foldBusy: true,
    foldPhase: "waiting",
    armed: undefined as { version: number; atMs: number } | undefined,
    nowMs: 100_000,
  };
  const armed = { version: 7, atMs: 100_000 - IDLE_SETTLE_MIN_GAP_MS };
  assert.equal(decideIdleSettlement(base), "arm", "the first idle poll changes nothing");
  assert.equal(decideIdleSettlement({ ...base, armed }), "settle", "an idle poll a poll interval later, with no frame since");
  assert.equal(decideIdleSettlement({ ...base, armed: { version: 7, atMs: 100_000 - 150 } }), "wait",
    "two idle polls 150 ms apart (a poll after another member's event) do not settle");
  assert.equal(decideIdleSettlement({ ...base, armed: { version: 6, atMs: 0 } }), "arm",
    "a frame since the arming poll starts the count again");
  for (const [agent, why] of [
    [{ ...idleAgent, response_phase: "generating" as const }, "a response phase"],
    [{ ...idleAgent, progress: { ...idleAgent.progress!, run_state: "run_open" } }, "an open run"],
  ] as const) {
    assert.equal(decideIdleSettlement({ ...base, agent, armed }), "reset",
      `never while the server reports a live run: ${why}`);
  }
  assert.equal(decideIdleSettlement({ ...base, logVersionNow: 8, armed: { version: 8, atMs: 0 } }), "reset",
    "never when a frame arrived while the poll was in flight");
  assert.equal(decideIdleSettlement({ ...base, logVersionAtRequest: undefined, armed }), "reset",
    "never for a log the poll did not cover");
  assert.equal(decideIdleSettlement({ ...base, sendInFlight: true, armed }), "reset",
    "never while this console's own send is in flight");
  assert.equal(decideIdleSettlement({ ...base, foldBusy: false, foldPhase: null, armed }), "reset",
    "nothing to settle");
  assert.equal(decideIdleSettlement({ ...base, foldBusy: false, foldPhase: "waiting", armed }), "settle",
    "a stale phase alone is settled too");
});

test("the gap between the two idle polls is measured on the monotonic clock, not the wall clock", () => {
  const ownPerformanceNow = Object.getOwnPropertyDescriptor(performance, "now");
  const realDateNow = Date.now;
  let wallMs = 1_700_000_000_000;
  let monotonicMs = 50_000;
  Date.now = () => wallMs;
  Object.defineProperty(performance, "now", { configurable: true, writable: true, value: () => monotonicMs });
  try {
    const base = {
      agent: idleAgent,
      logVersionAtRequest: 7,
      logVersionNow: 7,
      sendInFlight: false,
      foldBusy: true,
      foldPhase: "waiting",
    };
    const armed = { version: 7, atMs: idleSettleClock.now() };
    wallMs += 3_600_000;
    monotonicMs += 2_000;
    assert.equal(decideIdleSettlement({ ...base, armed, nowMs: idleSettleClock.now() }), "wait",
      "the computer's clock set an hour ahead does not settle two polls 2 s apart");
    wallMs -= 7_200_000;
    monotonicMs += IDLE_SETTLE_MIN_GAP_MS;
    assert.equal(decideIdleSettlement({ ...base, armed, nowMs: idleSettleClock.now() }), "settle",
      "the computer's clock set back does not hold the member open past a poll interval");
  } finally {
    Date.now = realDateNow;
    if (ownPerformanceNow) Object.defineProperty(performance, "now", ownPerformanceNow);
    else delete (performance as { now?: unknown }).now;
  }
});
