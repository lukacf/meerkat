import { fireEvent, render, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { ConsoleFrame } from "../../../console-core/src/runtime-types";
import { buildConversationViewState, mapFramesToTimelineEntries as shared } from "../../../console-core/src/adapters";
import { mapFramesToTimelineEntries as stock } from "../../../../console/src/lib/adapters";
import { ChatPane } from "../../../../console/src/panels/ChatPane";
import { ConversationTranscript } from "./conversation-transcript";

const agent = { agent_id: "worker", member_id: "worker", identity: "worker", label: "Worker", kind: "mob_agent" };
const A = "01920000-0000-7000-8000-000000000001";
const B = "01920000-0000-7000-8000-000000000002";
const selectedText = "Preserve this exact selection: A\u030a, \u00e5 and \ud83d\ude80.";
const prefix = `${selectedText}\n\nStill arriving`;
const complete = `${selectedText}\n\nComplete provisional answer.\n`;
const canonical = `${selectedText}\n\nCanonical corrected answer.\n`;
const noop = () => {};

function frame(id: string, event: string, data: Record<string, unknown>, extra: Partial<ConsoleFrame> = {}): ConsoleFrame {
  return { id, event, data, identity: "worker", runtimeKey: "runtime", sessionId: "session",
    runId: "run", interactionId: "interaction", sourceKind: "console_event", timestampMs: 1000, ...extra };
}

function delta(id: string, assistant: string, text: string, extra: Partial<ConsoleFrame> = {}) {
  return frame(id, "text_delta", { assistant_message_id: assistant, delta: text }, extra);
}

function finished(id: string, assistant: string, text: string, extra: Partial<ConsoleFrame> = {}) {
  return frame(id, "text_complete", { assistant_message_id: assistant, content: text }, extra);
}

function history(id: string, assistant: unknown, text: string, extra: Partial<ConsoleFrame> = {}) {
  return frame(id, "text_complete", {
    ...(assistant === undefined ? {} : { assistant_message_id: assistant }), text, result: text,
    message: { role: "block_assistant", blocks: [{ block_type: "text", data: { text } }], stop_reason: "end_turn" },
  }, { sourceKind: "session_history", ...extra });
}

function paragraph(container: HTMLElement, text: string): HTMLParagraphElement {
  const node = [...container.querySelectorAll("p")].find(item => item.textContent === text);
  expect(node, "the expected exact paragraph is rendered").toBeDefined();
  return node!;
}

function targets(node: Element) {
  return {
    paragraph: node,
    document: node.closest<HTMLElement>(".cc-markdown-document")!,
    row: node.closest<HTMLElement>("[data-conversation-row-id]")!,
    quote: node.closest<HTMLElement>("[data-quote-message-id]")!,
  };
}

afterEach(() => window.getSelection()?.removeAllRanges());

describe("shared canonical presentation copy", () => {
  for (const withReasoning of [false, true]) {
    it(`copies the exact canonical answer once across a routine tool${withReasoning ? " and reasoning" : ""}`, async () => {
      const first = `${selectedText}\n\n`;
      const second = "The release check passed.\n";
      const source = first + second;
      const saved = history("canonical-copy-frame", A, source);
      (saved.data as Record<string, unknown>).message = { role: "block_assistant", blocks: [
        ...(withReasoning ? [{ block_type: "reasoning", data: { text: "Check the release prerequisites." } }] : []),
        { block_type: "text", data: { text: first } },
        { block_type: "tool_use", data: { id: "release-check", name: "workgraph_ready", args: { labels: ["release"] } } },
        { block_type: "text", data: { text: second } },
      ], stop_reason: "end_turn" };
      const entries = shared(agent, [saved], { textMode: "markdown" });
      expect(entries).toHaveLength(1);
      expect(entries[0].id).toBe("canonical-copy-frame");
      expect(entries[0].kind === "message" && entries[0].copyText).toBe(source);
      const result = render(<ConversationTranscript viewState={buildConversationViewState({ memberId: "worker", agentLabel: "Worker", entries })} />);
      const documents = [...result.container.querySelectorAll<HTMLElement>(".cc-markdown-document")];
      expect(documents).toHaveLength(2);
      documents.forEach(node => expect(node.closest<HTMLElement>("[data-quote-message-id]")?.dataset.quoteMessageId).toBe("canonical-copy-frame"));
      const rows = [...result.container.querySelectorAll<HTMLElement>("[data-conversation-row-id]")];
      expect(new Set(rows.map(node => node.dataset.conversationRowId)).size).toBe(rows.length);
      const previousClipboard = Object.getOwnPropertyDescriptor(navigator, "clipboard");
      const writeText = vi.fn().mockResolvedValue(undefined);
      Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
      try {
        fireEvent.click(result.getByRole("button", { name: "Copy response", exact: true }));
        await waitFor(() => expect(writeText).toHaveBeenCalledTimes(1));
        expect(writeText).toHaveBeenCalledWith(source);
      } finally {
        if (previousClipboard) Object.defineProperty(navigator, "clipboard", previousClipboard);
        else Reflect.deleteProperty(navigator, "clipboard");
      }
    });
  }
});

for (const [surface, map] of [["stock", stock], ["shared", shared]] as const) {
describe(`${surface} assistant group fallback`, () => {
  for (const leadingKind of ["tool", "image"] as const) {
  it(`preserves selected answer and reasoning after a leading ${leadingKind} hydrates without run or interaction IDs`, () => {
    const owner = { runId: undefined, interactionId: undefined };
    const reasoning = "Check the release prerequisites.";
    const image = { image_id: "release-image", blob_id: "release-image-blob", media_type: "image/png" };
    const leadingFrame = leadingKind === "tool"
      ? frame("leading-tool", "tool_call_requested", { id: "release-check", name: "workgraph_ready", args: { labels: ["release"] } }, owner)
      : frame("leading-image", "assistant_image_appended", { assistant_message_id: A, image }, owner);
    const live = [
      leadingFrame,
      frame("thinking-live", "reasoning_delta", { assistant_message_id: A, delta: reasoning }, owner),
      frame("thinking-done", "reasoning_complete", { assistant_message_id: A, content: reasoning }, owner),
      delta("answer-live", A, `${selectedText}\n`, owner),
    ];
    const sourceId = `canonical-leading-${leadingKind}`;
    const saved = history(sourceId, A, `${selectedText}\n`, owner);
    (saved.data as Record<string, unknown>).message = { role: "block_assistant", blocks: [
      leadingKind === "tool"
        ? { block_type: "tool_use", data: { id: "release-check", name: "workgraph_ready", args: { labels: ["release"] } } }
        : { block_type: "image", data: image },
      { block_type: "reasoning", data: { text: reasoning } },
      { block_type: "text", data: { text: `${selectedText}\n` } },
    ], stop_reason: "end_turn" };
    const view = (frames: ConsoleFrame[]) => {
      const entries = map(agent, frames, { textMode: "markdown" });
      return surface === "stock"
        ? <ChatPane agent={agent} agentLabel="Worker" identity="worker" entries={entries} phase={null}
            draft="Unsent draft" sending={false} staged={[]} onDraftChange={noop} onStagedChange={noop} onSend={() => false} />
        : <ConversationTranscript viewState={buildConversationViewState({ memberId: "worker", agentLabel: "Worker", entries })} />;
    };
    const result = render(view(live));
    const original = targets(paragraph(result.container, selectedText));
    const originalTurn = original.row.closest<HTMLElement>("[data-conversation-turn-id]")!;
    const originalGroup = original.row.closest(".cc-message-group");
    const thinking = result.container.querySelector<HTMLDetailsElement>("details.cc-rich-thinking")!;
    expect(thinking.querySelector(".cc-rich-thinking__body")?.textContent).toBe(reasoning);
    thinking.open = false;
    const range = document.createRange();
    range.selectNodeContents(original.paragraph);
    window.getSelection()!.removeAllRanges();
    window.getSelection()!.addRange(range);
    expect(window.getSelection()!.toString()).toBe(selectedText);

    const canonicalEntries = map(agent, [...live, saved], { textMode: "markdown" });
    expect(canonicalEntries).toHaveLength(1);
    expect(canonicalEntries[0].id).toBe(sourceId);
    result.rerender(view([...live, saved]));
    const current = targets(paragraph(result.container, selectedText));
    expect(current.quote.dataset.quoteMessageId).toBe(sourceId);
    expect(current.row.dataset.conversationRowId).toBe(original.row.dataset.conversationRowId);
    expect(current.row.closest("[data-conversation-turn-id]"), `canonical ${leadingKind} prefix cannot replace the turn`).toBe(originalTurn);
    if (surface === "shared") expect(current.row.closest(".cc-message-group")).toBe(originalGroup);
    expect(current.row).toBe(original.row);
    expect(current.document).toBe(original.document);
    expect(current.paragraph).toBe(original.paragraph);
    expect(original.paragraph.isConnected).toBe(true);
    expect(window.getSelection()!.toString()).toBe(selectedText);
    const updatedThinking = result.container.querySelector<HTMLDetailsElement>("details.cc-rich-thinking")!;
    expect(updatedThinking).toBe(thinking);
    expect(updatedThinking.open, "reader-closed reasoning stays closed through hydration").toBe(false);
    expect(updatedThinking.querySelector(".cc-rich-thinking__body")?.textContent).toBe(reasoning);
  });
  }
});
}

for (const [surface, map] of [["stock", stock], ["shared", shared]] as const) {
  const project = (frames: ConsoleFrame[]) => map(agent, frames, { textMode: "markdown", renderInteractionStartsAsUser: true });
  function view(frames: ConsoleFrame[]) {
    const entries = project(frames);
    return surface === "stock"
      ? <ChatPane agent={agent} agentLabel="Worker" identity="worker" entries={entries} phase={null}
          draft="Unsent draft" sending={false} staged={[]} onDraftChange={noop} onStagedChange={noop} onSend={() => false} />
      : <ConversationTranscript viewState={buildConversationViewState({ memberId: "worker", agentLabel: "Worker", entries })} />;
  }

  describe(`${surface} assistant presentation identity`, () => {
    it("preserves equal reasoning disclosures and the selected answer when canonical history bundles them", () => {
      const reasoning = "Check again again and check again.";
      const user = frame("operator", "user_input", { content: "Check this twice." }, { sourceKind: "send" });
      const live = [user,
        frame("thinking-a", "reasoning_delta", { assistant_message_id: A, delta: reasoning }, { timestampMs: 1001 }),
        frame("thinking-a-done", "reasoning_complete", { assistant_message_id: A, content: reasoning }, { timestampMs: 1002 }),
        frame("thinking-b", "reasoning_delta", { assistant_message_id: A, delta: reasoning }, { timestampMs: 1003 }),
        frame("thinking-b-done", "reasoning_complete", { assistant_message_id: A, content: reasoning }, { timestampMs: 1004 }),
        delta("answer-live", A, `${selectedText}\n`, { timestampMs: 1005 }),
        finished("answer-done", A, `${selectedText}\n`, { timestampMs: 1006 }),
      ];
      const saved = history("canonical-rich-frame", A, `${selectedText}\n`, { timestampMs: 1007 });
      (saved.data as Record<string, unknown>).message = { role: "block_assistant", blocks: [
        { block_type: "reasoning", data: { text: reasoning } },
        { block_type: "reasoning", data: { text: reasoning } },
        { block_type: "text", data: { text: `${selectedText}\n` } },
      ], stop_reason: "end_turn" };
      const result = render(view(live));
      const original = targets(paragraph(result.container, selectedText));
      const originalDocumentId = original.document.dataset.markdownDocumentId;
      const originalRowId = original.row.dataset.conversationRowId;
      const disclosures = [...result.container.querySelectorAll<HTMLDetailsElement>("details.cc-rich-thinking")];
      expect(disclosures).toHaveLength(2);
      expect(disclosures.map(node => node.querySelector(".cc-rich-thinking__body")?.textContent)).toEqual([reasoning, reasoning]);
      disclosures.forEach(node => { node.open = true; });
      const range = document.createRange();
      range.selectNodeContents(original.paragraph);
      window.getSelection()!.removeAllRanges();
      window.getSelection()!.addRange(range);
      expect(window.getSelection()!.toString()).toBe(selectedText);

      result.rerender(view([...live, saved]));
      const current = targets(paragraph(result.container, selectedText));
      expect(current.quote.dataset.quoteMessageId, "the answer keeps canonical source provenance").toBe("canonical-rich-frame");
      expect(current.row, "canonical reasoning cannot take over the answer's row").toBe(original.row);
      expect(current.document).toBe(original.document);
      expect(current.paragraph).toBe(original.paragraph);
      expect(current.row.dataset.conversationRowId).toBe(originalRowId);
      expect(current.document.dataset.markdownDocumentId).toBe(originalDocumentId);
      expect(original.paragraph.isConnected).toBe(true);
      expect(window.getSelection()!.toString()).toBe(selectedText);
      const updated = [...result.container.querySelectorAll<HTMLDetailsElement>("details.cc-rich-thinking")];
      expect(updated).toHaveLength(2);
      expect(updated.map(node => node.querySelector(".cc-rich-thinking__body")?.textContent)).toEqual([reasoning, reasoning]);
      updated.forEach((node, index) => {
        expect(node.querySelector("summary")?.textContent).toMatch(/thinking/i);
        expect(node, "each equal reasoning occurrence retains its disclosure").toBe(disclosures[index]);
        expect(node.open, "user-opened reasoning stays open through canonical hydration").toBe(true);
        expect(node.compareDocumentPosition(current.paragraph) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
      });
      const rowIds = [...result.container.querySelectorAll<HTMLElement>("[data-conversation-row-id]")].map(node => node.dataset.conversationRowId);
      expect(new Set(rowIds).size).toBe(rowIds.length);
    });

    for (const withUser of [false, true]) {
      it(`keeps selected DOM through completion and canonical history ${withUser ? "after a user row" : "without a user prefix"}`, () => {
        const user = withUser ? [frame("operator", "user_input", { content: "Review this reply." }, { sourceKind: "send" })] : [];
        const live = delta("opening-delta", A, prefix);
        const done = finished("completed-event", A, complete);
        const saved = history("canonical-frame", A, canonical);
        const result = render(view([...user, live]));
        expect(result.container.querySelector('[data-quote-message-id="operator"]') !== null).toBe(withUser);
        const original = targets(paragraph(result.container, selectedText));
        expect(original.document.dataset.streaming).toBe("true");
        const originalDocumentId = original.document.dataset.markdownDocumentId;
        const range = document.createRange();
        range.selectNodeContents(original.paragraph);
        window.getSelection()!.removeAllRanges();
        window.getSelection()!.addRange(range);
        expect(window.getSelection()!.toString()).toBe(selectedText);

        for (const [frames, expectedSource, expectedFrame] of [
          [[...user, live, done], complete, "completed-event"],
          [[...user, live, done, saved], canonical, "canonical-frame"],
        ] as const) {
          result.rerender(view([...frames]));
          const current = targets(paragraph(result.container, selectedText));
          expect(current.row, "the rendered row is the same DOM node").toBe(original.row);
          expect(current.document, "the Markdown document is the same DOM node").toBe(original.document);
          expect(current.paragraph, "the selected paragraph is the same DOM node").toBe(original.paragraph);
          expect(original.paragraph.isConnected).toBe(true);
          expect(current.document.dataset.markdownDocumentId).toBe(originalDocumentId);
          expect(current.document.dataset.streaming).toBe("false");
          expect(window.getSelection()!.toString()).toBe(selectedText);
          expect(current.quote.dataset.quoteMessageId, "quote provenance uses the current source frame").toBe(expectedFrame);
          expect(current.quote.dataset.quoteSource).toBe(expectedSource);
          expect(project([...frames]).some(entry => entry.id === expectedFrame)).toBe(true);
          expect(result.container.querySelectorAll(".cc-markdown-document")).toHaveLength(withUser ? 2 : 1);
        }
      });
    }

    it("keeps equal replies with different assistant IDs separate in the same run", () => {
      const text = "Exactly the same reply.";
      const first = delta("first-live", A, text);
      const firstDone = finished("first-done", A, text);
      const second = delta("second-live", B, text);
      const result = render(view([first, firstDone, second]));
      const documents = [...result.container.querySelectorAll<HTMLElement>(".cc-markdown-document")];
      expect(documents).toHaveLength(2);
      expect(new Set(documents.map(node => node.dataset.markdownDocumentId)).size).toBe(2);
      const rows = documents.map(node => node.closest("[data-conversation-row-id]"));
      result.rerender(view([first, firstDone, second, history("first-saved", A, text), history("second-saved", B, text)]));
      const updated = [...result.container.querySelectorAll<HTMLElement>(".cc-markdown-document")];
      expect(updated).toHaveLength(2);
      updated.forEach((node, index) => {
        expect(node).toBe(documents[index]);
        expect(node.closest("[data-conversation-row-id]")).toBe(rows[index]);
        expect(node.closest<HTMLElement>("[data-quote-message-id]")!.dataset.quoteMessageId).toBe(index ? "second-saved" : "first-saved");
      });
    });

    for (const scope of [{ sessionId: "fork-session" }, { runtimeKey: "other-runtime" }, { identity: "other-worker" }]) {
      it(`isolates the same assistant ID in another ${Object.keys(scope)[0]}`, () => {
        const text = "Equal content in separate scopes.";
        const first = delta("scope-a-live", A, text);
        const second = delta("scope-b-live", A, text, scope);
        const result = render(view([first, second]));
        const documents = [...result.container.querySelectorAll<HTMLElement>(".cc-markdown-document")];
        expect(documents).toHaveLength(2);
        expect(new Set(documents.map(node => node.dataset.markdownDocumentId)).size).toBe(2);
        result.rerender(view([first, second, history("scope-a-saved", A, text), history("scope-b-saved", A, text, scope)]));
        const updated = [...result.container.querySelectorAll<HTMLElement>(".cc-markdown-document")];
        expect(updated).toHaveLength(2);
        updated.forEach((node, index) => expect(node).toBe(documents[index]));
      });
    }

    it("retains source-frame fallback for legacy and invalid unscoped messages", () => {
      const frames = [history("legacy-frame", undefined, "Legacy reply."),
        history("unscoped-frame", A, "Unscoped reply.", { sessionId: undefined })];
      const entries = project(frames);
      expect(entries.map(entry => entry.id)).toEqual(["legacy-frame", "unscoped-frame"]);
      entries.forEach(entry => expect("renderKey" in entry ? entry.renderKey : undefined).toBeUndefined());
      const result = render(view(frames));
      const documents = [...result.container.querySelectorAll<HTMLElement>(".cc-markdown-document")];
      expect(documents.map(node => node.dataset.markdownDocumentId)).toEqual(["legacy-frame:text:0", "unscoped-frame:text:0"]);
      result.rerender(view(frames));
      [...result.container.querySelectorAll(".cc-markdown-document")].forEach((node, index) => expect(node).toBe(documents[index]));
    });
  });
}
