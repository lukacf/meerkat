import assert from "node:assert/strict";
import test from "node:test";
import { mapFramesToTimelineEntries as shared } from "./adapters";
import { mapFramesToTimelineEntries as stock } from "../../../console/src/lib/adapters";
import { conversationEntryText } from "./conversation";
import { createConsoleContextRecord } from "./context-record";
import type { ConsoleFrame } from "./runtime-types";

const common = { sessionId: "source-session", identity: "router:main", runtimeKey: "source-runtime", runId: "same-run", interactionId: "same-interaction" };
function frame(id: string, event: string, assistantId: string, data: Record<string, unknown>, extra: Partial<ConsoleFrame> = {}): ConsoleFrame {
  return { ...common, id, event, timestampMs: 100, sourceKind: "console_event", data: { ...data, assistant_message_id: assistantId }, ...extra };
}
function history(id: string, assistantId: string, text: string, extra: Partial<ConsoleFrame> = {}): ConsoleFrame {
  return frame(id, "text_complete", assistantId, { text, message: {
    role: "block_assistant", assistant_message_id: assistantId,
    blocks: [{ block_type: "text", data: { text } }],
  } }, { sourceKind: "session_history", ...extra });
}

for (const [name, mapper] of [["stock", stock], ["shared", shared]] as const) {
  const map = (frames: ConsoleFrame[]) => mapper(null, frames, { renderTextDeltas: true, textMode: "markdown" });

  test(`${name}: canonical assistant identity retains exact source bytes and its quote frame`, () => {
    const source = "  # Release\r\n\r\nA\u030A and 🚀\n\n  closing whitespace  \n";
    const canonical = history("canonical-source-frame", "assistant-occurrence", source);
    const live = frame("provisional-source-frame", "text_delta", "assistant-occurrence", { delta: "draft response" });
    const result = frame("rewritten-result-frame", "interaction_complete", "assistant-occurrence", { result: "hook replacement must not become source" });
    const late = frame("late-source-frame", "text_delta", "assistant-occurrence", { delta: " late stale bytes" });
    for (const frames of [[live, result, canonical, late], [canonical, live, late, result]]) {
      const entries = map(frames);
      assert.equal(entries.length, 1, JSON.stringify(entries));
      const entry = entries[0];
      assert.equal(entry.id, canonical.id, "quote source remains the canonical frame, not the assistant occurrence ID");
      assert.equal(conversationEntryText(entry), source);
      const quote = "A\u030A and 🚀";
      const context = createConsoleContextRecord({ id: "quote", sourceScope: "source-runtime", sourceIdentity: "router:main", messageId: entry.id, label: "Router", quote, sourceText: conversationEntryText(entry) });
      assert.equal(context.messageId, "canonical-source-frame");
      assert.notEqual(context.messageId, "assistant-occurrence");
      assert.deepEqual(context.sourceRange, { start: source.indexOf(quote), end: source.indexOf(quote) + quote.length, unit: "utf16" });
    }
  });

  test(`${name}: equal canonical replies in one run retain independent quote source frames`, () => {
    const first = history("canonical-first-frame", "assistant-first", "Acknowledged.");
    const second = history("canonical-second-frame", "assistant-second", "Acknowledged.", { timestampMs: 101 });
    const entries = map([first, second]);
    assert.equal(entries.length, 2, JSON.stringify(entries));
    assert.deepEqual(entries.map(entry => entry.id), [first.id, second.id]);
    assert.deepEqual(entries.map(conversationEntryText), ["Acknowledged.", "Acknowledged."]);
    const quotes = entries.map((entry, index) => createConsoleContextRecord({ id: `quote-${index}`, sourceScope: "source-runtime", sourceIdentity: "router:main", messageId: entry.id, label: "Router", quote: conversationEntryText(entry), sourceText: conversationEntryText(entry) }));
    assert.notEqual(quotes[0].messageId, quotes[1].messageId);
  });

  test(`${name}: canonical sibling blocks preserve exact no-separator copy and block identities`, () => {
    const first = "  first\r\n";
    const last = "\nA\u030A and 🚀  ";
    const input = frame("canonical-rich-frame", "text_complete", "containing-assistant", { text: first + last, message: {
      role: "block_assistant", assistant_message_id: "containing-assistant", blocks: [
        { block_type: "text", data: { text: first } },
        { block_type: "reasoning", data: { text: "private rationale" } },
        { block_type: "tool_use", data: { id: "tool-block", name: "lookup", args: { query: "release" } } },
        { block_type: "text", data: { text: last } },
      ],
    } }, { sourceKind: "session_history" });
    const entries = map([input]);
    assert.equal(entries.length, 1);
    const entry = entries[0];
    assert.equal(entry.id, input.id);
    assert.equal(conversationEntryText(entry), first + last);
    assert.equal(entry.kind, "message");
    if (entry.kind !== "message") throw new Error("expected a message");
    const tool = entry.blocks?.find(block => block.type === "tool-call");
    assert.equal(tool?.type === "tool-call" && tool.toolCallId, "tool-block");
    assert.ok(entry.blocks?.filter(block => block.type === "markdown").every(block => block.id !== "containing-assistant"));
  });

  test(`${name}: a partial history page only replaces its own assistant occurrence`, () => {
    const first = frame("live-first-source", "text_delta", "assistant-first", { delta: "Same answer" });
    const second = frame("live-second-source", "text_delta", "assistant-second", { delta: "Same answer" });
    const partial = history("canonical-second-source", "assistant-second", "Same answer");
    const entries = map([first, second, partial]);
    assert.equal(entries.length, 2, JSON.stringify(entries));
    assert.deepEqual(entries.map(entry => entry.id), [first.id, partial.id]);
    assert.deepEqual(entries.map(conversationEntryText), ["Same answer", "Same answer"]);
  });
}
