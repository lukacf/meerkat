export const SIDEBAR_PINS_STORAGE_PREFIX = "meta-console-sidebar-pins";
export const SIDEBAR_SECTION_ORDER_STORAGE_PREFIX = "meta-console-sidebar-section-order";
export const SIDEBAR_SUBGROUP_ORDER_STORAGE_PREFIX = "meta-console-sidebar-subgroup-order";
export const SECTION_COLLAPSE_STORAGE_PREFIX = "meta-console-sidebar-sections";
export const SUBGROUP_COLLAPSE_STORAGE_PREFIX = "meta-console-sidebar-subgroups";

export const SIDEBAR_STORAGE_PREFIXES = [
  SIDEBAR_PINS_STORAGE_PREFIX,
  SIDEBAR_SECTION_ORDER_STORAGE_PREFIX,
  SIDEBAR_SUBGROUP_ORDER_STORAGE_PREFIX,
  SECTION_COLLAPSE_STORAGE_PREFIX,
  SUBGROUP_COLLAPSE_STORAGE_PREFIX,
] as const;

export type ConsoleSidebarDropPosition = "before" | "after";

export interface ConsoleSidebarStorageLike {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

export interface ConsoleSidebarEnumerableStorage {
  readonly length: number;
  key(index: number): string | null;
  removeItem(key: string): void;
}

export function sidebarStorageKey(prefix: string, namespace: string | undefined): string {
  return `${prefix}:${namespace?.trim() || "default"}`;
}

export function readSidebarStringSet(
  storage: ConsoleSidebarStorageLike | null | undefined,
  key: string,
): Set<string> | null {
  if (!storage) return null;
  try {
    const raw = storage.getItem(key);
    if (raw === null) return null;
    const parsed = JSON.parse(raw);
    if (!Array.isArray(parsed)) return new Set();
    return new Set(parsed.filter((value): value is string => typeof value === "string" && value.trim().length > 0));
  } catch {
    return null;
  }
}

export function writeSidebarStringSet(
  storage: ConsoleSidebarStorageLike | null | undefined,
  key: string,
  value: Set<string>,
): void {
  if (!storage) return;
  try {
    const normalized = Array.from(value)
      .map((item) => item.trim())
      .filter(Boolean)
      .sort();
    storage.setItem(key, JSON.stringify(Array.from(new Set(normalized))));
  } catch {
    /* ignore */
  }
}

export function readSidebarStringList(
  storage: ConsoleSidebarStorageLike | null | undefined,
  key: string,
): string[] | null {
  if (!storage) return null;
  try {
    const raw = storage.getItem(key);
    if (raw === null) return null;
    const parsed = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    const seen = new Set<string>();
    const out: string[] = [];
    for (const value of parsed) {
      if (typeof value !== "string") continue;
      const trimmed = value.trim();
      if (!trimmed || seen.has(trimmed)) continue;
      seen.add(trimmed);
      out.push(trimmed);
    }
    return out;
  } catch {
    return null;
  }
}

export function writeSidebarStringList(
  storage: ConsoleSidebarStorageLike | null | undefined,
  key: string,
  value: string[],
): void {
  if (!storage) return;
  try {
    const seen = new Set<string>();
    const normalized = value
      .map((item) => item.trim())
      .filter((item) => {
        if (!item || seen.has(item)) return false;
        seen.add(item);
        return true;
      });
    storage.setItem(key, JSON.stringify(normalized));
  } catch {
    /* ignore */
  }
}

export function pruneStaleSidebarStorage(
  storage: ConsoleSidebarEnumerableStorage | null | undefined,
  scope: string,
  activeNamespace: string,
): void {
  if (!storage) return;
  try {
    const scopePrefix = encodeURIComponent(scope.trim());
    const activeKeys = new Set(SIDEBAR_STORAGE_PREFIXES.map((prefix) => `${prefix}:${activeNamespace}`));
    const stale: string[] = [];
    for (let i = 0; i < storage.length; i += 1) {
      const key = storage.key(i);
      if (!key || activeKeys.has(key)) continue;
      if (SIDEBAR_STORAGE_PREFIXES.some((prefix) => key.startsWith(`${prefix}:${scopePrefix}:`))) {
        stale.push(key);
      }
    }
    for (const key of stale) storage.removeItem(key);
  } catch {
    /* ignore */
  }
}

export function applyConsoleSidebarOrder(items: string[], storedOrder: string[] | null | undefined): string[] {
  const available = new Set(items);
  const seen = new Set<string>();
  const ordered: string[] = [];
  for (const item of storedOrder || []) {
    if (!available.has(item) || seen.has(item)) continue;
    ordered.push(item);
    seen.add(item);
  }
  for (const item of items) {
    if (seen.has(item)) continue;
    ordered.push(item);
    seen.add(item);
  }
  return ordered;
}

export function reorderConsoleSidebarOrder(
  items: string[],
  dragged: string,
  target: string,
  where: ConsoleSidebarDropPosition,
): string[] {
  if (dragged === target || !items.includes(dragged) || !items.includes(target)) return items;
  const withoutDragged = items.filter((item) => item !== dragged);
  const targetIndex = withoutDragged.indexOf(target);
  if (targetIndex < 0) return items;
  const insertAt = where === "after" ? targetIndex + 1 : targetIndex;
  const next = [...withoutDragged];
  next.splice(insertAt, 0, dragged);
  return next;
}
