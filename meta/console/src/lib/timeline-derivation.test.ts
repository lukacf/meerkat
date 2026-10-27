import assert from "node:assert/strict";
import test from "node:test";

import { RealisticTimeline, assistantReply, type WireFrame } from "../perf/realistic-transcript";
import { deriveTimelineEntries, timelineDerivationStats, type TimelineDerivation, type TimelineDerivationOptions } from "./adapters";
import { createIdentityLogCore, pushFrame, sortedEvents } from "./identity-log";
import { parseSseFrames } from "./network";
import type { ConsoleAgent, ConsoleFrame } from "../types";
import { reconcileRuntimeAppendFrames } from "../../../packages/console-core/src/runtime-append-projection";

const agent = { identity: "router:main", label: "Router" } as unknown as ConsoleAgent;
const options: TimelineDerivationOptions = { renderInteractionStartsAsUser: true, renderTextDeltas: true };

function consoleFrames(frames: WireFrame[]): ConsoleFrame[] {
  return parseSseFrames(frames.map((frame) => `event: frame\ndata: ${JSON.stringify({ type: "frame", frame })}\n\n`).join(""));
}

test("a streamed reply over a long realistic log extends the previous derivation for every text chunk", () => {
  const timeline = new RealisticTimeline(40);
  const log = createIdentityLogCore();
  for (const frame of consoleFrames(timeline.recent(2_000).frames)) pushFrame(log, frame.id, frame);
  let derivation: TimelineDerivation = deriveTimelineEntries(agent, sortedEvents(log), options);
  const stream = consoleFrames(timeline.streamReply(assistantReply(3) + assistantReply(4), 9));
  let deltas = 0;
  let extended = 0;
  for (const frame of stream) {
    pushFrame(log, frame.id, frame);
    const previous = derivation;
    const resumable = previous.resume !== null;
    derivation = deriveTimelineEntries(agent, sortedEvents(log), options, previous);
    if (frame.event === "text_delta") deltas += 1;
    if (resumable && previous.resume === null) extended += 1;
    assert.deepEqual(derivation.entries, deriveTimelineEntries(agent, sortedEvents(log).slice(), options).entries);
  }
  // Every text chunk continues the fold; the run and turn starts do not.
  assert.ok(deltas > 50, `stream has ${deltas} chunks`);
  assert.equal(extended, deltas);
  const last = derivation.entries.at(-1);
  assert.equal(last?.kind, "message");
});

/// Stream a reply into a log built from `history`, deriving incrementally and
/// checking every step against a full derivation.
function streamOver(timeline: RealisticTimeline, history: WireFrame[], reply: WireFrame[]) {
  const log = createIdentityLogCore();
  for (const frame of consoleFrames(history)) pushFrame(log, frame.id, frame);
  let derivation: TimelineDerivation = deriveTimelineEntries(agent, sortedEvents(log), options);
  let deltas = 0;
  let extended = 0;
  for (const frame of consoleFrames(reply)) {
    pushFrame(log, frame.id, frame);
    const previous = derivation;
    const resumable = previous.resume !== null;
    derivation = deriveTimelineEntries(agent, sortedEvents(log), options, previous);
    if (frame.event === "text_delta") deltas += 1;
    if (resumable && previous.resume === null) extended += 1;
    assert.deepEqual(derivation.entries, deriveTimelineEntries(agent, sortedEvents(log).slice(), options).entries);
  }
  return { derivation, deltas, extended };
}

test("a reply streamed after a gateway restart extends the derivation for every text chunk", () => {
  // The previous process published the history up to source sequence ~1,900;
  // this process numbers the member's stream from 1 again, in a new epoch.
  const timeline = new RealisticTimeline(40, { restarted: true });
  const history = timeline.recent(2_000).frames;
  const reply = timeline.streamReply(assistantReply(3) + assistantReply(4), 9);
  const historyMax = Math.max(...history.map((frame) => Number((frame.payload as Record<string, unknown>).source_sequence ?? 0)));
  assert.ok(historyMax > 500, `history reaches sequence ${historyMax}`);
  assert.equal((reply.find((frame) => frame.kind === "text_delta")!.payload as Record<string, unknown>).source_sequence, 3);
  const { derivation, deltas, extended } = streamOver(timeline, history, reply);
  assert.ok(deltas > 50, `stream has ${deltas} chunks`);
  assert.equal(extended, deltas, "every chunk continues the fold instead of re-deriving the log");
  // Arrival orders the new epoch after the old one: the reply is last.
  const last = derivation.entries.at(-1)!;
  assert.equal(last.kind, "message");
  assert.match(JSON.stringify(last), /retry policy|deployment notes|Findings/i);
});

test("without stream epochs a restarted sequence still derives correctly, but only in full", () => {
  // Older stores carry no epoch: their sequences stay session-scoped, so the
  // restarted ones cannot be proven to append and each chunk re-derives.
  const timeline = new RealisticTimeline(20, { restarted: true });
  const strip = (frames: WireFrame[]) => frames.map((frame) => {
    const { source_epoch: _epoch, ...payload } = frame.payload as Record<string, unknown>;
    return { ...frame, payload };
  });
  const { deltas, extended } = streamOver(timeline, strip(timeline.recent(2_000).frames), strip(timeline.streamReply(assistantReply(5), 12)));
  assert.ok(deltas > 20);
  assert.equal(extended, 0);
});

test("a streamed text chunk presents only the open reply, not the transcript", () => {
  const timeline = new RealisticTimeline(40);
  const log = createIdentityLogCore();
  for (const frame of consoleFrames(timeline.recent(2_000).frames)) pushFrame(log, frame.id, frame);
  let derivation: TimelineDerivation = deriveTimelineEntries(agent, sortedEvents(log), options);
  const settled = derivation.entries.length;
  let chunks = 0;
  let presented = 0;
  for (const frame of consoleFrames(timeline.streamReply(assistantReply(3) + assistantReply(4), 9))) {
    pushFrame(log, frame.id, frame);
    const before = timelineDerivationStats.presented;
    derivation = deriveTimelineEntries(agent, sortedEvents(log), options, derivation);
    if (frame.event !== "text_delta") continue;
    chunks += 1;
    presented += timelineDerivationStats.presented - before;
  }
  // Each chunk re-presented all of the settled transcript before.
  assert.ok(settled > 100, `${settled} settled entries`);
  assert.ok(chunks > 50, `stream has ${chunks} chunks`);
  assert.ok(presented / chunks <= 1.05, `${(presented / chunks).toFixed(2)} entries presented per chunk`);
});

test("an unchanged entry keeps its object identity across derivations", () => {
  const timeline = new RealisticTimeline(12);
  const log = createIdentityLogCore();
  for (const frame of consoleFrames(timeline.recent(600).frames)) pushFrame(log, frame.id, frame);
  const first = deriveTimelineEntries(agent, sortedEvents(log), options);
  const stream = consoleFrames(timeline.streamReply(assistantReply(1), 16));
  let derivation = first;
  for (const frame of stream) {
    pushFrame(log, frame.id, frame);
    derivation = deriveTimelineEntries(agent, sortedEvents(log), options, derivation);
  }
  const unchanged = first.entries.filter((entry) => derivation.entries.includes(entry));
  // All settled history entries are reused; only the new reply is new.
  assert.equal(unchanged.length, first.entries.length);
});

// Seeded fuzz over the frame vocabulary the global passes care about.
function rng(seed: number) {
  let state = seed >>> 0;
  return () => {
    state = (state * 1664525 + 1013904223) >>> 0;
    return state / 4294967296;
  };
}

export function fuzzFrames(seed: number, count: number): ConsoleFrame[] {
  const random = rng(seed);
  const pick = <T,>(items: readonly T[]): T => items[Math.floor(random() * items.length)];
  const frames: ConsoleFrame[] = [];
  let cursor = 1;
  let ts = 1_700_000_000_000;
  const sessions = ["s-1", "s-2"];
  for (let i = 0; i < count; i++) {
    const turn = Math.floor(i / 7);
    const session = random() < 0.85 ? sessions[0] : sessions[1];
    const run = `run-${turn % 4}`;
    const interaction = `${"0".repeat(8)}-0000-0000-0000-${String(turn % 5).padStart(12, "0")}`;
    const message = random() < 0.7 ? `m-${turn % 3}` : undefined;
    const kind = pick([
      "text_delta", "text_delta", "text_delta", "text_delta", "reasoning_delta", "reasoning_complete",
      "user_input", "interaction_started", "run_started", "turn_started", "turn_completed",
      "tool_call_requested", "tool_execution_completed", "text_complete", "interaction_complete", "run_completed",
      "history_text", "history_user", "assistant_history_snapshot", "compaction_started", "compaction_completed",
      "system_notice", "retrying",
    ] as const);
    const roll = random();
    ts += roll < 0.4 ? 0 : roll < 0.9 ? Math.floor(random() * 2_000) : -Math.floor(random() * 3_000);
    const timestampMs = random() < 0.08 ? undefined : ts;
    const frameCursor = random() < 0.05 ? undefined : `console:${random() < 0.9 ? cursor++ : Math.max(1, cursor - 3)}`;
    const base = {
      id: `f-${seed}-${i}`,
      identity: "router:main",
      runtimeKey: "default",
      sessionId: session,
      cursor: frameCursor,
      timestampMs,
      interactionId: random() < 0.9 ? interaction : undefined,
      runId: random() < 0.8 ? run : undefined,
    };
    const sequence = random() < 0.85 ? i : Math.max(0, i - 4);
    const live = { ...base, sourceKind: "console_event" as const };
    const carrier = message ? { assistant_message_id: message } : {};
    switch (kind) {
      case "text_delta":
        frames.push({ ...live, event: "text_delta", data: { ...carrier, delta: pick(["Hello ", "world. ", "- item\n", "`code` ", "**bold** "]), source_sequence: sequence } });
        break;
      case "reasoning_delta":
        frames.push({ ...live, event: "reasoning_delta", data: { ...carrier, delta: "thinking ", source_sequence: sequence } });
        break;
      case "reasoning_complete":
        frames.push({ ...live, event: "reasoning_complete", data: { ...carrier, content: "thinking done", source_sequence: sequence } });
        break;
      case "user_input":
      case "interaction_started":
        frames.push({ ...base, sourceKind: pick(["send", "console_event"]), event: kind, data: { content: `question ${turn}` } });
        break;
      case "run_started":
      case "turn_started":
      case "turn_completed":
      case "retrying":
        frames.push({ ...live, event: kind, data: { ...carrier, source_sequence: sequence, input: { content: `question ${turn}`, kind: "content" } } });
        break;
      case "tool_call_requested":
        frames.push({ ...live, event: kind, data: { id: `call-${turn}`, name: "read_file", args: { path: "a" }, source_sequence: sequence } });
        break;
      case "tool_execution_completed":
        frames.push({ ...live, event: kind, data: { id: `call-${turn}`, name: "read_file", is_error: false, result: "ok", source_sequence: sequence } });
        break;
      case "text_complete":
        frames.push({ ...live, event: kind, data: { ...carrier, content: "Hello world. ", source_sequence: sequence } });
        break;
      case "interaction_complete":
      case "run_completed":
        frames.push({ ...live, event: kind, data: { ...carrier, result: "Hello world. ", type: "run_completed", source_sequence: sequence } });
        break;
      case "history_text":
        frames.push({ ...base, sourceKind: "session_history", event: "text_complete", data: { ...carrier, message: { ...carrier, role: "block_assistant", blocks: [{ block_type: "text", data: { text: "Hello world. " } }] }, text: "Hello world. ", result: "Hello world. " } });
        break;
      case "history_user":
        frames.push({ ...base, sourceKind: "session_history", event: "user_input", data: { content: [{ type: "text", text: `question ${turn}` }], message: { role: "user", content: `question ${turn}` } } });
        break;
      case "assistant_history_snapshot":
        frames.push({ ...base, interactionId: undefined, runId: undefined, sourceKind: "session_history", event: kind, cursor: `console:${cursor++}`, data: { assistant_message_ids: ["m-0", "m-1"].slice(0, 1 + Math.floor(random() * 2)), complete: true, observed_through: `console:${Math.max(1, cursor - 2)}`, session_id: session } });
        break;
      case "compaction_started":
      case "compaction_completed":
        frames.push({ ...live, event: kind, data: { source_sequence: sequence } });
        break;
      case "system_notice":
        frames.push({ ...base, sourceKind: "console_event", event: kind, data: { message: { role: "system_notice", body: "Peer message from analyst: status ok" } } });
        break;
    }
  }
  return frames;
}

test("incremental extension equals full derivation over seeded fuzzed frame sequences", () => {
  let extended = 0;
  let checks = 0;
  for (let seed = 1; seed <= 250; seed++) {
    const frames = fuzzFrames(seed, 18 + (seed % 23));
    for (const variant of [options, { ...options, renderInteractionStartsAsUser: false }]) {
      let derivation = deriveTimelineEntries(agent, frames.slice(0, 1), variant);
      for (let k = 2; k <= frames.length; k++) {
        const previous = derivation;
        const resumable = previous.resume !== null;
        derivation = deriveTimelineEntries(agent, frames.slice(0, k), variant, previous);
        if (resumable && previous.resume === null) extended += 1;
        assert.deepEqual(derivation.entries, deriveTimelineEntries(agent, frames.slice(0, k), variant).entries, `seed ${seed} at ${k}`);
        checks += 1;
      }
    }
  }
  assert.ok(extended > 200, `incremental path exercised ${extended} times in ${checks} checks`);
});

test("source sequences order frames within one stream epoch, never across epochs", () => {
  const frame = (id: string, cursor: number, sequence: number, epoch?: string): ConsoleFrame => ({
    id, event: "text_delta", identity: "router:main", cursor: `console:${cursor}`, timestampMs: 1_000 + cursor,
    runtimeKey: "default", sessionId: "01a0f7d0-4be7-7d02-bce5-b29fc7d82249", sourceKind: "console_event",
    data: { delta: id, source_sequence: sequence, ...(epoch ? { source_epoch: epoch } : {}) },
  } as unknown as ConsoleFrame);
  const ids = (frames: ConsoleFrame[]) => reconcileRuntimeAppendFrames(frames).map((f) => f.id);
  // Within an epoch, sequence repairs arrival order.
  assert.deepEqual(ids([frame("b", 1, 2, "p.0.1"), frame("a", 2, 1, "p.0.1")]), ["a", "b"]);
  // A restarted stream (new epoch) is not interleaved into the previous one.
  const restarted = [frame("old-1", 1, 100, "p.0.1"), frame("old-2", 2, 101, "p.0.1"), frame("new-1", 3, 1, "q.0.1"), frame("new-2", 4, 2, "q.0.1")];
  assert.deepEqual(ids(restarted), ["old-1", "old-2", "new-1", "new-2"]);
  // Older frames without an epoch keep the session-wide comparison.
  const legacy = restarted.map((f) => ({ ...f, data: { ...(f.data as Record<string, unknown>), source_epoch: undefined } }));
  assert.deepEqual(ids(legacy), ["new-1", "new-2", "old-1", "old-2"]);
});

test("extending a derivation never moves the boundary that holds older notices back", () => {
  const session = "01a0f7d0-4be7-7d02-bce5-b29fc7d82249";
  const frame = (id: string, event: string, cursor: number, timestampMs: number, data: Record<string, unknown>,
    extra: Partial<ConsoleFrame> = {}): ConsoleFrame => ({
    id, event, identity: "router:main", runtimeKey: "default", sessionId: session, cursor: `console:${cursor}`,
    timestampMs, sourceKind: "console_event", data, ...extra,
  });
  const loaded = [
    frame("asked", "user_input", 10, 9_000, { content: "Is the export done?" }),
    frame("image", "runtime_notice_snapshot", 11, 9_200, {
      session_id: session, complete: true, observed_through: "console:10", settled_attempts: [],
      notices: [{ offset: 1, message: { role: "system_notice", kind: "generic", body: "Export finished.",
        created_at: new Date(7_000).toISOString(),
        runtime_origin: { session_id: session, run_id: "run-held", input_id: "input-held", append_ordinal: 0 } } }],
    }, { sourceKind: "session_history" }),
  ];
  // The newest cursor, stamped before every loaded frame. In store order
  // nothing else keeps this chunk from extending the previous fold.
  const late = frame("late", "text_delta", 12, 6_000, { delta: "Checking." }, { runId: "run-late" });
  const paged: TimelineDerivationOptions = { renderTextDeltas: true, hasOlderHistory: true };
  const shows = (derivation: TimelineDerivation) => JSON.stringify(derivation.entries).includes("Export finished.");
  const previous = deriveTimelineEntries(agent, loaded, paged);
  assert.equal(shows(previous), false, "the notice predates every loaded frame");
  const full = deriveTimelineEntries(agent, [...loaded, late], paged);
  assert.equal(shows(full), true, "the older chunk extends the loaded range past the notice");
  assert.deepEqual(deriveTimelineEntries(agent, [...loaded, late], paged, previous).entries, full.entries);
});
