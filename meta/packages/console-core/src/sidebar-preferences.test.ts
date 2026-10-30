import assert from "node:assert/strict";

import {
  DEFAULT_CONSOLE_SIDEBAR_VIEW,
  SECTION_COLLAPSE_STORAGE_PREFIX,
  SIDEBAR_AGENT_ORDER_MIN_CAPACITY,
  SIDEBAR_AGENT_ORDER_STORAGE_PREFIX,
  SIDEBAR_AGENT_PINS_STORAGE_PREFIX,
  SIDEBAR_PINS_STORAGE_PREFIX,
  SIDEBAR_STORAGE_PREFIXES,
  SIDEBAR_VIEW_STORAGE_PREFIX,
  SUBGROUP_COLLAPSE_STORAGE_PREFIX,
  applyConsoleSidebarOrder,
  capConsoleSidebarAgentOrder,
  listConsoleSidebarAgentOrder,
  mergeConsoleSidebarSiblingOrder,
  pruneStaleSidebarStorage,
  readConsoleSidebarView,
  readSidebarStringList,
  readSidebarStringSet,
  reorderConsoleSidebarOrder,
  sidebarStorageKey,
  writeConsoleSidebarView,
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

test("manual agent order lists present agents after the stored ones and keeps absent keys", () => {
  assert.deepEqual(
    listConsoleSidebarAgentOrder(["agent:c", "agent:gone", "agent:a"], ["agent:a", "agent:b", "agent:c", "agent:d"]),
    ["agent:c", "agent:gone", "agent:a", "agent:b", "agent:d"],
  );
  assert.deepEqual(listConsoleSidebarAgentOrder([], ["agent:a", "agent:a", ""]), ["agent:a"]);
});

test("a reordered sibling block returns where its first key stood and leaves other keys in place", () => {
  const stored = ["x", "b", "y", "a", "z", "c"];
  assert.deepEqual(mergeConsoleSidebarSiblingOrder(stored, ["c", "a", "b"]), ["x", "c", "a", "b", "y", "z"]);
  assert.deepEqual(mergeConsoleSidebarSiblingOrder(["x", "y"], ["b", "a"]), ["x", "y", "b", "a"]);
  assert.deepEqual(mergeConsoleSidebarSiblingOrder(stored, []), stored);
});

test("manual agent order cap drops the oldest absent keys first and never a present agent", () => {
  const present = new Set(["keep:1", "keep:2"]);
  const absent = Array.from({ length: SIDEBAR_AGENT_ORDER_MIN_CAPACITY + 3 }, (_, index) => `gone:${index}`);
  const capped = capConsoleSidebarAgentOrder(["keep:1", ...absent, "keep:2"], present);
  assert.equal(capped.length, SIDEBAR_AGENT_ORDER_MIN_CAPACITY);
  assert.equal(capped[0], "keep:1");
  assert.equal(capped[capped.length - 1], "keep:2");
  assert.equal(capped.includes("gone:0"), false);
  assert.equal(capped.includes("gone:4"), false);
  assert.equal(capped.includes("gone:5"), true);

  const roster = new Set(Array.from({ length: SIDEBAR_AGENT_ORDER_MIN_CAPACITY }, (_, index) => `agent:${index}`));
  const large = [...roster, "gone:a"];
  assert.deepEqual(capConsoleSidebarAgentOrder(large, roster), large);
});

test("roster view preferences round-trip and fall back per choice", () => {
  const storage = new MemoryStorage();
  const key = sidebarStorageKey(SIDEBAR_VIEW_STORAGE_PREFIX, "principal-a");

  assert.equal(readConsoleSidebarView(storage, key), null);
  writeConsoleSidebarView(storage, key, { groupBy: "none", sortBy: "activity" });
  assert.deepEqual(readConsoleSidebarView(storage, key), { groupBy: "none", sortBy: "activity" });
  assert.deepEqual(JSON.parse(storage.getItem(key) || "{}"), { v: 1, group_by: "none", sort_by: "activity" });

  storage.setItem(key, JSON.stringify({ group_by: "teams", sort_by: "name" }));
  assert.deepEqual(readConsoleSidebarView(storage, key), { groupBy: "category", sortBy: "name" });
  storage.setItem(key, "not json");
  assert.deepEqual(readConsoleSidebarView(storage, key), DEFAULT_CONSOLE_SIDEBAR_VIEW);
});

test("per-user sidebar keys are outside the configuration prefixes that pruning removes", () => {
  const storage = new MemoryStorage();
  const scope = encodeURIComponent("runtime-a");
  const orderKey = `${SIDEBAR_AGENT_ORDER_STORAGE_PREFIX}:${scope}:hash-old`;
  const viewKey = `${SIDEBAR_VIEW_STORAGE_PREFIX}:${scope}:hash-old`;
  const pinsKey = `${SIDEBAR_AGENT_PINS_STORAGE_PREFIX}:${scope}:hash-old`;
  for (const key of [orderKey, viewKey, pinsKey]) storage.setItem(key, "[]");

  pruneStaleSidebarStorage(storage, "runtime-a", `${scope}:hash-new`);

  assert.notEqual(storage.getItem(orderKey), null);
  assert.notEqual(storage.getItem(viewKey), null);
  assert.notEqual(storage.getItem(pinsKey), null);
  for (const prefix of [SIDEBAR_AGENT_ORDER_STORAGE_PREFIX, SIDEBAR_VIEW_STORAGE_PREFIX, SIDEBAR_AGENT_PINS_STORAGE_PREFIX]) {
    assert.equal((SIDEBAR_STORAGE_PREFIXES as readonly string[]).includes(prefix), false);
  }
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
