import { describe, expect, it } from "vitest";
import {
  consoleComposerDraftKey,
  consoleSendStorageKey,
  legacyConsoleQueueKey,
  predecessorConsoleSendStorageKey,
  purgeConsoleSendScope,
  removeOlderConsoleQueues,
} from "./send-attempt-storage";
import {
  LAST_PREFERENCE_SCOPE_KEY,
  adoptSharedSectionPreferences,
  readPreference,
  rememberPreferenceScope,
  writePreference,
} from "./preference-scope";

/** A Storage stand-in that enumerates its keys like the browser's. */
function memoryStorage(entries: Record<string, string> = {}) {
  const data = new Map(Object.entries(entries));
  return {
    data,
    get length() { return data.size; },
    key: (index: number) => [...data.keys()][index] ?? null,
    getItem: (key: string) => data.get(key) ?? null,
    setItem: (key: string, value: string) => { data.set(key, value); },
    removeItem: (key: string) => { data.delete(key); },
  };
}

describe("older queues", () => {
  it("an import removes the unscoped, predecessor and host-only copies, and nothing else", () => {
    const own = consoleSendStorageKey("scope-a", "agent");
    const other = consoleSendStorageKey("scope-b", "agent");
    const storage = memoryStorage({
      [legacyConsoleQueueKey("agent")]: "[]",
      [predecessorConsoleSendStorageKey("scope-a", "agent")]: "{}",
      [consoleSendStorageKey("host", "agent")]: "{}",
      [own]: "{}",
      [other]: "{}",
      [legacyConsoleQueueKey("another-agent")]: "[]",
    });
    removeOlderConsoleQueues(storage, "scope-a", "agent", "host");
    expect([...storage.data.keys()].sort()).toEqual([legacyConsoleQueueKey("another-agent"), other, own].sort());
  });
});

describe("signing out", () => {
  it("removes one user's queues and this tab's drafts, and keeps everything else", () => {
    const namespace = JSON.stringify(["", "console-storage:v1:a"]);
    const otherNamespace = JSON.stringify(["", "console-storage:v1:b"]);
    const local = memoryStorage({
      [consoleSendStorageKey(namespace, "agent-1")]: "{}",
      [consoleSendStorageKey(namespace, "agent-2")]: "{}",
      [predecessorConsoleSendStorageKey(namespace, "agent-1")]: "{}",
      [consoleSendStorageKey(otherNamespace, "agent-1")]: "{}",
      [`meta-console-sidebar-agent-pins:${encodeURIComponent(namespace)}`]: "[]",
      "meta-console-theme:x": "dark",
    });
    const session = memoryStorage({
      [consoleComposerDraftKey(namespace, "agent-1", "tab/pane")]: "{}",
      [consoleComposerDraftKey(namespace, "agent-1")]: "{}",
      [consoleComposerDraftKey(otherNamespace, "agent-1", "tab/pane")]: "{}",
      "meta-composer-tab:v1": "tab",
    });
    purgeConsoleSendScope(local, session, namespace);
    expect([...local.data.keys()].sort()).toEqual([
      consoleSendStorageKey(otherNamespace, "agent-1"),
      `meta-console-sidebar-agent-pins:${encodeURIComponent(namespace)}`,
      "meta-console-theme:x",
    ].sort());
    expect([...session.data.keys()].sort()).toEqual([
      consoleComposerDraftKey(otherNamespace, "agent-1", "tab/pane"),
      "meta-composer-tab:v1",
    ].sort());
  });
});

describe("per-user preferences", () => {
  it("adopts a shared value once, keeping a value the scope already has", () => {
    const storage = memoryStorage({ "meta-console-theme": "dark", "meta-console-density": "compact", "meta-console-density:a": "comfortable" });
    expect(readPreference(storage, "meta-console-theme", "a")).toBe("dark");
    expect(storage.getItem("meta-console-theme")).toBeNull();
    expect(readPreference(storage, "meta-console-theme", "b")).toBeNull();
    expect(readPreference(storage, "meta-console-density", "a")).toBe("comfortable");
    expect(storage.getItem("meta-console-density")).toBeNull();
  });

  it("reads the last scope's value while the scope is unknown, else the shared one", () => {
    const storage = memoryStorage({ "meta-console-theme": "dark" });
    expect(readPreference(storage, "meta-console-theme", null)).toBe("dark");
    // Unknown scope and no last scope: a choice holds for the page only.
    writePreference(storage, "meta-console-theme", null, "light");
    expect(storage.getItem("meta-console-theme")).toBe("dark");
    rememberPreferenceScope(storage, "a");
    expect(storage.getItem(LAST_PREFERENCE_SCOPE_KEY)).toBe("a");
    storage.setItem("meta-console-theme:a", "light");
    expect(readPreference(storage, "meta-console-theme", null)).toBe("light");
    writePreference(storage, "meta-console-theme", null, "dark");
    expect(storage.getItem("meta-console-theme:a")).toBe("dark");
    writePreference(storage, "meta-console-theme", "b", "light");
    expect(storage.getItem("meta-console-theme:b")).toBe("light");
  });

  it("moves the roster's section preferences to the user's namespace once", () => {
    const storage = memoryStorage({
      "meta-console-sidebar-sections:shared": '["Done"]',
      "meta-console-sidebar-section-order:shared": '["Working","Done"]',
      "meta-console-sidebar-subgroups:shared": '["x"]',
      "meta-console-sidebar-subgroups:own": '["kept"]',
      "meta-console-sidebar-agent-order:shared": '["not a section preference"]',
    });
    adoptSharedSectionPreferences(storage, "shared", "own");
    expect(Object.fromEntries(storage.data)).toEqual({
      "meta-console-sidebar-sections:own": '["Done"]',
      "meta-console-sidebar-section-order:own": '["Working","Done"]',
      "meta-console-sidebar-subgroups:own": '["kept"]',
      "meta-console-sidebar-agent-order:shared": '["not a section preference"]',
    });
  });
});
