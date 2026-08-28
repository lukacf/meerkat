// Generated agent-event payload types for the Meerkat TypeScript SDK
// Source: artifacts/schemas/events.json (named $defs of the event roots)
//
// Every named payload definition in the event schema is emitted here so a
// consumer can name the reason enums and payload records the runtime
// publishes instead of mirroring them by hand. Coverage is asserted by
// scripts/verify_sdk_event_inventory.py.

import type {
  AssistantImageId,
  BackgroundJobTerminalStatus,
  BlobId,
  BlobRef,
  CommsNoticeKind,
  CompactionRewriteRange,
  ContentBlock,
  ContentInput,
  DeferredCatalogDelta,
  ExternalToolDeltaPhase,
  GeminiImageMetadata,
  OpenAiImageMetadata,
  PeerId,
  PromptText,
  Provider,
  ProviderImageMetadata,
  RevisedPromptDisposition,
  RevisedPromptSource,
  SenderContentTaint,
  SkillKey,
  SkillName,
  SourceUuid,
  SystemNoticePeer,
  ToolConfigChangeDomain,
  ToolConfigChangeOperation,
  ToolConfigChangeStatus,
  ToolConfigChangedPayload,
  ToolName,
  TranscriptEditRewriteRange,
  TranscriptRewriteReason,
  TranscriptRewriteSelection,
} from './types.js';

export type AgentErrorClass = "llm" | "store" | "tool" | "policy_indeterminate" | "mcp" | "session_not_found" | "budget" | "max_tokens" | "content_filtered" | "max_turns" | "cancelled" | "invalid_state" | "operation_not_found" | "depth_limit" | "concurrency_limit" | "config" | "internal" | "build" | "auth" | "callback_pending" | "skill" | "structured_output" | "invalid_output_schema" | "hook" | "terminal" | "no_pending_boundary";

/**
 * Stable identifier for a configured hook.
 */
export type HookId = string;

/**
 * Hook points available in V1.
 */
export type HookPoint = "run_started" | "run_completed" | "run_failed" | "pre_llm_request" | "post_llm_response" | "pre_tool_execution" | "post_tool_execution" | "turn_boundary";

/**
 * Typed reason codes for guardrail denials.
 */
export type HookReasonCode = "policy_violation" | "safety_violation" | "schema_violation" | "timeout" | "runtime_error";

export type LlmProviderErrorKind = "invalid_request" | "content_filtered" | "server_error" | "server_overloaded" | "connection_reset" | "unknown" | "stream_parse_error" | "incomplete_response" | "authorization_route_changed" | "request_too_large";

export type LlmProviderErrorRetryability = "retryable" | "non_retryable";

/**
 * Closed machine-owned classifier for why a turn reached a terminal failure.
 */
export type TurnTerminalCauseKind = "unknown" | "hook_denied" | "hook_failure" | "llm_failure" | "tool_failure" | "structured_output_validation_failed" | "budget_exhausted" | "time_budget_exceeded" | "retry_exhausted" | "turn_limit_reached" | "runtime_apply_failure" | "fatal_failure";

/**
 * Terminal outcome of a turn.
 */
export type TurnTerminalOutcome = "none" | "completed" | "failed" | "cancelled" | "budget_exhausted" | "time_budget_exceeded" | "structured_output_validation_failed";

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

/**
 * Typed public event payload for a canonical assistant image block appended to history.
 */
export interface AssistantImageEvent {
  blob_ref: BlobRef;
  height: number;
  image_id: AssistantImageId;
  media_type: string;
  meta: ProviderImageMetadata;
  revised_prompt: RevisedPromptDisposition;
  width: number;
}

/**
 * Type of budget being tracked
 */
export type BudgetType = "tokens" | "time" | "tool_calls";

/**
 * Typed cause of a compaction projection-handoff refusal.
 *
 * The runtime coordinator refuses for structurally different reasons, and a
 * host routing severity (page vs log) needs the cause, not a message
 * substring. This is the wire-stable vocabulary carried by
 * [`crate::event::CompactionFailureReason::ProjectionHandoffRefused`];
 * [`CompactionCommitCoordinationError::refusal`] is its only producer.
 */
export type CompactionHandoffRefusal = "session_mismatch" | "runtime_epoch_rotated" | "runtime_epoch_retired" | "runtime_binding_rotated" | "runtime_binding_absent" | "durable_projection_unsupported" | "unclassified";

/**
 * Whether the history a failed compaction preserved can still be sent.
 *
 * This is the severity discriminator for compaction-persistence failures: the
 * same refusal is a log line on a session that still fits its window and a
 * hard wedge on one that does not (every subsequent turn is refused, and the
 * only path out is a compaction that persists). Classification reuses the
 * pre-dispatch authorities rather than a second limit table:
 * [`ContextBudgetState`](crate::ContextBudgetState) over the active model
 * profile's window, and the compactor's effective request-byte cap over the
 * exact provider-lowered body size.
 *
 * Both authorities are read at composed-request scope, which is the scope the
 * question is asked at: the transcript is not what crosses the provider
 * boundary, the request built from it is. A transcript-only token measure
 * under-reports every request by the tool schemas that ride it, which is the
 * exact shape that used to be reported as [`Self::StillFits`] while every
 * turn died on the provider's context limit.
 */
export type CompactionPreservedHistoryFit = "unclassified" | "still_fits" | "over_window";

/**
 * Typed, serializable cause of a non-fatal compaction failure.
 *
 * Compaction is best-effort: when it fails the agent continues with the
 * uncompacted history. This enum is the wire-stable projection of the
 * in-flight [`crate::agent::compact::CompactionError`] plus the post-summary
 * commit failures (memory indexing, transcript rewrite) surfaced from the
 * agent loop. Each variant carries the typed cause; the [`Display`] impl
 * renders the human-facing message consumers used to read off the old
 * `error: String` field.
 *
 * [`Display`]: std::fmt::Display
 */
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

/**
 * Provider-native convention used to normalize tokens presented to a model.
 */
export type PresentedTokenConvention = "anthropic_disjoint_input_components" | "open_ai_input_includes_cached_subset" | "gemini_prompt_includes_cached_subset" | "open_ai_compatible_prompt_includes_cache_details" | "host_declared_inclusive_input_total";

/**
 * How the provider adapter obtained the normalized presented-token total.
 */
export type TokenAggregationProvenance = "sum_disjoint_provider_components" | "provider_inclusive_input_total";

/**
 * One provider adapter's normalized accounting for a single model turn.
 */
export interface ProviderTokenAccounting {
  aggregation: TokenAggregationProvenance;
  convention: PresentedTokenConvention;
  model: string;
  presented_tokens: number;
  provider: Provider;
}

/**
 * Token usage statistics.
 *
 * # The same field names carry two different denominators
 *
 * This shape is reused for two semantically different accounts, and
 * `input_tokens` does not mean the same thing in both:
 *
 * - **Per-call** ([`TurnUsage`], carried by `turn_completed.usage`): raw
 *   provider counters for exactly one provider request. For Anthropic
 *   `input_tokens` is the *uncached* input only, with cache-write and
 *   cache-read input reported separately in `cache_creation_tokens` and
 *   `cache_read_tokens`. The normalized presented-input total for that one
 *   call is [`TurnUsage::presented_tokens`].
 * - **Cumulative** ([`CumulativeUsage`], carried by `run_completed.usage`): a
 *   running total over every provider call recorded on the *session*, not on
 *   one run. Its `input_tokens` is the saturating sum of each call's
 *   *presented* tokens (see [`CumulativeUsage::add_turn`]), and its cache
 *   detail fields are always `None` because their relationship to the input
 *   total is provider-specific.
 *
 * # What consumers must not sum
 *
 * - Never sum `run_completed.usage` with per-call usage, and never sum
 *   `run_completed.usage` across runs: each value is already the whole
 *   session's total to date, so adding two of them double-counts everything
 *   before the later one. Take the latest value.
 * - Never sum per-call `input_tokens` and expect it to match
 *   `run_completed.usage.input_tokens`. On a cache-heavy Anthropic session the
 *   two use different denominators and will not agree. Sum
 *   `turn_completed.usage.accounting.presented_tokens` (that is,
 *   [`TurnUsage::presented_tokens`]) instead, which is exactly what
 *   [`CumulativeUsage::add_turn`] does.
 * - Do not expect the per-call rows to reconcile with the cumulative account
 *   either. Intermediate tool-loop calls, the structured-output extraction
 *   call, and the compaction summary call are all charged to the cumulative
 *   account and publish no `turn_completed` row, so the rows cover a strict
 *   subset of the tokens.
 *
 * The worked example lives in `docs/reference/usage-accounting.mdx`. Its
 * numbers are pinned against the agent loop by
 * `turn_rows_cover_one_call_while_the_run_total_is_session_cumulative`
 * (`meerkat-core/src/agent/usage_accounting_tests.rs`) and against this type's
 * arithmetic by `cumulative_usage_matches_documented_aggregation_example`.
 */
export type Usage = {
  cache_creation_tokens?: number | null;
  cache_read_tokens?: number | null;
  input_tokens: number;
  output_tokens: number;
  provider_accounting?: ProviderTokenAccounting | null;
};

/**
 * Cumulative usage across committed turns. Cache detail counters are not
 * aggregated because their relationship to input totals is provider-specific.
 *
 * This value is **already a total**, and the total is session-scoped: on the
 * event stream it is `Session::total_usage()`, which is persisted with the
 * session and restored on resume, so `run_completed.usage` on the second run of
 * a session already contains the first run's calls. Adding it to per-call
 * usage, or summing the values observed on two runs, double-counts. It also
 * intentionally carries no [`crate::ProviderTokenAccounting`], because one
 * session may span providers and models and so cannot truthfully claim a
 * single per-call convention; per-model attribution is read from the per-call
 * `turn_completed` rows, which cover only the calls that closed a run.
 */
export type CumulativeUsage = Usage;

/**
 * Which side of a turn commit produced the discarded proof.
 *
 * Load-bearing for hosts: `PersistedEvidence` is inherited poison from an
 * earlier turn, `AuthoredThisTurn` is this turn's own lowering disagreeing
 * with the committed transcript. The two have different root causes and are
 * otherwise indistinguishable from the outside.
 */
export type CacheBreakpointDiscardOrigin = "authored_this_turn" | "persisted_evidence";

/**
 * Why one provider-authored cache breakpoint was discarded instead of
 * retained as durable session evidence.
 *
 * A cache breakpoint is an optimization artifact anchored to the exact
 * transcript head a provider lowered. Ordinary transcript motion - a
 * synthetic-notice refresh, a compaction, a re-materialized prefix - moves
 * that anchor without saying anything about the transcript's own integrity.
 * Every variant here therefore describes the ARTIFACT. Faults that describe
 * the committed TRANSCRIPT stay [`CacheBreakpointEvidenceError`].
 */
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

/**
 * Stable provider-independent identity of an authored cache breakpoint.
 */
export type CacheBreakpointBoundary = {
  kind: "system_profile_prefix";
  message_count: number;
} | {
  kind: "transcript_after";
  message_count: number;
};

/**
 * Identity of a discarded proof, when the proof itself could be decoded.
 */
export interface DiscardedCacheBreakpointIdentity {
  boundary: CacheBreakpointBoundary;
  model: string;
  provider: Provider;
}

/**
 * One provider-authored cache proof that could not be retained.
 */
export type DiscardedCacheBreakpoint = {
  identity?: DiscardedCacheBreakpointIdentity | null;
  origin: CacheBreakpointDiscardOrigin;
  reason: CacheBreakpointDiscardReason;
};

/**
 * One turn's provider-authored accounting named a different provider/model
 * than the request it answered.
 *
 * # Why this is not the same fault as absent accounting
 *
 * A mismatched identity still arrives with a complete, internally consistent
 * measurement: the [`PresentedTokenConvention`] travels with the number, so
 * the counters mean what they say regardless of which name is attached. The
 * disputed fact is attribution alone, so the token axis still advances on the
 * number the provider actually sent. Absent accounting has no number at all
 * and therefore cannot advance anything. Collapsing the two would either kill
 * correct work over a name or fabricate counters over silence.
 *
 * # Why the reported identity is preserved verbatim
 *
 * Rewriting `reported_*` to the active identity would publish an agreement
 * that was never observed - a guess laundered as evidence. Both sides are
 * carried so a host can see exactly who disagreed with whom.
 */
export interface DisputedTurnUsageAccountingIdentity {
  active_model: string;
  active_provider: Provider;
  reported_model: string;
  reported_provider: Provider;
}

/**
 * Typed reason a hook execution failed (engine-level fault, not a guardrail
 * denial).
 *
 * Mirrors the [`HookReasonCode`] precedent: the variant is the typed owner of
 * the failure cause; the human-readable string is a [`Display`] derivation,
 * never a separately-stored field.
 *
 * [`Display`]: std::fmt::Display
 */
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

/**
 * Typed reason an interaction stream was abandoned before normal terminal
 * delivery. This is intentionally distinct from [`InteractionStreamState::Expired`]:
 * expiry proves that the attach TTL elapsed while the stream was still
 * reserved, whereas abandonment records an observed failure.
 */
export type InteractionStreamAbandonReason = "send_failed" | "admission_rejected" | "response_rejected" | "terminal_delivery_failed";

/**
 * Typed reason an interaction-scoped run failed (terminal event for tap
 * subscribers).
 *
 * Mirrors [`CompactionFailureReason`]: the variant is the typed owner of the
 * failure cause; the human-readable string is a [`Display`] derivation,
 * never a separately-carried field.
 *
 * [`Display`]: std::fmt::Display
 */
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

/**
 * Unique identifier for an interaction.
 */
export type InteractionId = string;

/**
 * Closed classifier for recoverable LLM failures.
 */
export type LlmRetryFailureKind = "rate_limited" | "network_timeout" | "call_timeout" | "retryable_provider_error";

/**
 * Typed recoverable LLM failure carried through retry authority.
 */
export type LlmRetryFailure = {
  duration_ms?: number | null;
  kind: LlmRetryFailureKind;
  message: string;
  provider: string;
  retry_after_ms?: number | null;
};

/**
 * Typed retry delay plan selected for a recoverable LLM failure.
 */
export type LlmRetryPlan = {
  attempt: number;
  budget_capped: boolean;
  computed_delay_ms: number;
  max_retries: number;
  rate_limit_floor_applied: boolean;
  retry_after_hint_ms?: number | null;
  selected_delay_ms: number;
};

/**
 * Recoverable LLM retry lifecycle payload accepted by turn authority.
 */
export interface LlmRetrySchedule {
  failure: LlmRetryFailure;
  plan: LlmRetryPlan;
}

/**
 * One externally routed callback tool call inside a suspended assistant
 * tool-use batch.
 */
export interface PendingCallbackToolCall {
  args: unknown;
  tool_name: string;
  tool_use_id: string;
}

/**
 * Typed input fact for a run boundary.
 *
 * A run either starts from caller-provided content or resumes from tool
 * results already staged at the session's pending-continuation boundary.
 * The pending-tail case is its own typed variant — run-boundary events and
 * hooks never fabricate an empty-string prompt to stand in for it.
 */
export type RunInput = {
  content: ContentInput;
  kind: "content";
} | {
  kind: "pending_tool_results";
};

/**
 * Warnings emitted during schema lowering.
 */
export interface SchemaWarning {
  message: string;
  path: string;
  provider: Provider;
}

/**
 * Typed semantic kind of a provider-executed (server-side) tool.
 *
 * This is the single typed owner of the server-tool identity. Each provider
 * adapter parses its native discriminator into this enum once at the streaming
 * boundary, so downstream consumers never re-classify by matching a
 * `name: String`. Provider-native sub-event detail (e.g. OpenAI's
 * `web_search_call` vs `web_search_result`) is preserved in the accompanying
 * `content` JSON, not in this kind.
 *
 * `ProviderNative` is the verbatim escape hatch for dynamic provider tool
 * names (Anthropic `server_tool_use` carries an arbitrary tool name that must
 * round-trip exactly on replay). It is the only variant carrying a string,
 * and that string IS the typed fact — not a re-derivable label.
 */
export type ServerToolKind = {
  kind: "web_search";
} | {
  kind: "google_search";
} | {
  kind: "provider_native";
  name: string;
};

/**
 * Unique identifier for a session (UUID v7 for time-ordering)
 */
export type SessionId = string;

/**
 * Slug-validated capability identifier for skill requirements.
 *
 * Replaces the legacy `Vec<String>` capability lists with a typed
 * namespace. Parsed at construction, so callers cannot smuggle invalid
 * identifiers into descriptors or requirements.
 */
export type CapabilityId = string;

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

/**
 * Why the model stopped generating
 */
export type StopReason = "end_turn" | "tool_use" | "max_tokens" | "stop_sequence" | "content_filter" | "cancelled";

/**
 * Typed reason a best-effort event stream dropped frames.
 *
 * The variant is the typed owner of the truncation cause; the human-readable
 * string is a [`Display`](std::fmt::Display) derivation, never a
 * separately-carried field. `StreamTruncated` is a UI hint — the terminal
 * event remains authoritative.
 */
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

export type ToolCallArguments = Record<string, unknown>;

export interface SystemTime {
  nanos_since_epoch: number;
  secs_since_epoch: number;
}

/**
 * Immutable rewrite commit that advances a session transcript head.
 */
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

/**
 * Rolling, canonical identity of an ordered exact rewrite-commit prefix.
 *
 * This is a semantic graph fact, not a replay-cursor assertion. It is carried
 * by the graph, folded into checkpoint authority, and independently matched
 * against the EventStore's receipt. One ordinary lineage-tail commit extends
 * the accumulator with one commit serialization; it never re-hashes the
 * accumulated prefix.
 */
export interface TranscriptRewritePrefixAccumulator {
  digest: string;
  occurrence_count: number;
}

/**
 * Receipt for one non-empty ordered transcript-rewrite suffix.
 *
 * The transition is self-verifying:
 * `start_prefix.extend(commits) == end_prefix`. Occurrence generations are
 * checked by [`TranscriptRewritePrefixAccumulator::extend`], so neither a gap
 * nor a duplicate can be hidden inside one receipt.
 */
export interface TranscriptRewriteAuditReceiptBatch {
  commits: TranscriptRewriteCommit[];
  end_prefix: TranscriptRewritePrefixAccumulator;
  start_prefix: TranscriptRewritePrefixAccumulator;
}

/**
 * Immutable transcript revision body retained by the session-local graph.
 */
export type TranscriptRevisionBody = {
  created_at: SystemTime;
  messages: unknown[];
  parent_revision?: string | null;
  revision: string;
};

/**
 * Self-contained append-only transcript rewrite record.
 */
export interface TranscriptRewriteRecord {
  commit: TranscriptRewriteCommit;
  digest_format?: number;
  parent_body: TranscriptRevisionBody;
  revision_body: TranscriptRevisionBody;
}

/**
 * Usage for one provider turn.
 *
 * This is deliberately distinct from cumulative [`Usage`] on event
 * boundaries, while retaining the existing flat wire shape.
 *
 * # Model and provider attribution
 *
 * `accounting` is the single owner of per-call attribution: it carries the
 * [`crate::Provider`] and the model string of the request that produced these
 * counters, minted by the provider adapter from the lowered request rather than
 * from configuration intent. A consumer reading only the event stream can
 * therefore attribute one `turn_completed` row to a model without joining
 * against session metadata:
 * `turn_completed.usage.accounting.provider` /
 * `turn_completed.usage.accounting.model`.
 *
 * There is deliberately no second copy of the model or provider beside
 * `accounting` on the event: attribution has one owner.
 */
export type TurnUsage = {
  accounting: ProviderTokenAccounting;
  cache_creation_tokens?: number | null;
  cache_read_tokens?: number | null;
  input_tokens: number;
  output_tokens: number;
  provider_accounting?: ProviderTokenAccounting | null;
};

/**
 * Provider token accounting for one model turn was absent.
 *
 * # What this makes untrue, and what it does not
 *
 * A provider that streams a complete answer and no usage event has stated
 * nothing about tokens. The turn's SEMANTIC facts - what the model said,
 * which tools it asked for, what may be committed to the transcript - are
 * untouched by that silence, so this absence terminalizes none of them. Only
 * the accounting axis is affected, and it is affected by being left exactly
 * where it was: no counter advances, no per-call row is published, and no
 * value is substituted for the one the provider did not send.
 *
 * This value states the absence and nothing more. It does not assert that the
 * turn completed: the turn's own terminal fact has its own owner, and a turn
 * whose accounting went missing can still fail afterwards on unrelated
 * grounds.
 *
 * # Why the identity here is not accounting
 *
 * `provider` and `model` name the REQUEST this turn was lowered for. They are
 * the address of the missing measurement, not a reconstruction of it, and
 * carry no counters. This is exactly the line
 * [`ProviderTokenAccounting::host_declared`] would cross: it would mint
 * provider attribution for numbers no provider issued.
 */
export interface UnmeasuredTurnUsageAccounting {
  model: string;
  provider: Provider;
}

/**
 * Events emitted during agent execution
 *
 * These events form the streaming API for consumers.
 */
export type AgentEvent = {
  input: RunInput;
  session_id: SessionId;
  type: "run_started";
} | {
  extraction_required?: boolean;
  result: string;
  session_id: SessionId;
  structured_output?: unknown;
  terminal_cause_kind?: TurnTerminalCauseKind | null;
  type: "run_completed";
  usage: CumulativeUsage;
} | {
  schema_warnings?: SchemaWarning[] | null;
  session_id: SessionId;
  structured_output: unknown;
  type: "extraction_succeeded";
} | {
  attempts: number;
  last_output: string;
  reason: string;
  session_id: SessionId;
  type: "extraction_failed";
} | {
  error_report: AgentErrorReport;
  session_id: SessionId;
  terminal_cause_kind?: TurnTerminalCauseKind | null;
  type: "run_failed";
} | {
  hook_id: HookId;
  point: HookPoint;
  type: "hook_started";
} | {
  duration_ms: number;
  hook_id: HookId;
  point: HookPoint;
  type: "hook_completed";
} | {
  hook_id: HookId;
  point: HookPoint;
  reason: HookFailureReason;
  type: "hook_failed";
} | {
  hook_id: HookId;
  message: string;
  payload?: unknown;
  point: HookPoint;
  reason_code: HookReasonCode;
  type: "hook_denied";
} | {
  turn_number: number;
  type: "turn_started";
} | {
  delta: string;
  type: "reasoning_delta";
} | {
  content: string;
  type: "reasoning_complete";
} | {
  delta: string;
  type: "text_delta";
} | {
  content: string;
  type: "text_complete";
} | {
  content: unknown;
  id?: string | null;
  kind: ServerToolKind;
  type: "server_tool_content";
} | {
  image: AssistantImageEvent;
  type: "assistant_image_appended";
} | {
  args: ToolCallArguments;
  id: string;
  name: string;
  type: "tool_call_requested";
} | {
  content: ContentBlock[];
  id: string;
  is_error: boolean;
  name: string;
  type: "tool_result_received";
} | {
  stop_reason: StopReason;
  type: "turn_completed";
  usage?: TurnUsage | null;
} | {
  id: string;
  name: string;
  type: "tool_execution_started";
} | {
  content: ContentBlock[];
  duration_ms: number;
  id: string;
  is_error: boolean;
  name: string;
  type: "tool_execution_completed";
} | {
  id: string;
  name: string;
  timeout_ms: number;
  type: "tool_execution_timed_out";
} | {
  estimated_history_tokens: number;
  input_tokens: number;
  message_count: number;
  type: "compaction_started";
} | {
  messages_after: number;
  messages_before: number;
  summary_tokens: number;
  type: "compaction_completed";
} | {
  reason: CompactionFailureReason;
  type: "compaction_failed";
} | {
  budget_type: BudgetType;
  limit: number;
  percent: number;
  type: "budget_warning";
  used: number;
} | {
  retry: LlmRetrySchedule;
  type: "retrying";
} | {
  injection_bytes: number;
  skills: SkillKey[];
  type: "skills_resolved";
} | {
  reason: SkillResolutionFailureReason;
  skill_key?: SkillKey | null;
  type: "skill_resolution_failed";
} | {
  interaction_id: InteractionId;
  result: string;
  structured_output?: unknown;
  type: "interaction_complete";
} | {
  args: unknown;
  interaction_id: InteractionId;
  pending_tool_calls?: PendingCallbackToolCall[];
  tool_name: string;
  type: "interaction_callback_pending";
} | {
  interaction_id: InteractionId;
  reason: InteractionFailureReason;
  type: "interaction_failed";
} | {
  reason: StreamTruncationReason;
  type: "stream_truncated";
} | {
  payload: ToolConfigChangedPayload;
  type: "tool_config_changed";
} | {
  detail: string;
  display_name: string;
  job_id: string;
  terminal_status: BackgroundJobTerminalStatus;
  type: "background_job_completed";
} | {
  record: TranscriptRewriteRecord;
  session_id: SessionId;
  type: "transcript_rewrite_committed";
} | {
  final_assistant_text?: string | null;
  receipt: TranscriptRewriteAuditReceiptBatch;
  session_id: SessionId;
  type: "transcript_rewrite_audit_receipt_committed";
} | {
  discarded: DiscardedCacheBreakpoint[];
  retained: number;
  session_id: SessionId;
  type: "provider_cache_breakpoints_discarded";
} | {
  kind: CommsNoticeKind;
  peer?: SystemNoticePeer | null;
  request_id?: string | null;
  sender_taint?: SenderContentTaint | null;
  type: "peer_content_ingested";
} | {
  session_id: SessionId;
  type: "turn_usage_accounting_unmeasured";
  unmeasured: UnmeasuredTurnUsageAccounting;
} | {
  dispute: DisputedTurnUsageAccountingIdentity;
  session_id: SessionId;
  type: "turn_usage_accounting_identity_disputed";
};

/**
 * Scope attribution frame for multi-agent streaming.
 */
export type StreamScopeFrame = {
  scope: "primary";
  session_id: string;
} | {
  agent_identity: string;
  flow_run_id: string;
  scope: "mob_member";
};
