"""Generated wire types for Meerkat SDK.

Contract version: 0.4.0
"""

from dataclasses import dataclass, field
from typing import Any, Optional


CONTRACT_VERSION = "0.4.0"


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
class McpLiveOpResponse:
    """Response payload for live MCP operations."""
    applied_at_turn: Optional[int] = None
    operation: str = ''
    persisted: bool = False
    server_name: Optional[str] = None
    session_id: str = ''
    status: str = ''

