import { render } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import type { ConsoleFrame } from "../../../console-core/src/runtime-types";
import { buildConversationViewState, mapFramesToTimelineEntries as shared } from "../../../console-core/src/adapters";
import { mapFramesToTimelineEntries as stock } from "../../../../console/src/lib/adapters";
import { ChatPane } from "../../../../console/src/panels/ChatPane";
import { ConversationTranscript } from "./conversation-transcript";

const agent = { agent_id: "worker", member_id: "worker", identity: "worker", label: "Worker", kind: "mob_agent" };
const interaction = "6117b9cd-2d38-5a0c-8892-93ef8846e6d3";
const otherInteraction = "01920000-0000-7000-8000-000000000002";
const selectedText = "Preserve this exact selection: A\u030a, \u00e5 and \ud83d\ude80.";
const inputText = `${selectedText}\n\nKeep this instruction exactly.\n`;
const noop = () => {};

function frame(id: string, event: string, data: Record<string, unknown>, extra: Partial<ConsoleFrame> = {}): ConsoleFrame {
  return { id, event, data, identity: "worker", runtimeKey: "runtime", sessionId: "session",
    interactionId: interaction, sourceKind: "console_event", timestampMs: 2000, ...extra };
}

function history(id: string, text = inputText, extra: Partial<ConsoleFrame> = {}): ConsoleFrame {
  return frame(id, "user_input", { content: [{ type: "text", text }],
    message: { role: "user", content: text, identity: { interaction_id: extra.interactionId ?? interaction, run_id: "run" } },
  }, { sourceKind: "session_history", runId: "run", timestampMs: 2017, ...extra });
}

function send(id: string, text = inputText, extra: Partial<ConsoleFrame> = {}): ConsoleFrame {
  return frame(id, "user_input", { content: text, origin: "console:visual-acceptance", origin_kind: "operator" },
    { sourceKind: "send", status: "delivered", timestampMs: 2013, ...extra });
}

function answer(id: string, text: string, extra: Partial<ConsoleFrame> = {}): ConsoleFrame {
  return frame(id, "text_complete", { content: text, assistant_message_id: `assistant-${id}` },
    { runId: "run", timestampMs: 2020, ...extra });
}

function paragraph(container: HTMLElement, text: string): HTMLParagraphElement {
  const node = [...container.querySelectorAll("p")].find(item => item.textContent === text);
  expect(node, "the exact paragraph is rendered").toBeDefined();
  return node!;
}

function targets(node: Element) {
  return { paragraph: node, document: node.closest<HTMLElement>(".cc-markdown-document")!,
    row: node.closest<HTMLElement>("[data-conversation-row-id]")!,
    quote: node.closest<HTMLElement>("[data-quote-message-id]")!,
    turn: node.closest<HTMLElement>("[data-conversation-turn-id]")! };
}

function documents(container: HTMLElement) {
  return [...container.querySelectorAll<HTMLElement>(".cc-markdown-document")];
}

afterEach(() => window.getSelection()?.removeAllRanges());

for (const [surface, map] of [["stock", stock], ["shared", shared]] as const) {
  const project = (frames: ConsoleFrame[]) => map(agent, frames, { textMode: "markdown", renderInteractionStartsAsUser: true });
  function view(frames: ConsoleFrame[]) {
    const entries = project(frames);
    return surface === "stock"
      ? <ChatPane agent={agent} agentLabel="Worker" identity="worker" entries={entries} phase={null}
          draft="Unsent draft" sending={false} staged={[]} onDraftChange={noop} onStagedChange={noop} onSend={() => false} />
      : <ConversationTranscript viewState={buildConversationViewState({ memberId: "worker", agentLabel: "Worker", entries })} />;
  }

  describe(`${surface} user presentation identity`, () => {
    for (const sendFirst of [true, false]) {
      it(`preserves selected history and its reply when the earlier send arrives ${sendFirst ? "first" : "last"} in the page`, () => {
        const saved = history("saved-input");
        const admitted = send("admitted-input");
        const reply = answer("reply", "The following response stays mounted.");
        const result = render(view([saved, reply]));
        const original = targets(paragraph(result.container, selectedText));
        const originalReply = targets(paragraph(result.container, "The following response stays mounted."));
        expect(original.row.textContent).toContain("User message");
        expect(original.quote.dataset.quoteMessageId).toBe("saved-input");
        const range = document.createRange();
        range.selectNodeContents(original.paragraph);
        window.getSelection()!.removeAllRanges();
        window.getSelection()!.addRange(range);
        expect(window.getSelection()!.toString()).toBe(selectedText);

        const older = [history("older-input", "Earlier instruction.", { interactionId: otherInteraction, timestampMs: 1000 }),
          answer("older-reply", "Earlier response.", { interactionId: otherInteraction, timestampMs: 1001 })];
        const frames = [...older, ...(sendFirst ? [admitted, saved, reply] : [saved, reply, admitted])];
        result.rerender(view(frames));
        const current = targets(paragraph(result.container, selectedText));
        const currentReply = targets(paragraph(result.container, "The following response stays mounted."));
        expect(current.quote.dataset.quoteMessageId, "quote provenance follows the earlier admitted source").toBe("admitted-input");
        expect(current.quote.dataset.quoteSource).toBe(inputText);
        expect(current.row.textContent).toContain("Operator");
        expect(project(frames).filter(entry => entry.identity.role === "user").map(entry => entry.id)).toEqual(["older-input", "admitted-input"]);
        expect(current.turn, "the containing turn stays mounted").toBe(original.turn);
        expect(current.row, "the selected row stays mounted").toBe(original.row);
        expect(current.document, "the selected Markdown document stays mounted").toBe(original.document);
        expect(current.paragraph, "the selected paragraph stays mounted").toBe(original.paragraph);
        expect(current.document.dataset.markdownDocumentId).toBe(original.document.dataset.markdownDocumentId);
        expect(current.row.dataset.conversationRowId).toBe(original.row.dataset.conversationRowId);
        expect(original.paragraph.isConnected).toBe(true);
        expect(window.getSelection()!.toString()).toBe(selectedText);
        expect(currentReply.row, "the following assistant row stays mounted").toBe(originalReply.row);
        expect(currentReply.document).toBe(originalReply.document);
        expect(documents(result.container)).toHaveLength(4);
      });
    }

    for (const [label, scope] of [
      ["interaction", { interactionId: otherInteraction }],
      ["session", { sessionId: "fork-session" }],
      ["runtime", { runtimeKey: "other-runtime" }],
      ["identity", { identity: "other-worker" }],
    ] as const) {
      it(`keeps equal inputs in another ${label} separate through hydration`, () => {
        const text = "Identical instructions in distinct occurrences.";
        const initial = [history("first-saved", text), history("second-saved", text, scope)];
        const result = render(view(initial));
        const original = documents(result.container);
        expect(original).toHaveLength(2);
        expect(new Set(original.map(node => node.dataset.markdownDocumentId)).size).toBe(2);
        const rows = original.map(node => node.closest("[data-conversation-row-id]"));
        const frames = [send("first-send", text), ...initial, send("second-send", text, scope)];
        result.rerender(view(frames));
        const updated = documents(result.container);
        expect(updated).toHaveLength(2);
        expect(project(frames).map(entry => entry.id)).toEqual(["first-send", "second-send"]);
        updated.forEach((node, index) => {
          expect(node.closest<HTMLElement>("[data-quote-message-id]")!.dataset.quoteMessageId).toBe(index ? "second-send" : "first-send");
          expect(node).toBe(original[index]);
          expect(node.closest("[data-conversation-row-id]")).toBe(rows[index]);
        });
      });
    }

    for (const [label, extra] of [
      ["missing interaction", { interactionId: undefined }],
      ["empty interaction", { interactionId: "" }],
      ["legacy interaction", { interactionId: "console-interaction-legacy" }],
      ["missing session", { sessionId: undefined }],
      ["empty session", { sessionId: " " }],
    ] as const) {
      it(`keeps the source-frame fallback for ${label}`, () => {
        const frames = [history("legacy-input", "Legacy input remains readable.", extra)];
        const entry = project(frames)[0];
        expect(entry.id).toBe("legacy-input");
        expect(entry.renderKey).toBeUndefined();
        const result = render(view(frames));
        const original = documents(result.container)[0];
        expect(original.dataset.markdownDocumentId).toBe("legacy-input:text:0");
        result.rerender(view(frames));
        expect(documents(result.container)[0]).toBe(original);
        expect(original.closest<HTMLElement>("[data-quote-message-id]")!.dataset.quoteMessageId).toBe("legacy-input");
      });
    }

    function realtime(id: string, rowSequence: number, invalid = false): ConsoleFrame {
      const input = history(id, "The same spoken instruction.");
      input.data = { content: "The same spoken instruction.", message: { role: "user", content: "The same spoken instruction.",
        identity: { interaction_id: interaction, realtime_origin: invalid ? "malformed" : {
          session_id: "session", channel_id: "voice-channel", canonical_row_sequence: rowSequence,
          provider_item_ids: [`speech-${rowSequence}`],
        } } } };
      return input;
    }

    it("keeps distinct realtime rows in one interaction separate when source frames refresh", () => {
      const result = render(view([realtime("speech-a", 2), realtime("speech-b", 3)]));
      const original = documents(result.container);
      expect(original).toHaveLength(2);
      expect(new Set(original.map(node => node.dataset.markdownDocumentId)).size).toBe(2);
      const refreshed = [realtime("speech-a-refreshed", 2), realtime("speech-b-refreshed", 3)];
      result.rerender(view(refreshed));
      expect(project(refreshed).map(entry => entry.id)).toEqual(["speech-a-refreshed", "speech-b-refreshed"]);
      const updated = documents(result.container);
      expect(updated).toHaveLength(2);
      updated.forEach((node, index) => {
        expect(node.closest<HTMLElement>("[data-quote-message-id]")!.dataset.quoteMessageId).toBe(index ? "speech-b-refreshed" : "speech-a-refreshed");
        expect(node).toBe(original[index]);
      });
    });

    it("does not use the interaction as a fallback for invalid explicit realtime origin", () => {
      const frames = [realtime("invalid-a", 2, true), realtime("invalid-b", 3, true)];
      const entries = project(frames);
      expect(entries.map(entry => entry.id)).toEqual(["invalid-a", "invalid-b"]);
      entries.forEach(entry => expect(entry.renderKey).toBeUndefined());
      const result = render(view(frames));
      expect(documents(result.container).map(node => node.dataset.markdownDocumentId)).toEqual(["invalid-a:text:0", "invalid-b:text:0"]);
    });
  });
}
