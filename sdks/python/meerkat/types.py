"""Public domain types for the Meerkat Python SDK.

These are the types returned by :class:`~meerkat.Session` and
:class:`~meerkat.MeerkatClient` methods.  They replace the ``Wire*``
prefixed generated types which are now an internal implementation detail.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, Union

from .generated.types import CONTRACT_VERSION as CONTRACT_VERSION  # re-export
from .generated.types import (
    McpAddParams as McpAddParams,
    McpLiveOpResponse as McpLiveOpResponse,
    McpReloadParams as McpReloadParams,
    McpRemoveParams as McpRemoveParams,
)

# Re-export Usage from events so there's a single canonical definition.
from .events import Usage as Usage  # noqa: F401


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
    is_active: bool = False


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
