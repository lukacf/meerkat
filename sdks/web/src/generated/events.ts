// Generated raw event types for @rkat/web
// Source: artifacts/schemas/events.json

export type BudgetType = "tokens" | "time" | "tool_calls";

export type HookId = string;

export type HookPatch = {
  max_tokens?: number | null;
  patch_type: "llm_request";
  provider_params?: unknown;
  temperature?: number | null;
} | {
  patch_type: "assistant_text";
  text: string;
} | {
  args: unknown;
  patch_type: "tool_args";
} | {
  content: string;
  is_error?: boolean | null;
  patch_type: "tool_result";
} | {
  patch_type: "run_result";
  text: string;
};

export interface HookPatchEnvelope {
  hook_id: HookId;
  patch: HookPatch;
  point: HookPoint;
  published_at: string;
  revision: HookRevision;
}

export type HookPoint = "run_started" | "run_completed" | "run_failed" | "pre_llm_request" | "post_llm_response" | "pre_tool_execution" | "post_tool_execution" | "turn_boundary";

export type HookReasonCode = "policy_violation" | "safety_violation" | "schema_violation" | "timeout" | "runtime_error";

export type HookRevision = number;

export type InteractionId = string;

export type SessionId = string;

export type SkillId = string;

export type StopReason = "end_turn" | "tool_use" | "max_tokens" | "stop_sequence" | "content_filter" | "cancelled";

export type ToolConfigChangeOperation = "add" | "remove" | "reload";

export type ToolConfigChangedPayload = {
  applied_at_turn?: number | null;
  operation: ToolConfigChangeOperation;
  persisted: boolean;
  status: string;
  target: string;
};

export type Usage = {
  cache_creation_tokens?: number | null;
  cache_read_tokens?: number | null;
  input_tokens: number;
  output_tokens: number;
};

export interface RunStartedEvent {
  prompt: string;
  session_id: SessionId;
  type: "run_started";
}

export interface RunCompletedEvent {
  result: string;
  session_id: SessionId;
  type: "run_completed";
  usage: Usage;
}

export interface RunFailedEvent {
  error: string;
  session_id: SessionId;
  type: "run_failed";
}

export interface HookStartedEvent {
  hook_id: string;
  point: HookPoint;
  type: "hook_started";
}

export interface HookCompletedEvent {
  duration_ms: number;
  hook_id: string;
  point: HookPoint;
  type: "hook_completed";
}

export interface HookFailedEvent {
  error: string;
  hook_id: string;
  point: HookPoint;
  type: "hook_failed";
}

export interface HookDeniedEvent {
  hook_id: string;
  message: string;
  payload?: unknown;
  point: HookPoint;
  reason_code: HookReasonCode;
  type: "hook_denied";
}

export interface HookRewriteAppliedEvent {
  hook_id: string;
  patch: HookPatch;
  point: HookPoint;
  type: "hook_rewrite_applied";
}

export interface HookPatchPublishedEvent {
  envelope: HookPatchEnvelope;
  hook_id: string;
  point: HookPoint;
  type: "hook_patch_published";
}

export interface TurnStartedEvent {
  turn_number: number;
  type: "turn_started";
}

export interface ReasoningDeltaEvent {
  delta: string;
  type: "reasoning_delta";
}

export interface ReasoningCompleteEvent {
  content: string;
  type: "reasoning_complete";
}

export interface TextDeltaEvent {
  delta: string;
  type: "text_delta";
}

export interface TextCompleteEvent {
  content: string;
  type: "text_complete";
}

export interface ToolCallRequestedEvent {
  args: unknown;
  id: string;
  name: string;
  type: "tool_call_requested";
}

export interface ToolResultReceivedEvent {
  id: string;
  is_error: boolean;
  name: string;
  type: "tool_result_received";
}

export interface TurnCompletedEvent {
  stop_reason: StopReason;
  type: "turn_completed";
  usage: Usage;
}

export interface ToolExecutionStartedEvent {
  id: string;
  name: string;
  type: "tool_execution_started";
}

export interface ToolExecutionCompletedEvent {
  duration_ms: number;
  has_images?: boolean;
  id: string;
  is_error: boolean;
  name: string;
  result: string;
  type: "tool_execution_completed";
}

export interface ToolExecutionTimedOutEvent {
  id: string;
  name: string;
  timeout_ms: number;
  type: "tool_execution_timed_out";
}

export interface CompactionStartedEvent {
  estimated_history_tokens: number;
  input_tokens: number;
  message_count: number;
  type: "compaction_started";
}

export interface CompactionCompletedEvent {
  messages_after: number;
  messages_before: number;
  summary_tokens: number;
  type: "compaction_completed";
}

export interface CompactionFailedEvent {
  error: string;
  type: "compaction_failed";
}

export interface BudgetWarningEvent {
  budget_type: BudgetType;
  limit: number;
  percent: number;
  type: "budget_warning";
  used: number;
}

export interface RetryingEvent {
  attempt: number;
  delay_ms: number;
  error: string;
  max_attempts: number;
  type: "retrying";
}

export interface SkillsResolvedEvent {
  injection_bytes: number;
  skills: SkillId[];
  type: "skills_resolved";
}

export interface SkillResolutionFailedEvent {
  error: string;
  reference: string;
  type: "skill_resolution_failed";
}

export interface InteractionCompleteEvent {
  interaction_id: InteractionId;
  result: string;
  type: "interaction_complete";
}

export interface InteractionFailedEvent {
  error: string;
  interaction_id: InteractionId;
  type: "interaction_failed";
}

export interface StreamTruncatedEvent {
  reason: string;
  type: "stream_truncated";
}

export interface ToolConfigChangedEvent {
  payload: ToolConfigChangedPayload;
  type: "tool_config_changed";
}

export const KNOWN_AGENT_EVENT_TYPES = [
  "run_started",
  "run_completed",
  "run_failed",
  "hook_started",
  "hook_completed",
  "hook_failed",
  "hook_denied",
  "hook_rewrite_applied",
  "hook_patch_published",
  "turn_started",
  "reasoning_delta",
  "reasoning_complete",
  "text_delta",
  "text_complete",
  "tool_call_requested",
  "tool_result_received",
  "turn_completed",
  "tool_execution_started",
  "tool_execution_completed",
  "tool_execution_timed_out",
  "compaction_started",
  "compaction_completed",
  "compaction_failed",
  "budget_warning",
  "retrying",
  "skills_resolved",
  "skill_resolution_failed",
  "interaction_complete",
  "interaction_failed",
  "stream_truncated",
  "tool_config_changed"
] as const;

export type KnownAgentEventType = typeof KNOWN_AGENT_EVENT_TYPES[number];

export type AgentEvent =
  RunStartedEvent |
  RunCompletedEvent |
  RunFailedEvent |
  HookStartedEvent |
  HookCompletedEvent |
  HookFailedEvent |
  HookDeniedEvent |
  HookRewriteAppliedEvent |
  HookPatchPublishedEvent |
  TurnStartedEvent |
  ReasoningDeltaEvent |
  ReasoningCompleteEvent |
  TextDeltaEvent |
  TextCompleteEvent |
  ToolCallRequestedEvent |
  ToolResultReceivedEvent |
  TurnCompletedEvent |
  ToolExecutionStartedEvent |
  ToolExecutionCompletedEvent |
  ToolExecutionTimedOutEvent |
  CompactionStartedEvent |
  CompactionCompletedEvent |
  CompactionFailedEvent |
  BudgetWarningEvent |
  RetryingEvent |
  SkillsResolvedEvent |
  SkillResolutionFailedEvent |
  InteractionCompleteEvent |
  InteractionFailedEvent |
  StreamTruncatedEvent |
  ToolConfigChangedEvent;
