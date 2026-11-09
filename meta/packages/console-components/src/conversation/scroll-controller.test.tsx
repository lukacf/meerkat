import { act, fireEvent, render, screen } from "@testing-library/react";
import { useLayoutEffect, useRef } from "react";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";

import {
  CONVERSATION_POSITION_LIMIT,
  ConversationPositionCache,
  captureConversationAnchor,
  conversationIsAtEnd,
  isConversationScrollTarget,
  restoreConversationAnchor,
} from "./scroll-geometry";
import { useConversationScrollController, type ConversationScrollControllerOptions, type ConversationViewportKey } from "./scroll-controller";

describe("scroll geometry", () => {
  test("preserves a visible row through prepend and chooses its nearest retained neighbor", () => {
    const anchor = captureConversationAnchor([{ id: "a", top: -80, bottom: -20 }, { id: "b", top: -10, bottom: 60 }, { id: "c", top: 70, bottom: 100 }])!;
    expect(anchor.rowId).toBe("b");
    expect(restoreConversationAnchor(anchor, [{ id: "b", top: 240, bottom: 300 }], 100, 800)).toEqual({ rowId: "b", scrollTop: 350, exact: true });
    expect(restoreConversationAnchor(anchor, [{ id: "c", top: 130, bottom: 160 }], 100, 800)).toEqual({ rowId: "c", scrollTop: 160, exact: false });
    expect(restoreConversationAnchor(anchor, [], 100, 800)).toBeNull();
  });
  test("uses a small tested live-edge threshold and clamps overscroll", () => {
    expect(conversationIsAtEnd(168, 300, 100)).toBe(true);
    expect(conversationIsAtEnd(167, 300, 100)).toBe(false);
    expect(conversationIsAtEnd(-10, 80, 100)).toBe(true);
  });
  test("bounds position retention and clears only the previous authority", () => {
    const cache = new ConversationPositionCache();
    const position = { mode: "reading-history" as const, anchor: null, scrollTop: 20 };
    for (let i = 0; i < 101; i += 1) cache.remember(JSON.stringify(["a", i]), position);
    expect(cache.size).toBe(CONVERSATION_POSITION_LIMIT);
    expect(cache.read(JSON.stringify(["a", 0]))).toBeUndefined();
    cache.remember(JSON.stringify(["b", 0]), position);
    cache.deleteAuthority("a");
    expect(cache.size).toBe(1);
    expect(cache.read(JSON.stringify(["b", 0]))).toBe(position);
  });
  test("nested scroll surfaces consume their own vertical and horizontal gestures", () => {
    const viewport = document.createElement("div");
    const nested = document.createElement("pre");
    const target = document.createElement("code");
    viewport.append(nested); nested.append(target); document.body.append(viewport);
    nested.style.overflowY = "auto";
    Object.defineProperties(nested, { scrollHeight: { value: 400 }, clientHeight: { value: 100 } });
    nested.scrollTop = 30;
    expect(isConversationScrollTarget(target, viewport, -10)).toBe(false);
    expect(isConversationScrollTarget(target, viewport, 10)).toBe(false);
    nested.scrollTop = 0;
    expect(isConversationScrollTarget(target, viewport, -10)).toBe(true);
    expect(isConversationScrollTarget(target, viewport, 1, 20)).toBe(false);
    viewport.remove();
  });
});

const rect = (top: number, height: number) => ({ top, bottom: top + height, height, left: 0, right: 200, width: 200, x: 0, y: top, toJSON() {} });
type Row = {
  id: string; height: number; text?: string;
  /** A row inside a windowed transcript's turn element; while `parked` is
   * set the turn is hidden at that measured placeholder height, and its row
   * still lays out at `height` from the turn's top, as a parked turn's rows
   * do once the turn has outgrown its measurement. */
  parked?: number;
};
/** Layout height a row takes in the transcript: a parked turn's placeholder. */
const advance = (row: Row) => row.parked ?? row.height;
const baseRows: Row[] = Array.from({ length: 10 }, (_, i) => ({ id: `row-${i}`, height: 100 }));
/** Row rect reads by row id, for asserting how much of the transcript a layout pass measures. */
const rowReads = new Map<string, number>();
/** Resize observer callbacks, delivered by the test as a browser does after layout. */
const resizeCallbacks = new Set<() => void>();
class DeliveredResizeObserver {
  constructor(private readonly callback: () => void) {}
  observe() { resizeCallbacks.add(this.callback); }
  unobserve() {}
  disconnect() { resizeCallbacks.delete(this.callback); }
}
function deliverResize() { for (const callback of [...resizeCallbacks]) callback(); }
function Harness({ rows = baseRows, conversation = "test", viewportKey, submittedRowId, revealAnchor, revealTimeoutMs, height = 200, resizeEachCommit = true, header = 0 }: {
  rows?: Row[]; conversation?: string; viewportKey?: ConversationViewportKey; submittedRowId?: string | null; revealAnchor?: ConversationScrollControllerOptions["revealAnchor"]; revealTimeoutMs?: number; height?: number;
  /** Deliver a resize after every commit, as rows and the viewport change size here. */
  resizeEachCommit?: boolean;
  /** Height of a non-row element above the rows, absent when 0. */
  header?: number;
}) {
  const viewportRef = useRef<HTMLDivElement | null>(null);
  useLayoutEffect(() => {
    const viewport = viewportRef.current!;
    Object.defineProperties(viewport, {
      scrollHeight: { configurable: true, get: () => header + rows.reduce((sum, row) => sum + advance(row), 0) },
      clientHeight: { configurable: true, get: () => height },
    });
    viewport.getBoundingClientRect = () => rect(0, height);
    let top = header;
    for (const row of rows) {
      const node = Array.from(viewport.querySelectorAll<HTMLElement>("[data-conversation-row-id]")).find((node) => node.dataset.conversationRowId === row.id)!;
      const rowTop = top;
      node.getBoundingClientRect = () => {
        rowReads.set(row.id, (rowReads.get(row.id) ?? 0) + 1);
        return rect(rowTop - viewport.scrollTop, row.height);
      };
      // A parked turn's placeholder: its measured size, from its row's top.
      if (row.parked !== undefined) node.parentElement!.getBoundingClientRect = () => rect(rowTop - viewport.scrollTop, row.parked!);
      top += advance(row);
    }
  });
  const scroll = useConversationScrollController({ viewportRef, viewportKey, conversationId: conversation, contentVersion: rows, submittedRowId, revealAnchor, revealTimeoutMs });
  useLayoutEffect(() => { if (resizeEachCommit) deliverResize(); });
  return <>
    <div data-testid="viewport" tabIndex={0} ref={viewportRef}>{header ? <button>Load older</button> : null}{rows.map((row) => "parked" in row
      ? <div className="turn" hidden={row.parked !== undefined} key={row.id}><div data-conversation-row-id={row.id}>{row.text ?? row.id}</div></div>
      : <div data-conversation-row-id={row.id} key={row.id}>{row.text ?? row.id}</div>)}</div>
    <span data-testid="mode">{scroll.mode}</span>
    {scroll.missingAnchor ? <div role="status">Earlier position is unavailable</div> : null}
    {scroll.revealingAnchor ? <div role="status">Restoring earlier position</div> : null}
    {scroll.awayFromEnd ? <button onClick={scroll.jumpToLatest}>Jump to latest</button> : null}
    <button onClick={() => scroll.jumpToRow("row-2")}>Turn 2</button>
    <button onClick={scroll.captureBeforePrepend}>Capture before prepend</button>
    <button onClick={scroll.notifyLayoutChange}>Notify layout</button>
  </>;
}
function userScroll(viewport: HTMLElement, top: number) { viewport.scrollTop = top; fireEvent.scroll(viewport); }
beforeEach(() => { vi.stubGlobal("ResizeObserver", DeliveredResizeObserver); });
afterEach(() => { vi.restoreAllMocks(); vi.useRealTimers(); vi.unstubAllGlobals(); resizeCallbacks.clear(); });

describe("conversation scroll intent", () => {
  test("a pending browser reveal scroll after a live-edge tool expands keeps following", () => {
    const view = render(<Harness />);
    const viewport = screen.getByTestId("viewport");
    // The browser reveals a nearby control before its delayed scroll event.
    viewport.scrollTop = 770;
    fireEvent.pointerDown(viewport.querySelector('[data-conversation-row-id="row-9"]')!);
    // Native disclosure growth precedes both the pending scroll and layout.
    Object.defineProperty(viewport, "scrollHeight", { configurable: true, value: 1103 });
    fireEvent.scroll(viewport);
    expect(screen.getByTestId("mode")).toHaveTextContent("following-end");
    const expanded = [...baseRows, { id: "tool-detail", height: 103 }];
    view.rerender(<Harness rows={expanded} />);
    expect(viewport.scrollTop).toBe(903);
    fireEvent.wheel(viewport, { deltaY: -100 });
    userScroll(viewport, 803);
    view.rerender(<Harness rows={[...expanded, { id: "streamed", height: 150 }]} />);
    expect(screen.getByTestId("mode")).toHaveTextContent("reading-history");
    expect(viewport.scrollTop).toBe(803);
  });
  test("a changed native scroll after a live-edge browser reveal still pauses", () => {
    const view = render(<Harness />);
    const viewport = screen.getByTestId("viewport");
    viewport.scrollTop = 770;
    fireEvent.pointerDown(viewport.querySelector('[data-conversation-row-id="row-9"]')!);
    userScroll(viewport, 600);
    view.rerender(<Harness rows={[...baseRows, { id: "streamed", height: 150 }]} />);
    expect(screen.getByTestId("mode")).toHaveTextContent("reading-history");
    expect(viewport.scrollTop).toBe(600);
  });
  test.each(["wheel-down", "ArrowDown", "PageDown", "End", " ", "pointer"])("a no-op %s at the live edge keeps following content growth", (gesture) => {
    const view = render(<Harness />);
    const viewport = screen.getByTestId("viewport");
    const lastRow = viewport.querySelector('[data-conversation-row-id="row-9"]')!;
    if (gesture === "wheel-down") fireEvent.wheel(lastRow, { deltaY: 100 });
    else if (gesture === "pointer") fireEvent.pointerDown(lastRow);
    else fireEvent.keyDown(viewport, { key: gesture });
    expect(screen.getByTestId("mode")).toHaveTextContent("following-end");
    view.rerender(<Harness rows={[...baseRows, { id: "streamed", height: 160 }]} />);
    expect(viewport.scrollTop).toBe(960);
  });
  test("a no-op upward gesture in a short transcript keeps following", () => {
    const view = render(<Harness rows={[{ id: "one", height: 100 }]} />);
    const viewport = screen.getByTestId("viewport");
    fireEvent.wheel(viewport, { deltaY: -100 });
    fireEvent.keyDown(viewport, { key: "Home" });
    expect(screen.getByTestId("mode")).toHaveTextContent("following-end");
    view.rerender(<Harness rows={[{ id: "one", height: 450 }]} />);
    expect(viewport.scrollTop).toBe(250);
  });
  test("upward reading intent and text selection still pause at the live edge", () => {
    const view = render(<Harness />);
    const viewport = screen.getByTestId("viewport");
    fireEvent.wheel(viewport, { deltaY: -100 });
    userScroll(viewport, 600);
    view.rerender(<Harness rows={[...baseRows, { id: "streamed", height: 160 }]} />);
    expect(viewport.scrollTop).toBe(600);
    fireEvent.click(screen.getByRole("button", { name: "Jump to latest" }));
    const text = viewport.querySelector('[data-conversation-row-id="row-9"]')!.firstChild!;
    const selection = window.getSelection()!;
    const range = document.createRange(); range.selectNodeContents(text);
    selection.removeAllRanges(); selection.addRange(range);
    fireEvent(document, new Event("selectionchange"));
    expect(screen.getByTestId("mode")).toHaveTextContent("reading-history");
    selection.removeAllRanges();
  });
  test("pointer interaction captures a browser-revealed position before a focus render can restore the old anchor", () => {
    const view = render(<Harness />);
    const viewport = screen.getByTestId("viewport");
    userScroll(viewport, 225);
    // Native focus/reveal can scroll before the browser emits its scroll event.
    // A render between mouse down and up must not move the target away.
    viewport.scrollTop = 525;
    fireEvent.pointerDown(viewport.querySelector('[data-conversation-row-id="row-6"]')!);
    view.rerender(<Harness />);
    expect(viewport.scrollTop).toBe(525);
    expect(screen.getByTestId("mode")).toHaveTextContent("reading-history");
  });
  test("starts at end, then 100 content updates preserve the historical row within 2px", () => {
    const view = render(<Harness />);
    const viewport = screen.getByTestId("viewport");
    expect(viewport.scrollTop).toBe(800);
    userScroll(viewport, 225);
    expect(screen.getByTestId("mode")).toHaveTextContent("reading-history");
    for (let i = 1; i <= 100; i += 1) {
      view.rerender(<Harness rows={baseRows.map((row, index) => index === 9 ? { ...row, height: 100 + i, text: `token ${i}` } : row)} />);
      expect(Math.abs(viewport.scrollTop - 225)).toBeLessThanOrEqual(2);
    }
    fireEvent.click(screen.getByRole("button", { name: "Jump to latest" }));
    expect(viewport.scrollTop).toBe(900);
    expect(screen.getByTestId("mode")).toHaveTextContent("following-end");
  });
  test("a parked turn's row reaching into the viewport past its placeholder never holds the reading position", () => {
    // Main CI run 38511905113: a scroll put the reading heading at the top
    // of a turn right below a parked turn whose content had outgrown its
    // placeholder by 98 px. The parked turn's last row, laid out past its
    // placeholder, was the first row reaching into the viewport and became
    // the anchor; unparking moved the reader's content down 98 px while the
    // controller held that row still.
    const rows: Row[] = [
      { id: "peer-a", height: 100 },
      { id: "peer-b", height: 140, parked: 42 },
      { id: "seed-user", height: 64 },
      { id: "seed-answer", height: 2000 },
      { id: "current", height: 300 },
    ];
    const view = render(<Harness rows={rows} />);
    const viewport = screen.getByTestId("viewport");
    const seedTop = () => viewport.querySelector('[data-conversation-row-id="seed-user"]')!.getBoundingClientRect().top;
    const peerBottom = () => viewport.querySelector('[data-conversation-row-id="peer-b"]')!.getBoundingClientRect().bottom;
    // The reader lands with the seed's first row 46 px above the top, while
    // the parked row still reaches 52 px into the viewport.
    userScroll(viewport, 188);
    expect([seedTop(), peerBottom()]).toEqual([-46, 52]);
    expect(screen.getByTestId("mode")).toHaveTextContent("reading-history");
    // Unparking lays the turn out at its content height: 98 px more above.
    view.rerender(<Harness rows={rows.map((row) => row.id === "peer-b" ? { ...row, parked: undefined } : row)} />);
    expect(seedTop()).toBe(-46);
    expect(viewport.scrollTop).toBe(286);
  });
  test("a reader within a parked turn's placeholder keeps that turn's rows in place across its unpark", () => {
    // A jump can land inside a parked turn before the window mounts it. Its
    // rows already lie where they will be, so its row holds the position and
    // only what follows the grown turn moves.
    const rows: Row[] = [
      { id: "older", height: 300 },
      { id: "parked", height: 400, parked: 300 },
      { id: "after", height: 1000 },
    ];
    const view = render(<Harness rows={rows} />);
    const viewport = screen.getByTestId("viewport");
    const parkedTop = () => viewport.querySelector('[data-conversation-row-id="parked"]')!.getBoundingClientRect().top;
    userScroll(viewport, 350);
    expect(parkedTop()).toBe(-50);
    view.rerender(<Harness rows={rows.map((row) => row.id === "parked" ? { ...row, parked: undefined } : row)} />);
    expect(parkedTop()).toBe(-50);
    expect(viewport.scrollTop).toBe(350);
  });
  test("preserves the row through history prepend, earlier disclosure growth and viewport resize", () => {
    const view = render(<Harness />);
    const viewport = screen.getByTestId("viewport");
    userScroll(viewport, 225);
    fireEvent.click(screen.getByRole("button", { name: "Capture before prepend" }));
    const prepended = [{ id: "older", height: 150 }, ...baseRows];
    view.rerender(<Harness rows={prepended} />);
    expect(viewport.scrollTop).toBe(375);
    view.rerender(<Harness rows={prepended.map((row) => row.id === "row-0" ? { ...row, height: 220 } : row)} height={150} />);
    expect(viewport.scrollTop).toBe(495);
    expect(viewport.querySelector('[data-conversation-row-id="row-2"]')!.getBoundingClientRect().top).toBe(-25);
  });
  test("restores local conversation position and keeps independently mounted panes separate", () => {
    const view = render(<Harness conversation="one" />);
    const viewport = screen.getByTestId("viewport");
    userScroll(viewport, 225);
    view.rerender(<Harness conversation="two" />);
    expect(viewport.scrollTop).toBe(800);
    userScroll(viewport, 450);
    view.rerender(<Harness conversation="one" />);
    expect(viewport.scrollTop).toBe(225);
    view.unmount();
    render(<Harness conversation="one" />);
    expect(screen.getByTestId("viewport").scrollTop).toBe(800);
  });
  test("explicit pane keys retain remount position and discard the previous principal on authority change", () => {
    const key = { authority: "principal-scroll-test", identity: "agent", conversation: "one", pane: "left" };
    const view = render(<Harness viewportKey={key} />);
    userScroll(screen.getByTestId("viewport"), 250);
    view.unmount();
    const second = render(<Harness viewportKey={key} />);
    expect(screen.getByTestId("viewport").scrollTop).toBe(250);
    second.rerender(<Harness viewportKey={{ ...key, authority: "different-principal" }} />);
    expect(screen.getByTestId("viewport").scrollTop).toBe(800);
    second.rerender(<Harness viewportKey={key} />);
    expect(screen.getByTestId("viewport").scrollTop).toBe(800);
  });
  test("rail jumps enter reading mode and cannot scroll an outer document", () => {
    const outerScroll = vi.fn(); HTMLElement.prototype.scrollIntoView = outerScroll;
    render(<Harness />);
    fireEvent.click(screen.getByRole("button", { name: "Turn 2" }));
    expect(screen.getByTestId("viewport").scrollTop).toBe(176);
    expect(screen.getByTestId("mode")).toHaveTextContent("reading-history");
    expect(outerScroll).not.toHaveBeenCalled();
  });
  test("a queued/nonexistent row cannot move history, a canonical submitted row can anchor once present", () => {
    const view = render(<Harness />);
    const viewport = screen.getByTestId("viewport");
    userScroll(viewport, 225);
    view.rerender(<Harness submittedRowId="accepted" />);
    expect(viewport.scrollTop).toBe(225);
    view.rerender(<Harness submittedRowId="accepted" rows={[...baseRows, { id: "accepted", height: 100 }]} />);
    expect(viewport.scrollTop).toBe(900);
    expect(screen.getByTestId("mode")).toHaveTextContent("anchoring-submitted-turn");
    view.rerender(<Harness submittedRowId="accepted" rows={[...baseRows, { id: "accepted", height: 100 }, { id: "response", height: 100 }]} />);
    expect(viewport.scrollTop).toBe(900);
  });
  test("holding a submitted turn while its reply streams measures only the anchor row", () => {
    const view = render(<Harness />);
    const viewport = screen.getByTestId("viewport");
    const withReply = (text: string, height: number) => [...baseRows, { id: "accepted", height: 100 }, { id: "response", height, text }];
    view.rerender(<Harness submittedRowId="accepted" rows={withReply("", 50)} />);
    expect(screen.getByTestId("mode")).toHaveTextContent("anchoring-submitted-turn");
    const anchored = viewport.scrollTop;
    rowReads.clear();
    // Each streamed token is a content commit. Reading every mounted row's
    // rect on each one forced a whole-transcript layout per token.
    for (let token = 1; token <= 5; token += 1) {
      view.rerender(<Harness submittedRowId="accepted" rows={withReply("token ".repeat(token), 50 + token * 40)} />);
    }
    expect(viewport.scrollTop).toBe(anchored);
    expect([...rowReads.keys()]).toEqual(["accepted"]);
    // One pass per commit, plus at most one follow-up pass from its publish.
    expect(rowReads.get("accepted")).toBeLessThanOrEqual(10);
  });
  test("steady streaming commits read no geometry; the post-layout resize pass restores", () => {
    // Resize callbacks run after the frame's layout, so geometry read there
    // is already computed. A read in the commit's layout effect forced a
    // synchronous layout of the whole transcript on every streamed token.
    const resized = () => act(deliverResize);
    const view = render(<Harness resizeEachCommit={false} />);
    const viewport = screen.getByTestId("viewport");
    expect(viewport.scrollTop).toBe(800);
    // Following the live edge: growth lands when the observer reports it.
    view.rerender(<Harness resizeEachCommit={false} rows={baseRows.map((row, index) => index === 9 ? { ...row, height: 150 } : row)} />);
    resized();
    expect(viewport.scrollTop).toBe(850);
    const withReply = (text: string, height: number, above = 100) => [
      ...baseRows.map((row, index) => index === 9 ? { ...row, height: above } : row),
      { id: "accepted", height: 100 },
      { id: "response", height, text },
    ];
    view.rerender(<Harness resizeEachCommit={false} submittedRowId="accepted" rows={withReply("", 50)} />);
    expect(screen.getByTestId("mode")).toHaveTextContent("anchoring-submitted-turn");
    const anchored = viewport.scrollTop;
    rowReads.clear();
    for (let token = 1; token <= 5; token += 1) {
      view.rerender(<Harness resizeEachCommit={false} submittedRowId="accepted" rows={withReply("token ".repeat(token), 50 + token * 40)} />);
    }
    expect(rowReads.size).toBe(0);
    resized();
    expect(viewport.scrollTop).toBe(anchored);
    expect([...rowReads.keys()]).toEqual(["accepted"]);
    // A row above the anchor grows: the resize pass restores the anchor
    // before the frame paints.
    view.rerender(<Harness resizeEachCommit={false} submittedRowId="accepted" rows={withReply("token ".repeat(5), 250, 160)} />);
    resized();
    expect(viewport.scrollTop).toBe(anchored + 60);
    expect(screen.getByTestId("mode")).toHaveTextContent("anchoring-submitted-turn");
  });
  test("does not treat a consumed acceptance as a new send after remount or identity return", () => {
    const key = { authority: "accepted-remount", identity: "agent", conversation: "one", pane: "left" };
    const view = render(<Harness viewportKey={key} submittedRowId="row-8" />);
    expect(screen.getByTestId("mode")).toHaveTextContent("anchoring-submitted-turn");
    userScroll(screen.getByTestId("viewport"), 225);
    view.unmount();
    const next = render(<Harness viewportKey={key} submittedRowId="row-8" />);
    expect(screen.getByTestId("viewport").scrollTop).toBe(225);
    next.rerender(<Harness viewportKey={{ ...key, identity: "other" }} submittedRowId={null} />);
    next.rerender(<Harness viewportKey={key} submittedRowId="row-8" />);
    expect(screen.getByTestId("viewport").scrollTop).toBe(225);
    next.rerender(<Harness viewportKey={{ ...key, pane: "right" }} submittedRowId="row-8" />);
    expect(screen.getByTestId("viewport").scrollTop).toBe(776);
  });
  test("retains an accepted row awaiting DOM arrival across a remount", () => {
    const key = { authority: "pending-accepted-remount", identity: "agent", conversation: "one", pane: "left" };
    const view = render(<Harness viewportKey={key} submittedRowId="accepted-later" />);
    view.unmount();
    render(<Harness viewportKey={key} submittedRowId="accepted-later" rows={[...baseRows, { id: "accepted-later", height: 100 }]} />);
    expect(screen.getByTestId("mode")).toHaveTextContent("anchoring-submitted-turn");
  });
  test("failed asynchronous reveal restores a retained neighbor and exposes unavailability", async () => {
    vi.useFakeTimers();
    const key = { authority: "failed-reveal", identity: "agent", conversation: "one", pane: "left" };
    const view = render(<Harness viewportKey={key} />);
    userScroll(screen.getByTestId("viewport"), 225);
    view.unmount();
    let fail!: (reason: Error) => void;
    const reveal = vi.fn(() => new Promise<boolean>((_resolve, reject) => { fail = reject; }));
    const retained = [{ id: "replacement", height: 300 }, ...baseRows.slice(3)];
    const next = render(<Harness viewportKey={key} rows={retained} revealAnchor={reveal} />);
    expect(screen.getByRole("status")).toHaveTextContent("Restoring earlier position");
    await act(async () => { fail(new Error("history unavailable")); await Promise.resolve(); vi.advanceTimersByTime(20); });
    expect(screen.getByTestId("viewport").scrollTop).toBe(225);
    expect(screen.getByRole("status")).toHaveTextContent("Earlier position is unavailable");
    next.rerender(<Harness viewportKey={key} rows={retained} revealAnchor={reveal} />);
    expect(reveal).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("status")).toHaveTextContent("Earlier position is unavailable");
  });
  test("keeps a remembered anchor through an empty loading seed and its native scroll clamp", async () => {
    vi.useFakeTimers();
    const key = { authority: "identity-seed-loading", identity: "agent", conversation: "one", pane: "left" };
    const view = render(<Harness viewportKey={key} />);
    const viewport = screen.getByTestId("viewport");
    userScroll(viewport, 225);
    view.rerender(<Harness viewportKey={{ ...key, identity: "other", conversation: "other" }}
      rows={baseRows.map(row => ({ ...row, id: `other-${row.id}` }))} />);
    let completeSeed!: (available: boolean) => void;
    const reveal = vi.fn(() => new Promise<boolean>(resolve => { completeSeed = resolve; }));
    view.rerender(<Harness viewportKey={key} rows={[]} revealAnchor={reveal} />);
    // Removing the old identity's rows clamps scrollTop in a real browser. It
    // is not a wheel, pointer or keyboard decision to abandon the saved row.
    viewport.scrollTop = 0;
    fireEvent.scroll(viewport);
    await act(async () => { vi.advanceTimersByTime(100); });
    expect(screen.getByRole("status")).toHaveTextContent("Restoring earlier position");
    expect(reveal).toHaveBeenCalledTimes(1);
    expect(reveal).toHaveBeenCalledWith("row-2", expect.any(AbortSignal));
    view.rerender(<Harness viewportKey={key} rows={baseRows} revealAnchor={reveal} />);
    await act(async () => { completeSeed(true); await Promise.resolve(); vi.advanceTimersByTime(20); });
    expect(viewport.querySelector('[data-conversation-row-id="row-2"]')!.getBoundingClientRect().top).toBe(-25);
    expect(screen.queryByRole("status")).toBeNull();
    expect(screen.getByTestId("mode")).toHaveTextContent("reading-history");
  });
  test("a confirmed reveal waits for the row to mount, even if a frame passes before the host's rows commit", async () => {
    vi.useFakeTimers();
    const key = { authority: "identity-seed-commit-race", identity: "agent", conversation: "one", pane: "left" };
    const view = render(<Harness viewportKey={key} />);
    userScroll(screen.getByTestId("viewport"), 225);
    view.rerender(<Harness viewportKey={{ ...key, identity: "other", conversation: "other" }}
      rows={baseRows.map(row => ({ ...row, id: `other-${row.id}` }))} />);
    let completeSeed!: (available: boolean) => void;
    const reveal = vi.fn(() => new Promise<boolean>(resolve => { completeSeed = resolve; }));
    view.rerender(<Harness viewportKey={key} rows={[]} revealAnchor={reveal} />);
    const viewport = screen.getByTestId("viewport");
    // The host has the row in its own frames and says so, but its rows reach
    // the DOM only on its next commit, after an animation frame has passed.
    await act(async () => { completeSeed(true); await Promise.resolve(); vi.advanceTimersByTime(40); });
    expect(screen.getByRole("status")).toHaveTextContent("Restoring earlier position");
    view.rerender(<Harness viewportKey={key} rows={baseRows} revealAnchor={reveal} />);
    expect(viewport.querySelector('[data-conversation-row-id="row-2"]')!.getBoundingClientRect().top).toBe(-25);
    expect(screen.queryByRole("status")).toBeNull();
    expect(screen.getByTestId("mode")).toHaveTextContent("reading-history");
  });
  test("a confirmed reveal whose next content commit lacks the row reports it unavailable", async () => {
    vi.useFakeTimers();
    const key = { authority: "identity-seed-commit-missing", identity: "agent", conversation: "one", pane: "left" };
    const view = render(<Harness viewportKey={key} />);
    userScroll(screen.getByTestId("viewport"), 225);
    view.unmount();
    let completeSeed!: (available: boolean) => void;
    const reveal = () => new Promise<boolean>(resolve => { completeSeed = resolve; });
    const next = render(<Harness viewportKey={key} rows={[]} revealAnchor={reveal} />);
    await act(async () => { completeSeed(true); await Promise.resolve(); });
    expect(screen.getByRole("status")).toHaveTextContent("Restoring earlier position");
    next.rerender(<Harness viewportKey={key} rows={baseRows.slice(5)} revealAnchor={reveal} />);
    expect(screen.getByRole("status")).toHaveTextContent("Earlier position is unavailable");
  });
  test("reports an empty exhausted seed unavailable only after the host completes its query", async () => {
    vi.useFakeTimers();
    const key = { authority: "identity-seed-exhausted", identity: "agent", conversation: "one", pane: "left" };
    const view = render(<Harness viewportKey={key} />);
    userScroll(screen.getByTestId("viewport"), 225);
    view.unmount();
    let completeSeed!: (available: boolean) => void;
    const reveal = () => new Promise<boolean>(resolve => { completeSeed = resolve; });
    render(<Harness viewportKey={key} rows={[]} revealAnchor={reveal} />);
    const viewport = screen.getByTestId("viewport");
    viewport.scrollTop = 0;
    fireEvent.scroll(viewport);
    await act(async () => { vi.advanceTimersByTime(100); });
    expect(screen.getByRole("status")).toHaveTextContent("Restoring earlier position");
    await act(async () => { completeSeed(false); await Promise.resolve(); });
    expect(screen.getByRole("status")).toHaveTextContent("Earlier position is unavailable");
    expect(viewport.querySelectorAll("[data-conversation-row-id]")).toHaveLength(0);
  });
  test("an explicit scroll gesture cancels pending restoration and late seed completion", async () => {
    vi.useFakeTimers();
    const key = { authority: "identity-seed-user-intent", identity: "agent", conversation: "one", pane: "left" };
    const view = render(<Harness viewportKey={key} />);
    userScroll(screen.getByTestId("viewport"), 225);
    view.unmount();
    let completeSeed!: (available: boolean) => void;
    let signal!: AbortSignal;
    const reveal = (_row: string, request: AbortSignal) => {
      signal = request;
      return new Promise<boolean>(resolve => { completeSeed = resolve; });
    };
    const next = render(<Harness viewportKey={key} rows={baseRows.slice(5)} revealAnchor={reveal} />);
    const viewport = screen.getByTestId("viewport");
    expect(screen.getByRole("status")).toHaveTextContent("Restoring earlier position");
    fireEvent.wheel(viewport, { deltaY: 100 });
    userScroll(viewport, 100);
    expect(signal.aborted).toBe(true);
    next.rerender(<Harness viewportKey={key} rows={baseRows} revealAnchor={reveal} />);
    await act(async () => { completeSeed(true); await Promise.resolve(); vi.advanceTimersByTime(20); });
    expect(viewport.querySelector('[data-conversation-row-id="row-6"]')!.getBoundingClientRect().top).toBe(0);
    expect(screen.queryByRole("status")).toBeNull();
  });
  test("bounds never-settling and legacy boolean reveals without claiming the row exists", async () => {
    vi.useFakeTimers();
    const key = { authority: "bounded-reveal", identity: "agent", conversation: "one", pane: "left" };
    const view = render(<Harness viewportKey={key} />);
    userScroll(screen.getByTestId("viewport"), 225); view.unmount();
    let signal!: AbortSignal;
    const reveal = vi.fn((_id: string, requestSignal: AbortSignal) => { signal = requestSignal; return new Promise<boolean>(() => {}); });
    const next = render(<Harness viewportKey={key} rows={baseRows.slice(3)} revealAnchor={reveal} revealTimeoutMs={100} />);
    await act(async () => { vi.advanceTimersByTime(120); });
    expect(signal.aborted).toBe(true);
    expect(screen.getByRole("status")).toHaveTextContent("Earlier position is unavailable");
    next.unmount();
    const legacyKey = { ...key, pane: "legacy" };
    const seed = render(<Harness viewportKey={legacyKey} />);
    userScroll(screen.getByTestId("viewport"), 225); seed.unmount();
    render(<Harness viewportKey={legacyKey} rows={baseRows.slice(3)} revealAnchor={() => true} />);
    await act(async () => { vi.advanceTimersByTime(40); });
    expect(screen.getByRole("status")).toHaveTextContent("Earlier position is unavailable");
  });
  test("aborts an old reveal on identity change and ignores its late completion", async () => {
    vi.useFakeTimers();
    const key = { authority: "cancel-reveal", identity: "agent", conversation: "one", pane: "left" };
    const view = render(<Harness viewportKey={key} />);
    userScroll(screen.getByTestId("viewport"), 225); view.unmount();
    let signal!: AbortSignal;
    let finish!: (found: boolean) => void;
    const reveal = (_id: string, requestSignal: AbortSignal) => { signal = requestSignal; return new Promise<boolean>((resolve) => { finish = resolve; }); };
    const next = render(<Harness viewportKey={key} rows={baseRows.slice(3)} revealAnchor={reveal} />);
    next.rerender(<Harness viewportKey={{ ...key, identity: "other" }} />);
    expect(signal.aborted).toBe(true);
    await act(async () => { finish(true); await Promise.resolve(); vi.advanceTimersByTime(20); });
    expect(screen.getByTestId("viewport").scrollTop).toBe(800);
    expect(screen.queryByRole("status")).toBeNull();
  });
  test("requests a missing saved row once and restores it after the host reveals it", () => {
    const key = { authority: "reveal-scroll-test", identity: "agent", conversation: "one", pane: "left" };
    const view = render(<Harness viewportKey={key} />);
    userScroll(screen.getByTestId("viewport"), 225);
    view.unmount();
    const reveal = vi.fn(() => true);
    const next = render(<Harness viewportKey={key} rows={baseRows.slice(5)} revealAnchor={reveal} />);
    expect(reveal).toHaveBeenCalledWith("row-2", expect.any(AbortSignal));
    next.rerender(<Harness viewportKey={key} revealAnchor={reveal} />);
    expect(screen.getByTestId("viewport").scrollTop).toBe(225);
    expect(reveal).toHaveBeenCalledTimes(1);
  });
  test("user scrolling back within the live edge resumes following", () => {
    const view = render(<Harness />);
    const viewport = screen.getByTestId("viewport");
    userScroll(viewport, 100);
    userScroll(viewport, 780);
    expect(screen.getByTestId("mode")).toHaveTextContent("following-end");
    view.rerender(<Harness rows={[...baseRows, { id: "new", height: 100 }]} />);
    expect(viewport.scrollTop).toBe(900);
  });
  test("a scroll away from the live edge is not pinned back by a commit before its event arrives", () => {
    // Browser order: a scroll lands, a streamed commit can run in a task
    // before the next frame delivers the scroll event, and the post-layout
    // resize pass runs after it. Re-pinning on that commit snapped a reader
    // (or a test's scrollIntoView) back to the bottom before the controller
    // saw the scroll (console e2e chat-pane-older-history-demand-paging).
    const view = render(<Harness resizeEachCommit={false} />);
    const viewport = screen.getByTestId("viewport");
    expect(viewport.scrollTop).toBe(800);
    viewport.scrollTop = 0;
    view.rerender(<Harness resizeEachCommit={false} rows={baseRows.map((row, index) => index === 9 ? { ...row, text: "streamed" } : row)} />);
    expect(viewport.scrollTop).toBe(0);
    fireEvent.scroll(viewport);
    act(deliverResize);
    expect(viewport.scrollTop).toBe(0);
    expect(screen.getByTestId("mode")).toHaveTextContent("reading-history");
  });

  test("an element inserted above the rows keeps a reader's anchor, though no row resizes", async () => {
    // Rows that only move, as when the older-history control appears above
    // them, report no resize; the controller still restores the anchor
    // instead of leaving the shift for the next resize to correct
    // (console e2e real-stock-presentation).
    const view = render(<Harness resizeEachCommit={false} />);
    const viewport = screen.getByTestId("viewport");
    userScroll(viewport, 300);
    expect(screen.getByTestId("mode")).toHaveTextContent("reading-history");
    view.rerender(<Harness resizeEachCommit={false} header={36} />);
    await act(async () => { await new Promise((resolve) => requestAnimationFrame(() => resolve(null))); });
    expect(viewport.scrollTop).toBe(336);
    expect(screen.getByTestId("mode")).toHaveTextContent("reading-history");
  });

  test("every scroll write, including a restore on a session change, runs with snapping and scroll anchoring off", () => {
    // A session change re-runs the observer effect, whose cleanup restores
    // the viewport's own scroll-snap-type and overflow-anchor before its
    // setup turns them off again; a restore written in between snapped to a
    // turn start 62 px away (console e2e real-stock-reading-intent).
    const view = render(<Harness conversation="one" />);
    const viewport = screen.getByTestId("viewport");
    userScroll(viewport, 225);
    view.rerender(<Harness conversation="two" />);
    const writes: Array<{ top: number; snap: string; anchor: string }> = [];
    const descriptor = Object.getOwnPropertyDescriptor(Element.prototype, "scrollTop")!;
    Object.defineProperty(viewport, "scrollTop", {
      configurable: true,
      get: () => descriptor.get!.call(viewport),
      set: (top: number) => {
        writes.push({ top, snap: viewport.style.scrollSnapType, anchor: viewport.style.overflowAnchor });
        descriptor.set!.call(viewport, top);
      },
    });
    // Back to the first conversation: its remembered anchor is restored.
    view.rerender(<Harness conversation="one" />);
    expect(writes.length).toBeGreaterThan(0);
    for (const write of writes) expect(write).toMatchObject({ snap: "none", anchor: "none" });
    expect(viewport.scrollTop).toBe(225);
  });

  test("a scroll that lands at the live edge keeps following without measuring rows", () => {
    // A clamp after the content shrinks emits a native scroll the controller
    // did not write. Following keeps no anchor, so reading every mounted row
    // to capture one was wasted.
    render(<Harness />);
    const viewport = screen.getByTestId("viewport");
    rowReads.clear();
    // The content shrinks by 20 px and the browser clamps scrollTop to the
    // new end: the scroll lands exactly at the live edge.
    Object.defineProperty(viewport, "scrollHeight", { configurable: true, value: 980 });
    userScroll(viewport, 780);
    expect(screen.getByTestId("mode")).toHaveTextContent("following-end");
    expect(rowReads.size).toBe(0);
  });
});

describe("leaving the live edge upward", () => {
  // Following the end, a reader scrolls up by a few pixels while a reply
  // streams in. Any upward movement the controller did not write leaves the
  // live edge at once: the 32 px live-edge band applies only on the way down,
  // and the growth must not snap the reader back to the end.
  const grown = [...baseRows, { id: "streamed", height: 160 }];
  test.each([1, 2, 5, 10])("a %i px wheel scroll up during content growth is not snapped back", (distance) => {
    const view = render(<Harness />);
    const viewport = screen.getByTestId("viewport");
    expect(viewport.scrollTop).toBe(800);
    fireEvent.wheel(viewport.querySelector('[data-conversation-row-id="row-9"]')!, { deltaY: -distance });
    userScroll(viewport, 800 - distance);
    view.rerender(<Harness rows={grown} />);
    expect(screen.getByTestId("mode")).toHaveTextContent("reading-history");
    expect(viewport.scrollTop).toBe(800 - distance);
  });
  test.each([1, 2, 5, 10])("a %i px scroll-event-only move up (scrollbar or touch) during content growth is not snapped back", (distance) => {
    const view = render(<Harness />);
    const viewport = screen.getByTestId("viewport");
    userScroll(viewport, 800 - distance);
    view.rerender(<Harness rows={grown} />);
    expect(screen.getByTestId("mode")).toHaveTextContent("reading-history");
    expect(viewport.scrollTop).toBe(800 - distance);
  });
  test.each([1, 2, 5, 10])("a %i px move up that chains out of a nested scroller during content growth is not snapped back", (distance) => {
    const view = render(<Harness />);
    const viewport = screen.getByTestId("viewport");
    // A code block that can still scroll up owns the wheel, so the wheel path
    // does not leave the live edge; the viewport scroll that follows when the
    // gesture chains out of it must.
    const nested = document.createElement("pre");
    nested.style.overflowY = "auto";
    Object.defineProperties(nested, { scrollHeight: { configurable: true, value: 400 }, clientHeight: { configurable: true, value: 100 } });
    viewport.querySelector('[data-conversation-row-id="row-9"]')!.append(nested);
    nested.scrollTop = 30;
    fireEvent.wheel(nested, { deltaY: -distance });
    expect(screen.getByTestId("mode")).toHaveTextContent("following-end");
    userScroll(viewport, 800 - distance);
    view.rerender(<Harness rows={grown} />);
    expect(screen.getByTestId("mode")).toHaveTextContent("reading-history");
    expect(viewport.scrollTop).toBe(800 - distance);
  });
  test("our own write to the end is not mistaken for a user scroll", () => {
    const view = render(<Harness />);
    const viewport = screen.getByTestId("viewport");
    view.rerender(<Harness rows={grown} />);
    expect(viewport.scrollTop).toBe(960);
    // The native scroll event for the controller's own write arrives later.
    fireEvent.scroll(viewport);
    expect(screen.getByTestId("mode")).toHaveTextContent("following-end");
    view.rerender(<Harness rows={[...grown, { id: "more", height: 40 }]} />);
    expect(viewport.scrollTop).toBe(1000);
  });
});

describe("a downward gesture's sub-pixel settle at the live edge", () => {
  // A downward wheel or End at the end of the transcript can leave scrollTop a
  // pixel above the computed end: scrollHeight and clientHeight are rounded,
  // the scroll range is not. That settle is not the reader moving up, and must
  // not leave the live edge (browser-e2e "opening the actual tool at live edge
  // remains following" read top 2225 -> 2224 after a down-wheel).
  const grown = [...baseRows, { id: "streamed", height: 160 }];
  test.each(["wheel", "End"])("a 1 px upward settle after a downward %s keeps following content growth", (gesture) => {
    const view = render(<Harness />);
    const viewport = screen.getByTestId("viewport");
    expect(viewport.scrollTop).toBe(800);
    if (gesture === "wheel") fireEvent.wheel(viewport.querySelector('[data-conversation-row-id="row-9"]')!, { deltaY: 350 });
    else fireEvent.keyDown(viewport, { key: "End" });
    userScroll(viewport, 799);
    expect(screen.getByTestId("mode")).toHaveTextContent("following-end");
    view.rerender(<Harness rows={grown} />);
    expect(viewport.scrollTop).toBe(960);
  });
  test("a larger upward move after a downward wheel still leaves the live edge", () => {
    const view = render(<Harness />);
    const viewport = screen.getByTestId("viewport");
    fireEvent.wheel(viewport.querySelector('[data-conversation-row-id="row-9"]')!, { deltaY: 350 });
    userScroll(viewport, 798);
    view.rerender(<Harness rows={grown} />);
    expect(screen.getByTestId("mode")).toHaveTextContent("reading-history");
    expect(viewport.scrollTop).toBe(798);
  });
  test("a pointer press clears the downward gesture: a 1 px move up after it leaves", () => {
    const view = render(<Harness />);
    const viewport = screen.getByTestId("viewport");
    fireEvent.wheel(viewport.querySelector('[data-conversation-row-id="row-9"]')!, { deltaY: 350 });
    // A scrollbar drag starts with a press on the viewport.
    fireEvent.pointerDown(viewport);
    userScroll(viewport, 799);
    view.rerender(<Harness rows={grown} />);
    expect(screen.getByTestId("mode")).toHaveTextContent("reading-history");
    expect(viewport.scrollTop).toBe(799);
  });
});

describe("a native settle of the end after a content press", () => {
  // browser-e2e "opening the actual tool at live edge remains following" on
  // the shared host, recorded geometry: the controller wrote the computed end
  // 2225 after a copy click, then the browser settled it to 2224 before the
  // tool header's press (a pointer action reveals its target first), and the
  // opened tool was held by an anchor instead of followed. Rows total 2425 px
  // in a 200 px viewport, so the computed end is 2225.
  const rows: Row[] = [...Array.from({ length: 24 }, (_, i) => ({ id: `row-${i}`, height: 100 })), { id: "tool", height: 25 }];
  const opened: Row[] = [...rows.slice(0, -1), { id: "tool", height: 136 }];
  const row = (viewport: HTMLElement, id: string) => viewport.querySelector(`[data-conversation-row-id="${id}"]`)!;
  /** A pointer press carrying its pointer type, which jsdom's events lack. */
  const press = (target: Element, pointerType: string) => {
    const event = new MouseEvent("pointerdown", { bubbles: true, cancelable: true });
    Object.defineProperty(event, "pointerType", { value: pointerType });
    fireEvent(target, event);
  };

  test("the recorded 2225 -> 2224 trace keeps following when the tool opens", () => {
    const view = render(<Harness rows={rows} />);
    const viewport = screen.getByTestId("viewport");
    expect(viewport.scrollTop).toBe(2225);
    fireEvent.wheel(row(viewport, "tool"), { deltaY: 350 });
    userScroll(viewport, 2224);
    fireEvent.keyDown(viewport, { key: "End" });
    // The copy control's press, then a commit that rewrites the computed end.
    press(row(viewport, "tool"), "mouse");
    view.rerender(<Harness rows={[...rows]} />);
    expect(viewport.scrollTop).toBe(2225);
    // The browser snaps the end a pixel up before the next press.
    userScroll(viewport, 2224);
    expect(screen.getByTestId("mode")).toHaveTextContent("following-end");
    press(row(viewport, "tool"), "mouse");
    view.rerender(<Harness rows={opened} />);
    expect(screen.getByTestId("mode")).toHaveTextContent("following-end");
    expect(viewport.scrollTop).toBe(2336);
  });

  test("a native scroll down after a tool opens at the end keeps following", () => {
    // browser-e2e "opening the actual tool at live edge remains following",
    // locally reproduced trace (shared host, tall layout): following at the
    // computed end 2225, the tool header press, the disclosure grows the
    // content by 111 px, and only then the browser's own scroll event reports
    // 2226 (a pixel down). Judged against the grown end 2336 that position
    // was outside the live-edge band, so the pane left following.
    const view = render(<Harness rows={rows} />);
    const viewport = screen.getByTestId("viewport");
    expect(viewport.scrollTop).toBe(2225);
    press(row(viewport, "tool"), "mouse");
    Object.defineProperty(viewport, "scrollHeight", { configurable: true, value: 2536 });
    userScroll(viewport, 2226);
    expect(screen.getByTestId("mode")).toHaveTextContent("following-end");
    view.rerender(<Harness rows={opened} />);
    expect(screen.getByTestId("mode")).toHaveTextContent("following-end");
    expect(viewport.scrollTop).toBe(2336);
  });

  test("a native scroll down after content growth does not resume following while reading", () => {
    const view = render(<Harness rows={rows} />);
    const viewport = screen.getByTestId("viewport");
    userScroll(viewport, 2200);
    expect(screen.getByTestId("mode")).toHaveTextContent("reading-history");
    Object.defineProperty(viewport, "scrollHeight", { configurable: true, value: 2536 });
    userScroll(viewport, 2201);
    view.rerender(<Harness rows={opened} />);
    expect(screen.getByTestId("mode")).toHaveTextContent("reading-history");
    expect(viewport.scrollTop).toBe(2201);
  });

  test.each([2, 5, 40])("a %i px upward drag after a content press still leaves the live edge", (distance) => {
    const view = render(<Harness rows={rows} />);
    const viewport = screen.getByTestId("viewport");
    press(row(viewport, "tool"), "mouse");
    userScroll(viewport, 2225 - distance);
    view.rerender(<Harness rows={opened} />);
    expect(screen.getByTestId("mode")).toHaveTextContent("reading-history");
    expect(viewport.scrollTop).toBe(2225 - distance);
  });

  test.each(["touch", "pen"])("a 1 px move after a %s press on the content leaves the live edge", (pointerType) => {
    const view = render(<Harness rows={rows} />);
    const viewport = screen.getByTestId("viewport");
    press(row(viewport, "tool"), pointerType);
    userScroll(viewport, 2224);
    view.rerender(<Harness rows={opened} />);
    expect(screen.getByTestId("mode")).toHaveTextContent("reading-history");
    expect(viewport.scrollTop).toBe(2224);
  });

  test("a 1 px move after a middle press (autoscroll) on the content leaves the live edge", () => {
    const view = render(<Harness rows={rows} />);
    const viewport = screen.getByTestId("viewport");
    const event = new MouseEvent("pointerdown", { bubbles: true, cancelable: true, button: 1 });
    Object.defineProperty(event, "pointerType", { value: "mouse" });
    fireEvent(row(viewport, "tool"), event);
    userScroll(viewport, 2224);
    view.rerender(<Harness rows={opened} />);
    expect(screen.getByTestId("mode")).toHaveTextContent("reading-history");
  });

  test("a 1 px move after a primary mouse press on the viewport (its scrollbar) leaves the live edge", () => {
    const view = render(<Harness rows={rows} />);
    const viewport = screen.getByTestId("viewport");
    expect(viewport.scrollTop).toBe(2225);
    press(viewport, "mouse");
    userScroll(viewport, 2224);
    view.rerender(<Harness rows={opened} />);
    expect(screen.getByTestId("mode")).toHaveTextContent("reading-history");
    expect(viewport.scrollTop).toBe(2224);
  });

  test("a 1 px move after a content press that does not land at the grown end leaves the live edge", () => {
    // Content grows by 10 px before the controller's write: the move is a
    // pixel, but it lands inside the live-edge band, not at the end.
    render(<Harness rows={rows} />);
    const viewport = screen.getByTestId("viewport");
    press(row(viewport, "tool"), "mouse");
    Object.defineProperty(viewport, "scrollHeight", { configurable: true, value: 2435 });
    userScroll(viewport, 2224);
    expect(screen.getByTestId("mode")).toHaveTextContent("reading-history");
  });

  test("a content press does not turn a 1 px move at the end into following while reading", () => {
    render(<Harness rows={rows} />);
    const viewport = screen.getByTestId("viewport");
    // An upward wheel at the end leaves the live edge before it scrolls.
    fireEvent.wheel(row(viewport, "tool"), { deltaY: -1 });
    expect(screen.getByTestId("mode")).toHaveTextContent("reading-history");
    expect(viewport.scrollTop).toBe(2225);
    press(row(viewport, "tool"), "mouse");
    userScroll(viewport, 2224);
    expect(screen.getByTestId("mode")).toHaveTextContent("reading-history");
  });

  test("a content press does not resume following from a reading position near the end", () => {
    const view = render(<Harness rows={rows} />);
    const viewport = screen.getByTestId("viewport");
    userScroll(viewport, 2224);
    expect(screen.getByTestId("mode")).toHaveTextContent("reading-history");
    press(row(viewport, "tool"), "mouse");
    userScroll(viewport, 2223);
    view.rerender(<Harness rows={opened} />);
    expect(screen.getByTestId("mode")).toHaveTextContent("reading-history");
  });
});
