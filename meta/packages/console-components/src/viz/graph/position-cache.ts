// Positions cached per node id (design spec A3.9), so a refresh never
// reshuffles the picture: sessionStorage under meta-console-viz:<key>, where
// the key is "<panel>:<scope>". Bounded, and quiet when storage is missing
// or full.

import type { LayoutPoint } from "../layout/types";

export const POSITION_CACHE_PREFIX = "meta-console-viz:";
/** At most this many nodes per key; the rest are not cached. */
export const POSITION_CACHE_LIMIT = 2000;

function storage(): Storage | null {
  try {
    return typeof sessionStorage === "undefined" ? null : sessionStorage;
  } catch {
    return null;
  }
}

/** The cached positions for `key`, or an empty map. */
export function readPositionCache(key: string): Map<string, LayoutPoint> {
  const store = storage();
  const result = new Map<string, LayoutPoint>();
  if (!store || !key) return result;
  try {
    const raw = store.getItem(POSITION_CACHE_PREFIX + key);
    if (!raw) return result;
    const parsed: unknown = JSON.parse(raw);
    if (!parsed || typeof parsed !== "object" || !("p" in parsed)) return result;
    const entries = (parsed as { p: unknown }).p;
    if (!entries || typeof entries !== "object") return result;
    for (const [id, value] of Object.entries(entries as Record<string, unknown>)) {
      if (!Array.isArray(value) || value.length !== 2) continue;
      const [x, y] = value;
      if (typeof x === "number" && typeof y === "number" && Number.isFinite(x) && Number.isFinite(y)) result.set(id, { x, y });
    }
  } catch {
    return new Map();
  }
  return result;
}

/** Stores `positions` for `key` (rounded to 0.1 px), replacing what was there. */
export function writePositionCache(key: string, positions: ReadonlyMap<string, LayoutPoint>): void {
  const store = storage();
  if (!store || !key) return;
  const p: Record<string, [number, number]> = {};
  let count = 0;
  for (const [id, point] of positions) {
    if (count >= POSITION_CACHE_LIMIT) break;
    p[id] = [Math.round(point.x * 10) / 10, Math.round(point.y * 10) / 10];
    count += 1;
  }
  try {
    store.setItem(POSITION_CACHE_PREFIX + key, JSON.stringify({ v: 1, p }));
  } catch {
    // Full or blocked storage: the picture is recomputed next time.
  }
}

const MINIMAP_PREFIX = "meta-console-viz-minimap:";

/** The panel part of a cache key ("topology:ops" -> "topology"). */
export function panelOfKey(key: string | undefined): string {
  return key ? key.split(":")[0] || key : "";
}

/** Whether the person hid the minimap in this panel (persisted in localStorage). */
export function readMinimapHidden(panel: string): boolean {
  if (!panel) return false;
  try {
    return typeof localStorage !== "undefined" && localStorage.getItem(MINIMAP_PREFIX + panel) === "hidden";
  } catch {
    return false;
  }
}

export function writeMinimapHidden(panel: string, hidden: boolean): void {
  if (!panel) return;
  try {
    if (typeof localStorage === "undefined") return;
    if (hidden) localStorage.setItem(MINIMAP_PREFIX + panel, "hidden");
    else localStorage.removeItem(MINIMAP_PREFIX + panel);
  } catch {
    // Storage blocked: the toggle lasts for this view only.
  }
}
