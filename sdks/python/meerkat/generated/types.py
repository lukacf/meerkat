from __future__ import annotations

"""Generated wire types for Meerkat SDK.

Contract version: 0.8.6
"""

from dataclasses import dataclass, field
from typing import Any, Literal, NotRequired, Optional, Required, TypedDict

from .errors import MeerkatError


CONTRACT_VERSION = "0.8.6"


Value = Any


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
    terminal_cause_kind: Optional[str] = None
    structured_output: Optional[Any] = None
    extraction_error: Optional[dict[str, Any]] = None
    schema_warnings: Optional[list[Any]] = None
    skill_diagnostics: Optional[dict] = None


@dataclass
class WireToolResult:
    """Tool result transcript item."""
    tool_use_id: str = ''
    content: Optional[WireToolResultContent] = None
    is_error: Optional[bool] = None


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
    skill_refs: list[dict[str, str]] = field(default_factory=list)



def _wire_parse_error(context: str, message: str) -> MeerkatError:
    """INVALID_RESPONSE error for the generated fail-closed wire parsers (K21)."""
    return MeerkatError("INVALID_RESPONSE", f"invalid {context}: {message}")


def _expect_wire_object(value: Any, context: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise _wire_parse_error(context, "expected object")
    return value


def _require_wire_field(data: dict[str, Any], key: str, context: str) -> Any:
    if data.get(key) is None:
        raise _wire_parse_error(context, f"missing required field `{key}`")
    return data[key]


def _expect_wire_str(value: Any, context: str) -> str:
    if not isinstance(value, str):
        raise _wire_parse_error(context, "expected string")
    return value


def _expect_wire_bool(value: Any, context: str) -> bool:
    if not isinstance(value, bool):
        raise _wire_parse_error(context, "expected boolean")
    return value


def _expect_wire_int(value: Any, context: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int):
        raise _wire_parse_error(context, "expected integer")
    return value


def _expect_wire_number(value: Any, context: str) -> float:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise _wire_parse_error(context, "expected number")
    return float(value)


def _expect_wire_list(value: Any, context: str) -> list[Any]:
    if not isinstance(value, list):
        raise _wire_parse_error(context, "expected array")
    return value


def _expect_wire_enum(value: Any, values: tuple[str, ...], context: str) -> Any:
    if not isinstance(value, str) or value not in values:
        raise _wire_parse_error(context, f"expected one of {list(values)}")
    return value


def _expect_wire_const(value: Any, expected: Any, context: str) -> Any:
    if value != expected:
        raise _wire_parse_error(context, f"expected constant `{expected}`")
    return value



@dataclass
class McpStdioConfig:
    """Stdio transport configuration"""
    command: str
    args: Optional[list[str]] = None
    env: Optional[dict[str, str]] = None


@dataclass
class McpHttpConfig:
    """HTTP transport configuration (streamable HTTP or legacy SSE)"""
    url: str
    headers: Optional[dict[str, str]] = None
    transport: Optional[McpHttpTransport] = None


class McpStdioServerConfig(TypedDict, total=False):
    """Typed stdio variant for MCP server configuration."""
    name: Required[str]
    command: Required[str]
    args: NotRequired[list[str]]
    env: NotRequired[dict[str, str]]
    connect_timeout_secs: NotRequired[int]


class McpHttpServerConfig(TypedDict, total=False):
    """Typed HTTP variant for MCP server configuration."""
    name: Required[str]
    url: Required[str]
    headers: NotRequired[dict[str, str]]
    transport: NotRequired[McpHttpTransport]
    connect_timeout_secs: NotRequired[int]


McpServerConfig = McpStdioServerConfig | McpHttpServerConfig

@dataclass
class McpAddParams:
    """Request payload for `mcp/add`."""
    server_config: McpServerConfig
    session_id: str
    persisted: Optional[bool] = None


@dataclass
class McpRemoveParams:
    """Request payload for `mcp/remove`."""
    server_name: str
    session_id: str
    persisted: Optional[bool] = None


@dataclass
class McpReloadParams:
    """Request payload for optional `mcp/reload`."""
    session_id: str
    persisted: Optional[bool] = None
    server_name: Optional[str] = None


@dataclass
class McpLiveOpResponse:
    """Response payload for live MCP operations."""
    operation: Literal['add', 'remove', 'reload']
    persisted: bool
    session_id: str
    status: Literal['staged', 'applied', 'rejected']
    applied_at_turn: Optional[int] = None
    server_name: Optional[str] = None


# Canonical skill slug (lowercase, dash-separated).
SkillName = str

# Where a skill was discovered.
SkillScope = Literal['builtin', 'project', 'user']

# Source lifecycle status in the identity registry.
SourceIdentityStatus = Literal['active', 'disabled', 'retired']

# Source transport class used for identity governance.
SourceTransportKind = Literal['embedded', 'filesystem', 'git', 'http', 'stdio']

# Canonical source identifier.
SourceUuid = str

# Canonical typed name for an LLM-callable tool.
#
# The model-facing wire shape is still the provider's string tool name, but
# internal policy, routing, and registry surfaces carry this handle so raw
# strings do not become the tool identity owner.
ToolName = str

@dataclass
class SkillEntry:
    """Wire representation of a skill entry (for list responses)."""
    description: str
    is_active: bool
    key: SkillKey
    name: str
    scope: SkillScope
    source: SkillSourceProvenance
    shadowed_by: Optional[SkillSourceProvenance] = None


@dataclass
class SkillKey:
    """Canonical runtime identity for a skill.

This is the single identity carried across every surface — the wire parses
directly into this struct, tools receive this struct, the registry stores
this struct. There is no slash-delimited string path form."""
    skill_name: SkillName
    source_uuid: SourceUuid


@dataclass
class SkillSourceProvenance:
    """Typed source provenance for a skill entry. `display_name` is presentation
metadata only; `source_uuid` is the stable identity."""
    display_name: str
    fingerprint: str
    source_uuid: SourceUuid
    transport_kind: SourceTransportKind
    status: Optional[SourceIdentityStatus] = None


# Reason a live channel was skipped during config propagation.
class WireLiveHotSwapSkipReasonNoOpOrOverride(TypedDict, total=False):
    kind: Required[Literal['no_op_or_override']]

class WireLiveHotSwapSkipReasonIdentityLookupFailed(TypedDict, total=False):
    error: Required[str]
    kind: Required[Literal['identity_lookup_failed']]

WireLiveHotSwapSkipReason = WireLiveHotSwapSkipReasonNoOpOrOverride | WireLiveHotSwapSkipReasonIdentityLookupFailed

# Why a live channel refresh failed during config propagation.
class WireLiveChannelRefreshFailureOpenConfigBuildFailed(TypedDict, total=False):
    error: Required[str]
    kind: Required[Literal['open_config_build_failed']]

class WireLiveChannelRefreshFailureSnapshotVersionFailed(TypedDict, total=False):
    error: Required[str]
    kind: Required[Literal['snapshot_version_failed']]

class WireLiveChannelRefreshFailureEnqueueFailed(TypedDict, total=False):
    error: Required[str]
    kind: Required[Literal['enqueue_failed']]

class WireLiveChannelRefreshFailureQueueAcceptanceRejected(TypedDict, total=False):
    error: Required[str]
    kind: Required[Literal['queue_acceptance_rejected']]

WireLiveChannelRefreshFailure = WireLiveChannelRefreshFailureOpenConfigBuildFailed | WireLiveChannelRefreshFailureSnapshotVersionFailed | WireLiveChannelRefreshFailureEnqueueFailed | WireLiveChannelRefreshFailureQueueAcceptanceRejected

# Why a live channel close failed during config propagation.
class WireLiveChannelCloseFailureSignalFailed(TypedDict, total=False):
    error: Required[str]
    kind: Required[Literal['signal_failed']]

class WireLiveChannelCloseFailureCloseAuthorityRejected(TypedDict, total=False):
    error: Required[str]
    kind: Required[Literal['close_authority_rejected']]

class WireLiveChannelCloseFailureCommitHandoffMissing(TypedDict, total=False):
    kind: Required[Literal['commit_handoff_missing']]

class WireLiveChannelCloseFailureHostCommitFailed(TypedDict, total=False):
    error: Required[str]
    kind: Required[Literal['host_commit_failed']]

WireLiveChannelCloseFailure = WireLiveChannelCloseFailureSignalFailed | WireLiveChannelCloseFailureCloseAuthorityRejected | WireLiveChannelCloseFailureCommitHandoffMissing | WireLiveChannelCloseFailureHostCommitFailed

@dataclass
class WireLiveHotSwapSkip:
    """A live channel that was skipped during config propagation."""
    reason: WireLiveHotSwapSkipReason
    session_id: str


@dataclass
class WireLiveSwapFailure:
    """A live channel whose hot-swap failed during config propagation."""
    error: str
    session_id: str


@dataclass
class WireLiveRefreshFailure:
    """A live channel whose refresh failed during config propagation."""
    failure: WireLiveChannelRefreshFailure
    session_id: str


@dataclass
class WireLiveCloseFailure:
    """A live channel whose close failed during config propagation."""
    failure: WireLiveChannelCloseFailure
    session_id: str


@dataclass
class WireLiveConfigPropagationReport:
    """Typed wire projection of the live-config propagation report."""
    clean: bool
    close_failed: list[WireLiveCloseFailure]
    closed: list[str]
    refresh_failed: list[WireLiveRefreshFailure]
    refreshed: list[str]
    skipped: list[WireLiveHotSwapSkip]
    swap_failed: list[WireLiveSwapFailure]
    swapped: list[str]


@dataclass
class CallbackToolDefinition:
    """Public callback tool definition accepted by `tools/register`.

The server stamps callback provenance itself; clients provide only the
model-facing definition."""
    description: str
    input_schema: Any
    name: ToolName


@dataclass
class ConfigEnvelope:
    """Wire envelope returned by config APIs across surfaces."""
    config: Any
    generation: int
    backend: Optional[str] = None
    instance_id: Optional[str] = None
    realm_id: Optional[str] = None
    resolved_paths: Optional[dict[str, Any]] = None


@dataclass
class ConfigPatchParams:
    """Parameters for the RPC `config/patch` method — RFC-7386 merge-patch
(bare patch or wrapped with an optimistic-concurrency generation)."""
    expected_generation: Optional[int] = None
    patch: Optional[Any] = None


@dataclass
class ConfigWriteResult:
    """Result of a `config/set` or `config/patch` write.

`cfg`-gated off wasm32 alongside `meerkat_core::config_runtime`, which
owns the embedded envelope (the browser runtime serves no config writes)."""
    config: Any
    generation: int
    backend: Optional[str] = None
    instance_id: Optional[str] = None
    live_propagation: Optional[WireLiveConfigPropagationReport] = None
    realm_id: Optional[str] = None
    resolved_paths: Optional[dict[str, Any]] = None


@dataclass
class InterruptResult:
    """Shared interrupt result wire contract (REST and RPC)."""
    interrupted: bool
    result: Literal['interrupted', 'staged_noop']
    session_id: str


@dataclass
class ServerCapabilities:
    """Capabilities returned by the RPC server during `initialize`."""
    contract_version: str
    methods: list[str]
    server_info: dict[str, Any]


@dataclass
class SkillListResponse:
    """Wire response for listing skills with introspection data."""
    skills: list[SkillEntry]


@dataclass
class ToolsRegisterParams:
    """Parameters for `tools/register`."""
    tools: list[CallbackToolDefinition]


@dataclass
class ToolsRegisterResult:
    """Result for `tools/register`."""
    registered: int


@dataclass
class WorkEventsResult:
    """Result envelope for work-event list reads (`workgraph/events`)."""
    events: list[Any]

    @classmethod
    def from_wire(cls, value: Any) -> "WorkEventsResult":
        """Fail-closed wire parser (K21): raises MeerkatError
        (INVALID_RESPONSE) on missing or mistyped fields.
        """
        data = _expect_wire_object(value, 'WorkEventsResult')
        return cls(
            events=list(_expect_wire_list(_require_wire_field(data, 'events', 'WorkEventsResult'), 'WorkEventsResult.events')),
        )


@dataclass
class WorkItemsResult:
    """Result envelope for work-item list reads (`workgraph/list`, `workgraph/ready`)."""
    items: list[Any]

    @classmethod
    def from_wire(cls, value: Any) -> "WorkItemsResult":
        """Fail-closed wire parser (K21): raises MeerkatError
        (INVALID_RESPONSE) on missing or mistyped fields.
        """
        data = _expect_wire_object(value, 'WorkItemsResult')
        return cls(
            items=list(_expect_wire_list(_require_wire_field(data, 'items', 'WorkItemsResult'), 'WorkItemsResult.items')),
        )


# Parameters for the RPC `config/set` method — replace the config (bare
# config or wrapped with an optimistic-concurrency generation).
ConfigSetParams = dict[str, Any]

# Typed event envelope for the generic `session/external_event` and
# `/sessions/{id}/external-events` surfaces.
#
# Not `PartialEq`: the `GenericJson.payload` and
# `PeerResponseTerminal.result` bodies ride as `Box<RawValue>` — opaque
# caller-supplied JSON that never gets pattern-matched at this layer.
# Allow-listed per `dogma-blind-spots` §7.
class SessionExternalEventEnvelopeGenericJson(TypedDict, total=False):
    blocks: NotRequired[Optional[list[WireContentBlock]]]
    event_type: Required[str]
    kind: Required[Literal['generic_json']]
    payload: Required[Any]

class SessionExternalEventEnvelopePeerResponseTerminal(TypedDict, total=False):
    display_name: NotRequired[Optional[PeerName]]
    kind: Required[Literal['peer_response_terminal']]
    peer_id: Required[PeerId]
    request_id: Required[str]
    result: Required[Any]
    status: Required[Literal['completed', 'failed', 'cancelled']]

SessionExternalEventEnvelope = SessionExternalEventEnvelopeGenericJson | SessionExternalEventEnvelopePeerResponseTerminal

# `auth/login/device_complete` success body.
class WireDeviceCompleteResultPending(TypedDict, total=False):
    state: Required[Literal['pending']]

class WireDeviceCompleteResultSlowDown(TypedDict, total=False):
    state: Required[Literal['slow_down']]

class WireDeviceCompleteResultAccessDenied(TypedDict, total=False):
    state: Required[Literal['access_denied']]

class WireDeviceCompleteResultExpired(TypedDict, total=False):
    state: Required[Literal['expired']]

class WireDeviceCompleteResultReady(TypedDict, total=False):
    auth_binding: Required[WireAuthBindingRef]
    binding_id: Required[str]
    expires_at: NotRequired[Optional[str]]
    has_refresh_token: Required[bool]
    profile_id: Required[str]
    provider: Required[str]
    realm_id: Required[str]
    scopes: Required[list[str]]
    state: Required[Literal['ready']]

WireDeviceCompleteResult = WireDeviceCompleteResultPending | WireDeviceCompleteResultSlowDown | WireDeviceCompleteResultAccessDenied | WireDeviceCompleteResultExpired | WireDeviceCompleteResultReady

@dataclass
class ApprovalDecideParams:
    """`approval/decide` parameters."""
    actor: str
    approval_id: str
    decision: Literal['approve', 'deny']
    provenance: Optional[Any] = None
    reason: Optional[str] = None


@dataclass
class ApprovalGetParams:
    """`approval/get` parameters."""
    approval_id: str


@dataclass
class ApprovalListParams:
    """`approval/list` parameters."""
    filter: Optional[dict[str, Any]] = None


@dataclass
class ApprovalListResult:
    """`approval/list` result."""
    approvals: list[dict[str, Any]]


@dataclass
class ApprovalRecord:
    """Durable approval record."""
    allowed_decisions: list[Literal['approve', 'deny']]
    approval_id: str
    created_at: str
    metadata: dict[str, Any]
    owner: dict[str, Any]
    proposed_action: dict[str, Any]
    requester: str
    resource: dict[str, Any]
    risk: Literal['low', 'medium', 'high', 'critical']
    status: Literal['pending', 'approved', 'denied', 'expired', 'cancelled']
    updated_at: str
    decision: Optional[dict[str, Any]] = None
    expires_at: Optional[str] = None
    request_body: Optional[Any] = None
    request_provenance: Optional[Any] = None


@dataclass
class ApprovalRequestParams:
    """`approval/request` parameters."""
    allowed_decisions: list[Literal['approve', 'deny']]
    owner: dict[str, Any]
    proposed_action: dict[str, Any]
    requester: str
    resource: dict[str, Any]
    risk: Literal['low', 'medium', 'high', 'critical']
    expires_at: Optional[str] = None
    metadata: Optional[dict[str, Any]] = None
    request_body: Optional[Any] = None
    request_provenance: Optional[Any] = None


@dataclass
class ArchiveSessionParams:
    """Parameters for `session/archive`."""
    session_id: str


@dataclass
class ArtifactDownloadParams:
    """Request payload for ArtifactDownloadParams."""
    artifact_id: str
    expected_media_type: Optional[str] = None


@dataclass
class ArtifactDownloadResult:
    """Wire payload for ArtifactDownloadResult."""
    payload: dict[str, Any]
    record: dict[str, Any]


@dataclass
class ArtifactIdParams:
    """Request payload for ArtifactIdParams."""
    artifact_id: str


@dataclass
class ArtifactListParams:
    """Request payload for ArtifactListParams."""
    label_equals: Optional[dict[str, str]] = None
    session_id: Optional[str] = None


@dataclass
class ArtifactListResult:
    """Wire payload for ArtifactListResult."""
    artifacts: list[dict[str, Any]]


@dataclass
class ArtifactRecord:
    """Stable artifact record."""
    artifact_id: str
    artifact_type: Literal['text', 'log', 'command_output', 'diff', 'patch', 'generated_file', 'test_report', 'screenshot', 'image', 'json', 'binary'] | dict[str, str]
    content_handle: BlobRef | dict[str, Any]
    created_at: str
    handle: dict[str, Any]
    media_type: str
    size_bytes: int
    title: str
    hash: Optional[str] = None
    metadata: Optional[dict[str, Any]] = None
    owner: Optional[dict[str, Any]] = None
    producer: Optional[str] = None
    provenance: Optional[dict[str, str]] = None


@dataclass
class BindingIdParams:
    """Request payload for binding-scoped auth methods."""
    binding_id: str
    realm_id: str
    profile_id: Optional[str] = None


@dataclass
class BlobGetParams:
    """Parameters for `blob/get`."""
    blob_id: str


@dataclass
class BlobPayload:
    """Resolved blob bytes returned by the blob store."""
    blob_id: BlobId
    data: str
    media_type: str


@dataclass
class CreateProfileParams:
    """Request payload for `auth/profile/create`."""
    auth_method: str
    binding_id: str
    realm_id: str
    secret: str
    profile_id: Optional[str] = None


@dataclass
class CreateScheduleRequest:
    """Request payload for CreateScheduleRequest."""
    target: dict[str, Any]
    trigger: dict[str, Any]
    description: Optional[str] = None
    labels: Optional[dict[str, str]] = None
    misfire_policy: Optional[dict[str, Any]] = None
    missing_target_policy: Optional[Literal['skip', 'mark_misfired']] = None
    name: Optional[str] = None
    overlap_policy: Optional[Literal['allow_concurrent', 'skip_if_running']] = None
    planning_horizon_days: Optional[int] = None
    planning_horizon_occurrences: Optional[int] = None


@dataclass
class DeferredCreateResult:
    """Result for deferred `session/create` (no turn executed)."""
    session_id: str
    session_ref: Optional[str] = None


@dataclass
class DeviceCompleteParams:
    """Request payload for `auth/login/device_complete`."""
    binding_id: str
    device_code: str
    provider: str
    realm_id: str
    profile_id: Optional[str] = None


@dataclass
class DeviceStartParams:
    """Request payload for `auth/login/device_start`."""
    binding_id: str
    provider: str
    realm_id: str
    profile_id: Optional[str] = None


@dataclass
class EventsLatestCursorParams:
    """Parameters for `events/latest_cursor`."""
    scope: dict[str, Any]


@dataclass
class EventsLatestCursorResult:
    """Result for `events/latest_cursor`."""
    contract_version: ContractVersion
    cursor: dict[str, Any]


@dataclass
class EventsListSinceParams:
    """Parameters for `events/list_since`."""
    scope: dict[str, Any]
    cursor: Optional[dict[str, Any]] = None
    limit: Optional[int] = None


@dataclass
class EventsListSinceResult:
    """Result for `events/list_since`."""
    contract_version: ContractVersion
    from_cursor: dict[str, Any]
    has_more: bool
    latest_cursor: dict[str, Any]
    scope: dict[str, Any]
    events: Optional[list[dict[str, Any]]] = None


@dataclass
class EventsSnapshotParams:
    """Parameters for `events/snapshot`."""
    scope: dict[str, Any]


@dataclass
class EventsSnapshotResult:
    """Result for `events/snapshot`."""
    contract_version: ContractVersion
    cursor: dict[str, Any]
    scope: dict[str, Any]
    snapshot: dict[str, Any]


@dataclass
class ForkSessionAtParams:
    """Request payload for `session/fork_at`."""
    message_index: int
    session_id: str
    running_behavior: Optional[Literal['reject']] = None
    tool_access_policy: Optional[WireToolAccessPolicy] = None


@dataclass
class ForkSessionReplaceParams:
    """Request payload for `session/fork_replace`.

`replacement` rides as the typed [`WireTranscriptReplacement`] mirror so
the emitted JSON schema is the closed replacement enum rather than a bare
`serde_json::Value`. The consuming surface lowers it into the core
[`TranscriptReplacement`] via [`WireTranscriptReplacement::into_core`]."""
    message_index: int
    replacement: WireTranscriptReplacement
    session_id: str
    running_behavior: Optional[Literal['reject']] = None
    tool_access_policy: Optional[WireToolAccessPolicy] = None


@dataclass
class HelpRequest:
    """Request body for dedicated help surfaces."""
    question: str
    execution_mode: Optional[Literal['explain_only', 'plan_execution']] = None
    max_tokens: Optional[int] = None
    model: Optional[str] = None
    prompt: Optional[str] = None
    provider: Optional[str] = None


@dataclass
class HelpResponse:
    """Canonical run result for wire protocol."""
    session_id: str
    text: str
    tool_calls: int
    turns: int
    usage: dict[str, Any]
    extraction_error: Optional[dict[str, Any]] = None
    schema_warnings: Optional[list[dict[str, Any]]] = None
    session_ref: Optional[str] = None
    skill_diagnostics: Optional[dict[str, Any]] = None
    structured_output: Optional[Any] = None
    terminal_cause_kind: Optional[Literal['unknown', 'hook_denied', 'hook_failure', 'llm_failure', 'tool_failure', 'structured_output_validation_failed', 'budget_exhausted', 'time_budget_exceeded', 'retry_exhausted', 'turn_limit_reached', 'runtime_apply_failure', 'fatal_failure']] = None


@dataclass
class InjectSystemContextParams:
    """Parameters for `session/inject_context`.

The injected body is the typed [`CoreRenderable`] owner rather than a bare
`text` string: surfaces parse their inbound payload into the renderable at
the ingress boundary and the handler threads it straight through to
`AppendSystemContextRequest.content`. A plain-text client payload still
deserializes via `CoreRenderable`'s tagged `text` variant."""
    content: dict[str, Any]
    session_id: str
    idempotency_key: Optional[str] = None
    source: Optional[str] = None


@dataclass
class InjectSystemContextResult:
    """Result for `session/inject_context`."""
    status: Literal['applied', 'staged', 'duplicate']


@dataclass
class InterruptParams:
    """Parameters for `turn/interrupt`."""
    session_id: str


@dataclass
class ListSessionTranscriptRevisionsParams:
    """Request payload for `session/transcript_revisions`."""
    session_id: str
    limit: Optional[int] = None
    offset: Optional[int] = None


@dataclass
class ListSessionsParams:
    """Parameters for `session/list` (all optional)."""
    labels: Optional[dict[str, str]] = None
    limit: Optional[int] = None
    offset: Optional[int] = None


@dataclass
class ListSessionsResult:
    """Result for `session/list`."""
    sessions: list[dict[str, Any]]


@dataclass
class LoginCompleteParams:
    """Request payload for `auth/login/complete`."""
    binding_id: str
    code: str
    provider: str
    realm_id: str
    redirect_uri: str
    state: str
    profile_id: Optional[str] = None


@dataclass
class LoginStartParams:
    """Request payload for `auth/login/start`."""
    binding_id: str
    provider: str
    realm_id: str
    redirect_uri: str
    profile_id: Optional[str] = None


@dataclass
class ProvisionApiKeyParams:
    """Request payload for `auth/login/provision_api_key`."""
    access_token: str
    binding_id: Optional[str] = None
    profile_id: Optional[str] = None
    realm_id: Optional[str] = None


@dataclass
class ReadSessionHistoryParams:
    """Parameters for `session/history`."""
    session_id: str
    limit: Optional[int] = None
    offset: Optional[int] = None


@dataclass
class ReadSessionParams:
    """Parameters for `session/read`."""
    session_id: str


@dataclass
class ReadSessionTranscriptRevisionParams:
    """Request payload for `session/transcript_revision`."""
    revision: str
    session_id: str
    limit: Optional[int] = None
    offset: Optional[int] = None


@dataclass
class SessionInputStateParams:
    """Parameters for `session/input_status`."""
    selector: dict[str, Any]
    session_id: str


@dataclass
class SessionInputStateSelector:
    """Typed selector for `session/input_status`: exactly one lookup key."""


@dataclass
class SessionInputStateResult:
    """Result for `session/input_status`."""
    state: Optional[dict[str, Any]] = None


@dataclass
class RealmIdParams:
    """Request payload for `auth/profile/list` and `realm/get`."""
    realm_id: str


@dataclass
class RestoreSessionTranscriptRevisionParams:
    """Request payload for `session/restore_transcript_revision`."""
    reason: TranscriptRewriteReason
    revision: str
    session_id: str
    actor: Optional[str] = None
    expected_parent_revision: Optional[str] = None
    running_behavior: Optional[Literal['reject']] = None


@dataclass
class RewriteSessionTranscriptParams:
    """Request payload for `session/rewrite_transcript`."""
    reason: TranscriptRewriteReason
    replacement: list[TranscriptRewriteMessage]
    selection: TranscriptRewriteSelection
    session_id: str
    actor: Optional[str] = None
    expected_parent_revision: Optional[str] = None
    running_behavior: Optional[Literal['reject']] = None


@dataclass
class RuntimeHostCapabilities:
    """Runtime capability surface for a host."""
    contract_version: ContractVersion
    features: RuntimeHostFeatureFlags


@dataclass
class RuntimeHostFeatureFlags:
    """Existing host capability facts exposed as typed booleans."""
    approvals: bool
    artifacts: bool
    blobs: bool
    comms: bool
    event_replay: bool
    external_members: bool
    mcp_live: bool
    mobs: bool
    runtime_backed_sessions: bool
    schedules: bool
    secure_remote_rpc: bool
    session_events: bool
    session_streams: bool
    skills: bool
    multi_host_mobs: Optional[bool] = None


@dataclass
class RuntimeHostHealth:
    """Runtime health projection for a host."""
    contract_version: ContractVersion
    status: Literal['ok', 'degraded', 'unhealthy']
    checks: Optional[dict[str, Literal['ok', 'degraded', 'unhealthy']]] = None


@dataclass
class RuntimeHostInfo:
    """Read-only host information. This is a projection, not a registry entry."""
    capabilities: RuntimeHostCapabilities
    contract_version: ContractVersion
    endpoints: dict[str, Any]
    health: RuntimeHostHealth
    host_id: str
    host_id_scope: Literal['process', 'realm_instance']
    process_name: str
    process_version: str
    realm: dict[str, Any]
    placement_labels: Optional[dict[str, str]] = None
    policy_profile_summary: Optional[str] = None


@dataclass
class Schedule:
    """Canonical public projection returned by every `schedule/*` RPC method.

The durable schedule domain object has a custom persistence serializer
which includes generated machine authority state. That persistence shape
must not double as the public contract. This projection deliberately keeps
the existing flattened schedule configuration while excluding the private
machine state, and it is the single source for both production responses
and generated SDK schemas."""
    created_at_utc: str
    misfire_policy: dict[str, Any]
    missing_target_policy: Literal['skip', 'mark_misfired']
    next_occurrence_ordinal: int
    overlap_policy: Literal['allow_concurrent', 'skip_if_running']
    phase: Literal['active', 'paused', 'deleted']
    planning_horizon_days: int
    planning_horizon_occurrences: int
    revision: int
    schedule_id: str
    target: dict[str, Any]
    trigger: dict[str, Any]
    updated_at_utc: str
    deleted_at_utc: Optional[str] = None
    description: Optional[str] = None
    labels: Optional[dict[str, str]] = None
    name: Optional[str] = None
    planning_cursor_utc: Optional[str] = None
    superseded_ack_ids: Optional[list[str]] = None


@dataclass
class ScheduleToolCallParams:
    """Parameters for `schedule/call`."""
    name: str
    arguments: Optional[Any] = None


@dataclass
class ScheduleToolsResult:
    """Result for `schedule/tools`."""
    tools: list[Any]


@dataclass
class SessionForkResult:
    """Result of creating an edited transcript branch."""
    message_count: int
    session_id: str
    source_session_id: str
    session_ref: Optional[str] = None


@dataclass
class SessionPeerResponseTerminalParams:
    """Dedicated request payload for `session/peer_response_terminal`.

Not `PartialEq`: `result` is opaque peer-returned bytes carried as
`Box<RawValue>` (pass-through fidelity — the peer is the typed
authority over its own payload shape). Allow-listed per
`dogma-blind-spots` §7 alongside tool-call args."""
    peer_id: PeerId
    request_id: str
    result: Any
    session_id: str
    status: Literal['completed', 'failed', 'cancelled']
    display_name: Optional[PeerName] = None


@dataclass
class SessionTranscriptRewriteResult:
    """Result of committing a same-session transcript rewrite."""
    commit: dict[str, Any]
    message_count: int
    parent_revision: str
    revision: str
    session_id: str


@dataclass
class WireProvisionApiKeyResult:
    """`auth/login/provision_api_key` success body."""
    auth_binding: WireAuthBindingRef
    auth_mode: str
    binding_id: str
    has_api_key: bool
    profile_id: str
    provider: str
    realm_id: str
    scopes: list[str]


@dataclass
class WireSessionTranscriptRevision:
    """Full transcript revision body in canonical wire format."""
    has_more: bool
    head_revision: str
    message_count: int
    messages: list[dict[str, Any]]
    offset: int
    revision: str
    session_id: str
    limit: Optional[int] = None
    session_ref: Optional[str] = None


@dataclass
class WireSessionTranscriptRevisionList:
    """Ordered (oldest-first) transcript revision commit list in canonical wire
format."""
    entries: list[dict[str, Any]]
    head_revision: str


@dataclass
class MobWireParams:
    """Request payload for `mob/wire`."""
    member: str
    mob_id: str
    peer: dict[str, str] | dict[str, WireTrustedPeerSpec]


@dataclass
class MobUnwireParams:
    """Request payload for `mob/unwire`."""
    member: str
    mob_id: str
    peer: dict[str, str] | dict[str, WireTrustedPeerSpec]


@dataclass
class MobIdParams:
    """Shared request payload for mob methods that address a mob by id."""
    mob_id: str


@dataclass
class MobMemberParams:
    """Shared request payload for mob methods that address one member by identity."""
    agent_identity: str
    mob_id: str


@dataclass
class MobCreateParams:
    """Request payload for `mob/create`."""
    definition: MobDefinitionInput


@dataclass
class MobCreateResult:
    """Response payload for `mob/create`."""
    mob_id: str


@dataclass
class MobListResult:
    """Response payload for `mob/list`."""
    mobs: list[dict[str, Any]]


@dataclass
class MobStatusResult:
    """One active mob row returned by `mob/list`."""
    mob_id: str
    status: Literal['Creating', 'Running', 'Stopped', 'Completed', 'Destroyed']


@dataclass
class MobLifecycleParams:
    """Request payload for `mob/lifecycle`."""
    action: Literal['stop', 'resume', 'complete', 'reset', 'destroy']
    mob_id: str


@dataclass
class MobLifecycleResult:
    """Response payload for `mob/lifecycle`."""
    action: Literal['stop', 'resume', 'complete', 'reset', 'destroy']
    mob_id: str
    ok: bool
    destroy_report: Optional[Any] = None


@dataclass
class MobSpawnParams:
    """Request payload for `mob/spawn`."""
    agent_identity: str
    mob_id: str
    profile: str
    additional_instructions: Optional[list[str]] = None
    auth_binding: Optional[WireAuthBindingRef] = None
    auto_wire_parent: Optional[bool] = None
    backend: Optional[WireMobBackendKind] = None
    binding: Optional[WireRuntimeBinding] = None
    context: Optional[Any] = None
    inherited_tool_filter: Optional[WireToolFilter] = None
    initial_message: Optional[WireContentInput] = None
    labels: Optional[dict[str, str]] = None
    launch_mode: Optional[WireMemberLaunchMode] = None
    model_override: Optional[str] = None
    override_profile: Optional[WireMobProfile] = None
    placement: Optional[str] = None
    runtime_mode: Optional[WireMobRuntimeMode] = None
    shell_env: Optional[dict[str, str]] = None
    tool_access_policy: Optional[WireToolAccessPolicy] = None


@dataclass
class MobSpawnResult:
    """Response payload for `mob/spawn`."""
    agent_identity: str
    member_ref: WireMemberRef
    mob_id: str


@dataclass
class MobSpawnSpecParams:
    """Per-member request payload inside `mob/spawn_many`."""
    agent_identity: str
    profile: str
    additional_instructions: Optional[list[str]] = None
    auth_binding: Optional[WireAuthBindingRef] = None
    backend: Optional[WireMobBackendKind] = None
    context: Optional[Any] = None
    initial_message: Optional[WireContentInput] = None
    labels: Optional[dict[str, str]] = None
    model_override: Optional[str] = None
    placement: Optional[WireHostRef] = None
    runtime_mode: Optional[WireMobRuntimeMode] = None


@dataclass
class MobSpawnManyParams:
    """Request payload for `mob/spawn_many`."""
    mob_id: str
    specs: list[MobSpawnSpecParams]


@dataclass
class MobSpawnManyResult:
    """Response payload for `mob/spawn_many`."""
    results: list[MobSpawnManyResultEntry]


@dataclass
class MobSpawnManyResultEntry:
    """One typed result entry in a `mob/spawn_many` response."""
    result: MobSpawnManyResultPayload
    status: MobSpawnManyResultStatus


@dataclass
class MobSpawnManySpawnedResult:
    """Successful per-member `mob/spawn_many` result payload."""
    agent_identity: str
    member_ref: WireMemberRef


@dataclass
class MobSpawnManyFailedResult:
    """Failed per-member `mob/spawn_many` result payload."""
    cause: MobSpawnManyFailureCause
    message: str


@dataclass
class MobSpawnReceiptWire:
    """Identity-native payload for `EnsureMemberOutcome::Spawned`."""
    agent_identity: str
    member_ref: WireMemberRef


@dataclass
class MobMemberListEntryWire:
    """Public roster entry returned by `mob/ensure_member`'s `Existed` outcome
(and other surfaces that want a typed snapshot of a single member). Mirrors
the public-facing fields of `meerkat_mob::runtime::MobMemberListEntry`
without leaking bridge-internal fields."""
    agent_identity: str
    is_final: bool
    member_ref: WireMemberRef
    role: str
    runtime_mode: WireMobRuntimeMode
    status: WireMobMemberStatus
    error: Optional[str] = None
    labels: Optional[dict[str, str]] = None
    wired_to: Optional[list[str]] = None


@dataclass
class WireMemberProgressSnapshot:
    """Wire payload for WireMemberProgressSnapshot."""
    health: Literal['healthy', 'degraded', 'wedged', 'unknown']
    in_flight_work: int
    last_progress_at_ms: int
    last_progress_event: Literal['execution_advanced', 'became_idle', 'unchanged']
    run_state: Literal['idle', 'run_open', 'unknown']


@dataclass
class WireMobToolConfig:
    """Tool configuration embedded in a wire mob profile override."""
    builtins: Optional[bool] = None
    comms: Optional[bool] = None
    image_generation: Optional[bool] = None
    mcp: Optional[list[str]] = None
    memory: Optional[bool] = None
    mob: Optional[bool] = None
    schedule: Optional[bool] = None
    shell: Optional[bool] = None
    workgraph: Optional[bool] = None


@dataclass
class WireMobProfile:
    """Profile override for `mob/spawn`."""
    model: str
    auto_compact_threshold: Optional[int] = None
    backend: Optional[WireMobBackendKind] = None
    external_addressable: Optional[bool] = None
    image_generation_provider: Optional[Provider] = None
    max_inline_peer_notifications: Optional[int] = None
    output_schema: Optional[Any] = None
    peer_description: Optional[str] = None
    provider: Optional[Provider] = None
    provider_params: Optional[Any] = None
    resume_overrides: Optional[list[WireMobResumeOverrideField]] = None
    runtime_mode: Optional[WireMobRuntimeMode] = None
    self_hosted_server_id: Optional[str] = None
    skills: Optional[list[str]] = None
    tools: Optional[WireMobToolConfig] = None


@dataclass
class WireMobRun:
    """Typed public projection of a single flow run for `mob/flow_status`.

The canonical identity and lifecycle fields (`run_id`, `mob_id`, `flow_id`,
`status`) are typed; the remaining kernel-owned step/loop projection rides
along as the `kernel` map. Producers project a domain `MobRun` into this
shape so consumers never re-derive run identity or lifecycle from a free
`serde_json::Value`."""
    flow_id: str
    mob_id: str
    run_id: str
    status: WireMobRunStatus


@dataclass
class WireMobRunResultEnvelope:
    """Typed output envelope for a completed or in-flight mob flow run."""
    flow_id: str
    mob_id: str
    run_id: str
    status: WireMobRunStatus
    outputs: Optional[dict[str, Any]] = None
    result: Optional[Any] = None


@dataclass
class WirePeerConnectivitySnapshot:
    """Live connectivity summary for a member's currently wired peers. Mirrors
`meerkat_mob::MobPeerConnectivitySnapshot`."""
    reachable_peer_count: int
    unknown_peer_count: int
    unreachable_peers: Optional[list[WireUnreachablePeer]] = None


@dataclass
class WireUnreachablePeer:
    """One currently wired peer that is known to be unreachable."""
    peer: str
    reason: Optional[str] = None


@dataclass
class WireMobError:
    """Typed mob error projection for wire surfaces. Carries the closed failure
class alongside the human-readable message so consumers branch on the typed
`code` rather than parsing the free-form `message`. Reuses
[`MobSpawnManyFailureCause`] as the canonical closed mob-error vocabulary."""
    code: MobSpawnManyFailureCause
    message: str


@dataclass
class MobEnsureMemberParams:
    """Request payload for `mob/ensure_member`."""
    mob_id: str
    spec: MobMemberSpecWire


@dataclass
class MobEnsureMemberResult:
    """Response payload for `mob/ensure_member`."""
    outcome: dict[str, MobSpawnReceiptWire] | dict[str, MobMemberListEntryWire]


@dataclass
class MobReconcileParams:
    """Request payload for `mob/reconcile`."""
    mob_id: str
    desired: Optional[list[MobMemberSpecWire]] = None
    options: Optional[MobReconcileOptionsWire] = None


@dataclass
class MobReconcileResult:
    """Response payload for `mob/reconcile`."""
    report: MobReconcileReportWire


@dataclass
class MobListMembersMatchingParams:
    """Request payload for `mob/list_members_matching`."""
    mob_id: str
    filter: Optional[dict[str, Any]] = None


@dataclass
class MobListMembersMatchingResult:
    """Response payload for `mob/list_members_matching`. Each member is the raw
roster entry JSON."""
    members: Optional[list[Any]] = None


@dataclass
class MobRetireResult:
    """Response payload for `mob/retire`."""
    retired: bool


@dataclass
class MobRespawnParams:
    """Request payload for `mob/respawn`."""
    agent_identity: str
    mob_id: str
    initial_message: Optional[WireContentInput] = None


@dataclass
class MobRespawnResult:
    """Response payload for `mob/respawn`."""
    receipt: dict[str, Any]
    status: Literal['completed', 'topology_restore_failed']
    failed_peer_ids: Optional[list[str]] = None


@dataclass
class MobWireResult:
    """Response payload for `mob/wire`."""
    wired: bool


@dataclass
class MobWireMembersBatchEdge:
    """One local-member edge in `mob/wire_members_batch`."""
    a: str
    b: str


@dataclass
class MobWireMembersBatchParams:
    """Request payload for `mob/wire_members_batch`."""
    edges: list[MobWireMembersBatchEdge]
    mob_id: str


@dataclass
class MobWireMembersBatchResult:
    """Response payload for `mob/wire_members_batch`."""
    already_wired: list[MobWireMembersBatchEdge]
    requested: int
    wired: list[MobWireMembersBatchEdge]


@dataclass
class MobUnwireResult:
    """Response payload for `mob/unwire`."""
    unwired: bool


@dataclass
class MobMembersResult:
    """Response payload for `mob/members`."""
    members: list[MobMemberListEntryWire]
    mob_id: str


@dataclass
class MobEventsParams:
    """Request payload for `mob/events`."""
    mob_id: str
    after_cursor: Optional[int] = None
    limit: Optional[int] = None
    strict: Optional[bool] = None


@dataclass
class MobEventsResult:
    """Response payload for `mob/events`."""
    events: list[Any]


@dataclass
class MobMemberSendParams:
    """Request payload for host-side mob member delivery."""
    agent_identity: str
    content: WireContentInput
    mob_id: str
    handling_mode: Optional[Literal['queue', 'steer']] = None
    render_metadata: Optional[dict[str, Any]] = None


@dataclass
class MobMemberSendResult:
    """Response payload for host-side mob member delivery."""
    agent_identity: str
    handling_mode: Literal['queue', 'steer']
    member_ref: WireMemberRef
    mob_id: str


@dataclass
class MobIngressInteractionParams:
    """Request payload for `mob/ingress_interaction`.

This is the ergonomic "ensure an ingress member, then deliver user input"
path. It composes the existing declarative roster and member-send
semantics without introducing a separate thread/project runtime."""
    content: WireContentInput
    mob_id: str
    spec: MobMemberSpecWire
    handling_mode: Optional[Literal['queue', 'steer']] = None
    render_metadata: Optional[dict[str, Any]] = None


@dataclass
class MobIngressInteractionResult:
    """Response payload for `mob/ingress_interaction`."""
    agent_identity: str
    delivery: MobMemberSendResult
    ensure_outcome: dict[str, MobSpawnReceiptWire] | dict[str, MobMemberListEntryWire]
    events_after_cursor: int
    latest_event_cursor: int
    member_ref: WireMemberRef
    mob_id: str


@dataclass
class MobAppendSystemContextParams:
    """Request payload for `mob/append_system_context`."""
    agent_identity: str
    mob_id: str
    text: str
    idempotency_key: Optional[str] = None
    source: Optional[str] = None


@dataclass
class MobAppendSystemContextResult:
    """Response payload for `mob/append_system_context`."""
    agent_identity: str
    mob_id: str
    status: Literal['applied', 'staged', 'duplicate']


@dataclass
class MobFlowsResult:
    """Response payload for `mob/flows`."""
    flows: list[str]
    mob_id: str


@dataclass
class MobRunParams:
    """Request payload for `mob/run`.

Starts the pack's callable flow. `flow_id` defaults to `main`; `prompt` is
sugar for `params.prompt` when the caller does not provide that key."""
    mob_id: str
    flow_id: Optional[str] = None
    params: Optional[Any] = None
    prompt: Optional[str] = None


@dataclass
class MobFlowRunParams:
    """Request payload for `mob/flow_run`."""
    flow_id: str
    mob_id: str
    params: Optional[Any] = None


@dataclass
class MobFlowRunResult:
    """Response payload for `mob/flow_run`."""
    run_id: str


@dataclass
class MobFlowStatusParams:
    """Request payload for `mob/flow_status`."""
    mob_id: str
    run_id: str


@dataclass
class MobFlowStatusResult:
    """Response payload for `mob/flow_status`.

`run` is `None` when the requested run id has no persisted run."""
    run: Optional[WireMobRun] = None


@dataclass
class MobRunResultParams:
    """Request payload for `mob/run_result`."""
    mob_id: str
    run_id: str


@dataclass
class MobRunResult:
    """Response payload for `mob/run_result`.

`run` is `None` when the requested run id has no persisted run."""
    run: Optional[WireMobRunResultEnvelope] = None


@dataclass
class MobFlowCancelParams:
    """Request payload for `mob/flow_cancel`."""
    mob_id: str
    run_id: str


@dataclass
class MobFlowCancelResult:
    """Response payload for `mob/flow_cancel`."""
    canceled: bool


@dataclass
class MobSpawnHelperParams:
    """Request payload for `mob/spawn_helper`."""
    mob_id: str
    prompt: str
    agent_identity: Optional[str] = None
    auth_binding: Optional[WireAuthBindingRef] = None
    backend: Optional[WireMobBackendKind] = None
    model_override: Optional[str] = None
    role_name: Optional[str] = None
    runtime_mode: Optional[WireMobRuntimeMode] = None


@dataclass
class MobForkHelperParams:
    """Request payload for `mob/fork_helper`."""
    mob_id: str
    prompt: str
    source_member_id: str
    agent_identity: Optional[str] = None
    auth_binding: Optional[WireAuthBindingRef] = None
    backend: Optional[WireMobBackendKind] = None
    fork_context: Optional[Any] = None
    model_override: Optional[str] = None
    role_name: Optional[str] = None
    runtime_mode: Optional[WireMobRuntimeMode] = None


@dataclass
class MobHelperResult:
    """Response payload for `mob/spawn_helper` and `mob/fork_helper`."""
    agent_identity: str
    member_ref: WireMemberRef
    tokens_used: int
    output: Optional[str] = None


@dataclass
class MobForceCancelResult:
    """Response payload for `mob/force_cancel`."""
    cancelled: bool


@dataclass
class MobTurnStartParams:
    """Request payload for `mob/turn_start`.

`provider_params` and `auth_binding` carry the canonical Inherit/Set/Clear
tri-state via [`WireTurnMetadataOverride`]; unknown fields (including the
retired `clear_*` split wire form) fail closed at the serde boundary via
`deny_unknown_fields`, which also keeps the emitted JSON Schema's
`additionalProperties: false` aligned with the deserializer."""
    agent_identity: str
    mob_id: str
    prompt: WireContentInput
    additional_instructions: Optional[list[str]] = None
    auth_binding: Optional[dict[str, Any]] = None
    injected_context: Optional[list[WireContentInput]] = None
    keep_alive: Optional[bool] = None
    max_tokens: Optional[int] = None
    model: Optional[str] = None
    output_schema: Optional[Any] = None
    provider: Optional[str] = None
    provider_params: Optional[dict[str, Any]] = None
    self_hosted_server_id: Optional[str] = None
    skill_refs: Optional[list[SkillKey]] = None
    structured_output_retries: Optional[int] = None
    system_prompt: Optional[str] = None
    turn_tool_overlay: Optional[PublicTurnToolOverlay] = None


@dataclass
class MobMemberStatusResult:
    """Response payload for `mob/member_status`."""
    is_final: bool
    member_ref: WireMemberRef
    status: WireMobMemberStatus
    tokens_used: int
    comms_reachability: Optional[WireReachability] = None
    control_reachability: Optional[WireReachability] = None
    current_session_id: Optional[str] = None
    error: Optional[str] = None
    external_member: Optional[Any] = None
    freshness_reason: Optional[str] = None
    kickoff: Optional[Any] = None
    last_seen_ms: Optional[int] = None
    lifecycle_capabilities: Optional[WireMemberLifecycleCapabilities] = None
    non_portable_disabled: Optional[list[WireNonPortableResourceKind]] = None
    output_preview: Optional[str] = None
    peer_connectivity: Optional[WirePeerConnectivity] = None
    placement: Optional[WireHostRef] = None
    progress: Optional[WireMemberProgressSnapshot] = None
    resolved_capabilities: Optional[WireResolvedModelCapabilities] = None


@dataclass
class MobSnapshotResult:
    """Response payload for `mob/snapshot`."""
    members: list[MobMemberListEntryWire]
    mob_id: str
    status: Literal['Creating', 'Running', 'Stopped', 'Completed', 'Destroyed']


@dataclass
class MobDestroyResult:
    """Response payload for `mob/destroy`."""
    destroy_report: Any
    mob_id: str
    ok: bool


@dataclass
class SupervisorRotationReportWire:
    """Confirmed supervisor rotation report returned by `mob/rotate_supervisor`."""
    current_epoch: int
    previous_epoch: int
    public_peer_id: str


@dataclass
class MobRotateSupervisorResult:
    """Response payload for `mob/rotate_supervisor`."""
    mob_id: str
    ok: bool
    report: SupervisorRotationReportWire


@dataclass
class MobSubmitWorkParams:
    """Request payload for `mob/submit_work`.

Identifies the member through the opaque [`WireMemberRef`] handle the
server resolves against the live roster — callers do not pass
`generation` or `fence_token`."""
    content: WireContentInput
    member_ref: WireMemberRef
    injected_context: Optional[list[WireContentInput]] = None
    objective_id: Optional[str] = None
    origin: Optional[Literal['external', 'internal']] = None
    work_ref: Optional[str] = None


@dataclass
class MobSubmitWorkResult:
    """Response payload for `mob/submit_work`."""
    member_ref: WireMemberRef
    mob_id: str
    work_ref: str
    objective_id: Optional[str] = None


@dataclass
class MobConcludeObjectiveParams:
    """Explicitly concludes one machine-owned kickoff objective."""
    member_ref: WireMemberRef
    objective_id: str
    outcome: str


@dataclass
class MobConcludeObjectiveResult:
    """Wire payload for MobConcludeObjectiveResult."""
    concluded: bool
    member_ref: WireMemberRef
    objective_id: str


@dataclass
class MobCancelWorkParams:
    """Request payload for `mob/cancel_work`."""
    mob_id: str
    work_ref: str


@dataclass
class MobCancelWorkResult:
    """Response payload for `mob/cancel_work`."""
    mob_id: str
    ok: bool


@dataclass
class MobCancelAllWorkParams:
    """Request payload for `mob/cancel_all_work`."""
    member_ref: WireMemberRef


@dataclass
class MobCancelAllWorkResult:
    """Response payload for `mob/cancel_all_work`."""
    mob_id: str
    ok: bool


@dataclass
class MobWaitParams:
    """Shared request payload for mob readiness waits."""
    mob_id: str
    member_ids: Optional[list[str]] = None
    timeout_ms: Optional[int] = None


@dataclass
class MobWaitMembersResult:
    """Response payload for `mob/wait_kickoff` and `mob/wait_ready`."""
    members: list[Any]


@dataclass
class MobProfileCreateParams:
    """Request payload for `mob/profile/create`."""
    name: str
    profile: MobProfileInput


@dataclass
class MobProfileNameParams:
    """Request payload for `mob/profile/get`."""
    name: str


@dataclass
class MobProfileLookupResult:
    """Stored realm profile projection returned by `mob/profile/*`."""
    name: str
    created_at: Optional[str] = None
    not_found: Optional[bool] = None
    profile: Optional[WireMobProfile] = None
    revision: Optional[int] = None
    updated_at: Optional[str] = None


@dataclass
class MobProfileListResult:
    """Response payload for `mob/profile/list`."""
    profiles: list[MobProfileLookupResult]


@dataclass
class MobProfileUpdateParams:
    """Request payload for `mob/profile/update`."""
    expected_revision: int
    name: str
    profile: MobProfileInput


@dataclass
class MobProfileDeleteParams:
    """Request payload for `mob/profile/delete`."""
    expected_revision: int
    name: str


@dataclass
class MobProfileDeleteResult:
    """Response payload for `mob/profile/delete`."""
    deleted_revision: int
    name: str


@dataclass
class MobStreamOpenParams:
    """Request payload for `mob/stream_open`."""
    mob_id: str
    agent_identity: Optional[str] = None


@dataclass
class MobStreamOpenResult:
    """Response payload for `mob/stream_open`."""
    opened: bool
    stream_id: str


@dataclass
class MobStreamCloseParams:
    """Request payload for `mob/stream_close`."""
    stream_id: str


@dataclass
class MobStreamCloseResult:
    """Response payload for `mob/stream_close`."""
    already_closed: bool
    closed: bool
    stream_id: str


@dataclass
class MobGrantScopesParams:
    """Request payload for `mob/grant_scopes`."""
    mob_id: str
    principal: str
    scopes: list[WireControlScope]
    expires_at_ms: Optional[int] = None


@dataclass
class MobGrantScopesResult:
    """Response payload for `mob/grant_scopes`."""
    record: WireGrantRecord


@dataclass
class MobRevokeScopesParams:
    """Request payload for `mob/revoke_scopes`. `scopes: None` revokes the
principal's entire grant."""
    mob_id: str
    principal: str
    scopes: Optional[list[WireControlScope]] = None


@dataclass
class MobRevokeScopesResult:
    """Response payload for `mob/revoke_scopes`."""
    removed: bool


@dataclass
class MobGrantsResult:
    """Response payload for `mob/grants`."""
    grants: list[WireGrantRecord]


@dataclass
class MobMemberHistoryParams:
    """Request payload for `mob/member_history`."""
    agent_identity: str
    mob_id: str
    from_index: Optional[int] = None
    limit: Optional[int] = None


@dataclass
class MobMemberHistoryResult:
    """Response payload for `mob/member_history`. Pagination facts live inside
`page` (one owner); this envelope adds the placement/provenance facts
only the controlling host knows."""
    generation: int
    page: WireMemberHistoryPageBody
    provenance: WireProjectionProvenance
    placement: Optional[WireHostRef] = None


@dataclass
class WireMemberHistoryPageBody:
    """Shared transcript page body used by both the bridge
`MemberHistoryPage` reply and the console `mob/member_history` result —
same page shape for local and remote members."""
    complete: bool
    from_index: int
    message_count: int
    messages: list[WireHistoryRow]
    next_index: Optional[int] = None


@dataclass
class MobHostsResult:
    """Response payload for `mob/hosts`."""
    hosts: list[MobHostStatus]


@dataclass
class MobHostStatus:
    """One tracked host row for `mob/hosts` (A13).

`endpoint`, `authority_epoch`, and `capabilities` are the CommitHostBind
facts — present for `Bound` hosts, typed-absent for a `Requested`-phase
host (an open or failed bind window commits nothing; fabricating empty
values would launder ceremony state into committed facts).

`control_reachability`/`last_seen_ms`/`freshness_reason` are fed by the
observer-local periodic `HostStatus` driver shared with orphan
reconciliation. They remain typed-absent until the first observation and
never become durable membership facts."""
    bind_phase: WireHostBindPhase
    host_id: WireHostRef
    materialized_member_count: int
    authority_epoch: Optional[int] = None
    capabilities: Optional[WireHostCapabilityFlags] = None
    control_reachability: Optional[WireReachability] = None
    endpoint: Optional[str] = None
    freshness_reason: Optional[str] = None
    last_seen_ms: Optional[int] = None


@dataclass
class WireHostCapabilityFlags:
    """Wire mirror of the DSL `HostCapabilityFlags` single enumeration (§6.1) —
the machine owns the fact; this is its console projection.

Field vocabulary matches the machine maps and the domain
`HostCapabilityReport` exactly (ADJ-P7-1, FLAG-A2): `u64` protocol bounds
and an OPEN `BTreeSet<String>` provider vocabulary — a newer member host
advertising a provider this build's enum lacks must stay representable
(no silent caps)."""
    approval_forwarding: bool
    autonomous_members: bool
    durable_sessions: bool
    engine_version: str
    hard_cancel_member: bool
    mcp: bool
    memory_store: bool
    protocol_max: int
    protocol_min: int
    live_endpoint: Optional[str] = None
    resolvable_providers: Optional[list[str]] = None
    tracked_input_cancel: Optional[bool] = None


@dataclass
class MobRouteInstallsResult:
    """Response payload for the route-install status projection."""
    complete: bool
    outstanding: list[WireRouteInstallObligation]


@dataclass
class WireRouteInstallObligation:
    """One outstanding cross-host route-install obligation."""
    edge_a: str
    edge_b: str
    host: WireHostRef


@dataclass
class MobBindHostParams:
    """Request payload for `mob/bind_host`."""
    descriptor: WireHostBindingDescriptor
    mob_id: str


@dataclass
class MobBindHostResult:
    """Response payload for `mob/bind_host`."""
    authority_epoch: int
    capabilities: WireHostCapabilityFlags
    host_id: WireHostRef


@dataclass
class MobRevokeHostParams:
    """Request payload for `mob/revoke_host`."""
    host_id: WireHostRef
    mob_id: str


@dataclass
class MobRevokeHostResult:
    """Response payload for `mob/revoke_host`."""
    host_id: WireHostRef
    released_members: list[str]


@dataclass
class MobHardCancelParams:
    """Request payload for `mob/hard_cancel_member` (DEC-P6E-8). `reason` is
REQUIRED: the handle verb demands one, and a handler-minted default
string would be handler-owned meaning (DEC-P7A-2)."""
    agent_identity: str
    mob_id: str
    reason: str


@dataclass
class MobHardCancelResult:
    """Response payload for `mob/hard_cancel_member`. A dedicated type (not
[`MobForceCancelResult`] reuse) so the hard/force distinction stays
legible in SDK type names (DEC-P7A-2)."""
    cancelled: bool


@dataclass
class MobMemberLiveOpenParams:
    """Request payload for `mob/member_live_open` (§16.4). Result reuses
`LiveOpenResult` verbatim."""
    agent_identity: str
    mob_id: str
    transport: Optional[LiveOpenTransport] = None
    turning_mode: Optional[RealtimeTurningMode] = None


@dataclass
class MobMemberLiveChannelParams:
    """Request payload for `mob/member_live_close`. Close-what-you-name
(ADJ-P6B-15): `channel_id` is REQUIRED — a reconciling console can never
race-kill a channel a concurrent legitimate open just minted. The status
read has its own params type ([`MobMemberLiveStatusParams`]) because its
`channel_id` is optional by contract."""
    agent_identity: str
    channel_id: str
    mob_id: str


@dataclass
class MobMemberLiveStatusParams:
    """Request payload for `mob/member_live_status` (§16.9, ADJ-P6B-2).
`channel_id: None` IS the reply-loss discovery primitive — it resolves
"the member's active channel" on the owning host, so an orphaned open's
id can be discovered and closed. A dedicated type (not
[`MobMemberLiveChannelParams`]) so the wire cannot amputate the
discovery read (DEC-P7A-2)."""
    agent_identity: str
    mob_id: str
    channel_id: Optional[str] = None


@dataclass
class MobMemberLiveControlParams:
    """Request payload for `mob/member_live_control`."""
    agent_identity: str
    channel_id: str
    mob_id: str
    verb: BridgeLiveControlVerb


@dataclass
class PublicTurnToolOverlay:
    """Public caller-safe per-turn tool overlay."""
    allowed_tools: Optional[list[ToolName]] = None
    blocked_tools: Optional[list[ToolName]] = None


@dataclass
class MobDefinitionInput:
    """Public mob definition input for `mob/create`.

This mirrors the public creation contract shape. Runtime-owned lifecycle and
bookkeeping fields such as internal owner/runtime bindings,
`session_cleanup_policy`, `is_implicit`, and internal-only profile tool
bundles are intentionally not part of this schema.

Not `Eq`: `profiles` transitively carries float provider params."""
    id: str
    profiles: dict[str, MobProfileBindingInput]
    backend: Optional[MobBackendConfigInput] = None
    event_router: Optional[MobEventRouterConfigInput] = None
    flows: Optional[dict[str, MobFlowSpecInput]] = None
    image_generation_provider: Optional[Provider] = None
    limits: Optional[MobLimitsSpecInput] = None
    models: Optional[dict[str, CustomModelConfig]] = None
    orchestrator: Optional[MobOrchestratorInput] = None
    skills: Optional[dict[str, MobSkillSourceInput]] = None
    spawn_policy: Optional[MobSpawnPolicyInput] = None
    supervisor: Optional[MobSupervisorSpecInput] = None
    topology: Optional[MobTopologySpecInput] = None
    wiring: Optional[MobWiringRulesInput] = None


@dataclass
class MobBackendConfigInput:
    """Request payload for MobBackendConfigInput."""
    default: Optional[WireMobBackendKind] = None
    external: Optional[MobExternalBackendConfigInput] = None


@dataclass
class MobEventRouterConfigInput:
    """Request payload for MobEventRouterConfigInput."""
    buffer_size: Optional[int] = None
    exclude_patterns: Optional[list[str]] = None
    include_patterns: Optional[list[str]] = None


@dataclass
class MobExternalBackendConfigInput:
    """Request payload for MobExternalBackendConfigInput."""
    address_base: str
    supervisor_bridge: Optional[Any] = None


@dataclass
class MobFlowSpecInput:
    """Request payload for MobFlowSpecInput."""
    description: Optional[str] = None
    root: Optional[MobFrameSpecInput] = None
    steps: Optional[dict[str, MobFlowStepInput]] = None


@dataclass
class MobFlowStepInput:
    """Request payload for MobFlowStepInput."""
    message: WireContentInput
    role: str
    allowed_tools: Optional[list[str]] = None
    blocked_tools: Optional[list[str]] = None
    branch: Optional[str] = None
    collection_policy: Optional[MobCollectionPolicyInput] = None
    condition: Optional[MobConditionExprInput] = None
    depends_on: Optional[list[str]] = None
    depends_on_mode: Optional[MobDependencyModeInput] = None
    dispatch_mode: Optional[MobDispatchModeInput] = None
    expected_schema_ref: Optional[str] = None
    output_format: Optional[MobStepOutputFormatInput] = None
    timeout_ms: Optional[int] = None


@dataclass
class MobFrameSpecInput:
    """Request payload for MobFrameSpecInput."""
    nodes: dict[str, MobFlowNodeInput]


@dataclass
class MobLimitsSpecInput:
    """Request payload for MobLimitsSpecInput."""
    cancel_grace_timeout_ms: Optional[int] = None
    max_active_frames: Optional[int] = None
    max_active_nodes: Optional[int] = None
    max_flow_duration_ms: Optional[int] = None
    max_frame_depth: Optional[int] = None
    max_orphaned_turns: Optional[int] = None
    max_step_retries: Optional[int] = None


@dataclass
class MobOrchestratorInput:
    """Request payload for MobOrchestratorInput."""
    profile: str


@dataclass
class MobProfileInput:
    """Request payload for MobProfileInput."""
    model: str
    auto_compact_threshold: Optional[int] = None
    backend: Optional[WireMobBackendKind] = None
    external_addressable: Optional[bool] = None
    image_generation_provider: Optional[Provider] = None
    max_inline_peer_notifications: Optional[int] = None
    output_schema: Optional[Any] = None
    peer_description: Optional[str] = None
    provider: Optional[Provider] = None
    provider_params: Optional[Any] = None
    resume_overrides: Optional[list[WireMobResumeOverrideField]] = None
    runtime_mode: Optional[WireMobRuntimeMode] = None
    self_hosted_server_id: Optional[str] = None
    skills: Optional[list[str]] = None
    tools: Optional[MobToolConfigInput] = None


@dataclass
class MobRoleWiringRuleInput:
    """Request payload for MobRoleWiringRuleInput."""
    a: str
    b: str


@dataclass
class MobSupervisorSpecInput:
    """Request payload for MobSupervisorSpecInput."""
    escalation_threshold: int
    role: str
    escalation_turn_timeout_ms: Optional[int] = None


@dataclass
class MobToolConfigInput:
    """Request payload for MobToolConfigInput."""
    builtins: Optional[bool] = None
    comms: Optional[bool] = None
    image_generation: Optional[bool] = None
    mcp: Optional[list[str]] = None
    memory: Optional[bool] = None
    mob: Optional[bool] = None
    schedule: Optional[bool] = None
    shell: Optional[bool] = None
    workgraph: Optional[bool] = None


@dataclass
class MobTopologyRuleInput:
    """Request payload for MobTopologyRuleInput."""
    allowed: bool
    from_role: str
    to_role: str


@dataclass
class MobTopologySpecInput:
    """Request payload for MobTopologySpecInput."""
    mode: MobPolicyModeInput
    rules: list[MobTopologyRuleInput]


@dataclass
class MobWiringRulesInput:
    """Request payload for MobWiringRulesInput."""
    auto_wire_orchestrator: Optional[bool] = None
    role_wiring: Optional[list[MobRoleWiringRuleInput]] = None


@dataclass
class CustomModelConfig:
    """User-defined model registry entry (`[models.<id>]` in `config.toml` or a
mob definition).

Declares an uncatalogued model that an API provider serves so a single
definition feeds provider inference, compaction scaling, capability gates,
and call timeouts through the effective [`crate::ModelRegistry`].

Capability flags are conservative when omitted: an undeclared capability is
treated as absent rather than guessed from the model name."""
    provider: Provider
    call_timeout_secs: Optional[int] = None
    context_window: Optional[int] = None
    display_name: Optional[str] = None
    max_output_tokens: Optional[int] = None
    vision: Optional[bool] = None
    web_search: Optional[bool] = None


@dataclass
class MobMemberSpecWire:
    """Per-member spec for `mob/ensure_member` and the `desired` entries of
`mob/reconcile`.

Mirrors the essential, codegen-friendly fields of
[`meerkat_mob::SpawnMemberSpec`]. Complex sub-types (tool access policy,
budget split, inherited tool filter, override profile) are not on this
wire surface — callers that need that parity should use the non-declarative
`mob/spawn` method."""
    agent_identity: str
    profile: str
    additional_instructions: Optional[list[str]] = None
    auto_wire_parent: Optional[bool] = None
    backend: Optional[WireMobBackendKind] = None
    binding: Optional[WireRuntimeBinding] = None
    context: Optional[Any] = None
    initial_message: Optional[WireContentInput] = None
    labels: Optional[dict[str, str]] = None
    placement: Optional[WireHostRef] = None
    runtime_mode: Optional[WireMobRuntimeMode] = None


@dataclass
class MobReconcileOptionsWire:
    """Options controlling a `mob/reconcile` pass."""
    retire_stale: Optional[bool] = None


@dataclass
class MobReconcileReportWire:
    """Summary produced by a `mob/reconcile` pass."""
    desired: Optional[list[str]] = None
    failures: Optional[list[MobReconcileFailureWire]] = None
    retained: Optional[list[str]] = None
    retired: Optional[list[str]] = None
    spawned: Optional[list[MobSpawnReceiptWire]] = None


@dataclass
class MobReconcileFailureWire:
    """Per-identity failure in a `mob/reconcile` pass."""
    agent_identity: str
    error: WireMobError
    stage: WireMobReconcileStage


@dataclass
class WireHostBindingDescriptor:
    """Host flavor of the `--comms-binding-out` descriptor: what `rkat mob host`
writes for the operator to hand to the controlling mob. Mirrors the
member descriptor's shape (typed Ed25519 identity, never a raw peer id)
with `kind: "host"` and the optional advertised live endpoint (DL5/DL6)."""
    address: str
    bootstrap_token: BridgeBootstrapToken
    identity: WireTrustedPeerIdentity
    kind: WireHostBindingDescriptorKind
    live_endpoint: Optional[str] = None


@dataclass
class WireGrantRecord:
    """One control-plane grant record (A9)."""
    principal: str
    scopes: list[WireControlScope]
    expires_at_ms: Optional[int] = None


@dataclass
class WireMemberLifecycleCapabilities:
    """Lifecycle capabilities available for a member at its placement (§19.L7)."""
    resume_after_restart: bool
    revisions: bool
    transcript_edits: bool


@dataclass
class AttentionBindingRequest:
    """Wire payload for AttentionBindingRequest."""
    binding_id: str
    namespace: Optional[str] = None
    realm_id: Optional[str] = None


@dataclass
class AttentionBindingResult:
    """Wire payload for AttentionBindingResult."""
    attention: WorkAttentionBinding


@dataclass
class AttentionListRequest:
    """Wire payload for AttentionListRequest."""
    namespace: Optional[str] = None
    realm_id: Optional[str] = None
    status: Optional[WorkAttentionStatus] = None
    target: Optional[WorkAttentionTarget] = None


@dataclass
class AttentionListResult:
    """Wire payload for AttentionListResult."""
    attention: list[WorkAttentionBinding]

    @classmethod
    def from_wire(cls, value: Any) -> "AttentionListResult":
        """Fail-closed wire parser (K21): raises MeerkatError
        (INVALID_RESPONSE) on missing or mistyped fields.
        """
        data = _expect_wire_object(value, 'AttentionListResult')
        return cls(
            attention=[WorkAttentionBinding.from_wire(_item) for _item in _expect_wire_list(_require_wire_field(data, 'attention', 'AttentionListResult'), 'AttentionListResult.attention')],
        )


@dataclass
class AttentionProjectionPolicy:
    """Wire payload for AttentionProjectionPolicy."""
    include_parent_context: Optional[bool] = None
    max_text_chars: Optional[int] = None

    @classmethod
    def from_wire(cls, value: Any) -> "AttentionProjectionPolicy":
        """Fail-closed wire parser (K21): raises MeerkatError
        (INVALID_RESPONSE) on missing or mistyped fields.
        """
        data = _expect_wire_object(value, 'AttentionProjectionPolicy')
        return cls(
            include_parent_context=(_expect_wire_bool(data['include_parent_context'], 'AttentionProjectionPolicy.include_parent_context') if data.get('include_parent_context') is not None else None),
            max_text_chars=(_expect_wire_int(data['max_text_chars'], 'AttentionProjectionPolicy.max_text_chars') if data.get('max_text_chars') is not None else None),
        )


@dataclass
class GoalStatusRequest:
    """Wire payload for GoalStatusRequest."""
    binding_id: str
    namespace: Optional[str] = None
    realm_id: Optional[str] = None


@dataclass
class GoalStatusResult:
    """Wire payload for GoalStatusResult."""
    attention: WorkAttentionBinding
    item: WorkItem

    @classmethod
    def from_wire(cls, value: Any) -> "GoalStatusResult":
        """Fail-closed wire parser (K21): raises MeerkatError
        (INVALID_RESPONSE) on missing or mistyped fields.
        """
        data = _expect_wire_object(value, 'GoalStatusResult')
        return cls(
            attention=WorkAttentionBinding.from_wire(_require_wire_field(data, 'attention', 'GoalStatusResult')),
            item=WorkItem.from_wire(_require_wire_field(data, 'item', 'GoalStatusResult')),
        )


@dataclass
class ReadyWorkFilter:
    """Request payload for ReadyWorkFilter."""
    labels: Optional[list[str]] = None
    limit: Optional[int] = None
    namespace: Optional[str] = None
    realm_id: Optional[str] = None


@dataclass
class WorkAttentionBinding:
    """Wire payload for WorkAttentionBinding."""
    binding_id: str
    created_at: str
    delegated_authority: AttentionDelegatedAuthority
    mode: WorkAttentionMode
    status: WorkAttentionStatus
    target: WorkAttentionTarget
    updated_at: str
    work_ref: WorkItemRef
    machine_state: Optional[dict[str, Any]] = None
    projection_policy: Optional[AttentionProjectionPolicy] = None

    @classmethod
    def from_wire(cls, value: Any) -> "WorkAttentionBinding":
        """Fail-closed wire parser (K21): raises MeerkatError
        (INVALID_RESPONSE) on missing or mistyped fields.
        """
        data = _expect_wire_object(value, 'WorkAttentionBinding')
        return cls(
            binding_id=_expect_wire_str(_require_wire_field(data, 'binding_id', 'WorkAttentionBinding'), 'WorkAttentionBinding.binding_id'),
            created_at=_expect_wire_str(_require_wire_field(data, 'created_at', 'WorkAttentionBinding'), 'WorkAttentionBinding.created_at'),
            delegated_authority=parse_attention_delegated_authority(_require_wire_field(data, 'delegated_authority', 'WorkAttentionBinding')),
            machine_state=(_expect_wire_object(data['machine_state'], 'WorkAttentionBinding.machine_state') if data.get('machine_state') is not None else None),
            mode=parse_work_attention_mode(_require_wire_field(data, 'mode', 'WorkAttentionBinding')),
            projection_policy=(AttentionProjectionPolicy.from_wire(data['projection_policy']) if data.get('projection_policy') is not None else None),
            status=parse_work_attention_status(_require_wire_field(data, 'status', 'WorkAttentionBinding')),
            target=parse_work_attention_target(_require_wire_field(data, 'target', 'WorkAttentionBinding')),
            updated_at=_expect_wire_str(_require_wire_field(data, 'updated_at', 'WorkAttentionBinding'), 'WorkAttentionBinding.updated_at'),
            work_ref=WorkItemRef.from_wire(_require_wire_field(data, 'work_ref', 'WorkAttentionBinding')),
        )


@dataclass
class WorkGraphEventFilter:
    """Request payload for WorkGraphEventFilter."""
    after_seq: Optional[int] = None
    all_namespaces: Optional[bool] = None
    limit: Optional[int] = None
    namespace: Optional[str] = None
    realm_id: Optional[str] = None


@dataclass
class WorkGraphEventsResponse:
    """Wire payload for WorkGraphEventsResponse."""
    events: list[WorkGraphEvent]

    @classmethod
    def from_wire(cls, value: Any) -> "WorkGraphEventsResponse":
        """Fail-closed wire parser (K21): raises MeerkatError
        (INVALID_RESPONSE) on missing or mistyped fields.
        """
        data = _expect_wire_object(value, 'WorkGraphEventsResponse')
        return cls(
            events=[WorkGraphEvent.from_wire(_item) for _item in _expect_wire_list(_require_wire_field(data, 'events', 'WorkGraphEventsResponse'), 'WorkGraphEventsResponse.events')],
        )


@dataclass
class WorkGraphIdParams:
    """Parameters identifying a single WorkGraph item by id within an optional
realm/namespace scope (`workgraph/get`)."""
    id: str
    namespace: Optional[str] = None
    realm_id: Optional[str] = None


@dataclass
class WorkGraphItemsResponse:
    """Wire payload for WorkGraphItemsResponse."""
    items: list[WorkItem]

    @classmethod
    def from_wire(cls, value: Any) -> "WorkGraphItemsResponse":
        """Fail-closed wire parser (K21): raises MeerkatError
        (INVALID_RESPONSE) on missing or mistyped fields.
        """
        data = _expect_wire_object(value, 'WorkGraphItemsResponse')
        return cls(
            items=[WorkItem.from_wire(_item) for _item in _expect_wire_list(_require_wire_field(data, 'items', 'WorkGraphItemsResponse'), 'WorkGraphItemsResponse.items')],
        )


@dataclass
class WorkGraphSnapshot:
    """Wire payload for WorkGraphSnapshot."""
    all_namespaces: bool
    captured_at: str
    edges: list[WorkEdge]
    items: list[WorkItem]
    ready_item_ids: list[str]
    realm_id: str
    attention: Optional[list[WorkAttentionBinding]] = None
    event_high_water_mark: Optional[int] = None
    namespace: Optional[str] = None

    @classmethod
    def from_wire(cls, value: Any) -> "WorkGraphSnapshot":
        """Fail-closed wire parser (K21): raises MeerkatError
        (INVALID_RESPONSE) on missing or mistyped fields.
        """
        data = _expect_wire_object(value, 'WorkGraphSnapshot')
        return cls(
            all_namespaces=_expect_wire_bool(_require_wire_field(data, 'all_namespaces', 'WorkGraphSnapshot'), 'WorkGraphSnapshot.all_namespaces'),
            attention=([WorkAttentionBinding.from_wire(_item) for _item in _expect_wire_list(data['attention'], 'WorkGraphSnapshot.attention')] if data.get('attention') is not None else None),
            captured_at=_expect_wire_str(_require_wire_field(data, 'captured_at', 'WorkGraphSnapshot'), 'WorkGraphSnapshot.captured_at'),
            edges=[WorkEdge.from_wire(_item) for _item in _expect_wire_list(_require_wire_field(data, 'edges', 'WorkGraphSnapshot'), 'WorkGraphSnapshot.edges')],
            event_high_water_mark=(_expect_wire_int(data['event_high_water_mark'], 'WorkGraphSnapshot.event_high_water_mark') if data.get('event_high_water_mark') is not None else None),
            items=[WorkItem.from_wire(_item) for _item in _expect_wire_list(_require_wire_field(data, 'items', 'WorkGraphSnapshot'), 'WorkGraphSnapshot.items')],
            namespace=(_expect_wire_str(data['namespace'], 'WorkGraphSnapshot.namespace') if data.get('namespace') is not None else None),
            ready_item_ids=[_expect_wire_str(_item, 'WorkGraphSnapshot.ready_item_ids[]') for _item in _expect_wire_list(_require_wire_field(data, 'ready_item_ids', 'WorkGraphSnapshot'), 'WorkGraphSnapshot.ready_item_ids')],
            realm_id=_expect_wire_str(_require_wire_field(data, 'realm_id', 'WorkGraphSnapshot'), 'WorkGraphSnapshot.realm_id'),
        )


@dataclass
class WorkGraphSnapshotFilter:
    """Request payload for WorkGraphSnapshotFilter."""
    all_namespaces: Optional[bool] = None
    include_terminal: Optional[bool] = None
    labels: Optional[list[str]] = None
    limit: Optional[int] = None
    namespace: Optional[str] = None
    realm_id: Optional[str] = None
    statuses: Optional[list[WorkStatus]] = None


@dataclass
class WorkItem:
    """Wire payload for WorkItem."""
    completion_policy: WorkCompletionPolicy
    created_at: str
    id: str
    machine_state: dict[str, Any]
    namespace: str
    priority: Literal['low', 'medium', 'high']
    realm_id: str
    revision: int
    status: Literal['open', 'in_progress', 'blocked', 'completed', 'cancelled', 'failed']
    title: str
    updated_at: str
    claim: Optional[WorkItemClaim] = None
    description: Optional[str] = None
    due_at: Optional[str] = None
    evidence_refs: Optional[list[WorkEvidenceRef]] = None
    external_refs: Optional[list[WorkItemExternalRef]] = None
    labels: Optional[list[str]] = None
    not_before: Optional[str] = None
    owner: Optional[WorkItemOwner] = None
    snoozed_until: Optional[str] = None
    terminal_at: Optional[str] = None

    @classmethod
    def from_wire(cls, value: Any) -> "WorkItem":
        """Fail-closed wire parser (K21): raises MeerkatError
        (INVALID_RESPONSE) on missing or mistyped fields.
        """
        data = _expect_wire_object(value, 'WorkItem')
        return cls(
            claim=(WorkItemClaim.from_wire(data['claim']) if data.get('claim') is not None else None),
            completion_policy=parse_work_completion_policy(_require_wire_field(data, 'completion_policy', 'WorkItem')),
            created_at=_expect_wire_str(_require_wire_field(data, 'created_at', 'WorkItem'), 'WorkItem.created_at'),
            description=(_expect_wire_str(data['description'], 'WorkItem.description') if data.get('description') is not None else None),
            due_at=(_expect_wire_str(data['due_at'], 'WorkItem.due_at') if data.get('due_at') is not None else None),
            evidence_refs=([WorkEvidenceRef.from_wire(_item) for _item in _expect_wire_list(data['evidence_refs'], 'WorkItem.evidence_refs')] if data.get('evidence_refs') is not None else None),
            external_refs=([WorkItemExternalRef.from_wire(_item) for _item in _expect_wire_list(data['external_refs'], 'WorkItem.external_refs')] if data.get('external_refs') is not None else None),
            id=_expect_wire_str(_require_wire_field(data, 'id', 'WorkItem'), 'WorkItem.id'),
            labels=([_expect_wire_str(_item, 'WorkItem.labels[]') for _item in _expect_wire_list(data['labels'], 'WorkItem.labels')] if data.get('labels') is not None else None),
            machine_state=_expect_wire_object(_require_wire_field(data, 'machine_state', 'WorkItem'), 'WorkItem.machine_state'),
            namespace=_expect_wire_str(_require_wire_field(data, 'namespace', 'WorkItem'), 'WorkItem.namespace'),
            not_before=(_expect_wire_str(data['not_before'], 'WorkItem.not_before') if data.get('not_before') is not None else None),
            owner=(WorkItemOwner.from_wire(data['owner']) if data.get('owner') is not None else None),
            priority=_expect_wire_enum(_require_wire_field(data, 'priority', 'WorkItem'), ('low', 'medium', 'high',), 'WorkItem.priority'),
            realm_id=_expect_wire_str(_require_wire_field(data, 'realm_id', 'WorkItem'), 'WorkItem.realm_id'),
            revision=_expect_wire_int(_require_wire_field(data, 'revision', 'WorkItem'), 'WorkItem.revision'),
            snoozed_until=(_expect_wire_str(data['snoozed_until'], 'WorkItem.snoozed_until') if data.get('snoozed_until') is not None else None),
            status=_expect_wire_enum(_require_wire_field(data, 'status', 'WorkItem'), ('open', 'in_progress', 'blocked', 'completed', 'cancelled', 'failed',), 'WorkItem.status'),
            terminal_at=(_expect_wire_str(data['terminal_at'], 'WorkItem.terminal_at') if data.get('terminal_at') is not None else None),
            title=_expect_wire_str(_require_wire_field(data, 'title', 'WorkItem'), 'WorkItem.title'),
            updated_at=_expect_wire_str(_require_wire_field(data, 'updated_at', 'WorkItem'), 'WorkItem.updated_at'),
        )


@dataclass
class WorkItemFilter:
    """Request payload for WorkItemFilter."""
    all_namespaces: Optional[bool] = None
    include_terminal: Optional[bool] = None
    labels: Optional[list[str]] = None
    limit: Optional[int] = None
    namespace: Optional[str] = None
    realm_id: Optional[str] = None
    statuses: Optional[list[WorkStatus]] = None


@dataclass
class WorkItemRef:
    """Wire payload for WorkItemRef."""
    item_id: str
    namespace: str
    realm_id: str

    @classmethod
    def from_wire(cls, value: Any) -> "WorkItemRef":
        """Fail-closed wire parser (K21): raises MeerkatError
        (INVALID_RESPONSE) on missing or mistyped fields.
        """
        data = _expect_wire_object(value, 'WorkItemRef')
        return cls(
            item_id=_expect_wire_str(_require_wire_field(data, 'item_id', 'WorkItemRef'), 'WorkItemRef.item_id'),
            namespace=_expect_wire_str(_require_wire_field(data, 'namespace', 'WorkItemRef'), 'WorkItemRef.namespace'),
            realm_id=_expect_wire_str(_require_wire_field(data, 'realm_id', 'WorkItemRef'), 'WorkItemRef.realm_id'),
        )


@dataclass
class WorkEdge:
    """Wire payload for WorkEdge."""
    created_at: str
    from_id: str
    kind: WorkEdgeKind
    namespace: str
    realm_id: str
    to_id: str

    @classmethod
    def from_wire(cls, value: Any) -> "WorkEdge":
        """Fail-closed wire parser (K21): raises MeerkatError
        (INVALID_RESPONSE) on missing or mistyped fields.
        """
        data = _expect_wire_object(value, 'WorkEdge')
        return cls(
            created_at=_expect_wire_str(_require_wire_field(data, 'created_at', 'WorkEdge'), 'WorkEdge.created_at'),
            from_id=_expect_wire_str(_require_wire_field(data, 'from_id', 'WorkEdge'), 'WorkEdge.from_id'),
            kind=parse_work_edge_kind(_require_wire_field(data, 'kind', 'WorkEdge')),
            namespace=_expect_wire_str(_require_wire_field(data, 'namespace', 'WorkEdge'), 'WorkEdge.namespace'),
            realm_id=_expect_wire_str(_require_wire_field(data, 'realm_id', 'WorkEdge'), 'WorkEdge.realm_id'),
            to_id=_expect_wire_str(_require_wire_field(data, 'to_id', 'WorkEdge'), 'WorkEdge.to_id'),
        )


@dataclass
class WorkEvidenceRef:
    """Wire payload for WorkEvidenceRef."""
    id: str
    kind: str
    confirmation_kind: Optional[WorkEvidenceKind] = None
    confirming_owner_key: Optional[WorkOwnerKey] = None
    label: Optional[str] = None
    summary: Optional[str] = None

    @classmethod
    def from_wire(cls, value: Any) -> "WorkEvidenceRef":
        """Fail-closed wire parser (K21): raises MeerkatError
        (INVALID_RESPONSE) on missing or mistyped fields.
        """
        data = _expect_wire_object(value, 'WorkEvidenceRef')
        return cls(
            confirmation_kind=(parse_work_evidence_kind(data['confirmation_kind']) if data.get('confirmation_kind') is not None else None),
            confirming_owner_key=(WorkOwnerKey.from_wire(data['confirming_owner_key']) if data.get('confirming_owner_key') is not None else None),
            id=_expect_wire_str(_require_wire_field(data, 'id', 'WorkEvidenceRef'), 'WorkEvidenceRef.id'),
            kind=_expect_wire_str(_require_wire_field(data, 'kind', 'WorkEvidenceRef'), 'WorkEvidenceRef.kind'),
            label=(_expect_wire_str(data['label'], 'WorkEvidenceRef.label') if data.get('label') is not None else None),
            summary=(_expect_wire_str(data['summary'], 'WorkEvidenceRef.summary') if data.get('summary') is not None else None),
        )


@dataclass
class WorkGraphEvent:
    """Wire payload for WorkGraphEvent."""
    at: str
    kind: WorkGraphEventKind
    namespace: str
    realm_id: str
    item_id: Optional[str] = None
    payload: Optional[Any] = None
    seq: Optional[int] = None

    @classmethod
    def from_wire(cls, value: Any) -> "WorkGraphEvent":
        """Fail-closed wire parser (K21): raises MeerkatError
        (INVALID_RESPONSE) on missing or mistyped fields.
        """
        data = _expect_wire_object(value, 'WorkGraphEvent')
        return cls(
            at=_expect_wire_str(_require_wire_field(data, 'at', 'WorkGraphEvent'), 'WorkGraphEvent.at'),
            item_id=(_expect_wire_str(data['item_id'], 'WorkGraphEvent.item_id') if data.get('item_id') is not None else None),
            kind=parse_work_graph_event_kind(_require_wire_field(data, 'kind', 'WorkGraphEvent')),
            namespace=_expect_wire_str(_require_wire_field(data, 'namespace', 'WorkGraphEvent'), 'WorkGraphEvent.namespace'),
            payload=data.get('payload'),
            realm_id=_expect_wire_str(_require_wire_field(data, 'realm_id', 'WorkGraphEvent'), 'WorkGraphEvent.realm_id'),
            seq=(_expect_wire_int(data['seq'], 'WorkGraphEvent.seq') if data.get('seq') is not None else None),
        )


@dataclass
class WorkOwnerKey:
    """Wire payload for WorkOwnerKey."""
    id: str
    kind: WorkOwnerKind

    @classmethod
    def from_wire(cls, value: Any) -> "WorkOwnerKey":
        """Fail-closed wire parser (K21): raises MeerkatError
        (INVALID_RESPONSE) on missing or mistyped fields.
        """
        data = _expect_wire_object(value, 'WorkOwnerKey')
        return cls(
            id=_expect_wire_str(_require_wire_field(data, 'id', 'WorkOwnerKey'), 'WorkOwnerKey.id'),
            kind=parse_work_owner_kind(_require_wire_field(data, 'kind', 'WorkOwnerKey')),
        )


@dataclass
class WorkItemClaim:
    """Promoted inline object type WorkItemClaim."""
    claimed_at: str
    owner: WorkItemOwner
    lease_expires_at: Optional[str] = None

    @classmethod
    def from_wire(cls, value: Any) -> "WorkItemClaim":
        """Fail-closed wire parser (K21): raises MeerkatError
        (INVALID_RESPONSE) on missing or mistyped fields.
        """
        data = _expect_wire_object(value, 'WorkItemClaim')
        return cls(
            claimed_at=_expect_wire_str(_require_wire_field(data, 'claimed_at', 'WorkItemClaim'), 'WorkItemClaim.claimed_at'),
            lease_expires_at=(_expect_wire_str(data['lease_expires_at'], 'WorkItemClaim.lease_expires_at') if data.get('lease_expires_at') is not None else None),
            owner=WorkItemOwner.from_wire(_require_wire_field(data, 'owner', 'WorkItemClaim')),
        )


@dataclass
class WorkItemExternalRef:
    """Promoted inline object type WorkItemExternalRef."""
    id: str
    kind: str
    url: Optional[str] = None

    @classmethod
    def from_wire(cls, value: Any) -> "WorkItemExternalRef":
        """Fail-closed wire parser (K21): raises MeerkatError
        (INVALID_RESPONSE) on missing or mistyped fields.
        """
        data = _expect_wire_object(value, 'WorkItemExternalRef')
        return cls(
            id=_expect_wire_str(_require_wire_field(data, 'id', 'WorkItemExternalRef'), 'WorkItemExternalRef.id'),
            kind=_expect_wire_str(_require_wire_field(data, 'kind', 'WorkItemExternalRef'), 'WorkItemExternalRef.kind'),
            url=(_expect_wire_str(data['url'], 'WorkItemExternalRef.url') if data.get('url') is not None else None),
        )


@dataclass
class WorkItemOwner:
    """Promoted inline object type WorkItemOwner."""
    key: WorkOwnerKey
    display_name: Optional[str] = None

    @classmethod
    def from_wire(cls, value: Any) -> "WorkItemOwner":
        """Fail-closed wire parser (K21): raises MeerkatError
        (INVALID_RESPONSE) on missing or mistyped fields.
        """
        data = _expect_wire_object(value, 'WorkItemOwner')
        return cls(
            display_name=(_expect_wire_str(data['display_name'], 'WorkItemOwner.display_name') if data.get('display_name') is not None else None),
            key=WorkOwnerKey.from_wire(_require_wire_field(data, 'key', 'WorkItemOwner')),
        )


@dataclass
class BridgeAck:
    """Simple acknowledgment."""
    ok: bool


@dataclass
class BridgeBindPayload:
    """Bind a remote runtime to this supervisor."""
    bootstrap_token: BridgeBootstrapToken
    epoch: int
    expected_address: str
    expected_peer_id: str
    protocol_version: BridgeProtocolVersion
    supervisor: BridgePeerSpec


@dataclass
class BridgeBindResponse:
    """Response to a bind command."""
    address: str
    capabilities: BridgeCapabilities
    peer_id: str


@dataclass
class BridgeCapabilities:
    """Capabilities advertised by a member runtime on bind."""
    approval_forwarding: Optional[bool] = None
    autonomous_members: Optional[bool] = None
    current_protocol_version: Optional[BridgeProtocolVersion] = None
    default_protocol_version: Optional[BridgeProtocolVersion] = None
    deliver_member_input: Optional[bool] = None
    destroy_member: Optional[bool] = None
    durable_sessions: Optional[bool] = None
    engine_version: Optional[str] = None
    hard_cancel_member: Optional[bool] = None
    interrupt_member: Optional[bool] = None
    mcp: Optional[bool] = None
    memory_store: Optional[bool] = None
    observe_member: Optional[bool] = None
    resolvable_providers: Optional[list[Provider]] = None
    retire_member: Optional[bool] = None
    supported_protocol_versions: Optional[list[BridgeProtocolVersion]] = None
    tracked_input_cancel: Optional[bool] = None
    unwire_member: Optional[bool] = None
    wire_member: Optional[bool] = None


@dataclass
class BridgeDeliveryPayload:
    """Deliver one logical input to a member."""
    content: ContentInput
    epoch: int
    handling_mode: HandlingMode
    input_id: str
    protocol_version: BridgeProtocolVersion
    supervisor: BridgePeerSpec
    expected_member: Optional[BridgeMemberIncarnation] = None
    injected_context: Optional[list[ContentInput]] = None
    objective_id: Optional[str] = None
    outcome_tracking: Optional[Literal['interaction']] = None
    transcript_interaction_id: Optional[str] = None
    turn: Optional[BridgeTurnDirective] = None


@dataclass
class BridgeDeliveryResponse:
    """Full response to a delivery command.

The bridge delivery wire contract advertises only the accept-boundary
outcome (`outcome`) and the canonical input id. There is deliberately no
turn-completion payload here: a `DeliverMemberInput` command acknowledges
admission of one logical input, not the eventual turn result. Turn
completion is observed through the runtime completion seam
(`CompletionFeed` / `CompletionOutcome`), never re-derived onto this
delivery acknowledgement — advertising a completion field that every
producer fills with `None` was pure schema theater and has been removed
so the wire shape matches what is actually delivered."""
    input_id: str
    outcome: BridgeDeliveryOutcome
    canonical_input_id: Optional[str] = None


@dataclass
class BridgeDestroyResponse:
    """Response to a destroy command."""
    inputs_abandoned: int


@dataclass
class BridgeHardCancelPayload:
    """Explicit hard-cancel command payload.

`InterruptMember` is the cooperative boundary-break path. This payload is
intentionally separate so supervisors cannot accidentally collapse boundary
cancellation and immediate user/session interrupt authority onto one wire
command. `operation_id` remains stable across transport retries, while
`expected_run_id` fences the level-triggered request to one exact run: a
delayed retry can observe that run already terminal, but must never cancel a
newer run that subsequently became current."""
    epoch: int
    expected_member: BridgeMemberIncarnation
    expected_run_id: RunId
    operation_id: OperationId
    protocol_version: BridgeProtocolVersion
    reason: str
    supervisor: BridgePeerSpec


@dataclass
class BridgeObservationResponse:
    """Response to an observe command."""
    observed_at: str
    state: BridgeMemberRuntimeState
    accepting_inputs: Optional[bool] = None
    current_run_id: Optional[str] = None
    last_error: Optional[str] = None
    peer_connectivity: Optional[BridgePeerConnectivity] = None


@dataclass
class BridgePeerSpec:
    """Minimal trusted peer identity for supervisor bridge wire messages.

Mirrors `meerkat_core::comms::TrustedPeerDescriptor` (post-C-TRP) but is
self-contained in the contracts crate so neither sender nor receiver
needs a cross-crate dependency for deserialization. Fields stay
stringly at the wire boundary — `peer_id` is the canonical comms routing
UUID, while raw Ed25519 public key material is carried only in `pubkey`.
The typed `PeerId`/`PeerName`/`PeerAddress` atoms are re-hydrated on the
receiving side."""
    address: str
    name: str
    peer_id: str
    pubkey: Optional[list[int]] = None


@dataclass
class BridgePeerWiringPayload:
    """Peer wiring command payload.

`mob_peer_overlay` is present from protocol V3 onward. It is
`#[serde(default, skip_serializing_if)]` so that (a) a V2 wiring payload
emitted by a pre-overlay peer still deserializes here as `None` instead of
erroring on a missing field, and (b) a V3 payload remains byte-identical on
the wire to the pre-Option shape (the field is always `Some` when emitted by
V3 senders). A `None` overlay on a wiring command is rejected by the
receiver with a typed `UnsupportedProtocolVersion` cause."""
    epoch: int
    peer_spec: BridgePeerSpec
    protocol_version: BridgeProtocolVersion
    supervisor: BridgePeerSpec
    mob_peer_overlay: Optional[BridgeMobPeerOverlayHandoff] = None


@dataclass
class BridgeRetireResponse:
    """Response to a retire command."""
    inputs_abandoned: int
    inputs_pending_drain: int


@dataclass
class BridgeSupervisorPayload:
    """Supervisor authority credentials included in every bridge command."""
    epoch: int
    protocol_version: BridgeProtocolVersion
    supervisor: BridgePeerSpec


@dataclass
class CommsChecksumTokenParams:
    """Typed params for the actionable checksum-token request used by peer
request/response terminal-flow fixtures."""
    subject: str


@dataclass
class CommsChecksumTokenResult:
    """Typed result for a checksum-token peer response."""
    request_intent: CommsChecksumTokenResultIntent
    request_subject: str
    token: str


@dataclass
class CommsPeerLifecycleParams:
    """Typed params for one-way peer lifecycle notifications.

This is the public wire projection of the topology-update payloads that
used to travel as arbitrary JSON. `peer_spec` is the canonical typed
identity when the sender has it; `peer`, `role`, and `description` remain
inert presentation metadata for older projections."""
    peer: str
    description: Optional[str] = None
    peer_spec: Optional[BridgePeerSpec] = None
    role: Optional[str] = None


@dataclass
class CommsPeersParams:
    """Request payload for `comms/peers`."""
    session_id: str


@dataclass
class PeerAddress:
    """Typed peer address: transport atom plus endpoint string.

The `endpoint` is transport-specific (path for `Uds`, `host:port` for
`Tcp`, agent name for `Inproc`) but is carried as a validated `String`
so the transport atom can be branched on without re-parsing."""
    endpoint: str
    transport: PeerTransport


@dataclass
class PeerCapabilitySet:
    """Typed peer capability envelope for peer-directory output.

Extensions are intentionally opaque display/integration metadata. Core
routing, admission, and policy decisions must use typed fields such as
[`PeerDirectoryEntry::sendable_kinds`] instead of consulting this bag."""
    extensions: Optional[dict[str, Any]] = None
    version: Optional[int] = None


@dataclass
class PeerDirectoryEntry:
    """Wire payload for PeerDirectoryEntry."""
    address: PeerAddress
    capabilities: PeerCapabilitySet
    meta: dict[str, Any]
    name: PeerName
    peer_id: PeerId
    sendable_kinds: list[PeerSendability]
    source: PeerDirectorySource


@dataclass
class CommsPeersResult:
    """Response payload for `comms/peers`."""
    peers: list[PeerDirectoryEntry]


@dataclass
class SessionStreamOpenParams:
    """Request payload for `session/stream_open`."""
    session_id: str


@dataclass
class SessionStreamOpenResult:
    """Response payload for `session/stream_open`."""
    opened: bool
    session_id: str
    stream_id: str


@dataclass
class SessionStreamCloseParams:
    """Request payload for `session/stream_close`."""
    stream_id: str


@dataclass
class SessionStreamCloseResult:
    """Response payload for `session/stream_close`."""
    already_closed: bool
    closed: bool
    stream_id: str


@dataclass
class BridgeHostCapabilityRequirements:
    """Capabilities the controller requires the host to preserve before a bind
or rebind may become durable. The clause is computed from existing placed
residency/custody and validated host-side before authority mutation, so an
accepted remote ceremony can never be rejected only afterward by the
controller's machine."""
    autonomous_members: bool
    durable_sessions: bool
    protocol_v4: bool
    tracked_input_cancel: bool


@dataclass
class BridgeMemberIncarnation:
    """Exact cross-host member residency addressed by one directed delivery."""
    agent_identity: str
    binding_generation: int
    fence_token: int
    generation: int
    host_id: str
    member_session_id: str
    mob_id: str


@dataclass
class BridgeTurnCorrelation:
    """Flow-step correlation for a directed turn."""
    run_id: str
    step_id: str


@dataclass
class BridgeTurnDirective:
    """Flow-turn directive attached to a delivery (absent-omitted; fails closed
on pre-V4 receivers via their `deny_unknown_fields` boundary)."""
    correlation: BridgeTurnCorrelation
    tool_overlay: Optional[PublicTurnToolOverlay] = None


@dataclass
class BridgeTurnOutcomeAck:
    """Exact acknowledgement key for one consumed directed-turn terminal.

Agent identity and mob id are implied by the addressed member session;
generation + fence keep a reused input id from pruning a later residency."""
    fence_token: int
    generation: int
    input_id: str


@dataclass
class WireFlowFailureDetail:
    """Presentation detail for a failed directed turn.

`original_utf8_bytes` always names the byte length of the complete source
string. When `truncated` is true, `text` is the largest UTF-8 prefix the
member host could retain while keeping the complete
[`BridgeTurnOutcomeRecord`] within the durable protocol bound."""
    original_utf8_bytes: int
    text: str
    truncated: bool


@dataclass
class BridgeTurnOutcomeRecord:
    """Terminal outcome record for a directed turn, delivered as a sidecar on
`MemberEventsPage` (the event envelope rows themselves stay unchanged —
§7 envelope freeze)."""
    fence_token: int
    generation: int
    input_id: str
    outcome: WireFlowTurnOutcome
    terminal_seq: int


@dataclass
class BridgeMobPeerOverlayHandoff:
    """Generated MobMachine peer overlay handoff carried with peer wiring commands."""
    peer_specs: list[BridgePeerSpec]
    recipient_peer_id: str
    topology_epoch: int


# Unique identifier for a run (a single execution of the agent loop).
#
# Core emits this in `RunEvent` and tracks it across the run lifecycle.
RunId = str

# Unique identifier for an operation
OperationId = str

# Cursor into a member's event stream.
#
# `seq` is the owning host's durable `StoredEvent.seq` for the session
# bound at `generation` — NEVER a session-task or per-stream counter. A
# fresh-session respawn starts a new seq domain, while same-session Resume
# continues that session's seqs behind a host-recorded generation floor;
# page replies always carry the current generation so consumers restart at
# the owning host's resolved floor.
class BridgeEventCursorTail(TypedDict, total=False):
    cursor: Required[Literal['tail']]

class BridgeEventCursorAt(TypedDict, total=False):
    cursor: Required[Literal['at']]
    generation: Required[int]
    seq: Required[int]

BridgeEventCursor = BridgeEventCursorTail | BridgeEventCursorAt

# Supervisor bridge nested type BridgeOutboundTaintTarget.
class BridgeOutboundTaintTargetPlaced(TypedDict, total=False):
    placed: Required[BridgeMemberIncarnation]

BridgeOutboundTaintTarget = Literal['peer_only'] | BridgeOutboundTaintTargetPlaced

# Stable terminal class for an exact tracked-input cancellation.
class BridgeTrackedInputCancelOutcomeNoEffect(TypedDict, total=False):
    outcome: Required[Literal['no_effect']]

class BridgeTrackedInputCancelOutcomeCancelled(TypedDict, total=False):
    outcome: Required[Literal['cancelled']]

class BridgeTrackedInputCancelOutcomeTerminal(TypedDict, total=False):
    outcome: Required[Literal['terminal']]
    record: Required[BridgeTurnOutcomeRecord]

BridgeTrackedInputCancelOutcome = BridgeTrackedInputCancelOutcomeNoEffect | BridgeTrackedInputCancelOutcomeCancelled | BridgeTrackedInputCancelOutcomeTerminal

# Transport requested by `live/open`.
#
# Missing means "server default": prefer WebSocket when configured, otherwise
# WebRTC when that is the only configured live transport. This preserves the
# pre-WebRTC `live/open` shape while keeping the new branch typed.
LiveOpenTransport = Literal['websocket', 'webrtc']

# The seven flow-turn terminals plus channel-close (§18.1/§18.11). Closed.
#
# Failure details are structured because the member host may have to retain
# a UTF-8-safe prefix to keep one durable journal row within its protocol
# bound. Consumers can distinguish that representation compaction from the
# original failure itself without parsing prose.
class WireFlowTurnOutcomeExtractionFailedPayload(TypedDict, total=False):
    detail: Required[WireFlowFailureDetail]

class WireFlowTurnOutcomeExtractionFailed(TypedDict, total=False):
    extraction_failed: Required[WireFlowTurnOutcomeExtractionFailedPayload]

class WireFlowTurnOutcomeRunFailedPayload(TypedDict, total=False):
    detail: Required[WireFlowFailureDetail]

class WireFlowTurnOutcomeRunFailed(TypedDict, total=False):
    run_failed: Required[WireFlowTurnOutcomeRunFailedPayload]

class WireFlowTurnOutcomeInteractionFailedPayload(TypedDict, total=False):
    detail: Required[WireFlowFailureDetail]

class WireFlowTurnOutcomeInteractionFailed(TypedDict, total=False):
    interaction_failed: Required[WireFlowTurnOutcomeInteractionFailedPayload]

WireFlowTurnOutcome = Literal['run_completed'] | Literal['extraction_succeeded'] | Literal['interaction_complete'] | Literal['interaction_callback_pending'] | Literal['channel_closed'] | WireFlowTurnOutcomeExtractionFailed | WireFlowTurnOutcomeRunFailed | WireFlowTurnOutcomeInteractionFailed

# Stable discriminant for [`AuthError`]. Used in serialized error summaries
# and for SDK-facing wire codes.
AuthErrorKind = Literal['missing_secret', 'unsupported_combination', 'missing_required_metadata', 'workspace_mismatch', 'stale_credential', 'refresh_required', 'lease_absent', 'user_reauth_required', 'expired', 'refresh_failed', 'resolve_required', 'interactive_login_required', 'host_owned_unavailable', 'io', 'other']

# Wire projection of `meerkat_core::connection::ConnectionTargetError`'s
# variant set (closed; kinds only — the parameters stay in the diagnostic
# `reason` string).
ConnectionTargetErrorKind = Literal['missing_realm', 'unknown_realm', 'missing_default_binding', 'invalid_realm_id', 'invalid_binding_id', 'realm_config_invalid', 'binding_invalid', 'provider_mismatch', 'realm_chain']

# Closed control-plane scope vocabulary (A9). Grants and bridge scope
# denials speak exactly this set.
WireControlScope = Literal['list', 'read_history', 'subscribe_events', 'send_command', 'cancel', 'retire', 'wire_topology', 'live', 'admin_host', 'admin_grants']

# Typed "host cannot build the member" taxonomy (§14.4). Closed — every
# owning-host build failure maps onto one of these, never a generic
# failure. Externally tagged (like the parent cause enum) so the
# `kind`-named detail fields don't collide with a tag key.
class MemberBuildRejectionUnknownProviderForModelPayload(TypedDict, total=False):
    model: Required[str]

class MemberBuildRejectionUnknownProviderForModel(TypedDict, total=False):
    unknown_provider_for_model: Required[MemberBuildRejectionUnknownProviderForModelPayload]

class MemberBuildRejectionBindingUnresolvablePayload(TypedDict, total=False):
    kind: Required[ConnectionTargetErrorKind]

class MemberBuildRejectionBindingUnresolvable(TypedDict, total=False):
    binding_unresolvable: Required[MemberBuildRejectionBindingUnresolvablePayload]

class MemberBuildRejectionProviderAuthPayload(TypedDict, total=False):
    kind: Required[AuthErrorKind]

class MemberBuildRejectionProviderAuth(TypedDict, total=False):
    provider_auth: Required[MemberBuildRejectionProviderAuthPayload]

class MemberBuildRejectionSelfHostedServerMissingPayload(TypedDict, total=False):
    server_id: Required[str]

class MemberBuildRejectionSelfHostedServerMissing(TypedDict, total=False):
    self_hosted_server_missing: Required[MemberBuildRejectionSelfHostedServerMissingPayload]

MemberBuildRejection = MemberBuildRejectionUnknownProviderForModel | MemberBuildRejectionBindingUnresolvable | MemberBuildRejectionProviderAuth | MemberBuildRejectionSelfHostedServerMissing

# Optional typed domain for tool-configuration change payloads.
ToolConfigChangeDomain = Literal['tool_scope', 'deferred_catalog']

# Operation kind for live tool configuration changes.
ToolConfigChangeOperation = Literal['add', 'remove', 'reload']

# Canonical lifecycle phase for external-tool boundary deltas.
ExternalToolDeltaPhase = Literal['pending', 'applied', 'draining', 'forced', 'failed']

# Structured status data for live tool configuration change notifications.
class ToolConfigChangeStatusBoundaryApplied(TypedDict, total=False):
    base_changed: Required[bool]
    kind: Required[Literal['boundary_applied']]
    revision: Required[int]
    visible_changed: Required[bool]

class ToolConfigChangeStatusDeferredCatalogDelta(TypedDict, total=False):
    added_hidden_count: Required[int]
    kind: Required[Literal['deferred_catalog_delta']]
    pending_source_count: Required[int]
    removed_hidden_count: Required[int]

class ToolConfigChangeStatusWarningFailedClosed(TypedDict, total=False):
    error: Required[str]
    kind: Required[Literal['warning_failed_closed']]

class ToolConfigChangeStatusExternalToolDelta(TypedDict, total=False):
    detail: NotRequired[Optional[str]]
    kind: Required[Literal['external_tool_delta']]
    phase: Required[ExternalToolDeltaPhase]

ToolConfigChangeStatus = ToolConfigChangeStatusBoundaryApplied | ToolConfigChangeStatusDeferredCatalogDelta | ToolConfigChangeStatusWarningFailedClosed | ToolConfigChangeStatusExternalToolDelta

@dataclass
class SystemNoticePeer:
    """Peer identity carried in a typed comms transcript block.

`id` is the canonical routing identity ([`crate::comms::PeerId`]), serialized
as a hyphenated UUID string on the wire; `display_name` is the presentation
label. Keeping `id` typed lets the projection logic consume the identity
directly instead of re-parsing a `String` back into a `PeerId`."""
    id: PeerId
    display_name: Optional[str] = None


@dataclass
class DeferredCatalogDelta:
    """Additive hidden-catalog delta metadata for runtime notices."""
    added_hidden_names: Optional[list[ToolName]] = None
    pending_sources: Optional[list[str]] = None
    removed_hidden_names: Optional[list[ToolName]] = None


@dataclass
class ToolConfigChangedPayload:
    """Payload for tool configuration change notifications.

The typed `status_info` is the sole owner of the change status; display
text is derived at read time via [`ToolConfigChangedPayload::status_text`]."""
    operation: ToolConfigChangeOperation
    persisted: bool
    status_info: ToolConfigChangeStatus
    target: str
    applied_at_turn: Optional[int] = None
    deferred_catalog_delta: Optional[DeferredCatalogDelta] = None
    domain: Optional[ToolConfigChangeDomain] = None


@dataclass
class ScheduleIdParams:
    """Request payload for schedule id lookups."""
    schedule_id: str


@dataclass
class ListSchedulesParams:
    """Request payload for schedule/list."""
    labels: Optional[dict[str, str]] = None
    limit: Optional[int] = None
    offset: Optional[int] = None


@dataclass
class ScheduleOccurrencesParams:
    """Request payload for schedule/occurrences."""
    schedule_id: str
    include_terminal: Optional[bool] = None


@dataclass
class UpdateScheduleParams:
    """Request payload for schedule/update."""
    schedule_id: str
    description: Optional[str] = None
    expected_revision: Optional[int] = None
    labels: Optional[dict[str, str]] = None
    misfire_policy: Optional[dict[str, Any]] = None
    missing_target_policy: Optional[Literal['skip', 'mark_misfired']] = None
    name: Optional[str] = None
    overlap_policy: Optional[Literal['allow_concurrent', 'skip_if_running']] = None
    planning_horizon_days: Optional[int] = None
    planning_horizon_occurrences: Optional[int] = None
    target: Optional[dict[str, Any]] = None
    trigger: Optional[dict[str, Any]] = None


@dataclass
class WireRenderMetadata:
    """Public render metadata contract for mob member delivery."""
    class_: Literal['user_prompt', 'peer_message', 'peer_request', 'peer_response', 'external_event', 'flow_step', 'continuation', 'system_notice', 'tool_scope_notice', 'ops_progress']
    salience: Optional[Literal['background', 'normal', 'important', 'urgent']] = None


# Typed external peer identity for public mob wiring surfaces.
class WireTrustedPeerIdentityEd25519PublicKey(TypedDict, total=False):
    kind: Required[Literal['ed25519_public_key']]
    public_key: Required[str]

WireTrustedPeerIdentity = WireTrustedPeerIdentityEd25519PublicKey

@dataclass
class WireTrustedPeerSpec:
    """Minimal trusted peer spec for public mob wiring surfaces.

`identity` is required and resolves to the Ed25519 signing public key
plus the canonical comms `PeerId` derived from that key. MCP callers do
not provide raw peer IDs, and missing key material fails at the boundary."""
    address: str
    identity: WireTrustedPeerIdentity
    name: str


@dataclass
class RuntimeStateResult:
    """Response payload for runtime-backed session status projections."""
    state: WireRuntimeState


@dataclass
class RealtimeAudioFormat:
    """Descriptor for an expected or actual realtime audio format."""
    channels: int
    mime_type: str
    sample_rate_hz: int


@dataclass
class RealtimeCapabilities:
    """Provider-session capability projection used by the live adapter to
publish what the negotiated provider session can actually do."""
    interrupt_supported: bool
    tool_lifecycle_events_supported: bool
    transcript_supported: bool
    video_supported: bool
    audio_input_format: Optional[RealtimeAudioFormat] = None
    audio_output_format: Optional[RealtimeAudioFormat] = None
    input_kinds: Optional[list[RealtimeInputKind]] = None
    output_kinds: Optional[list[RealtimeOutputKind]] = None
    turning_modes: Optional[list[RealtimeTurningMode]] = None


@dataclass
class RealtimeTextChunk:
    """A text chunk for realtime ingress/egress."""
    text: str


@dataclass
class RealtimeAudioChunk:
    """An opaque realtime audio chunk with MIME + format metadata.

Both sender and receiver MUST stamp `sample_rate_hz` and `channels` so the
transport layer can validate against the provider session's negotiated
format instead of silently producing garbled audio when an ESP32 or browser
client ships the wrong rate."""
    channels: int
    data: str
    mime_type: str
    sample_rate_hz: int


@dataclass
class RealtimeVideoChunk:
    """An opaque realtime video chunk with MIME metadata."""
    data: str
    mime_type: str


@dataclass
class RealtimeImageChunk:
    """An opaque still-image chunk for vision-capable realtime models.

`mime_type` is the IANA type of the encoded bytes; supported formats are
provider-specific. The current OpenAI Realtime adapter accepts only
`image/png` and `image/jpeg`. `data` is the base64-encoded image; the
provider seam renders it into the provider-native form (for OpenAI
Realtime, an `input_image` content part carrying a data URL)."""
    data: str
    idempotency_key: str
    mime_type: str


@dataclass
class LiveOpenParams:
    """Request payload for `live/open`.

R3-1 (P1): `turning_mode` lets callers pick between the provider-managed
(server-VAD) flow and the explicit-commit flow. The G9 typed text-only
`live/commit_input { response_modality: text }` path requires
`ExplicitCommit` because the OpenAI realtime API rejects
`input_audio_buffer.commit` unless the session was opened with explicit
commit semantics (server VAD owns commits otherwise). Pre-R3-1 the
handler hard-coded `ProviderManaged`, which made the typed text-only
path unreachable on a public channel — calling it killed the channel
instead of producing sideband text.

Default (`None`) preserves the prior wire shape: callers that omit the
field get `ProviderManaged`, matching the legacy behavior."""
    session_id: str
    seed_max_chars: Optional[int] = None
    transport: Optional[LiveOpenTransport] = None
    turning_mode: Optional[RealtimeTurningMode] = None


@dataclass
class WireLiveChannelCapabilities:
    """Wire projection of [`meerkat_core::live_adapter::LiveChannelCapabilities`].

Typed-boolean matrix advertised when a live channel opens. SDK consumers
(Python `client.py`, TypeScript `client.ts`, Web SDK) get typed access to
`image_in` / `video_in` / `transcript_supported` etc. without needing to
hand-decode an opaque JSON object. CC5: closes the typed-surface gap that
hid T8's "anticipate `gpt-realtime-2`" goal at the SDK boundary.

Field shape mirrors the core type 1:1 — adding a new capability requires
extending both the core type and this mirror; the `From` impls below
fail-closed if a field is dropped (compile error: missing field). New
modalities appear here as additional typed booleans, never as
stringly-typed lists or provider-specific enums."""
    audio_in: bool
    audio_out: bool
    barge_in_supported: bool
    image_in: bool
    provider_native_resume: bool
    text_in: bool
    text_out: bool
    transcript_supported: bool
    video_in: bool


# Wire projection of [`meerkat_core::live_adapter::LiveContinuityMode`].
#
# Internally-tagged on `mode` (snake_case) — matches the core enum's serde
# shape exactly so the wire payload is byte-identical. CC6: closes the
# typed-surface gap. SDK consumers get a discriminated union (TS) or
# tagged-variant (Python) instead of a raw JSON blob, and the
# `ProviderNativeResume { provider_session_id }` payload field is visible
# to schema codegen.
#
# **Breaking-change note (pre-1.0 dogma, no shims):** T12 already moved the
# core enum from a bare-string serde shape (`"transcript_only"`) to the
# internally-tagged form (`{"mode":"transcript_only"}`). This wire mirror
# makes that shape change visible to schema codegen and SDK clients.
class WireLiveContinuityModeFresh(TypedDict, total=False):
    mode: Required[Literal['fresh']]

class WireLiveContinuityModeTranscriptOnly(TypedDict, total=False):
    mode: Required[Literal['transcript_only']]

class WireLiveContinuityModeDegraded(TypedDict, total=False):
    mode: Required[Literal['degraded']]

class WireLiveContinuityModeProviderNativeResume(TypedDict, total=False):
    mode: Required[Literal['provider_native_resume']]
    provider_session_id: Required[str]

class WireLiveContinuityModeUnknown(TypedDict, total=False):
    debug: Required[str]
    mode: Required[Literal['unknown']]

WireLiveContinuityMode = WireLiveContinuityModeFresh | WireLiveContinuityModeTranscriptOnly | WireLiveContinuityModeDegraded | WireLiveContinuityModeProviderNativeResume | WireLiveContinuityModeUnknown

# Wire projection of [`meerkat_core::live_adapter::LiveTransportBootstrap`].
#
# Internally-tagged on `transport` (snake_case) — matches the core enum's
# serde shape exactly so the wire payload is byte-identical. G8 (P2):
# closes the typed-surface gap that left `transport: unknown` in TS and
# `Any` in Python at the SDK boundary.
#
# The core enum is `#[non_exhaustive]`; new transports (e.g. WebRTC
# reintroduction per Round-4 T4) appear here as additional typed variants
# rather than as a free-form JSON blob.
class WireLiveTransportBootstrapWebsocket(TypedDict, total=False):
    token: Required[str]
    transport: Required[Literal['websocket']]
    url: Required[str]

class WireLiveTransportBootstrapWebrtc(TypedDict, total=False):
    answer_method: Required[str]
    http_url: NotRequired[Optional[str]]
    token: Required[str]
    transport: Required[Literal['webrtc']]

class WireLiveTransportBootstrapUnknown(TypedDict, total=False):
    debug: Required[str]
    transport: Required[Literal['unknown']]

WireLiveTransportBootstrap = WireLiveTransportBootstrapWebsocket | WireLiveTransportBootstrapWebrtc | WireLiveTransportBootstrapUnknown

@dataclass
class LiveOpenResult:
    """Response payload for `live/open`.

`capabilities`, `continuity`, and `transport` are typed wire-side mirrors
of the core `LiveChannelCapabilities` / `LiveContinuityMode` /
`LiveTransportBootstrap` so SDK codegen sees the real shape (typed
booleans, internally-tagged variant payloads, discriminated transport
union) instead of an opaque JSON blob. CC5/CC6 (PR #650 verifier
follow-up); G8 (P2): `transport` typed-mirror.

`Eq`: all fields are strings/enums/bools, and the bridge reply chain
(`BridgeReply::MemberLiveChannelOpened`) requires it."""
    capabilities: WireLiveChannelCapabilities
    channel_id: str
    continuity: WireLiveContinuityMode
    transport: WireLiveTransportBootstrap


@dataclass
class LiveWebrtcAnswerParams:
    """Request payload for `live/webrtc/answer`."""
    channel_id: str
    offer_sdp: str
    token: str


@dataclass
class LiveWebrtcAnswerResult:
    """Response payload for `live/webrtc/answer`."""
    answer_sdp: str


@dataclass
class LiveChannelParams:
    """Request payload for `live/status`, `live/close`, and `live/interrupt`.
They all take the same `{channel_id}` shape; this struct is the typed
name for it.

`live/commit_input` no longer uses this shape — it carries an optional
`response_modality` override (G9) so callers can request a text-only
response on an audio-first channel without flipping the channel
modality. See [`LiveCommitInputParams`]."""
    channel_id: str


@dataclass
class LiveStatusResult:
    """Response payload for `live/status`.

R6-3 (P2): `status` is now the typed [`WireLiveAdapterStatus`] mirror
(with R5-3's `Unknown { debug }` floor) instead of the core
[`LiveAdapterStatus`] under a `schemars(with = "serde_json::Value")`
shroud. SDK codegen emits the typed discriminated union (TS) / typed
dict union (Python) for `live/status` instead of `unknown` / `Any`.
The runtime handler converts core → wire at the boundary; clients
that fully understood the previous JSON shape see byte-identical
payloads (the wire mirror serializes byte-compatible with the core
enum — see `wire_live_adapter_status_byte_compatible_with_core`)."""
    channel_id: str
    status: WireLiveAdapterStatus


@dataclass
class LiveSendInputParams:
    """Request payload for `live/send_input`.

**`BREAKING_LIVE_WIRE_FORMAT_V1`** (H48): `chunk` is a nested object, not
a flattened sibling of `channel_id`. WS protocol clients that piggyback on
this shape must use the nested form."""
    channel_id: str
    chunk: LiveInputChunkWire


@dataclass
class LiveSendInputErrorData:
    """Typed JSON-RPC error data for scoped `live/send_input` rejections.

The channel remains usable; callers route on the structured adapter error
code instead of parsing the human-readable JSON-RPC error message."""
    error_code: WireLiveAdapterErrorCode


@dataclass
class LiveTruncateParams:
    """Request payload for `live/truncate`.

`item_id` and `content_index` are the provider-side handle for the
assistant item being truncated; `audio_played_ms` is the client-tracked
playback cursor at the moment of truncation. There is no server-side
playback-cursor read API — clients track playback locally and pass the
cursor in here when they want to truncate."""
    audio_played_ms: int
    channel_id: str
    content_index: int
    item_id: str


# Wire projection of [`meerkat_core::live_adapter::LiveResponseModality`].
#
# Internally-tagged on `modality` (snake_case) — matches the core enum's
# serde shape exactly so the wire payload is byte-identical. G9: closes
# the typed-surface gap so SDK clients can pick `audio` vs `text` on a
# per-response basis without round-tripping through a free-form string.
#
# The core enum is `#[non_exhaustive]`; new modalities (e.g. structured
# output, image) appear here as additional typed variants.
#
# R5-3 (P3): no longer `Copy` because `Unknown { debug: String }` carries
# an owned payload. The enum is small and conversions across the boundary
# move-or-clone, so the loss of `Copy` is not material at call sites.
class WireLiveResponseModalityAudio(TypedDict, total=False):
    modality: Required[Literal['audio']]

class WireLiveResponseModalityText(TypedDict, total=False):
    modality: Required[Literal['text']]

class WireLiveResponseModalityUnknown(TypedDict, total=False):
    debug: Required[str]
    modality: Required[Literal['unknown']]

WireLiveResponseModality = WireLiveResponseModalityAudio | WireLiveResponseModalityText | WireLiveResponseModalityUnknown

@dataclass
class LiveCommitInputParams:
    """Request payload for `live/commit_input`.

G9: optional `response_modality` lets the caller request a text-only
response on an otherwise-audio channel without flipping the channel
modality. `None` keeps the channel default (`audio` for the OpenAI
realtime adapter today)."""
    channel_id: str
    response_modality: Optional[WireLiveResponseModality] = None


# Status of a `live/refresh` request relative to the adapter pump.
#
# R4-5 (P3): the refresh path is asynchronous — host queue acceptance happens
# before the adapter pump has applied the resulting `session.update`. The
# realtime stream is the source of truth for the actual refresh outcome
# (failures surface as `LiveAdapterObservation::Error`).
#
# Today generated runtime authority emits only `Queued`. The enum is
# `#[non_exhaustive]` so a future generated contract can add a typed status
# without changing the object shape. SDK consumers must route on the string
# value and fail closed for values outside the generated contract they were
# built with.
#
# Serializes as a plain string (no envelope) so [`LiveRefreshResult`] keeps
# SDK codegen on the simple-struct path.
LiveRefreshStatus = Literal['queued']

@dataclass
class LiveRefreshResult:
    """Response payload for `live/refresh`.

R4-5 (P3): replaces the previous untyped `{"refresh_enqueued": true}`
JSON blob with the typed `status` discriminator. Clients route on
`status`.

See [`LiveRefreshStatus`] for the variant set and the contract on
asynchronous adapter-pump application."""
    status: Literal['queued']


# Typed public result class for `live/close`.
#
# Today generated runtime authority emits only `Closed`. The enum is
# `#[non_exhaustive]` so future generated contracts can add explicit result
# classes without changing the object shape.
LiveCloseStatus = Literal['closed']

@dataclass
class LiveCloseResult:
    """Response payload for `live/close`.

Clients route on the typed `status` discriminator."""
    status: Literal['closed']


# Typed public result class for `live/send_input`.
#
# Today generated runtime authority emits only `Sent`. The enum is
# `#[non_exhaustive]` so future generated contracts can add explicit result
# classes without changing the object shape.
LiveSendInputStatus = Literal['sent']

@dataclass
class LiveSendInputResult:
    """Response payload for `live/send_input`.

Clients route on the typed `status` discriminator."""
    status: Literal['sent']


# Typed public result class for `live/commit_input`.
#
# Today generated runtime authority emits only `Committed`. The enum is
# `#[non_exhaustive]` so future generated contracts can add explicit result
# classes without changing the object shape.
LiveCommitInputStatus = Literal['committed']

@dataclass
class LiveCommitInputResult:
    """Response payload for `live/commit_input`.

Clients route on the typed `status` discriminator."""
    status: Literal['committed']


# Typed public result class for `live/interrupt`.
#
# Today generated runtime authority emits only `Interrupted`. The enum is
# `#[non_exhaustive]` so future generated contracts can add explicit result
# classes without changing the object shape.
LiveInterruptStatus = Literal['interrupted']

@dataclass
class LiveInterruptResult:
    """Response payload for `live/interrupt`.

Clients route on the typed `status` discriminator."""
    status: Literal['interrupted']


# Typed public result class for `live/truncate`.
#
# Today generated runtime authority emits only `Truncated`. The enum is
# `#[non_exhaustive]` so future generated contracts can add explicit result
# classes without changing the object shape.
LiveTruncateStatus = Literal['truncated']

@dataclass
class LiveTruncateResult:
    """Response payload for `live/truncate`.

Clients route on the typed `status` discriminator."""
    status: Literal['truncated']


# Public-wire mirror of [`RealtimeTranscriptEvent`].
#
# The core event stream also contains
# [`RealtimeTranscriptEvent::UserContentFinal`], an internal canonical-state
# command that may carry inline image bytes. That variant is deliberately
# absent here, making private user content unrepresentable on
# [`WireLiveAdapterObservation`] by construction rather than relying on each
# transport to remember an output filter.
#
# The schema retains the historical `RealtimeTranscriptEvent` name so SDK
# consumers keep the existing generated type name while the Rust public wire
# surface gains a distinct, safe type.
class RealtimeTranscriptEventItemObserved(TypedDict, total=False):
    item_id: Required[str]
    previous_item_id: NotRequired[Optional[str]]
    response_id: NotRequired[Optional[str]]
    role: Required[RealtimeTranscriptRole]
    type: Required[Literal['item_observed']]

class RealtimeTranscriptEventItemSkipped(TypedDict, total=False):
    item_id: Required[str]
    previous_item_id: NotRequired[Optional[str]]
    type: Required[Literal['item_skipped']]

class RealtimeTranscriptEventUserTranscriptFinal(TypedDict, total=False):
    content_index: Required[int]
    item_id: Required[str]
    previous_item_id: NotRequired[Optional[str]]
    text: Required[str]
    type: Required[Literal['user_transcript_final']]

class RealtimeTranscriptEventAssistantTextDelta(TypedDict, total=False):
    content_index: Required[int]
    delta: Required[str]
    delta_id: Required[str]
    item_id: Required[str]
    previous_item_id: NotRequired[Optional[str]]
    response_id: Required[str]
    type: Required[Literal['assistant_text_delta']]

class RealtimeTranscriptEventAssistantTranscriptDelta(TypedDict, total=False):
    content_index: Required[int]
    delta: Required[str]
    delta_id: Required[str]
    item_id: Required[str]
    previous_item_id: NotRequired[Optional[str]]
    response_id: Required[str]
    type: Required[Literal['assistant_transcript_delta']]

class RealtimeTranscriptEventAssistantTranscriptTruncated(TypedDict, total=False):
    content_index: Required[int]
    item_id: Required[str]
    response_id: Required[str]
    text: Required[str]
    type: Required[Literal['assistant_transcript_truncated']]

class RealtimeTranscriptEventAssistantTranscriptFinalText(TypedDict, total=False):
    content_index: Required[int]
    item_id: Required[str]
    response_id: Required[str]
    text: Required[str]
    type: Required[Literal['assistant_transcript_final_text']]

class RealtimeTranscriptEventAssistantTurnCompleted(TypedDict, total=False):
    response_id: Required[str]
    stop_reason: Required[Literal['end_turn', 'tool_use', 'max_tokens', 'stop_sequence', 'content_filter', 'cancelled']]
    type: Required[Literal['assistant_turn_completed']]
    usage: Required[dict[str, Any]]

class RealtimeTranscriptEventAssistantTurnInterrupted(TypedDict, total=False):
    response_id: Required[str]
    type: Required[Literal['assistant_turn_interrupted']]

RealtimeTranscriptEvent = RealtimeTranscriptEventItemObserved | RealtimeTranscriptEventItemSkipped | RealtimeTranscriptEventUserTranscriptFinal | RealtimeTranscriptEventAssistantTextDelta | RealtimeTranscriptEventAssistantTranscriptDelta | RealtimeTranscriptEventAssistantTranscriptTruncated | RealtimeTranscriptEventAssistantTranscriptFinalText | RealtimeTranscriptEventAssistantTurnCompleted | RealtimeTranscriptEventAssistantTurnInterrupted

# Provider-neutral role for a realtime transcript item.
RealtimeTranscriptRole = Literal['user', 'assistant']

# Wire mirror of [`meerkat_core::live_adapter::LiveDegradationReason`].
#
# Internally-tagged on `kind` (snake_case) — matches the core enum's serde
# shape exactly. SDK consumers route on `kind` to distinguish the
# non-payload variants from the typed `other { detail }` payload.
class WireLiveDegradationReasonRateLimited(TypedDict, total=False):
    kind: Required[Literal['rate_limited']]

class WireLiveDegradationReasonProviderThrottled(TypedDict, total=False):
    kind: Required[Literal['provider_throttled']]

class WireLiveDegradationReasonNetworkUnstable(TypedDict, total=False):
    kind: Required[Literal['network_unstable']]

class WireLiveDegradationReasonOther(TypedDict, total=False):
    detail: Required[str]
    kind: Required[Literal['other']]

class WireLiveDegradationReasonUnknown(TypedDict, total=False):
    debug: Required[str]
    kind: Required[Literal['unknown']]

WireLiveDegradationReason = WireLiveDegradationReasonRateLimited | WireLiveDegradationReasonProviderThrottled | WireLiveDegradationReasonNetworkUnstable | WireLiveDegradationReasonOther | WireLiveDegradationReasonUnknown

# Wire mirror of [`meerkat_core::live_adapter::LiveAdapterStatus`].
#
# Internally-tagged on `status` (snake_case). The `degraded` variant
# references [`WireLiveDegradationReason`] so the typed reason is visible at
# the SDK boundary.
class WireLiveAdapterStatusIdle(TypedDict, total=False):
    status: Required[Literal['idle']]

class WireLiveAdapterStatusOpening(TypedDict, total=False):
    status: Required[Literal['opening']]

class WireLiveAdapterStatusReady(TypedDict, total=False):
    status: Required[Literal['ready']]

class WireLiveAdapterStatusDegraded(TypedDict, total=False):
    reason: Required[WireLiveDegradationReason]
    status: Required[Literal['degraded']]

class WireLiveAdapterStatusClosing(TypedDict, total=False):
    status: Required[Literal['closing']]

class WireLiveAdapterStatusClosed(TypedDict, total=False):
    status: Required[Literal['closed']]

class WireLiveAdapterStatusUnknown(TypedDict, total=False):
    debug: Required[str]
    status: Required[Literal['unknown']]

WireLiveAdapterStatus = WireLiveAdapterStatusIdle | WireLiveAdapterStatusOpening | WireLiveAdapterStatusReady | WireLiveAdapterStatusDegraded | WireLiveAdapterStatusClosing | WireLiveAdapterStatusClosed | WireLiveAdapterStatusUnknown

# Wire projection of LiveConfigRejectionReason (tagged on `kind`).
class WireLiveConfigRejectionReasonChannelIdentitySwap(TypedDict, total=False):
    auth_binding_changed: NotRequired[bool]
    from_model: Required[str]
    from_provider: Required[WireProvider]
    kind: Required[Literal['channel_identity_swap']]
    to_model: Required[str]
    to_provider: Required[WireProvider]

class WireLiveConfigRejectionReasonNonRealtimeResolution(TypedDict, total=False):
    detail: Required[str]
    kind: Required[Literal['non_realtime_resolution']]

class WireLiveConfigRejectionReasonImageInputNotImplemented(TypedDict, total=False):
    kind: Required[Literal['image_input_not_implemented']]

class WireLiveConfigRejectionReasonImageInputUnsupportedMime(TypedDict, total=False):
    kind: Required[Literal['image_input_unsupported_mime']]
    mime_type: Required[str]

class WireLiveConfigRejectionReasonImageInputContentMismatch(TypedDict, total=False):
    kind: Required[Literal['image_input_content_mismatch']]
    mime_type: Required[str]

class WireLiveConfigRejectionReasonImageInputInvalidBase64(TypedDict, total=False):
    kind: Required[Literal['image_input_invalid_base64']]

class WireLiveConfigRejectionReasonImageInputTooLarge(TypedDict, total=False):
    actual_bytes: Required[int]
    kind: Required[Literal['image_input_too_large']]
    max_bytes: Required[int]

class WireLiveConfigRejectionReasonImageInputIdempotencyKeyInvalid(TypedDict, total=False):
    actual_bytes: Required[int]
    kind: Required[Literal['image_input_idempotency_key_invalid']]
    max_bytes: Required[int]

class WireLiveConfigRejectionReasonImageInputIdempotencyConflict(TypedDict, total=False):
    kind: Required[Literal['image_input_idempotency_conflict']]

class WireLiveConfigRejectionReasonImageInputHistoryBudgetExceeded(TypedDict, total=False):
    kind: Required[Literal['image_input_history_budget_exceeded']]
    max_decoded_bytes: Required[int]

class WireLiveConfigRejectionReasonImageInputRequiresCommit(TypedDict, total=False):
    kind: Required[Literal['image_input_requires_commit']]

class WireLiveConfigRejectionReasonInputTooLarge(TypedDict, total=False):
    actual_bytes: Required[int]
    kind: Required[Literal['input_too_large']]
    max_bytes: Required[int]

class WireLiveConfigRejectionReasonInputBackpressured(TypedDict, total=False):
    kind: Required[Literal['input_backpressured']]
    max_pending_bytes: Required[int]

class WireLiveConfigRejectionReasonImageInputBackpressured(TypedDict, total=False):
    kind: Required[Literal['image_input_backpressured']]
    max_pending_bytes: Required[int]

class WireLiveConfigRejectionReasonImageInputTransportUnsupported(TypedDict, total=False):
    kind: Required[Literal['image_input_transport_unsupported']]
    transport: Required[str]

class WireLiveConfigRejectionReasonVideoFrameInputNotImplemented(TypedDict, total=False):
    kind: Required[Literal['video_frame_input_not_implemented']]

class WireLiveConfigRejectionReasonUnsupportedInputChunkVariant(TypedDict, total=False):
    kind: Required[Literal['unsupported_input_chunk_variant']]

class WireLiveConfigRejectionReasonRefreshModelSwap(TypedDict, total=False):
    from_model: Required[str]
    kind: Required[Literal['refresh_model_swap']]
    to_model: Required[str]

class WireLiveConfigRejectionReasonRefreshProviderSwap(TypedDict, total=False):
    from_provider: Required[WireProvider]
    kind: Required[Literal['refresh_provider_swap']]
    to_provider: Required[WireProvider]

class WireLiveConfigRejectionReasonRefreshAudioConfigMismatch(TypedDict, total=False):
    detail: Required[str]
    kind: Required[Literal['refresh_audio_config_mismatch']]

class WireLiveConfigRejectionReasonRefreshTranscriptRewriteRequiresReopen(TypedDict, total=False):
    kind: Required[Literal['refresh_transcript_rewrite_requires_reopen']]

class WireLiveConfigRejectionReasonAudioInputFormatMismatch(TypedDict, total=False):
    actual_channels: Required[int]
    actual_sample_rate_hz: Required[int]
    expected_channels: Required[int]
    expected_sample_rate_hz: Required[int]
    kind: Required[Literal['audio_input_format_mismatch']]

class WireLiveConfigRejectionReasonOther(TypedDict, total=False):
    detail: Required[str]
    kind: Required[Literal['other']]

class WireLiveConfigRejectionReasonUnknown(TypedDict, total=False):
    debug: Required[str]
    kind: Required[Literal['unknown']]

WireLiveConfigRejectionReason = WireLiveConfigRejectionReasonChannelIdentitySwap | WireLiveConfigRejectionReasonNonRealtimeResolution | WireLiveConfigRejectionReasonImageInputNotImplemented | WireLiveConfigRejectionReasonImageInputUnsupportedMime | WireLiveConfigRejectionReasonImageInputContentMismatch | WireLiveConfigRejectionReasonImageInputInvalidBase64 | WireLiveConfigRejectionReasonImageInputTooLarge | WireLiveConfigRejectionReasonImageInputIdempotencyKeyInvalid | WireLiveConfigRejectionReasonImageInputIdempotencyConflict | WireLiveConfigRejectionReasonImageInputHistoryBudgetExceeded | WireLiveConfigRejectionReasonImageInputRequiresCommit | WireLiveConfigRejectionReasonInputTooLarge | WireLiveConfigRejectionReasonInputBackpressured | WireLiveConfigRejectionReasonImageInputBackpressured | WireLiveConfigRejectionReasonImageInputTransportUnsupported | WireLiveConfigRejectionReasonVideoFrameInputNotImplemented | WireLiveConfigRejectionReasonUnsupportedInputChunkVariant | WireLiveConfigRejectionReasonRefreshModelSwap | WireLiveConfigRejectionReasonRefreshProviderSwap | WireLiveConfigRejectionReasonRefreshAudioConfigMismatch | WireLiveConfigRejectionReasonRefreshTranscriptRewriteRequiresReopen | WireLiveConfigRejectionReasonAudioInputFormatMismatch | WireLiveConfigRejectionReasonOther | WireLiveConfigRejectionReasonUnknown

# Wire mirror of [`meerkat_core::live_adapter::LiveAdapterErrorCode`].
#
# Internally-tagged on `code` (snake_case). SDK consumers route on `code`
# to distinguish payload-less variants from typed payload variants
# (`config_rejected { reason }`, `other { raw }`). FIX-SDK-OBS: makes the
# R5-9 `CommandRejected` observation's typed code visible at the SDK
# boundary instead of a free-form blob.
#
# R5-2 (P2 dogma): `config_rejected.reason` is now a typed
# [`WireLiveConfigRejectionReason`] mirror so SDK consumers route on the
# variant rather than parsing English from the previous free-form
# `String`.
class WireLiveAdapterErrorCodeConnectionFailed(TypedDict, total=False):
    code: Required[Literal['connection_failed']]

class WireLiveAdapterErrorCodeConnectionLost(TypedDict, total=False):
    code: Required[Literal['connection_lost']]

class WireLiveAdapterErrorCodeConfigRejected(TypedDict, total=False):
    code: Required[Literal['config_rejected']]
    reason: Required[WireLiveConfigRejectionReason]

class WireLiveAdapterErrorCodeProviderError(TypedDict, total=False):
    code: Required[Literal['provider_error']]

class WireLiveAdapterErrorCodeAuthenticationFailed(TypedDict, total=False):
    code: Required[Literal['authentication_failed']]

class WireLiveAdapterErrorCodeInternalError(TypedDict, total=False):
    code: Required[Literal['internal_error']]

class WireLiveAdapterErrorCodeOther(TypedDict, total=False):
    code: Required[Literal['other']]
    raw: Required[str]

class WireLiveAdapterErrorCodeUnknown(TypedDict, total=False):
    code: Required[Literal['unknown']]
    debug: Required[str]

WireLiveAdapterErrorCode = WireLiveAdapterErrorCodeConnectionFailed | WireLiveAdapterErrorCodeConnectionLost | WireLiveAdapterErrorCodeConfigRejected | WireLiveAdapterErrorCodeProviderError | WireLiveAdapterErrorCodeAuthenticationFailed | WireLiveAdapterErrorCodeInternalError | WireLiveAdapterErrorCodeOther | WireLiveAdapterErrorCodeUnknown

# Wire mirror of [`meerkat_core::live_adapter::LiveAdapterObservation`].
#
# FIX-SDK-OBS: closes the R5-4 verifier gap. The core enum is the canonical
# shape adapters emit, but it is not registered for schema emission and is
# therefore invisible at the SDK boundary — browser/Python clients receive
# observations as untyped JSON and cannot type-narrow on
# `assistant_audio_chunk` (to read the new `item_id` / `response_id` /
# `content_index` fields driving `live/truncate`) or on `command_rejected`
# (a typed channel-survives error introduced in R5-9). The wire mirror
# makes every variant visible to schema codegen and produces a discriminated
# TypeScript union / typed Python `TypedDict` union.
#
# Serde shape mirrors the public subset of the core enum: internally-tagged
# on `observation` (snake_case). Round-trip with public core variants is
# byte-identical (see `wire_live_adapter_observation_byte_compatible_with_core`).
#
# Field types reference other wire mirrors where they exist
# ([`WireStopReason`], [`WireUsage`], [`WireLiveAdapterStatus`],
# [`WireLiveAdapterErrorCode`]) and the public-safe
# [`WireRealtimeTranscriptEvent`].
#
# Audio data is base64-encoded on the wire (matches the core
# [`LiveAdapterObservation::AssistantAudioChunk`] base64 mode); the wire
# mirror carries `data` as a `String` so the schema emits `String` instead
# of an opaque `Vec<u8>` JSON-array shape.
class WireLiveAdapterObservationReady(TypedDict, total=False):
    observation: Required[Literal['ready']]

class WireLiveAdapterObservationUserTranscriptFinal(TypedDict, total=False):
    content_index: NotRequired[Optional[int]]
    observation: Required[Literal['user_transcript_final']]
    previous_item_id: NotRequired[Optional[str]]
    provider_item_id: NotRequired[Optional[str]]
    text: Required[str]

class WireLiveAdapterObservationAssistantTextDelta(TypedDict, total=False):
    content_index: NotRequired[Optional[int]]
    delta: Required[str]
    delta_id: NotRequired[Optional[str]]
    observation: Required[Literal['assistant_text_delta']]
    previous_item_id: NotRequired[Optional[str]]
    provider_item_id: NotRequired[Optional[str]]
    response_id: NotRequired[Optional[str]]

class WireLiveAdapterObservationAssistantTranscriptDelta(TypedDict, total=False):
    content_index: NotRequired[Optional[int]]
    delta: Required[str]
    delta_id: NotRequired[Optional[str]]
    observation: Required[Literal['assistant_transcript_delta']]
    previous_item_id: NotRequired[Optional[str]]
    provider_item_id: NotRequired[Optional[str]]
    response_id: NotRequired[Optional[str]]

class WireLiveAdapterObservationAssistantAudioChunk(TypedDict, total=False):
    channels: Required[int]
    content_index: NotRequired[Optional[int]]
    data: Required[str]
    item_id: NotRequired[Optional[str]]
    observation: Required[Literal['assistant_audio_chunk']]
    response_id: NotRequired[Optional[str]]
    sample_rate_hz: Required[int]

class WireLiveAdapterObservationAssistantTranscriptFinal(TypedDict, total=False):
    content_index: NotRequired[Optional[int]]
    observation: Required[Literal['assistant_transcript_final']]
    previous_item_id: NotRequired[Optional[str]]
    provider_item_id: Required[str]
    response_id: NotRequired[Optional[str]]
    stop_reason: Required[Literal['end_turn', 'tool_use', 'max_tokens', 'stop_sequence', 'content_filter', 'cancelled']]
    text: Required[str]
    usage: Required[dict[str, Any]]

class WireLiveAdapterObservationAssistantTranscriptTruncated(TypedDict, total=False):
    content_index: NotRequired[Optional[int]]
    observation: Required[Literal['assistant_transcript_truncated']]
    previous_item_id: NotRequired[Optional[str]]
    provider_item_id: NotRequired[Optional[str]]
    response_id: NotRequired[Optional[str]]
    text: NotRequired[Optional[str]]

class WireLiveAdapterObservationRealtimeTranscript(TypedDict, total=False):
    event: Required[RealtimeTranscriptEvent]
    observation: Required[Literal['realtime_transcript']]

class WireLiveAdapterObservationUserContentCommitted(TypedDict, total=False):
    content_index: Required[int]
    idempotency_key: Required[str]
    item_id: Required[str]
    media_type: Required[str]
    observation: Required[Literal['user_content_committed']]
    previous_item_id: NotRequired[Optional[str]]

class WireLiveAdapterObservationToolCallRequested(TypedDict, total=False):
    arguments: Required[Any]
    observation: Required[Literal['tool_call_requested']]
    provider_call_id: Required[str]
    tool_name: Required[str]

class WireLiveAdapterObservationTurnInterrupted(TypedDict, total=False):
    observation: Required[Literal['turn_interrupted']]
    response_id: NotRequired[Optional[str]]

class WireLiveAdapterObservationTurnCompleted(TypedDict, total=False):
    observation: Required[Literal['turn_completed']]
    response_id: NotRequired[Optional[str]]
    stop_reason: Required[Literal['end_turn', 'tool_use', 'max_tokens', 'stop_sequence', 'content_filter', 'cancelled']]
    usage: Required[dict[str, Any]]

class WireLiveAdapterObservationStatusChanged(TypedDict, total=False):
    observation: Required[Literal['status_changed']]
    status: Required[WireLiveAdapterStatus]

class WireLiveAdapterObservationError(TypedDict, total=False):
    code: Required[WireLiveAdapterErrorCode]
    message: Required[str]
    observation: Required[Literal['error']]

class WireLiveAdapterObservationCommandRejected(TypedDict, total=False):
    code: Required[WireLiveAdapterErrorCode]
    message: Required[str]
    observation: Required[Literal['command_rejected']]

class WireLiveAdapterObservationUnknown(TypedDict, total=False):
    debug: Required[str]
    observation: Required[Literal['unknown']]

WireLiveAdapterObservation = WireLiveAdapterObservationReady | WireLiveAdapterObservationUserTranscriptFinal | WireLiveAdapterObservationAssistantTextDelta | WireLiveAdapterObservationAssistantTranscriptDelta | WireLiveAdapterObservationAssistantAudioChunk | WireLiveAdapterObservationAssistantTranscriptFinal | WireLiveAdapterObservationAssistantTranscriptTruncated | WireLiveAdapterObservationRealtimeTranscript | WireLiveAdapterObservationUserContentCommitted | WireLiveAdapterObservationToolCallRequested | WireLiveAdapterObservationTurnInterrupted | WireLiveAdapterObservationTurnCompleted | WireLiveAdapterObservationStatusChanged | WireLiveAdapterObservationError | WireLiveAdapterObservationCommandRejected | WireLiveAdapterObservationUnknown

@dataclass
class RuntimeAcceptResult:
    """Response payload for runtime-backed input submission."""
    outcome_type: Literal['accepted', 'deduplicated', 'rejected']
    existing_id: Optional[str] = None
    input_id: Optional[str] = None
    policy: Optional[Literal['stage', 'queue', 'immediate']] = None
    reason: Optional[str] = None
    state: Optional[dict[str, Any]] = None


@dataclass
class WireInputStateHistoryEntry:
    """Input transition history entry for RPC-facing snapshots."""
    from_: Literal['accepted', 'queued', 'staged', 'applied', 'applied_pending_consumption', 'consumed', 'superseded', 'coalesced', 'abandoned']
    timestamp: str
    to: Literal['accepted', 'queued', 'staged', 'applied', 'applied_pending_consumption', 'consumed', 'superseded', 'coalesced', 'abandoned']
    reason: Optional[str] = None


@dataclass
class WireInputState:
    """RPC-facing input state snapshot.

All fields are typed. Wave B replaced six former `serde_json::Value`
fields with typed projections so the wire carries no untyped carriers."""
    created_at: str
    current_state: Literal['accepted', 'queued', 'staged', 'applied', 'applied_pending_consumption', 'consumed', 'superseded', 'coalesced', 'abandoned']
    input_id: str
    updated_at: str
    attempt_count: Optional[int] = None
    durability: Optional[Literal['durable', 'volatile', 'ephemeral']] = None
    history: Optional[list[WireInputStateHistoryEntry]] = None
    idempotency_key: Optional[str] = None
    last_boundary_sequence: Optional[int] = None
    last_run_id: Optional[str] = None
    persisted_input: Optional[dict[str, Any]] = None
    policy: Optional[Literal['stage', 'queue', 'immediate']] = None
    reconstruction_source: Optional[Literal['live', 'event_store', 'snapshot', 'replay']] = None
    recovery_count: Optional[int] = None
    terminal_outcome: Optional[Literal['completed', 'abandoned', 'superseded', 'coalesced', 'cancelled']] = None


@dataclass
class ScheduleListResult:
    """Response payload for schedule/list."""
    schedules: list[Schedule]


@dataclass
class ScheduleOccurrencesResult:
    """Response payload for schedule/occurrences."""
    occurrences: list[dict[str, Any]]


@dataclass
class WireResolvedModelCapabilities:
    """Stable resolved model/session/member capability projection.

This is the UI-facing capability shape for already-resolved identities.
Static catalog entries can expose the same bits, but callers should read
this projection from session/member surfaces when identity can change at
runtime."""
    image_generation: Optional[bool] = None
    image_input: Optional[bool] = None
    image_tool_results: Optional[bool] = None
    inline_video: Optional[bool] = None
    realtime: Optional[bool] = None
    vision: Optional[bool] = None
    web_search: Optional[bool] = None


@dataclass
class WireSessionInfo:
    """Canonical session info for wire protocol."""
    created_at: int
    is_active: bool
    message_count: int
    model: str
    provider: str
    session_id: str
    updated_at: int
    labels: Optional[dict[str, str]] = None
    last_assistant_text: Optional[str] = None
    resolved_capabilities: Optional[WireResolvedModelCapabilities] = None
    session_ref: Optional[str] = None


@dataclass
class WireSessionSummary:
    """Canonical session summary for wire protocol."""
    created_at: int
    is_active: bool
    message_count: int
    session_id: str
    total_tokens: int
    updated_at: int
    labels: Optional[dict[str, str]] = None
    session_ref: Optional[str] = None


@dataclass
class ContractVersion:
    """Semantic version for the Meerkat wire contract.

Start at 0.1.0 — allow breaking changes during Phase 1-3 development.
Bump to 1.0.0 when the SDK Builder ships (Phase 5) and external
consumers exist. Before 1.0.0, minor bumps can be breaking."""
    major: int
    minor: int
    patch: int


@dataclass
class CatalogModelEntry:
    """A single model entry in the catalog response."""
    display_name: str
    id: str
    tier: Literal['recommended', 'supported']
    context_window: Optional[int] = None
    max_output_tokens: Optional[int] = None
    profile: Optional[dict[str, Any]] = None
    server_id: Optional[str] = None


@dataclass
class ProviderCatalog:
    """Provider-level grouping in the catalog response."""
    default_model_id: str
    models: list[CatalogModelEntry]
    provider: str


@dataclass
class ModelsCatalogResponse:
    """Response for `models/catalog` — the resolved model catalog.

This is **not** a pure compiled-in snapshot. Surfaces build it from the
config-backed `ModelRegistry`, which combines the compiled-in
`meerkat-models` catalog with any config-declared self-hosted aliases
(entries that carry a [`CatalogModelEntry::server_id`]). Two responses for
the same binary can therefore differ when the active config declares
different self-hosted models.

Provider resolution for these entries is exact-catalog-match, not name
prefix inference: an entry's provider is the one recorded for its catalog
id (or its self-hosted alias config), never inferred from a `claude-*` /
`gpt-*` / `gemini-*` name prefix. Uncatalogued model names resolve through
the registry, not by prefix guessing."""
    contract_version: ContractVersion
    providers: list[ProviderCatalog]


@dataclass
class WireModelProfile:
    """Runtime profile for a model — capabilities and parameter schema."""
    model_family: str
    params_schema: Any
    supports_reasoning: bool
    supports_temperature: bool
    supports_thinking: bool
    beta_headers: Optional[list[dict[str, Any]]] = None
    image_generation: Optional[bool] = None
    image_input: Optional[bool] = None
    image_tool_results: Optional[bool] = None
    inline_video: Optional[bool] = None
    realtime: Optional[bool] = None
    supports_web_search: Optional[bool] = None
    vision: Optional[bool] = None


@dataclass
class WireAssistantImageRef:
    """Generated assistant image reference."""
    blob_ref: BlobRef
    height: int
    image_id: AssistantImageId
    media_type: str
    width: int


@dataclass
class WireGenerateImageRequest:
    """Canonical generate_image request payload."""
    count: int
    format: Literal['auto', 'png', 'jpeg', 'webp']
    intent: dict[str, Any]
    quality: Literal['auto', 'low', 'medium', 'high']
    size: dict[str, Any]
    target: dict[str, Any]
    provider_params: Optional[Any] = None


@dataclass
class WireGenerateImageExecutionPlan:
    """Provider-owned image generation execution plan."""
    backend: Literal['hosted_tool', 'provider_api', 'native_model']
    capabilities: dict[str, Any]
    max_count: int
    provider: str
    requires_scoped_override: bool
    provider_plan: Optional[Any] = None


@dataclass
class WireImageGenerationToolResult:
    """Canonical generate_image tool result payload."""
    native_metadata: ProviderImageMetadata
    operation_id: str
    provider_text: dict[str, Any]
    revised_prompt: RevisedPromptDisposition
    terminal: dict[str, Any]
    images: Optional[list[dict[str, Any]]] = None
    warnings: Optional[list[dict[str, Any]]] = None


@dataclass
class WireAuthBindingRef:
    """Wire projection of [`meerkat_core::AuthBindingRef`].

Pure structural shape — no `"realm:binding"` string form. Wave-b deleted
`parse` and `Display` on both the core type and the wire projection so
the colon-joined form cannot travel across wire boundaries."""
    binding: str
    realm: str
    profile: Optional[str] = None


@dataclass
class WireBackendProfile:
    """Wire projection of [`meerkat_core::BackendProfile`]."""
    backend_kind: str
    id: str
    provider: str
    base_url: Optional[str] = None
    options: Optional[Any] = None


@dataclass
class WireAuthProfile:
    """Wire projection of [`meerkat_core::AuthProfile`]. Sensitive credential
material is NOT wire-projected; callers that inspect profile metadata use
the server-side `auth.profile.get` RPC or the
`GET /auth/bindings/{binding_id}` REST path; both return typed redacted
shapes. `source_kind` is a
discriminator for the credential-source variant."""
    auth_method: str
    id: str
    provider: str
    source_kind: str


@dataclass
class WireProviderBinding:
    """Wire projection of [`meerkat_core::ProviderBinding`]."""
    auth_profile: str
    backend_profile: str
    id: str
    allow_auth_override: Optional[bool] = None
    default_model: Optional[str] = None
    require_metadata_account: Optional[bool] = None
    require_metadata_workspace: Optional[bool] = None


@dataclass
class WireRealmConnectionSet:
    """Wire projection of [`meerkat_core::RealmConnectionSet`]. Returned
from the `realm/get` / `GET /realm/:id` endpoints."""
    auth_profiles: dict[str, WireAuthProfile]
    backends: dict[str, WireBackendProfile]
    bindings: dict[str, WireProviderBinding]
    realm_id: str
    default_binding: Optional[str] = None


@dataclass
class WireBindingIdentity:
    """Identifies a binding inside a realm on the wire. Shared by every
auth REST response that returns a `{realm_id, binding_id, auth_binding}`
trio. Built from a typed [`meerkat_core::AuthBindingRef`] so the three
fields always agree; the `realm_id`/`binding_id` strings carry the
slug form for wire consumers that key by string."""
    auth_binding: WireAuthBindingRef
    binding_id: str
    realm_id: str


@dataclass
class WireAuthProfileCreated:
    """`POST /auth/profiles` (create) success body."""
    auth_binding: WireAuthBindingRef
    auth_method: str
    binding_id: str
    profile_id: str
    provider: str
    realm_id: str
    stored: bool


@dataclass
class WireAuthProfileDetail:
    """`GET /auth/bindings/{binding_id}` success body."""
    auth_binding: WireAuthBindingRef
    auth_profile: WireAuthProfile
    binding_id: str
    profile_id: str


@dataclass
class WireAuthProfileCleared:
    """`DELETE /auth/bindings/{binding_id}` /
`POST /auth/bindings/{binding_id}/logout` success body."""
    auth_binding: WireAuthBindingRef
    binding_id: str
    cleared: bool
    profile_id: str
    realm_id: str


@dataclass
class WireLoginStart:
    """`POST /auth/login/start` success body."""
    authorize_url: str
    provider: str
    redirect_uri: str
    state: str


@dataclass
class WireLoginReady:
    """`POST /auth/login/complete` / ready leg of device-code success body.

The optional `state` field distinguishes the flat `POST
/auth/login/complete` response (no `state` set) from the device-code
ready leg (`state = "ready"`) which is part of the pending/slow_down/
access_denied/expired/ready tagged protocol."""
    auth_binding: WireAuthBindingRef
    binding_id: str
    has_refresh_token: bool
    profile_id: str
    provider: str
    realm_id: str
    scopes: list[str]
    expires_at: Optional[str] = None
    state: Optional[str] = None


@dataclass
class WireDeviceStart:
    """`POST /auth/login/device/start` success body."""
    device_code: str
    expires_in: int
    interval: int
    provider: str
    user_code: str
    verification_uri: str
    verification_uri_complete: Optional[str] = None


@dataclass
class WireRealmSummary:
    """Realm summary entry returned by `GET /realms`."""
    auth_profile_count: int
    backend_count: int
    binding_count: int
    realm_id: str
    default_binding: Optional[str] = None


@dataclass
class WireRealmList:
    """`GET /realms` success body."""
    realms: list[WireRealmSummary]


@dataclass
class WireAuthProfilesList:
    """`GET /auth/profiles` success body — realm-scoped lists of backend,
auth, and binding profiles."""
    auth_profiles: list[WireAuthProfile]
    backend_profiles: list[WireBackendProfile]
    bindings: list[WireProviderBinding]
    realm_id: str


@dataclass
class WireAuthStatus:
    """Wire projection of the auth-profile status. Returned from
`auth.status.get` / `GET /auth/bindings/{binding_id}/status`."""
    auth_method: str
    profile_id: str
    provider: str
    state: Literal['valid', 'expiring', 'expired', 'reauth_required', 'refresh_failed'] | Literal['released'] | Literal['absent'] | Literal['missing_credential']
    account_id: Optional[str] = None
    expires_at: Optional[str] = None
    last_error: Optional[dict[str, Any]] = None
    last_refresh_at: Optional[str] = None


@dataclass
class WireAuthStatusDetail:
    """`GET /auth/bindings/{binding_id}/status` success body. Richer than
[`WireAuthStatus`] — also carries `realm_id` / `binding_id` /
`auth_binding` / `has_refresh_token` so the caller can key by
binding directly."""
    auth_binding: WireAuthBindingRef
    auth_method: str
    binding_id: str
    has_refresh_token: bool
    profile_id: str
    provider: str
    realm_id: str
    state: Literal['valid', 'expiring', 'expired', 'reauth_required', 'refresh_failed'] | Literal['released'] | Literal['absent'] | Literal['missing_credential']
    account_id: Optional[str] = None
    expires_at: Optional[str] = None
    last_refresh_at: Optional[str] = None


# Stable wire kind for auth errors. Mirrors `meerkat_core::AuthErrorKind`
# on the wire as a normalized string.
class WireAuthErrorMissingSecret(TypedDict, total=False):
    kind: Required[Literal['missing_secret']]

class WireAuthErrorUnsupportedCombination(TypedDict, total=False):
    auth: Required[str]
    backend: Required[str]
    kind: Required[Literal['unsupported_combination']]

class WireAuthErrorMissingRequiredMetadata(TypedDict, total=False):
    field: Required[str]
    kind: Required[Literal['missing_required_metadata']]

class WireAuthErrorWorkspaceMismatch(TypedDict, total=False):
    kind: Required[Literal['workspace_mismatch']]

class WireAuthErrorExpired(TypedDict, total=False):
    kind: Required[Literal['expired']]

class WireAuthErrorStaleCredential(TypedDict, total=False):
    kind: Required[Literal['stale_credential']]

class WireAuthErrorRefreshRequired(TypedDict, total=False):
    kind: Required[Literal['refresh_required']]

class WireAuthErrorLeaseAbsent(TypedDict, total=False):
    kind: Required[Literal['lease_absent']]

class WireAuthErrorUserReauthRequired(TypedDict, total=False):
    kind: Required[Literal['user_reauth_required']]

class WireAuthErrorRefreshFailed(TypedDict, total=False):
    detail: Required[str]
    kind: Required[Literal['refresh_failed']]

class WireAuthErrorResolveRequired(TypedDict, total=False):
    detail: Required[str]
    kind: Required[Literal['resolve_required']]

class WireAuthErrorInteractiveLoginRequired(TypedDict, total=False):
    kind: Required[Literal['interactive_login_required']]

class WireAuthErrorHostOwnedUnavailable(TypedDict, total=False):
    kind: Required[Literal['host_owned_unavailable']]

class WireAuthErrorIo(TypedDict, total=False):
    detail: Required[str]
    kind: Required[Literal['io']]

class WireAuthErrorOther(TypedDict, total=False):
    detail: Required[str]
    kind: Required[Literal['other']]

WireAuthError = WireAuthErrorMissingSecret | WireAuthErrorUnsupportedCombination | WireAuthErrorMissingRequiredMetadata | WireAuthErrorWorkspaceMismatch | WireAuthErrorExpired | WireAuthErrorStaleCredential | WireAuthErrorRefreshRequired | WireAuthErrorLeaseAbsent | WireAuthErrorUserReauthRequired | WireAuthErrorRefreshFailed | WireAuthErrorResolveRequired | WireAuthErrorInteractiveLoginRequired | WireAuthErrorHostOwnedUnavailable | WireAuthErrorIo | WireAuthErrorOther

# Wire-safe content block (no `source_path` — internal only).
class WireContentBlockText(TypedDict, total=False):
    text: Required[str]
    type: Required[Literal['text']]

class WireContentBlockImageInline(TypedDict, total=False):
    media_type: Required[str]
    type: Required[Literal['image']]
    data: Required[str]
    source: Required[Literal['inline']]

class WireContentBlockImageBlob(TypedDict, total=False):
    media_type: Required[str]
    type: Required[Literal['image']]
    blob_id: Required[str]
    source: Required[Literal['blob']]

class WireContentBlockVideoInline(TypedDict, total=False):
    duration_ms: Required[int]
    media_type: Required[str]
    type: Required[Literal['video']]
    data: Required[str]
    source: Required[Literal['inline']]

class WireContentBlockVideoUri(TypedDict, total=False):
    duration_ms: Required[int]
    media_type: Required[str]
    type: Required[Literal['video']]
    source: Required[Literal['uri']]
    uri: Required[str]

class WireContentBlockStructured(TypedDict, total=False):
    data: Required[Any]
    type: Required[Literal['structured']]

class WireContentBlockUnknown(TypedDict, total=False):
    type: Required[Literal['unknown']]

WireContentBlock = WireContentBlockText | WireContentBlockImageInline | WireContentBlockImageBlob | WireContentBlockVideoInline | WireContentBlockVideoUri | WireContentBlockStructured | WireContentBlockUnknown

# Wire-safe content input (mirrors `ContentInput`).
WireContentInput = str | list[WireContentBlock]

# Supported LLM providers.
#
# `JsonSchema` is derived unconditionally (schemars is a non-optional
# meerkat-core dependency): config-owned types such as
# [`crate::config::CustomModelConfig`] embed the typed provider directly and
# derive their schemas without the `schema` feature.
Provider = Literal['anthropic', 'openai', 'gemini', 'self_hosted', 'other']

# Server-resolved opaque handle for a mob member.
#
# Encodes `{mob_id, agent_identity}` as a single base64url-encoded token
# that callers treat as opaque. The server resolves the current
# `AgentRuntimeId` and fence token against the live mob roster on every
# dispatch — clients never reason about `generation` or `fence_token`
# directly.
#
# Use [`WireMemberRef::encode`] to produce a token and
# [`WireMemberRef::decode`] inside an RPC handler to recover the
# `(mob_id, agent_identity)` pair before resolving against the runtime.
WireMemberRef = str

# Mob RPC helper wire type for WireMobBackendKind.
WireMobBackendKind = Literal['session', 'external']

# Profile fields that win over durable session metadata on resume.
#
# Wire twin of `meerkat_mob::ResumeOverrideField`; closed snake_case
# vocabulary, parsed fail-closed at the wire boundary.
WireMobResumeOverrideField = Literal['model', 'provider', 'provider_params']

# Runtime binding for spawn requests.
#
# First step toward identity-first mobs. Carries backend-specific binding
# details at spawn time. `External` requires typed process identity; callers
# do not supply raw comms peer IDs.
class WireRuntimeBindingSession(TypedDict, total=False):
    kind: Required[Literal['session']]

class WireRuntimeBindingExternal(TypedDict, total=False):
    address: Required[str]
    bootstrap_token: NotRequired[Optional[BridgeBootstrapToken]]
    identity: Required[WireTrustedPeerIdentity]
    kind: Required[Literal['external']]

WireRuntimeBinding = WireRuntimeBindingSession | WireRuntimeBindingExternal

# How a mob member should be launched by `mob/spawn`.
class WireMemberLaunchModeFresh(TypedDict, total=False):
    mode: Required[Literal['fresh']]

class WireMemberLaunchModeResume(TypedDict, total=False):
    bridge_session_id: Required[str]
    mode: Required[Literal['resume']]

class WireMemberLaunchModeFork(TypedDict, total=False):
    fork_context: NotRequired[WireForkContext]
    mode: Required[Literal['fork']]
    source_member_id: Required[str]

WireMemberLaunchMode = WireMemberLaunchModeFresh | WireMemberLaunchModeResume | WireMemberLaunchModeFork

# Conversation history scope used when forking a mob member.
class WireForkContextFullHistory(TypedDict, total=False):
    type: Required[Literal['full_history']]

class WireForkContextLastMessages(TypedDict, total=False):
    count: Required[int]
    type: Required[Literal['last_messages']]

WireForkContext = WireForkContextFullHistory | WireForkContextLastMessages

# Public tool access policy for a spawned member or delegated session fork.
class WireToolAccessPolicyInherit(TypedDict, total=False):
    type: Required[Literal['inherit']]

class WireToolAccessPolicyAllowList(TypedDict, total=False):
    type: Required[Literal['allow_list']]
    value: Required[list[str]]

class WireToolAccessPolicyDenyList(TypedDict, total=False):
    type: Required[Literal['deny_list']]
    value: Required[list[str]]

WireToolAccessPolicy = WireToolAccessPolicyInherit | WireToolAccessPolicyAllowList | WireToolAccessPolicyDenyList

# Pre-resolved tool filter inherited by a spawned mob member.
WireToolFilter = Literal['All'] | dict[str, list[str]]

# Mob RPC helper wire type for WireMemberState.
WireMemberState = Any

# Execution status mirroring `meerkat_mob::runtime::MobMemberStatus`.
WireMobMemberStatus = Literal['active', 'retiring', 'broken', 'completed', 'unknown']

# Mob RPC helper wire type for WireMobRuntimeMode.
WireMobRuntimeMode = Literal['autonomous_host', 'turn_driven']

# Typed failure cause for one failed `mob/spawn_many` member row.
MobSpawnManyFailureCause = Literal['profile_not_found', 'member_not_found', 'member_already_exists', 'not_externally_addressable', 'invalid_transition', 'wiring_error', 'bridge_command_rejected', 'member_restore_failed', 'kickoff_wait_timed_out', 'ready_wait_timed_out', 'definition_error', 'flow_not_found', 'flow_failed', 'run_not_found', 'run_canceled', 'flow_turn_timed_out', 'frame_depth_limit_exceeded', 'frame_atomic_persistence_unavailable', 'spec_revision_conflict', 'schema_validation', 'insufficient_targets', 'topology_violation', 'bridge_delivery_rejected', 'supervisor_escalation', 'unsupported_for_mode', 'missing_member_capability', 'reset_barrier', 'storage_error', 'session_error', 'comms_error', 'callback_pending', 'stale_fence_token', 'stale_event_cursor', 'work_not_found', 'internal']

# Typed status for one `mob/spawn_many` row.
MobSpawnManyResultStatus = Literal['spawned', 'failed']

# Typed payload for one `mob/spawn_many` row.
MobSpawnManyResultPayload = MobSpawnManySpawnedResult | MobSpawnManyFailedResult

# Mob RPC helper wire type for MobCollectionPolicyInput.
class MobCollectionPolicyInputAll(TypedDict, total=False):
    type: Required[Literal['all']]

class MobCollectionPolicyInputAny(TypedDict, total=False):
    type: Required[Literal['any']]

class MobCollectionPolicyInputQuorum(TypedDict, total=False):
    n: Required[int]
    type: Required[Literal['quorum']]

MobCollectionPolicyInput = MobCollectionPolicyInputAll | MobCollectionPolicyInputAny | MobCollectionPolicyInputQuorum

# Mob RPC helper wire type for MobConditionExprInput.
class MobConditionExprInputEq(TypedDict, total=False):
    op: Required[Literal['eq']]
    path: Required[str]
    value: Required[Any]

class MobConditionExprInputIn(TypedDict, total=False):
    op: Required[Literal['in']]
    path: Required[str]
    values: Required[list[Any]]

class MobConditionExprInputGt(TypedDict, total=False):
    op: Required[Literal['gt']]
    path: Required[str]
    value: Required[Any]

class MobConditionExprInputLt(TypedDict, total=False):
    op: Required[Literal['lt']]
    path: Required[str]
    value: Required[Any]

class MobConditionExprInputAnd(TypedDict, total=False):
    exprs: Required[list[MobConditionExprInput]]
    op: Required[Literal['and']]

class MobConditionExprInputOr(TypedDict, total=False):
    exprs: Required[list[MobConditionExprInput]]
    op: Required[Literal['or']]

class MobConditionExprInputNot(TypedDict, total=False):
    expr: Required[MobConditionExprInput]
    op: Required[Literal['not']]

MobConditionExprInput = MobConditionExprInputEq | MobConditionExprInputIn | MobConditionExprInputGt | MobConditionExprInputLt | MobConditionExprInputAnd | MobConditionExprInputOr | MobConditionExprInputNot

# Mob RPC helper wire type for MobDependencyModeInput.
MobDependencyModeInput = Literal['all', 'any']

# Mob RPC helper wire type for MobDispatchModeInput.
MobDispatchModeInput = Literal['fan_out', 'one_to_one', 'fan_in']

# Mob RPC helper wire type for MobFlowNodeInput.
class MobFlowNodeInputStep(TypedDict, total=False):
    branch: NotRequired[Optional[str]]
    depends_on: NotRequired[list[str]]
    depends_on_mode: NotRequired[MobDependencyModeInput]
    kind: Required[Literal['step']]
    step_id: Required[str]

class MobFlowNodeInputRepeatUntil(TypedDict, total=False):
    body: Required[MobFrameSpecInput]
    depends_on: NotRequired[list[str]]
    depends_on_mode: NotRequired[MobDependencyModeInput]
    kind: Required[Literal['repeat_until']]
    loop_id: Required[str]
    max_iterations: Required[int]
    until: Required[MobConditionExprInput]

MobFlowNodeInput = MobFlowNodeInputStep | MobFlowNodeInputRepeatUntil

# Mob RPC helper wire type for MobPolicyModeInput.
MobPolicyModeInput = Literal['advisory', 'strict']

# Profile binding input: either an inline profile or a realm profile reference.
#
# Not `Eq`: `Inline(MobProfileInput)` transitively carries float provider
# params (`temperature`, `top_p`) so `Eq` cannot be derived without
# losing fidelity.
MobProfileBindingInput = dict[str, Any] | MobProfileInput

# Mob RPC helper wire type for MobSkillSourceInput.
class MobSkillSourceInputInline(TypedDict, total=False):
    content: Required[str]
    source: Required[Literal['inline']]

class MobSkillSourceInputPath(TypedDict, total=False):
    path: Required[str]
    source: Required[Literal['path']]

MobSkillSourceInput = MobSkillSourceInputInline | MobSkillSourceInputPath

# Mob RPC helper wire type for MobSpawnPolicyInput.
class MobSpawnPolicyInputNone(TypedDict, total=False):
    mode: Required[Literal['none']]

class MobSpawnPolicyInputAuto(TypedDict, total=False):
    mode: Required[Literal['auto']]
    profile_map: Required[dict[str, str]]

MobSpawnPolicyInput = MobSpawnPolicyInputNone | MobSpawnPolicyInputAuto

# Explicit step output format. Omitting `output_format` on a step is
# meaningful — the definition layer resolves a schema-aware default (`json`
# when the step declares `expected_schema_ref`, `text` otherwise) — so the
# wire shape keeps "omitted" representable instead of baking in a default.
MobStepOutputFormatInput = Literal['json', 'text']

# Closed wire stage for a per-identity `mob/reconcile` failure.
WireMobReconcileStage = Literal['spawn', 'retire']

# Opaque host reference: the host's canonical comms `PeerId` string.
WireHostRef = str

# Host bind lifecycle phase as recorded by the controlling machine.
WireHostBindPhase = Literal['requested', 'bound']

# Who attests a remotely-served projection (§7/§20): `HostClaimed` facts
# are only what the owning host reports; `ControllingHostVerified` facts
# were checked against controlling-machine records.
WireProjectionProvenance = Literal['host_claimed', 'controlling_host_verified']

# Observer-computed reachability class (§7.5). A projection from typed
# bridge/pump outcomes — never a membership fact, and a DIFFERENT fact
# from the bridge's own `BridgePeerConnectivity`.
WireReachability = Literal['reachable', 'stale', 'unreachable', 'unknown']

# Merged vocabulary of resources that cannot cross hosts (A4).
#
# No `ScheduleTools` kind (DEC-6): A3-final permits schedule tools on
# remote members, so no producer for that kind can exist — the wire and
# machine vocabularies agree on these six.
WireNonPortableResourceKind = Literal['rust_bundles', 'per_spawn_external_tools', 'mob_default_external_tools', 'default_llm_client_override', 'host_surface_mcp_allowlist', 'workgraph_tools']

# Lifecycle status of a flow run on the wire. Mirrors
# `meerkat_mob::MobRunStatus` so consumers branch on a closed type rather than
# re-deriving meaning from a free-form status string.
WireMobRunStatus = Literal['pending', 'running', 'completed', 'failed', 'canceled']

# Tri-state peer-connectivity projection for `mob/member_status`.
#
# Distinguishes "connectivity is not applicable to this member" (no bridge
# session backs the member) from "the live probe timed out" (the answer is
# transiently unknown) from a resolved connectivity snapshot. The legacy
# `Option<MobPeerConnectivitySnapshot>` projection collapsed both the
# not-applicable and timed-out cases into `None`, laundering a transient
# probe fault into the same shape as a structurally-absent binding.
class WirePeerConnectivityNotApplicable(TypedDict, total=False):
    status: Required[Literal['not_applicable']]

class WirePeerConnectivityProbeTimedOut(TypedDict, total=False):
    status: Required[Literal['probe_timed_out']]

class WirePeerConnectivityKnown(TypedDict, total=False):
    snapshot: Required[WirePeerConnectivitySnapshot]
    status: Required[Literal['known']]

WirePeerConnectivity = WirePeerConnectivityNotApplicable | WirePeerConnectivityProbeTimedOut | WirePeerConnectivityKnown

# Marker discriminant for the host binding descriptor file.
WireHostBindingDescriptorKind = Literal['host']

# Live-channel control verbs. `Truncate` mirrors `LiveTruncateParams`
# minus `channel_id` (carried by the payload).
class BridgeLiveControlVerbCommitInput(TypedDict, total=False):
    verb: Required[Literal['commit_input']]

class BridgeLiveControlVerbInterrupt(TypedDict, total=False):
    verb: Required[Literal['interrupt']]

class BridgeLiveControlVerbTruncate(TypedDict, total=False):
    audio_played_ms: Required[int]
    content_index: Required[int]
    item_id: Required[str]
    verb: Required[Literal['truncate']]

class BridgeLiveControlVerbRefresh(TypedDict, total=False):
    verb: Required[Literal['refresh']]

BridgeLiveControlVerb = BridgeLiveControlVerbCommitInput | BridgeLiveControlVerbInterrupt | BridgeLiveControlVerbTruncate | BridgeLiveControlVerbRefresh

# Per-verb result of `ControlMemberLiveChannel` — mirrors the typed
# statuses the local live command results carry.
class BridgeLiveControlOutcomeCommitInput(TypedDict, total=False):
    status: Required[LiveCommitInputStatus]
    verb: Required[Literal['commit_input']]

class BridgeLiveControlOutcomeInterrupt(TypedDict, total=False):
    status: Required[LiveInterruptStatus]
    verb: Required[Literal['interrupt']]

class BridgeLiveControlOutcomeTruncate(TypedDict, total=False):
    status: Required[LiveTruncateStatus]
    verb: Required[Literal['truncate']]

class BridgeLiveControlOutcomeRefresh(TypedDict, total=False):
    status: Required[LiveRefreshStatus]
    verb: Required[Literal['refresh']]

BridgeLiveControlOutcome = BridgeLiveControlOutcomeCommitInput | BridgeLiveControlOutcomeInterrupt | BridgeLiveControlOutcomeTruncate | BridgeLiveControlOutcomeRefresh

# WorkGraph RPC helper wire type for AttentionDelegatedAuthority.
AttentionDelegatedAuthority = Literal['add_evidence', 'close_own_review_item', 'request_closure', 'close_if_policy_allows']

# WorkGraph RPC helper wire type for GoalAttentionTarget.
class GoalAttentionTargetSession(TypedDict, total=False):
    kind: Required[Literal['session']]
    session_id: Required[str]

class GoalAttentionTargetOwner(TypedDict, total=False):
    kind: Required[Literal['owner']]
    owner_key: Required[WorkOwnerKey]

GoalAttentionTarget = GoalAttentionTargetSession | GoalAttentionTargetOwner

# WorkGraph RPC helper wire type for WorkAttentionMode.
WorkAttentionMode = Literal['pursue', 'coordinate', 'review', 'falsify', 'judge', 'observe']

# WorkGraph RPC helper wire type for WorkAttentionStatus.
class WorkAttentionStatusActive(TypedDict, total=False):
    state: Required[Literal['active']]

class WorkAttentionStatusPaused(TypedDict, total=False):
    state: Required[Literal['paused']]
    until: NotRequired[Optional[str]]

class WorkAttentionStatusSuperseded(TypedDict, total=False):
    state: Required[Literal['superseded']]

class WorkAttentionStatusStopped(TypedDict, total=False):
    state: Required[Literal['stopped']]

WorkAttentionStatus = WorkAttentionStatusActive | WorkAttentionStatusPaused | WorkAttentionStatusSuperseded | WorkAttentionStatusStopped

# WorkGraph RPC helper wire type for WorkAttentionTarget.
class WorkAttentionTargetSession(TypedDict, total=False):
    kind: Required[Literal['session']]
    session_id: Required[str]

class WorkAttentionTargetLoweredOwner(TypedDict, total=False):
    kind: Required[Literal['lowered_owner']]
    owner_key: Required[WorkOwnerKey]

WorkAttentionTarget = WorkAttentionTargetSession | WorkAttentionTargetLoweredOwner

# WorkGraph RPC helper wire type for WorkCompletionPolicy.
class WorkCompletionPolicySelfAttest(TypedDict, total=False):
    kind: Required[Literal['self_attest']]

class WorkCompletionPolicyHostConfirmed(TypedDict, total=False):
    kind: Required[Literal['host_confirmed']]

class WorkCompletionPolicyPrincipalConfirmed(TypedDict, total=False):
    kind: Required[Literal['principal_confirmed']]

class WorkCompletionPolicySupervisor(TypedDict, total=False):
    kind: Required[Literal['supervisor']]
    owner_key: Required[WorkOwnerKey]

class WorkCompletionPolicyReviewerQuorum(TypedDict, total=False):
    kind: Required[Literal['reviewer_quorum']]
    threshold: Required[int]

WorkCompletionPolicy = WorkCompletionPolicySelfAttest | WorkCompletionPolicyHostConfirmed | WorkCompletionPolicyPrincipalConfirmed | WorkCompletionPolicySupervisor | WorkCompletionPolicyReviewerQuorum

# WorkGraph RPC helper wire type for WorkEdgeKind.
WorkEdgeKind = Literal['blocks', 'parent', 'related', 'supersedes', 'derived_from']

# Typed classification of confirmation evidence.
#
# This is the canonical signal the `WorkGraphLifecycleMachine` consumes to
# decide completion-policy satisfaction. The producer
# (`confirmation_evidence_for_policy`) sets this field; the raw
# [`WorkEvidenceRef::kind`] string remains only as opaque provenance/display
# and is never re-read to classify evidence for the satisfaction decision.
WorkEvidenceKind = Literal['host_confirmation', 'principal_confirmation', 'supervisor_confirmation', 'reviewer_confirmation'] | Literal['self_attest']

# WorkGraph RPC helper wire type for WorkGraphEventKind.
WorkGraphEventKind = Literal['created', 'updated', 'claimed', 'released', 'blocked', 'closed', 'linked', 'evidence_added', 'attention_created', 'attention_updated']

# WorkGraph RPC helper wire type for WorkOwnerKind.
WorkOwnerKind = Literal['principal', 'agent', 'session', 'mob', 'label']

# WorkGraph RPC helper wire type for WorkStatus.
WorkStatus = Literal['open', 'in_progress', 'blocked', 'completed', 'cancelled', 'failed']

WorkGraphStatus = Literal["open", "in_progress", "blocked", "completed", "cancelled", "failed"]

WorkGraphPriority = Literal["low", "medium", "high"]

# Shared operation kind for live MCP operations.
McpLiveOperation = Literal['add', 'remove', 'reload']

# Shared status for live MCP operations.
McpLiveOpStatus = Literal['staged', 'applied', 'rejected']

# HTTP transport selection for URL-based servers
McpHttpTransport = Literal['streamable-http', 'sse']

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

# Turning mode for a provider realtime session.
RealtimeTurningMode = Literal['provider_managed', 'explicit_commit']

# Input modality kind.
RealtimeInputKind = Literal['text', 'audio', 'video'] | Literal['image']

# Output modality kind.
RealtimeOutputKind = Literal['text', 'audio', 'video']

# Modality-neutral provider input chunk.
class RealtimeInputChunkTextChunk(TypedDict, total=False):
    text: Required[str]
    kind: Required[Literal['text_chunk']]

class RealtimeInputChunkAudioChunk(TypedDict, total=False):
    channels: Required[int]
    data: Required[str]
    mime_type: Required[str]
    sample_rate_hz: Required[int]
    kind: Required[Literal['audio_chunk']]

class RealtimeInputChunkVideoChunk(TypedDict, total=False):
    data: Required[str]
    mime_type: Required[str]
    kind: Required[Literal['video_chunk']]

class RealtimeInputChunkImageChunk(TypedDict, total=False):
    data: Required[str]
    idempotency_key: Required[str]
    mime_type: Required[str]
    kind: Required[Literal['image_chunk']]

RealtimeInputChunk = RealtimeInputChunkTextChunk | RealtimeInputChunkAudioChunk | RealtimeInputChunkVideoChunk | RealtimeInputChunkImageChunk

# Modality-tagged input chunk for `live/send_input`.
#
# Audio / image / video-frame payloads are base64 strings (`data`); the
# modality-specific metadata (`sample_rate_hz` / `channels` for audio,
# `mime` for image, `codec` / `timestamp_ms` for video frames) lets the
# adapter validate against the negotiated provider format.
#
# T11: `Image` and `VideoFrame` mirror the typed variants on
# [`meerkat_core::live_adapter::LiveInputChunk`]. Adapters that do not
# implement a variant must reject with a typed
# `LiveAdapterErrorCode::ConfigRejected` rather than collapsing onto a
# free-form provider error string.
class LiveInputChunkWireAudio(TypedDict, total=False):
    channels: Required[int]
    data: Required[str]
    kind: Required[Literal['audio']]
    sample_rate_hz: Required[int]

class LiveInputChunkWireText(TypedDict, total=False):
    kind: Required[Literal['text']]
    text: Required[str]

class LiveInputChunkWireImage(TypedDict, total=False):
    data: Required[str]
    idempotency_key: Required[str]
    kind: Required[Literal['image']]
    mime: Required[str]

class LiveInputChunkWireVideoFrame(TypedDict, total=False):
    codec: Required[str]
    data: Required[str]
    kind: Required[Literal['video_frame']]
    timestamp_ms: Required[int]

LiveInputChunkWire = LiveInputChunkWireAudio | LiveInputChunkWireText | LiveInputChunkWireImage | LiveInputChunkWireVideoFrame

# Discriminator for runtime-backed input submission responses.
RuntimeAcceptOutcomeType = Literal['accepted', 'deduplicated', 'rejected']

# Public input lifecycle state projection used by RPC surfaces.
WireInputLifecycleState = Literal['accepted', 'queued', 'staged', 'applied', 'applied_pending_consumption', 'consumed', 'superseded', 'coalesced', 'abandoned']

# Canonical stop reason for transcript messages.
WireStopReason = Literal['end_turn', 'tool_use', 'max_tokens', 'stop_sequence', 'content_filter', 'cancelled']

# Wire-safe tool result content that handles both legacy string and array formats.
WireToolResultContent = str | list[WireContentBlock]

# Wire-safe projection of [`meerkat_core::Provider`].
#
# `WireProvider` pins each provider's canonical wire name with an explicit
# `#[serde(rename)]` (`"openai"`, `"anthropic"`, `"gemini"`, …). The core
# `Provider` enum now agrees (its `OpenAI` variant carries an explicit
# `#[serde(rename = "openai")]` so `rename_all = "snake_case"` can no longer
# mangle it into `"open_a_i"`); `WireProvider` exists as the dedicated wire
# mirror so SDK consumers get a stable, explicitly-named contract plus the
# fail-loud `Unknown` sentinel below, per the wire-mirror dogma used
# throughout this module.
WireProvider = Literal['anthropic', 'openai', 'gemini', 'self_hosted', 'other'] | Literal['unknown']

# Provider continuity metadata for transcript blocks.
class WireProviderMetaAnthropic(TypedDict, total=False):
    provider: Required[Literal['anthropic']]
    signature: Required[str]

class WireProviderMetaAnthropicRedacted(TypedDict, total=False):
    data: Required[str]
    provider: Required[Literal['anthropic_redacted']]

class WireProviderMetaAnthropicCompaction(TypedDict, total=False):
    content: Required[str]
    provider: Required[Literal['anthropic_compaction']]

class WireProviderMetaGemini(TypedDict, total=False):
    provider: Required[Literal['gemini']]
    thoughtSignature: Required[str]

class WireProviderMetaOpenAi(TypedDict, total=False):
    encrypted_content: NotRequired[Optional[str]]
    id: Required[str]
    phase: NotRequired[Optional[str]]
    provider: Required[Literal['open_ai']]
    response_id: NotRequired[Optional[str]]

class WireProviderMetaOpenAiResponse(TypedDict, total=False):
    provider: Required[Literal['open_ai_response']]
    response_id: Required[str]

class WireProviderMetaUnknown(TypedDict, total=False):
    provider: Required[Literal['unknown']]

WireProviderMeta = WireProviderMetaAnthropic | WireProviderMetaAnthropicRedacted | WireProviderMetaAnthropicCompaction | WireProviderMetaGemini | WireProviderMetaOpenAi | WireProviderMetaOpenAiResponse | WireProviderMetaUnknown

# Wire projection of `meerkat_core::TranscriptSource`. Lane provenance
# for spoken-transcript blocks.
#
# R7-4 (P3 dogma): the previous shape used a wildcard arm in
# `From<TranscriptSource>` that fell through to `Spoken`, silently
# misattributing future core variants as user-spoken transcript. The
# fix mirrors the live-wire pattern from R5-3 / R6-5 — future core
# variants surface as `Unknown { debug }` (a fail-loud sentinel), and
# the reverse direction is `TryFrom` returning
# `WireConversionError::TranscriptSource { debug }` for the `Unknown`
# case rather than fabricating a typed `Spoken`.
class WireTranscriptSourceSpoken(TypedDict, total=False):
    kind: Required[Literal['spoken']]

class WireTranscriptSourceUnknown(TypedDict, total=False):
    debug: Required[str]
    kind: Required[Literal['unknown']]

WireTranscriptSource = WireTranscriptSourceSpoken | WireTranscriptSourceUnknown

# Transcript block inside a block-assistant message.
#
# Not `PartialEq`: `ToolUse.args` is `Box<RawValue>` for pass-through
# fidelity (core invariant — opaque from provider to dispatcher), and
# `RawValue` does not derive equality. Equivalence checks should
# round-trip through serialization and compare the serialized bytes.
class WireAssistantBlockTextData(TypedDict, total=False):
    meta: NotRequired[Optional[WireProviderMeta]]
    text: Required[str]

class WireAssistantBlockText(TypedDict, total=False):
    block_type: Required[Literal['text']]
    data: Required[WireAssistantBlockTextData]

class WireAssistantBlockTranscriptData(TypedDict, total=False):
    meta: NotRequired[Optional[WireProviderMeta]]
    source: Required[WireTranscriptSource]
    text: Required[str]

class WireAssistantBlockTranscript(TypedDict, total=False):
    block_type: Required[Literal['transcript']]
    data: Required[WireAssistantBlockTranscriptData]

class WireAssistantBlockReasoningData(TypedDict, total=False):
    meta: NotRequired[Optional[WireProviderMeta]]
    text: NotRequired[str]

class WireAssistantBlockReasoning(TypedDict, total=False):
    block_type: Required[Literal['reasoning']]
    data: Required[WireAssistantBlockReasoningData]

class WireAssistantBlockToolUseData(TypedDict, total=False):
    args: Required[Any]
    id: Required[str]
    meta: NotRequired[Optional[WireProviderMeta]]
    name: Required[str]

class WireAssistantBlockToolUse(TypedDict, total=False):
    block_type: Required[Literal['tool_use']]
    data: Required[WireAssistantBlockToolUseData]

class WireAssistantBlockServerToolContentData(TypedDict, total=False):
    content: Required[Any]
    id: NotRequired[Optional[str]]
    kind: Required[WireServerToolKind]
    meta: NotRequired[Optional[WireProviderMeta]]

class WireAssistantBlockServerToolContent(TypedDict, total=False):
    block_type: Required[Literal['server_tool_content']]
    data: Required[WireAssistantBlockServerToolContentData]

class WireAssistantBlockImageData(TypedDict, total=False):
    blob_ref: Required[BlobRef]
    height: Required[int]
    image_id: Required[AssistantImageId]
    media_type: Required[str]
    meta: Required[ProviderImageMetadata]
    revised_prompt: Required[RevisedPromptDisposition]
    width: Required[int]

class WireAssistantBlockImage(TypedDict, total=False):
    block_type: Required[Literal['image']]
    data: Required[WireAssistantBlockImageData]

class WireAssistantBlockUnknown(TypedDict, total=False):
    block_type: Required[Literal['unknown']]

WireAssistantBlock = WireAssistantBlockText | WireAssistantBlockTranscript | WireAssistantBlockReasoning | WireAssistantBlockToolUse | WireAssistantBlockServerToolContent | WireAssistantBlockImage | WireAssistantBlockUnknown

@dataclass
class TranscriptRewriteReason:
    """Audit annotation carried with a transcript rewrite commit.

The free-form kind is for review, debugging, and provenance only. It never
classifies a rewrite as compaction; [`TranscriptRewriteSelection`] owns that
semantic through its opaque typed compaction range."""
    kind: str
    note: Optional[str] = None


@dataclass
class CompactionRewriteRange:
    """Opaque range carried by the typed compaction rewrite semantic."""
    end: int
    start: int


@dataclass
class TranscriptEditRewriteRange:
    """Opaque current-format range carried by an ordinary transcript edit."""
    end: int
    start: int


# A concrete transcript span selected for same-session rewrite.
class TranscriptRewriteSelectionMessageRange(TypedDict, total=False):
    end: Required[int]
    start: Required[int]
    type: Required[Literal['message_range']]

class TranscriptRewriteSelectionEditMessageRange(TypedDict, total=False):
    range: Required[TranscriptEditRewriteRange]
    type: Required[Literal['edit_message_range']]

class TranscriptRewriteSelectionCompactionMessageRange(TypedDict, total=False):
    range: Required[CompactionRewriteRange]
    type: Required[Literal['compaction_message_range']]

TranscriptRewriteSelection = TranscriptRewriteSelectionMessageRange | TranscriptRewriteSelectionEditMessageRange | TranscriptRewriteSelectionCompactionMessageRange

# Fork transcript replacement helper type for BackgroundJobTerminalStatus.
BackgroundJobTerminalStatus = Literal['completed', 'failed', 'aborted', 'cancelled', 'retired', 'terminated']

# Fork transcript replacement helper type for CommsNoticeKind.
CommsNoticeKind = str

# Canonical realm-local blob identifier.
#
# The identifier is content-addressed, but storage and GC semantics remain
# realm-scoped.
BlobId = str

# Fork transcript replacement helper type for ContentBlock.
class ContentBlockText(TypedDict, total=False):
    text: Required[str]
    type: Required[Literal['text']]

class ContentBlockImageInline(TypedDict, total=False):
    media_type: Required[str]
    type: Required[Literal['image']]
    data: Required[str]
    source: Required[Literal['inline']]

class ContentBlockImageBlob(TypedDict, total=False):
    media_type: Required[str]
    type: Required[Literal['image']]
    blob_id: Required[BlobId]
    source: Required[Literal['blob']]

class ContentBlockVideoInline(TypedDict, total=False):
    duration_ms: Required[int]
    media_type: Required[str]
    type: Required[Literal['video']]
    data: Required[str]
    source: Required[Literal['inline']]

class ContentBlockVideoUri(TypedDict, total=False):
    duration_ms: Required[int]
    media_type: Required[str]
    type: Required[Literal['video']]
    source: Required[Literal['uri']]
    uri: Required[str]

class ContentBlockStructured(TypedDict, total=False):
    data: Required[Any]
    type: Required[Literal['structured']]

class ContentBlockSkillContext(TypedDict, total=False):
    skill_key: Required[SkillKey]
    text: Required[str]
    type: Required[Literal['skill_context']]

ContentBlock = ContentBlockText | ContentBlockImageInline | ContentBlockImageBlob | ContentBlockVideoInline | ContentBlockVideoUri | ContentBlockStructured | ContentBlockSkillContext

# Sender-declared content-taint classification for peer content.
#
# This is the typed vocabulary for the optional taint declaration a sender
# stamps onto content-bearing comms envelopes (`Message` / `Request` /
# `Response`). `Clean` and `Tainted` are the two DECLARED states.
#
# `None` at the carriers (`MessageKind::*.content_taint`,
# `SystemNoticeBlock::Comms.sender_taint`, runtime `PeerInput.sender_taint`)
# means "the sender made no declaration" — a REAL third state. Receivers
# must never coalesce `None` into `Clean`: an absent declaration carries no
# trust information, while `Clean` is an affirmative sender claim.
SenderContentTaint = Literal['clean', 'tainted']

# Direction for runtime-authored communication metadata.
SystemNoticeDirection = Literal['incoming', 'outgoing', 'internal']

# Canonical runtime identity for a peer.
#
# `PeerId` is the routing key: the router and trust store key by `PeerId`,
# never by `PeerName`. Two peers may legitimately share a display `PeerName`
# (per the Wave-B V5 dogma note), but their `PeerId`s never collide — the
# underlying UUID is globally unique.
#
# Constructed freshly (`PeerId::new`) for a peer minted locally, parsed
# from a hyphenated UUID (`PeerId::parse`) when we've been given an identity
# over the wire, or derived from a 32-byte Ed25519 public key when a transport
# still authenticates by raw signing key.
PeerId = str

# Typed runtime-authored transcript metadata.
#
# These blocks are the durable contract for comms, tool/MCP state, auth,
# background work, and other runtime facts. They are never user-authored text.
class SystemNoticeBlockComms(TypedDict, total=False):
    content: NotRequired[list[ContentBlock]]
    direction: Required[SystemNoticeDirection]
    intent: NotRequired[Optional[str]]
    kind: Required[CommsNoticeKind]
    payload: NotRequired[Any]
    peer: NotRequired[Optional[SystemNoticePeer]]
    request_id: NotRequired[Optional[str]]
    sender_taint: NotRequired[Optional[SenderContentTaint]]
    status: NotRequired[Optional[str]]
    summary: NotRequired[Optional[str]]
    type: Required[Literal['comms']]

class SystemNoticeBlockExternalEvent(TypedDict, total=False):
    body: NotRequired[Optional[str]]
    content: NotRequired[list[ContentBlock]]
    event_type: Required[str]
    payload: NotRequired[Any]
    source: Required[str]
    summary: NotRequired[Optional[str]]
    type: Required[Literal['external_event']]

class SystemNoticeBlockToolConfig(TypedDict, total=False):
    payload: Required[ToolConfigChangedPayload]
    type: Required[Literal['tool_config']]

class SystemNoticeBlockMcp(TypedDict, total=False):
    detail: NotRequired[Optional[str]]
    operation: NotRequired[Optional[ToolConfigChangeOperation]]
    pending_sources: NotRequired[list[str]]
    persisted: NotRequired[bool]
    phase: NotRequired[Optional[ExternalToolDeltaPhase]]
    server_id: NotRequired[Optional[str]]
    type: Required[Literal['mcp']]

class SystemNoticeBlockBackgroundJob(TypedDict, total=False):
    detail: NotRequired[Optional[str]]
    display_name: NotRequired[Optional[str]]
    job_id: Required[str]
    status: Required[BackgroundJobTerminalStatus]
    type: Required[Literal['background_job']]

class SystemNoticeBlockAuth(TypedDict, total=False):
    binding: NotRequired[Optional[str]]
    detail: NotRequired[Optional[str]]
    state: Required[str]
    type: Required[Literal['auth']]

class SystemNoticeBlockRuntimeNotice(TypedDict, total=False):
    category: Required[str]
    detail: NotRequired[Optional[str]]
    payload: NotRequired[Any]
    type: Required[Literal['runtime_notice']]

class SystemNoticeBlockUnknown(TypedDict, total=False):
    payload: NotRequired[Any]
    summary: NotRequired[Optional[str]]
    type: Required[Literal['unknown']]

SystemNoticeBlock = SystemNoticeBlockComms | SystemNoticeBlockExternalEvent | SystemNoticeBlockToolConfig | SystemNoticeBlockMcp | SystemNoticeBlockBackgroundJob | SystemNoticeBlockAuth | SystemNoticeBlockRuntimeNotice | SystemNoticeBlockUnknown

# Typed system notice content carried in the transcript.
SystemNoticeKind = Literal['generic', 'comms', 'external_event', 'mcp_pending', 'mcp', 'background_job', 'tool_scope', 'tool_scope_warning'] | Literal['auth_reauth_required']

# Typed transcript role for a user-channel message.
#
# This is the canonical replacement for `[Context compacted]` string-prefix
# folklore in the transcript-continuity save-guard. A user message produced as
# a runtime compaction boundary carries [`TranscriptUserRole::CompactionSummary`];
# everything else stays [`TranscriptUserRole::Conversational`]. The producer of
# the compaction summary sets this typed marker; the save-guard reads the typed
# field instead of classifying the rendered message body by content.
TranscriptUserRole = Literal['conversational', 'compaction_summary', 'injected_context']

# Fork transcript replacement helper type for AssistantImageId.
AssistantImageId = str

@dataclass
class BlobRef:
    """Durable image reference owned by transcript/runtime state."""
    blob_id: BlobId
    media_type: str


@dataclass
class GeminiImageMetadata:
    """Fork transcript replacement helper type for GeminiImageMetadata."""
    target_model: str
    continuity_ref: Optional[str] = None
    response_id: Optional[str] = None


@dataclass
class OpenAiImageMetadata:
    """Fork transcript replacement helper type for OpenAiImageMetadata."""
    target_model: str
    image_generation_call_id: Optional[str] = None
    response_id: Optional[str] = None


# Fork transcript replacement helper type for ProviderImageMetadata.
class ProviderImageMetadataNotEmitted(TypedDict, total=False):
    provider: Required[Literal['not_emitted']]

class ProviderImageMetadataOpenai(TypedDict, total=False):
    image_generation_call_id: NotRequired[Optional[str]]
    response_id: NotRequired[Optional[str]]
    target_model: Required[str]
    provider: Required[Literal['openai']]

class ProviderImageMetadataGemini(TypedDict, total=False):
    continuity_ref: NotRequired[Optional[str]]
    response_id: NotRequired[Optional[str]]
    target_model: Required[str]
    provider: Required[Literal['gemini']]

ProviderImageMetadata = ProviderImageMetadataNotEmitted | ProviderImageMetadataOpenai | ProviderImageMetadataGemini

@dataclass
class PromptText:
    """Fork transcript replacement helper type for PromptText."""
    content: str


# Fork transcript replacement helper type for RevisedPromptSource.
RevisedPromptSource = Literal['provider', 'meerkat_projection']

# Fork transcript replacement helper type for RevisedPromptDisposition.
class RevisedPromptDispositionNotRequested(TypedDict, total=False):
    disposition: Required[Literal['not_requested']]

class RevisedPromptDispositionUnsupportedByBackend(TypedDict, total=False):
    disposition: Required[Literal['unsupported_by_backend']]

class RevisedPromptDispositionUnchanged(TypedDict, total=False):
    disposition: Required[Literal['unchanged']]

class RevisedPromptDispositionRevised(TypedDict, total=False):
    disposition: Required[Literal['revised']]
    source: Required[RevisedPromptSource]
    text: Required[PromptText]

RevisedPromptDisposition = RevisedPromptDispositionNotRequested | RevisedPromptDispositionUnsupportedByBackend | RevisedPromptDispositionUnchanged | RevisedPromptDispositionRevised

# Wire projection of `meerkat_core::ServerToolKind`.
#
# Mirrors the typed semantic owner of a provider-executed tool. The core enum
# is `#[non_exhaustive]`; a future semantic kind that lacks an explicit arm in
# the forward `From` surfaces as `Unknown { debug }` rather than silently
# collapsing into a plausible-but-wrong kind. SDK consumers route on the
# `kind` discriminator and treat `unknown` as unrecognized.
#
# **When a new core variant is added, add an explicit arm in the forward
# `From` impl above the wildcard — `Unknown` is the floor, not the
# destination.**
class WireServerToolKindWebSearch(TypedDict, total=False):
    kind: Required[Literal['web_search']]

class WireServerToolKindGoogleSearch(TypedDict, total=False):
    kind: Required[Literal['google_search']]

class WireServerToolKindProviderNative(TypedDict, total=False):
    kind: Required[Literal['provider_native']]
    name: Required[str]

class WireServerToolKindUnknown(TypedDict, total=False):
    debug: Required[str]
    kind: Required[Literal['unknown']]

WireServerToolKind = WireServerToolKindWebSearch | WireServerToolKindGoogleSearch | WireServerToolKindProviderNative | WireServerToolKindUnknown

# Public transcript message accepted by same-session rewrite APIs.
class TranscriptRewriteMessageSystem(TypedDict, total=False):
    content: Required[str]
    created_at: NotRequired[Optional[str]]
    role: Required[Literal['system']]

class TranscriptRewriteMessageSystemNotice(TypedDict, total=False):
    blocks: NotRequired[list[SystemNoticeBlock]]
    body: NotRequired[Optional[str]]
    created_at: NotRequired[Optional[str]]
    kind: Required[SystemNoticeKind]
    role: Required[Literal['system_notice']]

class TranscriptRewriteMessageUser(TypedDict, total=False):
    content: Required[WireContentInput]
    created_at: NotRequired[Optional[str]]
    role: Required[Literal['user']]
    transcript_role: NotRequired[TranscriptUserRole]

class TranscriptRewriteMessageBlockAssistant(TypedDict, total=False):
    blocks: Required[list[WireAssistantBlock]]
    created_at: NotRequired[Optional[str]]
    role: Required[Literal['block_assistant']]
    stop_reason: NotRequired[WireStopReason]

class TranscriptRewriteMessageToolResults(TypedDict, total=False):
    created_at: NotRequired[Optional[str]]
    results: Required[list[WireToolResult]]
    role: Required[Literal['tool_results']]

TranscriptRewriteMessage = TranscriptRewriteMessageSystem | TranscriptRewriteMessageSystemNotice | TranscriptRewriteMessageUser | TranscriptRewriteMessageBlockAssistant | TranscriptRewriteMessageToolResults

# Typed wire mirror of [`meerkat_core::TranscriptReplacement`].
#
# R-#87 (P3 dogma): `ForkSessionReplaceParams.replacement` previously carried
# the core `TranscriptReplacement` directly with a
# `#[schemars(with = "serde_json::Value")]` override, so the emitted JSON
# schema collapsed to a bare `Value` (artifacts/schemas/params.json) and the
# SDK codegen saw an untyped bag. The core enum cannot derive `JsonSchema`
# directly: it embeds `Message` and `AssistantBlock`, neither of which has a
# `JsonSchema` impl in `meerkat-core` (and `Message::BlockAssistant` carries
# `Box<RawValue>` tool-call args).
#
# This wire mirror is schema-emittable because it is built entirely from
# types that already carry `JsonSchema` derives in this module:
# [`TranscriptRewriteMessage`] (the public message shape, with its own
# `into_core`), [`WireContentBlock`], and [`WireAssistantBlock`]. Conversion
# into the core enum is fallible — every inner conversion already returns a
# typed [`WireConversionError`](crate::wire::error::WireConversionError), so
# malformed payloads surface a typed parse fault at the boundary rather than
# being smuggled through as an opaque `Value`.
#
# The serde tag/rename mirror the core enum (`#[serde(tag = "type",
# rename_all = "snake_case")]`) so the wire bytes are unchanged: the only
# observable delta is that the schema is now the closed enum.
class WireTranscriptReplacementMessage(TypedDict, total=False):
    message: Required[TranscriptRewriteMessage]
    type: Required[Literal['message']]

class WireTranscriptReplacementUserContentBlock(TypedDict, total=False):
    block: Required[WireContentBlock]
    block_index: Required[int]
    type: Required[Literal['user_content_block']]

class WireTranscriptReplacementAssistantBlock(TypedDict, total=False):
    block: Required[WireAssistantBlock]
    block_index: Required[int]
    type: Required[Literal['assistant_block']]

class WireTranscriptReplacementToolResultContentBlock(TypedDict, total=False):
    block: Required[WireContentBlock]
    block_index: Required[int]
    result_index: Required[int]
    type: Required[Literal['tool_result_content_block']]

WireTranscriptReplacement = WireTranscriptReplacementMessage | WireTranscriptReplacementUserContentBlock | WireTranscriptReplacementAssistantBlock | WireTranscriptReplacementToolResultContentBlock

# Machine-owned image operation phase.
class WireImageOperationPhaseRequested(TypedDict, total=False):
    phase: Required[Literal['requested']]

class WireImageOperationPhaseValidating(TypedDict, total=False):
    phase: Required[Literal['validating']]

class WireImageOperationPhaseAwaitingApproval(TypedDict, total=False):
    approval_id: Required[str]
    phase: Required[Literal['awaiting_approval']]

class WireImageOperationPhasePlanResolved(TypedDict, total=False):
    phase: Required[Literal['plan_resolved']]

class WireImageOperationPhaseProjectionSnapshotted(TypedDict, total=False):
    phase: Required[Literal['projection_snapshotted']]

class WireImageOperationPhaseScopedOverrideActive(TypedDict, total=False):
    phase: Required[Literal['scoped_override_active']]

class WireImageOperationPhaseProviderCallInFlight(TypedDict, total=False):
    phase: Required[Literal['provider_call_in_flight']]

class WireImageOperationPhaseProviderResultCaptured(TypedDict, total=False):
    phase: Required[Literal['provider_result_captured']]

class WireImageOperationPhaseBlobCommitPending(TypedDict, total=False):
    phase: Required[Literal['blob_commit_pending']]

class WireImageOperationPhaseResultCommitted(TypedDict, total=False):
    phase: Required[Literal['result_committed']]

class WireImageOperationPhaseRestoringScopedOverride(TypedDict, total=False):
    phase: Required[Literal['restoring_scoped_override']]

class WireImageOperationPhaseTerminal(TypedDict, total=False):
    phase: Required[Literal['terminal']]
    terminal: Required[dict[str, Any]]

WireImageOperationPhase = WireImageOperationPhaseRequested | WireImageOperationPhaseValidating | WireImageOperationPhaseAwaitingApproval | WireImageOperationPhasePlanResolved | WireImageOperationPhaseProjectionSnapshotted | WireImageOperationPhaseScopedOverrideActive | WireImageOperationPhaseProviderCallInFlight | WireImageOperationPhaseProviderResultCaptured | WireImageOperationPhaseBlobCommitPending | WireImageOperationPhaseResultCommitted | WireImageOperationPhaseRestoringScopedOverride | WireImageOperationPhaseTerminal

# Model recommendation tier.
WireModelTier = Literal['recommended', 'supported']

# Request command carried inside public `comms/send` surfaces.
class CommsCommandInput(TypedDict, total=False):
    allow_self_session: NotRequired[Optional[bool]]
    blocks: NotRequired[Optional[list[ContentBlock]]]
    body: Required[str]
    handling_mode: NotRequired[Optional[HandlingMode]]
    kind: Required[Literal['input']]
    source: NotRequired[Optional[Literal['tcp', 'uds', 'stdin', 'webhook', 'rpc']]]
    stream: NotRequired[Optional[Literal['none', 'reserve_interaction']]]

class CommsCommandPeerMessage(TypedDict, total=False):
    blocks: NotRequired[Optional[list[ContentBlock]]]
    body: Required[str]
    content_taint: NotRequired[Optional[SendTaintOverride]]
    handling_mode: NotRequired[Optional[HandlingMode]]
    kind: Required[Literal['peer_message']]
    to: Required[PeerId]

class CommsCommandPeerLifecycle(TypedDict, total=False):
    kind: Required[Literal['peer_lifecycle']]
    lifecycle_kind: Required[Literal['mob.peer_added', 'mob.peer_retired', 'mob.peer_unwired'] | Literal['mob.dismiss']]
    params: Required[CommsPeerLifecycleParams]
    to: Required[PeerId]

class CommsCommandPeerRequest(TypedDict, total=False):
    blocks: NotRequired[Optional[list[ContentBlock]]]
    content_taint: NotRequired[Optional[SendTaintOverride]]
    handling_mode: NotRequired[Optional[HandlingMode]]
    intent: Required[CommsPeerRequestIntent]
    kind: Required[Literal['peer_request']]
    params: Required[CommsPeerRequestParams]
    stream: NotRequired[Optional[Literal['none', 'reserve_interaction']]]
    to: Required[PeerId]

class CommsCommandPeerResponse(TypedDict, total=False):
    blocks: NotRequired[Optional[list[ContentBlock]]]
    content_taint: NotRequired[Optional[SendTaintOverride]]
    handling_mode: NotRequired[Optional[HandlingMode]]
    in_reply_to: Required[str]
    kind: Required[Literal['peer_response']]
    result: NotRequired[Optional[CommsPeerResponseResult]]
    status: Required[Literal['accepted', 'completed', 'failed']]
    to: Required[PeerId]

CommsCommandRequest = CommsCommandInput | CommsCommandPeerMessage | CommsCommandPeerLifecycle | CommsCommandPeerRequest | CommsCommandPeerResponse

# One-time bootstrap proof exchanged between a mob supervisor and a
# member runtime on initial bind.
#
# Transparent over the wire (`#[serde(transparent)]` — a bare JSON string),
# but carries a redacting `Debug` impl and has no `Display` impl so the
# raw secret cannot accidentally land in logs or panic messages. Treat it
# like an API key: read `as_str()` only at the comms/transport boundary.
BridgeBootstrapToken = str

# A typed command sent from a supervisor to a member runtime, a mob host
# daemon (host-addressed family), or — for exactly one family
# (`MemberOperatorRequest`) — from a member host to the controlling host.
#
# The full `PartialEq + Eq` chain is load-bearing: the comms envelope
# enums (`CommsPeerRequestParams` et al.) derive `Eq` over this type.
# Opaque JSON in payloads therefore rides [`WireOpaqueJson`], never a
# bare `serde_json::Value`.
class BridgeCommandBindMember(TypedDict, total=False):
    bootstrap_token: Required[BridgeBootstrapToken]
    command: Required[Literal['bind_member']]
    epoch: Required[int]
    expected_address: Required[str]
    expected_peer_id: Required[str]
    protocol_version: Required[BridgeProtocolVersion]
    supervisor: Required[BridgePeerSpec]

class BridgeCommandAuthorizeSupervisor(TypedDict, total=False):
    command: Required[Literal['authorize_supervisor']]
    epoch: Required[int]
    protocol_version: Required[BridgeProtocolVersion]
    supervisor: Required[BridgePeerSpec]

class BridgeCommandRevokeSupervisor(TypedDict, total=False):
    command: Required[Literal['revoke_supervisor']]
    epoch: Required[int]
    protocol_version: Required[BridgeProtocolVersion]
    supervisor: Required[BridgePeerSpec]

class BridgeCommandDeliverMemberInput(TypedDict, total=False):
    command: Required[Literal['deliver_member_input']]
    content: Required[ContentInput]
    epoch: Required[int]
    expected_member: NotRequired[Optional[BridgeMemberIncarnation]]
    handling_mode: Required[HandlingMode]
    injected_context: NotRequired[list[ContentInput]]
    input_id: Required[str]
    objective_id: NotRequired[Optional[str]]
    outcome_tracking: NotRequired[Optional[Literal['interaction']]]
    protocol_version: Required[BridgeProtocolVersion]
    supervisor: Required[BridgePeerSpec]
    transcript_interaction_id: NotRequired[Optional[str]]
    turn: NotRequired[Optional[BridgeTurnDirective]]

class BridgeCommandObserveMember(TypedDict, total=False):
    command: Required[Literal['observe_member']]
    epoch: Required[int]
    protocol_version: Required[BridgeProtocolVersion]
    supervisor: Required[BridgePeerSpec]

class BridgeCommandInterruptMember(TypedDict, total=False):
    command: Required[Literal['interrupt_member']]
    epoch: Required[int]
    expected_member: NotRequired[Optional[BridgeMemberIncarnation]]
    protocol_version: Required[BridgeProtocolVersion]
    supervisor: Required[BridgePeerSpec]

class BridgeCommandHardCancelMember(TypedDict, total=False):
    command: Required[Literal['hard_cancel_member']]
    epoch: Required[int]
    expected_member: Required[BridgeMemberIncarnation]
    expected_run_id: Required[RunId]
    operation_id: Required[OperationId]
    protocol_version: Required[BridgeProtocolVersion]
    reason: Required[str]
    supervisor: Required[BridgePeerSpec]

class BridgeCommandCancelTrackedMemberInput(TypedDict, total=False):
    command: Required[Literal['cancel_tracked_member_input']]
    epoch: Required[int]
    expected_member: Required[BridgeMemberIncarnation]
    input_id: Required[str]
    protocol_version: Required[BridgeProtocolVersion]
    supervisor: Required[BridgePeerSpec]

class BridgeCommandRetireMember(TypedDict, total=False):
    command: Required[Literal['retire_member']]
    epoch: Required[int]
    protocol_version: Required[BridgeProtocolVersion]
    supervisor: Required[BridgePeerSpec]

class BridgeCommandDestroyMember(TypedDict, total=False):
    command: Required[Literal['destroy_member']]
    epoch: Required[int]
    protocol_version: Required[BridgeProtocolVersion]
    supervisor: Required[BridgePeerSpec]

class BridgeCommandWireMember(TypedDict, total=False):
    command: Required[Literal['wire_member']]
    epoch: Required[int]
    mob_peer_overlay: NotRequired[Optional[BridgeMobPeerOverlayHandoff]]
    peer_spec: Required[BridgePeerSpec]
    protocol_version: Required[BridgeProtocolVersion]
    supervisor: Required[BridgePeerSpec]

class BridgeCommandUnwireMember(TypedDict, total=False):
    command: Required[Literal['unwire_member']]
    epoch: Required[int]
    mob_peer_overlay: NotRequired[Optional[BridgeMobPeerOverlayHandoff]]
    peer_spec: Required[BridgePeerSpec]
    protocol_version: Required[BridgeProtocolVersion]
    supervisor: Required[BridgePeerSpec]

class BridgeCommandDeclareMemberOutboundTaint(TypedDict, total=False):
    command: Required[Literal['declare_member_outbound_taint']]
    epoch: Required[int]
    protocol_version: Required[BridgeProtocolVersion]
    supervisor: Required[BridgePeerSpec]
    taint: NotRequired[Optional[SenderContentTaint]]
    target: NotRequired[Optional[BridgeOutboundTaintTarget]]

class BridgeCommandReadMemberHistory(TypedDict, total=False):
    command: Required[Literal['read_member_history']]
    epoch: Required[int]
    expected_member: Required[BridgeMemberIncarnation]
    from_index: NotRequired[Optional[int]]
    limit: NotRequired[Optional[int]]
    protocol_version: Required[BridgeProtocolVersion]
    supervisor: Required[BridgePeerSpec]

class BridgeCommandPollMemberEvents(TypedDict, total=False):
    command: Required[Literal['poll_member_events']]
    cursor: Required[BridgeEventCursor]
    epoch: Required[int]
    expected_member: Required[BridgeMemberIncarnation]
    max: NotRequired[Optional[int]]
    max_outcomes: NotRequired[Optional[int]]
    outcome_acks: NotRequired[list[BridgeTurnOutcomeAck]]
    protocol_version: Required[BridgeProtocolVersion]
    supervisor: Required[BridgePeerSpec]
    wait_ms: NotRequired[Optional[int]]

class BridgeCommandOpenMemberLiveChannel(TypedDict, total=False):
    command: Required[Literal['open_member_live_channel']]
    epoch: Required[int]
    expected_member: Required[BridgeMemberIncarnation]
    protocol_version: Required[BridgeProtocolVersion]
    supervisor: Required[BridgePeerSpec]
    transport: NotRequired[Optional[LiveOpenTransport]]
    turning_mode: NotRequired[Optional[RealtimeTurningMode]]

class BridgeCommandCloseMemberLiveChannel(TypedDict, total=False):
    channel_id: Required[str]
    command: Required[Literal['close_member_live_channel']]
    epoch: Required[int]
    expected_member: Required[BridgeMemberIncarnation]
    protocol_version: Required[BridgeProtocolVersion]
    supervisor: Required[BridgePeerSpec]

class BridgeCommandMemberLiveChannelStatus(TypedDict, total=False):
    channel_id: NotRequired[Optional[str]]
    command: Required[Literal['member_live_channel_status']]
    epoch: Required[int]
    expected_member: Required[BridgeMemberIncarnation]
    protocol_version: Required[BridgeProtocolVersion]
    supervisor: Required[BridgePeerSpec]

class BridgeCommandControlMemberLiveChannel(TypedDict, total=False):
    channel_id: Required[str]
    command: Required[Literal['control_member_live_channel']]
    epoch: Required[int]
    expected_member: Required[BridgeMemberIncarnation]
    protocol_version: Required[BridgeProtocolVersion]
    supervisor: Required[BridgePeerSpec]
    verb: Required[BridgeLiveControlVerb]

class BridgeCommandBindHost(TypedDict, total=False):
    binding_generation: Required[int]
    bootstrap_proof: Required[str]
    command: Required[Literal['bind_host']]
    epoch: Required[int]
    expected_address: Required[str]
    expected_host_peer_id: Required[str]
    mob_id: Required[str]
    protocol_version: Required[BridgeProtocolVersion]
    required_capabilities: Required[BridgeHostCapabilityRequirements]
    supervisor: Required[BridgePeerSpec]

class BridgeCommandRebindHost(TypedDict, total=False):
    binding_generation: Required[int]
    command: Required[Literal['rebind_host']]
    epoch: Required[int]
    mob_id: Required[str]
    protocol_version: Required[BridgeProtocolVersion]
    required_capabilities: Required[BridgeHostCapabilityRequirements]
    supervisor: Required[BridgePeerSpec]

class BridgeCommandRevokeHost(TypedDict, total=False):
    binding_generation: Required[int]
    command: Required[Literal['revoke_host']]
    epoch: Required[int]
    mob_id: Required[str]
    protocol_version: Required[BridgeProtocolVersion]
    supervisor: Required[BridgePeerSpec]

class BridgeCommandMaterializeMember(TypedDict, total=False):
    binding_generation: Required[int]
    command: Required[Literal['materialize_member']]
    epoch: Required[int]
    fence_token: Required[int]
    generation: Required[int]
    launch: Required[dict[str, Literal['fresh']] | dict[str, Any]]
    protocol_version: Required[BridgeProtocolVersion]
    spec: Required[dict[str, Any]]
    spec_digest: Required[str]
    supervisor: Required[BridgePeerSpec]

class BridgeCommandReleaseMember(TypedDict, total=False):
    agent_identity: Required[str]
    binding_generation: Required[int]
    command: Required[Literal['release_member']]
    epoch: Required[int]
    fence_token: Required[int]
    generation: Required[int]
    mob_id: Required[str]
    protocol_version: Required[BridgeProtocolVersion]
    supervisor: Required[BridgePeerSpec]

class BridgeCommandInstallPeerTrust(TypedDict, total=False):
    agent_identity: Required[str]
    binding_generation: Required[int]
    command: Required[Literal['install_peer_trust']]
    epoch: Required[int]
    mob_id: Required[str]
    peer: Required[BridgePeerSpec]
    protocol_version: Required[BridgeProtocolVersion]
    supervisor: Required[BridgePeerSpec]

class BridgeCommandRemovePeerTrust(TypedDict, total=False):
    agent_identity: Required[str]
    binding_generation: Required[int]
    command: Required[Literal['remove_peer_trust']]
    epoch: Required[int]
    mob_id: Required[str]
    peer: Required[BridgePeerSpec]
    protocol_version: Required[BridgeProtocolVersion]
    supervisor: Required[BridgePeerSpec]

class BridgeCommandHostStatus(TypedDict, total=False):
    binding_generation: Required[int]
    command: Required[Literal['host_status']]
    epoch: Required[int]
    mob_id: Required[str]
    protocol_version: Required[BridgeProtocolVersion]
    supervisor: Required[BridgePeerSpec]

class BridgeCommandMemberOperatorRequest(TypedDict, total=False):
    agent_identity: Required[str]
    command: Required[Literal['member_operator_request']]
    op: Required[dict[str, Any] | dict[str, Literal['list_members']] | dict[str, Literal['mob_list_flows']]]
    protocol_version: Required[BridgeProtocolVersion]
    request_id: Required[str]
    requester_fence_token: Required[int]
    requester_generation: Required[int]
    requester_host_binding_generation: Required[int]
    requester_host_id: Required[str]
    requester_member_session_id: Required[str]

class BridgeCommandObserveSupervisorRotation(TypedDict, total=False):
    command: Required[Literal['observe_supervisor_rotation']]
    observer: Required[BridgePeerSpec]
    observer_epoch: Required[int]
    operation_id: Required[str]
    protocol_version: Required[BridgeProtocolVersion]

BridgeCommand = BridgeCommandBindMember | BridgeCommandAuthorizeSupervisor | BridgeCommandRevokeSupervisor | BridgeCommandDeliverMemberInput | BridgeCommandObserveMember | BridgeCommandInterruptMember | BridgeCommandHardCancelMember | BridgeCommandCancelTrackedMemberInput | BridgeCommandRetireMember | BridgeCommandDestroyMember | BridgeCommandWireMember | BridgeCommandUnwireMember | BridgeCommandDeclareMemberOutboundTaint | BridgeCommandReadMemberHistory | BridgeCommandPollMemberEvents | BridgeCommandOpenMemberLiveChannel | BridgeCommandCloseMemberLiveChannel | BridgeCommandMemberLiveChannelStatus | BridgeCommandControlMemberLiveChannel | BridgeCommandBindHost | BridgeCommandRebindHost | BridgeCommandRevokeHost | BridgeCommandMaterializeMember | BridgeCommandReleaseMember | BridgeCommandInstallPeerTrust | BridgeCommandRemovePeerTrust | BridgeCommandHostStatus | BridgeCommandMemberOperatorRequest | BridgeCommandObserveSupervisorRotation

# Outcome of a delivery attempt.
class BridgeDeliveryOutcomeAccepted(TypedDict, total=False):
    outcome: Required[Literal['accepted']]

class BridgeDeliveryOutcomeDeduplicated(TypedDict, total=False):
    existing_input_id: Required[str]
    outcome: Required[Literal['deduplicated']]

class BridgeDeliveryOutcomeRejected(TypedDict, total=False):
    cause: Required[BridgeDeliveryRejectionCause]
    outcome: Required[Literal['rejected']]
    reason: Required[str]

BridgeDeliveryOutcome = BridgeDeliveryOutcomeAccepted | BridgeDeliveryOutcomeDeduplicated | BridgeDeliveryOutcomeRejected

# Typed vocabulary for why member input delivery was rejected.
#
# This mirrors the runtime accept-boundary rejection vocabulary at the
# bridge wire boundary. Callers should branch on this typed cause and treat
# the sibling `reason` string on [`BridgeDeliveryOutcome::Rejected`] as
# operator-facing presentation only.
class BridgeDeliveryRejectionCauseNotReady(TypedDict, total=False):
    kind: Required[Literal['not_ready']]
    state: Required[BridgeMemberRuntimeState]

class BridgeDeliveryRejectionCauseDurabilityViolation(TypedDict, total=False):
    detail: Required[str]
    kind: Required[Literal['durability_violation']]

class BridgeDeliveryRejectionCausePeerHandlingModeInvalid(TypedDict, total=False):
    detail: Required[str]
    kind: Required[Literal['peer_handling_mode_invalid']]

class BridgeDeliveryRejectionCauseInternal(TypedDict, total=False):
    detail: Required[str]
    kind: Required[Literal['internal']]

class BridgeDeliveryRejectionCauseTurnDirectiveUnsupported(TypedDict, total=False):
    detail: Required[str]
    kind: Required[Literal['turn_directive_unsupported']]

class BridgeDeliveryRejectionCauseOutcomeJournalFull(TypedDict, total=False):
    kind: Required[Literal['outcome_journal_full']]
    limit: Required[int]
    retained: Required[int]

class BridgeDeliveryRejectionCauseStaleMemberIncarnation(TypedDict, total=False):
    current: Required[BridgeMemberIncarnation]
    kind: Required[Literal['stale_member_incarnation']]

class BridgeDeliveryRejectionCauseStaleMemberResidency(TypedDict, total=False):
    current: NotRequired[Optional[BridgeMemberIncarnation]]
    expected: Required[BridgeMemberIncarnation]
    kind: Required[Literal['stale_member_residency']]

BridgeDeliveryRejectionCause = BridgeDeliveryRejectionCauseNotReady | BridgeDeliveryRejectionCauseDurabilityViolation | BridgeDeliveryRejectionCausePeerHandlingModeInvalid | BridgeDeliveryRejectionCauseInternal | BridgeDeliveryRejectionCauseTurnDirectiveUnsupported | BridgeDeliveryRejectionCauseOutcomeJournalFull | BridgeDeliveryRejectionCauseStaleMemberIncarnation | BridgeDeliveryRejectionCauseStaleMemberResidency

# Wire projection of a member's runtime state.
BridgeMemberRuntimeState = Literal['initializing', 'idle', 'attached', 'running', 'retired', 'stopped', 'destroyed']

# Connectivity class observed for the bridged member runtime.
BridgePeerConnectivity = Literal['reachable', 'unreachable', 'unknown']

# A supported supervisor bridge wire protocol version.
#
# The JSON representation remains the historic integer so persisted records
# and wire payloads do not change shape. Construction is intentionally routed
# through this type so unsupported values fail at serde/TryFrom boundaries
# instead of being carried deeper as raw integers.
BridgeProtocolVersion = int

# Typed vocabulary for why a bridge command was rejected.
#
# Callers branch on the typed `cause` to drive recovery logic; the
# accompanying `reason` string is for operator diagnostics only and must
# not be pattern-matched. Reserve `Internal` for true invariant
# violations — ordinary validation failures get a specific cause.
#
# Not `Copy` since V4: several causes carry typed detail payloads.
class BridgeRejectionCauseStaleCursorPayload(TypedDict, total=False):
    generation: Required[int]
    watermark: Required[int]

class BridgeRejectionCauseStaleCursor(TypedDict, total=False):
    stale_cursor: Required[BridgeRejectionCauseStaleCursorPayload]

class BridgeRejectionCauseOversizedEventPayload(TypedDict, total=False):
    durable_seq: Required[int]
    encoded_bytes: Required[int]
    generation: Required[int]
    max_bytes: Required[int]
    next_seq: Required[int]

class BridgeRejectionCauseOversizedEvent(TypedDict, total=False):
    oversized_event: Required[BridgeRejectionCauseOversizedEventPayload]

class BridgeRejectionCauseHistoryRowTooLargePayload(TypedDict, total=False):
    encoded_bytes: Required[int]
    index: Required[int]
    max_bytes: Required[int]

class BridgeRejectionCauseHistoryRowTooLarge(TypedDict, total=False):
    history_row_too_large: Required[BridgeRejectionCauseHistoryRowTooLargePayload]

class BridgeRejectionCauseScopeDeniedPayload(TypedDict, total=False):
    presented: Required[list[WireControlScope]]
    required: Required[WireControlScope]

class BridgeRejectionCauseScopeDenied(TypedDict, total=False):
    scope_denied: Required[BridgeRejectionCauseScopeDeniedPayload]

class BridgeRejectionCauseMaterializeBuildRejectedPayload(TypedDict, total=False):
    cause: Required[MemberBuildRejection]

class BridgeRejectionCauseMaterializeBuildRejected(TypedDict, total=False):
    materialize_build_rejected: Required[BridgeRejectionCauseMaterializeBuildRejectedPayload]

class BridgeRejectionCauseModelUnresolvablePayload(TypedDict, total=False):
    model: Required[str]

class BridgeRejectionCauseModelUnresolvable(TypedDict, total=False):
    model_unresolvable: Required[BridgeRejectionCauseModelUnresolvablePayload]

class BridgeRejectionCauseAuthBindingUnresolvablePayload(TypedDict, total=False):
    binding: Required[str]
    realm: Required[str]

class BridgeRejectionCauseAuthBindingUnresolvable(TypedDict, total=False):
    auth_binding_unresolvable: Required[BridgeRejectionCauseAuthBindingUnresolvablePayload]

class BridgeRejectionCauseMcpCommandMissingPayload(TypedDict, total=False):
    server: Required[str]

class BridgeRejectionCauseMcpCommandMissing(TypedDict, total=False):
    mcp_command_missing: Required[BridgeRejectionCauseMcpCommandMissingPayload]

class BridgeRejectionCauseEnvKeyMissingPayload(TypedDict, total=False):
    key: Required[str]

class BridgeRejectionCauseEnvKeyMissing(TypedDict, total=False):
    env_key_missing: Required[BridgeRejectionCauseEnvKeyMissingPayload]

class BridgeRejectionCauseHostEngineVersionChangedPayload(TypedDict, total=False):
    bound: Required[str]
    reported: Required[str]

class BridgeRejectionCauseHostEngineVersionChanged(TypedDict, total=False):
    host_engine_version_changed: Required[BridgeRejectionCauseHostEngineVersionChangedPayload]

class BridgeRejectionCauseModelNotRealtimePayload(TypedDict, total=False):
    model: Required[str]
    provider: Required[str]

class BridgeRejectionCauseModelNotRealtime(TypedDict, total=False):
    model_not_realtime: Required[BridgeRejectionCauseModelNotRealtimePayload]

class BridgeRejectionCauseLiveAdapterUnavailablePayload(TypedDict, total=False):
    provider: Required[str]

class BridgeRejectionCauseLiveAdapterUnavailable(TypedDict, total=False):
    live_adapter_unavailable: Required[BridgeRejectionCauseLiveAdapterUnavailablePayload]

class BridgeRejectionCauseLiveTransportUnsupportedPayload(TypedDict, total=False):
    requested: Required[str]

class BridgeRejectionCauseLiveTransportUnsupported(TypedDict, total=False):
    live_transport_unsupported: Required[BridgeRejectionCauseLiveTransportUnsupportedPayload]

class BridgeRejectionCauseCapabilityMissingPayload(TypedDict, total=False):
    capability: Required[str]

class BridgeRejectionCauseCapabilityMissing(TypedDict, total=False):
    capability_missing: Required[BridgeRejectionCauseCapabilityMissingPayload]

class BridgeRejectionCauseSessionOwnershipConflictPayload(TypedDict, total=False):
    session_id: Required[str]

class BridgeRejectionCauseSessionOwnershipConflict(TypedDict, total=False):
    session_ownership_conflict: Required[BridgeRejectionCauseSessionOwnershipConflictPayload]

BridgeRejectionCause = Literal['not_bound'] | Literal['stale_supervisor'] | Literal['sender_mismatch'] | Literal['already_bound'] | Literal['invalid_bootstrap_token'] | Literal['unsupported_protocol_version'] | Literal['invalid_supervisor_spec'] | Literal['invalid_peer_spec'] | Literal['address_mismatch'] | Literal['unsupported'] | Literal['internal'] | Literal['stale_fence'] | BridgeRejectionCauseStaleCursor | BridgeRejectionCauseOversizedEvent | BridgeRejectionCauseHistoryRowTooLarge | Literal['unavailable'] | BridgeRejectionCauseScopeDenied | Literal['spec_digest_mismatch'] | BridgeRejectionCauseMaterializeBuildRejected | BridgeRejectionCauseModelUnresolvable | BridgeRejectionCauseAuthBindingUnresolvable | BridgeRejectionCauseMcpCommandMissing | Literal['realm_backend_unavailable'] | BridgeRejectionCauseEnvKeyMissing | BridgeRejectionCauseHostEngineVersionChanged | BridgeRejectionCauseModelNotRealtime | BridgeRejectionCauseLiveAdapterUnavailable | Literal['live_transport_unavailable'] | Literal['live_channel_already_bound'] | Literal['live_channel_not_found'] | BridgeRejectionCauseLiveTransportUnsupported | Literal['resume_session_not_found'] | BridgeRejectionCauseCapabilityMissing | Literal['launch_mode_unsupported'] | Literal['launch_mode_placement_mismatch'] | BridgeRejectionCauseSessionOwnershipConflict

# A typed reply from a member runtime (or mob host daemon) back to the
# supervisor, and — for `MemberOperatorReply` — from the controlling host
# back to a member's host.
#
# Not `PartialEq`/`Eq`: `MemberHistoryPage` carries `WireSessionMessage`
# rows and `MemberEventsPage` carries `AgentEvent` envelopes; both ride
# serialized-form equality adapters (`WireHistoryRow` / `WireEventRow`) so
# the reply chain keeps the `Eq` the comms envelope enums derive over it.
class BridgeReplyBindMember(TypedDict, total=False):
    address: Required[str]
    capabilities: Required[BridgeCapabilities]
    peer_id: Required[str]
    result: Required[Literal['bind_member']]

class BridgeReplyAck(TypedDict, total=False):
    ok: Required[bool]
    result: Required[Literal['ack']]

class BridgeReplyObservation(TypedDict, total=False):
    accepting_inputs: NotRequired[Optional[bool]]
    current_run_id: NotRequired[Optional[str]]
    last_error: NotRequired[Optional[str]]
    observed_at: Required[str]
    peer_connectivity: NotRequired[Optional[BridgePeerConnectivity]]
    result: Required[Literal['observation']]
    state: Required[BridgeMemberRuntimeState]

class BridgeReplyDelivery(TypedDict, total=False):
    canonical_input_id: NotRequired[Optional[str]]
    input_id: Required[str]
    outcome: Required[BridgeDeliveryOutcome]
    result: Required[Literal['delivery']]

class BridgeReplyTrackedInputCancelled(TypedDict, total=False):
    expected_member: Required[BridgeMemberIncarnation]
    input_id: Required[str]
    outcome: Required[BridgeTrackedInputCancelOutcome]
    result: Required[Literal['tracked_input_cancelled']]

class BridgeReplyRetire(TypedDict, total=False):
    inputs_abandoned: Required[int]
    inputs_pending_drain: Required[int]
    result: Required[Literal['retire']]

class BridgeReplyDestroy(TypedDict, total=False):
    inputs_abandoned: Required[int]
    result: Required[Literal['destroy']]

class BridgeReplySupervisorRotationFound(TypedDict, total=False):
    result: Required[Literal['supervisor_rotation']]
    outcome: Required[Literal['found']]
    state: Required[dict[str, Any]]

class BridgeReplySupervisorRotationNotFound(TypedDict, total=False):
    result: Required[Literal['supervisor_rotation']]
    operation_id: Required[str]
    outcome: Required[Literal['not_found']]

class BridgeReplyRejected(TypedDict, total=False):
    cause: Required[BridgeRejectionCause]
    reason: Required[str]
    result: Required[Literal['rejected']]

class BridgeReplyBindHost(TypedDict, total=False):
    address: Required[str]
    binding_generation: Required[int]
    capabilities: Required[BridgeCapabilities]
    host_peer_id: Required[str]
    live_endpoint: NotRequired[Optional[str]]
    result: Required[Literal['bind_host']]

class BridgeReplyHostRebound(TypedDict, total=False):
    binding_generation: Required[int]
    capabilities: Required[BridgeCapabilities]
    host_peer_id: Required[str]
    live_endpoint: NotRequired[Optional[str]]
    result: Required[Literal['host_rebound']]

class BridgeReplyHostRevoked(TypedDict, total=False):
    binding_generation: Required[int]
    epoch: Required[int]
    host_peer_id: Required[str]
    mob_id: Required[str]
    released_members: Required[list[str]]
    result: Required[Literal['host_revoked']]

class BridgeReplyMemberHistoryPage(TypedDict, total=False):
    generation: Required[int]
    page: Required[WireMemberHistoryPageBody]
    result: Required[Literal['member_history_page']]

class BridgeReplyMemberEventsPage(TypedDict, total=False):
    events: Required[list[dict[str, Any]]]
    fence_token: Required[int]
    from_seq: Required[int]
    generation: Required[int]
    next_seq: Required[int]
    outcomes_complete: Required[bool]
    result: Required[Literal['member_events_page']]
    runtime_incarnation: Required[str]
    turn_outcomes: NotRequired[list[BridgeTurnOutcomeRecord]]
    watermark: Required[int]

class BridgeReplyMemberMaterialized(TypedDict, total=False):
    advertised_address: Required[str]
    engine_version: Required[str]
    launch_outcome: Required[Literal['fresh', 'resumed_live', 'resumed_from_snapshot']]
    member_peer_id: Required[str]
    member_pubkey: Required[str]
    resolved_auth_binding: NotRequired[Optional[WireAuthBindingRef]]
    result: Required[Literal['member_materialized']]
    session_id: Required[str]
    spec_digest: Required[str]

class BridgeReplyMemberReleased(TypedDict, total=False):
    disposal: Required[dict[str, Any]]
    result: Required[Literal['member_released']]

class BridgeReplyHostStatus(TypedDict, total=False):
    capabilities: Required[BridgeCapabilities]
    members: Required[list[dict[str, Any]]]
    result: Required[Literal['host_status']]
    runtime_incarnation: Required[str]

class BridgeReplyMemberLiveChannelOpened(TypedDict, total=False):
    open: Required[LiveOpenResult]
    result: Required[Literal['member_live_channel_opened']]

class BridgeReplyMemberLiveChannelClosed(TypedDict, total=False):
    result: Required[Literal['member_live_channel_closed']]
    status: Required[LiveCloseStatus]

class BridgeReplyMemberLiveChannelStatusReport(TypedDict, total=False):
    channel_id: Required[str]
    result: Required[Literal['member_live_channel_status_report']]
    status: Required[WireLiveAdapterStatus]

class BridgeReplyMemberLiveChannelControlled(TypedDict, total=False):
    outcome: Required[BridgeLiveControlOutcome]
    result: Required[Literal['member_live_channel_controlled']]

class BridgeReplyMemberOperatorReply(TypedDict, total=False):
    outcome: Required[dict[str, Any]]
    request_id: Required[str]
    result: Required[Literal['member_operator_reply']]

BridgeReply = BridgeReplyBindMember | BridgeReplyAck | BridgeReplyObservation | BridgeReplyDelivery | BridgeReplyTrackedInputCancelled | BridgeReplyRetire | BridgeReplyDestroy | BridgeReplySupervisorRotationFound | BridgeReplySupervisorRotationNotFound | BridgeReplyRejected | BridgeReplyBindHost | BridgeReplyHostRebound | BridgeReplyHostRevoked | BridgeReplyMemberHistoryPage | BridgeReplyMemberEventsPage | BridgeReplyMemberMaterialized | BridgeReplyMemberReleased | BridgeReplyHostStatus | BridgeReplyMemberLiveChannelOpened | BridgeReplyMemberLiveChannelClosed | BridgeReplyMemberLiveChannelStatusReport | BridgeReplyMemberLiveChannelControlled | BridgeReplyMemberOperatorReply

# Input content that can be either a plain text string or multimodal content blocks.
#
# Deserializes from either `"text"` or `[{type: "text", text: "..."}, ...]`.
# Provides `From<String>` and `From<&str>` for ergonomic construction.
ContentInput = str | list[ContentBlock]

# Request payload for `comms/send`.
class CommsSendParamsInput(TypedDict, total=False):
    allow_self_session: NotRequired[Optional[bool]]
    blocks: NotRequired[Optional[list[ContentBlock]]]
    body: Required[str]
    handling_mode: NotRequired[Optional[HandlingMode]]
    kind: Required[Literal['input']]
    session_id: Required[str]
    source: NotRequired[Optional[Literal['tcp', 'uds', 'stdin', 'webhook', 'rpc']]]
    stream: NotRequired[Optional[Literal['none', 'reserve_interaction']]]

class CommsSendParamsPeerMessage(TypedDict, total=False):
    blocks: NotRequired[Optional[list[ContentBlock]]]
    body: Required[str]
    content_taint: NotRequired[Optional[SendTaintOverride]]
    handling_mode: NotRequired[Optional[HandlingMode]]
    kind: Required[Literal['peer_message']]
    session_id: Required[str]
    to: Required[PeerId]

class CommsSendParamsPeerLifecycle(TypedDict, total=False):
    kind: Required[Literal['peer_lifecycle']]
    lifecycle_kind: Required[Literal['mob.peer_added', 'mob.peer_retired', 'mob.peer_unwired'] | Literal['mob.dismiss']]
    params: Required[CommsPeerLifecycleParams]
    session_id: Required[str]
    to: Required[PeerId]

class CommsSendParamsPeerRequest(TypedDict, total=False):
    blocks: NotRequired[Optional[list[ContentBlock]]]
    content_taint: NotRequired[Optional[SendTaintOverride]]
    handling_mode: NotRequired[Optional[HandlingMode]]
    intent: Required[CommsPeerRequestIntent]
    kind: Required[Literal['peer_request']]
    params: Required[CommsPeerRequestParams]
    session_id: Required[str]
    stream: NotRequired[Optional[Literal['none', 'reserve_interaction']]]
    to: Required[PeerId]

class CommsSendParamsPeerResponse(TypedDict, total=False):
    blocks: NotRequired[Optional[list[ContentBlock]]]
    content_taint: NotRequired[Optional[SendTaintOverride]]
    handling_mode: NotRequired[Optional[HandlingMode]]
    in_reply_to: Required[str]
    kind: Required[Literal['peer_response']]
    result: NotRequired[Optional[CommsPeerResponseResult]]
    session_id: Required[str]
    status: Required[Literal['accepted', 'completed', 'failed']]
    to: Required[PeerId]

CommsSendParams = CommsSendParamsInput | CommsSendParamsPeerMessage | CommsSendParamsPeerLifecycle | CommsSendParamsPeerRequest | CommsSendParamsPeerResponse

# Response payload for `comms/send`.
class CommsSendResultInputAccepted(TypedDict, total=False):
    interaction_id: Required[str]
    kind: Required[Literal['input_accepted']]
    stream_reserved: Required[bool]

class CommsSendResultPeerMessageSent(TypedDict, total=False):
    delivery: Required[Literal['acked', 'handed_off', 'queued']]
    envelope_id: Required[str]
    kind: Required[Literal['peer_message_sent']]

class CommsSendResultPeerLifecycleSent(TypedDict, total=False):
    envelope_id: Required[str]
    kind: Required[Literal['peer_lifecycle_sent']]

class CommsSendResultPeerRequestSent(TypedDict, total=False):
    envelope_id: Required[str]
    interaction_id: Required[str]
    kind: Required[Literal['peer_request_sent']]
    request_id: Required[str]
    stream_reserved: Required[bool]

class CommsSendResultPeerResponseSent(TypedDict, total=False):
    envelope_id: Required[str]
    in_reply_to: Required[str]
    kind: Required[Literal['peer_response_sent']]

CommsSendResult = CommsSendResultInputAccepted | CommsSendResultPeerMessageSent | CommsSendResultPeerLifecycleSent | CommsSendResultPeerRequestSent | CommsSendResultPeerResponseSent

# Closed discriminator carried in [`CommsChecksumTokenResult`].
CommsChecksumTokenResultIntent = Literal['checksum_token']

# Closed request-intent vocabulary for [`CommsCommandRequest::PeerRequest`].
#
# This is the canonical, core-owned set of intents a public `peer_request`
# command may carry. Unknown strings fail at the serde deserialization
# boundary and cannot fall through to a local match or string default — the
# closed set is enforced structurally, not by a runtime string comparison.
#
# The domain envelope [`CommsCommand::PeerRequest`] intentionally keeps a wider
# open intent space (it also carries mob topology intents such as
# `mob.peer_added`); this enum is the narrow vocabulary admitted at the public
# request surface. Surfaces that accept the public comms contract re-import
# this type so they share the same fail-closed guarantee.
CommsPeerRequestIntent = Literal['supervisor.bridge', 'checksum_token']

# Typed params for public `peer_request`.
CommsPeerRequestParams = BridgeCommand | CommsChecksumTokenParams

# Typed result payload for public `peer_response`.
#
# Compatibility JSON is intentionally not accepted here. Callers that include
# a `result` field must provide a typed bridge reply shape.
CommsPeerResponseResult = BridgeReply | CommsChecksumTokenResult

# Handling mode for ordinary content-bearing work.
#
# `Queue` means outer-loop / next-turn handling and leaves the current run
# untouched.
# `Steer` means inner-loop handling and requests injection at the earliest
# admissible cooperative boundary, while remaining ordinary work rather than
# an out-of-band control-plane command.
HandlingMode = Literal['queue', 'steer']

# Display-only slug for a peer.
#
# `PeerName` is **not** a routing key after Wave-B V5: the router resolves
# sends by [`PeerId`], and trust stores are keyed by [`PeerId`]. `PeerName`
# is retained so human-facing surfaces (CLI, REST `comms.peers`, logs) can
# render a recognisable handle next to the opaque id.
PeerName = str

# Typed transport atom for a peer address.
#
# Replaces the old free-form `address: String` on `PeerDirectoryEntry` so
# callers cannot accidentally invent new transports by string concatenation
# at a call site.
PeerTransport = Literal['inproc', 'uds', 'tcp']

# Comms/session-stream RPC contract for PeerDirectorySource.
PeerDirectorySource = Literal['trusted', 'inproc', 'trusted_and_inproc', 'unknown']

# Comms/session-stream RPC contract for PeerSendability.
PeerSendability = Literal['peer_message', 'peer_request', 'peer_response']

# Per-send tri-state override for the outbound content-taint declaration.
#
# Carried as `Option<SendTaintOverride>` on the comms send surfaces: an
# ABSENT override (`None`) inherits the runtime-level declaration installed
# via `set_outbound_content_taint`; `Undeclared` strips the declaration for
# this send (the envelope carries no taint field); `Declare(taint)` stamps
# exactly `taint`. Inherit, disable, and set are three different facts — a
# two-state override would collapse them.
SendTaintOverride = dict[str, SenderContentTaint] | Literal['undeclared']

# Canonical transcript message for public wire surfaces.
#
# Not `PartialEq`: the `BlockAssistant.blocks` variant carries
# `WireAssistantBlock`s which hold opaque tool-call args as
# `Box<RawValue>`. See `WireAssistantBlock` doc.
class WireSessionMessageSystem(TypedDict, total=False):
    content: Required[str]
    created_at: Required[str]
    role: Required[Literal['system']]

class WireSessionMessageSystemNotice(TypedDict, total=False):
    blocks: NotRequired[list[SystemNoticeBlock]]
    body: NotRequired[Optional[str]]
    created_at: Required[str]
    kind: Required[SystemNoticeKind]
    role: Required[Literal['system_notice']]

class WireSessionMessageUser(TypedDict, total=False):
    content: Required[WireContentInput]
    created_at: Required[str]
    interaction_id: NotRequired[Optional[str]]
    role: Required[Literal['user']]
    run_id: NotRequired[Optional[RunId]]
    transcript_role: NotRequired[TranscriptUserRole]

class WireSessionMessageBlockAssistant(TypedDict, total=False):
    blocks: Required[list[WireAssistantBlock]]
    created_at: Required[str]
    interaction_id: NotRequired[Optional[str]]
    role: Required[Literal['block_assistant']]
    run_id: NotRequired[Optional[RunId]]
    stop_reason: Required[WireStopReason]

class WireSessionMessageToolResults(TypedDict, total=False):
    created_at: Required[str]
    results: Required[list[WireToolResult]]
    role: Required[Literal['tool_results']]

WireSessionMessage = WireSessionMessageSystem | WireSessionMessageSystemNotice | WireSessionMessageUser | WireSessionMessageBlockAssistant | WireSessionMessageToolResults

# Equality adapter over a canonical wire transcript row.
#
# `WireSessionMessage` deliberately derives no `PartialEq` (opaque
# tool-call args ride `RawValue`), but the bridge reply chain that
# carries history pages must be `Eq` (the comms envelope enums derive
# it). Equality here is semantic-JSON equality of the serialized wire
# form — exactly the fact reply comparison needs. Transparent: the wire
# shape stays the raw row object.
WireHistoryRow = WireSessionMessage


def parse_work_completion_policy(value: Any) -> "WorkCompletionPolicy":
    """Fail-closed wire parser for WorkCompletionPolicy (K21)."""
    data = _expect_wire_object(value, 'WorkCompletionPolicy')
    tag = _expect_wire_str(_require_wire_field(data, 'kind', 'WorkCompletionPolicy'), 'WorkCompletionPolicy.kind')
    if tag == 'self_attest':
        parsed_self_attest: dict[str, Any] = {'kind': 'self_attest'}
        return parsed_self_attest
    if tag == 'host_confirmed':
        parsed_host_confirmed: dict[str, Any] = {'kind': 'host_confirmed'}
        return parsed_host_confirmed
    if tag == 'principal_confirmed':
        parsed_principal_confirmed: dict[str, Any] = {'kind': 'principal_confirmed'}
        return parsed_principal_confirmed
    if tag == 'supervisor':
        parsed_supervisor: dict[str, Any] = {'kind': 'supervisor'}
        parsed_supervisor['owner_key'] = WorkOwnerKey.from_wire(_require_wire_field(data, 'owner_key', 'WorkCompletionPolicy'))
        return parsed_supervisor
    if tag == 'reviewer_quorum':
        parsed_reviewer_quorum: dict[str, Any] = {'kind': 'reviewer_quorum'}
        parsed_reviewer_quorum['threshold'] = _expect_wire_int(_require_wire_field(data, 'threshold', 'WorkCompletionPolicy'), 'WorkCompletionPolicy.reviewer_quorum.threshold')
        return parsed_reviewer_quorum
    raise _wire_parse_error('WorkCompletionPolicy', f"unknown `kind` value `{tag}`")


def parse_work_edge_kind(value: Any) -> "WorkEdgeKind":
    """Fail-closed wire parser for WorkEdgeKind (K21)."""
    return _expect_wire_enum(value, ('blocks', 'parent', 'related', 'supersedes', 'derived_from',), 'WorkEdgeKind')


def parse_work_graph_event_kind(value: Any) -> "WorkGraphEventKind":
    """Fail-closed wire parser for WorkGraphEventKind (K21)."""
    return _expect_wire_enum(value, ('created', 'updated', 'claimed', 'released', 'blocked', 'closed', 'linked', 'evidence_added', 'attention_created', 'attention_updated',), 'WorkGraphEventKind')


def parse_work_evidence_kind(value: Any) -> "WorkEvidenceKind":
    """Fail-closed wire parser for WorkEvidenceKind (K21)."""
    return _expect_wire_enum(value, ('host_confirmation', 'principal_confirmation', 'supervisor_confirmation', 'reviewer_confirmation', 'self_attest',), 'WorkEvidenceKind')


def parse_attention_delegated_authority(value: Any) -> "AttentionDelegatedAuthority":
    """Fail-closed wire parser for AttentionDelegatedAuthority (K21)."""
    return _expect_wire_enum(value, ('add_evidence', 'close_own_review_item', 'request_closure', 'close_if_policy_allows',), 'AttentionDelegatedAuthority')


def parse_work_attention_mode(value: Any) -> "WorkAttentionMode":
    """Fail-closed wire parser for WorkAttentionMode (K21)."""
    return _expect_wire_enum(value, ('pursue', 'coordinate', 'review', 'falsify', 'judge', 'observe',), 'WorkAttentionMode')


def parse_work_attention_status(value: Any) -> "WorkAttentionStatus":
    """Fail-closed wire parser for WorkAttentionStatus (K21)."""
    data = _expect_wire_object(value, 'WorkAttentionStatus')
    tag = _expect_wire_str(_require_wire_field(data, 'state', 'WorkAttentionStatus'), 'WorkAttentionStatus.state')
    if tag == 'active':
        parsed_active: dict[str, Any] = {'state': 'active'}
        return parsed_active
    if tag == 'paused':
        parsed_paused: dict[str, Any] = {'state': 'paused'}
        if data.get('until') is not None:
            parsed_paused['until'] = _expect_wire_str(data['until'], 'WorkAttentionStatus.paused.until')
        return parsed_paused
    if tag == 'superseded':
        parsed_superseded: dict[str, Any] = {'state': 'superseded'}
        return parsed_superseded
    if tag == 'stopped':
        parsed_stopped: dict[str, Any] = {'state': 'stopped'}
        return parsed_stopped
    raise _wire_parse_error('WorkAttentionStatus', f"unknown `state` value `{tag}`")


def parse_work_attention_target(value: Any) -> "WorkAttentionTarget":
    """Fail-closed wire parser for WorkAttentionTarget (K21)."""
    data = _expect_wire_object(value, 'WorkAttentionTarget')
    tag = _expect_wire_str(_require_wire_field(data, 'kind', 'WorkAttentionTarget'), 'WorkAttentionTarget.kind')
    if tag == 'session':
        parsed_session: dict[str, Any] = {'kind': 'session'}
        parsed_session['session_id'] = _expect_wire_str(_require_wire_field(data, 'session_id', 'WorkAttentionTarget'), 'WorkAttentionTarget.session.session_id')
        return parsed_session
    if tag == 'lowered_owner':
        parsed_lowered_owner: dict[str, Any] = {'kind': 'lowered_owner'}
        parsed_lowered_owner['owner_key'] = WorkOwnerKey.from_wire(_require_wire_field(data, 'owner_key', 'WorkAttentionTarget'))
        return parsed_lowered_owner
    raise _wire_parse_error('WorkAttentionTarget', f"unknown `kind` value `{tag}`")


def parse_work_owner_kind(value: Any) -> "WorkOwnerKind":
    """Fail-closed wire parser for WorkOwnerKind (K21)."""
    return _expect_wire_enum(value, ('principal', 'agent', 'session', 'mob', 'label',), 'WorkOwnerKind')
