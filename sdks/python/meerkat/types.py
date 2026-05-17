"""Public domain types for the Meerkat Python SDK.

These are the types returned by :class:`~meerkat.Session` and
:class:`~meerkat.MeerkatClient` methods.  They replace the ``Wire*``
prefixed generated types which are now an internal implementation detail.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, Literal, NewType, NotRequired, TypedDict, Union

from .generated.types import CONTRACT_VERSION as CONTRACT_VERSION  # re-export
from .generated.types import (
    McpAddParams as McpAddParams,
    McpHttpConfig as McpHttpConfig,
    McpHttpServerConfig as McpHttpServerConfig,
    McpHttpTransport as McpHttpTransport,
    McpLiveOpResponse as McpLiveOpResponse,
    McpReloadParams as McpReloadParams,
    McpRemoveParams as McpRemoveParams,
    McpServerConfig as McpServerConfig,
    McpStdioConfig as McpStdioConfig,
    McpStdioServerConfig as McpStdioServerConfig,
    MobBackendConfigInput as MobBackendConfigInput,
    MobCreateParams as MobCreateParams,
    MobCreateResult as MobCreateResult,
    MobDefinitionInput as MobDefinitionInput,
    MobEnsureMemberParams as MobEnsureMemberParams,
    MobEnsureMemberResult as MobEnsureMemberResult,
    MobEventRouterConfigInput as MobEventRouterConfigInput,
    MobExternalBackendConfigInput as MobExternalBackendConfigInput,
    MobFlowNodeInput as MobFlowNodeInput,
    MobFlowSpecInput as MobFlowSpecInput,
    MobFlowStepInput as MobFlowStepInput,
    MobFrameSpecInput as MobFrameSpecInput,
    MobLimitsSpecInput as MobLimitsSpecInput,
    MobMemberListEntryWire as MobMemberListEntryWire,
    MobMemberSpecWire as MobMemberSpecWire,
    MobOrchestratorInput as MobOrchestratorInput,
    MobProfileBindingInput as MobProfileBindingInput,
    MobProfileInput as MobProfileInput,
    MobReconcileParams as MobReconcileParams,
    MobReconcileReportWire as MobReconcileReportWire,
    MobReconcileResult as MobReconcileResult,
    MobSpawnManyFailedResult as MobSpawnManyFailedResult,
    MobSpawnManyFailureCause as MobSpawnManyFailureCause,
    MobSpawnManyParams as MobSpawnManyParams,
    MobSpawnManyResult as MobSpawnManyResult,
    MobSpawnManyResultEntry as MobSpawnManyResultEntry,
    MobSpawnManyResultPayload as MobSpawnManyResultPayload,
    MobSpawnManyResultStatus as MobSpawnManyResultStatus,
    MobSpawnManySpawnedResult as MobSpawnManySpawnedResult,
    MobSpawnParams as MobSpawnParams,
    MobSpawnPolicyInput as MobSpawnPolicyInput,
    MobSpawnReceiptWire as MobSpawnReceiptWire,
    MobSpawnSpecParams as MobSpawnSpecParams,
    MobSupervisorSpecInput as MobSupervisorSpecInput,
    MobToolConfigInput as MobToolConfigInput,
    MobTopologySpecInput as MobTopologySpecInput,
    MobTurnStartParams as MobTurnStartParams,
    MobWireMembersBatchEdge as MobWireMembersBatchEdge,
    MobWireMembersBatchParams as MobWireMembersBatchParams,
    MobWireMembersBatchResult as MobWireMembersBatchResult,
    MobWiringRulesInput as MobWiringRulesInput,
    LiveChannelParams as LiveChannelParams,
    LiveCommitInputParams as LiveCommitInputParams,
    LiveInputChunkWire as LiveInputChunkWire,
    # R5-10: re-export typed `LiveInputChunkWire` variants so SDK consumers
    # can construct typed chunks at the `LiveSendInputParams.chunk` slot
    # without dipping into `meerkat.generated.types`.
    LiveInputChunkWireAudio as LiveInputChunkWireAudio,
    LiveInputChunkWireText as LiveInputChunkWireText,
    LiveInputChunkWireImage as LiveInputChunkWireImage,
    LiveInputChunkWireVideoFrame as LiveInputChunkWireVideoFrame,
    LiveOpenParams as LiveOpenParams,
    LiveOpenResult as LiveOpenResult,
    LiveWebrtcAnswerParams as LiveWebrtcAnswerParams,
    LiveWebrtcAnswerResult as LiveWebrtcAnswerResult,
    LiveSendInputParams as LiveSendInputParams,
    LiveStatusResult as LiveStatusResult,
    LiveTruncateParams as LiveTruncateParams,
    # CC5/CC6: typed wire mirrors for the live `capabilities` + `continuity`
    # shapes. Re-exported alongside `LiveOpenResult` so SDK consumers can
    # reach the typed booleans (`image_in`, `video_in`, etc.) and the
    # discriminated continuity-mode union without dipping into the
    # `meerkat.generated.types` module.
    WireLiveChannelCapabilities as WireLiveChannelCapabilities,
    WireLiveContinuityMode as WireLiveContinuityMode,
    WireLiveContinuityModeFresh as WireLiveContinuityModeFresh,
    WireLiveContinuityModeTranscriptOnly as WireLiveContinuityModeTranscriptOnly,
    WireLiveContinuityModeDegraded as WireLiveContinuityModeDegraded,
    WireLiveContinuityModeProviderNativeResume as WireLiveContinuityModeProviderNativeResume,
    # FIX-SDK-OBS: typed live-adapter observation discriminated union and
    # its supporting wire mirrors. Browser/Python clients can type-narrow
    # on the `observation` discriminator and read R5-4 identity fields
    # (`item_id`, `response_id`, `content_index`) on
    # `assistant_audio_chunk` plus the R5-9 `command_rejected` typed
    # channel-survives error variant.
    WireLiveAdapterObservation as WireLiveAdapterObservation,
    WireLiveAdapterObservationReady as WireLiveAdapterObservationReady,
    WireLiveAdapterObservationUserTranscriptFinal as WireLiveAdapterObservationUserTranscriptFinal,
    WireLiveAdapterObservationAssistantTextDelta as WireLiveAdapterObservationAssistantTextDelta,
    WireLiveAdapterObservationAssistantTranscriptDelta as WireLiveAdapterObservationAssistantTranscriptDelta,
    WireLiveAdapterObservationAssistantAudioChunk as WireLiveAdapterObservationAssistantAudioChunk,
    WireLiveAdapterObservationAssistantTranscriptFinal as WireLiveAdapterObservationAssistantTranscriptFinal,
    WireLiveAdapterObservationAssistantTranscriptTruncated as WireLiveAdapterObservationAssistantTranscriptTruncated,
    WireLiveAdapterObservationRealtimeTranscript as WireLiveAdapterObservationRealtimeTranscript,
    WireLiveAdapterObservationToolCallRequested as WireLiveAdapterObservationToolCallRequested,
    WireLiveAdapterObservationTurnInterrupted as WireLiveAdapterObservationTurnInterrupted,
    WireLiveAdapterObservationTurnCompleted as WireLiveAdapterObservationTurnCompleted,
    WireLiveAdapterObservationStatusChanged as WireLiveAdapterObservationStatusChanged,
    WireLiveAdapterObservationError as WireLiveAdapterObservationError,
    WireLiveAdapterObservationCommandRejected as WireLiveAdapterObservationCommandRejected,
    WireLiveAdapterStatus as WireLiveAdapterStatus,
    WireLiveAdapterErrorCode as WireLiveAdapterErrorCode,
    WireLiveConfigRejectionReason as WireLiveConfigRejectionReason,
    WireLiveConfigRejectionReasonChannelIdentitySwap as WireLiveConfigRejectionReasonChannelIdentitySwap,
    WireLiveConfigRejectionReasonNonRealtimeResolution as WireLiveConfigRejectionReasonNonRealtimeResolution,
    WireLiveConfigRejectionReasonImageInputNotImplemented as WireLiveConfigRejectionReasonImageInputNotImplemented,
    WireLiveConfigRejectionReasonVideoFrameInputNotImplemented as WireLiveConfigRejectionReasonVideoFrameInputNotImplemented,
    WireLiveConfigRejectionReasonUnsupportedInputChunkVariant as WireLiveConfigRejectionReasonUnsupportedInputChunkVariant,
    WireLiveConfigRejectionReasonRefreshModelSwap as WireLiveConfigRejectionReasonRefreshModelSwap,
    WireLiveConfigRejectionReasonRefreshProviderSwap as WireLiveConfigRejectionReasonRefreshProviderSwap,
    WireLiveConfigRejectionReasonRefreshAudioConfigMismatch as WireLiveConfigRejectionReasonRefreshAudioConfigMismatch,
    WireLiveConfigRejectionReasonAudioInputFormatMismatch as WireLiveConfigRejectionReasonAudioInputFormatMismatch,
    WireLiveConfigRejectionReasonOther as WireLiveConfigRejectionReasonOther,
    WireLiveConfigRejectionReasonUnknown as WireLiveConfigRejectionReasonUnknown,
    WireLiveTransportBootstrap as WireLiveTransportBootstrap,
    WireLiveTransportBootstrapWebsocket as WireLiveTransportBootstrapWebsocket,
    WireLiveTransportBootstrapWebrtc as WireLiveTransportBootstrapWebrtc,
    WireLiveTransportBootstrapUnknown as WireLiveTransportBootstrapUnknown,
    WireAssistantBlock as WireAssistantBlock,
    WireAssistantBlockText as WireAssistantBlockText,
    WireAssistantBlockTranscript as WireAssistantBlockTranscript,
    WireAssistantBlockReasoning as WireAssistantBlockReasoning,
    WireAssistantBlockToolUse as WireAssistantBlockToolUse,
    WireAssistantBlockServerToolContent as WireAssistantBlockServerToolContent,
    WireAssistantBlockImage as WireAssistantBlockImage,
    WireAssistantBlockUnknown as WireAssistantBlockUnknown,
    WireTranscriptSource as WireTranscriptSource,
    WireTranscriptSourceSpoken as WireTranscriptSourceSpoken,
    WireTranscriptSourceUnknown as WireTranscriptSourceUnknown,
    WireLiveDegradationReason as WireLiveDegradationReason,
    WireProvider as WireProvider,
    RealtimeAudioChunk as RealtimeAudioChunk,
    RealtimeCapabilities as RealtimeCapabilities,
    RealtimeInputChunk as RealtimeInputChunk,
    RealtimeInputKind as RealtimeInputKind,
    RealtimeOutputKind as RealtimeOutputKind,
    RealtimeTextChunk as RealtimeTextChunk,
    RealtimeTurningMode as RealtimeTurningMode,
    RealtimeVideoChunk as RealtimeVideoChunk,
    RuntimeAcceptResult as RuntimeAcceptResult,
    RuntimeStateResult as RuntimeStateResult,
    WireBudgetSplitPolicy as WireBudgetSplitPolicy,
    WireAssistantImageRef as WireAssistantImageRef,
    WireAuthBindingRef as WireAuthBindingRef,
    WireGenerateImageExecutionPlan as WireGenerateImageExecutionPlan,
    WireGenerateImageRequest as WireGenerateImageRequest,
    WireImageGenerationToolResult as WireImageGenerationToolResult,
    WireImageOperationPhase as WireImageOperationPhase,
    WireInputState as WireInputState,
    WireMemberLaunchMode as WireMemberLaunchMode,
    WireMemberRef as WireMemberRef,
    WireMemberState as WireMemberState,
    WireMobBackendKind as WireMobBackendKind,
    WireMobMemberStatus as WireMobMemberStatus,
    WireMobProfile as WireMobProfile,
    WireMobRuntimeMode as WireMobRuntimeMode,
    WireMobToolConfig as WireMobToolConfig,
    WireRuntimeBinding as WireRuntimeBinding,
    WireToolAccessPolicy as WireToolAccessPolicy,
    WireToolFilter as WireToolFilter,
)

PeerId = NewType("PeerId", str)
"""Canonical comms routing identity for a peer."""

PeerCorrelationId = NewType("PeerCorrelationId", str)
"""Canonical request/response correlation identity for peer interactions."""

# Re-export Usage from events so there's a single canonical definition.
from .events import Event, Usage as Usage  # noqa: F401


# ---------------------------------------------------------------------------
# Skill references (v2.1)
# ---------------------------------------------------------------------------

@dataclass(frozen=True, slots=True)
class SkillKey:
    """Structured skill identifier (source UUID + skill name)."""

    source_uuid: str
    skill_name: str


SkillRef = SkillKey
"""A skill reference, expressed as a structured :class:`SkillKey`."""


class TextBlock(TypedDict):
    type: Literal["text"]
    text: str


class InlineImageBlock(TypedDict, total=False):
    type: Literal["image"]
    media_type: str
    source: Literal["inline"]
    data: str


class BlobImageBlock(TypedDict):
    type: Literal["image"]
    media_type: str
    source: Literal["blob"]
    blob_id: str


class InlineVideoBlock(TypedDict, total=False):
    type: Literal["video"]
    media_type: str
    duration_ms: int
    source: Literal["inline"]
    data: str


ContentBlock = Union[TextBlock, InlineImageBlock, BlobImageBlock, InlineVideoBlock]
"""A multimodal content block accepted by input-bearing APIs."""

ContentInput = str | list[ContentBlock]
"""Canonical content input accepted by input-bearing APIs and returned by history surfaces."""


@dataclass(frozen=True, slots=True)
class BlobPayload:
    """Raw blob bytes fetched by blob id."""

    blob_id: str = ""
    media_type: str = ""
    data_base64: str = ""


# ---------------------------------------------------------------------------
# Domain types
# ---------------------------------------------------------------------------

@dataclass(frozen=True, slots=True)
class SchemaWarning:
    """Warning emitted when structured output doesn't match a provider's schema rules."""

    provider: str = ""
    path: str = ""
    message: str = ""


@dataclass(frozen=True, slots=True)
class SourceHealthSnapshot:
    """Runtime health snapshot for skill source resolution."""

    state: str = ""
    invalid_ratio: float = 0.0
    invalid_count: int = 0
    total_count: int = 0
    failure_streak: int = 0
    handshake_failed: bool = False


@dataclass(frozen=True, slots=True)
class SkillQuarantineDiagnostic:
    """Diagnostic details for a single quarantined skill entry."""

    source_uuid: str = ""
    skill_id: str = ""
    location: str = ""
    error_code: str = ""
    error_class: str = ""
    message: str = ""
    first_seen_unix_secs: int = 0
    last_seen_unix_secs: int = 0


@dataclass(frozen=True, slots=True)
class SkillRuntimeDiagnostics:
    """Runtime diagnostics emitted by the Rust skill subsystem."""

    source_health: SourceHealthSnapshot = field(default_factory=SourceHealthSnapshot)
    quarantined: list[SkillQuarantineDiagnostic] = field(default_factory=list)


@dataclass(frozen=True, slots=True)
class RunResult:
    """Result of an agent session creation or turn.

    Replaces ``WireRunResult``.  All fields use native Python types.
    """

    session_id: str = ""
    text: str = ""
    turns: int = 0
    tool_calls: int = 0
    usage: Usage = field(default_factory=Usage)
    terminal_cause_kind: str | None = None
    session_ref: str | None = None
    structured_output: Any = None
    extraction_error: ExtractionError | None = None
    schema_warnings: list[SchemaWarning] | None = None
    skill_diagnostics: SkillRuntimeDiagnostics | None = None


HelpExecutionMode = Literal["explain_only", "plan_execution"]


class HelpRequest(TypedDict):
    question: str
    prompt: NotRequired[str]
    execution_mode: NotRequired[HelpExecutionMode]
    model: NotRequired[str]
    provider: NotRequired[str]
    max_tokens: NotRequired[int]


@dataclass(frozen=True, slots=True)
class ExtractionError:
    """Structured-output extraction failure after a completed main run."""

    last_output: str = ""
    attempts: int = 0
    reason: str = ""


@dataclass(frozen=True, slots=True)
class SessionInfo:
    """Session identity and shared metadata.

    Shared base for session summary/detail responses.
    """

    session_id: str = ""
    session_ref: str | None = None
    created_at: int = 0
    updated_at: int = 0
    message_count: int = 0
    labels: dict[str, str] = field(default_factory=dict)
    is_active: bool = False


@dataclass(frozen=True, slots=True)
class SessionSummary(SessionInfo):
    """Summary returned by :meth:`~meerkat.MeerkatClient.list_sessions`."""

    total_tokens: int = 0


@dataclass(frozen=True, slots=True)
class SessionDetails(SessionInfo):
    """Details returned by :meth:`~meerkat.MeerkatClient.read_session`."""

    model: str = ""
    provider: str = ""
    last_assistant_text: str | None = None
    resolved_capabilities: ResolvedModelCapabilities | None = None


class ConfigEnvelope(TypedDict, total=False):
    """Config envelope returned by config APIs."""

    config: dict[str, Any]
    generation: int
    realm_id: str | None
    instance_id: str | None
    backend: str | None
    resolved_paths: dict[str, str] | None


class ExternalEventOutcome(TypedDict, total=False):
    """Outcome payload returned by `session/external_event`."""

    outcome_type: str
    input_id: str
    existing_id: str
    reason: str
    state: dict[str, Any] | None


class ScheduleRecord(TypedDict, total=False):
    """Canonical schedule payload."""

    schedule_id: str
    phase: str
    revision: int
    name: str | None
    description: str | None
    trigger: dict[str, Any]
    target: dict[str, Any]
    labels: dict[str, str]


class ScheduleOccurrenceRecord(TypedDict, total=False):
    """Canonical schedule occurrence payload."""

    occurrence_id: str
    schedule_id: str
    phase: str
    due_at_utc: str
    attempt_count: int


class ScheduleListResult(TypedDict):
    schedules: list[ScheduleRecord]


class ScheduleOccurrencesResult(TypedDict):
    occurrences: list[ScheduleOccurrenceRecord]


class ScheduleToolsResult(TypedDict):
    tools: list[dict[str, Any]]


class ScheduleToolCall(TypedDict, total=False):
    name: str
    arguments: Any


WorkGraphStatus = Literal[
    "open",
    "in_progress",
    "blocked",
    "completed",
    "cancelled",
    "failed",
]
WorkGraphPriority = Literal["low", "medium", "high"]
WorkGraphEdgeKind = Literal[
    "blocks",
    "parent",
    "related",
    "supersedes",
    "derived_from",
]
WorkGraphEventKind = Literal[
    "created",
    "updated",
    "claimed",
    "released",
    "blocked",
    "closed",
    "linked",
    "evidence_added",
]
WorkGraphOwnerKind = Literal["principal", "agent", "session", "mob", "label"]


class WorkGraphOwnerKey(TypedDict):
    kind: WorkGraphOwnerKind
    id: str


class WorkGraphOwner(TypedDict):
    key: WorkGraphOwnerKey
    display_name: NotRequired[str]


class WorkGraphClaim(TypedDict):
    owner: WorkGraphOwner
    claimed_at: str
    lease_expires_at: NotRequired[str]


class ExternalWorkRef(TypedDict, total=False):
    kind: str
    id: str
    url: str


class WorkEvidenceRef(TypedDict, total=False):
    kind: str
    id: str
    label: str
    summary: str


class WorkItem(TypedDict, total=False):
    id: str
    realm_id: str
    namespace: str
    title: str
    description: str
    status: WorkGraphStatus
    priority: WorkGraphPriority
    labels: list[str]
    owner: WorkGraphOwner
    claim: WorkGraphClaim
    revision: int
    due_at: str
    not_before: str
    snoozed_until: str
    created_at: str
    updated_at: str
    terminal_at: str
    external_refs: list[ExternalWorkRef]
    evidence_refs: list[WorkEvidenceRef]


class WorkGraphEdge(TypedDict, total=False):
    realm_id: str
    namespace: str
    kind: WorkGraphEdgeKind
    from_id: str
    to_id: str
    created_at: str


class WorkGraphEvent(TypedDict, total=False):
    seq: int
    realm_id: str
    namespace: str
    item_id: str
    kind: WorkGraphEventKind
    at: str
    payload: Any


class WorkItemListResult(TypedDict):
    items: list[WorkItem]


class WorkGraphEventsResult(TypedDict):
    events: list[WorkGraphEvent]


class WorkGraphSnapshot(TypedDict):
    realm_id: str
    namespace: NotRequired[str]
    all_namespaces: bool
    captured_at: str
    event_high_water_mark: NotRequired[int]
    items: list[WorkItem]
    edges: list[WorkGraphEdge]
    ready_item_ids: list[str]


class WorkGraphItemFilter(TypedDict, total=False):
    realm_id: str
    namespace: str
    all_namespaces: bool
    statuses: list[WorkGraphStatus]
    labels: list[str]
    include_terminal: bool
    limit: int


class WorkGraphReadyFilter(TypedDict, total=False):
    realm_id: str
    namespace: str
    labels: list[str]
    limit: int


class WorkGraphSnapshotFilter(WorkGraphItemFilter, total=False):
    pass


class WorkGraphEventFilter(TypedDict, total=False):
    realm_id: str
    namespace: str
    all_namespaces: bool
    after_seq: int
    limit: int


class MobEventCursorEntry(TypedDict, total=False):
    cursor: int
    event: dict[str, Any]


class MobEventsResult(TypedDict):
    events: list[MobEventCursorEntry]


class MobProfileTools(TypedDict, total=False):
    builtins: bool
    shell: bool
    comms: bool
    memory: bool
    mob: bool
    schedule: bool
    mcp: list[str]


class MobProfile(TypedDict, total=False):
    model: str
    skills: list[str]
    tools: MobProfileTools
    peer_description: str
    external_addressable: bool
    backend: str | None
    runtime_mode: str
    max_inline_peer_notifications: int | None
    output_schema: dict[str, Any] | None
    provider_params: dict[str, Any] | None


class StoredMobProfile(TypedDict):
    name: str
    profile: MobProfile
    revision: int
    created_at: str
    updated_at: str


class DeletedMobProfile(TypedDict):
    name: str
    deleted_revision: int


class ResolvedModelCapabilities(TypedDict):
    vision: bool
    image_input: bool
    image_tool_results: bool
    inline_video: bool
    realtime: bool
    web_search: bool
    image_generation: bool


class ModelProfile(TypedDict):
    model_family: str
    vision: bool
    image_input: bool
    image_tool_results: bool
    supports_temperature: bool
    supports_thinking: bool
    supports_reasoning: bool
    inline_video: bool
    realtime: bool
    supports_web_search: bool
    image_generation: bool
    params_schema: dict[str, Any]


class CatalogModelEntry(TypedDict, total=False):
    id: str
    display_name: str
    tier: str
    context_window: int | None
    max_output_tokens: int | None
    server_id: str | None
    profile: ModelProfile | None


class ProviderCatalog(TypedDict):
    provider: str
    default_model_id: str
    models: list[CatalogModelEntry]


class ContractVersion(TypedDict):
    major: int
    minor: int
    patch: int


class ModelsCatalogResponse(TypedDict):
    contract_version: ContractVersion
    providers: list[ProviderCatalog]


@dataclass(frozen=True, slots=True)
class SessionToolCall:
    """Legacy assistant tool call captured in transcript history."""

    id: str = ""
    name: str = ""
    args: Any = None


@dataclass(frozen=True, slots=True)
class SessionToolResult:
    """Tool result captured in transcript history."""

    tool_use_id: str = ""
    content: ContentInput = ""
    is_error: bool = False


@dataclass(frozen=True, slots=True)
class SessionAssistantBlock:
    """Ordered block inside a block-assistant transcript message.

    `block_type` carries the lane discriminator (``text``, ``transcript``,
    ``reasoning``, ``tool_use``, ``server_tool_content``, ``image``, ...).
    For ``transcript`` blocks ``source`` records the originating lane
    (today: ``spoken``); both ``text`` and ``transcript`` blocks expose
    their rendered string in ``text``.
    """

    block_type: str = ""
    text: str | None = None
    id: str | None = None
    name: str | None = None
    args: Any = None
    image_id: str | None = None
    blob_id: str | None = None
    media_type: str | None = None
    width: int | None = None
    height: int | None = None
    revised_prompt: dict[str, Any] | None = None
    meta: dict[str, Any] | None = None
    # Lane provenance for ``transcript`` blocks (e.g. ``"spoken"``).
    # ``None`` for non-transcript block types.
    source: str | None = None


@dataclass(frozen=True, slots=True)
class SessionMessage:
    """Canonical transcript message returned by session history APIs."""

    role: str = ""
    created_at: str = ""
    content: ContentInput | None = None
    tool_calls: list[SessionToolCall] = field(default_factory=list)
    stop_reason: str | None = None
    blocks: list[SessionAssistantBlock] = field(default_factory=list)
    results: list[SessionToolResult] = field(default_factory=list)


@dataclass(frozen=True, slots=True)
class SessionHistory:
    """Paginated transcript page for a session."""

    session_id: str = ""
    session_ref: str | None = None
    message_count: int = 0
    offset: int = 0
    limit: int | None = None
    has_more: bool = False
    messages: list[SessionMessage] = field(default_factory=list)


TranscriptEditRunningBehavior = Literal["reject"]
"""Behavior for transcript edit requests when the source session has active work."""


class TranscriptMessageReplacement(TypedDict):
    type: Literal["message"]
    message: dict[str, Any]


class TranscriptUserContentBlockReplacement(TypedDict):
    type: Literal["user_content_block"]
    block_index: int
    block: ContentBlock


class TranscriptAssistantBlockReplacement(TypedDict):
    type: Literal["assistant_block"]
    block_index: int
    block: dict[str, Any]


class TranscriptToolResultContentBlockReplacement(TypedDict):
    type: Literal["tool_result_content_block"]
    result_index: int
    block_index: int
    block: ContentBlock


TranscriptReplacement = (
    TranscriptMessageReplacement
    | TranscriptUserContentBlockReplacement
    | TranscriptAssistantBlockReplacement
    | TranscriptToolResultContentBlockReplacement
)
"""Typed transcript replacement used to create an edited session fork."""


@dataclass(frozen=True, slots=True)
class SessionForkResult:
    """Result of creating a forked transcript branch."""

    source_session_id: str = ""
    session_id: str = ""
    session_ref: str | None = None
    message_count: int = 0


@dataclass(frozen=True, slots=True)
class EventSourceIdentity:
    """Typed source identity for session or runtime event semantics."""

    type: str = ""
    session_id: str | None = None
    runtime_id: str | None = None
    interaction_id: str | None = None
    source_id: str | None = None


@dataclass(frozen=True, slots=True)
class EventEnvelope:
    """Session or agent event with delivery metadata."""

    event_id: str = ""
    source: EventSourceIdentity | None = None
    source_id: str = ""
    seq: int = 0
    timestamp_ms: int = 0
    payload: Event | None = None


@dataclass(frozen=True, slots=True)
class AttributedEvent:
    """Mob event annotated with the emitting member identity."""

    source: str = ""
    role: str = ""
    envelope: EventEnvelope = field(default_factory=EventEnvelope)


@dataclass(frozen=True, slots=True)
class Capability:
    """A runtime capability and its availability status."""

    id: str = ""
    description: str = ""
    status: str = ""

    @property
    def available(self) -> bool:
        """Whether this capability is available."""
        return self.status == "Available"
