import assert from "node:assert/strict";
import test from "node:test";
import { mapFramesToTimelineEntries as stock } from "./adapters";
import { mapFramesToTimelineEntries as shared } from "../../../packages/console-core/src/adapters";
import type { ConsoleFrame } from "../../../packages/console-core/src/runtime-types";

const agent = { agent_id: "worker", member_id: "worker", identity: "worker", label: "Worker", kind: "mob_agent" };
const A = "01920000-0000-7000-8000-000000000001";
const B = "01920000-0000-7000-8000-000000000002";
const C = "01920000-0000-7000-8000-000000000003";
function frame(id: string, event: string, data: Record<string, unknown>, extra: Partial<ConsoleFrame> = {}): ConsoleFrame {
  return { id, event, identity: "worker", runtimeKey: "runtime", sessionId: "session", runId: "run", interactionId: "interaction", sourceKind: "console_event", timestampMs: Number(id.replace(/\D/g, "")) || 10, data, ...extra };
}
function live(id: string, event: string, assistant: unknown, data: Record<string, unknown> = {}, extra: Partial<ConsoleFrame> = {}) {
  return frame(id, event, { ...data, assistant_message_id: assistant }, extra);
}
function history(id: string, assistant: unknown, text: string, extra: Partial<ConsoleFrame> = {}, blocks: unknown[] = [{ block_type: "text", data: { text } }]) {
  return live(id, "text_complete", assistant, { text, result: text, message: { role: "block_assistant", blocks, stop_reason: "end_turn" } }, { sourceKind: "session_history", ...extra });
}
function text(entry: any): string {
  return entry.copyText ?? entry.text ?? (entry.blocks ?? []).map((b: any) => b.type === "markdown" ? b.source : b.type === "thinking" ? "" : b.text ?? "").join("");
}
function assistantRows(entries: any[]) { return entries.filter(e => e.kind === "message" && e.identity.id === "worker"); }
function snapshot(id: string, ids: string[], observed = 50, extra: Partial<ConsoleFrame> = {}) {
  return frame(id, "assistant_history_snapshot", { session_id: "session", complete: true, observed_through: `console:${observed}`, assistant_message_ids: ids }, { sourceKind: "session_history", cursor: `console:${observed + 1}`, ...extra });
}
function positions(id: string, rows: Array<[string, number]>, observed: number) {
  return frame(id, "runtime_notice_snapshot", { session_id: "session", complete: true, observed_through: `console:${observed}`, notices: [], settled_attempts: [], history_positions: rows.map(([frame_id, offset]) => ({ frame_id, source_cursor: `session:${offset}` })) }, { sourceKind: "session_history", cursor: `console:${observed + 2}` });
}
for (const [surface, map] of [["stock", stock], ["shared", shared]] as const) {
  const project = (frames: ConsoleFrame[]) => map(agent, frames, { textMode: "markdown" });
  test(`${surface}: one partial history page binds only its exact occurrence among identical answers`, () => {
    const events = [A, B, C].flatMap((id, i) => [live(`start-${i}`, "turn_started", id), live(`delta-${i}`, "text_delta", id, { delta: "Identical answer" }), live(`done-${i}`, "text_complete", id, { content: "Identical answer" }), live(`turn-${i}`, "turn_completed", id)]);
    const rows = assistantRows(project([...events, history("history-B", B, "Identical answer")]));
    assert.equal(rows.length, 3);
    assert.equal(rows.filter(row => row.id === "history-B").length, 1);
    assert.deepEqual(rows.map(text), ["Identical answer", "Identical answer", "Identical answer"]);
  });
  test(`${surface}: canonical changed text wins in either arrival order and late chunks cannot replace it`, () => {
    const saved = history("saved", A, "  Canonical A\u030a\n\nLast paragraph.  ");
    const delta = live("draft", "text_delta", A, { delta: "Discarded provider draft" });
    const late = live("late", "text_delta", A, { delta: "late stale bytes" });
    for (const frames of [[delta, saved, late], [saved, delta, late]]) {
      const rows = assistantRows(project(frames));
      assert.deepEqual(rows.map(row => [row.id, text(row)]), [["saved", "  Canonical A\u030a\n\nLast paragraph.  "]]);
    }
  });
  test(`${surface}: repeated open turn and retry clear all provisional content before replacements`, () => {
    for (const boundary of ["retrying", "turn_started"]) {
      const rows = assistantRows(project([
        live("start", "turn_started", A), live("draft", "text_delta", A, { delta: "stale text" }),
        live("thinking", "reasoning_delta", A, { delta: "stale thought" }),
        live("image", "assistant_image_appended", A, { image: { blob_id: "old", image_id: "old-image" } }),
        live("reset", boundary, A), live("fresh", "text_delta", A, { delta: "fresh text" }),
        live("fresh-thinking", "reasoning_complete", A, { content: "fresh thought" }),
      ]));
      const serialized = JSON.stringify(rows);
      assert(!serialized.includes("stale")); assert(!serialized.includes("old-image"));
      assert(serialized.includes("fresh text")); assert(serialized.includes("fresh thought"));
    }
  });
  test(`${surface}: same-run different IDs have separate text and reasoning streams`, () => {
    const rows = assistantRows(project([
      live("a", "text_delta", A, { delta: "First" }), live("b", "text_delta", B, { delta: "Second" }),
      live("ra", "reasoning_delta", A, { delta: "Think A" }), live("rb", "reasoning_complete", B, { content: "Think B" }),
    ]));
    assert.deepEqual(rows.map(text).filter(Boolean), ["First", "Second"]);
    assert.deepEqual(rows.flatMap(row => row.blocks ?? []).filter(b => b.type === "thinking").map(b => b.text), ["Think A", "Think B"]);
  });
  test(`${surface}: explicit result references earlier canonical row without rendering hook result`, () => {
    const saved = history("earlier-row", A, "Earlier canonical answer", { runId: "earlier-run" });
    const complete = live("result", "interaction_complete", A, { type: "run_completed", result: "Hook rewrote this result" });
    const rows = assistantRows(project([saved, complete, live("extract", "text_delta", B, { delta: "Extraction output" })]));
    assert.deepEqual(rows.map(row => [row.id, text(row)]), [["earlier-row", "Earlier canonical answer"], ["extract", "Extraction output"]]);
    assert.equal(rows[0].runId, "earlier-run");
  });
  test(`${surface}: result reference alone does not manufacture canonical text or bind a different ID`, () => {
    const rows = assistantRows(project([history("other", B, "Equal text"), live("result", "run_completed", A, { result: "Equal text" })]));
    assert.deepEqual(rows.map(row => [row.id, text(row)]), [["other", "Equal text"]]);
  });
  test(`${surface}: forks, runtime and identity conflicts never share occurrence correspondence`, () => {
    for (const conflict of [{ sessionId: "fork" }, { runtimeKey: "other-runtime" }, { identity: "other-agent" }]) {
      const rows = assistantRows(project([live("draft", "text_delta", A, { delta: "Draft" }), history("other", A, "Canonical", conflict)]));
      assert.deepEqual(rows.map(text), ["Draft", "Canonical"]);
    }
  });
  test(`${surface}: absent, malformed and unscoped carriers cannot text-match a known occurrence`, () => {
    for (const carrier of [undefined, null, "", "   ", 4, {}]) {
      const rows = assistantRows(project([live("delta", "text_delta", carrier, { delta: "Same" }), history("saved", A, "Same")]));
      assert.deepEqual(rows.map(row => [row.id, text(row)]), [["delta", "Same"], ["saved", "Same"]]);
    }
    const rows = assistantRows(project([live("delta", "text_delta", A, { delta: "Same" }, { sessionId: undefined }), history("saved", A, "Same")]));
    assert.equal(rows.length, 2);
  });
  test(`${surface}: canonical rich-only and interleaved blocks replace provisional siblings in source order`, () => {
    const blocks = [
      { block_type: "text", data: { text: "Before" } },
      { block_type: "reasoning", data: { text: "Canonical reasoning" } },
      { block_type: "text", data: { text: "After" } },
      { block_type: "server_tool_content", data: { id: "search-1", kind: "web_search", content: { type: "web_search_call", name: "web_search", status: "completed", action: { query: "Canonical search" } } } },
      { block_type: "image", data: { image_id: "image-1", blob_id: "saved-blob", media_type: "image/png" } },
    ];
    const saved = history("saved", A, "BeforeAfter", {}, blocks);
    const rows = assistantRows(project([live("draft", "reasoning_delta", A, { delta: "stale" }), live("image", "assistant_image_appended", A, { image: { image_id: "image-1", blob_id: "stale-blob" } }), saved]));
    assert.equal(rows.length, 1); assert.equal(rows[0].id, "saved");
    assert.deepEqual(rows[0].blocks.map(b => b.type), ["markdown", "thinking", "markdown", "tool-call", "image"]);
    assert.equal(rows[0].blocks.at(-1).blobId, "saved-blob");
    assert.equal(text(rows[0]), "BeforeAfter");
    const imageOnly = history("effect", B, "", {}, [blocks.at(-1)]); imageOnly.event = "assistant_message";
    assert.equal(assistantRows(project([imageOnly]))[0].blocks[0].imageId, "image-1");
  });
  test(`${surface}: history removal and later restore rebuild bindings without permanent tombstones`, () => {
    const delta = live("draft", "text_delta", A, { delta: "Provisional" });
    const saved = history("saved", A, "Canonical");
    assert.deepEqual(assistantRows(project([delta, saved])).map(text), ["Canonical"]);
    assert.deepEqual(assistantRows(project([delta])).map(text), ["Provisional"]);
    assert.deepEqual(assistantRows(project([delta, saved])).map(text), ["Canonical"]);
  });
  test(`${surface}: compaction summary deltas stay outside the reserved occurrence`, () => {
    const rows = assistantRows(project([
      live("start", "turn_started", A), live("old", "text_delta", A, { delta: "Before retry" }),
      frame("compact", "compaction_started", {}), frame("summary", "text_delta", { delta: "Private compaction summary" }),
      frame("compacted", "compaction_completed", {}), live("restart", "turn_started", A), live("fresh", "text_delta", A, { delta: "Fresh answer" }),
    ]));
    assert.deepEqual(rows.map(text).filter(Boolean), ["Fresh answer"]);
  });
  test(`${surface}: complete snapshots discard covered provisional and stale canonical rows only`, () => {
    const frames = [
      live("old-draft", "text_delta", A, { delta: "Discarded" }, { cursor: "console:40" }),
      history("old-saved", A, "Old canonical", { cursor: "console:41" }),
      live("later", "text_delta", B, { delta: "Later live" }, { cursor: "console:60" }),
      live("unknown-cursor", "text_delta", C, { delta: "Uncovered" }), snapshot("complete", []),
    ];
    assert.deepEqual(assistantRows(project(frames)).map(text), ["Later live", "Uncovered"]);
    assert(!project(frames).some(e => JSON.stringify(e).includes("assistant_history_snapshot")));
  });
  test(`${surface}: latest snapshot restores an exact ID with old cached frames retained`, () => {
    const draft = live("draft", "text_delta", A, { delta: "Draft" }, { cursor: "console:5" });
    const saved = history("saved", A, "Canonical", { cursor: "console:6" });
    const removed = snapshot("removed", [], 10);
    const restored = snapshot("restored", [A], 20);
    assert.equal(assistantRows(project([draft, saved, removed])).length, 0);
    for (const markers of [[removed, restored], [restored, removed]]) {
      assert.deepEqual(assistantRows(project([draft, saved, ...markers])).map(row => [row.id, text(row)]), [["saved", "Canonical"]]);
    }
  });
  test(`${surface}: malformed, partial or conflicting snapshots never authorize absence`, () => {
    const draft = live("draft", "text_delta", A, { delta: "Still present" }, { cursor: "console:5" });
    const invalid = [
      { complete: false }, { session_id: "fork" }, { observed_through: "console:nope" },
      { assistant_message_ids: [null] }, { assistant_message_ids: [""] },
    ];
    for (const data of invalid) {
      const marker = snapshot("invalid", []); Object.assign(marker.data as object, data);
      assert.deepEqual(assistantRows(project([draft, marker])).map(text), ["Still present"]);
    }
    for (const extra of [{ sessionId: "fork" }, { runtimeKey: "other" }, { identity: "other" }, { sourceKind: "console_event" }]) {
      assert.deepEqual(assistantRows(project([draft, snapshot("conflict", [], 50, extra)])).map(text), ["Still present"]);
    }
  });
  test(`${surface}: finished text replaces a changed provisional text candidate for its exact ID`, () => {
    const rows = assistantRows(project([
      live("delta", "text_delta", A, { delta: "Old streamed draft" }),
      live("done", "text_complete", A, { content: "Final provisional text" }),
    ]));
    assert.deepEqual(rows.map(text), ["Final provisional text"]);
  });
  test(`${surface}: image identity remains independent from the containing assistant occurrence`, () => {
    const image = { image_id: "same-image", blob_id: "same-blob", media_type: "image/png" };
    const rows = assistantRows(project([
      live("first-image", "assistant_image_appended", A, { image }),
      live("second-image", "assistant_image_appended", B, { image }),
    ]));
    assert.equal(rows.length, 2);
  });
  test(`${surface}: matched current history positions handle ID to no-ID rewrite and restoration`, () => {
    const original = history("original", A, "Original", { cursor: "console:5", sourceCursor: "session:0" });
    const edited = history("edited", null, "Edited without identity", { cursor: "console:15", sourceCursor: "session:0" });
    const rewrite = [snapshot("rewrite", [], 20), positions("rewrite-positions", [[edited.id, 0]], 20)];
    assert.deepEqual(assistantRows(project([original, edited, ...rewrite])).map(row => row.id), [edited.id]);
    const restore = [snapshot("restore", [A], 30), positions("restore-positions", [[original.id, 0]], 30)];
    for (const markers of [[...rewrite, ...restore], [...restore, ...rewrite]]) {
      assert.deepEqual(assistantRows(project([original, edited, ...markers])).map(row => [row.id, text(row)]), [[original.id, "Original"]]);
    }
    assert.deepEqual(assistantRows(project([original, edited, snapshot("later-observation", [A], 40), positions("old-positions", [[original.id, 0]], 30)])).map(row => row.id), [original.id]);
  });
  test(`${surface}: assistant-only observations preserve compaction of no-ID rows without hiding new appends`, () => {
    const removed = history("removed-no-id", null, "Removed legacy answer", { cursor: "console:5", sourceCursor: "session:0" });
    const kept = history("kept", A, "Kept answer", { cursor: "console:10", sourceCursor: "session:1" });
    const appended = history("appended", B, "Appended answer", { cursor: "console:25", sourceCursor: "session:1" });
    const beyondObservation = history("newest-no-id", null, "Not observed yet", { cursor: "console:45", sourceCursor: "session:2" });
    const compacted = positions("compacted", [[kept.id, 0]], 20);
    const markers = [snapshot("compact-assistants", [A], 20), compacted, snapshot("appended-assistants", [A, B], 40)];
    for (const order of [markers, [...markers].reverse()]) {
      assert.deepEqual(assistantRows(project([removed, kept, appended, beyondObservation, ...order])).map(row => row.id),
        [kept.id, appended.id, beyondObservation.id]);
    }
    const restored = positions("restored", [[removed.id, 0], [kept.id, 1], [appended.id, 2]], 50);
    assert.deepEqual(assistantRows(project([removed, kept, appended, ...markers, restored, snapshot("restored-assistants", [A, B], 50)])).map(row => row.id),
      [removed.id, kept.id, appended.id], "an explicit later full image restores the legacy row");
  });
  test(`${surface}: position filtering stops at the latest settled assistant observation and exact scope`, () => {
    const old = history("old-no-id", null, "Still present", { cursor: "console:5", sourceCursor: "session:0" });
    const settled = snapshot("settled", [], 20);
    const initial = positions("initial", [[old.id, 0]], 20);
    const unsettled = positions("unsettled-removal", [], 30);
    for (const order of [[initial, unsettled], [unsettled, initial]]) {
      assert.deepEqual(assistantRows(project([old, settled, ...order])).map(row => row.id), [old.id]);
    }
    assert.deepEqual(assistantRows(project([old, initial, unsettled, settled, snapshot("now-settled", [], 40)])).map(row => row.id), []);
    for (const extra of [{ runtimeKey: "other" }, { identity: "other" }, { sessionId: "fork" }, { sourceKind: "console_event" }]) {
      assert.deepEqual(assistantRows(project([old, settled, { ...positions("other-scope", [], 20), ...extra }])).map(row => row.id), [old.id]);
    }
    const malformed = positions("malformed", [], 20);
    Object.assign(malformed.data as object, { history_positions_mode: "sparse", removed_history_frame_ids: [""] });
    assert.deepEqual(assistantRows(project([old, settled, malformed])).map(row => row.id), [old.id]);
    const conflict = positions("conflicting", [], 20);
    for (const order of [[initial, conflict], [conflict, initial]]) {
      assert.deepEqual(assistantRows(project([old, settled, ...order])).map(row => row.id), [old.id], "same-cursor conflict cannot prove absence");
      assert.deepEqual(assistantRows(project([old, settled, positions("older-removal", [], 10), ...order])).map(row => row.id), [old.id],
        "a conflicting restoration cannot inherit older negative position evidence");
    }
  });
  test(`${surface}: sparse position refreshes retain no-ID removals and explicit restoration under assistant bounds`, () => {
    const old = history("old-no-id", null, "Legacy answer", { cursor: "console:5", sourceCursor: "session:0" });
    const appended = history("new-no-id", null, "New legacy answer", { cursor: "console:25", sourceCursor: "session:0" });
    const compacted = positions("compacted", [], 20);
    const refresh = positions("notice-only", [], 30);
    Object.assign(refresh.data as object, { history_positions_mode: "sparse", removed_history_frame_ids: [] });
    const settled = snapshot("settled", [], 40);
    for (const images of [[compacted, refresh], [refresh, compacted]]) {
      assert.deepEqual(assistantRows(project([old, appended, settled, ...images])).map(row => row.id), [appended.id]);
    }
    const restore = positions("restore-one", [[old.id, 0]], 50);
    Object.assign(restore.data as object, { history_positions_mode: "sparse", removed_history_frame_ids: [appended.id] });
    assert.deepEqual(assistantRows(project([old, appended, compacted, refresh, restore, settled])).map(row => row.id), [appended.id],
      "a newer unsettled sparse observation cannot remove or restore assistant rows yet");
    assert.deepEqual(assistantRows(project([old, appended, compacted, refresh, restore, settled, snapshot("settled-restore", [], 60)])).map(row => row.id), [old.id]);
    const conflictingAssistant = snapshot("conflicting-settled", [A], 40);
    assert.deepEqual(assistantRows(project([old, compacted, refresh, settled, conflictingAssistant])).map(row => row.id), [old.id]);
  });
  test(`${surface}: position conflicts across the settled boundary cannot hide the restoring twin`, () => {
    const old = history("old-no-id", null, "Legacy answer", { cursor: "console:5", sourceCursor: "session:0" });
    const settled = snapshot("settled", [], 25);
    const removed = positions("removed", [], 20);
    const restored = positions("restored", [[old.id, 0]], 30);
    removed.cursor = restored.cursor = "console:50";
    for (const twins of [[removed, restored], [restored, removed]]) {
      assert.deepEqual(assistantRows(project([old, settled, ...twins])).map(row => row.id), [old.id],
        "a conflicting twin beyond the settled cutoff still disqualifies negative evidence at the same cursor");
      assert.deepEqual(assistantRows(project([old, settled, positions("earlier-removal", [], 10), ...twins])).map(row => row.id), [old.id],
        "the cross-boundary conflict also invalidates older negative position evidence");
    }
  });
  test(`${surface}: canonical source positions still order the rebound row among historical siblings`, () => {
    const draft = live("draft", "text_delta", A, { delta: "Draft" }, { cursor: "console:1", timestampMs: 1 });
    const saved = history("saved", A, "Canonical", { cursor: "console:10", sourceCursor: "session:2", timestampMs: 10 });
    const earlier = history("earlier", B, "Earlier", { cursor: "console:9", sourceCursor: "session:1", timestampMs: 9 });
    assert.deepEqual(assistantRows(project([draft, earlier, saved])).map(row => row.id), [earlier.id, saved.id]);
  });
  test(`${surface}: conflicting same-cursor snapshots cannot authorize negative evidence`, () => {
    const draft = live("draft", "text_delta", A, { delta: "Present" }, { cursor: "console:5" });
    const empty = snapshot("empty", [], 10), present = snapshot("present", [A], 10);
    for (const markers of [[empty, present], [present, empty]]) {
      assert.deepEqual(assistantRows(project([draft, ...markers])).map(text), ["Present"]);
    }
  });
  test(`${surface}: distinct image IDs sharing a blob remain separate within an occurrence`, () => {
    const rows = assistantRows(project([
      live("image-a", "assistant_image_appended", A, { image: { image_id: "a", blob_id: "shared-blob" } }),
      live("image-b", "assistant_image_appended", A, { image: { image_id: "b", blob_id: "shared-blob" } }),
    ]));
    assert.equal(rows.length, 2);
  });
  test(`${surface}: canonical tool blocks own their exact tool IDs and never consume equal-signature siblings`, () => {
    const blocks = (id: string) => [
      { block_type: "text", data: { text: "Before" } },
      { block_type: "tool_use", data: { id, name: "lookup", args: {} } },
      { block_type: "text", data: { text: "After" } },
    ];
    const events = [
      frame("live-tool", "tool_call_requested", { id: "tool-a", name: "lookup", args: {} }),
      history("first", A, "BeforeAfter", {}, blocks("tool-a")),
      live("history-tool", "tool_call_requested", A, { id: "tool-a", name: "lookup", args: {} }, { sourceKind: "session_history" }),
      history("second", B, "BeforeAfter", {}, blocks("tool-b")),
    ];
    const rows = assistantRows(project(events));
    assert.deepEqual(rows.map(row => row.id), ["first", "second"]);
    assert.deepEqual(rows.map(row => row.blocks.map(b => b.type)), [["markdown", "tool-call", "markdown"], ["markdown", "tool-call", "markdown"]]);
    assert.deepEqual(rows.flatMap(row => row.blocks).filter(b => b.type === "tool-call").map(b => b.toolCallId), ["tool-a", "tool-b"]);
  });
  test(`${surface}: a missing-context row cannot absorb conflicting concrete occurrence owners`, () => {
    const unknown = history("unknown", A, "Unknown context", { runtimeKey: undefined, identity: undefined });
    const left = history("left", A, "Runtime A", { runtimeKey: "A" });
    const right = history("right", A, "Runtime B", { runtimeKey: "B" });
    for (const input of [[unknown, left, right], [left, unknown, right], [right, left, unknown]]) {
      assert.equal(assistantRows(project(input)).length, 3);
    }
  });
  test(`${surface}: canonical server search blocks retain provider annotation evidence and typed names`, () => {
    const annotations = { block_type: "server_tool_content", data: { id: "message-id", kind: "web_search", content: { type: "message_annotations", annotations: [{ type: "url_citation", title: "Primary source", url: "https://example.com/primary" }] } } };
    const row = assistantRows(project([history("search", A, "Answer", {}, [
      { block_type: "text", data: { text: "Answer" } },
      { block_type: "server_tool_content", data: { id: "search-id", kind: "web_search", content: { type: "web_search_call", status: "completed", action: { query: "release notes" } } } },
      annotations,
    ])]))[0];
    const tools = row.blocks.filter(b => b.type === "tool-call");
    assert.equal(tools.length, 1);
    assert.equal(tools[0].name, "web_search");
    assert.match(tools[0].result, /Primary source/);
    assert.match(tools[0].result, /https:\/\/example.com\/primary/);
    const orphan = assistantRows(project([history("citation-only", B, "", {}, [annotations])]))[0];
    assert.match(JSON.stringify(orphan), /https:\/\/example.com\/primary/);
  });
  test(`${surface}: canonical inline tools retain typed live results before the history result page arrives`, () => {
    const rows = assistantRows(project([
      frame("call", "tool_call_requested", { id: "call-a", name: "lookup", args: {} }),
      frame("result", "tool_execution_completed", { id: "call-a", result: "Lookup complete", is_error: false }),
      history("saved", A, "", {}, [{ block_type: "tool_use", data: { id: "call-a", name: "lookup", args: {} } }]),
    ]));
    assert.equal(rows.length, 1);
    const tool = rows[0].blocks[0];
    assert.equal(tool.status, "success");
    assert.equal(tool.result, "Lookup complete");
    assert.equal(tool.completionEvidence.outcome, "success");
  });
  test(`${surface}: missing-context attempts cannot let another runtime retry erase concrete content`, () => {
    const rows = assistantRows(project([
      live("unknown", "text_delta", A, { delta: "Unknown" }, { runtimeKey: undefined, identity: undefined }),
      live("concrete-a", "text_delta", A, { delta: "Runtime A" }, { runtimeKey: "runtime-a" }),
      live("retry-b", "retrying", A, {}, { runtimeKey: "runtime-b" }),
      live("fresh-b", "text_delta", A, { delta: "Runtime B" }, { runtimeKey: "runtime-b" }),
    ]));
    assert(rows.some(row => text(row).includes("Runtime A")));
    assert(rows.some(row => text(row).includes("Runtime B")));
  });
}
