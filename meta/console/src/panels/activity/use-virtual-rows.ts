// Windowed rendering for long panel lists (console v2 spec A4.9: lists over
// 200 rows are virtualised): only the rows near the viewport mount, with a
// spacer above and below whose heights are the other rows' measured heights
// (or an estimate until a row has been seen), so the scroll height holds.
// Rows report their size through `measureRef`. Without a laid-out viewport
// (jsdom, a hidden pane) the first rows mount in full.

import React from "react";

/** Rows mounted when the viewport has no size yet. */
const UNMEASURED_ROWS = 300;

export interface VirtualRowsInput {
  count: number;
  keyAt: (index: number) => string;
  /** A row's height until it has been measured. */
  estimate: (index: number) => number;
  scrollRef: React.RefObject<HTMLElement | null>;
  /** The list inside the scroller, when other content sits above it. */
  listRef?: React.RefObject<HTMLElement | null>;
  /** Pixels mounted above and below the viewport. */
  overscanPx?: number;
}

export interface VirtualRows {
  start: number;
  end: number;
  padTop: number;
  padBottom: number;
  /** Ref callback for the row with this key: keeps its measured height. */
  measureRef: (key: string) => (element: HTMLElement | null) => void;
  /** Scroll the list so the row at `index` shows (at the top, or centred). */
  scrollToIndex: (index: number, align?: "start" | "center") => void;
  /** Whether the list sits at its top edge (the live edge of a newest-first list). */
  atTop: boolean;
}

/** The first index whose row ends after `y`. */
function firstAfter(offsets: readonly number[], y: number): number {
  let lo = 0;
  let hi = offsets.length - 1;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (offsets[mid + 1] > y) hi = mid;
    else lo = mid + 1;
  }
  return lo;
}

export function useVirtualRows({ count, keyAt, estimate, scrollRef, listRef, overscanPx = 600 }: VirtualRowsInput): VirtualRows {
  const heights = React.useRef(new Map<string, number>());
  const elements = React.useRef(new Map<string, HTMLElement>());
  const observer = React.useRef<ResizeObserver | null>(null);
  const callbacks = React.useRef(new Map<string, (element: HTMLElement | null) => void>());
  const [, setVersion] = React.useState(0);
  const [viewport, setViewport] = React.useState({ top: 0, height: 0 });
  const pending = React.useRef<number | null>(null);

  const bump = React.useCallback(() => {
    if (pending.current !== null) return;
    const run = () => {
      pending.current = null;
      setVersion((value) => value + 1);
    };
    pending.current = typeof requestAnimationFrame === "function" ? requestAnimationFrame(run) : (setTimeout(run, 16) as unknown as number);
  }, []);

  React.useEffect(() => {
    const element = scrollRef.current;
    if (!element) return undefined;
    let frame: number | null = null;
    const read = () => {
      frame = null;
      const list = listRef?.current;
      // The list's own top inside the scroller: content above it scrolls first.
      const listTop = list ? list.getBoundingClientRect().top - element.getBoundingClientRect().top + element.scrollTop : 0;
      const top = element.scrollTop - listTop;
      setViewport((current) => (current.top === top && current.height === element.clientHeight
        ? current
        : { top, height: element.clientHeight }));
    };
    const onScroll = () => {
      if (frame !== null) return;
      frame = typeof requestAnimationFrame === "function" ? requestAnimationFrame(read) : (setTimeout(read, 16) as unknown as number);
    };
    read();
    element.addEventListener("scroll", onScroll, { passive: true });
    const resize = typeof ResizeObserver === "function" ? new ResizeObserver(read) : null;
    resize?.observe(element);
    return () => {
      element.removeEventListener("scroll", onScroll);
      resize?.disconnect();
      if (frame !== null && typeof cancelAnimationFrame === "function") cancelAnimationFrame(frame);
    };
  }, [scrollRef, listRef]);

  React.useEffect(() => {
    if (typeof ResizeObserver !== "function") return undefined;
    const rows = new ResizeObserver((entries) => {
      let changed = false;
      for (const entry of entries) {
        const element = entry.target as HTMLElement;
        const key = element.dataset.virtualKey;
        if (!key) continue;
        const height = Math.round(element.getBoundingClientRect().height);
        if (height > 0 && heights.current.get(key) !== height) {
          heights.current.set(key, height);
          changed = true;
        }
      }
      if (changed) bump();
    });
    observer.current = rows;
    for (const element of elements.current.values()) rows.observe(element);
    return () => {
      rows.disconnect();
      observer.current = null;
    };
  }, [bump]);

  React.useEffect(() => () => {
    if (pending.current !== null && typeof cancelAnimationFrame === "function") cancelAnimationFrame(pending.current);
  }, []);

  const measureRef = React.useCallback((key: string) => {
    let callback = callbacks.current.get(key);
    if (callback) return callback;
    callback = (element: HTMLElement | null) => {
      const previous = elements.current.get(key);
      if (previous && previous !== element) {
        observer.current?.unobserve(previous);
        elements.current.delete(key);
      }
      if (!element) return;
      element.dataset.virtualKey = key;
      elements.current.set(key, element);
      observer.current?.observe(element);
    };
    callbacks.current.set(key, callback);
    if (callbacks.current.size > 4 * UNMEASURED_ROWS) {
      // Forget callbacks of rows long gone; React asks again when they remount.
      const first = callbacks.current.keys().next().value;
      if (first !== undefined && first !== key && !elements.current.has(first)) callbacks.current.delete(first);
    }
    return callback;
  }, []);

  const offsets: number[] = new Array(count + 1);
  offsets[0] = 0;
  for (let index = 0; index < count; index += 1) {
    offsets[index + 1] = offsets[index] + (heights.current.get(keyAt(index)) ?? estimate(index));
  }
  const total = offsets[count] ?? 0;
  let start = 0;
  let end = count;
  if (viewport.height > 0 && count > 0) {
    start = firstAfter(offsets, Math.max(0, viewport.top - overscanPx));
    end = Math.min(count, firstAfter(offsets, Math.max(0, viewport.top + viewport.height + overscanPx)) + 1);
  } else {
    end = Math.min(count, UNMEASURED_ROWS);
  }
  const padTop = offsets[start] ?? 0;
  const padBottom = Math.max(0, total - (offsets[end] ?? total));

  const scrollToIndex = React.useCallback((index: number, align: "start" | "center" = "start") => {
    const element = scrollRef.current;
    if (!element) return;
    const clamped = Math.max(0, Math.min(count - 1, index));
    const list = listRef?.current;
    const listTop = list ? list.getBoundingClientRect().top - element.getBoundingClientRect().top + element.scrollTop : 0;
    const top = (offsets[clamped] ?? 0) + listTop;
    const height = (offsets[clamped + 1] ?? top) - top;
    const target = align === "center" ? top - Math.max(0, (element.clientHeight - height) / 2) : top;
    element.scrollTo?.({ top: Math.max(0, target) });
    if (!element.scrollTo) element.scrollTop = Math.max(0, target);
    // `offsets` is this render's: the latest layout.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [count, total, scrollRef, listRef]);

  return { start, end, padTop, padBottom, measureRef, scrollToIndex, atTop: viewport.top <= 4 };
}
