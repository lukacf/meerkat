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
    labels: Optional[dict[str, Any]] = None
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
    labels: Optional[dict[str, Any]] = None
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


@dataclass
class WireTrustedPeerSpec:
    """Minimal trusted peer spec for public mob wiring surfaces.

`pubkey` is the Ed25519 signing public key (32 bytes) required so the
receiver can verify envelope signatures after trust registration.
Serialized as a 32-element JSON array of numbers (matching
`BridgePeerSpec`). Defaults to a zero pubkey for legacy clients —
the corresponding `TrustedPeerDescriptor::pubkey` will then be all
zeros, which makes signature verification fail closed. Production
clients MUST send the real pubkey."""
    address: str
    name: str
    peer_id: str
    pubkey: Optional[list[int]] = None


@dataclass
class MobWireResult:
    """Response payload for `mob/wire`."""
    wired: bool


@dataclass
class MobUnwireResult:
    """Response payload for `mob/unwire`."""
    unwired: bool


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
    labels: Optional[dict[str, Any]] = None
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
    labels: Optional[dict[str, Any]] = None
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
    auth_profiles: dict[str, Any]
    backends: dict[str, Any]
    bindings: dict[str, Any]
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
    connection_ref: dict[str, Any]
    realm_id: str


@dataclass
class WireAuthProfileCreated:
    """`POST /auth/profiles` (create) success body."""
    auth_method: str
    binding_id: str
    connection_ref: dict[str, Any]
    profile_id: str
    provider: str
    realm_id: str
    stored: bool


@dataclass
class WireAuthProfileDetail:
    """`GET /auth/bindings/{binding_id}` success body."""
    auth_profile: dict[str, Any]
    binding_id: str
    connection_ref: dict[str, Any]
    profile_id: str


@dataclass
class WireAuthProfileCleared:
    """`DELETE /auth/bindings/{binding_id}` /
`POST /auth/bindings/{binding_id}/logout` success body."""
    binding_id: str
    cleared: bool
    connection_ref: dict[str, Any]
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
    connection_ref: dict[str, Any]
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
    state: str
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
    connection_ref: dict[str, Any]
    has_refresh_token: bool
    profile_id: str
    provider: str
    realm_id: str
    state: str
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
WireToolResultContent = str | list[dict[str, Any]]

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
    to: Required[str]

class CommsCommandPeerLifecycle(TypedDict, total=False):
    kind: Required[Literal['peer_lifecycle']]
    lifecycle_kind: Required[Literal['mob.peer_added', 'mob.peer_retired', 'mob.peer_unwired']]
    params: NotRequired[Any]
    to: Required[str]

class CommsCommandPeerRequest(TypedDict, total=False):
    handling_mode: NotRequired[Literal['queue', 'steer']]
    intent: Required[str]
    kind: Required[Literal['peer_request']]
    params: NotRequired[Any]
    stream: NotRequired[Literal['none', 'reserve_interaction']]
    to: Required[str]

class CommsCommandPeerResponse(TypedDict, total=False):
    handling_mode: NotRequired[Literal['queue', 'steer']]
    in_reply_to: Required[str]
    kind: Required[Literal['peer_response']]
    result: NotRequired[Any]
    status: Required[Literal['accepted', 'completed', 'failed']]
    to: Required[str]

CommsCommandRequest = CommsCommandInput | CommsCommandPeerMessage | CommsCommandPeerLifecycle | CommsCommandPeerRequest | CommsCommandPeerResponse

# Response payload for `runtime/session_submission`.
InputStateResult = Optional[WireInputState]
