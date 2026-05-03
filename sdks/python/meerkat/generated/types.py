from __future__ import annotations

"""Generated wire types for Meerkat SDK.

Contract version: 0.6.0
"""

from dataclasses import dataclass, field
from typing import Any, Literal, NotRequired, Optional, Required, TypedDict


CONTRACT_VERSION = "0.6.0"


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
class McpAddParams:
    """Request payload for `mcp/add`."""
    server_config: Any
    server_name: str
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
    auto_wire_parent: Optional[bool] = None
    backend: Optional[WireMobBackendKind] = None
    binding: Optional[WireRuntimeBinding] = None
    budget_split_policy: Optional[WireBudgetSplitPolicy] = None
    connection_ref: Optional[WireConnectionRef] = None
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
    backend: Optional[WireMobBackendKind] = None
    connection_ref: Optional[WireConnectionRef] = None
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
    mcp: Optional[list[str]] = None
    memory: Optional[bool] = None
    mob: Optional[bool] = None
    mob_tasks: Optional[bool] = None
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
    clear_connection_ref: Optional[bool] = None
    clear_provider_params: Optional[bool] = None
    connection_ref: Optional[WireConnectionRef] = None
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
    realtime_attachment_status: Optional[str] = None


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
class MobRotateSupervisorResult:
    """Response payload for `mob/rotate_supervisor`."""
    mob_id: str
    ok: bool
    report: Any


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
    mcp_servers: Optional[dict[str, MobMcpServerConfigInput]] = None
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
class MobMcpServerConfigInput:
    """Request payload for MobMcpServerConfigInput."""
    command: Optional[list[str]] = None
    env: Optional[dict[str, str]] = None
    url: Optional[str] = None


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
    mcp: Optional[list[str]] = None
    memory: Optional[bool] = None
    mob: Optional[bool] = None
    mob_tasks: Optional[bool] = None
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
class RuntimeStateParams:
    """Request payload for `runtime/session_status`."""
    session_id: str


@dataclass
class RuntimeRealtimeAttachmentStatusParams:
    """Request payload for `session/realtime_attachment_status`."""
    session_id: str


@dataclass
class RealtimeOpenRequest:
    """Request payload for `realtime/open_info`."""
    role: RealtimeChannelRole
    target: RealtimeChannelTarget
    turning_mode: RealtimeTurningMode
    channel_config: Optional[RealtimeChannelConfig] = None
    reconnect_policy: Optional[RealtimeReconnectPolicy] = None


@dataclass
class RealtimeStatusParams:
    """Request payload for `realtime/status`."""
    target: RealtimeChannelTarget


@dataclass
class RealtimeCapabilitiesParams:
    """Request payload for `realtime/capabilities`."""
    target: RealtimeChannelTarget


@dataclass
class RuntimeAcceptParams:
    """Request payload for `runtime/session_submit`.

The runtime input body is owned by `meerkat-runtime`; contracts cannot
depend on the runtime crate without inverting the dependency graph. The
envelope is canonical here and the runtime crate remains the typed
authority over the `input` discriminator and variant body."""
    input: Any
    session_id: str


@dataclass
class RuntimeRetireParams:
    """Request payload for `runtime/session_retire`."""
    session_id: str


@dataclass
class RuntimeResetParams:
    """Request payload for `runtime/session_reset`."""
    session_id: str


@dataclass
class InputStateParams:
    """Request payload for `runtime/session_submission`."""
    input_id: str
    session_id: str


@dataclass
class InputListParams:
    """Request payload for `runtime/session_submissions`."""
    session_id: str


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
class McpLiveOpResponse:
    """Response payload for live MCP operations."""
    operation: Literal['add', 'remove', 'reload']
    persisted: bool
    session_id: str
    status: Literal['staged', 'applied', 'rejected']
    applied_at_turn: Optional[int] = None
    server_name: Optional[str] = None


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
    """Response payload for runtime/session_status."""
    state: WireRuntimeState


@dataclass
class RuntimeRealtimeAttachmentStatusResult:
    """Response payload for `session/realtime_attachment_status`."""
    status: Literal['unattached', 'intent_present_unbound', 'binding_not_ready', 'binding_ready', 'replacement_pending', 'reattach_required']


@dataclass
class RealtimeReconnectPolicy:
    """Public reconnect policy for a realtime channel."""
    initial_backoff_ms: int
    max_attempts: int
    max_backoff_ms: int
    max_total_ms: int


@dataclass
class RealtimeChannelConfig:
    """Per-channel runtime knobs negotiated at open time.

Additive fields only — clients that do not carry this struct inherit the
server-default behavior via `#[serde(default)]` on the parent
[`RealtimeOpenRequest`]."""
    tool_timeout_ms: Optional[int] = None


@dataclass
class RealtimeAudioFormat:
    """Descriptor for an expected or actual realtime audio format."""
    channels: int
    mime_type: str
    sample_rate_hz: int


@dataclass
class RealtimeCapabilities:
    """Product-facing realtime capability set for one target/provider combination."""
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
class RealtimeChannelStatus:
    """Public realtime channel status projection."""
    state: RealtimeChannelState
    attempt_count: Optional[int] = None
    deadline_at: Optional[str] = None
    next_retry_at: Optional[str] = None
    reason: Optional[str] = None


@dataclass
class RealtimeOpenInfo:
    """Response payload for `realtime/open_info`."""
    capabilities: RealtimeCapabilities
    default_protocol_version: str
    expires_at: str
    open_token: str
    target: RealtimeChannelTarget
    ws_url: str
    supported_protocol_versions: Optional[list[str]] = None


@dataclass
class RealtimeStatusResult:
    """Response payload for `realtime/status`."""
    status: RealtimeChannelStatus


@dataclass
class RealtimeCapabilitiesResult:
    """Response payload for `realtime/capabilities`."""
    capabilities: RealtimeCapabilities


@dataclass
class RealtimeTextChunk:
    """A text chunk for realtime ingress/egress."""
    text: str


@dataclass
class RealtimeTextDelta:
    """A text delta chunk for realtime output."""
    delta: str


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
class RealtimeBargeInTruncateFrame:
    """Payload for `channel.barge_in_truncate`.

Sent by the client the moment it detects the user starting to speak over
the assistant's audio, to tell the server what prefix was actually heard.
Field names mirror OpenAI Realtime's `conversation.item.truncate` so the
provider adapter can map straight through without re-encoding."""
    audio_played_ms: int
    content_index: int
    item_id: str


@dataclass
class AudioFormatMismatchContext:
    """Typed context carried alongside a [`RealtimeErrorCode::AudioFormatMismatch`]."""
    actual: RealtimeAudioFormat
    expected: RealtimeAudioFormat


@dataclass
class ToolCallTimeoutContext:
    """Typed context carried alongside a [`RealtimeErrorCode::ToolCallTimeout`]."""
    call_id: str
    elapsed_ms: int
    timeout_ms: int


@dataclass
class RealtimeChannelOpenFrame:
    """Payload for `channel.open`."""
    open_token: str
    protocol_version: str
    role: RealtimeChannelRole
    turning_mode: RealtimeTurningMode


@dataclass
class RealtimeChannelInputFrame:
    """Payload for `channel.input`."""
    chunk: RealtimeInputChunk


@dataclass
class RealtimeChannelOpenedFrame:
    """Payload for `channel.opened`."""
    capabilities: RealtimeCapabilities
    protocol_version: str
    role: RealtimeChannelRole
    status: RealtimeChannelStatus


@dataclass
class RealtimeChannelStatusFrame:
    """Payload for `channel.status`."""
    status: RealtimeChannelStatus


@dataclass
class RealtimeChannelEventFrame:
    """Payload for `channel.event`."""
    event: RealtimeEvent


@dataclass
class RealtimeChannelErrorFrame:
    """Payload for `channel.error`."""
    code: RealtimeErrorCode
    message: str
    details: Optional[RealtimeErrorDetails] = None


@dataclass
class RealtimeChannelClosedFrame:
    """Payload for `channel.closed`."""
    reason: Optional[str] = None


@dataclass
class RuntimeAcceptResult:
    """Response payload for `runtime/session_submit`."""
    outcome_type: Literal['accepted', 'deduplicated', 'rejected']
    existing_id: Optional[str] = None
    input_id: Optional[str] = None
    policy: Optional[Literal['stage', 'queue', 'immediate']] = None
    reason: Optional[str] = None
    state: Optional[dict[str, Any]] = None


@dataclass
class RuntimeRetireResult:
    """Response payload for `runtime/session_retire`."""
    inputs_abandoned: int
    inputs_pending_drain: Optional[int] = None


@dataclass
class RuntimeResetResult:
    """Response payload for `runtime/session_reset`."""
    inputs_abandoned: int


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
class InputListResult:
    """Response payload for `runtime/session_submissions`."""
    inputs: list[dict[str, Any]]


@dataclass
class ScheduleListResult:
    """Response payload for schedule/list."""
    schedules: list[dict[str, Any]]


@dataclass
class ScheduleOccurrencesResult:
    """Response payload for schedule/occurrences."""
    occurrences: list[dict[str, Any]]


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
    inline_video: bool
    model_family: str
    params_schema: Any
    supports_reasoning: bool
    supports_temperature: bool
    supports_thinking: bool
    beta_headers: Optional[list[dict[str, Any]]] = None
    supports_web_search: Optional[bool] = None


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
class WireConnectionRef:
    """Wire projection of [`meerkat_core::ConnectionRef`].

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
auth REST response that returns a `{realm_id, binding_id, connection_ref}`
trio. Built from a typed [`meerkat_core::ConnectionRef`] so the three
fields always agree; the `realm_id`/`binding_id` strings carry the
slug form for wire consumers that key by string."""
    binding_id: str
    connection_ref: WireConnectionRef
    realm_id: str


@dataclass
class WireAuthProfileCreated:
    """`POST /auth/profiles` (create) success body."""
    auth_method: str
    binding_id: str
    connection_ref: WireConnectionRef
    profile_id: str
    provider: str
    realm_id: str
    stored: bool


@dataclass
class WireAuthProfileDetail:
    """`GET /auth/bindings/{binding_id}` success body."""
    auth_profile: dict[str, Any]
    binding_id: str
    connection_ref: WireConnectionRef
    profile_id: str


@dataclass
class WireAuthProfileCleared:
    """`DELETE /auth/bindings/{binding_id}` /
`POST /auth/bindings/{binding_id}/logout` success body."""
    binding_id: str
    cleared: bool
    connection_ref: WireConnectionRef
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
    binding_id: str
    connection_ref: WireConnectionRef
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
`connection_ref` / `has_refresh_token` so the caller can key by
binding directly."""
    auth_method: str
    binding_id: str
    connection_ref: WireConnectionRef
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

WireAuthError = WireAuthErrorMissingSecret | WireAuthErrorUnsupportedCombination | WireAuthErrorMissingRequiredMetadata | WireAuthErrorWorkspaceMismatch | WireAuthErrorExpired | WireAuthErrorRefreshFailed | WireAuthErrorInteractiveLoginRequired | WireAuthErrorHostOwnedUnavailable | WireAuthErrorIo | WireAuthErrorOther

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
    bootstrap_token: NotRequired[str]
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

# Public live attachment status projection used by runtime and mob surfaces.
WireRealtimeAttachmentStatus = Literal['unattached', 'intent_present_unbound', 'binding_not_ready', 'binding_ready', 'replacement_pending', 'reattach_required']

# Target for a public realtime channel.
#
# Two variants, one for each addressing mode:
#
# - `SessionTarget` — standalone sessions (no mob-member continuity). The
#   session id is pinned for the channel's lifetime; when that session
#   ends, the channel ends.
#
# - `MobMember` — mob-member continuity (W3-H / dogma #4). Identity is the
#   canonical anchor, and the server resolves the current bridge session
#   on every tick from the MobMachine's `member_session_bindings` map.
#   Respawn atomically rotates the bridge session via the
#   `MemberSessionBindingChanged { old: Some, new: Some }` effect; the
#   channel survives without any SDK round-trip. A terminal
#   `MemberSessionBindingChanged { old: Some, new: None }` closes the
#   channel with `RealtimeErrorCode::BindingReleased`.
class RealtimeChannelTargetSessionTarget(TypedDict, total=False):
    session_id: Required[str]
    type: Required[Literal['session_target']]

class RealtimeChannelTargetMobMember(TypedDict, total=False):
    agent_identity: Required[str]
    mob_id: Required[str]
    type: Required[Literal['mob_member']]

RealtimeChannelTarget = RealtimeChannelTargetSessionTarget | RealtimeChannelTargetMobMember

# Opening role for a realtime channel.
RealtimeChannelRole = Literal['primary', 'observer']

# Turning mode for a realtime channel.
RealtimeTurningMode = Literal['provider_managed', 'explicit_commit']

# Input modality kind supported by a realtime channel.
RealtimeInputKind = Literal['text', 'audio', 'video']

# Output modality kind supported by a realtime channel.
RealtimeOutputKind = Literal['text', 'audio', 'video']

# Lifecycle state for a realtime channel.
RealtimeChannelState = Literal['opening', 'ready', 'interrupted', 'reconnecting', 'closed', 'error']

# Typed realtime channel error code. Replaces the prior freeform `String` on
# [`RealtimeChannelErrorFrame`]; all paths that mint a channel error pick
# exactly one variant so downstream SDKs can match without string folklore.
RealtimeErrorCode = Literal['invalid_frame', 'expected_channel_open', 'invalid_open_token', 'open_token_expired', 'role_mismatch', 'turning_mode_mismatch', 'unsupported_turning_mode', 'target_busy', 'unsupported_protocol_version', 'audio_format_mismatch', 'unauthorized_realm', 'tool_call_timeout', 'internal_error', 'reconnect_exhausted', 'invalid_target', 'channel_not_bound', 'runtime_internal', 'runtime_not_ready', 'provider_session_closed', 'provider_session_failed', 'provider_session_unavailable', 'unsupported_input_kind', 'no_pending_turn', 'observer_read_only', 'unexpected_channel_open', 'commit_turn_unavailable', 'channel_reconnecting', 'binding_released', 'authentication_failed', 'content_filtered', 'model_not_found', 'invalid_request']

# Structured typed context carried on a channel error frame. Each variant
# corresponds to a specific [`RealtimeErrorCode`] and lets clients match
# the reason without parsing the message string.
class RealtimeErrorDetailsAudioFormatMismatch(TypedDict, total=False):
    actual: Required[RealtimeAudioFormat]
    expected: Required[RealtimeAudioFormat]
    kind: Required[Literal['audio_format_mismatch']]

class RealtimeErrorDetailsToolCallTimeout(TypedDict, total=False):
    call_id: Required[str]
    elapsed_ms: Required[int]
    timeout_ms: Required[int]
    kind: Required[Literal['tool_call_timeout']]

class RealtimeErrorDetailsUnsupportedProtocolVersion(TypedDict, total=False):
    kind: Required[Literal['unsupported_protocol_version']]
    requested: Required[str]
    supported: Required[list[str]]

RealtimeErrorDetails = RealtimeErrorDetailsAudioFormatMismatch | RealtimeErrorDetailsToolCallTimeout | RealtimeErrorDetailsUnsupportedProtocolVersion

# Modality-neutral input chunk.
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

# Modality-neutral output chunk.
class RealtimeOutputChunkTextDelta(TypedDict, total=False):
    delta: Required[str]
    kind: Required[Literal['text_delta']]

class RealtimeOutputChunkAudioChunk(TypedDict, total=False):
    channels: Required[int]
    data: Required[str]
    mime_type: Required[str]
    sample_rate_hz: Required[int]
    kind: Required[Literal['audio_chunk']]

class RealtimeOutputChunkVideoChunk(TypedDict, total=False):
    data: Required[str]
    mime_type: Required[str]
    kind: Required[Literal['video_chunk']]

RealtimeOutputChunk = RealtimeOutputChunkTextDelta | RealtimeOutputChunkAudioChunk | RealtimeOutputChunkVideoChunk

# Normalized realtime event stream payload.
class RealtimeEventInputTranscriptPartial(TypedDict, total=False):
    text: Required[str]
    type: Required[Literal['input_transcript_partial']]

class RealtimeEventInputTranscriptFinal(TypedDict, total=False):
    prosody_hint: NotRequired[str]
    text: Required[str]
    type: Required[Literal['input_transcript_final']]

class RealtimeEventTurnStarted(TypedDict, total=False):
    type: Required[Literal['turn_started']]

class RealtimeEventTurnCommitted(TypedDict, total=False):
    type: Required[Literal['turn_committed']]

class RealtimeEventTurnCompleted(TypedDict, total=False):
    type: Required[Literal['turn_completed']]

class RealtimeEventOutputTextDelta(TypedDict, total=False):
    delta: Required[str]
    type: Required[Literal['output_text_delta']]

class RealtimeEventOutputAudioChunk(TypedDict, total=False):
    chunk: Required[RealtimeAudioChunk]
    type: Required[Literal['output_audio_chunk']]

class RealtimeEventOutputVideoChunk(TypedDict, total=False):
    chunk: Required[RealtimeVideoChunk]
    type: Required[Literal['output_video_chunk']]

class RealtimeEventInterrupted(TypedDict, total=False):
    type: Required[Literal['interrupted']]

class RealtimeEventToolCallRequested(TypedDict, total=False):
    call_id: Required[str]
    tool_name: Required[str]
    type: Required[Literal['tool_call_requested']]

class RealtimeEventToolCallCompleted(TypedDict, total=False):
    call_id: Required[str]
    type: Required[Literal['tool_call_completed']]

class RealtimeEventToolCallFailed(TypedDict, total=False):
    call_id: Required[str]
    error: Required[str]
    type: Required[Literal['tool_call_failed']]

class RealtimeEventToolCallTimedOut(TypedDict, total=False):
    call_id: Required[str]
    elapsed_ms: Required[int]
    type: Required[Literal['tool_call_timed_out']]

class RealtimeEventAssistantTranscriptTruncated(TypedDict, total=False):
    audio_played_ms: Required[int]
    item_id: Required[str]
    truncated_text: NotRequired[str]
    type: Required[Literal['assistant_transcript_truncated']]

class RealtimeEventStatusChanged(TypedDict, total=False):
    status: Required[RealtimeChannelStatus]
    type: Required[Literal['status_changed']]

class RealtimeEventNeedsReattach(TypedDict, total=False):
    type: Required[Literal['needs_reattach']]

RealtimeEvent = RealtimeEventInputTranscriptPartial | RealtimeEventInputTranscriptFinal | RealtimeEventTurnStarted | RealtimeEventTurnCommitted | RealtimeEventTurnCompleted | RealtimeEventOutputTextDelta | RealtimeEventOutputAudioChunk | RealtimeEventOutputVideoChunk | RealtimeEventInterrupted | RealtimeEventToolCallRequested | RealtimeEventToolCallCompleted | RealtimeEventToolCallFailed | RealtimeEventToolCallTimedOut | RealtimeEventAssistantTranscriptTruncated | RealtimeEventStatusChanged | RealtimeEventNeedsReattach

# Client-to-server realtime frame.
class RealtimeClientFrameChannelOpen(TypedDict, total=False):
    open_token: Required[str]
    protocol_version: Required[str]
    role: Required[RealtimeChannelRole]
    turning_mode: Required[RealtimeTurningMode]
    type: Required[Literal['channel.open']]

class RealtimeClientFrameChannelInput(TypedDict, total=False):
    chunk: Required[RealtimeInputChunk]
    type: Required[Literal['channel.input']]

class RealtimeClientFrameChannelCommitTurn(TypedDict, total=False):
    type: Required[Literal['channel.commit_turn']]

class RealtimeClientFrameChannelInterrupt(TypedDict, total=False):
    type: Required[Literal['channel.interrupt']]

class RealtimeClientFrameChannelBargeInTruncate(TypedDict, total=False):
    audio_played_ms: Required[int]
    content_index: Required[int]
    item_id: Required[str]
    type: Required[Literal['channel.barge_in_truncate']]

class RealtimeClientFrameChannelClose(TypedDict, total=False):
    type: Required[Literal['channel.close']]

RealtimeClientFrame = RealtimeClientFrameChannelOpen | RealtimeClientFrameChannelInput | RealtimeClientFrameChannelCommitTurn | RealtimeClientFrameChannelInterrupt | RealtimeClientFrameChannelBargeInTruncate | RealtimeClientFrameChannelClose

# Server-to-client realtime frame.
class RealtimeServerFrameChannelOpened(TypedDict, total=False):
    capabilities: Required[RealtimeCapabilities]
    protocol_version: Required[str]
    role: Required[RealtimeChannelRole]
    status: Required[RealtimeChannelStatus]
    type: Required[Literal['channel.opened']]

class RealtimeServerFrameChannelStatus(TypedDict, total=False):
    status: Required[RealtimeChannelStatus]
    type: Required[Literal['channel.status']]

class RealtimeServerFrameChannelEvent(TypedDict, total=False):
    event: Required[RealtimeEvent]
    type: Required[Literal['channel.event']]

class RealtimeServerFrameChannelError(TypedDict, total=False):
    code: Required[RealtimeErrorCode]
    details: NotRequired[RealtimeErrorDetails]
    message: Required[str]
    type: Required[Literal['channel.error']]

class RealtimeServerFrameChannelClosed(TypedDict, total=False):
    reason: NotRequired[str]
    type: Required[Literal['channel.closed']]

RealtimeServerFrame = RealtimeServerFrameChannelOpened | RealtimeServerFrameChannelStatus | RealtimeServerFrameChannelEvent | RealtimeServerFrameChannelError | RealtimeServerFrameChannelClosed

# Discriminator for `runtime/session_submit` responses.
RuntimeAcceptOutcomeType = Literal['accepted', 'deduplicated', 'rejected']

# Public input lifecycle state projection used by RPC surfaces.
WireInputLifecycleState = Literal['accepted', 'queued', 'staged', 'applied', 'applied_pending_consumption', 'consumed', 'superseded', 'coalesced', 'abandoned']

# Canonical stop reason for transcript messages.
WireStopReason = Literal['end_turn', 'tool_use', 'max_tokens', 'stop_sequence', 'content_filter', 'cancelled']

# Wire-safe tool result content that handles both legacy string and array formats.
WireToolResultContent = str | list[WireContentBlock]

# Transcript block inside a block-assistant message.
#
# Not `PartialEq`: `ToolUse.args` is `Box<RawValue>` for pass-through
# fidelity (core invariant — opaque from provider to dispatcher), and
# `RawValue` does not derive equality. Equivalence checks should
# round-trip through serialization and compare the serialized bytes.
class WireAssistantBlockText(TypedDict, total=False):
    block_type: Required[Literal['text']]
    data: Required[dict[str, Any]]

class WireAssistantBlockReasoning(TypedDict, total=False):
    block_type: Required[Literal['reasoning']]
    data: Required[dict[str, Any]]

class WireAssistantBlockToolUse(TypedDict, total=False):
    block_type: Required[Literal['tool_use']]
    data: Required[dict[str, Any]]

class WireAssistantBlockImage(TypedDict, total=False):
    block_type: Required[Literal['image']]
    data: Required[dict[str, Any]]

class WireAssistantBlockUnknown(TypedDict, total=False):
    block_type: Required[Literal['unknown']]

WireAssistantBlock = WireAssistantBlockText | WireAssistantBlockReasoning | WireAssistantBlockToolUse | WireAssistantBlockImage | WireAssistantBlockUnknown

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

# Typed wire request for `comms/send`.
#
# Variants are serde-tagged on `kind` and validated structurally at the
# deserialization boundary. Required fields per kind are enforced by the
# type system; invalid discriminators (`source`, `stream`, `handling_mode`,
# `status`) become serde deserialization errors rather than runtime
# string-match failures.
#
# Cross-field invariants that cannot be expressed structurally (e.g.
# `handling_mode` is forbidden on `Accepted` peer responses) are checked
# in [`CommsCommandRequest::into_command`].
class CommsCommandInput(TypedDict, total=False):
    allow_self_session: NotRequired[bool]
    blocks: NotRequired[list[dict[str, Any]]]
    body: Required[str]
    handling_mode: NotRequired[Literal['queue', 'steer']]
    kind: Required[Literal['input']]
    source: NotRequired[Literal['tcp', 'uds', 'stdin', 'webhook', 'rpc']]
    stream: NotRequired[Literal['none', 'reserve_interaction']]

class CommsCommandPeerMessage(TypedDict, total=False):
    blocks: NotRequired[list[dict[str, Any]]]
    body: Required[str]
    handling_mode: NotRequired[Literal['queue', 'steer']]
    kind: Required[Literal['peer_message']]
    to: Required[PeerId]

class CommsCommandPeerLifecycle(TypedDict, total=False):
    kind: Required[Literal['peer_lifecycle']]
    lifecycle_kind: Required[Literal['mob.peer_added', 'mob.peer_retired', 'mob.peer_unwired']]
    params: NotRequired[Any]
    to: Required[PeerId]

class CommsCommandPeerRequest(TypedDict, total=False):
    handling_mode: NotRequired[Literal['queue', 'steer']]
    intent: Required[str]
    kind: Required[Literal['peer_request']]
    params: NotRequired[Any]
    stream: NotRequired[Literal['none', 'reserve_interaction']]
    to: Required[PeerId]

class CommsCommandPeerResponse(TypedDict, total=False):
    handling_mode: NotRequired[Literal['queue', 'steer']]
    in_reply_to: Required[str]
    kind: Required[Literal['peer_response']]
    result: NotRequired[Any]
    status: Required[Literal['accepted', 'completed', 'failed']]
    to: Required[PeerId]

CommsCommandRequest = CommsCommandInput | CommsCommandPeerMessage | CommsCommandPeerLifecycle | CommsCommandPeerRequest | CommsCommandPeerResponse

# Request payload for `comms/send`.
class CommsSendParamsInput(TypedDict, total=False):
    allow_self_session: NotRequired[bool]
    blocks: NotRequired[list[dict[str, Any]]]
    body: Required[str]
    handling_mode: NotRequired[Literal['queue', 'steer']]
    kind: Required[Literal['input']]
    session_id: Required[str]
    source: NotRequired[Literal['tcp', 'uds', 'stdin', 'webhook', 'rpc']]
    stream: NotRequired[Literal['none', 'reserve_interaction']]

class CommsSendParamsPeerMessage(TypedDict, total=False):
    blocks: NotRequired[list[dict[str, Any]]]
    body: Required[str]
    handling_mode: NotRequired[Literal['queue', 'steer']]
    kind: Required[Literal['peer_message']]
    session_id: Required[str]
    to: Required[PeerId]

class CommsSendParamsPeerLifecycle(TypedDict, total=False):
    kind: Required[Literal['peer_lifecycle']]
    lifecycle_kind: Required[Literal['mob.peer_added', 'mob.peer_retired', 'mob.peer_unwired']]
    params: NotRequired[Any]
    session_id: Required[str]
    to: Required[PeerId]

class CommsSendParamsPeerRequest(TypedDict, total=False):
    handling_mode: NotRequired[Literal['queue', 'steer']]
    intent: Required[str]
    kind: Required[Literal['peer_request']]
    params: NotRequired[Any]
    session_id: Required[str]
    stream: NotRequired[Literal['none', 'reserve_interaction']]
    to: Required[PeerId]

class CommsSendParamsPeerResponse(TypedDict, total=False):
    handling_mode: NotRequired[Literal['queue', 'steer']]
    in_reply_to: Required[str]
    kind: Required[Literal['peer_response']]
    result: NotRequired[Any]
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

# Response payload for `runtime/session_submission`.
InputStateResult = Optional[WireInputState]
