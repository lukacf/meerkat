import assert from "node:assert/strict";
import test from "node:test";
import { mapFramesToTimelineEntries as stock } from "./adapters";
import { mapFramesToTimelineEntries as shared } from "../../../packages/console-core/src/adapters";
import type { ConsoleFrame } from "../../../packages/console-core/src/runtime-types";

const agent = { agent_id: "worker", member_id: "worker", identity: "worker", label: "Worker", kind: "mob_agent" };
const origin = (extra: Record<string, unknown> = {}) => ({
  session_id: "session", channel_id: "channel", canonical_row_sequence: 2,
  provider_item_ids: ["speech-a", "speech-b"], ...extra,
});
const expectedOrigin = { sessionId: "session", channelId: "channel", canonicalRowSequence: 2, providerItemIds: ["speech-a", "speech-b"] };
function history(id: string, value: unknown = origin(), extra: Partial<ConsoleFrame> = {}, core = false): ConsoleFrame {
  return {
    id, event: "text_complete", identity: "worker", runtimeKey: "runtime", sessionId: "session",
    sourceKind: "session_history", timestampMs: 10,
    data: { text: "Same spoken answer", result: "Same spoken answer", message: {
      role: "block_assistant", blocks: [{ block_type: "transcript", data: { text: "Same spoken answer" } }],
      ...(core ? { identity: { realtime_origin: value } } : { realtime_origin: value }),
    } }, ...extra,
  };
}
function userHistory(id: string, value: unknown = origin(), extra: Partial<ConsoleFrame> = {}): ConsoleFrame {
  const result = history(id, value, extra, true);
  result.event = "user_input";
  result.data = { content: "Same spoken input", message: { role: "user", content: "Same spoken input", identity: { realtime_origin: value } } };
  return result;
}
function visible(entry: any): string { return entry.copyText ?? entry.text ?? (entry.blocks ?? []).map((block: any) => block.source ?? block.text ?? "").join(""); }
for (const [surface, map] of [["stock", stock], ["shared", shared]] as const) {
  const project = (frames: ConsoleFrame[]) => map(agent, frames, { textMode: "markdown", renderInteractionStartsAsUser: true });
  const messages = (frames: ConsoleFrame[]) => project(frames).filter(entry => entry.kind === "message");
  test(`${surface}: canonical flat and core realtime origins preserve all provider items`, () => {
    for (const core of [false, true]) {
      const rows = messages([history("saved", origin(), {}, core)]);
      assert.equal(rows.length, 1);
      assert.deepEqual(rows[0].realtimeOrigin, expectedOrigin);
      assert.equal(visible(rows[0]), "Same spoken answer");
    }
  });
  test(`${surface}: legacy origin with no provider items stays visible without a live join key`, () => {
    const legacy = origin(); delete legacy.provider_item_ids;
    const rows = messages([history("legacy", legacy)]);
    assert.deepEqual(rows[0].realtimeOrigin, { ...expectedOrigin, providerItemIds: [] });
    assert.equal(visible(rows[0]), "Same spoken answer");
  });
  test(`${surface}: malformed, conflicting and unscoped origins never authorize a live join`, () => {
    const invalid = [null, "bad", [], origin({ session_id: "fork" }), origin({ session_id: "" }),
      origin({ channel_id: " " }), origin({ canonical_row_sequence: -1 }), origin({ canonical_row_sequence: 0.5 }),
      origin({ canonical_row_sequence: Number.MAX_SAFE_INTEGER + 1 }), origin({ provider_item_ids: "speech-a" }),
      origin({ provider_item_ids: ["speech-a", ""] }), origin({ provider_item_ids: [3] }), origin({ provider_item_ids: null })];
    for (const value of invalid) {
      const rows = messages([history("invalid", value)]);
      assert.equal(rows.length, 1);
      assert.equal(rows[0].realtimeOrigin, undefined, JSON.stringify(value));
    }
    for (const extra of [{ sessionId: undefined }, { sessionId: " " }, { sourceKind: "console_event" }]) {
      assert(!messages([history("unscoped", origin(), extra)]).some(row => row.realtimeOrigin));
    }
    const both = history("both");
    (both.data as any).message.identity = { realtime_origin: origin({ channel_id: "other-channel" }) };
    assert.equal(messages([both])[0].realtimeOrigin, undefined);
    (both.data as any).message.identity.realtime_origin = origin();
    assert.deepEqual(messages([both])[0].realtimeOrigin, expectedOrigin);
    (both.data as any).message.identity.realtime_origin = "malformed";
    assert.equal(messages([both])[0].realtimeOrigin, undefined);
    (both.data as any).message.identity.realtime_origin = null;
    assert.equal(messages([both])[0].realtimeOrigin, undefined);
  });
  test(`${surface}: repeated realtime occurrences cannot merge by text, run, interaction or time`, () => {
    for (const extra of [{}, { runId: "same-run" }, { interactionId: "01920000-0000-7000-8000-000000000001" }]) {
      const rows = messages([history("first", origin(), extra), history("second", origin({ canonical_row_sequence: 3, provider_item_ids: ["speech-c"] }), extra)]);
      assert.deepEqual(rows.map(row => row.id), ["first", "second"]);
      assert.deepEqual(rows.map(visible), ["Same spoken answer", "Same spoken answer"]);
    }
  });
  test(`${surface}: unkeyed session events never consume a canonical realtime row in either order`, () => {
    for (const value of [origin(), origin({ provider_item_ids: [""] })]) {
      for (const event of ["text_delta", "text_complete"]) {
        const live: ConsoleFrame = { id: "live", event, identity: "worker", runtimeKey: "runtime", sessionId: "session", runId: "run", sourceKind: "console_event", timestampMs: 9, data: { delta: "Same spoken answer", content: "Same spoken answer" } };
        const saved = history("saved", value, { runId: "run" });
        for (const frames of [[live, saved], [saved, live]]) {
          const rows = messages(frames);
          assert.equal(rows.length, 2, `${event}: ${frames[0].id}`);
          assert(rows.some(row => row.id === "saved" && visible(row) === "Same spoken answer"));
          assert(rows.some(row => row.id === "live" && visible(row) === "Same spoken answer"));
        }
      }
    }
  });
  test(`${surface}: legacy transcript blocks without origin remain independent canonical rows`, () => {
    const first = history("legacy-a");
    const second = history("legacy-b");
    delete (first.data as any).message.realtime_origin;
    delete (second.data as any).message.realtime_origin;
    const live: ConsoleFrame = { id: "unkeyed", event: "text_complete", sourceKind: "console_event", timestampMs: 9, data: { content: "Same spoken answer" } };
    const rows = messages([live, first, second]);
    assert.deepEqual(rows.map(row => row.id), ["unkeyed", "legacy-a", "legacy-b"]);
    assert(rows.every(row => !row.realtimeOrigin));
  });
  test(`${surface}: canonical realtime blocks retain source order and tool content`, () => {
    const saved = history("rich");
    (saved.data as any).message.blocks = [
      { block_type: "transcript", data: { text: "Before" } },
      { block_type: "reasoning", data: { text: "Thought" } },
      { block_type: "transcript", data: { text: "After" } },
      { block_type: "tool_use", data: { id: "voice-tool", name: "inspect", arguments: { path: "file" } } },
    ];
    const live: ConsoleFrame = { id: "tool", event: "tool_call_requested", data: { tool_call_id: "voice-tool", id: "voice-tool", name: "inspect", tool_name: "inspect", arguments: { path: "file" } } };
    const row = messages([live, saved]).find(entry => entry.id === "rich")!;
    assert.deepEqual(row.blocks?.map(block => block.type), ["markdown", "thinking", "markdown", "tool-call"]);
    const tool = row.blocks?.at(-1);
    assert.equal(tool?.type === "tool-call" && tool.toolCallId, "voice-tool");
  });
  test(`${surface}: typed user origin is carried only from canonical history and prevents interaction dedup`, () => {
    const rows = messages([userHistory("user-a", origin(), { interactionId: "same" }), userHistory("user-b", origin({ canonical_row_sequence: 3, provider_item_ids: ["input-b"] }), { interactionId: "same" })]);
    assert.deepEqual(rows.map(row => row.id), ["user-a", "user-b"]);
    assert(rows.every(row => row.identity.role === "user"));
    assert.deepEqual(rows[0].realtimeOrigin, expectedOrigin);
    const noWitness = userHistory("ordinary", null);
    delete (noWitness.data as any).message.identity;
    assert.equal(messages([noWitness])[0].realtimeOrigin, undefined);
    const provisional = userHistory("live-user", origin(), { sourceKind: "console_event" });
    assert.equal(messages([provisional])[0].realtimeOrigin, undefined);
  });
  test(`${surface}: different sessions and channels retain independent identical occurrences`, () => {
    const rows = messages([history("one"), history("fork", origin({ session_id: "fork" }), { sessionId: "fork" }), history("replacement", origin({ channel_id: "replacement" }))]);
    assert.deepEqual(rows.map(row => row.id), ["one", "fork", "replacement"]);
    assert.deepEqual(rows.map(row => row.realtimeOrigin?.channelId), ["channel", "channel", "replacement"]);
  });
}
