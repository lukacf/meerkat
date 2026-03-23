from __future__ import annotations

"""Generated wire types for Meerkat SDK.

Contract version: 0.4.13
"""

from dataclasses import dataclass, field
from typing import Any, Literal, Optional


CONTRACT_VERSION = "0.4.13"


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
    schema_warnings: Optional[list] = None


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
    content: str = ''
    is_error: Optional[bool] = None


@dataclass
class WireSessionMessage:
    """Canonical transcript message."""
    role: str = ''
    content: Optional[str] = None
    tool_calls: Optional[list] = None
    stop_reason: Optional[str] = None
    blocks: Optional[list] = None
    results: Optional[list] = None


@dataclass
class WireSessionHistory:
    """Paginated transcript page."""
    session_id: str = ''
    session_ref: Optional[str] = None
    message_count: int = 0
    offset: int = 0
    limit: Optional[int] = None
    has_more: bool = False
    messages: list = field(default_factory=list)


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
    capabilities: list = field(default_factory=list)


@dataclass
class CommsParams:
    """Comms parameters (available because comms capability is compiled)."""
    host_mode: bool = False
    comms_name: Optional[str] = None

    peer_meta: Optional[dict[str, Any]] = None


@dataclass
class SkillsParams:
    """Skills parameters (available because skills capability is compiled)."""
    skills_enabled: bool = False
    skill_references: list = field(default_factory=list)


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
class MobSendParams:
    """Request payload for `mob/send`."""
    content: WireContentInput = None
    handling_mode: Optional[WireHandlingMode] = None
    meerkat_id: str = ''
    mob_id: str = ''
    render_metadata: Optional[WireRenderMetadata] = None


@dataclass
class MobWireParams:
    """Request payload for `mob/wire`."""
    member: str = ''
    mob_id: str = ''
    peer: MobPeerTarget = None


@dataclass
class MobUnwireParams:
    """Request payload for `mob/unwire`."""
    member: str = ''
    mob_id: str = ''
    peer: MobPeerTarget = None


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
class McpLiveOpResponse:
    """Response payload for live MCP operations."""
    applied_at_turn: Optional[int] = None
    operation: McpLiveOperation = None
    persisted: bool = False
    server_name: Optional[str] = None
    session_id: str = ''
    status: McpLiveOpStatus = None


@dataclass
class WireRenderMetadata:
    """Public render metadata contract for mob member delivery."""
    class_: WireRenderClass = None
    salience: Optional[WireRenderSalience] = None


@dataclass
class MobSendResult:
    """Response payload for `mob/send`."""
    handling_mode: WireHandlingMode = None
    member_id: str = ''
    session_id: str = ''


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
    state: WireRuntimeState = None


@dataclass
class RuntimeAcceptResult:
    """Response payload for `runtime/accept`."""
    existing_id: Optional[str] = None
    input_id: Optional[str] = None
    outcome_type: RuntimeAcceptOutcomeType = None
    policy: Any = None
    reason: Optional[str] = None
    state: Optional[WireInputState] = None


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
    from_: WireInputLifecycleState = None
    reason: Optional[str] = None
    timestamp: str = ''
    to: WireInputLifecycleState = None


@dataclass
class WireInputState:
    """RPC-facing input state snapshot."""
    attempt_count: int = 0
    created_at: str = ''
    current_state: WireInputLifecycleState = None
    durability: Any = None
    history: list[WireInputStateHistoryEntry] = field(default_factory=list)
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


# Wire-safe content block (no `source_path` — internal only).
WireContentBlock = dict[str, Any]

# Wire-safe content input (mirrors `ContentInput`).
WireContentInput = str | list[WireContentBlock]

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
WireRuntimeState = Literal['initializing', 'idle', 'attached', 'running', 'recovering', 'retired', 'stopped', 'destroyed']

# Discriminator for `runtime/accept` responses.
RuntimeAcceptOutcomeType = Literal['accepted', 'deduplicated', 'rejected']

# Public input lifecycle state projection used by RPC surfaces.
WireInputLifecycleState = Literal['accepted', 'queued', 'staged', 'applied', 'applied_pending_consumption', 'consumed', 'superseded', 'coalesced', 'abandoned']

# Response payload for `input/state`.
InputStateResult = Optional[WireInputState]
