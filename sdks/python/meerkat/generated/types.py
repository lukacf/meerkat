from __future__ import annotations

"""Generated wire types for Meerkat SDK.

Contract version: 0.6.13
"""

from dataclasses import dataclass, field
from typing import Any, Literal, NotRequired, Optional, Required, TypedDict


CONTRACT_VERSION = "0.6.13"


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
class WireProviderMeta:
    """Provider continuity metadata."""
    provider: str = ''


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
    created_at: str = ''
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
    skill_refs: list[dict[str, str]] = field(default_factory=list)


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
    status: str


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
    budget_split_policy: Optional[WireBudgetSplitPolicy] = None
    context: Optional[Any] = None
    inherited_tool_filter: Optional[WireToolFilter] = None
    initial_message: Optional[WireContentInput] = None
    labels: Optional[dict[str, str]] = None
    launch_mode: Optional[WireMemberLaunchMode] = None
    override_profile: Optional[WireMobProfile] = None
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
    state: WireMemberState
    status: WireMobMemberStatus
    error: Optional[str] = None
    labels: Optional[dict[str, str]] = None
    wired_to: Optional[list[str]] = None


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


@dataclass
class WireMobProfile:
    """Profile override for `mob/spawn`."""
    model: str
    backend: Optional[WireMobBackendKind] = None
    external_addressable: Optional[bool] = None
    max_inline_peer_notifications: Optional[int] = None
    output_schema: Optional[Any] = None
    peer_description: Optional[str] = None
    provider_params: Optional[Any] = None
    runtime_mode: Optional[WireMobRuntimeMode] = None
    skills: Optional[list[str]] = None
    tools: Optional[WireMobToolConfig] = None


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
    status: str
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
    delivery: dict[str, Any]
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
    status: str


@dataclass
class MobFlowsResult:
    """Response payload for `mob/flows`."""
    flows: list[str]
    mob_id: str


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
    """Response payload for `mob/flow_status`."""
    run: Any


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
    backend: Optional[WireMobBackendKind] = None
    role_name: Optional[str] = None
    runtime_mode: Optional[WireMobRuntimeMode] = None


@dataclass
class MobForkHelperParams:
    """Request payload for `mob/fork_helper`."""
    mob_id: str
    prompt: str
    source_member_id: str
    agent_identity: Optional[str] = None
    backend: Optional[WireMobBackendKind] = None
    fork_context: Optional[Any] = None
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
    """Request payload for `mob/turn_start`."""
    agent_identity: str
    mob_id: str
    prompt: WireContentInput
    additional_instructions: Optional[list[str]] = None
    auth_binding: Optional[WireAuthBindingRef] = None
    clear_auth_binding: Optional[bool] = None
    clear_provider_params: Optional[bool] = None
    flow_tool_overlay: Optional[dict[str, Any]] = None
    keep_alive: Optional[bool] = None
    max_tokens: Optional[int] = None
    model: Optional[str] = None
    output_schema: Optional[Any] = None
    provider: Optional[str] = None
    provider_params: Optional[Any] = None
    skill_refs: Optional[list[dict[str, Any]]] = None
    structured_output_retries: Optional[int] = None
    system_prompt: Optional[str] = None


@dataclass
class MobMemberStatusResult:
    """Response payload for `mob/member_status`."""
    is_final: bool
    status: WireMobMemberStatus
    tokens_used: int
    current_session_id: Optional[str] = None
    error: Optional[str] = None
    external_member: Optional[Any] = None
    kickoff: Optional[Any] = None
    output_preview: Optional[str] = None
    peer_connectivity: Optional[Any] = None
    resolved_capabilities: Optional[WireResolvedModelCapabilities] = None


@dataclass
class MobSnapshotResult:
    """Response payload for `mob/snapshot`."""
    members: list[Any]
    mob_id: str
    status: str


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
    origin: Optional[Literal['external', 'internal']] = None
    work_ref: Optional[str] = None


@dataclass
class MobSubmitWorkResult:
    """Response payload for `mob/submit_work`."""
    member_ref: WireMemberRef
    mob_id: str
    work_ref: str


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
    profile: Optional[Any] = None
    revision: Optional[int] = None
    updated_at: Optional[str] = None


@dataclass
class MobProfileListResult:
    """Response payload for `mob/profile/list`."""
    profiles: list[dict[str, Any]]


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
    limits: Optional[MobLimitsSpecInput] = None
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
    backend: Optional[WireMobBackendKind] = None
    external_addressable: Optional[bool] = None
    max_inline_peer_notifications: Optional[int] = None
    output_schema: Optional[Any] = None
    peer_description: Optional[str] = None
    provider_params: Optional[Any] = None
    runtime_mode: Optional[WireMobRuntimeMode] = None
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
    error: str
    stage: WireMobReconcileStage


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
    current_protocol_version: Optional[BridgeProtocolVersion] = None
    default_protocol_version: Optional[BridgeProtocolVersion] = None
    deliver_member_input: Optional[bool] = None
    destroy_member: Optional[bool] = None
    hard_cancel_member: Optional[bool] = None
    interrupt_member: Optional[bool] = None
    observe_member: Optional[bool] = None
    retire_member: Optional[bool] = None
    supported_protocol_versions: Optional[list[BridgeProtocolVersion]] = None
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


@dataclass
class BridgeDeliveryResponse:
    """Full response to a delivery command."""
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
command."""
    epoch: int
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
    """Peer wiring command payload."""
    epoch: int
    peer_spec: BridgePeerSpec
    protocol_version: BridgeProtocolVersion
    supervisor: BridgePeerSpec


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
    reachability: PeerReachability
    sendable_kinds: list[PeerSendability]
    source: PeerDirectorySource
    last_unreachable_reason: Optional[PeerReachabilityReason] = None


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
    transport: Optional[Literal['websocket', 'webrtc']] = None
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
    http_url: NotRequired[str]
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
follow-up); G8 (P2): `transport` typed-mirror."""
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
# R4-5 (P3): the refresh path is asynchronous — `LiveAdapterHost::send_command`
# returns when the command has been queued on the adapter's mpsc channel,
# not when the adapter pump has applied the resulting `session.update`. The
# realtime stream is the source of truth for the actual refresh outcome
# (failures surface as `LiveAdapterObservation::Error`).
#
# Today every refresh path is `Queued`. The enum is `#[non_exhaustive]` so
# a future revision can add `AppliedSync` (e.g. when a oneshot ack from the
# adapter pump back through the command channel lands, or when a refresh
# is detected as a no-op against the currently-applied snapshot) without
# breaking the wire shape. SDK consumers route on the string value and
# treat unknown values as "outcome unknown — observe the realtime stream".
#
# Serializes as a plain string (no envelope) so [`LiveRefreshResult`] can
# place this typed status alongside the back-compat `refresh_enqueued`
# boolean as ordinary sibling fields, which keeps SDK codegen on the
# simple-struct path.
LiveRefreshStatus = Literal['queued']

@dataclass
class LiveRefreshResult:
    """Response payload for `live/refresh`.

R4-5 (P3): replaces the previous untyped `{"refresh_enqueued": true}`
JSON blob. The boolean `refresh_enqueued` field is preserved for back-
compat (legacy clients that pattern-match on it stay on the green path)
alongside the typed `status` discriminator. New code should route on
`status`.

See [`LiveRefreshStatus`] for the variant set and the contract on
asynchronous adapter-pump application."""
    refresh_enqueued: bool
    status: Literal['queued']


# A typed, identity-bearing realtime transcript event consumed by the session.
class RealtimeTranscriptEventItemObserved(TypedDict, total=False):
    item_id: Required[str]
    previous_item_id: NotRequired[str]
    response_id: NotRequired[str]
    role: Required[RealtimeTranscriptRole]
    type: Required[Literal['item_observed']]

class RealtimeTranscriptEventItemSkipped(TypedDict, total=False):
    item_id: Required[str]
    previous_item_id: NotRequired[str]
    type: Required[Literal['item_skipped']]

class RealtimeTranscriptEventUserTranscriptFinal(TypedDict, total=False):
    content_index: Required[int]
    item_id: Required[str]
    previous_item_id: NotRequired[str]
    text: Required[str]
    type: Required[Literal['user_transcript_final']]

class RealtimeTranscriptEventAssistantTextDelta(TypedDict, total=False):
    content_index: Required[int]
    delta: Required[str]
    delta_id: Required[str]
    item_id: Required[str]
    previous_item_id: NotRequired[str]
    response_id: Required[str]
    type: Required[Literal['assistant_text_delta']]

class RealtimeTranscriptEventAssistantTranscriptDelta(TypedDict, total=False):
    content_index: Required[int]
    delta: Required[str]
    delta_id: Required[str]
    item_id: Required[str]
    previous_item_id: NotRequired[str]
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
    stop_reason: Required[Any]
    type: Required[Literal['assistant_turn_completed']]
    usage: Required[Any]

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

# Wire mirror of
# [`meerkat_core::live_adapter::LiveConfigRejectionReason`]. R5-2 (P2
# dogma): pins the typed semantic-routing variants on the wire so SDKs
# can distinguish a runtime-side identity swap from an adapter-side
# input-modality rejection without string parsing.
#
# R6-5 (P3 dogma): closes the last typed-route-as-detail-string gap. The
# previous wildcard fallback collapsed future core variants into
# `Other { detail: "unknown_live_config_rejection_reason" }`, which SDK
# consumers had to pattern-match on the English `detail` to route — the
# exact "typed route becomes detail string" antipattern R3-6 + R5-3
# closed for transport / observation / continuity / modality / status /
# error_code. Future core variants now surface as `Unknown { debug }`
# with the `{:?}` projection of the source variant preserved for
# server-side observability.
class WireLiveConfigRejectionReasonChannelIdentitySwap(TypedDict, total=False):
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

class WireLiveConfigRejectionReasonVideoFrameInputNotImplemented(TypedDict, total=False):
    kind: Required[Literal['video_frame_input_not_implemented']]

class WireLiveConfigRejectionReasonUnsupportedInputChunkVariant(TypedDict, total=False):
    kind: Required[Literal['unsupported_input_chunk_variant']]

class WireLiveConfigRejectionReasonRefreshModelSwap(TypedDict, total=False):
    from_model: Required[str]
    kind: Required[Literal['refresh_model_swap']]
    to_model: Required[str]

class WireLiveConfigRejectionReasonRefreshProviderSwap(TypedDict, total=False):
    from_provider: Required[str]
    kind: Required[Literal['refresh_provider_swap']]
    to_provider: Required[str]

class WireLiveConfigRejectionReasonRefreshAudioConfigMismatch(TypedDict, total=False):
    detail: Required[str]
    kind: Required[Literal['refresh_audio_config_mismatch']]

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

WireLiveConfigRejectionReason = WireLiveConfigRejectionReasonChannelIdentitySwap | WireLiveConfigRejectionReasonNonRealtimeResolution | WireLiveConfigRejectionReasonImageInputNotImplemented | WireLiveConfigRejectionReasonVideoFrameInputNotImplemented | WireLiveConfigRejectionReasonUnsupportedInputChunkVariant | WireLiveConfigRejectionReasonRefreshModelSwap | WireLiveConfigRejectionReasonRefreshProviderSwap | WireLiveConfigRejectionReasonRefreshAudioConfigMismatch | WireLiveConfigRejectionReasonAudioInputFormatMismatch | WireLiveConfigRejectionReasonOther | WireLiveConfigRejectionReasonUnknown

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
# Serde shape mirrors the core enum exactly: internally-tagged on
# `observation` (snake_case). Round-trip with the core type is byte-
# identical (see `wire_live_adapter_observation_byte_compatible_with_core`).
#
# Field types reference other wire mirrors where they exist
# ([`WireStopReason`], [`WireUsage`], [`WireLiveAdapterStatus`],
# [`WireLiveAdapterErrorCode`]) and the canonical
# [`RealtimeTranscriptEvent`] (which already derives `JsonSchema` and is
# auto-promoted by the SDK codegen `Realtime*` allowlist rule).
#
# Audio data is base64-encoded on the wire (matches the core
# [`LiveAdapterObservation::AssistantAudioChunk`] base64 mode); the wire
# mirror carries `data` as a `String` so the schema emits `String` instead
# of an opaque `Vec<u8>` JSON-array shape.
class WireLiveAdapterObservationReady(TypedDict, total=False):
    observation: Required[Literal['ready']]

class WireLiveAdapterObservationUserTranscriptFinal(TypedDict, total=False):
    content_index: NotRequired[int]
    observation: Required[Literal['user_transcript_final']]
    previous_item_id: NotRequired[str]
    provider_item_id: NotRequired[str]
    text: Required[str]

class WireLiveAdapterObservationAssistantTextDelta(TypedDict, total=False):
    content_index: NotRequired[int]
    delta: Required[str]
    delta_id: NotRequired[str]
    observation: Required[Literal['assistant_text_delta']]
    previous_item_id: NotRequired[str]
    provider_item_id: NotRequired[str]
    response_id: NotRequired[str]

class WireLiveAdapterObservationAssistantTranscriptDelta(TypedDict, total=False):
    content_index: NotRequired[int]
    delta: Required[str]
    delta_id: NotRequired[str]
    observation: Required[Literal['assistant_transcript_delta']]
    previous_item_id: NotRequired[str]
    provider_item_id: NotRequired[str]
    response_id: NotRequired[str]

class WireLiveAdapterObservationAssistantAudioChunk(TypedDict, total=False):
    channels: Required[int]
    content_index: NotRequired[int]
    data: Required[str]
    item_id: NotRequired[str]
    observation: Required[Literal['assistant_audio_chunk']]
    response_id: NotRequired[str]
    sample_rate_hz: Required[int]

class WireLiveAdapterObservationAssistantTranscriptFinal(TypedDict, total=False):
    content_index: NotRequired[int]
    observation: Required[Literal['assistant_transcript_final']]
    previous_item_id: NotRequired[str]
    provider_item_id: Required[str]
    response_id: NotRequired[str]
    stop_reason: Required[Literal['end_turn', 'tool_use', 'max_tokens', 'stop_sequence', 'content_filter', 'cancelled']]
    text: Required[str]
    usage: Required[dict[str, Any]]

class WireLiveAdapterObservationAssistantTranscriptTruncated(TypedDict, total=False):
    content_index: NotRequired[int]
    observation: Required[Literal['assistant_transcript_truncated']]
    previous_item_id: NotRequired[str]
    provider_item_id: NotRequired[str]
    response_id: NotRequired[str]
    text: NotRequired[str]

class WireLiveAdapterObservationRealtimeTranscript(TypedDict, total=False):
    event: Required[RealtimeTranscriptEvent]
    observation: Required[Literal['realtime_transcript']]

class WireLiveAdapterObservationToolCallRequested(TypedDict, total=False):
    arguments: Required[Any]
    observation: Required[Literal['tool_call_requested']]
    provider_call_id: Required[str]
    tool_name: Required[str]

class WireLiveAdapterObservationTurnInterrupted(TypedDict, total=False):
    observation: Required[Literal['turn_interrupted']]
    response_id: NotRequired[str]

class WireLiveAdapterObservationTurnCompleted(TypedDict, total=False):
    observation: Required[Literal['turn_completed']]
    response_id: NotRequired[str]
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

WireLiveAdapterObservation = WireLiveAdapterObservationReady | WireLiveAdapterObservationUserTranscriptFinal | WireLiveAdapterObservationAssistantTextDelta | WireLiveAdapterObservationAssistantTranscriptDelta | WireLiveAdapterObservationAssistantAudioChunk | WireLiveAdapterObservationAssistantTranscriptFinal | WireLiveAdapterObservationAssistantTranscriptTruncated | WireLiveAdapterObservationRealtimeTranscript | WireLiveAdapterObservationToolCallRequested | WireLiveAdapterObservationTurnInterrupted | WireLiveAdapterObservationTurnCompleted | WireLiveAdapterObservationStatusChanged | WireLiveAdapterObservationError | WireLiveAdapterObservationCommandRejected | WireLiveAdapterObservationUnknown

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
    history: Optional[list[dict[str, Any]]] = None
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
    schedules: list[dict[str, Any]]


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
    """Semantic contract version triple."""


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
    models: list[dict[str, Any]]
    provider: str


@dataclass
class ModelsCatalogResponse:
    """Response for `models/catalog` — the compiled-in model catalog."""
    contract_version: dict[str, Any]
    providers: list[dict[str, Any]]


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
    blob_ref: dict[str, Any]
    height: int
    image_id: str
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
    native_metadata: dict[str, Any]
    operation_id: str
    provider_text: dict[str, Any]
    revised_prompt: dict[str, Any]
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
    auth_profiles: dict[str, dict[str, Any]]
    backends: dict[str, dict[str, Any]]
    bindings: dict[str, dict[str, Any]]
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
    auth_profile: dict[str, Any]
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
    realms: list[dict[str, Any]]


@dataclass
class WireAuthProfilesList:
    """`GET /auth/profiles` success body — realm-scoped lists of backend,
auth, and binding profiles."""
    auth_profiles: list[dict[str, Any]]
    backend_profiles: list[dict[str, Any]]
    bindings: list[dict[str, Any]]
    realm_id: str


@dataclass
class WireAuthStatus:
    """Wire projection of the auth-profile status. Returned from
`auth.status.get` / `GET /auth/bindings/{binding_id}/status`."""
    auth_method: str
    profile_id: str
    provider: str
    state: Literal['valid', 'expiring', 'expired', 'reauth_required', 'refresh_failed', 'unknown']
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
    state: Literal['valid', 'expiring', 'expired', 'reauth_required', 'refresh_failed', 'unknown']
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

WireAuthError = WireAuthErrorMissingSecret | WireAuthErrorUnsupportedCombination | WireAuthErrorMissingRequiredMetadata | WireAuthErrorWorkspaceMismatch | WireAuthErrorExpired | WireAuthErrorStaleCredential | WireAuthErrorRefreshRequired | WireAuthErrorLeaseAbsent | WireAuthErrorUserReauthRequired | WireAuthErrorRefreshFailed | WireAuthErrorInteractiveLoginRequired | WireAuthErrorHostOwnedUnavailable | WireAuthErrorIo | WireAuthErrorOther

# Wire-safe content block (no `source_path` — internal only).
class WireContentBlockText(TypedDict, total=False):
    text: Required[str]
    type: Required[Literal['text']]

class WireContentBlockImage(TypedDict, total=False):
    media_type: Required[str]
    type: Required[Literal['image']]

class WireContentBlockVideo(TypedDict, total=False):
    duration_ms: Required[int]
    media_type: Required[str]
    type: Required[Literal['video']]

class WireContentBlockUnknown(TypedDict, total=False):
    type: Required[Literal['unknown']]

WireContentBlock = WireContentBlockText | WireContentBlockImage | WireContentBlockVideo | WireContentBlockUnknown

# Wire-safe content input (mirrors `ContentInput`).
WireContentInput = str | list[WireContentBlock]

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

# Runtime binding for spawn requests.
#
# First step toward identity-first mobs. Carries backend-specific binding
# details at spawn time. `External` requires typed process identity; callers
# do not supply raw comms peer IDs.
class WireRuntimeBindingSession(TypedDict, total=False):
    kind: Required[Literal['session']]

class WireRuntimeBindingExternal(TypedDict, total=False):
    address: Required[str]
    bootstrap_token: NotRequired[BridgeBootstrapToken]
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

# Tool access policy for a spawned mob member.
class WireToolAccessPolicyInherit(TypedDict, total=False):
    type: Required[Literal['inherit']]

class WireToolAccessPolicyAllowList(TypedDict, total=False):
    type: Required[Literal['allow_list']]
    value: Required[list[str]]

class WireToolAccessPolicyDenyList(TypedDict, total=False):
    type: Required[Literal['deny_list']]
    value: Required[list[str]]

WireToolAccessPolicy = WireToolAccessPolicyInherit | WireToolAccessPolicyAllowList | WireToolAccessPolicyDenyList

# Budget split policy for a spawned mob member.
class WireBudgetSplitPolicyEqual(TypedDict, total=False):
    type: Required[Literal['equal']]

class WireBudgetSplitPolicyProportional(TypedDict, total=False):
    type: Required[Literal['proportional']]

class WireBudgetSplitPolicyRemaining(TypedDict, total=False):
    type: Required[Literal['remaining']]

class WireBudgetSplitPolicyFixed(TypedDict, total=False):
    type: Required[Literal['fixed']]
    value: Required[int]

WireBudgetSplitPolicy = WireBudgetSplitPolicyEqual | WireBudgetSplitPolicyProportional | WireBudgetSplitPolicyRemaining | WireBudgetSplitPolicyFixed

# Pre-resolved tool filter inherited by a spawned mob member.
WireToolFilter = Literal['All'] | dict[str, list[str]]

# Roster member lifecycle state for `MobMemberFilterWire`.
WireMemberState = Literal['active', 'retiring']

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
    branch: NotRequired[str]
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

# Mob RPC helper wire type for MobStepOutputFormatInput.
MobStepOutputFormatInput = Literal['json', 'text']

# Closed wire stage for a per-identity `mob/reconcile` failure.
WireMobReconcileStage = Literal['spawn', 'retire']

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
RealtimeInputKind = Literal['text', 'audio', 'video']

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

RealtimeInputChunk = RealtimeInputChunkTextChunk | RealtimeInputChunkAudioChunk | RealtimeInputChunkVideoChunk

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
# The core `Provider` enum uses `#[serde(rename_all = "snake_case")]` which
# transforms `OpenAI` to `"open_a_i"` on the wire -- not the conventional
# `"openai"`. `WireProvider` pins the correct wire names with explicit
# `#[serde(rename)]` on each variant so SDK consumers see `"openai"`,
# `"anthropic"`, `"gemini"`, etc.
#
# Includes an `Unknown { debug: String }` variant for future-proofing per
# the wire-mirror dogma used throughout this module.
WireProvider = Literal['anthropic', 'openai', 'gemini', 'self_hosted', 'other'] | Literal['unknown']

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
    meta: NotRequired[dict[str, Any]]
    text: Required[str]

class WireAssistantBlockText(TypedDict, total=False):
    block_type: Required[Literal['text']]
    data: Required[WireAssistantBlockTextData]

class WireAssistantBlockTranscriptData(TypedDict, total=False):
    meta: NotRequired[dict[str, Any]]
    source: Required[WireTranscriptSource]
    text: Required[str]

class WireAssistantBlockTranscript(TypedDict, total=False):
    block_type: Required[Literal['transcript']]
    data: Required[WireAssistantBlockTranscriptData]

class WireAssistantBlockReasoningData(TypedDict, total=False):
    meta: NotRequired[dict[str, Any]]
    text: NotRequired[str]

class WireAssistantBlockReasoning(TypedDict, total=False):
    block_type: Required[Literal['reasoning']]
    data: Required[WireAssistantBlockReasoningData]

class WireAssistantBlockToolUseData(TypedDict, total=False):
    args: Required[Any]
    id: Required[str]
    meta: NotRequired[dict[str, Any]]
    name: Required[str]

class WireAssistantBlockToolUse(TypedDict, total=False):
    block_type: Required[Literal['tool_use']]
    data: Required[WireAssistantBlockToolUseData]

class WireAssistantBlockServerToolContentData(TypedDict, total=False):
    content: Required[Any]
    id: NotRequired[str]
    meta: NotRequired[dict[str, Any]]
    name: Required[str]

class WireAssistantBlockServerToolContent(TypedDict, total=False):
    block_type: Required[Literal['server_tool_content']]
    data: Required[WireAssistantBlockServerToolContentData]

class WireAssistantBlockImageData(TypedDict, total=False):
    blob_ref: Required[dict[str, Any]]
    height: Required[int]
    image_id: Required[str]
    media_type: Required[str]
    meta: Required[dict[str, Any]]
    revised_prompt: Required[dict[str, Any]]
    width: Required[int]

class WireAssistantBlockImage(TypedDict, total=False):
    block_type: Required[Literal['image']]
    data: Required[WireAssistantBlockImageData]

class WireAssistantBlockUnknown(TypedDict, total=False):
    block_type: Required[Literal['unknown']]

WireAssistantBlock = WireAssistantBlockText | WireAssistantBlockTranscript | WireAssistantBlockReasoning | WireAssistantBlockToolUse | WireAssistantBlockServerToolContent | WireAssistantBlockImage | WireAssistantBlockUnknown

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
    allow_self_session: NotRequired[bool]
    blocks: NotRequired[list[ContentBlock]]
    body: Required[str]
    handling_mode: NotRequired[HandlingMode]
    kind: Required[Literal['input']]
    source: NotRequired[Literal['tcp', 'uds', 'stdin', 'webhook', 'rpc']]
    stream: NotRequired[Literal['none', 'reserve_interaction']]

class CommsCommandPeerMessage(TypedDict, total=False):
    blocks: NotRequired[list[ContentBlock]]
    body: Required[str]
    handling_mode: NotRequired[HandlingMode]
    kind: Required[Literal['peer_message']]
    to: Required[PeerId]

class CommsCommandPeerLifecycle(TypedDict, total=False):
    kind: Required[Literal['peer_lifecycle']]
    lifecycle_kind: Required[Literal['mob.peer_added', 'mob.peer_retired', 'mob.peer_unwired']]
    params: Required[CommsPeerLifecycleParams]
    to: Required[PeerId]

class CommsCommandPeerRequest(TypedDict, total=False):
    blocks: NotRequired[list[ContentBlock]]
    handling_mode: NotRequired[HandlingMode]
    intent: Required[CommsPeerRequestIntent]
    kind: Required[Literal['peer_request']]
    params: Required[CommsPeerRequestParams]
    stream: NotRequired[Literal['none', 'reserve_interaction']]
    to: Required[PeerId]

class CommsCommandPeerResponse(TypedDict, total=False):
    blocks: NotRequired[list[ContentBlock]]
    handling_mode: NotRequired[HandlingMode]
    in_reply_to: Required[str]
    kind: Required[Literal['peer_response']]
    result: NotRequired[CommsPeerResponseResult]
    status: Required[Literal['accepted', 'completed', 'failed']]
    to: Required[PeerId]

CommsCommandRequest = CommsCommandInput | CommsCommandPeerMessage | CommsCommandPeerLifecycle | CommsCommandPeerRequest | CommsCommandPeerResponse

# Canonical realm-local blob identifier.
#
# The identifier is content-addressed, but storage and GC semantics remain
# realm-scoped.
BlobId = str

# One-time bootstrap proof exchanged between a mob supervisor and a
# member runtime on initial bind.
#
# Transparent over the wire (`#[serde(transparent)]` — a bare JSON string),
# but carries a redacting `Debug` impl and has no `Display` impl so the
# raw secret cannot accidentally land in logs or panic messages. Treat it
# like an API key: read `as_str()` only at the comms/transport boundary.
BridgeBootstrapToken = str

# A typed command sent from a supervisor to a member runtime.
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
    handling_mode: Required[HandlingMode]
    input_id: Required[str]
    protocol_version: Required[BridgeProtocolVersion]
    supervisor: Required[BridgePeerSpec]

class BridgeCommandObserveMember(TypedDict, total=False):
    command: Required[Literal['observe_member']]
    epoch: Required[int]
    protocol_version: Required[BridgeProtocolVersion]
    supervisor: Required[BridgePeerSpec]

class BridgeCommandInterruptMember(TypedDict, total=False):
    command: Required[Literal['interrupt_member']]
    epoch: Required[int]
    protocol_version: Required[BridgeProtocolVersion]
    supervisor: Required[BridgePeerSpec]

class BridgeCommandHardCancelMember(TypedDict, total=False):
    command: Required[Literal['hard_cancel_member']]
    epoch: Required[int]
    protocol_version: Required[BridgeProtocolVersion]
    reason: Required[str]
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
    peer_spec: Required[BridgePeerSpec]
    protocol_version: Required[BridgeProtocolVersion]
    supervisor: Required[BridgePeerSpec]

class BridgeCommandUnwireMember(TypedDict, total=False):
    command: Required[Literal['unwire_member']]
    epoch: Required[int]
    peer_spec: Required[BridgePeerSpec]
    protocol_version: Required[BridgeProtocolVersion]
    supervisor: Required[BridgePeerSpec]

BridgeCommand = BridgeCommandBindMember | BridgeCommandAuthorizeSupervisor | BridgeCommandRevokeSupervisor | BridgeCommandDeliverMemberInput | BridgeCommandObserveMember | BridgeCommandInterruptMember | BridgeCommandHardCancelMember | BridgeCommandRetireMember | BridgeCommandDestroyMember | BridgeCommandWireMember | BridgeCommandUnwireMember

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

BridgeDeliveryRejectionCause = BridgeDeliveryRejectionCauseNotReady | BridgeDeliveryRejectionCauseDurabilityViolation | BridgeDeliveryRejectionCausePeerHandlingModeInvalid | BridgeDeliveryRejectionCauseInternal

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
BridgeRejectionCause = Literal['not_bound', 'stale_supervisor', 'sender_mismatch', 'already_bound', 'invalid_bootstrap_token', 'unsupported_protocol_version', 'invalid_supervisor_spec', 'invalid_peer_spec', 'address_mismatch', 'unsupported', 'internal']

# A typed reply from a member runtime back to the supervisor.
class BridgeReplyBindMember(TypedDict, total=False):
    address: Required[str]
    capabilities: Required[BridgeCapabilities]
    peer_id: Required[str]
    result: Required[Literal['bind_member']]

class BridgeReplyAck(TypedDict, total=False):
    ok: Required[bool]
    result: Required[Literal['ack']]

class BridgeReplyObservation(TypedDict, total=False):
    accepting_inputs: NotRequired[bool]
    current_run_id: NotRequired[str]
    last_error: NotRequired[str]
    observed_at: Required[str]
    peer_connectivity: NotRequired[BridgePeerConnectivity]
    result: Required[Literal['observation']]
    state: Required[BridgeMemberRuntimeState]

class BridgeReplyDelivery(TypedDict, total=False):
    canonical_input_id: NotRequired[str]
    input_id: Required[str]
    outcome: Required[BridgeDeliveryOutcome]
    result: Required[Literal['delivery']]

class BridgeReplyRetire(TypedDict, total=False):
    inputs_abandoned: Required[int]
    inputs_pending_drain: Required[int]
    result: Required[Literal['retire']]

class BridgeReplyDestroy(TypedDict, total=False):
    inputs_abandoned: Required[int]
    result: Required[Literal['destroy']]

class BridgeReplyRejected(TypedDict, total=False):
    cause: Required[BridgeRejectionCause]
    reason: Required[str]
    result: Required[Literal['rejected']]

BridgeReply = BridgeReplyBindMember | BridgeReplyAck | BridgeReplyObservation | BridgeReplyDelivery | BridgeReplyRetire | BridgeReplyDestroy | BridgeReplyRejected

# Comms/session-stream RPC contract for ContentBlock.
class ContentBlockText(TypedDict, total=False):
    text: Required[str]
    type: Required[Literal['text']]

class ContentBlockImage(TypedDict, total=False):
    media_type: Required[str]
    type: Required[Literal['image']]

class ContentBlockVideo(TypedDict, total=False):
    duration_ms: Required[int]
    media_type: Required[str]
    type: Required[Literal['video']]

ContentBlock = ContentBlockText | ContentBlockImage | ContentBlockVideo

# Input content that can be either a plain text string or multimodal content blocks.
#
# Deserializes from either `"text"` or `[{type: "text", text: "..."}, ...]`.
# Provides `From<String>` and `From<&str>` for ergonomic construction.
ContentInput = str | list[ContentBlock]

# Request payload for `comms/send`.
class CommsSendParamsInput(TypedDict, total=False):
    allow_self_session: NotRequired[bool]
    blocks: NotRequired[list[ContentBlock]]
    body: Required[str]
    handling_mode: NotRequired[HandlingMode]
    kind: Required[Literal['input']]
    session_id: Required[str]
    source: NotRequired[Literal['tcp', 'uds', 'stdin', 'webhook', 'rpc']]
    stream: NotRequired[Literal['none', 'reserve_interaction']]

class CommsSendParamsPeerMessage(TypedDict, total=False):
    blocks: NotRequired[list[ContentBlock]]
    body: Required[str]
    handling_mode: NotRequired[HandlingMode]
    kind: Required[Literal['peer_message']]
    session_id: Required[str]
    to: Required[PeerId]

class CommsSendParamsPeerLifecycle(TypedDict, total=False):
    kind: Required[Literal['peer_lifecycle']]
    lifecycle_kind: Required[Literal['mob.peer_added', 'mob.peer_retired', 'mob.peer_unwired']]
    params: Required[CommsPeerLifecycleParams]
    session_id: Required[str]
    to: Required[PeerId]

class CommsSendParamsPeerRequest(TypedDict, total=False):
    blocks: NotRequired[list[ContentBlock]]
    handling_mode: NotRequired[HandlingMode]
    intent: Required[CommsPeerRequestIntent]
    kind: Required[Literal['peer_request']]
    params: Required[CommsPeerRequestParams]
    session_id: Required[str]
    stream: NotRequired[Literal['none', 'reserve_interaction']]
    to: Required[PeerId]

class CommsSendParamsPeerResponse(TypedDict, total=False):
    blocks: NotRequired[list[ContentBlock]]
    handling_mode: NotRequired[HandlingMode]
    in_reply_to: Required[str]
    kind: Required[Literal['peer_response']]
    result: NotRequired[CommsPeerResponseResult]
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
    acked: Required[bool]
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

# Closed public request-intent contract for `peer_request`.
#
# Unknown strings fail during deserialization and cannot fall through to a
# local match/default path.
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

# Comms/session-stream RPC contract for PeerReachability.
PeerReachability = Literal['unknown', 'reachable', 'unreachable']

# Comms/session-stream RPC contract for PeerReachabilityReason.
PeerReachabilityReason = Literal['offline_or_no_ack', 'transport_error'] | Literal['admission_dropped']
