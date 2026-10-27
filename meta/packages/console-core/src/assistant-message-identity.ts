import type { ConsoleFrame } from "./runtime-types";

function carrier(frame: ConsoleFrame): Record<string, unknown> | undefined {
  const data = frame.data;
  return data !== null && typeof data === "object" && !Array.isArray(data)
    && Object.hasOwn(data, "assistant_message_id")
    ? data as Record<string, unknown>
    : undefined;
}

/** A malformed explicit carrier must not fall through to legacy text matching. */
export function hasAssistantMessageIdCarrier(frame: ConsoleFrame): boolean {
  const data = carrier(frame);
  return data !== undefined && data.assistant_message_id !== null;
}

/** The upstream ID is opaque. Validate presence without rewriting its bytes. */
export function assistantMessageId(frame: ConsoleFrame): string | undefined {
  const value = carrier(frame)?.assistant_message_id;
  return typeof value === "string" && value.trim().length > 0 ? value : undefined;
}

/** Forks retain IDs; only the actual frame session scopes correspondence. */
export function assistantMessageKey(frame: ConsoleFrame): string | undefined {
  const id = assistantMessageId(frame);
  return id !== undefined && typeof frame.sessionId === "string" && frame.sessionId.trim().length > 0
    ? JSON.stringify([frame.sessionId, id])
    : undefined;
}

/** Presentation identity survives replacement of an event by canonical history. */
export function assistantMessageRenderKey(frame: ConsoleFrame): string | undefined {
  const key = assistantMessageKey(frame);
  return key === undefined ? undefined
    : `assistant:${JSON.stringify([frame.runtimeKey ?? null, frame.identity ?? null, key])}`;
}

/** Numeric ConsoleCursor ordering without losing any of the native u64 range. */
export function assistantMessageCursorSequence(value: unknown): bigint | undefined {
  if (typeof value !== "string" || /^console:[0-9]+$/.exec(value)?.[0] !== value) return undefined;
  const sequence = BigInt(value.slice(8));
  return sequence <= 18446744073709551615n ? sequence : undefined;
}

export interface AssistantHistorySnapshot {
  sessionId: string;
  observedThrough: bigint;
  assistantMessageIds: ReadonlySet<string>;
}

type SnapshotMemo = {
  event: string;
  sourceKind: string | undefined;
  sessionId: string | undefined;
  cursor: string | undefined;
  data: unknown;
  ids: unknown;
  idCount: number;
  observedThrough: unknown;
  dataSessionId: unknown;
  complete: unknown;
  snapshot: AssistantHistorySnapshot | undefined;
};

// Every snapshot lists every assistant message of its session, and every
// transcript pass reads every snapshot frame, so parsing per call is
// quadratic in history. Parse once per frame; the memo is revalidated against
// every input the parse reads, so an in-place frame update is never stale.
const snapshotMemo = new WeakMap<ConsoleFrame, SnapshotMemo>();

/** Only a complete, correctly scoped history observation can establish absence. */
export function assistantHistorySnapshot(frame: ConsoleFrame): AssistantHistorySnapshot | undefined {
  if (frame.event !== "assistant_history_snapshot") return undefined;
  const data = frame.data && typeof frame.data === "object" && !Array.isArray(frame.data)
    ? frame.data as Record<string, unknown> : undefined;
  const ids = data?.assistant_message_ids;
  const idCount = Array.isArray(ids) ? ids.length : -1;
  const memo = snapshotMemo.get(frame);
  if (memo && memo.event === frame.event && memo.sourceKind === frame.sourceKind
    && memo.sessionId === frame.sessionId && memo.cursor === frame.cursor && memo.data === frame.data
    && memo.ids === ids && memo.idCount === idCount && memo.observedThrough === data?.observed_through
    && memo.dataSessionId === data?.session_id && memo.complete === data?.complete) {
    return memo.snapshot;
  }
  const snapshot = parseAssistantHistorySnapshot(frame);
  snapshotMemo.set(frame, {
    event: frame.event, sourceKind: frame.sourceKind, sessionId: frame.sessionId, cursor: frame.cursor,
    data: frame.data, ids, idCount, observedThrough: data?.observed_through,
    dataSessionId: data?.session_id, complete: data?.complete, snapshot,
  });
  return snapshot;
}

function parseAssistantHistorySnapshot(frame: ConsoleFrame): AssistantHistorySnapshot | undefined {
  if (frame.event !== "assistant_history_snapshot" || frame.sourceKind !== "session_history"
    || typeof frame.sessionId !== "string" || !frame.sessionId.trim()
    || !frame.data || typeof frame.data !== "object" || Array.isArray(frame.data)) return undefined;
  const data = frame.data as Record<string, unknown>;
  const cursor = assistantMessageCursorSequence(frame.cursor);
  const observedThrough = assistantMessageCursorSequence(data.observed_through);
  if (data.session_id !== frame.sessionId || data.complete !== true
    || cursor === undefined || observedThrough === undefined || observedThrough >= cursor
    || !Array.isArray(data.assistant_message_ids)) return undefined;
  const assistantMessageIds = new Set<string>();
  for (const id of data.assistant_message_ids) {
    if (typeof id !== "string" || !id.trim() || assistantMessageIds.has(id)) return undefined;
    assistantMessageIds.add(id);
  }
  return { sessionId: frame.sessionId, observedThrough, assistantMessageIds };
}
