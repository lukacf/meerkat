import { validateConsoleContexts, type ConsoleContextRecord } from "../../../packages/console-core/src/context-record";
import { recoverConsoleSendAttempt, validateConsoleSendAttempt, type ConsoleSendAttempt } from "../../../packages/console-core/src/send-attempt";

export interface ConsoleSendStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}
export type ConsoleSendLoad =
  | { kind: "ready"; attempts: ConsoleSendAttempt[] }
  | { kind: "blocked"; attempts: []; reason: string };
export const consoleSendStorageKey = (namespace: string, destination: string): string =>
  `meta-send-attempts:v1:${encodeURIComponent(namespace)}:${encodeURIComponent(destination)}`;

/** The key the predecessor (pre-0.9) console saved the same queue document under. */
export const predecessorConsoleSendStorageKey = (namespace: string, destination: string): string =>
  `mobkit-send-attempts:v1:${encodeURIComponent(namespace)}:${encodeURIComponent(destination)}`;

export function loadConsoleSendAttempts(storage: ConsoleSendStorage, namespace: string, destination: string, now = Date.now()): ConsoleSendLoad {
  return parseConsoleSendDocument(storage.getItem(consoleSendStorageKey(namespace, destination)), namespace, destination, now);
}

/**
 * The queue the predecessor (pre-0.9) console saved for this scope, read only
 * for the explicit "Older queued messages" import. Never consulted by
 * {@link loadConsoleSendAttempts}; its bytes are preserved after import.
 */
export function readPredecessorConsoleQueue(storage: ConsoleSendStorage, namespace: string, destination: string, now = Date.now()): ConsoleSendLoad {
  return parseConsoleSendDocument(storage.getItem(predecessorConsoleSendStorageKey(namespace, destination)), namespace, destination, now);
}

function parseConsoleSendDocument(raw: string | null, namespace: string, destination: string, now: number): ConsoleSendLoad {
  try {
    if (raw === null) return { kind: "ready", attempts: [] };
    const saved = JSON.parse(raw) as { version: number; namespace: string; destination: string; attempts: ConsoleSendAttempt[] };
    if (saved.version !== 1 || saved.namespace !== namespace || saved.destination !== destination || !Array.isArray(saved.attempts)) {
      throw new Error("Saved queue version or scope is unrecognized. Its bytes have been preserved.");
    }
    const ids = new Set<string>();
    const attempts = saved.attempts.map((attempt) => {
      validateConsoleSendAttempt(attempt);
      if (attempt.scope !== namespace || attempt.destination !== destination || ids.has(attempt.id)) {
        throw new Error("Saved queue scope or attempt IDs are invalid. Its bytes have been preserved.");
      }
      ids.add(attempt.id);
      return recoverConsoleSendAttempt(attempt, now);
    });
    return { kind: "ready", attempts };
  } catch (error) {
    return { kind: "blocked", attempts: [], reason: error instanceof Error ? error.message : "Saved queue cannot be read." };
  }
}

export function saveConsoleSendAttempts(storage: ConsoleSendStorage, namespace: string, destination: string, attempts: ConsoleSendAttempt[], previous?: ConsoleSendAttempt[], legacyImported = false): ConsoleSendAttempt[] {
  // Never replace unsupported data, including an older tab attempting a write.
  const current = loadConsoleSendAttempts(storage, namespace, destination);
  if (current.kind === "blocked") throw new Error(current.reason);
  let merged = attempts;
  if (previous) {
    const before = new Map(previous.map((attempt) => [attempt.id, attempt]));
    const next = new Map(attempts.map((attempt) => [attempt.id, attempt]));
    const latest = new Map(current.attempts.map((attempt) => [attempt.id, attempt]));
    for (const [id, old] of before) {
      const update = next.get(id);
      const concurrent = latest.get(id);
      if (!update) { latest.delete(id); continue; } // Explicit removal by this caller.
      // A concurrent removal is terminal for this stale view too.
      if (!concurrent) continue;
      if (JSON.stringify(update) === JSON.stringify(old)) continue;
      if (concurrent.state === "accepted") continue;
      if (concurrent && concurrent.state !== "draft" && update.state === "draft") {
        throw new Error("Another tab already attempted this message. Reload the queue before editing it.");
      }
      if (concurrent?.envelopeJson && update.envelopeJson !== concurrent.envelopeJson) {
        throw new Error("A frozen send envelope cannot be replaced by another tab.");
      }
      latest.set(id, update);
    }
    for (const [id, attempt] of next) if (!before.has(id) && !latest.has(id)) latest.set(id, attempt);
    // Apply local ordering to known rows while retaining independently added rows.
    merged = [...attempts.flatMap((attempt) => latest.has(attempt.id) ? [latest.get(attempt.id)!] : []),
      ...[...latest.values()].filter((attempt) => !next.has(attempt.id))];
  }
  for (const attempt of merged) {
    validateConsoleSendAttempt(attempt);
    if (attempt.scope !== namespace || attempt.destination !== destination) throw new Error("Cannot save a queue in another scope.");
  }
  merged = merged.map((attempt) => {
    const existing = current.attempts.find((item) => item.id === attempt.id);
    if (existing?.envelopeJson && existing.envelopeJson !== attempt.envelopeJson) throw new Error("A frozen send envelope cannot be replaced.");
    return existing?.state === "accepted" ? existing : attempt;
  });
  const previousDocument = storage.getItem(consoleSendStorageKey(namespace, destination));
  const alreadyImported = previousDocument ? JSON.parse(previousDocument).legacyImported === true : false;
  storage.setItem(consoleSendStorageKey(namespace, destination), JSON.stringify({ version: 1, namespace, destination,
    attempts: merged, ...(legacyImported || alreadyImported ? { legacyImported: true } : {}) }));
  return merged;
}

export function consoleLegacyQueueImported(storage: ConsoleSendStorage, namespace: string, destination: string): boolean {
  const raw = storage.getItem(consoleSendStorageKey(namespace, destination));
  return raw !== null && JSON.parse(raw).legacyImported === true;
}

export function readLegacyConsoleQueue(storage: ConsoleSendStorage, destination: string): Array<{ id: string; text: string; addedAt: number }> {
  const raw = storage.getItem(`mobkit-pending-stack:${destination}`);
  if (!raw) return [];
  const data: unknown = JSON.parse(raw);
  if (!Array.isArray(data) || data.some((item) => !item || typeof item.id !== "string" || typeof item.text !== "string" || !Number.isFinite(item.addedAt))) {
    throw new Error("Legacy queue cannot be imported; its bytes have been preserved.");
  }
  return data;
}

export interface ConsoleComposerDraft { text: string; contexts: ConsoleContextRecord[] }
export const consoleComposerDraftKey = (namespace: string, destination: string, composerId?: string): string =>
  composerId ? `meta-composer-draft:v2:${encodeURIComponent(namespace)}:${encodeURIComponent(destination)}:${encodeURIComponent(composerId)}`
    : `meta-composer-draft:v1:${encodeURIComponent(namespace)}:${encodeURIComponent(destination)}`;
/** sessionStorage survives reload and is separate for independently opened tabs. */
export function consoleComposerTabId(storage: ConsoleSendStorage | null, createId: () => string): string {
  const key = "meta-composer-tab:v1";
  try {
    const existing = storage?.getItem(key);
    if (existing?.trim()) return existing;
    const id = createId(); storage?.setItem(key, id); return id;
  } catch { return createId(); }
}
export function loadConsoleComposerDraft(storage: ConsoleSendStorage, namespace: string, destination: string, composerId?: string): ConsoleComposerDraft {
  const raw = storage.getItem(consoleComposerDraftKey(namespace, destination, composerId));
  if (raw === null) return { text: "", contexts: [] };
  const saved = JSON.parse(raw) as ConsoleComposerDraft & { version: number; namespace: string; destination: string; composerId?: string };
  if (saved.version !== (composerId ? 2 : 1) || saved.composerId !== composerId || saved.namespace !== namespace || saved.destination !== destination || typeof saved.text !== "string" || !Array.isArray(saved.contexts)) {
    throw new Error("Saved draft cannot be read. Its bytes have been preserved.");
  }
  validateConsoleContexts(saved.contexts);
  return { text: saved.text, contexts: saved.contexts };
}
export function saveConsoleComposerDraft(storage: ConsoleSendStorage, namespace: string, destination: string, draft: ConsoleComposerDraft, composerId?: string): void {
  loadConsoleComposerDraft(storage, namespace, destination, composerId); // Preserve unknown future data.
  validateConsoleContexts(draft.contexts);
  storage.setItem(consoleComposerDraftKey(namespace, destination, composerId), JSON.stringify({ version: composerId ? 2 : 1, namespace, destination, ...(composerId ? { composerId } : {}), ...draft }));
}
