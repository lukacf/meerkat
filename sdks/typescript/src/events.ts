/**
 * Typed event hierarchy for the Meerkat streaming API.
 *
 * Events form a discriminated union on the `type` field (snake_case to match
 * the wire protocol).  All other fields use idiomatic camelCase.
 * Missing fields are defaulted during parsing so partial streaming payloads
 * can still be represented as typed events.
 *
 * @example
 * ```ts
 * for await (const event of session.stream("Explain monads")) {
 *   switch (event.type) {
 *     case "text_delta":
 *       process.stdout.write(event.delta);
 *       break;
 *     case "tool_call_requested":
 *       console.log(`Calling ${event.name}`);
 *       break;
 *     case "turn_completed":
 *       console.log(`Tokens: ${event.usage.inputTokens} in / ${event.usage.outputTokens} out`);
 *       break;
 *   }
 * }
 * ```
 */

// ---------------------------------------------------------------------------
// Shared value types
// ---------------------------------------------------------------------------

/** Token usage for a single LLM turn. */
export interface Usage {
  readonly inputTokens: number;
  readonly outputTokens: number;
  readonly cacheCreationTokens?: number;
  readonly cacheReadTokens?: number;
}

/** Why an LLM turn ended. */
export type StopReason =
  | "end_turn"
  | "tool_use"
  | "max_tokens"
  | "stop_sequence"
  | "content_filter"
  | "cancelled";

/** Which budget dimension triggered a warning. */
export type BudgetType = "tokens" | "time" | "tool_calls";

/** Hook lifecycle points. */
export type HookPoint =
  | "run_started"
  | "run_completed"
  | "run_failed"
  | "pre_llm_request"
  | "post_llm_response"
  | "pre_tool_execution"
  | "post_tool_execution"
  | "turn_boundary";

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
  readonly usage: Usage;
}

export interface RunFailedEvent {
  readonly type: "run_failed";
  readonly sessionId: string;
  readonly error: string;
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

export interface ToolResultReceivedEvent {
  readonly type: "tool_result_received";
  readonly id: string;
  readonly name: string;
  readonly isError: boolean;
}

export interface TurnCompletedEvent {
  readonly type: "turn_completed";
  readonly stopReason: StopReason;
  readonly usage: Usage;
}

// ---------------------------------------------------------------------------
// Tool execution events
// ---------------------------------------------------------------------------

export interface ToolExecutionStartedEvent {
  readonly type: "tool_execution_started";
  readonly id: string;
  readonly name: string;
}

export interface ToolExecutionCompletedEvent {
  readonly type: "tool_execution_completed";
  readonly id: string;
  readonly name: string;
  readonly result: string;
  readonly isError: boolean;
  readonly durationMs: number;
}

export interface ToolExecutionTimedOutEvent {
  readonly type: "tool_execution_timed_out";
  readonly id: string;
  readonly name: string;
  readonly timeoutMs: number;
}

// ---------------------------------------------------------------------------
// Compaction events
// ---------------------------------------------------------------------------

export interface CompactionStartedEvent {
  readonly type: "compaction_started";
  readonly inputTokens: number;
  readonly estimatedHistoryTokens: number;
  readonly messageCount: number;
}

export interface CompactionCompletedEvent {
  readonly type: "compaction_completed";
  readonly summaryTokens: number;
  readonly messagesBefore: number;
  readonly messagesAfter: number;
}

export interface CompactionFailedEvent {
  readonly type: "compaction_failed";
  readonly error: string;
}

// ---------------------------------------------------------------------------
// Budget events
// ---------------------------------------------------------------------------

export interface BudgetWarningEvent {
  readonly type: "budget_warning";
  readonly budgetType: BudgetType;
  readonly used: number;
  readonly limit: number;
  readonly percent: number;
}

// ---------------------------------------------------------------------------
// Retry events
// ---------------------------------------------------------------------------

export interface RetryingEvent {
  readonly type: "retrying";
  readonly attempt: number;
  readonly maxAttempts: number;
  readonly error: string;
  readonly delayMs: number;
}

// ---------------------------------------------------------------------------
// Hook events
// ---------------------------------------------------------------------------

export interface HookStartedEvent {
  readonly type: "hook_started";
  readonly hookId: string;
  readonly point: HookPoint;
}

export interface HookCompletedEvent {
  readonly type: "hook_completed";
  readonly hookId: string;
  readonly point: HookPoint;
  readonly durationMs: number;
}

export interface HookFailedEvent {
  readonly type: "hook_failed";
  readonly hookId: string;
  readonly point: HookPoint;
  readonly error: string;
}

export interface HookDeniedEvent {
  readonly type: "hook_denied";
  readonly hookId: string;
  readonly point: HookPoint;
  readonly reasonCode: string;
  readonly message: string;
  readonly payload?: unknown;
}

export interface HookRewriteAppliedEvent {
  readonly type: "hook_rewrite_applied";
  readonly hookId: string;
  readonly point: HookPoint;
  readonly patch: Record<string, unknown>;
}

export interface HookPatchPublishedEvent {
  readonly type: "hook_patch_published";
  readonly hookId: string;
  readonly point: HookPoint;
  readonly envelope: Record<string, unknown>;
}

// ---------------------------------------------------------------------------
// Skill events
// ---------------------------------------------------------------------------

export interface SkillsResolvedEvent {
  readonly type: "skills_resolved";
  readonly skills: readonly string[];
  readonly injectionBytes: number;
}

export interface SkillResolutionFailedEvent {
  readonly type: "skill_resolution_failed";
  readonly reference: string;
  readonly error: string;
}

// ---------------------------------------------------------------------------
// Interaction (comms) events
// ---------------------------------------------------------------------------

export interface InteractionCompleteEvent {
  readonly type: "interaction_complete";
  readonly interactionId: string;
  readonly result: string;
}

export interface InteractionFailedEvent {
  readonly type: "interaction_failed";
  readonly interactionId: string;
  readonly error: string;
}

// ---------------------------------------------------------------------------
// Stream management events
// ---------------------------------------------------------------------------

export interface StreamTruncatedEvent {
  readonly type: "stream_truncated";
  readonly reason: string;
}

// ---------------------------------------------------------------------------
// Unknown / forward-compat
// ---------------------------------------------------------------------------

export interface UnknownEvent {
  readonly type: string;
  readonly [key: string]: unknown;
}

// ---------------------------------------------------------------------------
// Discriminated union
// ---------------------------------------------------------------------------

/** All known agent events, discriminated on `type`. */
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
  | ToolExecutionTimedOutEvent
  | CompactionStartedEvent
  | CompactionCompletedEvent
  | CompactionFailedEvent
  | BudgetWarningEvent
  | RetryingEvent
  | HookStartedEvent
  | HookCompletedEvent
  | HookFailedEvent
  | HookDeniedEvent
  | HookRewriteAppliedEvent
  | HookPatchPublishedEvent
  | SkillsResolvedEvent
  | SkillResolutionFailedEvent
  | InteractionCompleteEvent
  | InteractionFailedEvent
  | StreamTruncatedEvent
  | UnknownEvent;

// ---------------------------------------------------------------------------
// Type guards for the most commonly used events
// ---------------------------------------------------------------------------

export function isTextDelta(event: AgentEvent): event is TextDeltaEvent {
  return event.type === "text_delta";
}

export function isTextComplete(event: AgentEvent): event is TextCompleteEvent {
  return event.type === "text_complete";
}

export function isTurnCompleted(event: AgentEvent): event is TurnCompletedEvent {
  return event.type === "turn_completed";
}

export function isToolCallRequested(event: AgentEvent): event is ToolCallRequestedEvent {
  return event.type === "tool_call_requested";
}

export function isRunCompleted(event: AgentEvent): event is RunCompletedEvent {
  return event.type === "run_completed";
}

export function isRunFailed(event: AgentEvent): event is RunFailedEvent {
  return event.type === "run_failed";
}

// ---------------------------------------------------------------------------
// Wire → typed parser
// ---------------------------------------------------------------------------

function parseUsage(raw: Record<string, unknown> | undefined): Usage {
  if (!raw) return { inputTokens: 0, outputTokens: 0 };
  return {
    inputTokens: Number(raw.input_tokens ?? 0),
    outputTokens: Number(raw.output_tokens ?? 0),
    cacheCreationTokens: raw.cache_creation_tokens != null
      ? Number(raw.cache_creation_tokens)
      : undefined,
    cacheReadTokens: raw.cache_read_tokens != null
      ? Number(raw.cache_read_tokens)
      : undefined,
  };
}

/**
 * Parse a raw wire event dict into a typed {@link AgentEvent}.
 *
 * Unknown event types are returned as {@link UnknownEvent} for
 * forward-compatibility.
 */
export function parseEvent(raw: Record<string, unknown>): AgentEvent {
  const type = String(raw.type ?? "");

  switch (type) {
    // Session lifecycle
    case "run_started":
      return { type, sessionId: String(raw.session_id ?? ""), prompt: String(raw.prompt ?? "") };
    case "run_completed":
      return { type, sessionId: String(raw.session_id ?? ""), result: String(raw.result ?? ""), usage: parseUsage(raw.usage as Record<string, unknown>) };
    case "run_failed":
      return { type, sessionId: String(raw.session_id ?? ""), error: String(raw.error ?? "") };

    // Turn / LLM
    case "turn_started":
      return { type, turnNumber: Number(raw.turn_number ?? 0) };
    case "text_delta":
      return { type, delta: String(raw.delta ?? "") };
    case "text_complete":
      return { type, content: String(raw.content ?? "") };
    case "tool_call_requested":
      return { type, id: String(raw.id ?? ""), name: String(raw.name ?? ""), args: raw.args };
    case "tool_result_received":
      return { type, id: String(raw.id ?? ""), name: String(raw.name ?? ""), isError: Boolean(raw.is_error) };
    case "turn_completed":
      return { type, stopReason: String(raw.stop_reason ?? "end_turn") as StopReason, usage: parseUsage(raw.usage as Record<string, unknown>) };

    // Tool execution
    case "tool_execution_started":
      return { type, id: String(raw.id ?? ""), name: String(raw.name ?? "") };
    case "tool_execution_completed":
      return { type, id: String(raw.id ?? ""), name: String(raw.name ?? ""), result: String(raw.result ?? ""), isError: Boolean(raw.is_error), durationMs: Number(raw.duration_ms ?? 0) };
    case "tool_execution_timed_out":
      return { type, id: String(raw.id ?? ""), name: String(raw.name ?? ""), timeoutMs: Number(raw.timeout_ms ?? 0) };

    // Compaction
    case "compaction_started":
      return { type, inputTokens: Number(raw.input_tokens ?? 0), estimatedHistoryTokens: Number(raw.estimated_history_tokens ?? 0), messageCount: Number(raw.message_count ?? 0) };
    case "compaction_completed":
      return { type, summaryTokens: Number(raw.summary_tokens ?? 0), messagesBefore: Number(raw.messages_before ?? 0), messagesAfter: Number(raw.messages_after ?? 0) };
    case "compaction_failed":
      return { type, error: String(raw.error ?? "") };

    // Budget
    case "budget_warning":
      return { type, budgetType: String(raw.budget_type ?? "") as BudgetType, used: Number(raw.used ?? 0), limit: Number(raw.limit ?? 0), percent: Number(raw.percent ?? 0) };

    // Retry
    case "retrying":
      return { type, attempt: Number(raw.attempt ?? 0), maxAttempts: Number(raw.max_attempts ?? 0), error: String(raw.error ?? ""), delayMs: Number(raw.delay_ms ?? 0) };

    // Hooks
    case "hook_started":
      return { type, hookId: String(raw.hook_id ?? ""), point: String(raw.point ?? "") as HookPoint };
    case "hook_completed":
      return { type, hookId: String(raw.hook_id ?? ""), point: String(raw.point ?? "") as HookPoint, durationMs: Number(raw.duration_ms ?? 0) };
    case "hook_failed":
      return { type, hookId: String(raw.hook_id ?? ""), point: String(raw.point ?? "") as HookPoint, error: String(raw.error ?? "") };
    case "hook_denied":
      return { type, hookId: String(raw.hook_id ?? ""), point: String(raw.point ?? "") as HookPoint, reasonCode: String(raw.reason_code ?? ""), message: String(raw.message ?? ""), ...(raw.payload != null ? { payload: raw.payload } : {}) };
    case "hook_rewrite_applied":
      return { type, hookId: String(raw.hook_id ?? ""), point: String(raw.point ?? "") as HookPoint, patch: (raw.patch ?? {}) as Record<string, unknown> };
    case "hook_patch_published":
      return { type, hookId: String(raw.hook_id ?? ""), point: String(raw.point ?? "") as HookPoint, envelope: (raw.envelope ?? {}) as Record<string, unknown> };

    // Skills
    case "skills_resolved":
      return { type, skills: (raw.skills ?? []) as string[], injectionBytes: Number(raw.injection_bytes ?? 0) };
    case "skill_resolution_failed":
      return { type, reference: String(raw.reference ?? ""), error: String(raw.error ?? "") };

    // Interaction (comms)
    case "interaction_complete":
      return { type, interactionId: String(raw.interaction_id ?? ""), result: String(raw.result ?? "") };
    case "interaction_failed":
      return { type, interactionId: String(raw.interaction_id ?? ""), error: String(raw.error ?? "") };

    // Stream management
    case "stream_truncated":
      return { type, reason: String(raw.reason ?? "") };

    // Unknown — forward-compat
    default:
      return { type, ...raw } as UnknownEvent;
  }
}
