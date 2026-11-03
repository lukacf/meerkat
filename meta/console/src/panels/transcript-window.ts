import * as React from "react";

/// Transcript windowing by measured turn heights (#544).
///
/// Only the turns near the viewport are mounted. A run of other turns is
/// replaced by one spacer whose height is the sum of those turns' measured
/// border-box heights plus the flex gaps between them, so the scroll height
/// and everything under the viewport stay exactly where they were. A turn is
/// only ever represented by a spacer once it has been measured at the
/// current width with its current content; any other turn stays mounted. No
/// height is estimated.

/** Viewport heights mounted above and below the viewport. */
export const TURN_WINDOW_OVERSCAN = 1.5;
/** The window moves once the viewport comes this close (in viewport heights) to its edge. */
export const TURN_WINDOW_MARGIN = 0.5;
/** Measured turns this many either side of the window stay in the DOM,
 * parked (`hidden="until-found"` at their measured size), so the browser's
 * find-in-page still reaches them. */
export const TURN_FIND_BAND = 20;

export interface TurnMeasurement {
  /** Border-box block size in CSS pixels. */
  height: number;
  /** Render key the turn was measured under: its content and anything else
   * that changes how it renders (such as the day it follows). */
  key: string;
  /** Content width the turn was measured at. */
  width: number;
}

export interface TurnWindowInput {
  /** Turns in transcript order (the revealed range). */
  turns: readonly { id: string }[];
  measurements: ReadonlyMap<string, TurnMeasurement>;
  /** Each turn's render key now. */
  keys: readonly string[];
  /** Content width of the transcript now. */
  width: number;
  /** Flex gap between turns. */
  gap: number;
  /** Offset of the first turn's top within the scroll content. */
  top: number;
  scrollTop: number;
  viewportHeight: number;
  /** Indexes currently mounted around the viewport, for hysteresis. */
  current: { from: number; to: number } | null;
  /** Indexes that must stay mounted: a selection, keyboard focus, a requested jump. */
  pinned: ReadonlySet<number>;
}

export interface TurnWindowPlan {
  /** The window around the viewport, half-open. */
  range: { from: number; to: number };
  /** Every mounted index, ascending. */
  mounted: number[];
  /** Measured indexes kept in the DOM but parked, ascending. */
  parked: number[];
}

function measured(input: TurnWindowInput, index: number): TurnMeasurement | null {
  const turn = input.turns[index];
  const measurement = input.measurements.get(turn.id);
  return measurement && measurement.key === input.keys[index] && measurement.width === input.width ? measurement : null;
}

/// Which turns to mount. Pure: the hook supplies geometry it read after layout.
export function planTurnWindow(input: TurnWindowInput): TurnWindowPlan {
  const count = input.turns.length;
  if (count === 0) return { range: { from: 0, to: 0 }, mounted: [], parked: [] };
  // Turn tops from measured heights. A turn without a valid measurement is
  // mounted regardless, so its slot is only used for placing the viewport.
  const tops = new Array<number>(count + 1);
  tops[0] = input.top;
  let anyUnmeasured = false;
  for (let i = 0; i < count; i += 1) {
    const measurement = measured(input, i);
    if (!measurement) anyUnmeasured = true;
    tops[i + 1] = tops[i] + (measurement?.height ?? 0) + input.gap;
  }
  const viewTop = input.scrollTop;
  const viewBottom = input.scrollTop + input.viewportHeight;
  const firstEndingAfter = (y: number) => {
    let lo = 0;
    let hi = count;
    while (lo < hi) {
      const mid = (lo + hi) >>> 1;
      if (tops[mid + 1] - input.gap > y) hi = mid;
      else lo = mid + 1;
    }
    return lo;
  };
  const firstStartingAfter = (y: number) => {
    let lo = 0;
    let hi = count;
    while (lo < hi) {
      const mid = (lo + hi) >>> 1;
      if (tops[mid] > y) hi = mid;
      else lo = mid + 1;
    }
    return lo;
  };
  const span = (above: number, below: number) => ({
    from: Math.min(count - 1, firstEndingAfter(viewTop - above * input.viewportHeight)),
    to: Math.max(1, firstStartingAfter(viewBottom + below * input.viewportHeight)),
  });
  // Keep the current window while it still covers the viewport with margin.
  const needed = span(TURN_WINDOW_MARGIN, TURN_WINDOW_MARGIN);
  const current = input.current;
  const range = !anyUnmeasured && current && current.from <= needed.from && current.to >= needed.to && current.to <= count
    ? current
    : span(TURN_WINDOW_OVERSCAN, TURN_WINDOW_OVERSCAN);
  const mounted: number[] = [];
  const parked: number[] = [];
  for (let i = 0; i < count; i += 1) {
    if ((i >= range.from && i < range.to) || i === count - 1 || input.pinned.has(i) || !measured(input, i)) mounted.push(i);
    else if (i >= range.from - TURN_FIND_BAND && i < range.to + TURN_FIND_BAND) parked.push(i);
  }
  return { range, mounted, parked };
}

/** A transcript slot: a mounted turn, or a spacer standing for a run of turns. */
export type TurnSlot =
  | { kind: "turn"; index: number }
  | { kind: "parked"; index: number; height: number }
  | { kind: "spacer"; from: number; to: number; height: number };

/// Slots in order. A spacer for turns [from, to) is their heights plus the
/// gaps between them; the flex gap around it is the gap around any turn.
export function turnSlots(
  turns: readonly { id: string }[],
  mounted: readonly number[],
  measurements: ReadonlyMap<string, TurnMeasurement>,
  gap: number,
  parked: readonly number[] = [],
): TurnSlot[] {
  const slots: TurnSlot[] = [];
  let next = 0;
  const spacer = (from: number, to: number) => {
    if (from >= to) return;
    let height = gap * (to - from - 1);
    for (let i = from; i < to; i += 1) height += measurements.get(turns[i].id)?.height ?? 0;
    slots.push({ kind: "spacer", from, to, height });
  };
  const isMounted = new Set(mounted);
  for (const index of [...new Set([...mounted, ...parked])].sort((a, b) => a - b)) {
    spacer(next, index);
    slots.push(isMounted.has(index)
      ? { kind: "turn", index }
      : { kind: "parked", index, height: measurements.get(turns[index].id)?.height ?? 0 });
    next = index + 1;
  }
  spacer(next, turns.length);
  return slots;
}

const NO_ACTIONABLE_TURNS: ReadonlySet<string> = new Set();

export interface TurnWindow {
  slots: TurnSlot[];
  /** Mount a turn now (a jump or restore); false when it is not in the revealed range. */
  mount: (index: number) => boolean;
  /** Mount the turn with this id (once it is revealed) and move keyboard focus to it. */
  focusTurn: (id: string) => void;
}

/// Window the turns rendered into `bodyRef`, a flex column whose direct
/// children include `[data-conversation-turn-id]` turn elements. Disabled,
/// every turn is mounted (the unwindowed oracle).
export function useTurnWindow<T extends { id: string }>(
  bodyRef: React.RefObject<HTMLElement | null>,
  turns: readonly T[],
  enabled: boolean,
  /** A turn's render key: equal keys render equal heights at one width. */
  renderKey: (turn: T, index: number) => string,
  /** Turns holding something the reader must act on, such as a pending
   * approval: mounted wherever the reader is, so windowing never hides it. */
  actionable: ReadonlySet<string> = NO_ACTIONABLE_TURNS,
): TurnWindow {
  const measurements = React.useRef(new Map<string, TurnMeasurement>());
  const geometry = React.useRef({ width: 0, gap: 0, top: 0, viewportHeight: 0 });
  const turnsRef = React.useRef(turns);
  turnsRef.current = turns;
  // Keys rather than turn objects: a full transcript derivation after the
  // log trims rebuilds every turn object with unchanged content, which must
  // not remount every turn to measure it again.
  const keys = React.useMemo(() => turns.map(renderKey), [turns, renderKey]);
  const keysRef = React.useRef(keys);
  keysRef.current = keys;
  const pins = React.useRef({ selection: new Set<number>(), focus: new Set<number>(), jump: new Set<number>() });
  const actionableRef = React.useRef(actionable);
  actionableRef.current = actionable;
  const [plan, setPlan] = React.useState<TurnWindowPlan | null>(null);
  const planRef = React.useRef(plan);
  planRef.current = plan;

  const replan = React.useCallback(() => {
    const body = bodyRef.current;
    if (!enabled || !body) return;
    const current = turnsRef.current;
    const pinned = new Set<number>([...pins.current.selection, ...pins.current.focus, ...pins.current.jump]);
    if (actionableRef.current.size > 0) {
      current.forEach((turn, index) => { if (actionableRef.current.has(turn.id)) pinned.add(index); });
    }
    const next = planTurnWindow({
      turns: current,
      measurements: measurements.current,
      keys: keysRef.current,
      ...geometry.current,
      scrollTop: body.scrollTop,
      current: planRef.current?.range ?? null,
      pinned,
    });
    const previous = planRef.current;
    if (previous && previous.mounted.length === next.mounted.length
      && previous.mounted.every((index, i) => index === next.mounted[i])
      && previous.range.from === next.range.from && previous.range.to === next.range.to) return;
    planRef.current = next;
    // Test-only accounting on the render-count sink (console render-counts).
    const sink = (globalThis as { __consoleRenderCounts?: Record<string, number> }).__consoleRenderCounts;
    if (sink) sink.TurnWindowPlans = (sink.TurnWindowPlans ?? 0) + 1;
    setPlan(next);
  }, [bodyRef, enabled]);

  // Measure after layout: a ResizeObserver callback reads geometry the frame
  // already computed. Turns are observed as they mount.
  React.useLayoutEffect(() => {
    const body = bodyRef.current;
    if (!enabled || !body || typeof ResizeObserver === "undefined") return;
    const indexOf = new Map<Element, string>();
    const observer = new ResizeObserver((entries) => {
      const style = getComputedStyle(body);
      const width = body.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight);
      const gap = parseFloat(style.rowGap) || 0;
      geometry.current = { ...geometry.current, width, gap, viewportHeight: body.clientHeight };
      const indexById = new Map(turnsRef.current.map((turn, index) => [turn.id, index] as const));
      for (const entry of entries) {
        const id = indexOf.get(entry.target);
        if (id === undefined) continue;
        const index = indexById.get(id);
        // A parked turn reports its placeholder size, not a measurement.
        if (index === undefined || !entry.target.isConnected || entry.target.hasAttribute("hidden")) continue;
        const height = entry.borderBoxSize?.[0]?.blockSize ?? (entry.target as HTMLElement).offsetHeight;
        measurements.current.set(id, { height, key: keysRef.current[index], width });
      }
      const first = body.querySelector<HTMLElement>(":scope > [data-conversation-turn-id], :scope > [data-conversation-spacer]");
      if (first) geometry.current.top = first.offsetTop;
      replan();
    });
    observer.observe(body);
    const observeTurns = () => {
      // Unmounted turns are unobserved: the observer checks every target on
      // every frame, and a detached turn must not be retained.
      for (const element of [...indexOf.keys()]) {
        if (element.parentNode === body) continue;
        observer.unobserve(element);
        indexOf.delete(element);
      }
      for (const element of body.querySelectorAll(":scope > [data-conversation-turn-id]")) {
        if (indexOf.has(element)) continue;
        indexOf.set(element, (element as HTMLElement).dataset.conversationTurnId!);
        observer.observe(element);
      }
    };
    observeTurns();
    const mutation = new MutationObserver(observeTurns);
    mutation.observe(body, { childList: true });
    let frame = 0;
    const onScroll = () => {
      if (frame) return;
      frame = window.requestAnimationFrame(() => { frame = 0; replan(); });
    };
    body.addEventListener("scroll", onScroll, { passive: true });
    // Never unmount what the reader is selecting or focused in.
    const turnIndex = (node: Node | null) => {
      const element = node instanceof Element ? node : node?.parentElement;
      const turn = element?.closest<HTMLElement>("[data-conversation-turn-id]");
      if (!turn || !body.contains(turn)) return -1;
      return turnsRef.current.findIndex((candidate) => candidate.id === turn.dataset.conversationTurnId);
    };
    const onSelection = () => {
      const selection = body.ownerDocument.getSelection();
      const next = new Set<number>();
      if (selection && !selection.isCollapsed) {
        const a = turnIndex(selection.anchorNode);
        const b = turnIndex(selection.focusNode);
        if (a >= 0 || b >= 0) {
          const from = Math.min(a >= 0 ? a : b, b >= 0 ? b : a);
          const to = Math.max(a, b);
          for (let i = from; i <= to; i += 1) next.add(i);
        }
      }
      const same = next.size === pins.current.selection.size && [...next].every((i) => pins.current.selection.has(i));
      if (same) return;
      pins.current.selection = next;
      replan();
    };
    const onFocus = () => {
      const index = turnIndex(body.ownerDocument.activeElement);
      pins.current.focus = index >= 0 ? new Set([index]) : new Set();
      replan();
    };
    // Find-in-page reveals a parked turn: keep it mounted where the match is.
    const onBeforeMatch = (event: Event) => {
      const index = turnIndex(event.target as Node);
      if (index < 0) return;
      pins.current.jump = new Set([index]);
      replan();
    };
    body.addEventListener("beforematch", onBeforeMatch, true);
    body.ownerDocument.addEventListener("selectionchange", onSelection);
    body.addEventListener("focusin", onFocus);
    body.addEventListener("focusout", onFocus);
    return () => {
      observer.disconnect();
      mutation.disconnect();
      if (frame) window.cancelAnimationFrame(frame);
      body.removeEventListener("scroll", onScroll);
      body.removeEventListener("beforematch", onBeforeMatch, true);
      body.ownerDocument.removeEventListener("selectionchange", onSelection);
      body.removeEventListener("focusin", onFocus);
      body.removeEventListener("focusout", onFocus);
    };
  }, [bodyRef, enabled, replan]);

  // A mounted turn's size is tracked by the observer, so its measurement
  // holds for whatever key it renders under now. A turn whose key changes
  // while unmounted is measured again by mounting it.
  React.useLayoutEffect(() => {
    if (!enabled) return;
    const body = bodyRef.current;
    if (body) {
      const indexById = new Map(turns.map((turn, index) => [turn.id, index] as const));
      for (const element of body.querySelectorAll<HTMLElement>(":scope > [data-conversation-turn-id]")) {
        const index = indexById.get(element.dataset.conversationTurnId!);
        const measurement = index === undefined ? undefined : measurements.current.get(turns[index].id);
        if (index !== undefined && measurement && !element.hasAttribute("hidden")) measurement.key = keys[index];
      }
    }
    // No replan here: reading scrollTop mid-commit forces layout. Changed
    // turns mount (see slots) and the observer replans after layout.
  }, [turns, keys, enabled, bodyRef]);

  // A card that appears inside an unmounted turn changes no mounted element,
  // so nothing else would replan to mount it.
  const actionableKey = [...actionable].sort().join("\n");
  const actionableKeyRef = React.useRef(actionableKey);
  React.useEffect(() => {
    if (actionableKeyRef.current === actionableKey) return;
    actionableKeyRef.current = actionableKey;
    replan();
  }, [actionableKey, replan]);

  const mount = React.useCallback((index: number) => {
    if (index < 0 || index >= turnsRef.current.length) return false;
    // Pinned until the reader scrolls: the window then forms around it.
    pins.current.jump = new Set([index]);
    replan();
    const body = bodyRef.current;
    const release = () => {
      body?.removeEventListener("wheel", release);
      body?.removeEventListener("pointerdown", release);
      body?.removeEventListener("keydown", release);
      pins.current.jump = new Set();
    };
    body?.addEventListener("wheel", release, { passive: true });
    body?.addEventListener("pointerdown", release);
    body?.addEventListener("keydown", release);
    return true;
  }, [bodyRef, replan]);

  // Keyboard focus moving to a turn that is not mounted yet lands once it is.
  const pendingFocus = React.useRef<string | null>(null);
  const focusTurn = React.useCallback((id: string) => {
    pendingFocus.current = id;
    const index = turnsRef.current.findIndex((turn) => turn.id === id);
    if (index >= 0) {
      pins.current.focus = new Set([index]);
      mount(index);
    }
    // Already mounted: focus now; otherwise the layout effect below does.
    const element = bodyRef.current?.querySelector<HTMLElement>(`:scope > [data-conversation-turn-id="${CSS.escape(id)}"]:not([hidden])`);
    if (element) { pendingFocus.current = null; element.focus(); }
  }, [bodyRef, mount]);

  const slots = React.useMemo(() => {
    if (!enabled || !plan) return turns.map((_, index): TurnSlot => ({ kind: "turn", index }));
    // Indexes from a plan for an older turn list are re-derived next replan;
    // anything new is mounted until then.
    const mounted = new Set(plan.mounted.filter((index) => index < turns.length));
    for (let i = 0; i < turns.length; i += 1) {
      const measurement = measurements.current.get(turns[i].id);
      if (!measurement || measurement.key !== keys[i] || measurement.width !== geometry.current.width) mounted.add(i);
    }
    const parked = plan.parked.filter((index) => index < turns.length && !mounted.has(index));
    return turnSlots(turns, [...mounted].sort((a, b) => a - b), measurements.current, geometry.current.gap, parked);
  }, [enabled, plan, turns, keys]);

  React.useLayoutEffect(() => {
    const id = pendingFocus.current;
    if (!id) return;
    const element = bodyRef.current?.querySelector<HTMLElement>(`:scope > [data-conversation-turn-id="${CSS.escape(id)}"]:not([hidden])`);
    if (!element) return;
    pendingFocus.current = null;
    element.focus();
  });

  return React.useMemo(() => ({ slots, mount, focusTurn }), [slots, mount, focusTurn]);
}
