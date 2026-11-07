// The caller's own attention items (`meta/console/attention`): replies meant
// for the signed-in person alone, which no private conversation of theirs
// took (a request they made in a shared agent). The gateway answers only the
// caller's own items, read by the caller's verified identity; the console
// keeps each read for the scope (caller and runtime) that made it, so a user
// switch never shows the previous person's items.

export interface ConsoleAttentionOrigin {
  /** The agent the request was sent to. */
  identity: string;
  /** The request's interaction. */
  interactionId: string;
  /** The request's `user_input` frame. */
  inputFrameId: string;
}

export interface ConsoleAttentionItem {
  /** Opaque and stable; names the item to dismiss it. */
  id: string;
  /** A reply to the caller's own request. Items of other kinds are skipped. */
  kind: "private_reply";
  createdAtMs: number;
  /** The reply as the gateway's payload policies left it; `null` when they
   *  left no text (the card then says its content is hidden by policy). */
  text: string | null;
  /** Whether a payload policy rewrote the reply. */
  redacted: boolean;
  origin: ConsoleAttentionOrigin;
}

export interface ConsoleAttentionSnapshot {
  /** `unavailable`: the gateway keeps no attention list (or predates it). */
  status: "loading" | "ready" | "unavailable";
  items: ConsoleAttentionItem[];
}

export const CONSOLE_ATTENTION_LIMITS = {
  /** The gateway answers at most this many items: the newest. */
  items: 100,
  /** A longer reply is cut, with an ellipsis. */
  textLength: 20_000,
  intervalMs: 30_000,
} as const;

function stringOf(value: unknown): string | null {
  return typeof value === "string" && value.trim() ? value : null;
}

/** The items of one `meta/console/attention` answer, bounded: a malformed
 *  item, or one of a kind this console does not know, is dropped. A reply a
 *  payload policy rewrote is kept, one it left no text in too (`text` null),
 *  so the person still sees and dismisses it. `null` when the answer holds
 *  no item list at all. */
export function normalizeConsoleAttention(result: unknown): ConsoleAttentionItem[] | null {
  if (!result || typeof result !== "object") return null;
  const raw = (result as { items?: unknown }).items;
  if (!Array.isArray(raw)) return null;
  const items: ConsoleAttentionItem[] = [];
  const seen = new Set<string>();
  for (const entry of raw) {
    if (items.length >= CONSOLE_ATTENTION_LIMITS.items) break;
    if (!entry || typeof entry !== "object") continue;
    const record = entry as Record<string, unknown>;
    if (record.kind !== "private_reply") continue;
    const id = stringOf(record.id);
    const text = stringOf(record.text);
    const redacted = record.redacted === true;
    const origin = record.origin && typeof record.origin === "object"
      ? record.origin as Record<string, unknown>
      : null;
    const identity = stringOf(origin?.identity);
    const interactionId = stringOf(origin?.interaction_id);
    const inputFrameId = stringOf(origin?.input_frame_id);
    const createdAtMs = typeof record.created_at_ms === "number" && Number.isFinite(record.created_at_ms)
      ? record.created_at_ms
      : null;
    // A reply without text is one a policy left none of: kept, marked redacted.
    if (!id || (!text && !redacted) || !identity || !interactionId || !inputFrameId || createdAtMs === null || seen.has(id)) continue;
    seen.add(id);
    items.push({
      id,
      kind: "private_reply",
      createdAtMs,
      text: text && text.length > CONSOLE_ATTENTION_LIMITS.textLength
        ? `${text.slice(0, CONSOLE_ATTENTION_LIMITS.textLength - 1)}…`
        : text,
      redacted,
      origin: { identity, interactionId, inputFrameId },
    });
  }
  return items;
}

/** Whether a failed read says the gateway serves no attention list: it does
 *  not list the method, or answers that the method is not found. */
export function consoleAttentionUnavailable(error: unknown): boolean {
  if (!error || typeof error !== "object") return false;
  const record = error as { kind?: unknown; rpcError?: { code?: unknown } };
  return record.kind === "console-capability-unavailable" || record.rpcError?.code === -32601;
}

export interface ConsoleAttentionPollingEnvironment {
  visible(): boolean;
  onVisible(callback: () => void): () => void;
  setInterval(callback: () => void, ms: number): unknown;
  clearInterval(handle: unknown): void;
}

export interface ConsoleAttentionPolling {
  /** Read again now (after a dismissal). */
  refresh(): void;
  stop(): void;
}

/** Read the caller's attention list now, every `intervalMs` while the page is
 *  visible and when it becomes visible again, publishing each completed read.
 *  A read that a newer one superseded is never published. A gateway that
 *  serves no attention list publishes `unavailable` once and is not read
 *  again; any other failure keeps the last published items. */
export function startConsoleAttentionPolling(
  read: (signal: AbortSignal) => Promise<unknown>,
  publish: (snapshot: ConsoleAttentionSnapshot) => void,
  environment: ConsoleAttentionPollingEnvironment,
  intervalMs: number = CONSOLE_ATTENTION_LIMITS.intervalMs,
): ConsoleAttentionPolling {
  let stopped = false;
  let current: AbortController | null = null;
  let timer: unknown = null;
  let stopVisible: (() => void) | null = null;
  const stop = () => {
    stopped = true;
    current?.abort();
    if (timer !== null) environment.clearInterval(timer);
    stopVisible?.();
  };
  const load = async (force = false) => {
    if (stopped || (!force && !environment.visible())) return;
    current?.abort();
    const mine = new AbortController();
    current = mine;
    try {
      const result = await read(mine.signal);
      if (stopped || current !== mine) return;
      const items = normalizeConsoleAttention(result);
      if (items) publish({ status: "ready", items });
    } catch (error) {
      if (stopped || current !== mine) return;
      if (consoleAttentionUnavailable(error)) {
        publish({ status: "unavailable", items: [] });
        stop();
      }
    }
  };
  // The timers first: a read that stops the polling at once clears them.
  timer = environment.setInterval(() => void load(), intervalMs);
  stopVisible = environment.onVisible(() => void load());
  void load();
  return {
    refresh: () => void load(true),
    stop,
  };
}

/** The console frame kind of a reply appended to the person's own private
 *  thread: `payload.text`, and `payload.in_reply_to` with the request's
 *  interaction and `user_input` frame. */
export const PRIVATE_REPLY_FRAME_EVENT = "private_reply";

/** The text of a private reply frame, or null when the frame is not one or
 *  carries no text. */
export function privateReplyText(frame: { event?: string; data?: unknown }): string | null {
  if (frame.event !== PRIVATE_REPLY_FRAME_EVENT) return null;
  const data = frame.data && typeof frame.data === "object" ? frame.data as Record<string, unknown> : null;
  return stringOf(data?.text);
}
