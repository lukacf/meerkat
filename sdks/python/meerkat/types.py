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


ContentBlock = Union[TextBlock, InlineImageBlock, BlobImageBlock]
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
    """Summary of an active session.

    Returned by :meth:`~meerkat.MeerkatClient.list_sessions` and
    :meth:`~meerkat.MeerkatClient.read_session`.
    """

    session_id: str = ""
    session_ref: str | None = None
    created_at: str = ""
    updated_at: str = ""
    message_count: int = 0
    total_tokens: int = 0
    labels: dict[str, str] = field(default_factory=dict)
    is_active: bool = False
    model: str = ""
    provider: str = ""
    last_assistant_text: str | None = None


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
    """Mob event annotated with the emitting member identity."""

    source: str = ""
    profile: str = ""
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
