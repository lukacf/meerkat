import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { vi } from "vitest";

import {
  groupConversationTimelineEntries,
  type ConversationFlowRunEntry,
  type ConversationTimelineEntry,
  type ConversationViewState,
} from "@console-core";

import { ConversationPane } from "./conversation-pane";
import { ChatPane } from "../../../../console/src/panels/ChatPane";

function Icon({ name }: { name: string; className?: string }) {
  return <span>{name}</span>;
}

describe("ConversationPane", () => {
  test("renders the scroll tail inside the scrolling body and keeps it out of the footer", () => {
    const viewState: ConversationViewState = {
      conversationId: "thread-scroll-tail",
      entries: [],
      groups: [],
      turnDiff: null,
      emptyState: null,
    };

    render(
      <ConversationPane
        footer={<div data-testid="pane-footer-content">Footer</div>}
        scrollTail={<div data-testid="pane-scroll-tail">Scroll tail</div>}
        viewState={viewState}
      />,
    );

    const scrollTail = screen.getByTestId("pane-scroll-tail");
    const scrollContainer = scrollTail.closest(".cc-conversation-pane__scroll");
    const body = scrollTail.closest(".cc-conversation-pane__body");
    const footer = screen.getByTestId("pane-footer-content").closest(".cc-conversation-pane__footer");

    expect(scrollContainer).toContainElement(scrollTail);
    expect(body?.lastElementChild).toBe(scrollTail);
    expect(footer).not.toContainElement(scrollTail);
  });

  test("restores the specific flow-run card that was clicked", () => {
    const releaseCrew: ConversationFlowRunEntry = {
      id: "flow-run:release-crew",
      kind: "flow_run",
      identity: { id: "coordinator", label: "Coordinator", role: "assistant" },
      helperId: "helper-release",
      flowName: "Release crew",
      status: "stopped",
      restorable: true,
      rows: [],
    };
    const reviewCrew: ConversationFlowRunEntry = {
      id: "flow-run:review-crew",
      kind: "flow_run",
      identity: { id: "coordinator", label: "Coordinator", role: "assistant" },
      helperId: "helper-review",
      flowName: "Review crew",
      status: "stopped",
      restorable: true,
      rows: [],
    };
    const entries: ConversationTimelineEntry[] = [releaseCrew, reviewCrew];
    const onFlowRunRestore = vi.fn();

    render(
      <ConversationPane
        onFlowRunRestore={onFlowRunRestore}
        viewState={{
          conversationId: "thread-restorable-crews",
          entries,
          groups: groupConversationTimelineEntries(entries),
          turnDiff: null,
          emptyState: null,
        }}
      />,
    );

    const restoreButtons = screen.getAllByRole("button", { name: "Resume" });
    expect(restoreButtons).toHaveLength(2);

    fireEvent.click(restoreButtons[0]);
    fireEvent.click(restoreButtons[1]);

    expect(onFlowRunRestore).toHaveBeenNthCalledWith(1, "helper-release", releaseCrew);
    expect(onFlowRunRestore).toHaveBeenNthCalledWith(2, "helper-review", reviewCrew);
  });

  test("falls back to the empty state when turn diffs cannot be rendered", () => {
    const viewState: ConversationViewState = {
      conversationId: "thread-1",
      title: "New thread",
      entries: [],
      groups: [],
      turnDiff: {
        fileCount: 1,
        plus: 12,
        minus: 3,
        files: [{
          path: "desktop/renderer/src/app/App.tsx",
          plus: 12,
          minus: 3,
          hunks: [],
        }],
      },
      emptyState: {
        title: "New thread",
        subtitle: "Ask Meerkat to do something in this workspace.",
        projectLabel: "workspace",
        iconName: "i-cube",
        suggestions: [],
      },
    };

    render(<ConversationPane Icon={Icon} viewState={viewState} />);

    expect(screen.getByText("New thread")).toBeInTheDocument();
    expect(screen.getByText("Ask Meerkat to do something in this workspace.")).toBeInTheDocument();
  });

  test("renders turn rail markers with previews and jumps to a selected turn", () => {
    const scrollIntoView = vi.fn();
    const previousScrollIntoView = HTMLElement.prototype.scrollIntoView;
    HTMLElement.prototype.scrollIntoView = scrollIntoView;

    const entries: ConversationTimelineEntry[] = [
      {
        id: "user-1",
        kind: "message",
        variant: "plain",
        identity: { id: "user", label: "You", role: "user" },
        text: "Review the contract.",
      },
      {
        id: "assistant-1",
        kind: "message",
        variant: "plain",
        identity: { id: "assistant", label: "Assistant", role: "assistant" },
        text: "Done. I left a focused finding.",
      },
      {
        id: "summary-1",
        kind: "summary",
        identity: { id: "assistant", label: "Assistant", role: "assistant" },
        title: "2 files changed",
        plus: 10,
        minus: 1,
        files: [
          { name: "agent-memory-architecture.mdx", plus: 8, minus: 0 },
          { name: "docs.json", plus: 2, minus: 1 },
        ],
      },
      {
        id: "user-2",
        kind: "message",
        variant: "plain",
        identity: { id: "user", label: "You", role: "user" },
        text: "Write up the design.",
      },
      {
        id: "assistant-2",
        kind: "message",
        variant: "plain",
        identity: { id: "assistant", label: "Assistant", role: "assistant" },
        text: "Done. I added the design page.",
      },
    ];

    try {
      render(
        <ConversationPane
          Icon={Icon}
          viewState={{
            conversationId: "thread-rail",
            entries,
            groups: groupConversationTimelineEntries(entries),
            turnDiff: null,
            emptyState: null,
          }}
        />,
      );

      expect(screen.getByRole("navigation", { name: "Conversation turns" })).toBeInTheDocument();
      // The rail preview repeats the opening user line, so the text appears in
      // both the preview card and the transcript itself.
      expect(screen.getAllByText("Review the contract.").length).toBeGreaterThanOrEqual(1);
      expect(screen.getAllByText("Done. I left a focused finding.").length).toBeGreaterThanOrEqual(1);
      expect(screen.getAllByText("agent-memory-architecture.mdx").length).toBeGreaterThanOrEqual(1);
      expect(screen.getAllByText("docs.json").length).toBeGreaterThanOrEqual(1);

      fireEvent.click(screen.getByTestId("conversation-turn-rail:1"));

      expect(scrollIntoView).not.toHaveBeenCalled();
    } finally {
      HTMLElement.prototype.scrollIntoView = previousScrollIntoView;
    }
  });
});

test("passes explicit Markdown link policy through the shared pane", () => {
  const entry: ConversationTimelineEntry = {
    id: "link", kind: "message", variant: "rich",
    identity: { id: "agent", label: "Agent", role: "assistant" },
    blocks: [{ type: "markdown", id: "link-document", source: "[Local file](./README.md)", streaming: false }],
  };
  render(<ConversationPane markdownUrlPolicy={{ resolveLink: (url) => url === "./README.md" ? "https://example.com/README.md" : null }} viewState={{ conversationId: "policy", entries: [entry], groups: groupConversationTimelineEntries([entry]), turnDiff: null, emptyState: null }} />);
  expect(screen.getByRole("link", { name: "Local file" })).toHaveAttribute("href", "https://example.com/README.md");
});

describe("conversation quote and approval integration", () => {
  const entry = (id: string, text: string): ConversationTimelineEntry => ({ id, kind: "message", variant: "plain", identity: { id: "agent", label: "Agent", role: "assistant" }, interactionId: `interaction:${id}`, text });
  const state = (entries: ConversationTimelineEntry[]): ConversationViewState => ({ conversationId: "conversation", entries, groups: groupConversationTimelineEntries(entries), turnDiff: null, emptyState: null });
  test("quotes only a single message and rejects cross-message selection", () => {
    const onQuoteSelection = vi.fn();
    render(<ConversationPane viewState={state([entry("one", "First message"), entry("two", "Second message")])} onQuoteSelection={onQuoteSelection} />);
    const first = screen.getByText("First message").firstChild!;
    const second = screen.getByText("Second message").firstChild!;
    const range = document.createRange();
    range.setStart(first, 0); range.setEnd(first, 5);
    window.getSelection()!.removeAllRanges(); window.getSelection()!.addRange(range); fireEvent(document, new Event("selectionchange"));
    fireEvent.click(screen.getByRole("button", { name: "Add to message" }));
    expect(onQuoteSelection).toHaveBeenCalledWith({ text: "First", messageId: "one", sourceText: "First message" });
    range.setEnd(second, 6);
    window.getSelection()!.addRange(range); fireEvent(document, new Event("selectionchange"));
    fireEvent.click(screen.getByRole("button", { name: "Add to message" }));
    expect(screen.getByRole("alert")).toHaveTextContent("Select text from one message at a time.");
    expect(onQuoteSelection).toHaveBeenCalledTimes(1);
    window.getSelection()!.removeAllRanges();
  });
  test("places only exact provenance approvals and shares decision state", async () => {
    const { normalizePendingApproval } = await import("../../../console-core/src/pending-approvals");
    const request = (id: string, origin?: object) => normalizePendingApproval({ pending_id: id, action: `Approve ${id}`, action_id: "filesystem.write", actor_id: "agent", origin })!;
    const snapshot = { scopeKey: "test", status: "ready" as const, readOnly: false, decisions: {}, requests: [
      request("matching", { identity: "agent", conversation_id: "conversation", interaction_id: "interaction:one" }),
      request("conversation", { identity: "agent", conversation_id: "conversation" }),
      request("unattributed"), request("wrong", { identity: "other", interaction_id: "interaction:one" }),
    ] };
    const decide = vi.fn();
    render(<ConversationPane viewState={state([entry("one", "First message")])} approvalIdentity="agent" approvalSnapshot={snapshot} onApprovalDecision={decide} />);
    const card = screen.getByTestId("gating-pending:matching");
    expect(card.closest(".cc-conversation-turn")).toBeTruthy();
    expect(screen.getByTestId("gating-pending:conversation").closest(".cc-conversation-turn")).toBeNull();
    expect(screen.queryByTestId("gating-pending:unattributed")).toBeNull();
    expect(screen.queryByTestId("gating-pending:wrong")).toBeNull();
    fireEvent.click(screen.getByTestId("gating-action:matching:approve"));
    expect(decide).toHaveBeenCalledWith("matching", "approve");
  });
});


describe("bounded conversation turn navigation", () => {
  const history = (count: number): ConversationViewState => {
    const entries: ConversationTimelineEntry[] = Array.from({ length: count }, (_, index): ConversationTimelineEntry[] => [
      { id: `rail-user-${index}`, kind: "message", variant: "plain",
        identity: { id: "operator", label: "Operator", role: "user" }, text: `History checkpoint ${index}` },
      { id: `rail-reply-${index}`, kind: "message", variant: "plain",
        identity: { id: "agent", label: "Agent", role: "assistant" }, text: `Reply ${index}` },
    ]).flat();
    return { conversationId: "long-history", entries, groups: groupConversationTimelineEntries(entries), turnDiff: null, emptyState: null };
  };

  test("keeps a bounded rail while making every retained turn reachable in both directions", () => {
    render(<ConversationPane viewState={history(127)} />);
    const rail = screen.getByRole("navigation", { name: "Conversation turns" });
    const sourceIndexes = () => [...rail.querySelectorAll('[data-testid^="conversation-turn-rail:"]')]
      .map(node => Number(node.getAttribute("data-testid")!.split(":").at(-1)));
    const visited = new Set(sourceIndexes());
    expect(within(rail).getAllByRole("button").length).toBeLessThanOrEqual(48);
    expect(sourceIndexes()).toContain(126);
    let pages = 0;
    while (within(rail).queryByRole("button", { name: "Show earlier turns" })) {
      fireEvent.click(within(rail).getByRole("button", { name: "Show earlier turns" }));
      sourceIndexes().forEach(index => visited.add(index));
      expect(within(rail).getAllByRole("button").length).toBeLessThanOrEqual(48);
      expect(++pages).toBeLessThan(10);
    }
    expect([...visited].sort((a, b) => a - b)).toEqual(Array.from({ length: 127 }, (_, index) => index));
    expect(sourceIndexes()).toContain(0);
    while (within(rail).queryByRole("button", { name: "Show later turns" })) {
      fireEvent.click(within(rail).getByRole("button", { name: "Show later turns" }));
      expect(++pages).toBeLessThan(20);
    }
    expect(sourceIndexes()).toContain(126);
    expect(screen.getByText("History checkpoint 0", { selector: "p" })).toBeInTheDocument();
  });

  test("shrinks the rail tick budget to its measured band without removing transcript rows", () => {
    const previous = globalThis.ResizeObserver;
    const observations: Array<{ callback: ResizeObserverCallback; nodes: Set<Element> }> = [];
    globalThis.ResizeObserver = class {
      item: { callback: ResizeObserverCallback; nodes: Set<Element> };
      constructor(callback: ResizeObserverCallback) { this.item = { callback, nodes: new Set() }; observations.push(this.item); }
      observe(node: Element) { this.item.nodes.add(node); }
      unobserve(node: Element) { this.item.nodes.delete(node); }
      disconnect() { this.item.nodes.clear(); }
    } as unknown as typeof ResizeObserver;
    try {
      render(<ConversationPane viewState={history(127)} />);
      const rail = screen.getByRole("navigation", { name: "Conversation turns" });
      const observer = observations.find(item => item.nodes.has(rail));
      expect(observer).toBeDefined();
      // Observer callbacks are browser events; run through React's act boundary.
      act(() => observer!.callback([{ target: rail, contentRect: { height: 100 } } as ResizeObserverEntry], {} as ResizeObserver));
      expect(within(rail).getAllByRole("button").length).toBeLessThanOrEqual(8);
      expect(document.querySelectorAll("[data-conversation-row-id]")).toHaveLength(254);
    } finally { globalThis.ResizeObserver = previous; }
  });
});

describe("pane navigation with separate source and presentation IDs", () => {
  const viewportSelector = ".conv__body, .cc-conversation-pane__scroll";
  const rect = (top: number, height: number) => ({ top, bottom: top + height, height, left: 0, right: 600,
    width: 600, x: 0, y: top, toJSON() {} });
  const noop = () => {};
  const entries = (): ConversationTimelineEntry[] => Array.from({ length: 10 }, (_, index) => ({
    id: `source-${index}`, renderKey: `render-${index}`, kind: "message", variant: "plain",
    identity: index % 2 === 0 ? { id: "user", label: "You", role: "user" } : { id: "agent", label: "Agent", role: "assistant" },
    text: `Transcript row ${index}`,
  }));
  const state = (rows: ConversationTimelineEntry[]): ConversationViewState => ({
    conversationId: "source-id-consumers", entries: rows, groups: groupConversationTimelineEntries(rows), turnDiff: null, emptyState: null,
  });
  function pane(surface: "stock" | "shared", rows: ConversationTimelineEntry[], submittedRowId?: string) {
    return surface === "shared" ? <ConversationPane viewState={state(rows)} submittedRowId={submittedRowId} />
      : <ChatPane agent={null} agentLabel="Agent" identity="agent" entries={rows} phase={null} draft="Unsent draft"
          sending={false} staged={[]} onDraftChange={noop} onStagedChange={noop} onSend={() => false} submittedRowId={submittedRowId} />;
  }
  function viewport(container: HTMLElement): HTMLElement {
    return container.querySelector<HTMLElement>(viewportSelector)!;
  }
  function userScroll(node: HTMLElement, top: number) {
    node.scrollTop = top;
    fireEvent.scroll(node);
  }

  beforeEach(() => {
    vi.spyOn(Element.prototype, "scrollHeight", "get").mockImplementation(function (this: Element) {
      return this.matches(viewportSelector) ? this.querySelectorAll("[data-conversation-row-id]").length * 100 : 0;
    });
    vi.spyOn(Element.prototype, "clientHeight", "get").mockImplementation(function (this: Element) {
      return this.matches(viewportSelector) ? 200 : 0;
    });
    vi.spyOn(Element.prototype, "getBoundingClientRect").mockImplementation(function (this: Element) {
      if (this.matches(viewportSelector)) return rect(0, 200);
      const scrollNode = this.closest<HTMLElement>(viewportSelector);
      if (scrollNode && this.matches("[data-conversation-row-id]")) {
        const index = [...scrollNode.querySelectorAll("[data-conversation-row-id]")].indexOf(this);
        return rect(index * 100 - scrollNode.scrollTop, 100);
      }
      return rect(0, 0);
    });
  });
  afterEach(() => vi.restoreAllMocks());

  for (const surface of ["stock", "shared"] as const) {
    test(`${surface} anchors the accepted source ID at its rendered row and consumes it once`, () => {
      const rows = entries();
      const view = render(pane(surface, rows));
      const scrollNode = viewport(view.container);
      expect(scrollNode.scrollTop).toBe(800);
      userScroll(scrollNode, 225);
      view.rerender(pane(surface, rows, "source-6"));
      expect(scrollNode.querySelector('[data-conversation-row-id="source-6"]')).toBeNull();
      expect(scrollNode.querySelector('[data-conversation-row-id="render-6"]')).toBeInTheDocument();
      expect(scrollNode.scrollTop, "the accepted row is anchored 24px below the viewport top").toBe(576);
      const accepted = scrollNode.querySelector('[data-conversation-row-id="render-6"]')!;
      expect(accepted.querySelector('[data-quote-message-id="source-6"]')).toBeInTheDocument();
      userScroll(scrollNode, 300);
      view.rerender(pane(surface, [...rows], "source-6"));
      expect(scrollNode.scrollTop, "an already consumed source acceptance cannot pull the reader back").toBe(300);
    });

    test(`${surface} waits for a pending source acceptance until its differently keyed row arrives`, () => {
      const rows = entries();
      const view = render(pane(surface, rows));
      const scrollNode = viewport(view.container);
      userScroll(scrollNode, 225);
      view.rerender(pane(surface, rows, "accepted-source"));
      expect(scrollNode.scrollTop).toBe(225);
      const accepted: ConversationTimelineEntry = { id: "accepted-source", renderKey: "accepted-render", kind: "message", variant: "plain",
        identity: { id: "user", label: "You", role: "user" }, text: "Newly accepted instruction" };
      const reply: ConversationTimelineEntry = { id: "new-reply", renderKey: "reply-render", kind: "message", variant: "plain",
        identity: { id: "agent", label: "Agent", role: "assistant" }, text: "The new reply" };
      view.rerender(pane(surface, [...rows, accepted, reply], "accepted-source"));
      expect(scrollNode.scrollTop).toBe(976);
      expect(scrollNode.querySelector('[data-conversation-row-id="accepted-render"]')!.getBoundingClientRect().top).toBe(24);
    });

    test(`${surface} lets an explicit scroll cancel an acceptance before its row arrives`, () => {
      const rows = entries();
      const view = render(pane(surface, rows));
      const scrollNode = viewport(view.container);
      userScroll(scrollNode, 225);
      view.rerender(pane(surface, rows, "accepted-source"));
      expect(scrollNode.scrollTop).toBe(225);
      userScroll(scrollNode, 300);
      const accepted: ConversationTimelineEntry = { id: "accepted-source", renderKey: "accepted-render", kind: "message", variant: "plain",
        identity: { id: "user", label: "You", role: "user" }, text: "Newly accepted instruction" };
      const reply: ConversationTimelineEntry = { id: "new-reply", renderKey: "reply-render", kind: "message", variant: "plain",
        identity: { id: "agent", label: "Agent", role: "assistant" }, text: "The new reply" };
      view.rerender(pane(surface, [...rows, accepted, reply], "accepted-source"));
      expect(scrollNode.querySelector('[data-conversation-row-id="accepted-render"]')).toBeInTheDocument();
      expect(scrollNode.scrollTop, "the user's later scroll cancels the pending acceptance").toBe(300);
      expect(scrollNode.querySelector('[data-conversation-row-id="render-3"]')!.getBoundingClientRect().top).toBe(0);
    });

    test(`${surface} preserves source-only acceptance for legacy entries`, () => {
      const rows = entries().map(({ renderKey: _renderKey, ...entry }) => entry);
      const view = render(pane(surface, rows));
      const scrollNode = viewport(view.container);
      userScroll(scrollNode, 225);
      view.rerender(pane(surface, rows, "source-6"));
      expect(scrollNode.scrollTop).toBe(576);
      expect(scrollNode.querySelector('[data-conversation-row-id="source-6"]')!.getBoundingClientRect().top).toBe(24);
    });

    test(`${surface} accepts an explicit presentation row ID`, () => {
      const rows = entries();
      const view = render(pane(surface, rows));
      const scrollNode = viewport(view.container);
      userScroll(scrollNode, 225);
      view.rerender(pane(surface, rows, "render-6"));
      expect(scrollNode.scrollTop).toBe(576);
      expect(scrollNode.querySelector('[data-conversation-row-id="render-6"]')!.getBoundingClientRect().top).toBe(24);
    });

    test(`${surface} does not reconsume an accepted source ID when cleared rows rehydrate`, () => {
      const rows = entries();
      const view = render(pane(surface, rows, "source-6"));
      const scrollNode = viewport(view.container);
      expect(scrollNode.scrollTop).toBe(576);
      userScroll(scrollNode, 300);
      view.rerender(pane(surface, [], "source-6"));
      expect(scrollNode.querySelectorAll("[data-conversation-row-id]")).toHaveLength(0);
      view.rerender(pane(surface, rows, "source-6"));
      expect(scrollNode.scrollTop, "rehydration cannot treat the same source acceptance as a new send").toBe(300);
      expect(scrollNode.querySelector('[data-conversation-row-id="render-3"]')!.getBoundingClientRect().top).toBe(0);
    });
  }

  test("shared turn navigation targets the presentation row while retaining quote provenance", () => {
    const view = render(pane("shared", entries()));
    const scrollNode = viewport(view.container);
    const outerScroll = vi.fn();
    const previous = HTMLElement.prototype.scrollIntoView;
    HTMLElement.prototype.scrollIntoView = outerScroll;
    try {
      fireEvent.click(within(view.container).getByTestId("conversation-turn-rail:2"));
      expect(scrollNode.scrollTop).toBe(376);
      const target = scrollNode.querySelector('[data-conversation-row-id="render-4"]')!;
      expect(target.getBoundingClientRect().top).toBe(24);
      expect(target.querySelector('[data-quote-message-id="source-4"]')).toBeInTheDocument();
      expect(view.container.textContent).not.toContain("Earlier position is unavailable");
      expect(outerScroll).not.toHaveBeenCalled();
    } finally { HTMLElement.prototype.scrollIntoView = previous; }
  });
});
