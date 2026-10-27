import assert from "node:assert/strict";

import {
  SECTION_COLLAPSE_STORAGE_PREFIX,
  SIDEBAR_PINS_STORAGE_PREFIX,
  SUBGROUP_COLLAPSE_STORAGE_PREFIX,
  applyConsoleSidebarOrder,
  pruneStaleSidebarStorage,
  readSidebarStringList,
  readSidebarStringSet,
  reorderConsoleSidebarOrder,
  sidebarStorageKey,
  writeSidebarStringList,
  writeSidebarStringSet,
} from "./sidebar-preferences";

// Dual-runner: meta's console gates bundle this file with esbuild and run it
// under `node --test`; the meerkat-studio desktop app picks it up with vitest
// (globals enabled). Resolve the registrar for whichever runner is active.
type TestFn = (name: string, fn: () => void | Promise<void>) => void;
const nodeTestModule = "node:test";
const test: TestFn = process.env.VITEST
  ? ((globalThis as Record<string, unknown>).test as TestFn)
  : ((await import(/* @vite-ignore */ nodeTestModule)).default as unknown as TestFn);

test("sidebar preference storage normalizes string sets and lists", () => {
  const storage = new MemoryStorage();
  const setKey = sidebarStorageKey(SIDEBAR_PINS_STORAGE_PREFIX, "runtime-a");
  const listKey = sidebarStorageKey(SECTION_COLLAPSE_STORAGE_PREFIX, "runtime-a");

  writeSidebarStringSet(storage, setKey, new Set(["beta", "alpha", ""]));
  writeSidebarStringList(storage, listKey, [" beta ", "alpha", "beta", ""]);

  assert.deepEqual(Array.from(readSidebarStringSet(storage, setKey) || []), ["alpha", "beta"]);
  assert.deepEqual(readSidebarStringList(storage, listKey), ["beta", "alpha"]);
});

test("sidebar preference pruning removes stale namespaces only in the active scope", () => {
  const storage = new MemoryStorage();
  const activeNamespace = `${encodeURIComponent("runtime-a")}:hash-new`;
  const staleNamespace = `${encodeURIComponent("runtime-a")}:hash-old`;
  const otherScopeNamespace = `${encodeURIComponent("runtime-b")}:hash-old`;
  const activeKey = sidebarStorageKey(SIDEBAR_PINS_STORAGE_PREFIX, activeNamespace);
  const staleKey = sidebarStorageKey(SECTION_COLLAPSE_STORAGE_PREFIX, staleNamespace);
  const otherKey = sidebarStorageKey(SUBGROUP_COLLAPSE_STORAGE_PREFIX, otherScopeNamespace);

  writeSidebarStringSet(storage, activeKey, new Set(["identity:keep"]));
  writeSidebarStringSet(storage, staleKey, new Set(["agents"]));
  writeSidebarStringSet(storage, otherKey, new Set(["[\"agents\",\"ops\"]"]));

  pruneStaleSidebarStorage(storage, "runtime-a", activeNamespace);

  assert.notEqual(storage.getItem(activeKey), null);
  assert.equal(storage.getItem(staleKey), null);
  assert.notEqual(storage.getItem(otherKey), null);
});

test("sidebar order helpers preserve unknown categories and reorder by drop position", () => {
  const current = applyConsoleSidebarOrder(["alpha", "beta", "gamma"], ["gamma", "alpha"]);
  assert.deepEqual(current, ["gamma", "alpha", "beta"]);
  assert.deepEqual(reorderConsoleSidebarOrder(current, "beta", "gamma", "before"), ["beta", "gamma", "alpha"]);
  assert.deepEqual(reorderConsoleSidebarOrder(current, "beta", "gamma", "after"), ["gamma", "beta", "alpha"]);
});

class MemoryStorage {
  private values = new Map<string, string>();

  get length(): number {
    return this.values.size;
  }

  getItem(key: string): string | null {
    return this.values.get(key) ?? null;
  }

  setItem(key: string, value: string): void {
    this.values.set(key, value);
  }

  key(index: number): string | null {
    return Array.from(this.values.keys())[index] ?? null;
  }

  removeItem(key: string): void {
    this.values.delete(key);
  }
}
