import type { ConversationRealtimeOrigin } from "./conversation";
import type { ConsoleFrame } from "./runtime-types";

type RecordValue = Record<string, unknown>;
function record(value: unknown): RecordValue | undefined {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? value as RecordValue : undefined;
}
function identifier(value: unknown): value is string {
  return typeof value === "string" && value.trim().length > 0;
}
function canonicalMessage(frame: ConsoleFrame): RecordValue | undefined {
  if (frame.sourceKind !== "session_history") return undefined;
  const message = record(record(frame.data)?.message);
  return message && ["assistant", "block_assistant", "user"].includes(String(message.role)) ? message : undefined;
}
function carriers(frame: ConsoleFrame): unknown[] {
  const message = canonicalMessage(frame);
  if (!message) return [];
  const identity = record(message.identity);
  return [message, identity].flatMap(value => value && Object.hasOwn(value, "realtime_origin")
    ? [value.realtime_origin] : []);
}

/** Explicit but invalid provenance must not fall back to text or run matching. */
export function hasRealtimeMessageOriginCarrier(frame: ConsoleFrame): boolean {
  return carriers(frame).some(value => value !== null);
}

/** Typed legacy speech is canonical history even when no join key was saved. */
export function isRealtimeHistoryMessage(frame: ConsoleFrame): boolean {
  if (hasRealtimeMessageOriginCarrier(frame)) return true;
  const message = canonicalMessage(frame);
  return message?.role === "block_assistant" && Array.isArray(message.blocks)
    && message.blocks.some(value => {
      const block = record(value);
      return (block?.block_type ?? block?.type) === "transcript";
    });
}

function parse(value: unknown, sessionId: string): ConversationRealtimeOrigin | undefined {
  const origin = record(value);
  if (!origin || origin.session_id !== sessionId || !identifier(origin.channel_id)
    || !Number.isSafeInteger(origin.canonical_row_sequence)
    || (origin.canonical_row_sequence as number) < 0) return undefined;
  const items = origin.provider_item_ids === undefined ? [] : origin.provider_item_ids;
  if (!Array.isArray(items) || !items.every(identifier)) return undefined;
  return {
    sessionId, channelId: origin.channel_id, canonicalRowSequence: origin.canonical_row_sequence as number,
    providerItemIds: [...items],
  };
}

/** Only canonical history and its actual session can authorize a realtime join. */
export function realtimeMessageOrigin(frame: ConsoleFrame): ConversationRealtimeOrigin | undefined {
  if (!identifier(frame.sessionId)) return undefined;
  const values = carriers(frame);
  if (values.length === 0) return undefined;
  const parsed = values.map(value => parse(value, frame.sessionId!));
  const origin = parsed[0];
  if (!origin || parsed.some(value => !value || JSON.stringify(value) !== JSON.stringify(origin))) return undefined;
  return origin;
}
