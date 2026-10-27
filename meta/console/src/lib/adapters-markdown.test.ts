import assert from "node:assert/strict";
import test from "node:test";
import { conversationEntryText } from "@console-core";
import { mapFramesToTimelineEntries as stock, createUserEntry } from "./adapters";
import { mapFramesToTimelineEntries as shared } from "../../../packages/console-core/src/adapters";
import type { ConsoleFrame } from "../types";

function frame(id: string, event: string, data: unknown, extra: Partial<ConsoleFrame> = {}): ConsoleFrame {
  return { id, event, data, timestampMs: 1, ...extra };
}

for (const [name, mapper] of [["stock", stock], ["shared opt-in", (agent, frames, options = {}) => shared(agent, frames, { ...options, textMode: "markdown" })]] as const) {
  test(`${name}: raw live and final document use the same stable source identity`, () => {
    const source = "  # Heading\r\n\r\n- one\n  - two\n\n[ref][later]\n\n[later]: https://example.test\n";
    const frames = [frame("delta", "text_delta", { delta: source }, { interactionId: "run-1" })];
    const live = mapper(null, frames, { renderTextDeltas: true });
    const final = mapper(null, frames.concat(frame("complete", "interaction_complete", { result: source }, { interactionId: "run-1" })), { renderTextDeltas: true });
    assert.equal(live.length, 1);
    assert.equal(final.length, 1);
    assert.equal(live[0].kind, "message");
    if (live[0].kind !== "message" || final[0].kind !== "message") throw new Error("expected messages");
    assert.equal(live[0].blocks?.[0].type, "markdown");
    assert.deepEqual(live[0].blocks, [{ type: "markdown", id: "delta:text:0", source, streaming: true }]);
    assert.deepEqual(final[0].blocks, [{ type: "markdown", id: "delta:text:0", source, streaming: false }]);
    assert.equal(conversationEntryText(final[0]), source);
  });

  test(`${name}: final/history/user paths use the same source-preserving builder`, () => {
    const source = "  Created file.txt +1 -0\n\n[link](https://example.test)\n";
    for (const input of [
      frame("final", "interaction_complete", { result: source }),
      frame("history", "text_complete", { message: { role: "assistant", content: source } }, { sourceKind: "session_history" }),
      frame("history-block", "text_complete", { message: { role: "block_assistant", blocks: [{ block_type: "text", data: { text: source } }] } }, { sourceKind: "session_history" }),
      frame("user", "user_input", { content: source }),
    ]) {
      const entries = mapper(null, [input], { renderInteractionStartsAsUser: true });
      assert.equal(entries.length, 1, input.id);
      assert.equal(conversationEntryText(entries[0]), source, input.id);
      const entry = entries[0];
      assert.equal(entry.kind === "message" && entry.blocks?.[0].type, "markdown", input.id);
    }
  });

  test(`${name}: typed image and tool siblings stay typed beside Markdown`, () => {
    const entries = mapper(null, [frame("history", "text_complete", { message: { role: "block_assistant", blocks: [
      { block_type: "tool_use", data: { id: "tool-1", name: "read_file", args: { path: "a" } } },
      { block_type: "text", data: { text: "Answer\n" } },
    ] } }, { sourceKind: "session_history" })]);
    const entry = entries[0];
    assert.equal(entry.kind === "message" && entry.blocks?.[0].type, "tool-call");
    assert.equal(entry.kind === "message" && entry.blocks?.[1].type, "markdown");
  });
}

test("shared mapper retains the legacy default and stock optimistic sends opt into Markdown", () => {
  const legacy = shared(null, [frame("reply", "interaction_complete", { result: "# heading\n\nparagraph" })]);
  assert.equal(legacy[0].kind === "message" && legacy[0].blocks?.[0].type, "heading");
  const source = "  user text\n";
  const optimistic = createUserEntry(source);
  assert.equal(conversationEntryText(optimistic), source);
  assert.equal(optimistic.kind === "message" && optimistic.blocks?.[0].type, "markdown");
});
