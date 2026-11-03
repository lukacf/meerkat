import { useCallback, useLayoutEffect, useRef, useState, type RefObject } from "react";

import {
  CONVERSATION_ANCHOR_OFFSET_PX,
  CONVERSATION_END_ROUNDING_PX,
  ConversationPositionCache,
  captureConversationAnchor,
  conversationIsAtEnd,
  conversationScrollEnd,
  isConversationScrollTarget,
  restoreConversationAnchor,
  type ConversationRowGeometry,
  type ConversationScrollAnchor,
  type ConversationScrollMode,
} from "./scroll-geometry";

export type ConversationViewportKey = {
  /** Runtime, realm and principal authority, not a display label. */
  authority: string;
  identity: string;
  conversation: string;
  pane: string;
};

export type ConversationScrollControllerOptions = {
  viewportRef: RefObject<HTMLElement | null>;
  contentRef?: RefObject<HTMLElement | null>;
  /** Without this key, memory stays local to this mounted controller. */
  viewportKey?: ConversationViewportKey;
  conversationId: string;
  /** Change when content changes. DOM resize/load observers also cover late layout. */
  contentVersion?: unknown;
  /** Canonical accepted row only. A queued draft is not a submitted row. */
  submittedRowId?: string | null;
  /** Resolve an accepted source ID to its current DOM row without changing
   * the receipt used to remember or cancel a pending submission. */
  resolveSubmittedRowId?: (sourceId: string) => string | null;
  /**
   * Resolve after bounded history/reveal work completes, false when unavailable.
   * The controller verifies the row in the DOM; completion never proves existence.
   * A legacy true boolean requests one local render, then falls back if absent.
   */
  revealAnchor?: (rowId: string, signal: AbortSignal) => boolean | Promise<boolean>;
  /** Upper bound for a host reveal, default 15 seconds, maximum 60 seconds. */
  revealTimeoutMs?: number;
};

const sharedPositions = new ConversationPositionCache();
const ROW_SELECTOR = "[data-conversation-row-id]";

type Session = {
  key: string;
  authority: string | undefined;
  mode: ConversationScrollMode;
  anchor: ConversationScrollAnchor | null;
  pendingSubmittedRow: string | null;
  lastSubmittedRow: string | null;
  /// The position the controller last wrote (or observed for a pending
  /// browser reveal), read back from the viewport so it is exactly what that
  /// write's scroll event reports.
  expectedScrollTop: number | null;
  /// The last position seen, written or observed, so a scroll event can tell
  /// which way it moved.
  lastScrollTop: number | null;
  /// The reader's last explicit input: the direction of a vertical gesture
  /// (wheel or key), or a mouse press on the transcript's content, which
  /// cannot scroll it. A press that can start a scroll (on the viewport's own
  /// scrollbar, or by touch or pen) clears it. After a downward gesture or a
  /// content press, the browser can settle the end a pixel above the computed
  /// end (see CONVERSATION_END_ROUNDING_PX); that settle is not the reader
  /// moving up.
  lastGesture: "up" | "down" | "content-press" | null;
  requestedAnchor: string | null;
  awaitingAnchor: boolean;
  missingAnchor: boolean;
  /// `confirmed` holds the content version at which the host reported the
  /// row available; the row is expected in the next content commit.
  reveal?: { controller: AbortController; timer: number | null; frame: number | null; confirmed?: { version: unknown } };
};

function cancelReveal(session: Session): void {
  if (session.reveal) {
    session.reveal.controller.abort();
    if (session.reveal.timer !== null) window.clearTimeout(session.reveal.timer);
    if (session.reveal.frame !== null) window.cancelAnimationFrame(session.reveal.frame);
    session.reveal = undefined;
  }
  session.awaitingAnchor = false;
}

/** Geometry of every row outside a collapsed disclosure: the candidates for
 * a captured reading anchor and its neighbors.
 *
 * A parked turn of a windowed transcript (an element the transcript hides)
 * keeps its rows in the DOM for find-in-page, in a placeholder of the turn's
 * last measured size. They lay out as if the turn were unparked, each where
 * it will be once it is. When the turn has outgrown that measurement, its
 * last rows reach past the placeholder, over rows rendered below it, and a
 * reader at that point sees those rows instead. A parked row therefore
 * extends only to its placeholder's end: one that reaches into the viewport
 * only past it never becomes the anchor, since unparking moves everything
 * below the placeholder while that row stays. */
function rowGeometry(viewport: HTMLElement): ConversationRowGeometry[] {
  const top = viewport.getBoundingClientRect().top + viewport.clientTop;
  const placeholderEnds = new Map<Element, number>();
  const rows: ConversationRowGeometry[] = [];
  for (const row of Array.from(viewport.querySelectorAll<HTMLElement>(ROW_SELECTOR))) {
    if (row.closest("details:not([open])")) continue;
    const rect = row.getBoundingClientRect();
    let bottom = rect.bottom;
    const parked = row.closest("[hidden]");
    if (parked && viewport.contains(parked)) {
      let end = placeholderEnds.get(parked);
      if (end === undefined) {
        end = parked.getBoundingClientRect().bottom;
        placeholderEnds.set(parked, end);
      }
      bottom = Math.min(bottom, end);
    }
    rows.push({ id: row.dataset.conversationRowId!, top: rect.top - top, bottom: bottom - top });
  }
  return rows;
}

/** Geometry of just these rows (absent or collapsed rows are omitted), with
 * the same coordinates as `rowGeometry`. A row in a parked turn is found:
 * its own box lies where it will be once unparked, so an anchor or jump
 * that names it lands right. Holding an anchor steady needs only the anchor
 * row; reading every mounted row on each streamed commit made every token
 * pay for the whole transcript's rects. */
function rowGeometryOf(viewport: HTMLElement, ids: readonly string[]): ConversationRowGeometry[] {
  const top = viewport.getBoundingClientRect().top + viewport.clientTop;
  const rows: ConversationRowGeometry[] = [];
  for (const id of ids) {
    const row = viewport.querySelector<HTMLElement>(`[data-conversation-row-id="${id.replace(/["\\]/g, "\\$&")}"]`);
    if (!row || row.closest("details:not([open])")) continue;
    const rect = row.getBoundingClientRect();
    rows.push({ id, top: rect.top - top, bottom: rect.bottom - top });
  }
  return rows;
}

/** A session that changes position only when rows or the viewport resize:
 * following the live edge, or holding an anchor, with no submission or
 * reveal pending. */
function steadySession(session: Session): boolean {
  return !session.pendingSubmittedRow && !session.awaitingAnchor && !session.reveal
    && (session.mode === "following-end" || session.anchor !== null);
}

/** Preserve intent across streaming and layout without moving an outer document. */
export function useConversationScrollController(options: ConversationScrollControllerOptions) {
  const optionsRef = useRef(options);
  optionsRef.current = options;
  const localPositions = useRef(new ConversationPositionCache());
  const sessionRef = useRef<Session | null>(null);
  const frameRef = useRef<number | null>(null);
  const applyLayoutRef = useRef<() => void>(() => {});
  /// True while a ResizeObserver delivers row and viewport size changes. Its
  /// callbacks run after the frame's layout, so geometry read there is
  /// already computed instead of forcing a layout from script.
  const observingResizeRef = useRef(false);
  const [state, setState] = useState({ mode: "following-end" as ConversationScrollMode, awayFromEnd: false, missingAnchor: false, revealingAnchor: false });
  const key = options.viewportKey
    ? JSON.stringify([options.viewportKey.authority, options.viewportKey.identity, options.viewportKey.conversation, options.viewportKey.pane])
    : options.conversationId;
  const authority = options.viewportKey?.authority;

  const publish = useCallback((missing?: boolean) => {
    const viewport = optionsRef.current.viewportRef.current;
    const session = sessionRef.current;
    if (!viewport || !session) return;
    if (missing !== undefined) session.missingAnchor = missing;
    const missingAnchor = session.missingAnchor;
    const revealingAnchor = session.awaitingAnchor;
    const awayFromEnd = !conversationIsAtEnd(viewport.scrollTop, viewport.scrollHeight, viewport.clientHeight);
    setState((old) => old.mode === session.mode && old.awayFromEnd === awayFromEnd && old.missingAnchor === missingAnchor && old.revealingAnchor === revealingAnchor
      ? old : { mode: session.mode, awayFromEnd, missingAnchor, revealingAnchor });
    const cache = session.authority === undefined ? localPositions.current : sharedPositions;
    cache.remember(session.key, { mode: session.mode, anchor: session.anchor, scrollTop: viewport.scrollTop, lastSubmittedRow: session.lastSubmittedRow, pendingSubmittedRow: session.pendingSubmittedRow });
  }, []);

  const writeScroll = useCallback((top: number) => {
    const viewport = optionsRef.current.viewportRef.current;
    const session = sessionRef.current;
    if (!viewport || !session) return;
    const bounded = Math.max(0, Math.min(conversationScrollEnd(viewport.scrollHeight, viewport.clientHeight), top));
    // This controller owns scroll position, so snapping and browser scroll
    // anchoring stay off while it writes. On a session change the observer
    // effect's cleanup restores them before its setup turns them off again,
    // and a restore written in between snapped to the nearest turn start.
    if (viewport.style.scrollSnapType !== "none") viewport.style.scrollSnapType = "none";
    if (viewport.style.overflowAnchor !== "none") viewport.style.overflowAnchor = "none";
    if (Math.abs(viewport.scrollTop - bounded) > 0.1) viewport.scrollTop = bounded;
    // Read back the value the browser kept (clamped or rounded): this write's
    // scroll event reports exactly it, so the event is recognised as ours
    // without a tolerance that would also swallow a small user scroll.
    session.expectedScrollTop = viewport.scrollTop;
    session.lastScrollTop = session.expectedScrollTop;
  }, []);

  const applyLayout = useCallback(() => {
    const viewport = optionsRef.current.viewportRef.current;
    const session = sessionRef.current;
    if (!viewport || !session) return;
    // Row geometry forces layout and reads every mounted row. Following the
    // live edge needs none of it, and this runs on every commit, so measure
    // only on the paths that use rows.
    if (session.mode === "following-end" && !session.pendingSubmittedRow) {
      writeScroll(conversationScrollEnd(viewport.scrollHeight, viewport.clientHeight));
      // Leaving the live edge (scroll, wheel, key, selection, jump) captures
      // its own anchor, so none is kept while following.
      session.anchor = null;
      publish(false);
      return;
    }
    if (session.pendingSubmittedRow) {
      const resolveSubmitted = optionsRef.current.resolveSubmittedRowId;
      const submittedRowId = resolveSubmitted ? resolveSubmitted(session.pendingSubmittedRow) : session.pendingSubmittedRow;
      const [submitted] = submittedRowId ? rowGeometryOf(viewport, [submittedRowId]) : [];
      if (submitted) {
        cancelReveal(session);
        session.missingAnchor = false;
        session.mode = "anchoring-submitted-turn";
        session.pendingSubmittedRow = null;
        writeScroll(viewport.scrollTop + submitted.top - CONVERSATION_ANCHOR_OFFSET_PX);
        const [actual] = rowGeometryOf(viewport, [submitted.id]);
        session.anchor = { rowId: actual.id, offset: actual.top, neighbors: [] };
      }
    }
    if (session.mode === "following-end") {
      // Still waiting for the submitted row: follow the live edge. As on the
      // ordinary following path, leaving it captures its own anchor.
      writeScroll(conversationScrollEnd(viewport.scrollHeight, viewport.clientHeight));
      session.anchor = null;
      publish(false);
      return;
    }
    const anchor = session.anchor;
    let missing = session.missingAnchor;
    if (anchor) {
      // Holding the anchor needs only its row (or a recorded neighbor). Every
      // mounted row is read only when the anchor is gone, to reveal it or to
      // fall back to the nearest survivor.
      let rows = rowGeometryOf(viewport, [anchor.rowId, ...anchor.neighbors.map((neighbor) => neighbor.rowId)]);
      const found = rows.some((row) => row.id === anchor.rowId);
      if (!found) rows = rowGeometry(viewport);
      if (!found && session.requestedAnchor !== anchor.rowId) {
        session.requestedAnchor = anchor.rowId;
        cancelReveal(session);
        const reveal: NonNullable<Session["reveal"]> = { controller: new AbortController(), timer: null, frame: null };
        session.reveal = reveal;
        const isCurrent = () => sessionRef.current === session && session.reveal === reveal && !reveal.controller.signal.aborted;
        const finish = () => {
          if (!isCurrent()) return;
          cancelReveal(session);
          applyLayoutRef.current();
        };
        try {
          const result = optionsRef.current.revealAnchor?.(anchor.rowId, reveal.controller.signal) ?? false;
          if (result) {
            session.awaitingAnchor = true;
            if (typeof result === "boolean") {
              reveal.frame = window.requestAnimationFrame(finish);
            } else {
              const timeout = optionsRef.current.revealTimeoutMs;
              reveal.timer = window.setTimeout(finish, Number.isFinite(timeout) ? Math.max(1, Math.min(timeout!, 60_000)) : 15_000);
              Promise.resolve(result).then((available) => {
                if (!isCurrent()) return;
                if (!available) { finish(); return; }
                // The host has the row; its DOM arrives with its next content
                // commit, which can land after the next animation frame.
                // Restore when the row mounts, and give up only if a newer
                // commit still lacks it (or the timeout above expires).
                reveal.confirmed = { version: optionsRef.current.contentVersion };
                applyLayoutRef.current();
              }, finish);
            }
          } else cancelReveal(session);
        } catch {
          cancelReveal(session);
        }
      }
      const promisedCommitLacksRow = !found && session.reveal?.confirmed !== undefined
        && session.reveal.confirmed.version !== optionsRef.current.contentVersion;
      if (!found && session.awaitingAnchor && !promisedCommitLacksRow) {
        // Publishing can render again before the host reveal lands. Keep the
        // requested anchor until the host has mounted the promised row.
        publish(false);
        return;
      }
      cancelReveal(session);
      const restored = restoreConversationAnchor(anchor, rows, viewport.scrollTop, conversationScrollEnd(viewport.scrollHeight, viewport.clientHeight));
      if (!found) missing = true;
      if (restored) writeScroll(restored.scrollTop);
      // An unavailable row falls back to its nearest retained neighbor. Once
      // chosen it becomes the new anchor, instead of repeating reveal requests.
      if (!found) session.anchor = captureConversationAnchor(rowGeometry(viewport));
    } else {
      session.anchor = captureConversationAnchor(rowGeometry(viewport));
    }
    publish(missing);
  }, [publish, writeScroll]);
  applyLayoutRef.current = applyLayout;

  const notifyLayoutChange = useCallback(() => {
    if (frameRef.current !== null) return;
    frameRef.current = window.requestAnimationFrame(() => {
      frameRef.current = null;
      applyLayout();
    });
  }, [applyLayout]);

  const readHistory = useCallback(() => {
    const viewport = optionsRef.current.viewportRef.current;
    const session = sessionRef.current;
    if (!viewport || !session) return;
    session.mode = "reading-history";
    session.pendingSubmittedRow = null;
    session.requestedAnchor = null;
    cancelReveal(session);
    session.missingAnchor = false;
    session.anchor = captureConversationAnchor(rowGeometry(viewport));
    publish();
  }, [publish]);

  const jumpToLatest = useCallback(() => {
    const session = sessionRef.current;
    if (!session) return;
    session.mode = "following-end";
    session.pendingSubmittedRow = null;
    session.requestedAnchor = null;
    cancelReveal(session);
    session.missingAnchor = false;
    applyLayout();
  }, [applyLayout]);

  const jumpToRow = useCallback((rowId: string): boolean => {
    const viewport = optionsRef.current.viewportRef.current;
    const session = sessionRef.current;
    if (!viewport || !session) return false;
    session.mode = "reading-history";
    session.pendingSubmittedRow = null;
    session.requestedAnchor = null;
    cancelReveal(session);
    session.missingAnchor = false;
    session.anchor = { rowId, offset: CONVERSATION_ANCHOR_OFFSET_PX, neighbors: [] };
    applyLayout();
    // No smooth scrolling: a rail jump is exact, including under reduced motion.
    return rowGeometryOf(viewport, [rowId]).length > 0;
  }, [applyLayout]);

  useLayoutEffect(() => {
    const previous = sessionRef.current;
    if (!previous || previous.key !== key || previous.authority !== authority) {
      if (previous) cancelReveal(previous);
      if (previous && previous.authority !== authority) {
        if (previous.authority !== undefined) sharedPositions.deleteAuthority(previous.authority);
        localPositions.current.clear();
      }
      const remembered = (authority === undefined ? localPositions.current : sharedPositions).read(key);
      sessionRef.current = {
        key, authority,
        mode: remembered?.mode ?? "following-end",
        anchor: remembered?.anchor ?? null,
        pendingSubmittedRow: remembered?.pendingSubmittedRow ?? null,
        lastSubmittedRow: remembered?.lastSubmittedRow ?? null,
        expectedScrollTop: null,
        lastScrollTop: null,
        lastGesture: null,
        requestedAnchor: null,
        awaitingAnchor: false,
        missingAnchor: false,
      };
    }
    const session = sessionRef.current!;
    const sessionChanged = session !== previous;
    if (options.submittedRowId && session.lastSubmittedRow !== options.submittedRowId) {
      session.lastSubmittedRow = options.submittedRowId;
      session.pendingSubmittedRow = options.submittedRowId;
    }
    // A steady session (following the live edge, or holding an anchor) only
    // moves when a row or the viewport changes size, which the resize
    // observer reports after layout. Applying it here on every commit read
    // geometry with layout dirty: one forced layout per streamed token.
    if (sessionChanged || !observingResizeRef.current || !steadySession(session)) applyLayout();
  });

  useLayoutEffect(() => {
    const viewport = options.viewportRef.current;
    if (!viewport) return;
    const observedSession = sessionRef.current;
    const previousOverflowAnchor = viewport.style.overflowAnchor;
    const previousSnap = viewport.style.scrollSnapType;
    viewport.style.overflowAnchor = "none";
    viewport.style.scrollSnapType = "none";
    const onScroll = () => {
      const session = sessionRef.current;
      if (!session) return;
      const observed = viewport.scrollTop;
      const previous = session.lastScrollTop;
      session.lastScrollTop = observed;
      // Clearing rows while an identity's authorized history loads can clamp
      // scrollTop and emit a native scroll event. Preserve the pending anchor;
      // explicit pointer, wheel and keyboard intent cancel it via readHistory.
      if (session.awaitingAnchor) {
        publish();
        return;
      }
      if (session.expectedScrollTop !== null && Math.abs(observed - session.expectedScrollTop) <= 0.5) {
        publish();
        return;
      }
      session.expectedScrollTop = null;
      // Any upward movement the controller did not write leaves the live edge
      // at once, whatever produced it (wheel, scrollbar, touch, a gesture
      // chained out of a nested scroller). Only a scroll that is not upward,
      // or a clamp that lands exactly at the end, may stay or resume
      // following within the live-edge band: the band applies on the way
      // down. Staying in the band on the way up let the next streamed layout
      // pass snap the reader back to the end.
      const end = conversationScrollEnd(viewport.scrollHeight, viewport.clientHeight);
      // A settle is the browser snapping the end of its scroll range, which
      // can lie up to CONVERSATION_END_ROUNDING_PX above the computed end. It
      // follows a downward gesture, or a content press while following (a
      // pointer action can reveal its target before the press, snapping the
      // end differently from the controller's write). With no such input, or
      // after a press that can drag the scroll, a move of any size is the
      // reader's and leaves.
      const settle = previous !== null && previous - observed <= CONVERSATION_END_ROUNDING_PX
        && (session.lastGesture === "down"
          || (session.lastGesture === "content-press" && session.mode === "following-end"
            && end - observed <= CONVERSATION_END_ROUNDING_PX));
      const movedUp = previous !== null && observed < previous - 0.5 && end - observed > 0.5 && !settle;
      // While following, only an upward move leaves. Content can grow before
      // a native scroll event that did not move the reader up (a disclosure
      // opened at the end, then the browser's own scroll after the press), so
      // the band, measured against the grown end, would misread that
      // unchanged position as reading; the next layout pass re-pins the end.
      session.mode = !movedUp && (session.mode === "following-end"
        || conversationIsAtEnd(observed, viewport.scrollHeight, viewport.clientHeight)) ? "following-end" : "reading-history";
      session.pendingSubmittedRow = null;
      session.requestedAnchor = null;
      cancelReveal(session);
      session.missingAnchor = false;
      // Following keeps no anchor (see applyLayout), so a scroll that lands
      // at the live edge, such as a clamp after the content shrinks, need not
      // measure every mounted row.
      session.anchor = session.mode === "following-end" ? null : captureConversationAnchor(rowGeometry(viewport));
      publish();
    };
    const canLeaveLiveEdge = (delta: number) => sessionRef.current?.mode !== "following-end"
      || (delta < 0 ? viewport.scrollTop > 0 : !conversationIsAtEnd(viewport.scrollTop, viewport.scrollHeight, viewport.clientHeight));
    const noteGesture = (delta: number) => {
      const session = sessionRef.current;
      if (session && delta !== 0) session.lastGesture = delta < 0 ? "up" : "down";
    };
    const onWheel = (event: WheelEvent) => {
      noteGesture(event.deltaY);
      if (isConversationScrollTarget(event.target, viewport, event.deltaY, event.deltaX) && canLeaveLiveEdge(event.deltaY)) readHistory();
    };
    const onKey = (event: KeyboardEvent) => {
      if (event.target instanceof Element && event.target.closest("input,textarea,select,[contenteditable=true]")) return;
      const delta = ["ArrowUp", "PageUp", "Home"].includes(event.key) || (event.key === " " && event.shiftKey) ? -1
        : ["ArrowDown", "PageDown", "End", " "].includes(event.key) ? 1 : 0;
      noteGesture(delta);
      if (isConversationScrollTarget(event.target, viewport, delta) && canLeaveLiveEdge(delta)) readHistory();
    };
    const onSelection = () => {
      const selection = viewport.ownerDocument.getSelection();
      if (selection && !selection.isCollapsed && selection.anchorNode && viewport.contains(selection.anchorNode)) readHistory();
    };
    // A browser reveal or focus scroll can precede its scroll event. Capture
    // the visible position before a pointer-triggered render applies layout,
    // otherwise the old anchor can move a button between mouse down and up.
    const onPointerDown = (event: PointerEvent) => {
      // Clicking a tool or copy control at the live edge does not request
      // history. Actual scrolling and selection retain their own handlers.
      const session = sessionRef.current;
      // A press is not a scroll gesture. A primary mouse press on the content
      // cannot scroll it either (a selection drag leaves through
      // onSelection); one on the viewport itself (its scrollbar), a middle
      // press (autoscroll), or a touch or pen press can start a drag, so a
      // move after it is judged on its own.
      if (session) {
        session.lastGesture = event.target !== viewport && event.button === 0
          && event.pointerType !== "touch" && event.pointerType !== "pen"
          ? "content-press" : null;
      }
      if (session?.mode !== "following-end"
        || !conversationIsAtEnd(viewport.scrollTop, viewport.scrollHeight, viewport.clientHeight)) readHistory();
      else {
        // A nearby control can be revealed before its native scroll event.
        // Keep that observed position expected if the click grows its content.
        session.expectedScrollTop = viewport.scrollTop;
        session.lastScrollTop = session.expectedScrollTop;
      }
    };
    viewport.addEventListener("scroll", onScroll, { passive: true });
    viewport.addEventListener("pointerdown", onPointerDown, true);
    viewport.addEventListener("wheel", onWheel, { passive: true });
    viewport.addEventListener("keydown", onKey);
    viewport.addEventListener("load", notifyLayoutChange, true);
    viewport.ownerDocument.addEventListener("selectionchange", onSelection);
    window.addEventListener("resize", notifyLayoutChange);
    // Delivered after layout: apply at once, reading geometry layout already
    // computed, and correcting the scroll position before this frame paints.
    const resize = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(() => applyLayoutRef.current());
    observingResizeRef.current = resize !== null;
    const observeRows = () => {
      resize?.disconnect();
      resize?.observe(viewport);
      if (optionsRef.current.contentRef?.current) resize?.observe(optionsRef.current.contentRef.current);
      else viewport.querySelectorAll(ROW_SELECTOR).forEach((row) => resize?.observe(row));
    };
    observeRows();
    // Re-observing every row is a scan of the whole transcript; streamed
    // text mutates nodes inside a row, so only rows entering or leaving the
    // DOM require it.
    const touchesRows = (nodes: NodeList) => Array.from(nodes).some((node) => node instanceof Element
      && (node.matches(ROW_SELECTOR) || node.querySelector(ROW_SELECTOR) !== null));
    const outsideRows = (node: Node) => !(node instanceof Element ? node : node.parentElement)?.closest(ROW_SELECTOR);
    const mutation = typeof MutationObserver === "undefined" ? null : new MutationObserver((records) => {
      const rowsChanged = records.some((record) => record.type === "childList"
        && (touchesRows(record.addedNodes) || touchesRows(record.removedNodes)));
      if (rowsChanged) observeRows();
      // Text streaming inside a row resizes that row, which the resize
      // observer reports after layout; a frame callback here would read
      // geometry before layout instead. A change outside the rows, such as
      // the older-history control appearing above them, moves rows without
      // resizing any, so it still schedules a pass.
      const moved = rowsChanged || records.some((record) => outsideRows(record.target));
      const session = sessionRef.current;
      if (moved || !resize || !session || !steadySession(session)) notifyLayoutChange();
    });
    mutation?.observe(viewport, { childList: true, subtree: true, characterData: true, attributes: true, attributeFilter: ["open", "hidden"] });
    return () => {
      if (observedSession) {
        if (observedSession.awaitingAnchor) observedSession.requestedAnchor = null;
        cancelReveal(observedSession);
      }
      if (frameRef.current !== null) window.cancelAnimationFrame(frameRef.current);
      frameRef.current = null;
      viewport.removeEventListener("scroll", onScroll);
      viewport.removeEventListener("pointerdown", onPointerDown, true);
      viewport.removeEventListener("wheel", onWheel);
      viewport.removeEventListener("keydown", onKey);
      viewport.removeEventListener("load", notifyLayoutChange, true);
      viewport.ownerDocument.removeEventListener("selectionchange", onSelection);
      window.removeEventListener("resize", notifyLayoutChange);
      resize?.disconnect();
      observingResizeRef.current = false;
      mutation?.disconnect();
      viewport.style.overflowAnchor = previousOverflowAnchor;
      viewport.style.scrollSnapType = previousSnap;
    };
  }, [key, authority, options.viewportRef, notifyLayoutChange, publish, readHistory]);

  return { ...state, jumpToLatest, jumpToRow, readHistory, captureBeforePrepend: readHistory, notifyLayoutChange };
}
