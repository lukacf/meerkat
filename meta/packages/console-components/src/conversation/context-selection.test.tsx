import { useRef } from "react";
import { QuoteSelectionAction } from "./quote-selection-action";
import { JumpToLatest } from "./jump-to-latest";
import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { readConsoleQuoteSelection } from "./context-selection";
import { QuoteContextChips } from "./context-chips";
import { editConsoleContextQuote } from "../../../console-core/src/context-edit";
import { createConsoleContextRecord } from "../../../console-core/src/context-record";
import { ConversationPane } from "./conversation-pane";
import { ChatPane } from "../../../../console/src/panels/ChatPane";
import { ConversationMessageView } from "./conversation-message-view";
import { mapFramesToTimelineEntries as sharedMapper } from "../../../console-core/src/adapters";
import { mapFramesToTimelineEntries as stockMapper } from "../../../../console/src/lib/adapters";
afterEach(() => { cleanup(); window.getSelection()?.removeAllRanges(); });
const select = (start: Node, end = start) => {
  const range = document.createRange(); range.setStart(start, 0); range.setEnd(end, end.textContent!.length);
  const selection = window.getSelection()!; selection.removeAllRanges(); selection.addRange(range); return selection;
};
describe("transcript quote selection", () => {
  it("keeps canonical assistant source IDs through both quote selection surfaces", () => {
    const source = "  A\u030A and 🚀\n";
    const common = { sessionId: "source-session", runtimeKey: "source-runtime", identity: "router:main", interactionId: "one-interaction", runId: "one-run" };
    for (const mapper of [sharedMapper, stockMapper]) {
      const entries = mapper(null, [
        { ...common, id: "provisional-frame", event: "text_delta", data: { assistant_message_id: "assistant-occurrence", delta: "provisional source" } },
        { ...common, id: "canonical-frame", event: "text_complete", sourceKind: "session_history", data: {
          assistant_message_id: "assistant-occurrence", text: source,
          message: { role: "block_assistant", assistant_message_id: "assistant-occurrence", blocks: [{ block_type: "text", data: { text: source } }] },
        } },
      ], { renderTextDeltas: true, textMode: "markdown" });
      expect(entries).toHaveLength(1);
      for (const component of [
        <ChatPane identity="router:main" agentLabel="Router" agent={null} entries={entries} phase={null} draft="" sending={false} staged={[]} onDraftChange={vi.fn()} onStagedChange={vi.fn()} onSend={vi.fn()} />,
        <ConversationMessageView entry={entries[0]} Icon={() => null} />,
      ]) {
        const view = render(component);
        const quoteWrappers = view.container.querySelectorAll<HTMLElement>("[data-quote-message-id]");
        expect(quoteWrappers).toHaveLength(1);
        const wrapper = quoteWrappers[0];
        expect(wrapper.dataset.quoteMessageId).toBe("canonical-frame");
        expect(wrapper.dataset.quoteSource).toBe(source);
        const paragraph = wrapper.querySelector("p")!;
        const result = readConsoleQuoteSelection(view.container, select(paragraph.firstChild!));
        expect(result).toEqual({ kind: "selected", quote: { messageId: "canonical-frame", sourceText: source, text: "A\u030A and 🚀" } });
        if (result.kind !== "selected") throw new Error("expected canonical quote selection");
        const record = createConsoleContextRecord({ id: "selected", sourceScope: "source-runtime", sourceIdentity: "router:main", label: "Router", quote: result.quote.text, messageId: result.quote.messageId, sourceText: result.quote.sourceText });
        expect(record.messageId).not.toBe("assistant-occurrence");
        expect(record.sourceRange).toEqual({ start: 2, end: 2 + result.quote.text.length, unit: "utf16" });
        window.getSelection()?.removeAllRanges();
        view.unmount();
      }
    }
  });
  it("captures a single opted-in message and exact rendered unicode text", () => {
    const { container } = render(<div data-quote-message-id="msg" data-quote-source="🌳 café">🌳 café</div>);
    const result = readConsoleQuoteSelection(container, select(container.firstChild!.firstChild!));
    expect(result).toEqual({ kind: "selected", quote: { text: "🌳 café", messageId: "msg", sourceText: "🌳 café" } });
  });
  it("rejects selection spanning messages and ignores navigation", () => {
    const { container } = render(<><p data-quote-message-id="a">first</p><p data-quote-message-id="b">second</p><nav>outside</nav></>);
    expect(readConsoleQuoteSelection(container, select(container.children[0].firstChild!, container.children[1].firstChild!))).toEqual({ kind: "rejected", message: "Select text from one message at a time." });
    expect(readConsoleQuoteSelection(container, select(container.children[2].firstChild!))).toEqual({ kind: "empty" });
  });
  it("excludes embedded metadata and controls", () => {
    const { container } = render(<p data-quote-message-id="a">hello<span data-quote-exclude="">tool metadata</span>world</p>);
    expect(readConsoleQuoteSelection(container, select(container.firstChild!.firstChild!, container.firstChild!.lastChild!)).kind).toBe("rejected");
  });
  it("shows destination, removable source chip and exact snapshot without sending", () => {
    const context = createConsoleContextRecord({ id: "q", sourceScope: "s", sourceIdentity: "agent", messageId: "a", label: "Source agent", quote: "exact  snapshot\nline" });
    const onRemove = vi.fn(); render(<QuoteContextChips records={[context]} destinationLabel="Target agent" onRemove={onRemove} />);
    expect(screen.getByRole("region", { name: "Quoted context for Target agent" })).toBeTruthy();
    expect(screen.getByText("Source agent")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Remove quote from Source agent" }));
    expect(onRemove).toHaveBeenCalledWith("q");
  });
  it("keeps the reusable composer clear when no transcript text is selected", () => {
    const viewState = { conversationId: "quote-feedback", entries: [], groups: [], emptyState: null, turnDiff: null };
    render(<ConversationPane viewState={viewState} onQuoteSelection={vi.fn()} footer={<textarea aria-label="Message" />} />);
    expect(screen.queryByRole("button", { name: "Add to message" })).toBeNull();
  });
  it("keeps the stock composer clear when no transcript text is selected", () => {
    render(<ChatPane identity="router:main" agentLabel="Router" agent={null} entries={[]} phase={null} draft="" sending={false}
      staged={[]} onDraftChange={vi.fn()} onStagedChange={vi.fn()} onSend={vi.fn()} onQuoteSelection={vi.fn()} />);
    expect(screen.queryByRole("button", { name: "Add to message" })).toBeNull();
  });
});

function SelectionFixture({ disabled = false, onQuote = vi.fn(), onError = vi.fn() }) {
  const viewportRef = useRef<HTMLDivElement>(null);
  return <div><div ref={viewportRef} data-testid="quote-viewport"><p data-quote-message-id="one" data-quote-source="🌳 café">🌳 café</p><p data-quote-message-id="two">second</p></div>
    <p data-testid="other-pane" data-quote-message-id="outside">another pane</p>
    <QuoteSelectionAction viewportRef={viewportRef} onQuote={onQuote} onError={onError} disabled={disabled} /></div>;
}
function announceSelection(start: Node, end = start) { select(start, end); fireEvent(document, new Event("selectionchange")); }
describe("contextual transcript controls", () => {
  it("keeps compact stock header actions named, target-specific and operable with custom labels", () => {
    const onInspect = vi.fn(), onRespawn = vi.fn(), onRetire = vi.fn();
    const agent = { agent_id: "router", member_id: "router", label: "Router", kind: "persistent", affordances: { can_respawn: true, can_retire: true } };
    const view = render(<ChatPane identity="router:main" agentLabel="Router" agent={agent} entries={[]} phase={null}
      draft="" sending={false} staged={[]} onDraftChange={vi.fn()} onStagedChange={vi.fn()} onSend={vi.fn()}
      headerVariant="compact" onInspect={onInspect} onRespawn={onRespawn} onRetire={onRetire}
      inspectLabel="Inspect agent" respawnLabel="Restart agent" retireLabel="Retire agent" />);
    expect(view.container.querySelector(".conv__title")).toHaveAttribute("title", "router:main");
    for (const [label, icon, action] of [
      ["Inspect agent", "i-info", onInspect], ["Restart agent", "i-refresh", onRespawn], ["Retire agent", "i-archive", onRetire],
    ] as const) {
      const button = screen.getByRole("button", { name: label, exact: true });
      expect(button).toHaveAttribute("aria-label", label);
      expect(button).toHaveAttribute("title", `${label} - router:main`);
      expect(button).toHaveAttribute("type", "button");
      expect(button.querySelector('[aria-hidden="true"] use')).toHaveAttribute("href", `#${icon}`);
      button.focus(); expect(button).toHaveFocus(); fireEvent.click(button); expect(action).toHaveBeenCalledOnce();
    }
    view.rerender(<ChatPane identity="router:main" agentLabel="Router" agent={{ ...agent, affordances: {} }} entries={[]} phase={null}
      draft="" sending={false} staged={[]} onDraftChange={vi.fn()} onStagedChange={vi.fn()} onSend={vi.fn()}
      onInspect={onInspect} onRespawn={onRespawn} onRetire={onRetire} />);
    expect(screen.getByRole("button", { name: "Details", exact: true })).toBeTruthy();
    expect(screen.queryByTestId("conv-action:respawn")).toBeNull();
    expect(screen.queryByTestId("conv-action:retire")).toBeNull();
  });
  it("keeps the stock turn rail clear of the latest control when the composer grows", () => {
    const previous = globalThis.ResizeObserver;
    const observations: Array<{ callback: ResizeObserverCallback; nodes: Set<Element> }> = [];
    globalThis.ResizeObserver = class {
      item: { callback: ResizeObserverCallback; nodes: Set<Element> };
      constructor(callback: ResizeObserverCallback) { this.item = { callback, nodes: new Set() }; observations.push(this.item); }
      observe(node: Element) { this.item.nodes.add(node); }
      unobserve(node: Element) { this.item.nodes.delete(node); }
      disconnect() { this.item.nodes.clear(); }
    } as unknown as typeof ResizeObserver;
    let transcriptBottom = 800;
    const bounds = vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement) {
      const top = this.classList.contains("conv") ? 100 : this.classList.contains("conv__body") ? 160 : 0;
      const bottom = this.classList.contains("conv") ? 1000 : this.classList.contains("conv__body") ? transcriptBottom : 0;
      return { x: 0, y: top, top, bottom, left: 0, right: 600, width: 600, height: bottom - top, toJSON() {} };
    });
    try {
      const entries = ["first request", "first response", "second request", "second response"].map((text, index) => ({
        id: `gutter-${index}`, kind: "message" as const, variant: "plain" as const,
        identity: { id: index % 2 ? "agent" : "user", label: index % 2 ? "Agent" : "You", role: index % 2 ? "assistant" as const : "user" as const },
        text, createdAt: `2026-09-26T00:00:0${index}.000Z`,
      }));
      const view = render(<ChatPane identity="router:main" agentLabel="Router" agent={null} entries={entries} phase={null}
        draft="" sending={false} staged={[]} onDraftChange={vi.fn()} onStagedChange={vi.fn()} onSend={vi.fn()} />);
      const rail = view.container.querySelector<HTMLElement>(".conv-turn-rail")!;
      const body = view.container.querySelector<HTMLElement>(".conv__body")!;
      const observer = observations.find(item => item.nodes.has(rail));
      expect(observer?.nodes.has(body)).toBe(true);
      expect(observer?.nodes.has(body.parentElement!)).toBe(true);
      expect(rail.style.top).toBe("76px");
      expect(rail.style.bottom).toBe("264px");
      transcriptBottom = 700;
      act(() => observer!.callback([{ target: body, contentRect: { height: 540 } } as ResizeObserverEntry], {} as ResizeObserver));
      expect(rail.style.bottom).toBe("364px");
      expect(view.container.querySelectorAll("[data-conversation-row-id]")).toHaveLength(4);
      view.unmount();
      expect(observer!.nodes.size).toBe(0);
    } finally { bounds.mockRestore(); globalThis.ResizeObserver = previous; }
  });
  it("shows an icon only for a local selection and adds its exact text without sending", () => {
    const onQuote = vi.fn(); render(<SelectionFixture onQuote={onQuote} />);
    expect(screen.queryByRole("button", { name: "Add to message" })).toBeNull();
    announceSelection(screen.getByText("🌳 café").firstChild!);
    const button = screen.getByRole("button", { name: "Add to message" });
    expect(button).toHaveTextContent(""); expect(button.querySelector("svg")).toBeTruthy();
    expect(onQuote).not.toHaveBeenCalled();
    fireEvent.click(button);
    expect(onQuote).toHaveBeenCalledExactlyOnceWith({ messageId: "one", sourceText: "🌳 café", text: "🌳 café" });
    expect(screen.queryByRole("button", { name: "Add to message" })).toBeNull();
    expect(window.getSelection()!.isCollapsed).toBe(true);
  });
  it("ignores other panes and disappears when selection clears, on Escape, and in read-only mode", () => {
    const view = render(<SelectionFixture />);
    announceSelection(screen.getByTestId("other-pane").firstChild!);
    expect(screen.queryByRole("button", { name: "Add to message" })).toBeNull();
    announceSelection(screen.getByText("🌳 café").firstChild!);
    expect(screen.getByRole("button", { name: "Add to message" })).toBeTruthy();
    fireEvent.keyDown(document, { key: "Escape" });
    expect(screen.queryByRole("button", { name: "Add to message" })).toBeNull();
    announceSelection(screen.getByText("🌳 café").firstChild!);
    window.getSelection()!.removeAllRanges(); fireEvent(document, new Event("selectionchange"));
    expect(screen.queryByRole("button", { name: "Add to message" })).toBeNull();
    announceSelection(screen.getByText("🌳 café").firstChild!);
    view.rerender(<SelectionFixture disabled />);
    expect(screen.queryByRole("button", { name: "Add to message" })).toBeNull();
  });
  it("retains the explicit rejection when the selection spans messages", () => {
    const onQuote = vi.fn(), onError = vi.fn(); render(<SelectionFixture onQuote={onQuote} onError={onError} />);
    announceSelection(screen.getByText("🌳 café").firstChild!, screen.getByText("second").firstChild!);
    fireEvent.click(screen.getByRole("button", { name: "Add to message" }));
    expect(onError).toHaveBeenCalledWith("Select text from one message at a time.");
    expect(onQuote).not.toHaveBeenCalled();
  });
  it("uses an accessible down-arrow icon whose activity follows the host", () => {
    const onClick = vi.fn(); const view = render(<JumpToLatest onClick={onClick} working />);
    const button = screen.getByRole("button", { name: "Jump to latest" });
    expect(button).toHaveTextContent(""); expect(button.querySelectorAll("svg")).toHaveLength(2);
    expect(button).toHaveAttribute("data-working", "true");
    fireEvent.click(button); expect(onClick).toHaveBeenCalledOnce();
    view.rerender(<JumpToLatest onClick={onClick} working={false} />);
    expect(button).toHaveAttribute("data-working", "false");
  });
});

const editableContext = createConsoleContextRecord({ id: "editable", sourceScope: "scope", sourceIdentity: "agent", messageId: "message", label: "Source", quote: "original", sourceText: "before original after" });
describe("editing user-provided quote snapshots", () => {
  it("saves exact multiline text only on explicit save, retaining metadata and removing the old source range", async () => {
    const onEdit = vi.fn();
    render(<QuoteContextChips records={[editableContext]} destinationLabel="Target" onEdit={onEdit} />);
    fireEvent.click(screen.getByRole("button", { name: "Edit quote from Source" }));
    const editor = screen.getByRole("textbox", { name: "Quote from Source" });
    expect(editor).toHaveValue("original");
    const exact = "  A\u030A and 🚀\n\n  revised snapshot  ";
    fireEvent.change(editor, { target: { value: exact } });
    expect(onEdit).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Save quote" }));
    await waitFor(() => expect(onEdit).toHaveBeenCalledExactlyOnceWith("editable", exact));
    await waitFor(() => expect(screen.queryByRole("textbox", { name: "Quote from Source" })).toBeNull());
    expect(screen.getByRole("button", { name: "Edit quote from Source" })).toHaveFocus();
    const next = editConsoleContextQuote([editableContext], "editable", exact);
    expect(next[0]).toEqual({ version: 1, id: "editable", sourceScope: "scope", sourceIdentity: "agent", messageId: "message", label: "Source", quote: exact });
    expect(editableContext.sourceRange).toEqual({ start: 7, end: 15, unit: "utf16" });
  });
  it("cancel and Escape preserve the original record without calling the host", () => {
    const onEdit = vi.fn(); render(<QuoteContextChips records={[editableContext]} destinationLabel="Target" onEdit={onEdit} />);
    for (const cancel of ["button", "escape"]) {
      fireEvent.click(screen.getByRole("button", { name: "Edit quote from Source" }));
      const editor = screen.getByRole("textbox", { name: "Quote from Source" });
      fireEvent.change(editor, { target: { value: "discard me" } });
      if (cancel === "button") fireEvent.click(screen.getByRole("button", { name: "Cancel quote edit" }));
      else fireEvent.keyDown(editor, { key: "Escape" });
      expect(screen.queryByRole("textbox")).toBeNull();
      expect(screen.getByRole("button", { name: "Edit quote from Source" })).toHaveFocus();
    }
    expect(onEdit).not.toHaveBeenCalled();
  });
  it("rejects empty and oversized aggregate snapshots before invoking the host", async () => {
    const onEdit = vi.fn(); render(<QuoteContextChips records={[editableContext]} destinationLabel="Target" onEdit={onEdit} />);
    fireEvent.click(screen.getByRole("button", { name: "Edit quote from Source" }));
    const editor = screen.getByRole("textbox", { name: "Quote from Source" });
    for (const value of ["", "🚀".repeat(20_000)]) {
      fireEvent.change(editor, { target: { value } });
      fireEvent.click(screen.getByRole("button", { name: "Save quote" }));
      await screen.findByRole("alert");
      expect(editor).toHaveValue(value);
    }
    expect(onEdit).not.toHaveBeenCalled();
    const second = { ...editableContext, id: "second", quote: "x".repeat(33_000), sourceRange: undefined };
    expect(() => editConsoleContextQuote([editableContext, second], "editable", "x".repeat(33_000))).toThrow(/64 KiB/);
  });
  it("keeps a failed async save editable and hides mutation controls for immutable snapshots", async () => {
    const onEdit = vi.fn(async () => { throw new Error("Storage unavailable"); });
    const view = render(<QuoteContextChips records={[editableContext]} destinationLabel="Target" onEdit={onEdit} />);
    fireEvent.click(screen.getByRole("button", { name: "Edit quote from Source" }));
    fireEvent.change(screen.getByRole("textbox"), { target: { value: "keep this edit" } });
    fireEvent.click(screen.getByRole("button", { name: "Save quote" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Storage unavailable");
    expect(screen.getByRole("textbox")).toHaveValue("keep this edit");
    view.rerender(<QuoteContextChips records={[editableContext]} destinationLabel="Target" />);
    expect(screen.queryByRole("textbox")).toBeNull();
    expect(screen.queryByRole("button", { name: /Edit quote/ })).toBeNull();
  });
  it("preserves list order and unaffected records and rejects stale IDs", () => {
    const second = { ...editableContext, id: "second" };
    const result = editConsoleContextQuote([editableContext, second], "second", "replacement");
    expect(result.map(record => record.id)).toEqual(["editable", "second"]);
    expect(result[0]).toBe(editableContext);
    expect(result[1].sourceRange).toBeUndefined();
    expect(editConsoleContextQuote([editableContext], "editable", "original")[0]).toBe(editableContext);
    expect(() => editConsoleContextQuote([editableContext], "gone", "replacement")).toThrow(/no longer/);
  });
});
