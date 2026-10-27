import { describe, expect, it } from "vitest";
import { createConsoleSendAttempt, beginConsoleSendAttempt, finishConsoleSendAttempt } from "../../../packages/console-core/src/send-attempt";
import { consoleSendStorageKey, loadConsoleSendAttempts, saveConsoleSendAttempts, readLegacyConsoleQueue, predecessorConsoleSendStorageKey, readPredecessorConsoleQueue } from "./send-attempt-storage";
const storage = () => { const data = new Map<string, string>(); return { data, getItem: (key: string) => data.get(key) ?? null, setItem: (key: string, value: string) => { data.set(key, value); } }; };
const draft = () => createConsoleSendAttempt({ id: "a", scope: "runtime/realm/principal", destination: "d", origin: "console:p1", idempotencyKey: "same-key", text: "hello", now: 1 });
describe("scoped queue storage", () => {
  it("survives reload with frozen attempts and keeps auth scopes separate", () => {
    const s = storage(); const a = beginConsoleSendAttempt(draft(), { owner: "tab1", now: 1, handlingMode: "steer" });
    saveConsoleSendAttempts(s, a.scope, "d", [a]);
    const loaded = loadConsoleSendAttempts(s, a.scope, "d", 20_000);
    expect(loaded.kind).toBe("ready");
    expect(loaded.attempts[0].state).toBe("outcome-unknown");
    expect(loaded.attempts[0].envelopeJson).toBe(a.envelopeJson);
    expect(loadConsoleSendAttempts(s, "another principal", "d").attempts).toEqual([]);
  });
  it("preserves unrecognized versions and refuses writes over them", () => {
    const s = storage(); const a = draft(); const bytes = '{"version":2,"future":"keep me"}';
    s.setItem(consoleSendStorageKey(a.scope, "d"), bytes);
    expect(loadConsoleSendAttempts(s, a.scope, "d").kind).toBe("blocked");
    expect(() => saveConsoleSendAttempts(s, a.scope, "d", [a])).toThrow(/unrecognized/);
    expect(s.getItem(consoleSendStorageKey(a.scope, "d"))).toBe(bytes);
  });
  it("never auto-imports legacy bytes and explicit reading preserves them", () => {
    const s = storage(); const bytes = '[{"id":"old","text":"keep","addedAt":1}]';
    s.setItem("mobkit-pending-stack:d", bytes);
    expect(loadConsoleSendAttempts(s, "runtime/realm/principal", "d").attempts).toEqual([]);
    expect(readLegacyConsoleQueue(s, "d")).toHaveLength(1);
    expect(s.getItem("mobkit-pending-stack:d")).toBe(bytes);
  });
  it("reads a predecessor queue only when asked, and preserves its bytes", () => {
    const s = storage(); const a = beginConsoleSendAttempt(draft(), { owner: "tab1", now: 1, handlingMode: "steer" });
    const bytes = JSON.stringify({ version: 1, namespace: a.scope, destination: "d", attempts: [a] });
    s.setItem(predecessorConsoleSendStorageKey(a.scope, "d"), bytes);
    expect(loadConsoleSendAttempts(s, a.scope, "d").attempts).toEqual([]);
    const predecessor = readPredecessorConsoleQueue(s, a.scope, "d", 20_000);
    expect(predecessor.kind).toBe("ready");
    expect(predecessor.attempts[0].envelopeJson).toBe(a.envelopeJson);
    expect(readPredecessorConsoleQueue(s, "another principal", "d").attempts).toEqual([]);
    expect(s.getItem(predecessorConsoleSendStorageKey(a.scope, "d"))).toBe(bytes);
  });
  it("exposes quota failure without deleting previous persisted content", () => {
    const s = storage(); const a = draft(); saveConsoleSendAttempts(s, a.scope, "d", [a]);
    const bytes = s.getItem(consoleSendStorageKey(a.scope, "d"));
    s.setItem = () => { throw new Error("quota"); };
    expect(() => saveConsoleSendAttempts(s, a.scope, "d", [])).toThrow("quota");
    expect(s.getItem(consoleSendStorageKey(a.scope, "d"))).toBe(bytes);
  });
});

import { loadConsoleComposerDraft, saveConsoleComposerDraft } from "./send-attempt-storage";
it("retains another tab's new intent and prevents stale edits to attempted intent", () => {
  const s = storage(); const a = draft(); saveConsoleSendAttempts(s, a.scope, "d", [a]);
  const b = { ...a, id: "b", idempotencyKey: "second" };
  saveConsoleSendAttempts(s, a.scope, "d", [a, b], [a]);
  const attempted = beginConsoleSendAttempt(a, { owner: "tab1", now: 1, handlingMode: "queue" });
  saveConsoleSendAttempts(s, a.scope, "d", [attempted], [a]);
  expect(loadConsoleSendAttempts(s, a.scope, "d", 1).attempts.map((item) => item.id)).toEqual(["a", "b"]);
  expect(() => saveConsoleSendAttempts(s, a.scope, "d", [{ ...a, text: "edited" }], [a])).toThrow(/Another tab/);
});
it("preserves draft quote snapshots through reload and scope change", () => {
  const s = storage();
  const context = { version: 1 as const, id: "q", sourceScope: "source", sourceIdentity: "sourceAgent", messageId: "deleted-source", quote: "exact\r\n🌳  quote", label: "Source" };
  saveConsoleComposerDraft(s, "runtime/realm/principal", "d", { text: "instruction", contexts: [context] });
  expect(loadConsoleComposerDraft(s, "runtime/realm/principal", "d")).toEqual({ text: "instruction", contexts: [context] });
  expect(loadConsoleComposerDraft(s, "another/principal", "d")).toEqual({ text: "", contexts: [] });
});

import { consoleLegacyQueueImported } from "./send-attempt-storage";
it("commits one-time legacy import atomically with its scoped queue", () => {
  const s = storage(); const a = draft();
  saveConsoleSendAttempts(s, a.scope, "d", [a], [], true);
  expect(consoleLegacyQueueImported(s, a.scope, "d")).toBe(true);
  saveConsoleSendAttempts(s, a.scope, "d", [], [a]);
  expect(consoleLegacyQueueImported(s, a.scope, "d")).toBe(true);
  expect(loadConsoleSendAttempts(s, a.scope, "d").attempts).toHaveLength(0);
});

it("preserves malformed known-version bytes and rejects overwriting them", () => {
  const s = storage(); const a = beginConsoleSendAttempt(draft(), { owner: "tab", now: 1, handlingMode: "queue" });
  for (const malformed of [{ ...a, lease: { owner: "tab", expiresAt: "never" } }, { ...a, state: "accepted", lease: undefined }]) {
    const bytes = JSON.stringify({ version: 1, namespace: a.scope, destination: "d", attempts: [malformed] });
    s.setItem(consoleSendStorageKey(a.scope, "d"), bytes);
    expect(loadConsoleSendAttempts(s, a.scope, "d").kind).toBe("blocked");
    expect(() => saveConsoleSendAttempts(s, a.scope, "d", [])).toThrow();
    expect(s.getItem(consoleSendStorageKey(a.scope, "d"))).toBe(bytes);
  }
});

it("never regresses or resurrects acceptance when another tab reports a late failure", () => {
  const s = storage(); const a = beginConsoleSendAttempt(draft(), { owner: "tab", now: Date.now(), handlingMode: "queue" });
  saveConsoleSendAttempts(s, a.scope, "d", [a]);
  const accepted = finishConsoleSendAttempt(a, { state: "accepted", interactionId: "receipt" });
  saveConsoleSendAttempts(s, a.scope, "d", [accepted], [a]);
  const failed = finishConsoleSendAttempt(a, { state: "outcome-unknown", error: "late loss" });
  expect(saveConsoleSendAttempts(s, a.scope, "d", [failed], [a])[0]).toEqual(accepted);
  saveConsoleSendAttempts(s, a.scope, "d", [], [accepted]);
  expect(saveConsoleSendAttempts(s, a.scope, "d", [failed], [a])).toEqual([]);
});

it("keeps distinct pane and tab drafts when another composer is submitted", () => {
  const s = storage(); const scope = "runtime/realm/principal";
  const q = { version: 1 as const, id: "q", sourceScope: scope, sourceIdentity: "d", messageId: "reply", quote: "unsent quote", label: "Source" };
  saveConsoleComposerDraft(s, scope, "d", { text: "pane A", contexts: [q] }, "tab-a/pane-a");
  saveConsoleComposerDraft(s, scope, "d", { text: "pane B", contexts: [q] }, "tab-a/pane-b");
  saveConsoleComposerDraft(s, scope, "d", { text: "tab B", contexts: [q] }, "tab-b/pane-a");
  saveConsoleComposerDraft(s, scope, "d", { text: "", contexts: [] }, "tab-a/pane-a");
  expect(loadConsoleComposerDraft(s, scope, "d", "tab-a/pane-b")).toEqual({ text: "pane B", contexts: [q] });
  expect(loadConsoleComposerDraft(s, scope, "d", "tab-b/pane-a")).toEqual({ text: "tab B", contexts: [q] });
});
