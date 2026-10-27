/**
 * Typed event hierarchy for Meta SDK streaming.
 *
 * Events form a discriminated union on the `type` field (snake_case to match
 * the wire protocol). All other fields use idiomatic camelCase.
 *
 * @example
 * ```ts
 * for await (const event of handle.subscribeAgent("agent-1")) {
 *   switch (event.event.type) {
 *     case "text_delta":
 *       process.stdout.write(event.event.delta);
 *       break;
 *     case "run_completed":
 *       console.log(`Done: ${event.event.result}`);
 *       break;
 *   }
 * }
 * ```
 */

import type { SseEvent } from "./sse.js";

// ---------------------------------------------------------------------------
// Session lifecycle events
// ---------------------------------------------------------------------------

export interface RunStartedEvent {
  readonly type: "run_started";
  readonly sessionId: string;
  readonly prompt: string;
}

export interface RunCompletedEvent {
  readonly type: "run_completed";
  readonly sessionId: string;
  readonly result: string;
  /**
   * Typed value produced by schema extraction when the run was
   * launched with a response schema. Absent (`undefined`) when the
   * run had no schema, or extraction failed (in which case `result`
   * is still the raw text).
   */
  readonly structuredOutput?: unknown;
}

/**
 * meerkat's typed failure fact on `run_failed` (`error_report` on the wire):
 * `class`, an optional typed `reason` whose `reason_type` is the stable
 * discriminator (`llm_auth_error`, `llm_rate_limited`, ...), and `message`.
 * `reasonType` mirrors `reason.reason_type` when present.
 */
export interface AgentErrorReport {
  readonly class: string;
  readonly message: string;
  readonly reason?: Record<string, unknown>;
  readonly reasonType?: string;
}

export interface RunFailedEvent {
  readonly type: "run_failed";
  readonly sessionId: string;
  /**
   * Operator-readable failure text. meerkat >= 0.7 carries no flat `error`
   * on the wire; this is derived from `error_report.message` when absent.
   */
  readonly error: string;
  /** The typed report, when the wire carried one. */
  readonly errorReport?: AgentErrorReport;
}

// ---------------------------------------------------------------------------
// Turn / LLM events
// ---------------------------------------------------------------------------

export interface TurnStartedEvent {
  readonly type: "turn_started";
  readonly turnNumber: number;
}

export interface TextDeltaEvent {
  readonly type: "text_delta";
  readonly delta: string;
}

export interface TextCompleteEvent {
  readonly type: "text_complete";
  readonly content: string;
}

export interface ToolCallRequestedEvent {
  readonly type: "tool_call_requested";
  readonly id: string;
  readonly name: string;
  readonly args: unknown;
}

/**
 * A tool result was fed back to the LLM. The only event that carries the
 * tool's output: `result` is the text of its content.
 * {@link ToolExecutionCompletedEvent} for the same `id` carries the
 * execution facts. An event from an older server, which carried the output
 * on the completion instead, reads `result` as `""`.
 */
export interface ToolResultReceivedEvent {
  readonly type: "tool_result_received";
  readonly id: string;
  readonly name: string;
  readonly result: string;
  readonly isError: boolean;
}

export interface TurnCompletedEvent {
  readonly type: "turn_completed";
  readonly stopReason: string;
}

// ---------------------------------------------------------------------------
// Tool execution events
// ---------------------------------------------------------------------------

export interface ToolExecutionStartedEvent {
  readonly type: "tool_execution_started";
  readonly id: string;
  readonly name: string;
}

/**
 * A tool finished executing: whether it failed, and how long it took. It
 * carries no output. The output is on {@link ToolResultReceivedEvent} for
 * the same `id`. An event from an older server that still carries `result`
 * parses, and the field is ignored.
 */
export interface ToolExecutionCompletedEvent {
  readonly type: "tool_execution_completed";
  readonly id: string;
  readonly name: string;
  readonly isError: boolean;
  readonly durationMs: number;
}

// ---------------------------------------------------------------------------
// Catch-all
// ---------------------------------------------------------------------------

export interface UnknownEvent {
  readonly type: string;
  readonly data: Record<string, unknown>;
}

// ---------------------------------------------------------------------------
// Union type
// ---------------------------------------------------------------------------

export type AgentEvent =
  | RunStartedEvent
  | RunCompletedEvent
  | RunFailedEvent
  | TurnStartedEvent
  | TextDeltaEvent
  | TextCompleteEvent
  | ToolCallRequestedEvent
  | ToolResultReceivedEvent
  | TurnCompletedEvent
  | ToolExecutionStartedEvent
  | ToolExecutionCompletedEvent
  | UnknownEvent;

// ---------------------------------------------------------------------------
// Event parser
// ---------------------------------------------------------------------------

function isPlainRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function parseAgentErrorReport(raw: unknown): AgentErrorReport | undefined {
  if (!isPlainRecord(raw)) return undefined;
  const reason = isPlainRecord(raw.reason) ? raw.reason : undefined;
  const reasonType = typeof reason?.reason_type === "string" ? reason.reason_type : undefined;
  return {
    class: String(raw.class ?? ""),
    message: String(raw.message ?? ""),
    ...(reason ? { reason } : {}),
    ...(reasonType ? { reasonType } : {}),
  };
}

type EventFactory = (raw: Record<string, unknown>) => AgentEvent;

const EVENT_MAP: Record<string, EventFactory> = {
  run_started: (raw) => ({
    type: "run_started",
    sessionId: String(raw.session_id ?? ""),
    prompt: String(raw.prompt ?? ""),
  }),
  run_completed: (raw) => {
    // `structured_output` is `skip_serializing_if = "Option::is_none"`
    // upstream, so the field is absent (not `null`) when there's no
    // schema-extracted value. Mirror that on the TS side: undefined
    // when missing, pass-through value otherwise.
    const evt: RunCompletedEvent = {
      type: "run_completed",
      sessionId: String(raw.session_id ?? ""),
      result: String(raw.result ?? ""),
    };
    if (raw.structured_output !== undefined) {
      return { ...evt, structuredOutput: raw.structured_output };
    }
    return evt;
  },
  run_failed: (raw) => {
    const errorReport = parseAgentErrorReport(raw.error_report);
    const evt: RunFailedEvent = {
      type: "run_failed",
      sessionId: String(raw.session_id ?? ""),
      error: String(raw.error ?? errorReport?.message ?? ""),
    };
    return errorReport ? { ...evt, errorReport } : evt;
  },
  turn_started: (raw) => ({
    type: "turn_started",
    turnNumber: Number(raw.turn_number ?? 0),
  }),
  text_delta: (raw) => ({
    type: "text_delta",
    delta: String(raw.delta ?? ""),
  }),
  text_complete: (raw) => ({
    type: "text_complete",
    content: String(raw.content ?? ""),
  }),
  tool_call_requested: (raw) => ({
    type: "tool_call_requested",
    id: String(raw.id ?? ""),
    name: String(raw.name ?? ""),
    args: raw.args ?? null,
  }),
  tool_result_received: (raw) => ({
    type: "tool_result_received",
    id: String(raw.id ?? ""),
    name: String(raw.name ?? ""),
    result: String(raw.result ?? ""),
    isError: Boolean(raw.is_error),
  }),
  turn_completed: (raw) => ({
    type: "turn_completed",
    stopReason: String(raw.stop_reason ?? ""),
  }),
  tool_execution_started: (raw) => ({
    type: "tool_execution_started",
    id: String(raw.id ?? ""),
    name: String(raw.name ?? ""),
  }),
  tool_execution_completed: (raw) => ({
    type: "tool_execution_completed",
    id: String(raw.id ?? ""),
    name: String(raw.name ?? ""),
    isError: Boolean(raw.is_error),
    durationMs: Number(raw.duration_ms ?? 0),
  }),
};

/**
 * Parse a raw event dict into a typed {@link AgentEvent}.
 *
 * Unknown event types are returned as {@link UnknownEvent} for
 * forward-compatibility with newer server versions.
 */
export function parseAgentEvent(raw: Record<string, unknown>): AgentEvent {
  const eventType = String(raw.type ?? "");
  const factory = EVENT_MAP[eventType];
  if (factory) {
    return factory(raw);
  }
  return { type: eventType, data: raw };
}

// ---------------------------------------------------------------------------
// Mob-level event (wraps agent event with source attribution)
// ---------------------------------------------------------------------------

/**
 * A mob-level attributed event from the runtime.
 *
 * Wraps an {@link AgentEvent} with the `memberId` of the agent that produced it.
 */
export interface MobEventEnvelope {
  readonly memberId: string;
  readonly event: AgentEvent;
  readonly timestampMs: number;
}

export function parseMobEventFromSse(sse: SseEvent): MobEventEnvelope {
  let raw: Record<string, unknown>;
  try {
    raw = JSON.parse(sse.data) as Record<string, unknown>;
  } catch {
    raw = {};
  }
  const memberId = String(raw.member_id ?? raw.source ?? "");
  const payload = (
    typeof raw.payload === "object" && raw.payload !== null
      ? raw.payload
      : raw
  ) as Record<string, unknown>;
  return {
    memberId,
    event: parseAgentEvent(payload),
    timestampMs: Number(raw.timestamp_ms ?? 0),
  };
}

// ---------------------------------------------------------------------------
// Agent-level event (typed wrapper for per-agent SSE stream)
// ---------------------------------------------------------------------------

/**
 * A per-agent event from the runtime.
 *
 * The `event` field contains the typed {@link AgentEvent} subtype.
 */
export interface AgentEventEnvelope {
  readonly eventType: string;
  readonly event: AgentEvent;
}

export function parseAgentEventFromSse(sse: SseEvent): AgentEventEnvelope {
  let raw: Record<string, unknown>;
  try {
    raw = JSON.parse(sse.data) as Record<string, unknown>;
  } catch {
    raw = {};
  }
  return {
    eventType: sse.event,
    event: parseAgentEvent(raw),
  };
}

// ---------------------------------------------------------------------------
// Type guards
// ---------------------------------------------------------------------------

export function isTextDelta(event: AgentEvent): event is TextDeltaEvent {
  return event.type === "text_delta";
}

export function isTextComplete(event: AgentEvent): event is TextCompleteEvent {
  return event.type === "text_complete";
}

export function isRunCompleted(
  event: AgentEvent,
): event is RunCompletedEvent {
  return event.type === "run_completed";
}

export function isRunFailed(event: AgentEvent): event is RunFailedEvent {
  return event.type === "run_failed";
}

export function isTurnCompleted(
  event: AgentEvent,
): event is TurnCompletedEvent {
  return event.type === "turn_completed";
}

export function isToolCallRequested(
  event: AgentEvent,
): event is ToolCallRequestedEvent {
  return event.type === "tool_call_requested";
}

// ---------------------------------------------------------------------------
// EventStream (typed async iterator wrapping SSE → domain events)
// ---------------------------------------------------------------------------

/**
 * Typed async iterator wrapping raw SSE events into domain events.
 */
export class EventStream<T> implements AsyncIterable<T> {
  constructor(
    private readonly _source: AsyncIterable<SseEvent>,
    private readonly _parse: (sse: SseEvent) => T,
  ) {}

  async *[Symbol.asyncIterator](): AsyncGenerator<T, void, undefined> {
    for await (const sse of this._source) {
      yield this._parse(sse);
    }
  }
}
