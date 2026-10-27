export const CONVERSATION_LIVE_EDGE_PX = 32;
export const CONVERSATION_ANCHOR_OFFSET_PX = 24;
/**
 * How far the browser's own end of the scroll range can sit above the
 * computed end, `scrollHeight - clientHeight`. Both heights are whole numbers
 * rounded from fractional layout sizes (fractional device pixel ratios, zoom,
 * sub-pixel line heights), so each is off by at most half a pixel and their
 * difference by less than one. The browser clamps to the true, unrounded end
 * and snaps scrollTop to a whole pixel, and different paths (a wheel, a key, a
 * reveal before a click) can snap that same end differently. A position within
 * this distance of the computed end is therefore the end itself, as far as
 * integer geometry can tell.
 */
export const CONVERSATION_END_ROUNDING_PX = 1;
export const CONVERSATION_POSITION_LIMIT = 100;

export type ConversationScrollMode = "following-end" | "anchoring-submitted-turn" | "reading-history";
export type ConversationRowGeometry = { id: string; top: number; bottom: number };
export type ConversationScrollAnchor = {
  rowId: string;
  offset: number;
  /** Nearby retained rows permit a stable fallback if the anchor is pruned. */
  neighbors: { rowId: string; offset: number }[];
};
export type ConversationScrollPosition = {
  mode: ConversationScrollMode;
  anchor: ConversationScrollAnchor | null;
  scrollTop: number;
  /** Last accepted submission already consumed by this viewport. */
  lastSubmittedRow?: string | null;
  pendingSubmittedRow?: string | null;
};

export function conversationScrollEnd(scrollHeight: number, clientHeight: number): number {
  return Math.max(0, scrollHeight - clientHeight);
}

export function conversationIsAtEnd(scrollTop: number, scrollHeight: number, clientHeight: number): boolean {
  return conversationScrollEnd(scrollHeight, clientHeight) - Math.max(0, scrollTop) <= CONVERSATION_LIVE_EDGE_PX;
}

/** Row coordinates are relative to the viewport's inner top edge. */
export function captureConversationAnchor(rows: readonly ConversationRowGeometry[]): ConversationScrollAnchor | null {
  if (!rows.length) return null;
  let index = rows.findIndex((row) => row.bottom > 0);
  if (index < 0) index = rows.length - 1;
  const row = rows[index];
  const neighbors = rows
    .map((candidate, candidateIndex) => ({ rowId: candidate.id, offset: candidate.top, distance: Math.abs(candidateIndex - index) }))
    .filter((candidate) => candidate.rowId !== row.id)
    .sort((a, b) => a.distance - b.distance)
    .slice(0, 6)
    .map(({ rowId, offset }) => ({ rowId, offset }));
  return { rowId: row.id, offset: row.top, neighbors };
}

export function restoreConversationAnchor(
  anchor: ConversationScrollAnchor,
  rows: readonly ConversationRowGeometry[],
  scrollTop: number,
  maxScroll: number,
): { scrollTop: number; rowId: string; exact: boolean } | null {
  const retained = new Map(rows.map((row) => [row.id, row]));
  const target = [{ rowId: anchor.rowId, offset: anchor.offset }, ...anchor.neighbors]
    .find((candidate) => retained.has(candidate.rowId));
  if (!target) return null;
  const row = retained.get(target.rowId)!;
  return {
    scrollTop: Math.max(0, Math.min(maxScroll, scrollTop + row.top - target.offset)),
    rowId: target.rowId,
    exact: target.rowId === anchor.rowId,
  };
}

/** A horizontal gesture or scrollable nested tool/code panel owns its gesture. */
export function isConversationScrollTarget(target: EventTarget | null, viewport: HTMLElement, deltaY: number, deltaX = 0): boolean {
  if (!(target instanceof Element) || !viewport.contains(target) || deltaY === 0 || Math.abs(deltaX) > Math.abs(deltaY)) return false;
  for (let element: Element | null = target; element && element !== viewport; element = element.parentElement) {
    const style = getComputedStyle(element);
    if (style.overflowY !== "auto" && style.overflowY !== "scroll") continue;
    const canScroll = deltaY < 0
      ? element.scrollTop > 0
      : element.scrollTop < element.scrollHeight - element.clientHeight;
    if (canScroll || style.overscrollBehaviorY === "contain" || style.overscrollBehaviorY === "none") return false;
  }
  return true;
}

/** No label-derived singleton state: callers explicitly own the cache namespace. */
export class ConversationPositionCache {
  private readonly entries = new Map<string, ConversationScrollPosition>();
  get size(): number { return this.entries.size; }
  read(key: string): ConversationScrollPosition | undefined { return this.entries.get(key); }
  remember(key: string, position: ConversationScrollPosition): void {
    this.entries.delete(key);
    this.entries.set(key, position);
    if (this.entries.size > CONVERSATION_POSITION_LIMIT) this.entries.delete(this.entries.keys().next().value!);
  }
  clear(): void { this.entries.clear(); }
  deleteAuthority(authority: string): void {
    for (const key of this.entries.keys()) {
      if (JSON.parse(key)[0] === authority) this.entries.delete(key);
    }
  }
}
