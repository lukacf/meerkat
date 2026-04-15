from __future__ import annotations

"""Generated wire types for Meerkat SDK.

Contract version: 0.5.2
"""

from dataclasses import dataclass, field
from typing import Any, Literal, Optional


CONTRACT_VERSION = "0.5.2"


@dataclass
class WireUsage:
    """Token usage information."""
    input_tokens: int = 0
    output_tokens: int = 0
    total_tokens: int = 0
    cache_creation_tokens: Optional[int] = None
    cache_read_tokens: Optional[int] = None


@dataclass
class WireRunResult:
    """Run result from agent execution."""
    session_id: str = ''
    session_ref: Optional[str] = None
    text: str = ''
    turns: int = 0
    tool_calls: int = 0
    usage: Optional[WireUsage] = None
    structured_output: Optional[Any] = None
    schema_warnings: Optional[list[Any]] = None
    skill_diagnostics: Optional[dict] = None


@dataclass
class WireProviderMeta:
    """Provider continuity metadata."""
    provider: str = ''


@dataclass
class WireAssistantBlock:
    """Block assistant transcript item."""
    block_type: str = ''
    data: Optional[dict] = None


@dataclass
class WireToolCall:
    """Legacy assistant tool call."""
    id: str = ''
    name: str = ''
    args: Optional[Any] = None


@dataclass
class WireToolResult:
    """Tool result transcript item."""
    tool_use_id: str = ''
    content: Optional[WireToolResultContent] = None
    is_error: Optional[bool] = None


@dataclass
class WireSessionMessage:
    """Canonical transcript message."""
    role: str = ''
    content: Optional[WireContentInput] = None
    tool_calls: Optional[list[WireToolCall]] = None
    stop_reason: Optional[WireStopReason] = None
    blocks: Optional[list[WireAssistantBlock]] = None
    results: Optional[list[WireToolResult]] = None


@dataclass
class WireSessionHistory:
    """Paginated transcript page."""
    session_id: str = ''
    session_ref: Optional[str] = None
    message_count: int = 0
    offset: int = 0
    limit: Optional[int] = None
    has_more: bool = False
    messages: list[WireSessionMessage] = field(default_factory=list)


@dataclass
class WireEvent:
    """Event from agent execution stream."""
    session_id: str = ''
    sequence: int = 0
    event: Optional[dict] = None
    contract_version: str = ''


@dataclass
class CapabilityEntry:
    """A single capability status."""
    id: str = ''
    description: str = ''
    status: str = 'available'


@dataclass
class CapabilitiesResponse:
    """Response from capabilities/get."""
    contract_version: str = ''
    capabilities: list[CapabilityEntry] = field(default_factory=list)


@dataclass
class CommsParams:
    """Comms parameters (available because comms capability is compiled)."""
    keep_alive: Optional[bool] = None
    comms_name: Optional[str] = None

    peer_meta: Optional[dict[str, Any]] = None


@dataclass
class SkillsParams:
    """Skills parameters (available because skills capability is compiled)."""
    skills_enabled: bool = False
    skill_references: list[str] = field(default_factory=list)


@dataclass
class McpAddParams:
    """Request payload for `mcp/add`."""
    persisted: bool = False
    server_config: Any = None
    server_name: str = ''
    session_id: str = ''


@dataclass
class McpRemoveParams:
    """Request payload for `mcp/remove`."""
    persisted: bool = False
    server_name: str = ''
    session_id: str = ''


@dataclass
class McpReloadParams:
    """Request payload for optional `mcp/reload`."""
    persisted: bool = False
    server_name: Optional[str] = None
    session_id: str = ''


@dataclass
class MobWireParams:
    """Request payload for `mob/wire`."""
    member: str = ''
    mob_id: str = ''
    peer: dict[str, str] | dict[str, WireTrustedPeerSpec] = None


@dataclass
class MobUnwireParams:
    """Request payload for `mob/unwire`."""
    member: str = ''
    mob_id: str = ''
    peer: dict[str, str] | dict[str, WireTrustedPeerSpec] = None


@dataclass
class RuntimeStateParams:
    """Request payload for `runtime/state`."""
    session_id: str = ''


@dataclass
class RuntimeAcceptParams:
    """Request payload for `runtime/accept`."""
    input: Any = None
    session_id: str = ''


@dataclass
class RuntimeRetireParams:
    """Request payload for `runtime/retire`."""
    session_id: str = ''


@dataclass
class RuntimeResetParams:
    """Request payload for `runtime/reset`."""
    session_id: str = ''


@dataclass
class InputStateParams:
    """Request payload for `input/state`."""
    input_id: str = ''
    session_id: str = ''


@dataclass
class InputListParams:
    """Request payload for `input/list`."""
    session_id: str = ''


@dataclass
class ScheduleIdParams:
    """Request payload for schedule id lookups."""
    schedule_id: str = ''


@dataclass
class ListSchedulesParams:
    """Request payload for schedule/list."""
    labels: Optional[dict[str, Any]] = None
    limit: Optional[int] = None
    offset: Optional[int] = None


@dataclass
class ScheduleOccurrencesParams:
    """Request payload for schedule/occurrences."""
    include_terminal: Optional[bool] = None
    schedule_id: str = ''


@dataclass
class UpdateScheduleParams:
    """Request payload for schedule/update."""
    description: Optional[str] = None
    expected_revision: Optional[int] = None
    labels: Optional[dict[str, Any]] = None
    misfire_policy: Optional[dict[str, Any]] = None
    missing_target_policy: Optional[Literal['skip', 'mark_misfired']] = None
    name: Optional[str] = None
    overlap_policy: Optional[Literal['allow_concurrent', 'skip_if_running']] = None
    planning_horizon_days: Optional[int] = None
    planning_horizon_occurrences: Optional[int] = None
    schedule_id: str = ''
    target: Optional[dict[str, Any]] = None
    trigger: Optional[dict[str, Any]] = None


@dataclass
class McpLiveOpResponse:
    """Response payload for live MCP operations."""
    applied_at_turn: Optional[int] = None
    operation: Literal['add', 'remove', 'reload'] = None
    persisted: bool = False
    server_name: Optional[str] = None
    session_id: str = ''
    status: Literal['staged', 'applied', 'rejected'] = None


@dataclass
class WireRenderMetadata:
    """Public render metadata contract for mob member delivery."""
    class_: Literal['user_prompt', 'peer_message', 'peer_request', 'peer_response', 'external_event', 'flow_step', 'continuation', 'system_notice', 'tool_scope_notice', 'ops_progress'] = None
    salience: Optional[Literal['background', 'normal', 'important', 'urgent']] = None


@dataclass
class WireTrustedPeerSpec:
    """Minimal trusted peer spec for public mob wiring surfaces."""
    address: str = ''
    name: str = ''
    peer_id: str = ''


@dataclass
class MobWireResult:
    """Response payload for `mob/wire`."""
    wired: bool = False


@dataclass
class MobUnwireResult:
    """Response payload for `mob/unwire`."""
    unwired: bool = False


@dataclass
class RuntimeStateResult:
    """Response payload for `runtime/state`."""
    state: Literal['initializing', 'idle', 'attached', 'running', 'retired', 'stopped', 'destroyed'] = None


@dataclass
class RuntimeAcceptResult:
    """Response payload for `runtime/accept`."""
    existing_id: Optional[str] = None
    input_id: Optional[str] = None
    outcome_type: Literal['accepted', 'deduplicated', 'rejected'] = None
    policy: Any = None
    reason: Optional[str] = None
    state: Optional[dict[str, Any]] = None


@dataclass
class RuntimeRetireResult:
    """Response payload for `runtime/retire`."""
    inputs_abandoned: int = 0
    inputs_pending_drain: int = 0


@dataclass
class RuntimeResetResult:
    """Response payload for `runtime/reset`."""
    inputs_abandoned: int = 0


@dataclass
class WireInputStateHistoryEntry:
    """Input transition history entry for RPC-facing snapshots."""
    from_: Literal['accepted', 'queued', 'staged', 'applied', 'applied_pending_consumption', 'consumed', 'superseded', 'coalesced', 'abandoned'] = None
    reason: Optional[str] = None
    timestamp: str = ''
    to: Literal['accepted', 'queued', 'staged', 'applied', 'applied_pending_consumption', 'consumed', 'superseded', 'coalesced', 'abandoned'] = None


@dataclass
class WireInputState:
    """RPC-facing input state snapshot."""
    attempt_count: int = 0
    created_at: str = ''
    current_state: Literal['accepted', 'queued', 'staged', 'applied', 'applied_pending_consumption', 'consumed', 'superseded', 'coalesced', 'abandoned'] = None
    durability: Any = None
    history: list[dict[str, Any]] = field(default_factory=list)
    idempotency_key: Optional[str] = None
    input_id: str = ''
    last_boundary_sequence: Optional[int] = None
    last_run_id: Optional[str] = None
    persisted_input: Any = None
    policy: Any = None
    reconstruction_source: Any = None
    recovery_count: int = 0
    terminal_outcome: Any = None
    updated_at: str = ''


@dataclass
class InputListResult:
    """Response payload for `input/list`."""
    input_ids: list[str] = field(default_factory=list)


@dataclass
class ScheduleListResult:
    """Response payload for schedule/list."""
    schedules: list[dict[str, Any]] = field(default_factory=list)


@dataclass
class ScheduleOccurrencesResult:
    """Response payload for schedule/occurrences."""
    occurrences: list[dict[str, Any]] = field(default_factory=list)


@dataclass
class WireSessionInfo:
    """Canonical session info for wire protocol."""
    created_at: int = 0
    is_active: bool = False
    labels: dict[str, Any] = field(default_factory=dict)
    last_assistant_text: Optional[str] = None
    message_count: int = 0
    model: str = ''
    provider: str = ''
    session_id: str = ''
    session_ref: Optional[str] = None
    updated_at: int = 0


@dataclass
class WireSessionSummary:
    """Canonical session summary for wire protocol."""
    created_at: int = 0
    is_active: bool = False
    labels: dict[str, Any] = field(default_factory=dict)
    message_count: int = 0
    session_id: str = ''
    session_ref: Optional[str] = None
    total_tokens: int = 0
    updated_at: int = 0


@dataclass
class ContractVersion:
    """Semantic contract version triple."""


@dataclass
class CatalogModelEntry:
    """A single model entry in the catalog response."""
    context_window: Optional[int] = None
    display_name: str = ''
    id: str = ''
    max_output_tokens: Optional[int] = None
    profile: Optional[dict[str, Any]] = None
    server_id: Optional[str] = None
    tier: str = ''


@dataclass
class ProviderCatalog:
    """Provider-level grouping in the catalog response."""
    default_model_id: str = ''
    models: list[dict[str, Any]] = field(default_factory=list)
    provider: str = ''


@dataclass
class ModelsCatalogResponse:
    """Response for `models/catalog` — the compiled-in model catalog."""
    contract_version: dict[str, Any] = field(default_factory=dict)
    providers: list[dict[str, Any]] = field(default_factory=list)


@dataclass
class WireModelProfile:
    """Runtime profile for a model — capabilities and parameter schema."""
    inline_video: bool = False
    model_family: str = ''
    params_schema: Any = None
    supports_reasoning: bool = False
    supports_temperature: bool = False
    supports_thinking: bool = False
    supports_web_search: bool = False


# Wire-safe content block (no `source_path` — internal only).
WireContentBlock = dict[str, Any]

# Wire-safe content input (mirrors `ContentInput`).
WireContentInput = str | list[dict[str, Any]]

# Shared operation kind for live MCP operations.
McpLiveOperation = Literal['add', 'remove', 'reload']

# Shared status for live MCP operations.
McpLiveOpStatus = Literal['staged', 'applied', 'rejected']

# Target for a mob wire/unwire call.
MobPeerTarget = dict[str, str] | dict[str, WireTrustedPeerSpec]

# Public handling mode for mob member delivery.
WireHandlingMode = Literal['queue', 'steer']

# Public render class contract for mob member delivery.
WireRenderClass = Literal['user_prompt', 'peer_message', 'peer_request', 'peer_response', 'external_event', 'flow_step', 'continuation', 'system_notice', 'tool_scope_notice', 'ops_progress']

# Public render salience contract for mob member delivery.
WireRenderSalience = Literal['background', 'normal', 'important', 'urgent']

# Public runtime state projection used by RPC surfaces.
WireRuntimeState = Literal['initializing', 'idle', 'attached', 'running', 'retired', 'stopped', 'destroyed']

# Discriminator for `runtime/accept` responses.
RuntimeAcceptOutcomeType = Literal['accepted', 'deduplicated', 'rejected']

# Public input lifecycle state projection used by RPC surfaces.
WireInputLifecycleState = Literal['accepted', 'queued', 'staged', 'applied', 'applied_pending_consumption', 'consumed', 'superseded', 'coalesced', 'abandoned']

# Canonical stop reason for transcript messages.
WireStopReason = Literal['end_turn', 'tool_use', 'max_tokens', 'stop_sequence', 'content_filter', 'cancelled']

# Wire-safe tool result content that handles both legacy string and array formats.
WireToolResultContent = str | list[dict[str, Any]]

# Model recommendation tier.
WireModelTier = str

# Response payload for `input/state`.
InputStateResult = Optional[WireInputState]
