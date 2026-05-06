// Generated raw event types for @rkat/web
// Source: artifacts/schemas/events.json

export type AgentErrorClass = "llm" | "store" | "tool" | "mcp" | "session_not_found" | "budget" | "max_tokens" | "content_filtered" | "max_turns" | "cancelled" | "invalid_state" | "operation_not_found" | "depth_limit" | "concurrency_limit" | "config" | "internal" | "build" | "auth" | "callback_pending" | "structured_output" | "invalid_output_schema" | "hook" | "terminal" | "no_pending_boundary";

export type AgentErrorReason = {
  reason_type: "llm_rate_limited";
  retry_after_ms?: number | null;
} | {
  max: number;
  reason_type: "llm_context_exceeded";
  requested: number;
} | {
  reason_type: "llm_auth_error";
} | {
  model: string;
  reason_type: "llm_invalid_model";
} | {
  provider_error: unknown;
  provider_error_kind: LlmProviderErrorKind;
  provider_error_retryability: LlmProviderErrorRetryability;
  reason_type: "llm_provider_error";
} | {
  duration_ms: number;
  reason_type: "llm_network_timeout";
} | {
  duration_ms: number;
  reason_type: "llm_call_timeout";
} | {
  hook_id?: HookId | null;
  point: HookPoint;
  reason_code: HookReasonCode;
  reason_type: "hook_denied";
} | {
  hook_id: HookId;
  reason_type: "hook_timeout";
  timeout_ms: number;
} | {
  hook_id: HookId;
  reason: string;
  reason_type: "hook_execution_failed";
} | {
  reason: string;
  reason_type: "hook_config_invalid";
} | {
  attempts: number;
  reason: string;
  reason_type: "structured_output_validation_failed";
} | {
  reason: string;
  reason_type: "invalid_output_schema";
} | {
  binding_key: string;
  message: string;
  reason_type: "auth_reauth_required";
} | {
  args: unknown;
  reason_type: "callback_pending";
  tool_name: string;
} | {
  cause_kind: TurnTerminalCauseKind;
  outcome: TurnTerminalOutcome;
  reason_type: "turn_terminal_cause";
};

export type AgentErrorReport = {
  class: AgentErrorClass;
  message: string;
  reason?: AgentErrorReason | null;
};

export type BackgroundJobTerminalStatus = "completed" | "failed" | "aborted" | "cancelled" | "retired" | "terminated";

export type BlobId = string;

export type BudgetType = "tokens" | "time" | "tool_calls";

export type CapabilityId = string;

export type ContentBlock = {
  text: string;
  type: "text";
} | {
  data: string;
  source: "inline";
} | {
  blob_id: BlobId;
  source: "blob";
} | {
  data: string;
  source: "inline";
};

export type ContentInput = string | ContentBlock[];

export interface DeferredCatalogDelta {
  added_hidden_names?: string[];
  pending_sources?: string[];
  removed_hidden_names?: string[];
}

export type ExternalToolDeltaPhase = "pending" | "applied" | "draining" | "forced" | "failed";

export type HookId = string;

export type HookPoint = "run_started" | "run_completed" | "run_failed" | "pre_llm_request" | "post_llm_response" | "pre_tool_execution" | "post_tool_execution" | "turn_boundary";

export type HookReasonCode = "policy_violation" | "safety_violation" | "schema_violation" | "timeout" | "runtime_error";

export type InteractionId = string;

export type LlmProviderErrorKind = "invalid_request" | "content_filtered" | "server_error" | "server_overloaded" | "connection_reset" | "unknown" | "stream_parse_error" | "incomplete_response";

export type LlmProviderErrorRetryability = "retryable" | "non_retryable";

export type LlmRetryFailure = {
  duration_ms?: number | null;
  kind: LlmRetryFailureKind;
  message: string;
  provider: string;
  retry_after_ms?: number | null;
};

export type LlmRetryFailureKind = "rate_limited" | "network_timeout" | "call_timeout" | "retryable_provider_error";

export type LlmRetryPlan = {
  attempt: number;
  budget_capped: boolean;
  computed_delay_ms: number;
  max_retries: number;
  rate_limit_floor_applied: boolean;
  retry_after_hint_ms?: number | null;
  selected_delay_ms: number;
};

export interface LlmRetrySchedule {
  failure: LlmRetryFailure;
  plan: LlmRetryPlan;
}

export type Provider = "anthropic" | "open_a_i" | "gemini" | "self_hosted" | "other";

export interface SchemaWarning {
  message: string;
  path: string;
  provider: Provider;
}

export type SessionId = string;

export interface SkillKey {
  skill_name: SkillName;
  source_uuid: SourceUuid;
}

export type SkillName = string;

export type SkillResolutionFailureReason = {
  key: SkillKey;
  reason_type: "not_found";
} | {
  capability: CapabilityId;
  key: SkillKey;
  reason_type: "capability_unavailable";
} | {
  message: string;
  reason_type: "load";
} | {
  message: string;
  reason_type: "parse";
} | {
  existing_fingerprint: string;
  new_fingerprint: string;
  reason_type: "source_uuid_collision";
  source_uuid: string;
} | {
  existing_source_uuid: string;
  fingerprint: string;
  mutated_source_uuid: string;
  reason_type: "source_uuid_mutation_without_lineage";
} | {
  event_id: string;
  event_kind: string;
  reason_type: "missing_skill_remaps";
} | {
  from_skill_name: string;
  from_source_uuid: string;
  reason_type: "remap_without_lineage";
  to_skill_name: string;
  to_source_uuid: string;
} | {
  alias: string;
  reason_type: "unknown_skill_alias";
} | {
  reason_type: "remap_cycle";
  skill_name: string;
  source_uuid: string;
} | {
  message: string;
  reason_type: "unknown";
};

export type SourceUuid = string;

export type StopReason = "end_turn" | "tool_use" | "max_tokens" | "stop_sequence" | "content_filter" | "cancelled";

export type ToolCallArguments = Record<string, unknown>;

export type ToolConfigChangeDomain = "tool_scope" | "deferred_catalog";

export type ToolConfigChangeOperation = "add" | "remove" | "reload";

export type ToolConfigChangeStatus = {
  base_changed: boolean;
  kind: "boundary_applied";
  revision: number;
  visible_changed: boolean;
} | {
  added_hidden_count: number;
  kind: "deferred_catalog_delta";
  pending_source_count: number;
  removed_hidden_count: number;
} | {
  error: string;
  kind: "warning_failed_closed";
} | {
  detail?: string | null;
  kind: "external_tool_delta";
  phase: ExternalToolDeltaPhase;
} | {
  kind: "legacy_status";
  status: string;
};

export type ToolConfigChangedPayload = {
  applied_at_turn?: number | null;
  deferred_catalog_delta?: DeferredCatalogDelta | null;
  domain?: ToolConfigChangeDomain | null;
  operation: ToolConfigChangeOperation;
  persisted: boolean;
  status: string;
  status_info?: ToolConfigChangeStatus | null;
  target: string;
};

export type TurnTerminalCauseKind = "unknown" | "hook_denied" | "hook_failure" | "llm_failure" | "tool_failure" | "structured_output_validation_failed" | "budget_exhausted" | "time_budget_exceeded" | "retry_exhausted" | "turn_limit_reached" | "runtime_apply_failure" | "fatal_failure";

export type TurnTerminalOutcome = "none" | "completed" | "failed" | "cancelled" | "budget_exhausted" | "time_budget_exceeded" | "structured_output_validation_failed";

export type Usage = {
  cache_creation_tokens?: number | null;
  cache_read_tokens?: number | null;
  input_tokens: number;
  output_tokens: number;
};

export interface RunStartedEvent {
  prompt: ContentInput;
  session_id: SessionId;
  type: "run_started";
}

export interface RunCompletedEvent {
  extraction_required?: boolean;
  result: string;
  session_id: SessionId;
  structured_output?: unknown;
  terminal_cause_kind?: TurnTerminalCauseKind | null;
  type: "run_completed";
  usage: Usage;
}

export interface ExtractionSucceededEvent {
  schema_warnings?: SchemaWarning[] | null;
  session_id: SessionId;
  structured_output: unknown;
  type: "extraction_succeeded";
}

export interface ExtractionFailedEvent {
  attempts: number;
  last_output: string;
  reason: string;
  session_id: SessionId;
  type: "extraction_failed";
}

export interface RunFailedEvent {
  error: string;
  error_class: AgentErrorClass;
  error_report?: AgentErrorReport | null;
  session_id: SessionId;
  terminal_cause_kind?: TurnTerminalCauseKind | null;
  type: "run_failed";
}

export interface HookStartedEvent {
  hook_id: HookId;
  point: HookPoint;
  type: "hook_started";
}

export interface HookCompletedEvent {
  duration_ms: number;
  hook_id: HookId;
  point: HookPoint;
  type: "hook_completed";
}

export interface HookFailedEvent {
  error: string;
  hook_id: HookId;
  point: HookPoint;
  type: "hook_failed";
}

export interface HookDeniedEvent {
  hook_id: HookId;
  message: string;
  payload?: unknown;
  point: HookPoint;
  reason_code: HookReasonCode;
  type: "hook_denied";
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

export interface ServerToolContentEvent {
  content: unknown;
  id?: string | null;
  name: string;
  type: "server_tool_content";
}

export interface ToolCallRequestedEvent {
  args: ToolCallArguments;
  id: string;
  name: string;
  type: "tool_call_requested";
}

export interface ToolResultReceivedEvent {
  content?: ContentBlock[];
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
  content?: ContentBlock[];
  duration_ms: number;
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
  retry?: LlmRetrySchedule | null;
  type: "retrying";
}

export interface SkillsResolvedEvent {
  injection_bytes: number;
  skills: SkillKey[];
  type: "skills_resolved";
}

export interface SkillResolutionFailedEvent {
  error?: string;
  reason?: SkillResolutionFailureReason;
  reference?: string;
  skill_key?: SkillKey | null;
  type: "skill_resolution_failed";
}

export interface InteractionCompleteEvent {
  interaction_id: InteractionId;
  result: string;
  structured_output?: unknown;
  type: "interaction_complete";
}

export interface InteractionCallbackPendingEvent {
  args: unknown;
  interaction_id: InteractionId;
  tool_name: string;
  type: "interaction_callback_pending";
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

export interface BackgroundJobCompletedEvent {
  detail: string;
  display_name: string;
  job_id: string;
  status?: string | null;
  terminal_status: BackgroundJobTerminalStatus;
  type: "background_job_completed";
}

export const KNOWN_AGENT_EVENT_TYPES = [
  "run_started",
  "run_completed",
  "extraction_succeeded",
  "extraction_failed",
  "run_failed",
  "hook_started",
  "hook_completed",
  "hook_failed",
  "hook_denied",
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
  "interaction_callback_pending",
  "interaction_failed",
  "stream_truncated",
  "tool_config_changed",
  "background_job_completed"
] as const;

export type KnownAgentEventType = typeof KNOWN_AGENT_EVENT_TYPES[number];

export type AgentEvent =
  RunStartedEvent |
  RunCompletedEvent |
  ExtractionSucceededEvent |
  ExtractionFailedEvent |
  RunFailedEvent |
  HookStartedEvent |
  HookCompletedEvent |
  HookFailedEvent |
  HookDeniedEvent |
  TurnStartedEvent |
  ReasoningDeltaEvent |
  ReasoningCompleteEvent |
  TextDeltaEvent |
  TextCompleteEvent |
  ServerToolContentEvent |
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
  InteractionCallbackPendingEvent |
  InteractionFailedEvent |
  StreamTruncatedEvent |
  ToolConfigChangedEvent |
  BackgroundJobCompletedEvent;
