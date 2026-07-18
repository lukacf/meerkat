/** Strict parsing for the canonical agent-event envelope. */

import { parseCoreEvent } from "./events.js";
import { MeerkatError } from "./generated/errors.js";
import type { AgentEventEnvelope, EventSourceIdentity } from "./types.js";

const CANONICAL_UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;

function invalidEnvelope(
  reason: string,
  raw: unknown,
): MeerkatError {
  return new MeerkatError(
    "INVALID_RESPONSE",
    `Invalid agent event envelope: ${reason}`,
    raw,
  );
}

function requireRecord(raw: unknown, field: string, envelope: unknown): Record<string, unknown> {
  if (typeof raw !== "object" || raw === null || Array.isArray(raw)) {
    throw invalidEnvelope(`${field} must be an object`, envelope);
  }
  return raw as Record<string, unknown>;
}

function requireNonEmptyString(
  raw: Record<string, unknown>,
  field: string,
  envelope: unknown,
): string {
  const value = raw[field];
  if (typeof value !== "string" || value.length === 0) {
    throw invalidEnvelope(`${field} must be a non-empty string`, envelope);
  }
  return value;
}

function requireUnsignedInteger(
  raw: Record<string, unknown>,
  field: string,
  envelope: unknown,
): number {
  const value = raw[field];
  if (typeof value !== "number" || !Number.isSafeInteger(value) || value < 0) {
    throw invalidEnvelope(`${field} must be a non-negative safe integer`, envelope);
  }
  return value;
}

function requireUuid(
  raw: Record<string, unknown>,
  field: string,
  envelope: unknown,
): string {
  const value = requireNonEmptyString(raw, field, envelope);
  if (!CANONICAL_UUID.test(value)) {
    throw invalidEnvelope(`${field} must be a canonical UUID`, envelope);
  }
  return value;
}

export function parseEventSourceIdentity(
  raw: unknown,
  envelope: unknown,
): EventSourceIdentity {
  const record = requireRecord(raw, "source", envelope);
  const type = requireNonEmptyString(record, "type", envelope);
  switch (type) {
    case "session":
      return {
        type,
        sessionId: requireUuid(record, "session_id", envelope),
      };
    case "runtime":
      return {
        type,
        runtimeId: requireNonEmptyString(record, "runtime_id", envelope),
      };
    case "interaction":
      return {
        type,
        interactionId: requireUuid(record, "interaction_id", envelope),
      };
    case "callback":
      return { type };
    case "external":
      return {
        type,
        sourceId: requireNonEmptyString(record, "source_id", envelope),
      };
    default:
      throw invalidEnvelope(`source.type has unsupported value ${JSON.stringify(type)}`, envelope);
  }
}

export function parseAgentEventEnvelope(raw: unknown): AgentEventEnvelope {
  const record = requireRecord(raw, "envelope", raw);
  const payload = requireRecord(record.payload, "payload", raw);
  let mobId: string | undefined;
  if (record.mob_id !== undefined && record.mob_id !== null) {
    mobId = requireNonEmptyString(record, "mob_id", raw);
  }
  return {
    eventId: requireUuid(record, "event_id", raw),
    source: parseEventSourceIdentity(record.source, raw),
    seq: requireUnsignedInteger(record, "seq", raw),
    timestampMs: requireUnsignedInteger(record, "timestamp_ms", raw),
    payload: parseCoreEvent(payload),
    ...(mobId !== undefined ? { mobId } : {}),
  };
}
