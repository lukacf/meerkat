"""Public domain types for the Meerkat Python SDK.

These are the types returned by :class:`~meerkat.Session` and
:class:`~meerkat.MeerkatClient` methods.  They replace the ``Wire*``
prefixed generated types which are now an internal implementation detail.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, Literal, TypedDict, Union

from .generated.types import CONTRACT_VERSION as CONTRACT_VERSION  # re-export
from .generated.types import (
    InputListParams as InputListParams,
    InputListResult as InputListResult,
    McpAddParams as McpAddParams,
    McpLiveOpResponse as McpLiveOpResponse,
    McpReloadParams as McpReloadParams,
    McpRemoveParams as McpRemoveParams,
    RuntimeAcceptParams as RuntimeAcceptParams,
    RuntimeAcceptResult as RuntimeAcceptResult,
    RuntimeResetParams as RuntimeResetParams,
    RuntimeResetResult as RuntimeResetResult,
    RuntimeRetireParams as RuntimeRetireParams,
    RuntimeRetireResult as RuntimeRetireResult,
    RuntimeStateParams as RuntimeStateParams,
    RuntimeStateResult as RuntimeStateResult,
    WireInputState as WireInputState,
)

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


SkillRef = Union[SkillKey, str]
"""A skill reference — either a :class:`SkillKey` or a legacy string like
``"<source_uuid>/<skill_name>"``."""


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
    session_ref: str | None = None
    structured_output: Any = None
    schema_warnings: list[SchemaWarning] | None = None
    skill_diagnostics: SkillRuntimeDiagnostics | None = None


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
    mob_tasks: bool
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


class ModelProfile(TypedDict):
    model_family: str
    supports_temperature: bool
    supports_thinking: bool
    supports_reasoning: bool
    inline_video: bool
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
    """Ordered block inside a block-assistant transcript message."""

    block_type: str = ""
    text: str | None = None
    id: str | None = None
    name: str | None = None
    args: Any = None
    meta: dict[str, Any] | None = None


@dataclass(frozen=True, slots=True)
class SessionMessage:
    """Canonical transcript message returned by session history APIs."""

    role: str = ""
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


@dataclass(frozen=True, slots=True)
class EventEnvelope:
    """Session or agent event with delivery metadata."""

    event_id: str = ""
    source_id: str = ""
    seq: int = 0
    timestamp_ms: int = 0
    payload: Event | None = None


@dataclass(frozen=True, slots=True)
class AttributedEvent:
    """Mob event annotated with the emitting member runtime identity."""

    source: str = ""
    source_fence_token: int | None = None
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
