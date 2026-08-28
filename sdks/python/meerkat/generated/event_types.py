"""Generated agent-event payload types for the Meerkat Python SDK.

Source: artifacts/schemas/events.json (named ``$defs`` of the event roots)

Every named payload definition in the event schema is emitted here so a
consumer can name the reason enums and payload records the runtime
publishes instead of mirroring them by hand. Coverage is asserted by
``scripts/verify_sdk_event_inventory.py``.
"""

from __future__ import annotations

from typing import Any, Literal, NotRequired, Optional, Required, TypedDict

from .types import (  # noqa: F401
    AssistantImageId,
    BackgroundJobTerminalStatus,
    BlobId,
    CommsNoticeKind,
    ContentBlock,
    ContentInput,
    ExternalToolDeltaPhase,
    PeerId,
    Provider,
    ProviderImageMetadata,
    RevisedPromptDisposition,
    RevisedPromptSource,
    SenderContentTaint,
    SkillName,
    SourceUuid,
    ToolConfigChangeDomain,
    ToolConfigChangeOperation,
    ToolConfigChangeStatus,
    ToolName,
    TranscriptRewriteSelection,
)

Value = Any


AgentErrorClass = Literal['llm', 'store', 'tool', 'policy_indeterminate', 'mcp', 'session_not_found', 'budget', 'max_tokens', 'content_filtered', 'max_turns', 'cancelled', 'invalid_state', 'operation_not_found', 'depth_limit', 'concurrency_limit', 'config', 'internal', 'build', 'auth', 'callback_pending', 'skill', 'structured_output', 'invalid_output_schema', 'hook', 'terminal', 'no_pending_boundary']


# Stable identifier for a configured hook.
HookId = str


# Hook points available in V1.
HookPoint = Literal['run_started', 'run_completed', 'run_failed', 'pre_llm_request', 'post_llm_response', 'pre_tool_execution', 'post_tool_execution', 'turn_boundary']


# Typed reason codes for guardrail denials.
HookReasonCode = Literal['policy_violation', 'safety_violation', 'schema_violation', 'timeout', 'runtime_error']


LlmProviderErrorKind = Literal['invalid_request', 'content_filtered', 'server_error', 'server_overloaded', 'connection_reset', 'unknown', 'stream_parse_error', 'incomplete_response'] | Literal['authorization_route_changed'] | Literal['request_too_large']


LlmProviderErrorRetryability = Literal['retryable', 'non_retryable']


# Closed machine-owned classifier for why a turn reached a terminal failure.
TurnTerminalCauseKind = Literal['unknown', 'hook_denied', 'hook_failure', 'llm_failure', 'tool_failure', 'structured_output_validation_failed', 'budget_exhausted', 'time_budget_exceeded', 'retry_exhausted', 'turn_limit_reached', 'runtime_apply_failure', 'fatal_failure']


# Terminal outcome of a turn.
TurnTerminalOutcome = Literal['none', 'completed', 'failed', 'cancelled', 'budget_exhausted', 'time_budget_exceeded', 'structured_output_validation_failed']


class AgentErrorReasonLlmRateLimited(TypedDict, total=False):
    reason_type: Required[Literal['llm_rate_limited']]
    retry_after_ms: NotRequired[Optional[int]]


class AgentErrorReasonLlmContextExceeded(TypedDict, total=False):
    max: Required[int]
    reason_type: Required[Literal['llm_context_exceeded']]
    requested: Required[int]


class AgentErrorReasonLlmAuthError(TypedDict, total=False):
    reason_type: Required[Literal['llm_auth_error']]


class AgentErrorReasonLlmInvalidModel(TypedDict, total=False):
    model: Required[str]
    reason_type: Required[Literal['llm_invalid_model']]


class AgentErrorReasonLlmProviderError(TypedDict, total=False):
    provider_error: Required[Any]
    provider_error_kind: Required[LlmProviderErrorKind]
    provider_error_retryability: Required[LlmProviderErrorRetryability]
    reason_type: Required[Literal['llm_provider_error']]


class AgentErrorReasonLlmNetworkTimeout(TypedDict, total=False):
    duration_ms: Required[int]
    reason_type: Required[Literal['llm_network_timeout']]


class AgentErrorReasonLlmCallTimeout(TypedDict, total=False):
    duration_ms: Required[int]
    reason_type: Required[Literal['llm_call_timeout']]


class AgentErrorReasonHookDenied(TypedDict, total=False):
    hook_id: NotRequired[Optional[HookId]]
    point: Required[HookPoint]
    reason_code: Required[HookReasonCode]
    reason_type: Required[Literal['hook_denied']]


class AgentErrorReasonHookTimeout(TypedDict, total=False):
    hook_id: Required[HookId]
    reason_type: Required[Literal['hook_timeout']]
    timeout_ms: Required[int]


class AgentErrorReasonHookExecutionFailed(TypedDict, total=False):
    hook_id: Required[HookId]
    reason: Required[str]
    reason_type: Required[Literal['hook_execution_failed']]


class AgentErrorReasonHookConfigInvalid(TypedDict, total=False):
    reason: Required[str]
    reason_type: Required[Literal['hook_config_invalid']]


class AgentErrorReasonStructuredOutputValidationFailed(TypedDict, total=False):
    attempts: Required[int]
    reason: Required[str]
    reason_type: Required[Literal['structured_output_validation_failed']]


class AgentErrorReasonInvalidOutputSchema(TypedDict, total=False):
    reason: Required[str]
    reason_type: Required[Literal['invalid_output_schema']]


class AgentErrorReasonAuthReauthRequired(TypedDict, total=False):
    binding_key: Required[str]
    message: Required[str]
    reason_type: Required[Literal['auth_reauth_required']]


class AgentErrorReasonCallbackPending(TypedDict, total=False):
    args: Required[Any]
    reason_type: Required[Literal['callback_pending']]
    tool_name: Required[str]
    tool_use_id: NotRequired[str]


class AgentErrorReasonTurnTerminalCause(TypedDict, total=False):
    cause_kind: Required[TurnTerminalCauseKind]
    outcome: Required[TurnTerminalOutcome]
    reason_type: Required[Literal['turn_terminal_cause']]


AgentErrorReason = AgentErrorReasonLlmRateLimited | AgentErrorReasonLlmContextExceeded | AgentErrorReasonLlmAuthError | AgentErrorReasonLlmInvalidModel | AgentErrorReasonLlmProviderError | AgentErrorReasonLlmNetworkTimeout | AgentErrorReasonLlmCallTimeout | AgentErrorReasonHookDenied | AgentErrorReasonHookTimeout | AgentErrorReasonHookExecutionFailed | AgentErrorReasonHookConfigInvalid | AgentErrorReasonStructuredOutputValidationFailed | AgentErrorReasonInvalidOutputSchema | AgentErrorReasonAuthReauthRequired | AgentErrorReasonCallbackPending | AgentErrorReasonTurnTerminalCause


class AgentErrorReport(TypedDict, total=False):
    class_: Required[AgentErrorClass]
    message: Required[str]
    reason: NotRequired[Optional[AgentErrorReason]]


class BlobRef(TypedDict, total=False):
    """Durable image reference owned by transcript/runtime state.
    """
    blob_id: Required[BlobId]
    media_type: Required[str]


class GeminiImageMetadata(TypedDict, total=False):
    continuity_ref: NotRequired[Optional[str]]
    response_id: NotRequired[Optional[str]]
    target_model: Required[str]


class OpenAiImageMetadata(TypedDict, total=False):
    image_generation_call_id: NotRequired[Optional[str]]
    response_id: NotRequired[Optional[str]]
    target_model: Required[str]


class PromptText(TypedDict, total=False):
    content: Required[str]


class AssistantImageEvent(TypedDict, total=False):
    """Typed public event payload for a canonical assistant image block appended to history.
    """
    blob_ref: Required[BlobRef]
    height: Required[int]
    image_id: Required[AssistantImageId]
    media_type: Required[str]
    meta: Required[ProviderImageMetadata]
    revised_prompt: Required[RevisedPromptDisposition]
    width: Required[int]


# Type of budget being tracked
BudgetType = Literal['tokens', 'time', 'tool_calls']


# Typed cause of a compaction projection-handoff refusal.
#
# The runtime coordinator refuses for structurally different reasons, and a
# host routing severity (page vs log) needs the cause, not a message
# substring. This is the wire-stable vocabulary carried by
# [`crate::event::CompactionFailureReason::ProjectionHandoffRefused`];
# [`CompactionCommitCoordinationError::refusal`] is its only producer.
CompactionHandoffRefusal = Literal['session_mismatch', 'runtime_epoch_rotated', 'runtime_epoch_retired', 'runtime_binding_rotated', 'runtime_binding_absent', 'durable_projection_unsupported', 'unclassified']


# Whether the history a failed compaction preserved can still be sent.
#
# This is the severity discriminator for compaction-persistence failures: the
# same refusal is a log line on a session that still fits its window and a
# hard wedge on one that does not (every subsequent turn is refused, and the
# only path out is a compaction that persists). Classification reuses the
# pre-dispatch authorities rather than a second limit table:
# [`ContextBudgetState`](crate::ContextBudgetState) over the active model
# profile's window, and the compactor's effective request-byte cap over the
# exact provider-lowered body size.
#
# Both authorities are read at composed-request scope, which is the scope the
# question is asked at: the transcript is not what crosses the provider
# boundary, the request built from it is. A transcript-only token measure
# under-reports every request by the tool schemas that ride it, which is the
# exact shape that used to be reported as [`Self::StillFits`] while every
# turn died on the provider's context limit.
CompactionPreservedHistoryFit = Literal['unclassified', 'still_fits', 'over_window']


class CompactionFailureReasonLlmFailed(TypedDict, total=False):
    """The LLM call summarizing the history failed.
    """
    error_class: Required[AgentErrorClass]
    kind: Required[Literal['llm_failed']]
    message: Required[str]


class CompactionFailureReasonEmptySummary(TypedDict, total=False):
    """The LLM returned an empty summary, so there was nothing to commit.
    """
    kind: Required[Literal['empty_summary']]


class CompactionFailureReasonCuratorFailed(TypedDict, total=False):
    """The host-supplied compaction curator failed to produce a summary, so
    there was nothing to commit (there is no LLM fallback).
    """
    kind: Required[Literal['curator_failed']]
    message: Required[str]


class CompactionFailureReasonEstimationFailed(TypedDict, total=False):
    """Token estimation over the history failed before summarization.
    """
    kind: Required[Literal['estimation_failed']]
    message: Required[str]


class CompactionFailureReasonMemoryIndexingFailed(TypedDict, total=False):
    """The memory store rejected indexing the discarded history, so the
    original (uncompacted) transcript was preserved.
    """
    attempted_entries: Required[int]
    kind: Required[Literal['memory_indexing_failed']]
    message: Required[str]


class CompactionFailureReasonTranscriptRewriteFailed(TypedDict, total=False):
    """Committing the post-summary transcript rewrite failed, so the original
    history was preserved.
    """
    kind: Required[Literal['transcript_rewrite_failed']]
    message: Required[str]


class CompactionFailureReasonProjectionHandoffRefused(TypedDict, total=False):
    """The runtime refused to authorize the durable transcript+memory handoff,
    so the original (uncompacted) history was preserved.

    Distinct from [`Self::MemoryIndexingFailed`]: the memory store never saw
    the batch. The refusal is a runtime-authority fact, and
    `preserved_history` says whether the session can still make progress
    while it persists: a wedged member needs paging, a fitting one needs a
    log line. When no authority could answer that at composed-request
    scope, it says so
    ([`CompactionPreservedHistoryFit::Unclassified`]) rather than guessing.
    """
    attempted_entries: Required[int]
    kind: Required[Literal['projection_handoff_refused']]
    message: Required[str]
    preserved_history: Required[CompactionPreservedHistoryFit]
    refusal: Required[CompactionHandoffRefusal]


# Typed, serializable cause of a non-fatal compaction failure.
#
# Compaction is best-effort: when it fails the agent continues with the
# uncompacted history. This enum is the wire-stable projection of the
# in-flight [`crate::agent::compact::CompactionError`] plus the post-summary
# commit failures (memory indexing, transcript rewrite) surfaced from the
# agent loop. Each variant carries the typed cause; the [`Display`] impl
# renders the human-facing message consumers used to read off the old
# `error: String` field.
#
# [`Display`]: std::fmt::Display
CompactionFailureReason = CompactionFailureReasonLlmFailed | CompactionFailureReasonEmptySummary | CompactionFailureReasonCuratorFailed | CompactionFailureReasonEstimationFailed | CompactionFailureReasonMemoryIndexingFailed | CompactionFailureReasonTranscriptRewriteFailed | CompactionFailureReasonProjectionHandoffRefused


class SkillKey(TypedDict, total=False):
    """Canonical runtime identity for a skill.

    This is the single identity carried across every surface — the wire parses
    directly into this struct, tools receive this struct, the registry stores
    this struct. There is no slash-delimited string path form.
    """
    skill_name: Required[SkillName]
    source_uuid: Required[SourceUuid]


# Provider-native convention used to normalize tokens presented to a model.
PresentedTokenConvention = Literal['anthropic_disjoint_input_components', 'open_ai_input_includes_cached_subset', 'gemini_prompt_includes_cached_subset', 'open_ai_compatible_prompt_includes_cache_details', 'host_declared_inclusive_input_total']


# How the provider adapter obtained the normalized presented-token total.
TokenAggregationProvenance = Literal['sum_disjoint_provider_components', 'provider_inclusive_input_total']


class ProviderTokenAccounting(TypedDict, total=False):
    """One provider adapter's normalized accounting for a single model turn.
    """
    aggregation: Required[TokenAggregationProvenance]
    convention: Required[PresentedTokenConvention]
    model: Required[str]
    presented_tokens: Required[int]
    provider: Required[Provider]


class Usage(TypedDict, total=False):
    """Token usage statistics.

    # The same field names carry two different denominators

    This shape is reused for two semantically different accounts, and
    `input_tokens` does not mean the same thing in both:

    - **Per-call** ([`TurnUsage`], carried by `turn_completed.usage`): raw
      provider counters for exactly one provider request. For Anthropic
      `input_tokens` is the *uncached* input only, with cache-write and
      cache-read input reported separately in `cache_creation_tokens` and
      `cache_read_tokens`. The normalized presented-input total for that one
      call is [`TurnUsage::presented_tokens`].
    - **Cumulative** ([`CumulativeUsage`], carried by `run_completed.usage`): a
      running total over every provider call recorded on the *session*, not on
      one run. Its `input_tokens` is the saturating sum of each call's
      *presented* tokens (see [`CumulativeUsage::add_turn`]), and its cache
      detail fields are always `None` because their relationship to the input
      total is provider-specific.

    # What consumers must not sum

    - Never sum `run_completed.usage` with per-call usage, and never sum
      `run_completed.usage` across runs: each value is already the whole
      session's total to date, so adding two of them double-counts everything
      before the later one. Take the latest value.
    - Never sum per-call `input_tokens` and expect it to match
      `run_completed.usage.input_tokens`. On a cache-heavy Anthropic session the
      two use different denominators and will not agree. Sum
      `turn_completed.usage.accounting.presented_tokens` (that is,
      [`TurnUsage::presented_tokens`]) instead, which is exactly what
      [`CumulativeUsage::add_turn`] does.
    - Do not expect the per-call rows to reconcile with the cumulative account
      either. Intermediate tool-loop calls, the structured-output extraction
      call, and the compaction summary call are all charged to the cumulative
      account and publish no `turn_completed` row, so the rows cover a strict
      subset of the tokens.

    The worked example lives in `docs/reference/usage-accounting.mdx`. Its
    numbers are pinned against the agent loop by
    `turn_rows_cover_one_call_while_the_run_total_is_session_cumulative`
    (`meerkat-core/src/agent/usage_accounting_tests.rs`) and against this type's
    arithmetic by `cumulative_usage_matches_documented_aggregation_example`.
    """
    cache_creation_tokens: NotRequired[Optional[int]]
    cache_read_tokens: NotRequired[Optional[int]]
    input_tokens: Required[int]
    output_tokens: Required[int]
    provider_accounting: NotRequired[Optional[ProviderTokenAccounting]]


# Cumulative usage across committed turns. Cache detail counters are not
# aggregated because their relationship to input totals is provider-specific.
#
# This value is **already a total**, and the total is session-scoped: on the
# event stream it is `Session::total_usage()`, which is persisted with the
# session and restored on resume, so `run_completed.usage` on the second run of
# a session already contains the first run's calls. Adding it to per-call
# usage, or summing the values observed on two runs, double-counts. It also
# intentionally carries no [`crate::ProviderTokenAccounting`], because one
# session may span providers and models and so cannot truthfully claim a
# single per-call convention; per-model attribution is read from the per-call
# `turn_completed` rows, which cover only the calls that closed a run.
CumulativeUsage = Usage


# Which side of a turn commit produced the discarded proof.
#
# Load-bearing for hosts: `PersistedEvidence` is inherited poison from an
# earlier turn, `AuthoredThisTurn` is this turn's own lowering disagreeing
# with the committed transcript. The two have different root causes and are
# otherwise indistinguishable from the outside.
CacheBreakpointDiscardOrigin = Literal['authored_this_turn', 'persisted_evidence']


class CacheBreakpointDiscardReasonBoundaryOutsideCommittedTranscript(TypedDict, total=False):
    """The anchored boundary is past the end of the committed transcript.
    """
    kind: Required[Literal['boundary_outside_committed_transcript']]
    message_count: Required[int]
    message_len: Required[int]


class CacheBreakpointDiscardReasonCanonicalPrefixMoved(TypedDict, total=False):
    """The committed transcript prefix at the anchored boundary is no longer
    the one the provider lowered.
    """
    kind: Required[Literal['canonical_prefix_moved']]


class CacheBreakpointDiscardReasonEvidenceUnusable(TypedDict, total=False):
    """The proof itself is not usable evidence, independently of any
    transcript: a malformed rendered identity, an incoherent
    provider/encoding pairing, or an undecodable persisted row.
    """
    detail: Required[str]
    kind: Required[Literal['evidence_unusable']]


class CacheBreakpointDiscardReasonProjectedBoundaryUnmappable(TypedDict, total=False):
    """Durable history retains superseded prompt versions, so the projected
    provider boundary has no raw-to-projected map to rebind through.
    """
    kind: Required[Literal['projected_boundary_unmappable']]


# Why one provider-authored cache breakpoint was discarded instead of
# retained as durable session evidence.
#
# A cache breakpoint is an optimization artifact anchored to the exact
# transcript head a provider lowered. Ordinary transcript motion - a
# synthetic-notice refresh, a compaction, a re-materialized prefix - moves
# that anchor without saying anything about the transcript's own integrity.
# Every variant here therefore describes the ARTIFACT. Faults that describe
# the committed TRANSCRIPT stay [`CacheBreakpointEvidenceError`].
CacheBreakpointDiscardReason = CacheBreakpointDiscardReasonBoundaryOutsideCommittedTranscript | CacheBreakpointDiscardReasonCanonicalPrefixMoved | CacheBreakpointDiscardReasonEvidenceUnusable | CacheBreakpointDiscardReasonProjectedBoundaryUnmappable


class CacheBreakpointBoundarySystemProfilePrefix(TypedDict, total=False):
    """Stable leading system/profile prefix. `message_count` is the exclusive
    canonical transcript boundary.
    """
    kind: Required[Literal['system_profile_prefix']]
    message_count: Required[int]


class CacheBreakpointBoundaryTranscriptAfter(TypedDict, total=False):
    """Explicit breakpoint after one canonical transcript message.
    """
    kind: Required[Literal['transcript_after']]
    message_count: Required[int]


# Stable provider-independent identity of an authored cache breakpoint.
CacheBreakpointBoundary = CacheBreakpointBoundarySystemProfilePrefix | CacheBreakpointBoundaryTranscriptAfter


class DiscardedCacheBreakpointIdentity(TypedDict, total=False):
    """Identity of a discarded proof, when the proof itself could be decoded.
    """
    boundary: Required[CacheBreakpointBoundary]
    model: Required[str]
    provider: Required[Provider]


class DiscardedCacheBreakpoint(TypedDict, total=False):
    """One provider-authored cache proof that could not be retained.
    """
    identity: NotRequired[Optional[DiscardedCacheBreakpointIdentity]]
    origin: Required[CacheBreakpointDiscardOrigin]
    reason: Required[CacheBreakpointDiscardReason]


class DisputedTurnUsageAccountingIdentity(TypedDict, total=False):
    """One turn's provider-authored accounting named a different provider/model
    than the request it answered.

    # Why this is not the same fault as absent accounting

    A mismatched identity still arrives with a complete, internally consistent
    measurement: the [`PresentedTokenConvention`] travels with the number, so
    the counters mean what they say regardless of which name is attached. The
    disputed fact is attribution alone, so the token axis still advances on the
    number the provider actually sent. Absent accounting has no number at all
    and therefore cannot advance anything. Collapsing the two would either kill
    correct work over a name or fabricate counters over silence.

    # Why the reported identity is preserved verbatim

    Rewriting `reported_*` to the active identity would publish an agreement
    that was never observed - a guess laundered as evidence. Both sides are
    carried so a host can see exactly who disagreed with whom.
    """
    active_model: Required[str]
    active_provider: Required[Provider]
    reported_model: Required[str]
    reported_provider: Required[Provider]


class HookFailureReasonTimeout(TypedDict, total=False):
    """The hook runtime did not complete within its configured timeout.
    """
    reason_code: Required[Literal['timeout']]
    timeout_ms: Required[int]


class HookFailureReasonExecutionFailed(TypedDict, total=False):
    """The hook runtime executed but failed.
    """
    message: Required[str]
    reason_code: Required[Literal['execution_failed']]


class HookFailureReasonConfigInvalid(TypedDict, total=False):
    """The hook configuration was rejected.
    """
    message: Required[str]
    reason_code: Required[Literal['config_invalid']]


class HookFailureReasonObserveOnlyViolation(TypedDict, total=False):
    """A background hook attempted a non-observe action, which is not
    permitted for observe-only background hooks at any hook point.
    """
    reason_code: Required[Literal['observe_only_violation']]


# Typed reason a hook execution failed (engine-level fault, not a guardrail
# denial).
#
# Mirrors the [`HookReasonCode`] precedent: the variant is the typed owner of
# the failure cause; the human-readable string is a [`Display`] derivation,
# never a separately-stored field.
#
# [`Display`]: std::fmt::Display
HookFailureReason = HookFailureReasonTimeout | HookFailureReasonExecutionFailed | HookFailureReasonConfigInvalid | HookFailureReasonObserveOnlyViolation


# Typed reason an interaction stream was abandoned before normal terminal
# delivery. This is intentionally distinct from [`InteractionStreamState::Expired`]:
# expiry proves that the attach TTL elapsed while the stream was still
# reserved, whereas abandonment records an observed failure.
InteractionStreamAbandonReason = Literal['send_failed', 'admission_rejected', 'response_rejected', 'terminal_delivery_failed']


class InteractionFailureReasonCancelled(TypedDict, total=False):
    """The interaction was cancelled before completing.
    """
    kind: Required[Literal['cancelled']]


class InteractionFailureReasonAbandoned(TypedDict, total=False):
    """The interaction was abandoned (runtime terminated, dropped, or abandoned
    with an error) before completing.
    """
    detail: Required[str]
    kind: Required[Literal['abandoned']]


class InteractionFailureReasonInteractionStreamAbandoned(TypedDict, total=False):
    """The machine-owned interaction stream terminal rejected or could not
    deliver the peer response. Unlike generic runtime abandonment, the
    typed reason is preserved for stream consumers.
    """
    kind: Required[Literal['interaction_stream_abandoned']]
    reason: Required[InteractionStreamAbandonReason]


class InteractionFailureReasonFinalizationFailed(TypedDict, total=False):
    """The interaction's main run completed but turn finalization failed.
    """
    detail: Required[str]
    kind: Required[Literal['finalization_failed']]


class InteractionFailureReasonExtractionFailed(TypedDict, total=False):
    """The main interaction run completed, but structured-output extraction
    failed. Keeping the extraction facts typed lets journal consumers
    preserve the extraction terminal class even though the exact
    per-input carrier is an `InteractionFailed` event.
    """
    attempts: Required[int]
    kind: Required[Literal['extraction_failed']]
    last_output: Required[str]
    reason: Required[str]


# Typed reason an interaction-scoped run failed (terminal event for tap
# subscribers).
#
# Mirrors [`CompactionFailureReason`]: the variant is the typed owner of the
# failure cause; the human-readable string is a [`Display`] derivation,
# never a separately-carried field.
#
# [`Display`]: std::fmt::Display
InteractionFailureReason = InteractionFailureReasonCancelled | InteractionFailureReasonAbandoned | InteractionFailureReasonInteractionStreamAbandoned | InteractionFailureReasonFinalizationFailed | InteractionFailureReasonExtractionFailed


# Unique identifier for an interaction.
InteractionId = str


# Closed classifier for recoverable LLM failures.
LlmRetryFailureKind = Literal['rate_limited', 'network_timeout', 'call_timeout', 'retryable_provider_error']


class LlmRetryFailure(TypedDict, total=False):
    """Typed recoverable LLM failure carried through retry authority.
    """
    duration_ms: NotRequired[Optional[int]]
    kind: Required[LlmRetryFailureKind]
    message: Required[str]
    provider: Required[str]
    retry_after_ms: NotRequired[Optional[int]]


class LlmRetryPlan(TypedDict, total=False):
    """Typed retry delay plan selected for a recoverable LLM failure.
    """
    attempt: Required[int]
    budget_capped: Required[bool]
    computed_delay_ms: Required[int]
    max_retries: Required[int]
    rate_limit_floor_applied: Required[bool]
    retry_after_hint_ms: NotRequired[Optional[int]]
    selected_delay_ms: Required[int]


class LlmRetrySchedule(TypedDict, total=False):
    """Recoverable LLM retry lifecycle payload accepted by turn authority.
    """
    failure: Required[LlmRetryFailure]
    plan: Required[LlmRetryPlan]


class PendingCallbackToolCall(TypedDict, total=False):
    """One externally routed callback tool call inside a suspended assistant
    tool-use batch.
    """
    args: Required[Any]
    tool_name: Required[str]
    tool_use_id: Required[str]


class RunInputContent(TypedDict, total=False):
    """The run starts from caller-provided content (text or blocks).
    """
    content: Required[ContentInput]
    kind: Required[Literal['content']]


class RunInputPendingToolResults(TypedDict, total=False):
    """The run resumes a pending continuation whose transcript tail is a
    staged tool-results message; there is no caller prompt.
    """
    kind: Required[Literal['pending_tool_results']]


# Typed input fact for a run boundary.
#
# A run either starts from caller-provided content or resumes from tool
# results already staged at the session's pending-continuation boundary.
# The pending-tail case is its own typed variant — run-boundary events and
# hooks never fabricate an empty-string prompt to stand in for it.
RunInput = RunInputContent | RunInputPendingToolResults


class SchemaWarning(TypedDict, total=False):
    """Warnings emitted during schema lowering.
    """
    message: Required[str]
    path: Required[str]
    provider: Required[Provider]


class ServerToolKindWebSearch(TypedDict, total=False):
    """Provider-hosted web search (Anthropic `web_search*`, OpenAI
    `web_search*`). Sub-event detail lives in the evidence `content`.
    """
    kind: Required[Literal['web_search']]


class ServerToolKindGoogleSearch(TypedDict, total=False):
    """Gemini grounding via Google Search (grounding metadata block).
    """
    kind: Required[Literal['google_search']]


class ServerToolKindProviderNative(TypedDict, total=False):
    """A provider-native server tool whose exact name must round-trip verbatim
    (Anthropic `server_tool_use` dynamic name and any unrecognized future
    server tool). The `name` is provider-owned and replayed unchanged.
    """
    kind: Required[Literal['provider_native']]
    name: Required[str]


# Typed semantic kind of a provider-executed (server-side) tool.
#
# This is the single typed owner of the server-tool identity. Each provider
# adapter parses its native discriminator into this enum once at the streaming
# boundary, so downstream consumers never re-classify by matching a
# `name: String`. Provider-native sub-event detail (e.g. OpenAI's
# `web_search_call` vs `web_search_result`) is preserved in the accompanying
# `content` JSON, not in this kind.
#
# `ProviderNative` is the verbatim escape hatch for dynamic provider tool
# names (Anthropic `server_tool_use` carries an arbitrary tool name that must
# round-trip exactly on replay). It is the only variant carrying a string,
# and that string IS the typed fact — not a re-derivable label.
ServerToolKind = ServerToolKindWebSearch | ServerToolKindGoogleSearch | ServerToolKindProviderNative


# Unique identifier for a session (UUID v7 for time-ordering)
SessionId = str


# Slug-validated capability identifier for skill requirements.
#
# Replaces the legacy `Vec<String>` capability lists with a typed
# namespace. Parsed at construction, so callers cannot smuggle invalid
# identifiers into descriptors or requirements.
CapabilityId = str


class SkillResolutionFailureReasonNotFound(TypedDict, total=False):
    key: Required[SkillKey]
    reason_type: Required[Literal['not_found']]


class SkillResolutionFailureReasonCapabilityUnavailable(TypedDict, total=False):
    capability: Required[CapabilityId]
    key: Required[SkillKey]
    reason_type: Required[Literal['capability_unavailable']]


class SkillResolutionFailureReasonLoad(TypedDict, total=False):
    message: Required[str]
    reason_type: Required[Literal['load']]


class SkillResolutionFailureReasonParse(TypedDict, total=False):
    message: Required[str]
    reason_type: Required[Literal['parse']]


class SkillResolutionFailureReasonSourceUuidCollision(TypedDict, total=False):
    existing_fingerprint: Required[str]
    new_fingerprint: Required[str]
    reason_type: Required[Literal['source_uuid_collision']]
    source_uuid: Required[str]


class SkillResolutionFailureReasonSourceUuidMutationWithoutLineage(TypedDict, total=False):
    existing_source_uuid: Required[str]
    fingerprint: Required[str]
    mutated_source_uuid: Required[str]
    reason_type: Required[Literal['source_uuid_mutation_without_lineage']]


class SkillResolutionFailureReasonMissingSkillRemaps(TypedDict, total=False):
    event_id: Required[str]
    event_kind: Required[str]
    reason_type: Required[Literal['missing_skill_remaps']]


class SkillResolutionFailureReasonRemapWithoutLineage(TypedDict, total=False):
    from_skill_name: Required[str]
    from_source_uuid: Required[str]
    reason_type: Required[Literal['remap_without_lineage']]
    to_skill_name: Required[str]
    to_source_uuid: Required[str]


class SkillResolutionFailureReasonUnknownSkillAlias(TypedDict, total=False):
    alias: Required[str]
    reason_type: Required[Literal['unknown_skill_alias']]


class SkillResolutionFailureReasonRemapCycle(TypedDict, total=False):
    reason_type: Required[Literal['remap_cycle']]
    skill_name: Required[str]
    source_uuid: Required[str]


class SkillResolutionFailureReasonUnknown(TypedDict, total=False):
    message: Required[str]
    reason_type: Required[Literal['unknown']]


SkillResolutionFailureReason = SkillResolutionFailureReasonNotFound | SkillResolutionFailureReasonCapabilityUnavailable | SkillResolutionFailureReasonLoad | SkillResolutionFailureReasonParse | SkillResolutionFailureReasonSourceUuidCollision | SkillResolutionFailureReasonSourceUuidMutationWithoutLineage | SkillResolutionFailureReasonMissingSkillRemaps | SkillResolutionFailureReasonRemapWithoutLineage | SkillResolutionFailureReasonUnknownSkillAlias | SkillResolutionFailureReasonRemapCycle | SkillResolutionFailureReasonUnknown


# Why the model stopped generating
StopReason = Literal['end_turn', 'tool_use', 'max_tokens', 'stop_sequence', 'content_filter', 'cancelled']


class StreamTruncationReasonChannelFull(TypedDict, total=False):
    """The per-interaction tap channel was full; streaming frames dropped.
    """
    kind: Required[Literal['channel_full']]


class StreamTruncationReasonStreamLagged(TypedDict, total=False):
    """A broadcast event stream lagged and skipped events.
    """
    dropped: Required[int]
    kind: Required[Literal['stream_lagged']]


class StreamTruncationReasonOutputAudioDegraded(TypedDict, total=False):
    """A live transport failed to deliver queued output-audio packets
    (e.g. WebRTC RTP pacing-queue backpressure). K16: delivery
    degradation is a typed, session-observable fact — never a
    transport-local counter.
    """
    dropped: Required[int]
    kind: Required[Literal['output_audio_degraded']]


class StreamTruncationReasonRemoteCursorOverrun(TypedDict, total=False):
    """A remote member's retained event window advanced beyond the
    controlling-side durable cursor. The pump resumes immediately after
    `watermark`; subscribers receive this marker before later events.
    """
    kind: Required[Literal['remote_cursor_overrun']]
    watermark: Required[int]


class StreamTruncationReasonOversizedRemoteEvent(TypedDict, total=False):
    """One immutable durable event row could not cross the bounded member
    bridge reply. The pump skips exactly this row and resumes at the next
    durable sequence; the omission is visible rather than log-only.
    """
    durable_seq: Required[int]
    encoded_bytes: Required[int]
    kind: Required[Literal['oversized_remote_event']]
    max_bytes: Required[int]


# Typed reason a best-effort event stream dropped frames.
#
# The variant is the typed owner of the truncation cause; the human-readable
# string is a [`Display`](std::fmt::Display) derivation, never a
# separately-carried field. `StreamTruncated` is a UI hint — the terminal
# event remains authoritative.
StreamTruncationReason = StreamTruncationReasonChannelFull | StreamTruncationReasonStreamLagged | StreamTruncationReasonOutputAudioDegraded | StreamTruncationReasonRemoteCursorOverrun | StreamTruncationReasonOversizedRemoteEvent


class SystemNoticePeer(TypedDict, total=False):
    """Peer identity carried in a typed comms transcript block.

    `id` is the canonical routing identity ([`crate::comms::PeerId`]), serialized
    as a hyphenated UUID string on the wire; `display_name` is the presentation
    label. Keeping `id` typed lets the projection logic consume the identity
    directly instead of re-parsing a `String` back into a `PeerId`.
    """
    display_name: NotRequired[Optional[str]]
    id: Required[PeerId]


ToolCallArguments = dict[str, Any]


class DeferredCatalogDelta(TypedDict, total=False):
    """Additive hidden-catalog delta metadata for runtime notices.
    """
    added_hidden_names: NotRequired[list[ToolName]]
    pending_sources: NotRequired[list[str]]
    removed_hidden_names: NotRequired[list[ToolName]]


class ToolConfigChangedPayload(TypedDict, total=False):
    """Payload for tool configuration change notifications.

    The typed `status_info` is the sole owner of the change status; display
    text is derived at read time via [`ToolConfigChangedPayload::status_text`].
    """
    applied_at_turn: NotRequired[Optional[int]]
    deferred_catalog_delta: NotRequired[Optional[DeferredCatalogDelta]]
    domain: NotRequired[Optional[ToolConfigChangeDomain]]
    operation: Required[ToolConfigChangeOperation]
    persisted: Required[bool]
    status_info: Required[ToolConfigChangeStatus]
    target: Required[str]


class SystemTime(TypedDict, total=False):
    nanos_since_epoch: Required[int]
    secs_since_epoch: Required[int]


class TranscriptRewriteReason(TypedDict, total=False):
    """Audit annotation carried with a transcript rewrite commit.

    The free-form kind is for review, debugging, and provenance only. It never
    classifies a rewrite as compaction; [`TranscriptRewriteSelection`] owns that
    semantic through its opaque typed compaction range.
    """
    kind: Required[str]
    note: NotRequired[Optional[str]]


class CompactionRewriteRange(TypedDict, total=False):
    """Opaque range carried by the typed compaction rewrite semantic.
    """
    end: Required[int]
    start: Required[int]


class TranscriptEditRewriteRange(TypedDict, total=False):
    """Opaque current-format range carried by an ordinary transcript edit.
    """
    end: Required[int]
    start: Required[int]


class TranscriptRewriteCommit(TypedDict, total=False):
    """Immutable rewrite commit that advances a session transcript head.
    """
    actor: NotRequired[Optional[str]]
    committed_at: Required[SystemTime]
    messages_after: Required[int]
    messages_before: Required[int]
    original_span_digest: Required[str]
    parent_revision: Required[str]
    reason: Required[TranscriptRewriteReason]
    replacement_digest: Required[str]
    revision: Required[str]
    rewrite_generation: NotRequired[int]
    selection: Required[TranscriptRewriteSelection]


class TranscriptRewritePrefixAccumulator(TypedDict, total=False):
    """Rolling, canonical identity of an ordered exact rewrite-commit prefix.

    This is a semantic graph fact, not a replay-cursor assertion. It is carried
    by the graph, folded into checkpoint authority, and independently matched
    against the EventStore's receipt. One ordinary lineage-tail commit extends
    the accumulator with one commit serialization; it never re-hashes the
    accumulated prefix.
    """
    digest: Required[str]
    occurrence_count: Required[int]


class TranscriptRewriteAuditReceiptBatch(TypedDict, total=False):
    """Receipt for one non-empty ordered transcript-rewrite suffix.

    The transition is self-verifying:
    `start_prefix.extend(commits) == end_prefix`. Occurrence generations are
    checked by [`TranscriptRewritePrefixAccumulator::extend`], so neither a gap
    nor a duplicate can be hidden inside one receipt.
    """
    commits: Required[list[TranscriptRewriteCommit]]
    end_prefix: Required[TranscriptRewritePrefixAccumulator]
    start_prefix: Required[TranscriptRewritePrefixAccumulator]


class TranscriptRevisionBody(TypedDict, total=False):
    """Immutable transcript revision body retained by the session-local graph.
    """
    created_at: Required[SystemTime]
    messages: Required[list[Any]]
    parent_revision: NotRequired[Optional[str]]
    revision: Required[str]


class TranscriptRewriteRecord(TypedDict, total=False):
    """Self-contained append-only transcript rewrite record.
    """
    commit: Required[TranscriptRewriteCommit]
    digest_format: NotRequired[int]
    parent_body: Required[TranscriptRevisionBody]
    revision_body: Required[TranscriptRevisionBody]


class TurnUsage(TypedDict, total=False):
    """Usage for one provider turn.

    This is deliberately distinct from cumulative [`Usage`] on event
    boundaries, while retaining the existing flat wire shape.

    # Model and provider attribution

    `accounting` is the single owner of per-call attribution: it carries the
    [`crate::Provider`] and the model string of the request that produced these
    counters, minted by the provider adapter from the lowered request rather than
    from configuration intent. A consumer reading only the event stream can
    therefore attribute one `turn_completed` row to a model without joining
    against session metadata:
    `turn_completed.usage.accounting.provider` /
    `turn_completed.usage.accounting.model`.

    There is deliberately no second copy of the model or provider beside
    `accounting` on the event: attribution has one owner.
    """
    accounting: Required[ProviderTokenAccounting]
    cache_creation_tokens: NotRequired[Optional[int]]
    cache_read_tokens: NotRequired[Optional[int]]
    input_tokens: Required[int]
    output_tokens: Required[int]
    provider_accounting: NotRequired[Optional[ProviderTokenAccounting]]


class UnmeasuredTurnUsageAccounting(TypedDict, total=False):
    """Provider token accounting for one model turn was absent.

    # What this makes untrue, and what it does not

    A provider that streams a complete answer and no usage event has stated
    nothing about tokens. The turn's SEMANTIC facts - what the model said,
    which tools it asked for, what may be committed to the transcript - are
    untouched by that silence, so this absence terminalizes none of them. Only
    the accounting axis is affected, and it is affected by being left exactly
    where it was: no counter advances, no per-call row is published, and no
    value is substituted for the one the provider did not send.

    This value states the absence and nothing more. It does not assert that the
    turn completed: the turn's own terminal fact has its own owner, and a turn
    whose accounting went missing can still fail afterwards on unrelated
    grounds.

    # Why the identity here is not accounting

    `provider` and `model` name the REQUEST this turn was lowered for. They are
    the address of the missing measurement, not a reconstruction of it, and
    carry no counters. This is exactly the line
    [`ProviderTokenAccounting::host_declared`] would cross: it would mint
    provider attribution for numbers no provider issued.
    """
    model: Required[str]
    provider: Required[Provider]


class AgentEventRunStarted(TypedDict, total=False):
    """Agent run started
    """
    input: Required[RunInput]
    session_id: Required[SessionId]
    type: Required[Literal['run_started']]


class AgentEventRunCompleted(TypedDict, total=False):
    """Agent run completed successfully
    """
    extraction_required: NotRequired[bool]
    result: Required[str]
    session_id: Required[SessionId]
    structured_output: NotRequired[Any]
    terminal_cause_kind: NotRequired[Optional[TurnTerminalCauseKind]]
    type: Required[Literal['run_completed']]
    usage: Required[CumulativeUsage]


class AgentEventExtractionSucceeded(TypedDict, total=False):
    """Structured-output extraction succeeded after a completed main run.
    """
    schema_warnings: NotRequired[Optional[list[SchemaWarning]]]
    session_id: Required[SessionId]
    structured_output: Required[Any]
    type: Required[Literal['extraction_succeeded']]


class AgentEventExtractionFailed(TypedDict, total=False):
    """Structured-output extraction failed after a completed main run.
    """
    attempts: Required[int]
    last_output: Required[str]
    reason: Required[str]
    session_id: Required[SessionId]
    type: Required[Literal['extraction_failed']]


class AgentEventRunFailed(TypedDict, total=False):
    """Agent run failed
    """
    error_report: Required[AgentErrorReport]
    session_id: Required[SessionId]
    terminal_cause_kind: NotRequired[Optional[TurnTerminalCauseKind]]
    type: Required[Literal['run_failed']]


class AgentEventHookStarted(TypedDict, total=False):
    """Hook invocation started.
    """
    hook_id: Required[HookId]
    point: Required[HookPoint]
    type: Required[Literal['hook_started']]


class AgentEventHookCompleted(TypedDict, total=False):
    """Hook invocation completed.
    """
    duration_ms: Required[int]
    hook_id: Required[HookId]
    point: Required[HookPoint]
    type: Required[Literal['hook_completed']]


class AgentEventHookFailed(TypedDict, total=False):
    """Hook invocation failed.
    """
    hook_id: Required[HookId]
    point: Required[HookPoint]
    reason: Required[HookFailureReason]
    type: Required[Literal['hook_failed']]


class AgentEventHookDenied(TypedDict, total=False):
    """Hook denied an action.
    """
    hook_id: Required[HookId]
    message: Required[str]
    payload: NotRequired[Any]
    point: Required[HookPoint]
    reason_code: Required[HookReasonCode]
    type: Required[Literal['hook_denied']]


class AgentEventTurnStarted(TypedDict, total=False):
    """New turn started (calling LLM)
    """
    turn_number: Required[int]
    type: Required[Literal['turn_started']]


class AgentEventReasoningDelta(TypedDict, total=False):
    """Streaming reasoning/thinking from the model
    """
    delta: Required[str]
    type: Required[Literal['reasoning_delta']]


class AgentEventReasoningComplete(TypedDict, total=False):
    """Reasoning/thinking complete for this block
    """
    content: Required[str]
    type: Required[Literal['reasoning_complete']]


class AgentEventTextDelta(TypedDict, total=False):
    """Streaming text from the model
    """
    delta: Required[str]
    type: Required[Literal['text_delta']]


class AgentEventTextComplete(TypedDict, total=False):
    """Text generation complete for this turn
    """
    content: Required[str]
    type: Required[Literal['text_complete']]


class AgentEventServerToolContent(TypedDict, total=False):
    """Provider-executed tool content surfaced during a model turn.
    """
    content: Required[Any]
    id: NotRequired[Optional[str]]
    kind: Required[ServerToolKind]
    type: Required[Literal['server_tool_content']]


class AgentEventAssistantImageAppended(TypedDict, total=False):
    """Canonical assistant image block appended to transcript history.
    """
    image: Required[AssistantImageEvent]
    type: Required[Literal['assistant_image_appended']]


class AgentEventToolCallRequested(TypedDict, total=False):
    """Model requested a tool call
    """
    args: Required[ToolCallArguments]
    id: Required[str]
    name: Required[str]
    type: Required[Literal['tool_call_requested']]


class AgentEventToolResultReceived(TypedDict, total=False):
    """Tool result received (injected into conversation).

    The conversation-level fact for a tool call: this result entered the
    transcript. [`AgentEvent::ToolExecutionCompleted`] is the paired
    execution-level fact for the same `id` and currently carries a full
    copy of these same blocks - see that variant before persisting both, or
    a durable consumer stores every result body twice.
    """
    content: Required[list[ContentBlock]]
    id: Required[str]
    is_error: Required[bool]
    name: Required[str]
    type: Required[Literal['tool_result_received']]


class AgentEventTurnCompleted(TypedDict, total=False):
    """Turn completed.

    # Why `usage` is optional

    This event states one semantic fact - a model turn reached its terminal
    and its assistant message is committed - and carries one accounting
    fact beside it. The two have different owners and different failure
    modes: a provider stream that ends without ever sending a usage event
    has said nothing about tokens while having said everything about the
    answer. Absence is therefore representable here, because the only
    alternatives are to fabricate counters no provider issued or to
    suppress the completion of a turn the caller has already read.

    `usage: None` means exactly "no accounting exists for this turn": no
    counter advanced, and no per-call row should be reconciled for it. The
    paired [`AgentEvent::TurnUsageAccountingUnmeasured`] carries the
    explanation (which provider and model went unaccounted); this field
    owns only the number's presence or absence. Consumers must skip an
    absent row, never treat it as zero.
    """
    stop_reason: Required[StopReason]
    type: Required[Literal['turn_completed']]
    usage: NotRequired[Optional[TurnUsage]]


class AgentEventToolExecutionStarted(TypedDict, total=False):
    """Starting tool execution
    """
    id: Required[str]
    name: Required[str]
    type: Required[Literal['tool_execution_started']]


class AgentEventToolExecutionCompleted(TypedDict, total=False):
    """Tool execution completed.

    # This carries a SECOND COPY of the result body

    This event and [`AgentEvent::ToolResultReceived`] are both emitted for
    every tool call, and they differ by exactly one field: `duration_ms`.
    Both carry the full `content` blocks. The two facts are genuinely
    distinct and both deserve to exist - `ToolResultReceived` is the
    CONVERSATION fact (this result was injected into the transcript),
    this is the EXECUTION fact (the call finished, and here is how long it
    took) - but the cost of the execution fact currently scales with the
    size of the result rather than with the fact itself.

    A consumer that durably persists BOTH events therefore stores every
    tool result body twice. This is not hypothetical: it was measured
    independently on two adopter fleets with different storage engines and
    different payload shapes - a console frame store (~348 MB against
    ~348 MB, pairwise-identical maxima, from 153 camera-tool calls) and a
    warehouse events table (89,033 rows / 3.35 GB against 88,934 rows /
    1.69 GB). Neither had configured it; both inherited it from this
    vocabulary. Note it is a PER-CALL cost, not a scale problem: one tool
    returning a large blob is enough.

    `id` is present on both events and is already the join key. A consumer
    persisting both should store the body once against `id` and join for
    the execution fact, rather than capping bytes at its own writer - a cap
    applied downstream still writes the body twice, only smaller, and every
    consumer would have to reimplement it.
    """
    content: Required[list[ContentBlock]]
    duration_ms: Required[int]
    id: Required[str]
    is_error: Required[bool]
    name: Required[str]
    type: Required[Literal['tool_execution_completed']]


class AgentEventToolExecutionTimedOut(TypedDict, total=False):
    """Tool execution timed out
    """
    id: Required[str]
    name: Required[str]
    timeout_ms: Required[int]
    type: Required[Literal['tool_execution_timed_out']]


class AgentEventCompactionStarted(TypedDict, total=False):
    """Context compaction started.
    """
    estimated_history_tokens: Required[int]
    input_tokens: Required[int]
    message_count: Required[int]
    type: Required[Literal['compaction_started']]


class AgentEventCompactionCompleted(TypedDict, total=False):
    """Context compaction completed successfully.
    """
    messages_after: Required[int]
    messages_before: Required[int]
    summary_tokens: Required[int]
    type: Required[Literal['compaction_completed']]


class AgentEventCompactionFailed(TypedDict, total=False):
    """Context compaction failed (non-fatal — agent continues with uncompacted history).
    """
    reason: Required[CompactionFailureReason]
    type: Required[Literal['compaction_failed']]


class AgentEventBudgetWarning(TypedDict, total=False):
    """Budget warning (approaching limits)
    """
    budget_type: Required[BudgetType]
    limit: Required[int]
    percent: Required[float]
    type: Required[Literal['budget_warning']]
    used: Required[int]


class AgentEventRetrying(TypedDict, total=False):
    """Retrying after a recoverable LLM failure.

    The typed schedule is the single owner of the retry facts (failure
    kind/provider/diagnostic and plan attempt/delay); display strings are
    derived from it, never carried beside it.
    """
    retry: Required[LlmRetrySchedule]
    type: Required[Literal['retrying']]


class AgentEventSkillsResolved(TypedDict, total=False):
    """Skills resolved for this turn.
    """
    injection_bytes: Required[int]
    skills: Required[list[SkillKey]]
    type: Required[Literal['skills_resolved']]


class AgentEventSkillResolutionFailed(TypedDict, total=False):
    """A skill reference could not be resolved.
    """
    reason: Required[SkillResolutionFailureReason]
    skill_key: NotRequired[Optional[SkillKey]]
    type: Required[Literal['skill_resolution_failed']]


class AgentEventInteractionComplete(TypedDict, total=False):
    """An interaction completed successfully (terminal event for tap subscribers).
    """
    interaction_id: Required[InteractionId]
    result: Required[str]
    structured_output: NotRequired[Any]
    type: Required[Literal['interaction_complete']]


class AgentEventInteractionCallbackPending(TypedDict, total=False):
    """An interaction reached an external callback boundary and is waiting for
    tool results before the session can continue.
    """
    args: Required[Any]
    interaction_id: Required[InteractionId]
    pending_tool_calls: NotRequired[list[PendingCallbackToolCall]]
    tool_name: Required[str]
    type: Required[Literal['interaction_callback_pending']]


class AgentEventInteractionFailed(TypedDict, total=False):
    """An interaction failed (terminal event for tap subscribers).
    """
    interaction_id: Required[InteractionId]
    reason: Required[InteractionFailureReason]
    type: Required[Literal['interaction_failed']]


class AgentEventStreamTruncated(TypedDict, total=False):
    """Some streaming events were dropped due to channel backpressure.
    Best-effort marker — the terminal event is authoritative.
    """
    reason: Required[StreamTruncationReason]
    type: Required[Literal['stream_truncated']]


class AgentEventToolConfigChanged(TypedDict, total=False):
    """Live tool configuration changed for this session.
    """
    payload: Required[ToolConfigChangedPayload]
    type: Required[Literal['tool_config_changed']]


class AgentEventBackgroundJobCompleted(TypedDict, total=False):
    """A background shell job completed (or failed/cancelled/timed out).
    """
    detail: Required[str]
    display_name: Required[str]
    job_id: Required[str]
    terminal_status: Required[BackgroundJobTerminalStatus]
    type: Required[Literal['background_job_completed']]


class AgentEventTranscriptRewriteCommitted(TypedDict, total=False):
    """Released 0.8.10 generation-zero full-body compatibility row.

    Current writers must never emit this variant. It is decoded only by the
    one-time compatibility reconciliation path; receipt batches supersede
    the redundant body copy.
    """
    record: Required[TranscriptRewriteRecord]
    session_id: Required[SessionId]
    type: Required[Literal['transcript_rewrite_committed']]


class AgentEventTranscriptRewriteAuditReceiptCommitted(TypedDict, total=False):
    """Receipt-only evidence for one non-empty ordered rewrite suffix.

    The sealed Session graph remains the singular body authority. The
    optional terminal assistant text is a delta-sized derived projection
    used to rebuild `summary.txt`; `None` explicitly removes a stale
    summary when the retained successor has no assistant text.
    """
    final_assistant_text: NotRequired[Optional[str]]
    receipt: Required[TranscriptRewriteAuditReceiptBatch]
    session_id: Required[SessionId]
    type: Required[Literal['transcript_rewrite_audit_receipt_committed']]


class AgentEventProviderCacheBreakpointsDiscarded(TypedDict, total=False):
    """Provider-authored cache breakpoints were discarded at a turn commit.

    A cache breakpoint is an optimization artifact anchored to one exact
    committed transcript prefix. Ordinary transcript motion unbinds it, and
    dropping it costs caching for the turn - never the turn itself. This
    event is the host-observable record of that cost, so a degraded turn is
    a routed fact rather than a log line: the same turn still completes.

    `origin` on each discard separates inherited poison (persisted by an
    earlier turn) from this turn's own lowering disagreeing with the
    committed transcript. Those have different root causes and are
    indistinguishable without it.
    """
    discarded: Required[list[DiscardedCacheBreakpoint]]
    retained: Required[int]
    session_id: Required[SessionId]
    type: Required[Literal['provider_cache_breakpoints_discarded']]


class AgentEventPeerContentIngested(TypedDict, total=False):
    """Inbound peer content was committed into this session's context.

    Emitted as a pure projection of the typed transcript carrier
    ([`crate::types::SystemNoticeBlock::Comms`] with incoming direction)
    at the moment the agent commits it — a queued delivery appended to the
    transcript at run assembly, or a steer delivery injected as a
    request-local User message at the model boundary. One event per committed comms
    block, so a host taint tracker classifies peer ingestion from typed
    facts (canonical peer identity + the sender's signed content-taint
    declaration) instead of parsing rendered projection text.

    The event stream is best-effort observability (see `event_tap`); the
    transcript block remains the durable owner of these facts.
    """
    kind: Required[CommsNoticeKind]
    peer: NotRequired[Optional[SystemNoticePeer]]
    request_id: NotRequired[Optional[str]]
    sender_taint: NotRequired[Optional[SenderContentTaint]]
    type: Required[Literal['peer_content_ingested']]


class AgentEventTurnUsageAccountingUnmeasured(TypedDict, total=False):
    """One model turn's provider token accounting was absent.

    The routed form of the `unmeasured:turn_usage_accounting` marker. It
    states exactly one thing: the provider stream for this turn carried no
    normalized accounting, so no accounting axis advanced for it - no
    budget charge, no session usage, no presented-token update.

    It deliberately does NOT claim the turn completed. This is published at
    the model boundary, before the boundary effects and terminal hooks that
    can still fail the turn on their own (unrelated) grounds, and the
    absence of accounting is true either way. Whether the turn completed is
    owned by [`AgentEvent::TurnCompleted`] - whose `usage` is `None` on the
    turns this marker names - and a turn that fails afterwards publishes
    its own terminal fact.

    What this is not is a cause of failure. A number nobody has cannot
    invalidate an answer the user has already read; see
    [`crate::UnmeasuredTurnUsageAccounting`].
    """
    session_id: Required[SessionId]
    type: Required[Literal['turn_usage_accounting_unmeasured']]
    unmeasured: Required[UnmeasuredTurnUsageAccounting]


class AgentEventTurnUsageAccountingIdentityDisputed(TypedDict, total=False):
    """One model turn's accounting named a provider/model other than the
    request it answered.

    The routed form of the `disputed:turn_usage_accounting_identity`
    marker. Unlike [`AgentEvent::TurnUsageAccountingUnmeasured`] the
    counters exist and are internally consistent, so the token axis still
    advances on them; what is in dispute is attribution. The reported
    identity is published exactly as the adapter minted it and is never
    rewritten to the active identity.
    """
    dispute: Required[DisputedTurnUsageAccountingIdentity]
    session_id: Required[SessionId]
    type: Required[Literal['turn_usage_accounting_identity_disputed']]


# Events emitted during agent execution
#
# These events form the streaming API for consumers.
AgentEvent = AgentEventRunStarted | AgentEventRunCompleted | AgentEventExtractionSucceeded | AgentEventExtractionFailed | AgentEventRunFailed | AgentEventHookStarted | AgentEventHookCompleted | AgentEventHookFailed | AgentEventHookDenied | AgentEventTurnStarted | AgentEventReasoningDelta | AgentEventReasoningComplete | AgentEventTextDelta | AgentEventTextComplete | AgentEventServerToolContent | AgentEventAssistantImageAppended | AgentEventToolCallRequested | AgentEventToolResultReceived | AgentEventTurnCompleted | AgentEventToolExecutionStarted | AgentEventToolExecutionCompleted | AgentEventToolExecutionTimedOut | AgentEventCompactionStarted | AgentEventCompactionCompleted | AgentEventCompactionFailed | AgentEventBudgetWarning | AgentEventRetrying | AgentEventSkillsResolved | AgentEventSkillResolutionFailed | AgentEventInteractionComplete | AgentEventInteractionCallbackPending | AgentEventInteractionFailed | AgentEventStreamTruncated | AgentEventToolConfigChanged | AgentEventBackgroundJobCompleted | AgentEventTranscriptRewriteCommitted | AgentEventTranscriptRewriteAuditReceiptCommitted | AgentEventProviderCacheBreakpointsDiscarded | AgentEventPeerContentIngested | AgentEventTurnUsageAccountingUnmeasured | AgentEventTurnUsageAccountingIdentityDisputed


class StreamScopeFramePrimary(TypedDict, total=False):
    """Top-level primary session scope.
    """
    scope: Required[Literal['primary']]
    session_id: Required[str]


class StreamScopeFrameMobMember(TypedDict, total=False):
    """Mob member scope for flow dispatch turns.
    """
    agent_identity: Required[str]
    flow_run_id: Required[str]
    scope: Required[Literal['mob_member']]


# Scope attribution frame for multi-agent streaming.
StreamScopeFrame = StreamScopeFramePrimary | StreamScopeFrameMobMember
