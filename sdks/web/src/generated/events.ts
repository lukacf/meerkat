// Generated raw event types for @rkat/web
// Source: artifacts/schemas/events.json

export type AgentErrorClass = "llm" | "store" | "tool" | "policy_indeterminate" | "mcp" | "session_not_found" | "budget" | "max_tokens" | "content_filtered" | "max_turns" | "cancelled" | "invalid_state" | "operation_not_found" | "depth_limit" | "concurrency_limit" | "config" | "internal" | "build" | "auth" | "callback_pending" | "skill" | "structured_output" | "invalid_output_schema" | "hook" | "terminal" | "no_pending_boundary";

export type AgentErrorReason = {
  model: string;
  provider: Provider;
  reason: ModelFallbackSkipReason;
  reason_type: "model_fallback_resume_held";
} | {
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
  tool_use_id?: string;
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

export type AnthropicCacheControlPolicy = "disabled" | "automatic" | "system_prefix" | "system_and_conversation";

export type AnthropicCacheTtl = "5m" | "1h";

export type AnthropicCompactionConfig = {
  kind: "auto";
} | {
  edit: OpaqueProviderBody;
  kind: "custom";
};

export type AnthropicContextWindow = "one_megabyte";

export type AnthropicEffort = "low" | "medium" | "high" | "max" | "x_high";

export type AnthropicInferenceGeo = {
  kind: "us";
} | {
  kind: "global";
} | {
  kind: "other";
  region: string;
};

export type AnthropicThinkingConfig = {
  type: "adaptive";
} | {
  budget_tokens: number;
  type: "enabled";
};

export interface AssistantImageEvent {
  blob_ref: BlobRef;
  height: number;
  image_id: AssistantImageId;
  media_type: string;
  meta: ProviderImageMetadata;
  revised_prompt: RevisedPromptDisposition;
  width: number;
}

export type AssistantImageId = string;

export type AuthBindingRef = {
  binding: BindingId;
  origin?: BindingOrigin;
  profile?: ProfileId | null;
  realm: RealmId;
};

export type BackgroundJobTerminalStatus = "completed" | "failed" | "aborted" | "cancelled" | "retired" | "terminated";

export type BindingId = string;

export type BindingOrigin = "configured" | "synthetic_env_default";

export type BlobId = string;

export interface BlobRef {
  blob_id: BlobId;
  media_type: string;
}

export type BudgetType = "tokens" | "time" | "tool_calls";

export type CacheBreakpointBoundary = {
  kind: "system_profile_prefix";
  message_count: number;
} | {
  kind: "transcript_after";
  message_count: number;
};

export type CacheBreakpointDiscardOrigin = "authored_this_turn" | "persisted_evidence";

export type CacheBreakpointDiscardReason = {
  kind: "boundary_outside_committed_transcript";
  message_count: number;
  message_len: number;
} | {
  kind: "canonical_prefix_moved";
} | {
  detail: string;
  kind: "evidence_unusable";
} | {
  kind: "projected_boundary_unmappable";
};

export type CapabilityId = string;

export type CommsNoticeKind = string;

export type CompactionFailureReason = {
  error_class: AgentErrorClass;
  kind: "llm_failed";
  message: string;
} | {
  kind: "empty_summary";
} | {
  kind: "curator_failed";
  message: string;
} | {
  kind: "estimation_failed";
  message: string;
} | {
  attempted_entries: number;
  kind: "memory_indexing_failed";
  message: string;
} | {
  kind: "transcript_rewrite_failed";
  message: string;
} | {
  attempted_entries: number;
  kind: "projection_handoff_refused";
  message: string;
  preserved_history: CompactionPreservedHistoryFit;
  refusal: CompactionHandoffRefusal;
};

export type CompactionHandoffRefusal = "session_mismatch" | "runtime_epoch_rotated" | "runtime_epoch_retired" | "runtime_binding_rotated" | "runtime_binding_absent" | "durable_projection_unsupported" | "unclassified";

export type CompactionPreservedHistoryFit = "unclassified" | "still_fits" | "over_window";

export interface CompactionRewriteRange {
  end: number;
  start: number;
}

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
} | {
  source: "uri";
  uri: string;
} | {
  data: unknown;
  type: "structured";
} | {
  skill_key: SkillKey;
  text: string;
  type: "skill_context";
};

export type ContentInput = string | ContentBlock[];

export type ContextBudgetEstimateProvenance = "canonical_forecast" | "exact_provider_token_count";

export type ContextBudgetFact = {
  context_window_tokens: number;
  estimate_provenance?: ContextBudgetEstimateProvenance;
  estimated_input_tokens: number;
  estimated_tool_tokens: number;
  estimated_total_tokens: number;
  lowered_request_provenance?: LoweredRequestProvenance | null;
  max_input_tokens?: number | null;
  overage_tokens: number;
  provider_issued_input_tokens?: number | null;
  provider_lowered_encoded_bytes?: number | null;
  remaining_tokens: number;
  reserved_output_tokens: number;
  state: ContextBudgetState;
};

export type ContextBudgetState = "within" | "forecast_exceeded" | "exceeded";

export type CumulativeUsage = Usage;

export interface DeferredCatalogDelta {
  added_hidden_names?: ToolName[];
  pending_sources?: string[];
  removed_hidden_names?: ToolName[];
}

export type DiscardedCacheBreakpoint = {
  identity?: DiscardedCacheBreakpointIdentity | null;
  origin: CacheBreakpointDiscardOrigin;
  reason: CacheBreakpointDiscardReason;
};

export interface DiscardedCacheBreakpointIdentity {
  boundary: CacheBreakpointBoundary;
  model: string;
  provider: Provider;
}

export interface DisputedTurnUsageAccountingIdentity {
  active_model: string;
  active_provider: Provider;
  reported_model: string;
  reported_provider: Provider;
}

export type ExternalToolDeltaPhase = "pending" | "applied" | "draining" | "forced" | "failed";

export type GeminiImageMetadata = {
  continuity_ref?: string | null;
  response_id?: string | null;
  target_model: string;
};

export type GeminiThinkingConfig = {
  include_thoughts?: boolean | null;
  thinking_budget?: number | null;
  thinking_level?: GeminiThinkingLevel | null;
};

export type GeminiThinkingLevel = "minimal" | "low" | "medium" | "high";

export type HookFailureReason = {
  reason_code: "timeout";
  timeout_ms: number;
} | {
  message: string;
  reason_code: "execution_failed";
} | {
  message: string;
  reason_code: "config_invalid";
} | {
  reason_code: "observe_only_violation";
};

export type HookId = string;

export type HookPoint = "run_started" | "run_completed" | "run_failed" | "pre_llm_request" | "post_llm_response" | "pre_tool_execution" | "post_tool_execution" | "turn_boundary" | "runtime_input_accepted" | "runtime_input_rejected" | "runtime_input_deduplicated" | "peer_ingress_committed" | "peer_egress_committed" | "interaction_completed";

export type HookReasonCode = "policy_violation" | "safety_violation" | "schema_violation" | "timeout" | "runtime_error";

export type InteractionFailureReason = {
  kind: "cancelled";
} | {
  detail: string;
  kind: "abandoned";
} | {
  kind: "interaction_stream_abandoned";
  reason: InteractionStreamAbandonReason;
} | {
  detail: string;
  kind: "finalization_failed";
} | {
  attempts: number;
  kind: "extraction_failed";
  last_output: string;
  reason: string;
};

export type InteractionId = string;

export type InteractionStreamAbandonReason = "send_failed" | "admission_rejected" | "response_rejected" | "terminal_delivery_failed";

export type LlmProviderErrorKind = "invalid_request" | "content_filtered" | "server_error" | "server_overloaded" | "connection_reset" | "unknown" | "stream_parse_error" | "incomplete_response" | "authorization_route_changed" | "request_too_large" | "quota_exhausted" | "policy_stop";

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

export type LoweredRequestEncoding = "anthropic_messages_json" | "open_ai_responses_json" | "open_ai_chat_completions_json" | "gemini_generate_content_json";

export interface LoweredRequestProvenance {
  body_sha256: number[];
  encoding: LoweredRequestEncoding;
  provider: Provider;
}

export type MeerkatSchema = unknown;

export type ModelFallbackSkipReason = "provider_boundary" | "auth_unavailable" | "context_fit" | "context_unknown" | "output_budget" | "tool_parity" | "modality_parity" | "request_unsupported" | "admission_unavailable";

export type ModelFallbackSkippedTarget = {
  context?: ContextBudgetFact | null;
  identity: SessionLlmIdentity;
  reason: ModelFallbackSkipReason;
};

export type OpaqueProviderBody = string;

export type OpenAiImageMetadata = {
  image_generation_call_id?: string | null;
  response_id?: string | null;
  target_model: string;
};

export type OpenAiPromptCacheMode = "implicit" | "explicit";

export type OpenAiPromptCacheOptions = {
  mode?: OpenAiPromptCacheMode | null;
  ttl?: OpenAiPromptCacheTtl | null;
};

export type OpenAiPromptCacheRetention = "in_memory" | "24h";

export type OpenAiPromptCacheTtl = "30m";

export type OpenAiReasoningContext = "auto" | "current_turn" | "all_turns";

export type OpenAiReasoningMode = "standard" | "pro";

export type OpenAiTextVerbosity = "low" | "medium" | "high";

export type OutputSchema = {
  compat?: SchemaCompat;
  format?: SchemaFormat;
  name?: string | null;
  schema: MeerkatSchema;
  strict?: boolean;
};

export type PeerId = string;

export interface PendingCallbackToolCall {
  args: unknown;
  tool_name: string;
  tool_use_id: string;
}

export type PresentedTokenConvention = "anthropic_disjoint_input_components" | "open_ai_input_includes_cached_subset" | "gemini_prompt_includes_cached_subset" | "open_ai_compatible_prompt_includes_cache_details" | "host_declared_inclusive_input_total";

export type ProfileId = string;

export interface PromptText {
  content: string;
}

export type Provider = "anthropic" | "openai" | "gemini" | "self_hosted" | "other";

export type ProviderImageMetadata = {
  provider: "not_emitted";
} | OpenAiImageMetadata | GeminiImageMetadata;

export type ProviderParamsOverride = {
  max_output_tokens?: number | null;
  provider_tag?: ProviderTag | null;
  reasoning?: ReasoningMode | null;
  temperature?: number | null;
  thinking_budget_tokens?: number | null;
  top_p?: number | null;
};

export type ProviderTag = {
  cache_control?: AnthropicCacheControlPolicy | null;
  cache_ttl?: AnthropicCacheTtl | null;
  compaction?: AnthropicCompactionConfig | null;
  context?: AnthropicContextWindow | null;
  effort?: AnthropicEffort | null;
  inference_geo?: AnthropicInferenceGeo | null;
  provider: "anthropic";
  structured_output?: OutputSchema | null;
  supports_temperature_override?: boolean | null;
  thinking?: AnthropicThinkingConfig | null;
  thinking_budget_tokens?: number | null;
  top_k?: number | null;
  web_search?: OpaqueProviderBody | null;
} | {
  chat_template_kwargs?: OpaqueProviderBody | null;
  frequency_penalty?: number | null;
  presence_penalty?: number | null;
  prompt_cache_enabled?: boolean | null;
  prompt_cache_key?: string | null;
  prompt_cache_options?: OpenAiPromptCacheOptions | null;
  prompt_cache_retention?: OpenAiPromptCacheRetention | null;
  provider: "open_ai";
  reasoning?: OpaqueProviderBody | null;
  reasoning_context?: OpenAiReasoningContext | null;
  reasoning_effort?: ReasoningEffort | null;
  reasoning_mode?: OpenAiReasoningMode | null;
  seed?: number | null;
  store?: boolean | null;
  structured_output?: OutputSchema | null;
  supports_reasoning_override?: boolean | null;
  supports_temperature_override?: boolean | null;
  text_verbosity?: OpenAiTextVerbosity | null;
  thinking?: OpaqueProviderBody | null;
  web_search?: OpaqueProviderBody | null;
} | {
  cached_content_name?: string | null;
  candidate_count?: number | null;
  google_search?: OpaqueProviderBody | null;
  provider: "gemini";
  structured_output?: OutputSchema | null;
  thinking?: GeminiThinkingConfig | null;
  thinking_budget?: number | null;
  thinking_level?: GeminiThinkingLevel | null;
  top_k?: number | null;
  top_p?: number | null;
} | {
  bag: StructuredProviderExtension;
  provider: "unknown";
};

export interface ProviderTokenAccounting {
  aggregation: TokenAggregationProvenance;
  convention: PresentedTokenConvention;
  model: string;
  presented_tokens: number;
  provider: Provider;
}

export type RealmId = string;

export type ReasoningEffort = "none" | "low" | "medium" | "high" | "xhigh" | "max";

export type ReasoningMode = "emit" | "silent" | "off";

export type RevisedPromptDisposition = {
  disposition: "not_requested";
} | {
  disposition: "unsupported_by_backend";
} | {
  disposition: "unchanged";
} | {
  disposition: "revised";
  source: RevisedPromptSource;
  text: PromptText;
};

export type RevisedPromptSource = "provider" | "meerkat_projection";

export type RunInput = {
  content: ContentInput;
  kind: "content";
} | {
  kind: "pending_tool_results";
};

export type SchemaCompat = "lossy" | "strict";

export type SchemaFormat = "meerkat_v1";

export interface SchemaWarning {
  message: string;
  path: string;
  provider: Provider;
}

export type SenderContentTaint = "clean" | "tainted";

export type ServerToolKind = {
  kind: "web_search";
} | {
  kind: "google_search";
} | {
  kind: "provider_native";
  name: string;
};

export type SessionId = string;

export type SessionLlmIdentity = {
  auth_binding?: AuthBindingRef | null;
  model: string;
  provider: Provider;
  provider_params?: ProviderParamsOverride | null;
  self_hosted_server_id?: string | null;
};

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

export type StreamScopeFrame = {
  scope: "primary";
  session_id: string;
} | {
  agent_identity: string;
  flow_run_id: string;
  scope: "mob_member";
};

export type StreamTruncationReason = {
  kind: "channel_full";
} | {
  dropped: number;
  kind: "stream_lagged";
} | {
  dropped: number;
  kind: "output_audio_degraded";
} | {
  kind: "remote_cursor_overrun";
  watermark: number;
} | {
  durable_seq: number;
  encoded_bytes: number;
  kind: "oversized_remote_event";
  max_bytes: number;
};

export interface StructuredProviderExtension {
  body?: string;
  key: string;
  namespace: string;
}

export type SystemNoticePeer = {
  display_name?: string | null;
  id: PeerId;
};

export interface SystemTime {
  nanos_since_epoch: number;
  secs_since_epoch: number;
}

export type TokenAggregationProvenance = "sum_disjoint_provider_components" | "provider_inclusive_input_total";

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
};

export type ToolConfigChangedPayload = {
  applied_at_turn?: number | null;
  deferred_catalog_delta?: DeferredCatalogDelta | null;
  domain?: ToolConfigChangeDomain | null;
  operation: ToolConfigChangeOperation;
  persisted: boolean;
  status_info: ToolConfigChangeStatus;
  target: string;
};

export type ToolName = string;

export interface TranscriptEditRewriteRange {
  end: number;
  start: number;
}

export type TranscriptRevisionBody = {
  created_at: SystemTime;
  messages: unknown[];
  parent_revision?: string | null;
  revision: string;
};

export interface TranscriptRewriteAuditReceiptBatch {
  commits: TranscriptRewriteCommit[];
  end_prefix: TranscriptRewritePrefixAccumulator;
  start_prefix: TranscriptRewritePrefixAccumulator;
}

export type TranscriptRewriteCommit = {
  actor?: string | null;
  committed_at: SystemTime;
  messages_after: number;
  messages_before: number;
  original_span_digest: string;
  parent_revision: string;
  reason: TranscriptRewriteReason;
  replacement_digest: string;
  revision: string;
  rewrite_generation?: number;
  selection: TranscriptRewriteSelection;
};

export interface TranscriptRewritePrefixAccumulator {
  digest: string;
  occurrence_count: number;
}

export type TranscriptRewriteReason = {
  kind: string;
  note?: string | null;
};

export interface TranscriptRewriteRecord {
  commit: TranscriptRewriteCommit;
  digest_format?: number;
  parent_body: TranscriptRevisionBody;
  revision_body: TranscriptRevisionBody;
}

export type TranscriptRewriteSelection = {
  end: number;
  start: number;
  type: "message_range";
} | {
  range: TranscriptEditRewriteRange;
  type: "edit_message_range";
} | {
  range: CompactionRewriteRange;
  type: "compaction_message_range";
};

export type TurnTerminalCauseKind = "unknown" | "hook_denied" | "hook_failure" | "llm_failure" | "tool_failure" | "structured_output_validation_failed" | "budget_exhausted" | "time_budget_exceeded" | "retry_exhausted" | "turn_limit_reached" | "runtime_apply_failure" | "fatal_failure";

export type TurnTerminalOutcome = "none" | "completed" | "failed" | "cancelled" | "budget_exhausted" | "time_budget_exceeded" | "structured_output_validation_failed";

export type TurnUsage = {
  accounting: ProviderTokenAccounting;
  cache_creation_tokens?: number | null;
  cache_read_tokens?: number | null;
  input_tokens: number;
  output_tokens: number;
  provider_accounting?: ProviderTokenAccounting | null;
};

export interface UnmeasuredTurnUsageAccounting {
  model: string;
  provider: Provider;
}

export type Usage = {
  cache_creation_tokens?: number | null;
  cache_read_tokens?: number | null;
  input_tokens: number;
  output_tokens: number;
  provider_accounting?: ProviderTokenAccounting | null;
};

export interface RunStartedEvent {
  input: RunInput;
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
  usage: CumulativeUsage;
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
  error_report: AgentErrorReport;
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
  hook_id: HookId;
  point: HookPoint;
  reason: HookFailureReason;
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
  kind: ServerToolKind;
  type: "server_tool_content";
}

export interface AssistantImageAppendedEvent {
  image: AssistantImageEvent;
  type: "assistant_image_appended";
}

export interface ToolCallRequestedEvent {
  args: ToolCallArguments;
  id: string;
  name: string;
  type: "tool_call_requested";
}

export interface ToolResultReceivedEvent {
  content: ContentBlock[];
  id: string;
  is_error: boolean;
  name: string;
  type: "tool_result_received";
}

export interface TurnCompletedEvent {
  stop_reason: StopReason;
  type: "turn_completed";
  usage?: TurnUsage | null;
}

export interface ToolExecutionStartedEvent {
  id: string;
  name: string;
  type: "tool_execution_started";
}

export interface ToolExecutionCompletedEvent {
  content: ContentBlock[];
  duration_ms: number;
  id: string;
  is_error: boolean;
  name: string;
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
  reason: CompactionFailureReason;
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
  retry: LlmRetrySchedule;
  type: "retrying";
}

export interface ModelFallbackSkippedEvent {
  retry: LlmRetrySchedule;
  target: ModelFallbackSkippedTarget;
  type: "model_fallback_skipped";
}

export interface ModelFallbackStagedEvent {
  previous: SessionLlmIdentity;
  retry: LlmRetrySchedule;
  target: SessionLlmIdentity;
  type: "model_fallback_staged";
}

export interface ModelFallbackCommittedEvent {
  previous: SessionLlmIdentity;
  retry: LlmRetrySchedule;
  target: SessionLlmIdentity;
  type: "model_fallback_committed";
}

export interface ModelFallbackTargetFailedEvent {
  error: AgentErrorReport;
  previous: SessionLlmIdentity;
  target: SessionLlmIdentity;
  type: "model_fallback_target_failed";
}

export interface SkillsResolvedEvent {
  injection_bytes: number;
  skills: SkillKey[];
  type: "skills_resolved";
}

export interface SkillResolutionFailedEvent {
  reason: SkillResolutionFailureReason;
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
  pending_tool_calls?: PendingCallbackToolCall[];
  tool_name: string;
  type: "interaction_callback_pending";
}

export interface InteractionFailedEvent {
  interaction_id: InteractionId;
  reason: InteractionFailureReason;
  type: "interaction_failed";
}

export interface StreamTruncatedEvent {
  reason: StreamTruncationReason;
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
  terminal_status: BackgroundJobTerminalStatus;
  type: "background_job_completed";
}

export interface TranscriptRewriteCommittedEvent {
  record: TranscriptRewriteRecord;
  session_id: SessionId;
  type: "transcript_rewrite_committed";
}

export interface TranscriptRewriteAuditReceiptCommittedEvent {
  final_assistant_text?: string | null;
  receipt: TranscriptRewriteAuditReceiptBatch;
  session_id: SessionId;
  type: "transcript_rewrite_audit_receipt_committed";
}

export interface ProviderCacheBreakpointsDiscardedEvent {
  discarded: DiscardedCacheBreakpoint[];
  retained: number;
  session_id: SessionId;
  type: "provider_cache_breakpoints_discarded";
}

export interface PeerContentIngestedEvent {
  kind: CommsNoticeKind;
  peer?: SystemNoticePeer | null;
  request_id?: string | null;
  sender_taint?: SenderContentTaint | null;
  type: "peer_content_ingested";
}

export interface TurnUsageAccountingUnmeasuredEvent {
  session_id: SessionId;
  type: "turn_usage_accounting_unmeasured";
  unmeasured: UnmeasuredTurnUsageAccounting;
}

export interface TurnUsageAccountingIdentityDisputedEvent {
  dispute: DisputedTurnUsageAccountingIdentity;
  session_id: SessionId;
  type: "turn_usage_accounting_identity_disputed";
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
  "assistant_image_appended",
  "tool_call_requested",
  "tool_result_received",
  "turn_completed",
  "turn_usage_accounting_unmeasured",
  "turn_usage_accounting_identity_disputed",
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
  "background_job_completed",
  "transcript_rewrite_committed",
  "peer_content_ingested",
  "provider_cache_breakpoints_discarded"
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
  AssistantImageAppendedEvent |
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
  ModelFallbackSkippedEvent |
  ModelFallbackStagedEvent |
  ModelFallbackCommittedEvent |
  ModelFallbackTargetFailedEvent |
  SkillsResolvedEvent |
  SkillResolutionFailedEvent |
  InteractionCompleteEvent |
  InteractionCallbackPendingEvent |
  InteractionFailedEvent |
  StreamTruncatedEvent |
  ToolConfigChangedEvent |
  BackgroundJobCompletedEvent |
  TranscriptRewriteCommittedEvent |
  TranscriptRewriteAuditReceiptCommittedEvent |
  ProviderCacheBreakpointsDiscardedEvent |
  PeerContentIngestedEvent |
  TurnUsageAccountingUnmeasuredEvent |
  TurnUsageAccountingIdentityDisputedEvent;
