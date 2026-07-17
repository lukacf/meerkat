"""Meerkat client — spawns rkat-rpc and communicates via JSON-RPC.

Example::

    async with MeerkatClient() as client:
        session = await client.create_session("Hello!")
        print(session.text)

        result = await session.turn("Tell me more")
        print(result.text)

        async with session.stream("Explain in detail") as events:
            async for event in events:
                match event:
                    case TextDelta(delta=chunk):
                        print(chunk, end="")
            print()
"""

from __future__ import annotations

import asyncio
from dataclasses import fields, is_dataclass
import json
import logging
import os
import platform
import shutil
import tarfile
import tempfile
import warnings
import zipfile
from pathlib import Path
from typing import Any, Literal, NotRequired, TypedDict, cast
from urllib.error import URLError
import urllib.request

from .errors import CapabilityUnavailableError, MeerkatError
from .events import Usage, parse_event
from .generated.types import CONTRACT_VERSION
from .generated.version_compat import (
    is_compatible_with as _generated_is_compatible_with,
)
from .generated.types import (
    BridgeLiveControlOutcome,
    BridgeLiveControlVerb,
    CallbackToolDefinition,
    SkillListResponse,
    AttentionListRequest,
    AttentionListResult,
    CapabilitiesResponse,
    ConfigPatchParams,
    ConfigSetParams,
    ConfigWriteResult,
    CommsSendResult,
    InterruptResult,
    ServerCapabilities,
    SkillEntry,
    SkillKey as WireSkillKey,
    SkillSourceProvenance,
    WorkEventsResult,
    WorkItemsResult,
    GoalStatusRequest,
    GoalStatusResult,
    LiveCloseResult,
    LiveCloseStatus,
    LiveOpenResult,
    LiveRefreshResult,
    LiveRefreshStatus,
    LiveStatusResult,
    McpServerConfig,
    MobBindHostParams,
    MobBindHostResult,
    MobDefinitionInput,
    MobFlowRunResult,
    MobGrantScopesParams,
    MobGrantScopesResult,
    MobGrantsResult,
    MobHardCancelParams,
    MobHardCancelResult,
    MobHostStatus,
    MobHostsResult,
    MobIdParams,
    MobMemberParams,
    MobMemberHistoryParams,
    MobMemberHistoryResult,
    MobMemberLiveChannelParams,
    MobMemberLiveControlParams,
    MobMemberLiveOpenParams,
    MobMemberLiveStatusParams,
    MobMemberStatusResult,
    MobRevokeHostParams,
    MobRevokeHostResult,
    MobRevokeScopesParams,
    MobRevokeScopesResult,
    MobRouteInstallsResult,
    MobRunParams,
    MobRunResult,
    MobRunResultParams,
    MobSpawnManyParams,
    MobSpawnManyFailedResult,
    MobSpawnManyResult,
    MobSpawnManyResultEntry,
    MobSpawnManySpawnedResult,
    MobSpawnSpecParams,
    MobRotateSupervisorResult,
    MobConcludeObjectiveParams,
    MobConcludeObjectiveResult,
    MobTurnStartParams,
    MobWireMembersBatchEdge,
    MobWireMembersBatchResult,
    PublicTurnToolOverlay,
    RealtimeTurningMode,
    WireAuthBindingRef,
    WireContentInput,
    WireHistoryRow,
    WireHostBindPhase,
    WireHostBindingDescriptor,
    WireHostCapabilityFlags,
    WireLiveChannelCapabilities,
    WireMemberLaunchMode,
    WireMobBackendKind,
    WireMobProfile,
    WireMobRuntimeMode,
    WireRuntimeBinding,
    WireToolAccessPolicy,
    WireToolFilter,
    WireMemberProgressSnapshot,
    WireMemberHistoryPageBody,
    WireProjectionProvenance,
    WireReachability,
    WireRouteInstallObligation,
    ToolsRegisterParams,
    ToolsRegisterResult,
)
from .generated.types import (
    ApprovalDecideParams as RpcApprovalDecideParams,
    ApprovalGetParams as RpcApprovalGetParams,
    ApprovalListParams as RpcApprovalListParams,
    ApprovalListResult as RpcApprovalListResult,
    ApprovalRecord as RpcApprovalRecord,
    ApprovalRequestParams as RpcApprovalRequestParams,
    ArchiveSessionParams as RpcArchiveSessionParams,
    ArtifactDownloadParams as RpcArtifactDownloadParams,
    ArtifactDownloadResult as RpcArtifactDownloadResult,
    ArtifactIdParams as RpcArtifactIdParams,
    ArtifactListParams as RpcArtifactListParams,
    ArtifactListResult as RpcArtifactListResult,
    ArtifactRecord as RpcArtifactRecord,
    BindingIdParams as RpcBindingIdParams,
    BlobGetParams as RpcBlobGetParams,
    BlobPayload as RpcBlobPayload,
    CreateProfileParams as RpcCreateProfileParams,
    CreateScheduleRequest as RpcCreateScheduleRequest,
    DeferredCreateResult as RpcDeferredCreateResult,
    DeviceCompleteParams as RpcDeviceCompleteParams,
    DeviceStartParams as RpcDeviceStartParams,
    EventsLatestCursorParams as RpcEventsLatestCursorParams,
    EventsLatestCursorResult as RpcEventsLatestCursorResult,
    EventsListSinceParams as RpcEventsListSinceParams,
    EventsListSinceResult as RpcEventsListSinceResult,
    EventsSnapshotParams as RpcEventsSnapshotParams,
    EventsSnapshotResult as RpcEventsSnapshotResult,
    ForkSessionAtParams as RpcForkSessionAtParams,
    ForkSessionReplaceParams as RpcForkSessionReplaceParams,
    HelpRequest as RpcHelpRequest,
    HelpResponse as RpcHelpResponse,
    InjectSystemContextParams as RpcInjectSystemContextParams,
    InjectSystemContextResult as RpcInjectSystemContextResult,
    InterruptParams as RpcInterruptParams,
    ListSessionTranscriptRevisionsParams as RpcListSessionTranscriptRevisionsParams,
    SessionInputStateParams as RpcSessionInputStateParams,
    SessionInputStateResult as RpcSessionInputStateResult,
    ListSessionsParams as RpcListSessionsParams,
    ListSessionsResult as RpcListSessionsResult,
    LoginCompleteParams as RpcLoginCompleteParams,
    LoginStartParams as RpcLoginStartParams,
    ProvisionApiKeyParams as RpcProvisionApiKeyParams,
    ReadSessionHistoryParams as RpcReadSessionHistoryParams,
    ReadSessionParams as RpcReadSessionParams,
    ReadSessionTranscriptRevisionParams as RpcReadSessionTranscriptRevisionParams,
    RealmIdParams as RpcRealmIdParams,
    RestoreSessionTranscriptRevisionParams as RpcRestoreSessionTranscriptRevisionParams,
    RewriteSessionTranscriptParams as RpcRewriteSessionTranscriptParams,
    RuntimeHostCapabilities as RpcRuntimeHostCapabilities,
    RuntimeHostHealth as RpcRuntimeHostHealth,
    RuntimeHostInfo as RpcRuntimeHostInfo,
    Schedule as RpcSchedule,
    ScheduleToolCallParams as RpcScheduleToolCallParams,
    ScheduleToolsResult as RpcScheduleToolsResult,
    SessionExternalEventEnvelope as RpcSessionExternalEventEnvelope,
    SessionForkResult as RpcSessionForkResult,
    SessionPeerResponseTerminalParams as RpcSessionPeerResponseTerminalParams,
    SessionTranscriptRewriteResult as RpcSessionTranscriptRewriteResult,
    WireDeviceCompleteResult as RpcWireDeviceCompleteResult,
    WireProvisionApiKeyResult as RpcWireProvisionApiKeyResult,
    WireRunResult as RpcWireRunResult,
    WireSessionTranscriptRevision as RpcWireSessionTranscriptRevision,
    WireSessionTranscriptRevisionList as RpcWireSessionTranscriptRevisionList,
)
from .mob import (
    Mob,
    MobHelperResult,
    MobKickoffMemberSnapshot,
    MobLifecycleAction,
    MobMember,
    MobMemberRef,
    MobMemberSnapshot,
    MobPeerConnectivity,
    MobPeerConnectivitySnapshot,
    MobReadyMemberSnapshot,
    MobRespawnResult,
    MobSpawnResult,
    MobSpawnSpec,
    MobUnreachablePeer,
    MobWireMembersBatchEdgeInput,
    WorkOrigin,
)
from .session import DeferredSession, Session, _normalize_skill_ref
from .streaming import (
    RPC_STDOUT_LIMIT_BYTES,
    EventStream,
    EventSubscription,
    _StdoutDispatcher,
)
from .tools import ToolRegistry
from .types import (
    AttributedEvent,
    BlobPayload,
    Capability,
    ConfigEnvelope,
    DeletedMobProfile,
    ExtractionError,
    ExternalEventOutcome,
    ContentBlock,
    ContentInput,
    ModelsCatalogResponse,
    MobControlScope,
    MobEventsResult,
    MobGrantRecord,
    MobMemberLiveTransport,
    MobProfile,
    HelpExecutionMode,
    PeerCorrelationId,
    PeerId,
    ScheduleListResult,
    ScheduleOccurrenceRecord,
    ScheduleOccurrencesResult,
    ScheduleRecord,
    ScheduleToolsResult,
    ScheduleToolCall,
    EventEnvelope,
    EventSourceIdentity,
    McpLiveOpResponse,
    ResolvedModelCapabilities,
    RunResult,
    SchemaWarning,
    SessionDetails,
    SessionAssistantBlock,
    SessionContentBlock,
    SessionContentInput,
    SessionForkResult,
    SessionHistory,
    SessionTranscriptRevision,
    SessionTranscriptRevisionEntry,
    SessionTranscriptRevisionList,
    SessionTranscriptRewriteResult,
    SessionSummary,
    SessionMessage,
    SessionToolResult,
    SkillKey,
    SkillQuarantineDiagnostic,
    SkillRef,
    SkillRuntimeDiagnostics,
    SourceHealthSnapshot,
    StoredMobProfile,
    SystemPromptOverride,
    TranscriptEditRunningBehavior,
    TranscriptReplacement,
    TranscriptRewriteInputMessage,
    TranscriptRewriteReason,
    TranscriptRewriteSelection,
    TranscriptUserRole,
    ReadyWorkFilter,
    WorkGraphEvent,
    WorkGraphEventFilter,
    WorkGraphEventsResponse,
    WorkGraphIdParams,
    WorkGraphItemsResponse,
    WorkGraphSnapshot,
    WorkGraphSnapshotFilter,
    WorkItem,
    WorkItemFilter,
)

_logger = logging.getLogger(__name__)

_MEERKAT_REPO = ("lukacf", "meerkat")
_MEERKAT_BINARY = "rkat-rpc"
_MEERKAT_BINARY_CACHE_ROOT = (
    Path.home() / ".cache" / "meerkat" / "bin" / _MEERKAT_BINARY
)
_MOB_SPAWN_MANY_FAILURE_CAUSES = {
    "profile_not_found",
    "member_not_found",
    "member_already_exists",
    "not_externally_addressable",
    "invalid_transition",
    "wiring_error",
    "bridge_command_rejected",
    "member_restore_failed",
    "kickoff_wait_timed_out",
    "ready_wait_timed_out",
    "definition_error",
    "flow_not_found",
    "flow_failed",
    "run_not_found",
    "run_canceled",
    "flow_turn_timed_out",
    "frame_depth_limit_exceeded",
    "frame_atomic_persistence_unavailable",
    "spec_revision_conflict",
    "schema_validation",
    "insufficient_targets",
    "topology_violation",
    "bridge_delivery_rejected",
    "supervisor_escalation",
    "unsupported_for_mode",
    "missing_member_capability",
    "reset_barrier",
    "storage_error",
    "session_error",
    "comms_error",
    "callback_pending",
    "stale_fence_token",
    "stale_event_cursor",
    "work_not_found",
    "internal",
}

_MOB_CONTROL_SCOPES = frozenset(
    {
        "list",
        "read_history",
        "subscribe_events",
        "send_command",
        "cancel",
        "retire",
        "wire_topology",
        "live",
        "admin_host",
        "admin_grants",
    }
)

RenderClass = Literal[
    "user_prompt",
    "peer_message",
    "peer_request",
    "peer_response",
    "external_event",
    "flow_step",
    "continuation",
    "system_notice",
    "tool_scope_notice",
    "ops_progress",
]
RenderSalience = Literal["background", "normal", "important", "urgent"]
RenderMetadata = TypedDict(
    "RenderMetadata",
    {"class": RenderClass, "salience": NotRequired[RenderSalience]},
)


def _wire_value(value: Any) -> Any:
    if is_dataclass(value):
        converted: dict[str, Any] = {}
        for field in fields(value):
            item = getattr(value, field.name)
            if item is not None:
                converted[field.name] = _wire_value(item)
        return converted
    if isinstance(value, dict):
        return {key: _wire_value(item) for key, item in value.items()}
    if isinstance(value, list):
        return [_wire_value(item) for item in value]
    return value


def _wire_params(value: Any) -> dict[str, Any]:
    converted = _wire_value(value)
    return converted if isinstance(converted, dict) else dict(converted)


def _normalize_mob_wire_members_batch_edge(
    edge: MobWireMembersBatchEdgeInput,
) -> dict[str, str]:
    raw = _wire_value(edge)
    if isinstance(raw, dict):
        a = raw.get("a")
        b = raw.get("b")
    elif isinstance(raw, (list, tuple)) and len(raw) == 2:
        a, b = raw
    else:
        raise MeerkatError(
            "INVALID_ARGS",
            "mob/wire_members_batch edges must be (a, b) pairs or {'a', 'b'} objects",
        )
    if not isinstance(a, str) or not a or not isinstance(b, str) or not b:
        raise MeerkatError(
            "INVALID_ARGS",
            "mob/wire_members_batch edge endpoints must be non-empty strings",
        )
    return {"a": a, "b": b}


def _parse_mob_wire_members_batch_edge(
    raw: Any, context: str
) -> MobWireMembersBatchEdge:
    if not isinstance(raw, dict):
        raise MeerkatError("INVALID_RESPONSE", f"{context}: edge must be an object")
    a = raw.get("a")
    b = raw.get("b")
    if not isinstance(a, str) or not a or not isinstance(b, str) or not b:
        raise MeerkatError(
            "INVALID_RESPONSE",
            f"{context}: edge endpoints must be non-empty strings",
        )
    return MobWireMembersBatchEdge(a=a, b=b)


def _parse_mob_wire_members_batch_report(
    raw: dict[str, Any],
) -> MobWireMembersBatchResult:
    context = "Invalid mob/wire_members_batch response"
    requested = raw.get("requested")
    if not isinstance(requested, int) or isinstance(requested, bool) or requested < 0:
        raise MeerkatError(
            "INVALID_RESPONSE",
            f"{context}: requested must be a non-negative integer",
        )
    wired_raw = raw.get("wired")
    already_wired_raw = raw.get("already_wired")
    if not isinstance(wired_raw, list):
        raise MeerkatError("INVALID_RESPONSE", f"{context}: wired must be a list")
    if not isinstance(already_wired_raw, list):
        raise MeerkatError(
            "INVALID_RESPONSE",
            f"{context}: already_wired must be a list",
        )
    return MobWireMembersBatchResult(
        requested=requested,
        wired=[_parse_mob_wire_members_batch_edge(edge, context) for edge in wired_raw],
        already_wired=[
            _parse_mob_wire_members_batch_edge(edge, context)
            for edge in already_wired_raw
        ],
    )


def _skill_keys_to_wire(refs: list[SkillRef] | None) -> list[dict[str, str]] | None:
    """Convert SkillKey-like refs to the bare SkillKey wire format."""
    if refs is None:
        return None
    return [
        {
            "source_uuid": _normalize_skill_ref(r).source_uuid,
            "skill_name": _normalize_skill_ref(r).skill_name,
        }
        for r in refs
    ]


def _skill_refs_to_wire(refs: list[SkillRef] | None) -> list[dict[str, str]] | None:
    """Convert SkillRef values to the tagged wire format for skill_refs."""
    keys = _skill_keys_to_wire(refs)
    if keys is None:
        return None
    return [
        {
            "kind": "structured",
            **key,
        }
        for key in keys
    ]


class MeerkatClient:
    """Async client that manages a Meerkat agent runtime via rkat-rpc.

    Use as an async context manager for automatic cleanup::

        async with MeerkatClient() as client:
            session = await client.create_session("Hello!")

    Or manage the lifecycle explicitly::

        client = MeerkatClient()
        await client.connect()
        ...
        await client.close()
    """

    def __init__(self, rkat_path: str = "rkat-rpc"):
        self._rkat_path = rkat_path
        self._legacy_rpc_subcommand = False
        self._process: asyncio.subprocess.Process | None = None
        self._request_id = 0
        self._capabilities: list[Capability] | None = None
        self._methods: set[str] = set()
        self._dispatcher: _StdoutDispatcher | None = None
        self._tool_registry = ToolRegistry()
        self._tool_registration_errors: dict[str, MeerkatError] = {}

    # -- Tool registration -------------------------------------------------

    def tool(
        self,
        name: str,
        *,
        description: str = "",
        input_schema: dict[str, Any] | None = None,
    ) -> Any:
        """Decorator to register a callback tool handler.

        Example::

            @client.tool("search", description="Search the web")
            async def handle_search(arguments: dict) -> str:
                return f"Results for {arguments.get('q', '')}"
        """

        def decorator(fn: Any) -> Any:
            self._tool_registry.register(
                name,
                fn,
                description=description,
                input_schema=input_schema,
            )
            # If already connected, register the new tool with the server immediately.
            if self._process and self._process.stdin:
                asyncio.ensure_future(
                    self._register_tool_with_server(name, description, input_schema)
                )
            return fn

        return decorator

    async def _register_tool_with_server(
        self,
        name: str,
        description: str,
        input_schema: dict[str, Any] | None,
    ) -> None:
        """Register a single tool with the server post-connect.

        Benign shutdown/disconnect faults return quietly (the local registry
        still holds the definition and the next ``connect()`` re-sends it);
        genuine server rejection / protocol faults are logged, recorded as
        programmatically-distinguishable state, and re-raised.
        """
        try:
            params = ToolsRegisterParams(
                tools=[
                    CallbackToolDefinition(
                        name=name,
                        description=description or f"Tool: {name}",
                        input_schema=input_schema or {"type": "object"},
                    )
                ]
            )
            result: ToolsRegisterResult | dict[str, Any] = await self._request(
                "tools/register",
                _wire_value(params),
            )
            if isinstance(result, ToolsRegisterResult):  # pragma: no cover - typing aid
                _ = result.registered
            else:
                _ = result.get("registered")
        except MeerkatError as exc:
            # Clean shutdown / disconnect is genuinely benign — the registry
            # already holds the definition and a future connect() will re-send it.
            if exc.code in ("CLIENT_CLOSED", "CONNECTION_CLOSED", "NOT_CONNECTED"):
                return
            # Server rejection / protocol error is a real fault. We are on a
            # detached task (asyncio.ensure_future), so re-raising is invisible
            # to the sync decorator caller. Record the typed fault as
            # caller-inspectable state so the silent-never-registers failure is
            # programmatically distinguishable, then log and re-raise.
            self._tool_registration_errors[name] = exc
            _logger.error(
                "post-connect tool registration failed for %r: [%s] %s",
                name,
                exc.code,
                exc.message,
            )
            raise
        # Success: clear any stale fault recorded by a prior failed attempt so a
        # later re-registration that succeeds no longer reports an error.
        self._tool_registration_errors.pop(name, None)

    # -- Async context manager ---------------------------------------------

    async def __aenter__(self) -> MeerkatClient:
        await self.connect()
        return self

    async def __aexit__(self, *_exc: object) -> None:
        await self.close()

    # -- Connection --------------------------------------------------------

    async def connect(
        self,
        *,
        realm_id: str | None = None,
        instance_id: str | None = None,
        realm_backend: str | None = None,
        isolated: bool = False,
        state_root: str | None = None,
        context_root: str | None = None,
        user_config_root: str | None = None,
        live_ws: bool = False,
        live_webrtc: bool = False,
    ) -> MeerkatClient:
        """Start the rkat-rpc subprocess and perform handshake."""
        if realm_id and isolated:
            raise MeerkatError(
                "INVALID_ARGS", "realm_id and isolated cannot both be set"
            )

        resolved_path, use_legacy = await self._resolve_rkat_binary(self._rkat_path)
        self._rkat_path = resolved_path
        self._legacy_rpc_subcommand = use_legacy

        has_advanced_opts = any(
            [
                isolated,
                realm_id,
                instance_id,
                realm_backend,
                state_root,
                context_root,
                user_config_root,
                live_ws,
                live_webrtc,
            ]
        )
        if self._legacy_rpc_subcommand and has_advanced_opts:
            raise MeerkatError(
                "LEGACY_BINARY_UNSUPPORTED",
                "Realm/context options require the standalone rkat-rpc binary. "
                "Install rkat-rpc and retry.",
            )

        args = self._build_args(
            use_legacy,
            isolated=isolated,
            realm_id=realm_id,
            instance_id=instance_id,
            realm_backend=realm_backend,
            state_root=state_root,
            context_root=context_root,
            user_config_root=user_config_root,
            live_ws=live_ws,
            live_webrtc=live_webrtc,
        )

        self._process = await asyncio.create_subprocess_exec(
            self._rkat_path,
            *args,
            stdin=asyncio.subprocess.PIPE,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
            limit=RPC_STDOUT_LIMIT_BYTES,
        )
        assert self._process.stdout is not None
        self._dispatcher = _StdoutDispatcher(self._process.stdout)
        if self._process.stdin:
            self._dispatcher.set_stdin_writer(self._process.stdin)
        self._dispatcher.start()

        # `initialize` returns the generated `ServerCapabilities` contract.
        result: ServerCapabilities | dict[str, Any] = await self._request(
            "initialize", {}
        )
        if isinstance(result, ServerCapabilities):  # pragma: no cover - typing aid
            result = result.__dict__
        server_version = result.get("contract_version", "")
        if not self._check_version_compatible(server_version, CONTRACT_VERSION):
            raise MeerkatError(
                "VERSION_MISMATCH",
                f"Server version {server_version} incompatible with SDK {CONTRACT_VERSION}",
            )
        self._methods = {str(method) for method in result.get("methods", [])}

        # `capabilities/get` returns the generated `CapabilitiesResponse`
        # contract; entries are validated field-wise below.
        caps_result: CapabilitiesResponse | dict[str, Any] = await self._request(
            "capabilities/get", {}
        )
        if isinstance(
            caps_result, CapabilitiesResponse
        ):  # pragma: no cover - typing aid
            caps_result = caps_result.__dict__
        context = "Invalid capabilities/get response"
        capabilities = self._require_present_list_field(
            caps_result, "capabilities", context
        )
        self._capabilities = []
        for c in capabilities:
            entry = self._require_dict(c, "capability", context)
            self._capabilities.append(
                Capability(
                    id=self._require_string_field(entry, "id", context),
                    description=self._require_string_field(
                        entry, "description", context
                    ),
                    status=self._parse_wire_capability_status(
                        entry.get("status"),
                        context,
                    ),
                )
            )

        # Register callback tools with the server if any were declared.
        if self._tool_registry:
            tools = [
                CallbackToolDefinition(
                    name=tool["name"],
                    description=tool["description"],
                    input_schema=tool["input_schema"],
                )
                for tool in self._tool_registry.definitions()
            ]
            params = ToolsRegisterParams(tools=tools)
            result: ToolsRegisterResult | dict[str, Any] = await self._request(
                "tools/register",
                _wire_value(params),
            )
            if isinstance(result, ToolsRegisterResult):  # pragma: no cover - typing aid
                _ = result.registered
            else:
                _ = result.get("registered")

        # Always install the tool handler so post-connect @client.tool()
        # registrations can serve callback requests.
        if self._dispatcher:
            self._dispatcher.set_tool_handler(self._tool_registry)

        return self

    async def close(self) -> None:
        """Terminate the rkat-rpc subprocess and release resources."""
        if self._dispatcher:
            await self._dispatcher.stop()
            self._dispatcher = None
        if self._process:
            if self._process.stdin:
                self._process.stdin.close()
            self._process.terminate()
            try:
                await asyncio.wait_for(self._process.wait(), timeout=5)
            except asyncio.TimeoutError:
                self._process.kill()
            self._process = None

    # -- Auth (Phase 4c.11 wrapper over auth.* RPC methods) ---------------

    async def list_realms(self) -> list[dict[str, Any]]:
        """List realms in the active rkat config. Delegates to
        `realm/list`. Returns a list of `{realm_id, default_binding,
        backend_count, auth_profile_count, binding_count}`."""
        result = await self._request("realm/list", {})
        context = "Invalid realm/list response"
        payload = self._require_dict(result, "result", context)
        realms = self._require_present_list_field(payload, "realms", context)
        parsed: list[dict[str, Any]] = []
        for index, raw in enumerate(realms):
            realm_context = f"{context}: realms[{index}]"
            realm = self._require_dict(raw, f"realms[{index}]", context)
            self._require_string_field(realm, "realm_id", realm_context)
            self._optional_string_field(realm, "default_binding", realm_context)
            self._require_non_negative_integer_field(
                realm, "backend_count", realm_context
            )
            self._require_non_negative_integer_field(
                realm, "auth_profile_count", realm_context
            )
            self._require_non_negative_integer_field(
                realm, "binding_count", realm_context
            )
            parsed.append(dict(realm))
        return parsed

    async def get_realm(self, realm_id: str) -> dict[str, Any]:
        """Fetch one realm's full WireRealmConnectionSet. Delegates to
        `realm/get`."""
        _rpc_signature: RpcRealmIdParams
        return await self._request("realm/get", {"realm_id": realm_id})

    async def list_auth_profiles(self, realm_id: str) -> dict[str, Any]:
        """List auth profiles / backend profiles / bindings for one
        realm. Delegates to `auth/profile/list`."""
        _rpc_signature: RpcRealmIdParams
        return await self._request("auth/profile/list", {"realm_id": realm_id})

    async def get_auth_profile(
        self, realm_id: str, binding_id: str, profile_id: str | None = None
    ) -> dict[str, Any]:
        """Fetch a binding-scoped auth profile via `auth/profile/get`."""
        _rpc_signature: RpcBindingIdParams
        params: dict[str, Any] = {"realm_id": realm_id, "binding_id": binding_id}
        if profile_id is not None:
            params["profile_id"] = profile_id
        return await self._request("auth/profile/get", params)

    async def create_auth_profile(self, params: dict[str, Any]) -> dict[str, Any]:
        """Create binding-scoped credentials via `auth/profile/create`."""
        _rpc_signature: RpcCreateProfileParams
        return await self._request("auth/profile/create", params)

    async def delete_auth_profile(
        self, realm_id: str, binding_id: str, profile_id: str | None = None
    ) -> None:
        """Clear a profile's persisted credentials via
        `auth/profile/delete`."""
        _rpc_signature: RpcBindingIdParams
        params: dict[str, Any] = {"realm_id": realm_id, "binding_id": binding_id}
        if profile_id is not None:
            params["profile_id"] = profile_id
        await self._request("auth/profile/delete", params)

    async def auth_login_start(
        self, provider: str, redirect_uri: str = "http://127.0.0.1:0/callback"
    ) -> dict[str, Any]:
        """Start an OAuth authorization-code login via
        `auth/login/start`. Returns `{authorize_url, state}`; client directs
        user to the URL then calls `auth_login_complete` once the redirect
        carries a code."""
        _rpc_signature: RpcLoginStartParams
        return await self._request(
            "auth/login/start",
            {"provider": provider, "redirect_uri": redirect_uri},
        )

    async def auth_login_complete(
        self,
        provider: str,
        code: str,
        state: str,
        *,
        realm_id: str,
        binding_id: str,
        profile_id: str | None = None,
        redirect_uri: str = "http://127.0.0.1:0/callback",
    ) -> dict[str, Any]:
        """Exchange an authorization code for tokens via
        `auth/login/complete`. Tokens land in the server-side credential
        source for the resolved binding."""
        _rpc_signature: RpcLoginCompleteParams
        params: dict[str, Any] = {
            "provider": provider,
            "code": code,
            "state": state,
            "realm_id": realm_id,
            "binding_id": binding_id,
            "redirect_uri": redirect_uri,
        }
        if profile_id is not None:
            params["profile_id"] = profile_id
        return await self._request("auth/login/complete", params)

    async def auth_login_device_start(self, provider: str) -> dict[str, Any]:
        """Start an OAuth device-code flow via
        `auth/login/device_start`. Returns the user_code to display +
        verification_uri + interval."""
        _rpc_signature: RpcDeviceStartParams
        return await self._request("auth/login/device_start", {"provider": provider})

    async def auth_login_device_complete(
        self,
        provider: str,
        device_code: str,
        *,
        realm_id: str,
        binding_id: str,
        profile_id: str | None = None,
    ) -> dict[str, Any]:
        """Poll once for device-code completion via
        `auth/login/device_complete`. Returns `{state: "pending" |
        "slow_down" | "access_denied" | "expired" | "ready", ...}`."""
        _rpc_signature: RpcDeviceCompleteParams | RpcWireDeviceCompleteResult
        params: dict[str, Any] = {
            "provider": provider,
            "device_code": device_code,
            "realm_id": realm_id,
            "binding_id": binding_id,
        }
        if profile_id is not None:
            params["profile_id"] = profile_id
        return await self._request("auth/login/device_complete", params)

    async def auth_provision_api_key(
        self,
        access_token: str,
        *,
        realm_id: str,
        binding_id: str,
        profile_id: str | None = None,
    ) -> dict[str, Any]:
        """Anthropic Console-OAuth → API key provisioning via
        `auth/login/provision_api_key` (plan §4b.5). The caller runs
        the Console-scope OAuth flow first; hands the resulting
        access_token here; the server POSTs to Anthropic's
        create_api_key endpoint and persists the returned key."""
        _rpc_signature: RpcProvisionApiKeyParams | RpcWireProvisionApiKeyResult
        params: dict[str, Any] = {
            "access_token": access_token,
            "realm_id": realm_id,
            "binding_id": binding_id,
        }
        if profile_id is not None:
            params["profile_id"] = profile_id
        return await self._request("auth/login/provision_api_key", params)

    async def auth_status(
        self, realm_id: str, binding_id: str, profile_id: str | None = None
    ) -> dict[str, Any]:
        """Report persisted-credential status for a binding via
        `auth/status/get`."""
        _rpc_signature: RpcBindingIdParams
        params: dict[str, Any] = {"realm_id": realm_id, "binding_id": binding_id}
        if profile_id is not None:
            params["profile_id"] = profile_id
        return await self._request("auth/status/get", params)

    async def auth_logout(
        self, realm_id: str, binding_id: str, profile_id: str | None = None
    ) -> dict[str, Any]:
        """Revoke + delete a binding's persisted credentials via
        `auth/logout`."""
        _rpc_signature: RpcBindingIdParams
        params: dict[str, Any] = {"realm_id": realm_id, "binding_id": binding_id}
        if profile_id is not None:
            params["profile_id"] = profile_id
        return await self._request("auth/logout", params)

    # -- Session lifecycle -------------------------------------------------

    async def create_session(
        self,
        prompt: str | list[ContentBlock],
        *,
        injected_context: list[str | list[ContentBlock]] | None = None,
        model: str | None = None,
        provider: str | None = None,
        auth_binding: dict[str, str] | None = None,
        system_prompt: SystemPromptOverride | None = None,
        max_tokens: int | None = None,
        output_schema: dict[str, Any] | None = None,
        structured_output_retries: int | None = None,
        hooks_override: dict[str, Any] | None = None,
        enable_builtins: bool | None = None,
        enable_shell: bool | None = None,
        enable_memory: bool | None = None,
        enable_schedule: bool | None = None,
        enable_workgraph: bool | None = None,
        enable_mob: bool | None = None,
        enable_web_search: bool | None = None,
        keep_alive: bool | None = None,
        comms_name: str | None = None,
        peer_meta: dict[str, Any] | None = None,
        budget_limits: dict[str, Any] | None = None,
        provider_params: dict[str, Any] | None = None,
        preload_skills: list[SkillRef] | None = None,
        skill_refs: list[SkillRef] | None = None,
        labels: dict[str, str] | None = None,
        additional_instructions: list[str] | None = None,
        app_context: dict[str, Any] | None = None,
        shell_env: dict[str, str] | None = None,
        external_tools: list[dict[str, Any]] | None = None,
    ) -> Session:
        """Create a new session, run the first turn, and return a :class:`Session`.

        Example::

            session = await client.create_session(
                "Summarise this project",
                model="claude-sonnet-4-5",
            )
            print(session.text)
        """
        _rpc_signature: RpcDeferredCreateResult | RpcWireRunResult
        params = self._build_create_params(
            prompt,
            injected_context=injected_context,
            model=model,
            provider=provider,
            auth_binding=auth_binding,
            system_prompt=system_prompt,
            max_tokens=max_tokens,
            output_schema=output_schema,
            structured_output_retries=structured_output_retries,
            hooks_override=hooks_override,
            enable_builtins=enable_builtins,
            enable_shell=enable_shell,
            enable_memory=enable_memory,
            enable_schedule=enable_schedule,
            enable_workgraph=enable_workgraph,
            enable_mob=enable_mob,
            enable_web_search=enable_web_search,
            keep_alive=keep_alive,
            comms_name=comms_name,
            peer_meta=peer_meta,
            budget_limits=budget_limits,
            provider_params=provider_params,
            preload_skills=preload_skills,
            skill_refs=skill_refs,
            labels=labels,
            additional_instructions=additional_instructions,
            app_context=app_context,
            shell_env=shell_env,
            external_tools=external_tools,
        )
        raw = await self._request("session/create", params)
        result = self._parse_run_result(raw)
        return Session(self, result)

    def create_session_streaming(
        self,
        prompt: str | list[ContentBlock],
        *,
        injected_context: list[str | list[ContentBlock]] | None = None,
        model: str | None = None,
        provider: str | None = None,
        auth_binding: dict[str, str] | None = None,
        system_prompt: SystemPromptOverride | None = None,
        max_tokens: int | None = None,
        output_schema: dict[str, Any] | None = None,
        structured_output_retries: int | None = None,
        hooks_override: dict[str, Any] | None = None,
        enable_builtins: bool | None = None,
        enable_shell: bool | None = None,
        enable_memory: bool | None = None,
        enable_schedule: bool | None = None,
        enable_workgraph: bool | None = None,
        enable_mob: bool | None = None,
        enable_web_search: bool | None = None,
        keep_alive: bool | None = None,
        comms_name: str | None = None,
        peer_meta: dict[str, Any] | None = None,
        budget_limits: dict[str, Any] | None = None,
        provider_params: dict[str, Any] | None = None,
        preload_skills: list[SkillRef] | None = None,
        skill_refs: list[SkillRef] | None = None,
        labels: dict[str, str] | None = None,
        additional_instructions: list[str] | None = None,
        app_context: dict[str, Any] | None = None,
        shell_env: dict[str, str] | None = None,
        external_tools: list[dict[str, Any]] | None = None,
    ) -> EventStream:
        """Create a new session and stream typed events from the first turn.

        Returns an :class:`EventStream` async context manager::

            async with client.create_session_streaming("Hello!") as stream:
                async for event in stream:
                    match event:
                        case TextDelta(delta=chunk):
                            print(chunk, end="", flush=True)
                session_id = stream.session_id
                result = stream.result
        """
        _rpc_signature: RpcDeferredCreateResult | RpcWireRunResult
        if not self._dispatcher or not self._process or not self._process.stdin:
            raise MeerkatError("NOT_CONNECTED", "Client not connected")
        params = self._build_create_params(
            prompt,
            injected_context=injected_context,
            model=model,
            provider=provider,
            auth_binding=auth_binding,
            system_prompt=system_prompt,
            max_tokens=max_tokens,
            output_schema=output_schema,
            structured_output_retries=structured_output_retries,
            hooks_override=hooks_override,
            enable_builtins=enable_builtins,
            enable_shell=enable_shell,
            enable_memory=enable_memory,
            enable_schedule=enable_schedule,
            enable_workgraph=enable_workgraph,
            enable_mob=enable_mob,
            enable_web_search=enable_web_search,
            keep_alive=keep_alive,
            comms_name=comms_name,
            peer_meta=peer_meta,
            budget_limits=budget_limits,
            provider_params=provider_params,
            preload_skills=preload_skills,
            skill_refs=skill_refs,
            labels=labels,
            additional_instructions=additional_instructions,
            app_context=app_context,
            shell_env=shell_env,
            external_tools=external_tools,
        )
        self._request_id += 1
        request_id = self._request_id
        event_queue = self._dispatcher.subscribe_pending_stream(request_id)
        response_future = self._dispatcher.expect_response(request_id)
        request = {
            "jsonrpc": "2.0",
            "id": request_id,
            "method": "session/create",
            "params": params,
        }
        data = (json.dumps(request) + "\n").encode()
        return EventStream(
            session_id="",
            event_queue=event_queue,
            response_future=response_future,
            dispatcher=self._dispatcher,
            parse_result=self._parse_run_result,
            pending_send=(self._process.stdin, data),
            pending_request_id=request_id,
        )

    async def create_deferred_session(
        self,
        prompt: str | list[ContentBlock],
        *,
        injected_context: list[str | list[ContentBlock]] | None = None,
        model: str | None = None,
        provider: str | None = None,
        auth_binding: dict[str, str] | None = None,
        system_prompt: SystemPromptOverride | None = None,
        max_tokens: int | None = None,
        output_schema: dict[str, Any] | None = None,
        structured_output_retries: int | None = None,
        hooks_override: dict[str, Any] | None = None,
        enable_builtins: bool | None = None,
        enable_shell: bool | None = None,
        enable_memory: bool | None = None,
        enable_schedule: bool | None = None,
        enable_workgraph: bool | None = None,
        enable_mob: bool | None = None,
        enable_web_search: bool | None = None,
        keep_alive: bool | None = None,
        comms_name: str | None = None,
        peer_meta: dict[str, Any] | None = None,
        budget_limits: dict[str, Any] | None = None,
        provider_params: dict[str, Any] | None = None,
        preload_skills: list[SkillRef] | None = None,
        skill_refs: list[SkillRef] | None = None,
        labels: dict[str, str] | None = None,
        additional_instructions: list[str] | None = None,
        app_context: dict[str, Any] | None = None,
        shell_env: dict[str, str] | None = None,
        external_tools: list[dict[str, Any]] | None = None,
    ) -> DeferredSession:
        """Create a new session without running the first turn.

        Returns a :class:`~meerkat.DeferredSession` whose
        :meth:`~meerkat.DeferredSession.start_turn` executes the first turn
        with optional per-turn overrides.

        Example::

            deferred = await client.create_deferred_session(
                "Summarise this project",
                model="claude-sonnet-4-5",
            )
            result = await deferred.start_turn("Begin")
        """
        _rpc_signature: RpcDeferredCreateResult | RpcWireRunResult
        params = self._build_create_params(
            prompt,
            injected_context=injected_context,
            model=model,
            provider=provider,
            auth_binding=auth_binding,
            system_prompt=system_prompt,
            max_tokens=max_tokens,
            output_schema=output_schema,
            structured_output_retries=structured_output_retries,
            hooks_override=hooks_override,
            enable_builtins=enable_builtins,
            enable_shell=enable_shell,
            enable_memory=enable_memory,
            enable_schedule=enable_schedule,
            enable_workgraph=enable_workgraph,
            enable_mob=enable_mob,
            enable_web_search=enable_web_search,
            keep_alive=keep_alive,
            comms_name=comms_name,
            peer_meta=peer_meta,
            budget_limits=budget_limits,
            provider_params=provider_params,
            preload_skills=preload_skills,
            skill_refs=skill_refs,
            labels=labels,
            additional_instructions=additional_instructions,
            app_context=app_context,
            shell_env=shell_env,
            external_tools=external_tools,
        )
        params["initial_turn"] = "deferred"
        raw = await self._request("session/create", params)
        context = "Invalid session/create deferred response"
        return DeferredSession(
            self,
            session_id=self._require_string_field(raw, "session_id", context),
            session_ref=self._optional_string_field(raw, "session_ref", context),
        )

    async def ask_help(
        self,
        question: str,
        *,
        prompt: str | None = None,
        execution_mode: HelpExecutionMode = "explain_only",
        model: str | None = None,
        provider: str | None = None,
        max_tokens: int | None = None,
    ) -> RunResult:
        """Ask Meerkat usage help through the dedicated ``help/ask`` RPC."""
        _rpc_signature: RpcHelpRequest | RpcHelpResponse
        params: dict[str, Any] = {
            "question": question,
            "execution_mode": execution_mode,
        }
        if prompt is not None:
            params["prompt"] = prompt
        if model is not None:
            params["model"] = model
        if provider is not None:
            params["provider"] = provider
        if max_tokens is not None:
            params["max_tokens"] = max_tokens
        raw = await self._request("help/ask", params)
        return self._parse_run_result(raw)

    # -- Session queries ---------------------------------------------------

    async def list_sessions(
        self,
        *,
        labels: dict[str, str] | None = None,
        limit: int | None = None,
        offset: int | None = None,
    ) -> list[SessionSummary]:
        """List active sessions."""
        _rpc_signature: RpcListSessionsParams | RpcListSessionsResult
        params: dict[str, Any] = {}
        if labels is not None:
            params["labels"] = labels
        if limit is not None:
            params["limit"] = limit
        if offset is not None:
            params["offset"] = offset
        result = await self._request("session/list", params)
        sessions = self._require_present_list_field(
            result, "sessions", "Invalid session/list response"
        )
        return [
            self._parse_session_summary(
                self._require_dict(
                    s, f"sessions[{idx}]", "Invalid session/list response"
                )
            )
            for idx, s in enumerate(sessions)
        ]

    async def read_session(self, session_id: str) -> SessionDetails:
        """Read detailed session state."""
        _rpc_signature: RpcReadSessionParams
        result = await self._request("session/read", {"session_id": session_id})
        return self._parse_session_details(result)

    async def send_external_event(
        self,
        session_id: str,
        event_type: str,
        payload: Any,
        *,
        blocks: list[ContentBlock] | None = None,
    ) -> ExternalEventOutcome:
        """Append a durable external event input to a session runtime."""
        _rpc_signature: RpcSessionExternalEventEnvelope
        params: dict[str, Any] = {
            "session_id": session_id,
            "kind": "generic_json",
            "event_type": event_type,
            "payload": payload,
        }
        if blocks is not None:
            params["blocks"] = blocks
        return await self._request("session/external_event", params)

    async def send_peer_response_terminal(
        self,
        session_id: str,
        peer_id: PeerId,
        request_id: PeerCorrelationId,
        status: Literal["completed", "failed", "cancelled"],
        result: Any,
        *,
        display_name: str | None = None,
    ) -> ExternalEventOutcome:
        """Admit a correlated terminal peer response through the typed ingress."""
        _rpc_signature: RpcSessionPeerResponseTerminalParams
        params: dict[str, Any] = {
            "session_id": session_id,
            "peer_id": str(peer_id),
            "request_id": str(request_id),
            "status": status,
            "result": result,
        }
        if display_name is not None:
            params["display_name"] = display_name
        return await self._request("session/peer_response_terminal", params)

    async def inject_context(
        self,
        session_id: str,
        text: str,
        *,
        source: str | None = None,
        idempotency_key: str | None = None,
    ) -> dict[str, Any]:
        """Inject system context into a session."""
        _rpc_signature: RpcInjectSystemContextParams | RpcInjectSystemContextResult
        params = {
            "session_id": session_id,
            "content": {"type": "text", "text": text},
        }
        if source:
            params["source"] = source
        if idempotency_key:
            params["idempotency_key"] = idempotency_key
        return await self._request("session/inject_context", params)

    async def input_state(
        self,
        session_id: str,
        *,
        input_id: str | None = None,
        idempotency_key: str | None = None,
    ) -> dict[str, Any] | None:
        """Read an input's stored runtime state by input id or idempotency key.

        Returns ``None`` when no input matches the selector — the durable
        reconciliation query for interrupted work after a host restart
        (terminal outcome, resolving run id, boundary sequence).
        """
        _rpc_signature: RpcSessionInputStateParams | RpcSessionInputStateResult
        if (input_id is None) == (idempotency_key is None):
            raise ValueError(
                "exactly one of input_id or idempotency_key must be provided"
            )
        params: dict[str, Any] = {"session_id": session_id}
        if input_id is not None:
            params["selector"] = {"by": "input_id", "value": input_id}
        else:
            params["selector"] = {"by": "idempotency_key", "value": idempotency_key}
        result = await self._request("session/input_status", params)
        state = result.get("state")
        return state if isinstance(state, dict) else None

    async def read_session_history(
        self,
        session_id: str,
        *,
        offset: int = 0,
        limit: int | None = None,
    ) -> SessionHistory:
        """Read paginated session transcript history."""
        _rpc_signature: RpcReadSessionHistoryParams
        params: dict[str, Any] = {"session_id": session_id, "offset": offset}
        if limit is not None:
            params["limit"] = limit
        raw = await self._request("session/history", params)
        return self._parse_session_history(raw)

    async def read_session_transcript_revision(
        self,
        session_id: str,
        revision: str,
        *,
        offset: int = 0,
        limit: int | None = None,
    ) -> SessionTranscriptRevision:
        """Read paginated transcript history for a retained revision."""
        _rpc_signature: (
            RpcReadSessionTranscriptRevisionParams | RpcWireSessionTranscriptRevision
        )
        params: dict[str, Any] = {
            "session_id": session_id,
            "revision": revision,
            "offset": offset,
        }
        if limit is not None:
            params["limit"] = limit
        raw = await self._request("session/transcript_revision", params)
        return self._parse_session_transcript_revision(raw)

    async def list_session_transcript_revisions(
        self,
        session_id: str,
        *,
        offset: int | None = None,
        limit: int | None = None,
    ) -> SessionTranscriptRevisionList:
        """List retained transcript revision commits with the current head."""
        _rpc_signature: (
            RpcListSessionTranscriptRevisionsParams
            | RpcWireSessionTranscriptRevisionList
        )
        params: dict[str, Any] = {"session_id": session_id}
        if offset is not None:
            params["offset"] = offset
        if limit is not None:
            params["limit"] = limit
        raw = await self._request("session/transcript_revisions", params)
        return self._parse_session_transcript_revision_list(raw)

    async def fork_session_at(
        self,
        session_id: str,
        message_index: int,
        *,
        running_behavior: TranscriptEditRunningBehavior | None = None,
        tool_access_policy: WireToolAccessPolicy | dict[str, Any] | None = None,
    ) -> SessionForkResult:
        """Fork an idle session at a transcript message index."""
        _rpc_signature: RpcForkSessionAtParams | RpcSessionForkResult
        params: dict[str, Any] = {
            "session_id": session_id,
            "message_index": message_index,
        }
        if running_behavior is not None:
            params["running_behavior"] = running_behavior
        if tool_access_policy is not None:
            params["tool_access_policy"] = _wire_value(tool_access_policy)
        raw = await self._request("session/fork_at", params)
        return self._parse_session_fork_result(raw)

    async def fork_session_replace(
        self,
        session_id: str,
        message_index: int,
        replacement: TranscriptReplacement,
        *,
        running_behavior: TranscriptEditRunningBehavior | None = None,
        tool_access_policy: WireToolAccessPolicy | dict[str, Any] | None = None,
    ) -> SessionForkResult:
        """Fork an idle session and apply a typed transcript replacement."""
        _rpc_signature: RpcForkSessionReplaceParams | RpcSessionForkResult
        params: dict[str, Any] = {
            "session_id": session_id,
            "message_index": message_index,
            "replacement": replacement,
        }
        if running_behavior is not None:
            params["running_behavior"] = running_behavior
        if tool_access_policy is not None:
            params["tool_access_policy"] = _wire_value(tool_access_policy)
        raw = await self._request("session/fork_replace", params)
        return self._parse_session_fork_result(raw)

    async def rewrite_session_transcript(
        self,
        session_id: str,
        selection: TranscriptRewriteSelection,
        replacement: list[TranscriptRewriteInputMessage],
        reason: TranscriptRewriteReason,
        *,
        actor: str | None = None,
        expected_parent_revision: str | None = None,
        running_behavior: TranscriptEditRunningBehavior | None = None,
    ) -> SessionTranscriptRewriteResult:
        """Commit a typed same-session transcript rewrite."""
        _rpc_signature: (
            RpcRewriteSessionTranscriptParams | RpcSessionTranscriptRewriteResult
        )
        params: dict[str, Any] = {
            "session_id": session_id,
            "selection": selection,
            "replacement": [
                self._serialize_transcript_rewrite_message(message)
                for message in replacement
            ],
            "reason": reason,
        }
        if actor is not None:
            params["actor"] = actor
        if expected_parent_revision is not None:
            params["expected_parent_revision"] = expected_parent_revision
        if running_behavior is not None:
            params["running_behavior"] = running_behavior
        raw = await self._request("session/rewrite_transcript", params)
        return self._parse_session_transcript_rewrite_result(raw)

    async def restore_session_transcript_revision(
        self,
        session_id: str,
        revision: str,
        reason: TranscriptRewriteReason,
        *,
        actor: str | None = None,
        expected_parent_revision: str | None = None,
        running_behavior: TranscriptEditRunningBehavior | None = None,
    ) -> SessionTranscriptRewriteResult:
        """Commit a typed rewrite that restores a retained transcript revision."""
        _rpc_signature: (
            RpcRestoreSessionTranscriptRevisionParams
            | RpcSessionTranscriptRewriteResult
        )
        params: dict[str, Any] = {
            "session_id": session_id,
            "revision": revision,
            "reason": reason,
        }
        if actor is not None:
            params["actor"] = actor
        if expected_parent_revision is not None:
            params["expected_parent_revision"] = expected_parent_revision
        if running_behavior is not None:
            params["running_behavior"] = running_behavior
        raw = await self._request("session/restore_transcript_revision", params)
        return self._parse_session_transcript_rewrite_result(raw)

    async def get_blob(self, blob_id: str) -> BlobPayload:
        _rpc_signature: RpcBlobGetParams | RpcBlobPayload
        raw = await self._request("blob/get", {"blob_id": blob_id})
        context = "Invalid blob/get response"
        return BlobPayload(
            blob_id=self._require_string_field(raw, "blob_id", context),
            media_type=self._require_string_field(raw, "media_type", context),
            data_base64=self._require_string_field(raw, "data", context),
        )

    async def get_runtime_host_info(self) -> dict[str, Any]:
        _rpc_signature: RpcRuntimeHostInfo
        return await self._request("runtime/host_info", {})

    async def get_runtime_host_capabilities(self) -> dict[str, Any]:
        _rpc_signature: RpcRuntimeHostCapabilities
        return await self._request("runtime/capabilities", {})

    async def get_runtime_host_health(self) -> dict[str, Any]:
        _rpc_signature: RpcRuntimeHostHealth
        return await self._request("runtime/health", {})

    async def request_approval(self, params: dict[str, Any]) -> dict[str, Any]:
        _rpc_signature: RpcApprovalRecord | RpcApprovalRequestParams
        return await self._request("approval/request", params)

    async def list_approvals(
        self, params: dict[str, Any] | None = None
    ) -> dict[str, Any]:
        _rpc_signature: RpcApprovalListParams | RpcApprovalListResult
        return await self._request("approval/list", params or {})

    async def get_approval(self, params: dict[str, Any]) -> dict[str, Any]:
        _rpc_signature: RpcApprovalGetParams | RpcApprovalRecord
        return await self._request("approval/get", params)

    async def decide_approval(self, params: dict[str, Any]) -> dict[str, Any]:
        _rpc_signature: RpcApprovalDecideParams | RpcApprovalRecord
        return await self._request("approval/decide", params)

    async def list_artifacts(
        self, params: dict[str, Any] | None = None
    ) -> dict[str, Any]:
        _rpc_signature: RpcArtifactListParams | RpcArtifactListResult
        return await self._request("artifact/list", params or {})

    async def get_artifact(self, params: dict[str, Any]) -> dict[str, Any]:
        _rpc_signature: RpcArtifactIdParams | RpcArtifactRecord
        return await self._request("artifact/get", params)

    async def download_artifact(self, params: dict[str, Any]) -> dict[str, Any]:
        _rpc_signature: RpcArtifactDownloadParams | RpcArtifactDownloadResult
        return await self._request("artifact/download", params)

    async def latest_event_cursor(self, params: dict[str, Any]) -> dict[str, Any]:
        _rpc_signature: RpcEventsLatestCursorParams | RpcEventsLatestCursorResult
        return await self._request("events/latest_cursor", params)

    async def list_events_since(self, params: dict[str, Any]) -> dict[str, Any]:
        _rpc_signature: RpcEventsListSinceParams | RpcEventsListSinceResult
        return await self._request("events/list_since", params)

    async def event_snapshot(self, params: dict[str, Any]) -> dict[str, Any]:
        _rpc_signature: RpcEventsSnapshotParams | RpcEventsSnapshotResult
        return await self._request("events/snapshot", params)

    # -- Capabilities ------------------------------------------------------

    @property
    def capabilities(self) -> list[Capability]:
        """Runtime capabilities discovered during handshake."""
        return self._capabilities or []

    def has_capability(self, capability_id: str) -> bool:
        """Check if a capability is available."""
        if capability_id == "mob":
            return bool({"mob/create", "mob/list"} & self._methods)
        return any(c.id == capability_id and c.available for c in self.capabilities)

    def require_capability(self, capability_id: str) -> None:
        """Raise :class:`CapabilityUnavailableError` if a capability is missing."""
        if not self.has_capability(capability_id):
            raise CapabilityUnavailableError(
                "CAPABILITY_UNAVAILABLE",
                f"Capability '{capability_id}' is not available",
            )

    # -- Config ------------------------------------------------------------

    async def get_config(self) -> ConfigEnvelope:
        return await self._request("config/get", {})

    async def set_config(
        self,
        config: dict[str, Any],
        *,
        expected_generation: int | None = None,
    ) -> ConfigWriteResult:
        params: ConfigSetParams = {"config": config}
        if expected_generation is not None:
            params["expected_generation"] = expected_generation
        return await self._request("config/set", params)

    async def patch_config(
        self,
        patch: dict[str, Any],
        *,
        expected_generation: int | None = None,
    ) -> ConfigWriteResult:
        params: ConfigPatchParams | dict[str, Any] = {"patch": patch}
        if expected_generation is not None:
            params["expected_generation"] = expected_generation
        return await self._request("config/patch", params)

    async def get_models_catalog(self) -> ModelsCatalogResponse:
        raw = await self._request("models/catalog", {})
        return self._parse_models_catalog(raw)

    async def create_schedule(self, request: dict[str, Any]) -> ScheduleRecord:
        _rpc_signature: RpcCreateScheduleRequest | RpcSchedule
        raw = await self._request("schedule/create", request)
        return self._parse_schedule_record(raw, "Invalid schedule/create response")

    async def get_schedule(self, schedule_id: str) -> ScheduleRecord:
        _rpc_signature: RpcSchedule
        raw = await self._request("schedule/get", {"schedule_id": schedule_id})
        return self._parse_schedule_record(raw, "Invalid schedule/get response")

    async def list_schedules(
        self,
        *,
        labels: dict[str, str] | None = None,
        limit: int | None = None,
        offset: int | None = None,
    ) -> ScheduleListResult:
        params: dict[str, Any] = {}
        if labels is not None:
            params["labels"] = labels
        if limit is not None:
            params["limit"] = limit
        if offset is not None:
            params["offset"] = offset
        raw = await self._request("schedule/list", params)
        raw = self._require_dict(raw, "response", "Invalid schedule/list response")
        return {
            "schedules": [
                self._parse_schedule_record(
                    self._require_dict(
                        schedule,
                        f"schedules[{index}]",
                        "Invalid schedule/list response",
                    ),
                    f"Invalid schedule/list response schedules[{index}]",
                )
                for index, schedule in enumerate(
                    self._require_present_list_field(
                        raw, "schedules", "Invalid schedule/list response"
                    )
                )
            ],
        }

    async def update_schedule(self, request: dict[str, Any]) -> ScheduleRecord:
        _rpc_signature: RpcSchedule
        raw = await self._request("schedule/update", request)
        return self._parse_schedule_record(raw, "Invalid schedule/update response")

    async def pause_schedule(self, schedule_id: str) -> ScheduleRecord:
        _rpc_signature: RpcSchedule
        raw = await self._request("schedule/pause", {"schedule_id": schedule_id})
        return self._parse_schedule_record(raw, "Invalid schedule/pause response")

    async def resume_schedule(self, schedule_id: str) -> ScheduleRecord:
        _rpc_signature: RpcSchedule
        raw = await self._request("schedule/resume", {"schedule_id": schedule_id})
        return self._parse_schedule_record(raw, "Invalid schedule/resume response")

    async def delete_schedule(self, schedule_id: str) -> ScheduleRecord:
        _rpc_signature: RpcSchedule
        raw = await self._request("schedule/delete", {"schedule_id": schedule_id})
        return self._parse_schedule_record(raw, "Invalid schedule/delete response")

    async def list_schedule_occurrences(
        self,
        schedule_id: str,
        *,
        include_terminal: bool | None = None,
    ) -> ScheduleOccurrencesResult:
        params: dict[str, Any] = {"schedule_id": schedule_id}
        if include_terminal is not None:
            params["include_terminal"] = include_terminal
        raw = await self._request("schedule/occurrences", params)
        raw = self._require_dict(
            raw, "response", "Invalid schedule/occurrences response"
        )
        return {
            "occurrences": [
                self._parse_schedule_occurrence(
                    self._require_dict(
                        occurrence,
                        f"occurrences[{index}]",
                        "Invalid schedule/occurrences response",
                    ),
                    f"Invalid schedule/occurrences response occurrences[{index}]",
                )
                for index, occurrence in enumerate(
                    self._require_present_list_field(
                        raw, "occurrences", "Invalid schedule/occurrences response"
                    )
                )
            ],
        }

    async def list_schedule_tools(self) -> ScheduleToolsResult:
        _rpc_signature: RpcScheduleToolsResult
        raw = await self._request("schedule/tools", {})
        raw = self._require_dict(raw, "response", "Invalid schedule/tools response")
        return {
            "tools": [
                self._require_dict(
                    tool, f"tools[{index}]", "Invalid schedule/tools response"
                )
                for index, tool in enumerate(
                    self._require_present_list_field(
                        raw, "tools", "Invalid schedule/tools response"
                    )
                )
            ]
        }

    async def call_schedule_tool(self, request: ScheduleToolCall) -> dict[str, Any]:
        _rpc_signature: RpcScheduleToolCallParams
        return await self._request("schedule/call", dict(request))

    async def get_workgraph_item(
        self,
        item_id: str,
        *,
        realm_id: str | None = None,
        namespace: str | None = None,
    ) -> WorkItem:
        params = WorkGraphIdParams(
            id=item_id,
            realm_id=realm_id,
            namespace=namespace,
        )
        raw = await self._request("workgraph/get", _wire_params(params))
        return WorkItem.from_wire(raw)

    async def list_workgraph_items(
        self,
        filter: WorkItemFilter | None = None,
    ) -> WorkItemsResult:
        raw = await self._request("workgraph/list", _wire_params(filter or {}))
        return WorkItemsResult(
            items=[
                WorkItem.from_wire(item)
                for item in MeerkatClient._require_list_field(
                    raw, "items", "Invalid workgraph/list response"
                )
            ]
        )

    async def list_ready_workgraph_items(
        self,
        filter: ReadyWorkFilter | None = None,
    ) -> WorkItemsResult:
        raw = await self._request("workgraph/ready", _wire_params(filter or {}))
        return WorkItemsResult(
            items=[
                WorkItem.from_wire(item)
                for item in MeerkatClient._require_list_field(
                    raw, "items", "Invalid workgraph/ready response"
                )
            ]
        )

    async def get_workgraph_snapshot(
        self,
        filter: WorkGraphSnapshotFilter | None = None,
    ) -> WorkGraphSnapshot:
        raw = await self._request("workgraph/snapshot", _wire_params(filter or {}))
        return WorkGraphSnapshot.from_wire(raw)

    async def list_workgraph_events(
        self,
        filter: WorkGraphEventFilter | None = None,
    ) -> WorkEventsResult:
        raw = await self._request("workgraph/events", _wire_params(filter or {}))
        return WorkEventsResult(
            events=[
                WorkGraphEvent.from_wire(event)
                for event in MeerkatClient._require_list_field(
                    raw, "events", "Invalid workgraph/events response"
                )
            ]
        )

    async def get_workgraph_goal_status(
        self, params: GoalStatusRequest
    ) -> GoalStatusResult:
        raw = await self._request("workgraph/goal/status", _wire_params(params))
        return GoalStatusResult.from_wire(raw)

    async def list_workgraph_attention(
        self,
        params: AttentionListRequest | None = None,
    ) -> AttentionListResult:
        raw = await self._request(
            "workgraph/attention/list", _wire_params(params or {})
        )
        return AttentionListResult.from_wire(raw)

    async def mcp_add(
        self,
        session_id: str,
        server_config: McpServerConfig,
        *,
        persisted: bool = False,
    ) -> McpLiveOpResponse:
        raw = await self._request(
            "mcp/add",
            {
                "session_id": session_id,
                "server_config": server_config,
                "persisted": persisted,
            },
        )
        return self._parse_mcp_live_response(raw)

    async def mcp_remove(
        self,
        session_id: str,
        server_name: str,
        *,
        persisted: bool = False,
    ) -> McpLiveOpResponse:
        raw = await self._request(
            "mcp/remove",
            {
                "session_id": session_id,
                "server_name": server_name,
                "persisted": persisted,
            },
        )
        return self._parse_mcp_live_response(raw)

    async def mcp_reload(
        self,
        session_id: str,
        *,
        server_name: str | None = None,
        persisted: bool = False,
    ) -> McpLiveOpResponse:
        payload: dict[str, Any] = {
            "session_id": session_id,
            "persisted": persisted,
        }
        if server_name is not None:
            payload["server_name"] = server_name
        raw = await self._request("mcp/reload", payload)
        return self._parse_mcp_live_response(raw)

    # -- Skills -------------------------------------------------------------

    @staticmethod
    def _parse_mcp_live_response(raw: dict[str, Any]) -> McpLiveOpResponse:
        session_id = raw.get("session_id")
        operation = raw.get("operation")
        status = raw.get("status")
        persisted = raw.get("persisted")
        if not isinstance(session_id, str) or not session_id:
            raise MeerkatError(
                "INVALID_RESPONSE", "Invalid mcp response: missing session_id"
            )
        if operation not in {"add", "remove", "reload"}:
            raise MeerkatError(
                "INVALID_RESPONSE", "Invalid mcp response: invalid operation"
            )
        if not isinstance(status, str) or not status:
            raise MeerkatError(
                "INVALID_RESPONSE", "Invalid mcp response: missing status"
            )
        if not isinstance(persisted, bool):
            raise MeerkatError(
                "INVALID_RESPONSE",
                "Invalid mcp response: persisted must be boolean",
            )
        return McpLiveOpResponse(**raw)

    async def list_skills(self) -> list[SkillEntry]:
        result = await self._request("skills/list", {})
        context = "Invalid skills/list response"
        skills = result.get("skills") if isinstance(result, dict) else None
        if not isinstance(skills, list):
            raise MeerkatError("INVALID_RESPONSE", f"{context}: skills must be a list")
        envelope = SkillListResponse(
            skills=[
                MeerkatClient._parse_skill_entry(entry, context) for entry in skills
            ]
        )
        return envelope.skills

    @staticmethod
    def _parse_skill_entry(entry: Any, context: str) -> SkillEntry:
        if not isinstance(entry, dict):
            raise MeerkatError(
                "INVALID_RESPONSE", f"{context}: skill entry must be an object"
            )
        name = MeerkatClient._require_string_field(entry, "name", context)
        description = entry.get("description")
        if not isinstance(description, str):
            raise MeerkatError(
                "INVALID_RESPONSE", f"{context}: description must be a string"
            )
        scope = entry.get("scope")
        if scope not in ("builtin", "project", "user"):
            raise MeerkatError(
                "INVALID_RESPONSE", f"{context}: scope must be a skill scope"
            )
        is_active = entry.get("is_active")
        if not isinstance(is_active, bool):
            raise MeerkatError(
                "INVALID_RESPONSE", f"{context}: is_active must be a boolean"
            )
        shadowed_by_raw = entry.get("shadowed_by")
        return SkillEntry(
            key=MeerkatClient._parse_wire_skill_key(entry.get("key"), context),
            name=name,
            description=description,
            scope=scope,
            source=MeerkatClient._parse_skill_source_provenance(
                entry.get("source"), context
            ),
            is_active=is_active,
            shadowed_by=MeerkatClient._parse_skill_source_provenance(
                shadowed_by_raw, context
            )
            if shadowed_by_raw is not None
            else None,
        )

    @staticmethod
    def _parse_wire_skill_key(value: Any, context: str) -> WireSkillKey:
        if not isinstance(value, dict):
            raise MeerkatError(
                "INVALID_RESPONSE", f"{context}: skill key must be an object"
            )
        return WireSkillKey(
            source_uuid=MeerkatClient._require_string_field(
                value, "source_uuid", context
            ),
            skill_name=MeerkatClient._require_string_field(
                value, "skill_name", context
            ),
        )

    @staticmethod
    def _parse_skill_source_provenance(
        value: Any, context: str
    ) -> SkillSourceProvenance:
        if not isinstance(value, dict):
            raise MeerkatError(
                "INVALID_RESPONSE", f"{context}: skill source must be an object"
            )
        kwargs: dict[str, Any] = {
            "source_uuid": MeerkatClient._require_string_field(
                value, "source_uuid", context
            ),
            "display_name": MeerkatClient._require_string_field(
                value, "display_name", context
            ),
            "transport_kind": MeerkatClient._require_string_field(
                value, "transport_kind", context
            ),
            "fingerprint": MeerkatClient._require_string_field(
                value, "fingerprint", context
            ),
        }
        if "status" in value:
            status = value.get("status")
            if not isinstance(status, str):
                raise MeerkatError(
                    "INVALID_RESPONSE", f"{context}: status must be a string"
                )
            kwargs["status"] = status
        return SkillSourceProvenance(**kwargs)

    async def subscribe_session_events(self, session_id: str) -> EventSubscription:
        return await self._open_event_subscription(
            "session/stream_open",
            {"session_id": session_id},
            "session/stream_close",
            self._parse_agent_event_envelope,
        )

    async def create_mob(
        self,
        *,
        definition: MobDefinitionInput | dict[str, Any],
    ) -> Mob:
        self.require_capability("mob")
        result = await self._request(
            "mob/create", {"definition": _wire_params(definition)}
        )
        return Mob(
            self,
            self._require_string_field(result, "mob_id", "Invalid mob/create response"),
        )

    def mob(self, mob_id: str) -> Mob:
        return Mob(self, mob_id)

    async def list_mobs(self) -> list[dict[str, Any]]:
        self.require_capability("mob")
        result = await self._request("mob/list", {})
        mobs = self._require_present_list_field(
            result, "mobs", "Invalid mob/list response"
        )
        parsed: list[dict[str, Any]] = []
        for idx, mob in enumerate(mobs):
            entry = self._require_dict(mob, f"mobs[{idx}]", "Invalid mob/list response")
            self._require_string_field(entry, "mob_id", "Invalid mob/list response")
            parsed.append(entry)
        return parsed

    async def mob_status(self, mob_id: str) -> dict[str, Any]:
        result = await self._request("mob/status", {"mob_id": mob_id})
        raw_status = result.get("status")
        if isinstance(raw_status, str) and raw_status:
            status = raw_status
        elif isinstance(raw_status, dict) and raw_status:
            status = next(iter(raw_status))
        else:
            raise MeerkatError(
                "INVALID_RESPONSE", "Invalid mob/status response: missing status"
            )
        return {"mob_id": str(result.get("mob_id", mob_id)), "status": status}

    async def list_mob_members(self, mob_id: str) -> list[MobMember]:
        result = await self._request("mob/members", {"mob_id": mob_id})
        members = self._require_list_field(
            result, "members", "Invalid mob/members response"
        )
        normalized: list[MobMember] = []
        for entry in members:
            if not isinstance(entry, dict):
                raise MeerkatError(
                    "INVALID_RESPONSE",
                    "Invalid mob/members response: member entry must be an object",
                )
            member_ref = entry.get("member_ref")
            if not isinstance(member_ref, str) or not member_ref:
                raise MeerkatError(
                    "INVALID_RESPONSE",
                    "Invalid mob/members response: entry missing member_ref",
                )
            agent_identity = entry.get("agent_identity")
            if not isinstance(agent_identity, str) or not agent_identity:
                raise MeerkatError(
                    "INVALID_RESPONSE",
                    "Invalid mob/members response: entry missing agent_identity",
                )
            role = entry.get("role")
            if not isinstance(role, str) or not role:
                raise MeerkatError(
                    "INVALID_RESPONSE",
                    "Invalid mob/members response: entry missing role",
                )
            normalized.append(
                {
                    "agent_identity": agent_identity,
                    "member_ref": member_ref,
                    "profile": role,
                    **(
                        {"peer_id": str(entry["peer_id"])}
                        if entry.get("peer_id") is not None
                        else {}
                    ),
                    **(
                        {"external_peer_specs": entry["external_peer_specs"]}
                        if isinstance(entry.get("external_peer_specs"), dict)
                        else {}
                    ),
                    **(
                        {"runtime_mode": str(entry["runtime_mode"])}
                        if entry.get("runtime_mode") is not None
                        else {}
                    ),
                    **(
                        {"wired_to": [str(peer) for peer in entry["wired_to"]]}
                        if isinstance(entry.get("wired_to"), list)
                        else {}
                    ),
                    **(
                        {
                            "labels": {
                                str(key): str(value)
                                for key, value in entry["labels"].items()
                            }
                        }
                        if isinstance(entry.get("labels"), dict)
                        else {}
                    ),
                    **(
                        {"status": str(entry["status"])}
                        if entry.get("status") is not None
                        else {}
                    ),
                    **(
                        {"error": str(entry["error"])}
                        if entry.get("error") is not None
                        else {}
                    ),
                    **(
                        {"is_final": bool(entry["is_final"])}
                        if entry.get("is_final") is not None
                        else {}
                    ),
                    **(
                        {"kickoff": entry["kickoff"]}
                        if isinstance(entry.get("kickoff"), dict)
                        else {}
                    ),
                }
            )
        return normalized

    async def send_mob_member_content(
        self,
        mob_id: str,
        agent_identity: str,
        content: str | list[dict[str, Any]],
        *,
        handling_mode: Literal["queue", "steer"] = "queue",
        render_metadata: RenderMetadata | None = None,
    ) -> dict[str, Any]:
        result = await self._request(
            "mob/member_send",
            {
                "mob_id": mob_id,
                "agent_identity": agent_identity,
                "content": content,
                "handling_mode": handling_mode,
                "render_metadata": render_metadata,
            },
        )
        member_ref = result.get("member_ref")
        if not isinstance(member_ref, str) or not member_ref:
            raise MeerkatError(
                "INVALID_RESPONSE",
                "Invalid mob/member_send response: missing member_ref",
            )
        receipt_handling_mode = result.get("handling_mode")
        resolved_mob_id = result.get("mob_id")
        if not isinstance(resolved_mob_id, str) or not resolved_mob_id:
            raise MeerkatError(
                "INVALID_RESPONSE",
                "Invalid mob/member_send response: missing mob_id",
            )
        if resolved_mob_id != mob_id:
            raise MeerkatError(
                "INVALID_RESPONSE",
                "Invalid mob/member_send response: mob_id mismatch",
            )
        resolved_identity = result.get("agent_identity")
        if not isinstance(resolved_identity, str) or not resolved_identity:
            raise MeerkatError(
                "INVALID_RESPONSE",
                "Invalid mob/member_send response: missing agent_identity",
            )
        return {
            "mob_id": resolved_mob_id,
            "agent_identity": resolved_identity,
            "member_ref": member_ref,
            "handling_mode": self._parse_wire_handling_mode(
                receipt_handling_mode,
                "Invalid mob/member_send response: invalid handling_mode",
            ),
        }

    async def spawn_mob_member(
        self,
        mob_id: str,
        *,
        profile: str,
        agent_identity: str,
        initial_message: WireContentInput | None = None,
        runtime_mode: WireMobRuntimeMode | None = None,
        backend: WireMobBackendKind | None = None,
        labels: dict[str, str] | None = None,
        context: dict[str, Any] | None = None,
        additional_instructions: list[str] | None = None,
        placement: str | None = None,
        binding: WireRuntimeBinding | dict[str, Any] | None = None,
        shell_env: dict[str, str] | None = None,
        auto_wire_parent: bool | None = None,
        launch_mode: WireMemberLaunchMode | dict[str, Any] | None = None,
        tool_access_policy: WireToolAccessPolicy | dict[str, Any] | None = None,
        inherited_tool_filter: WireToolFilter | dict[str, list[str]] | None = None,
        override_profile: WireMobProfile | dict[str, Any] | None = None,
        model_override: str | None = None,
        auth_binding: WireAuthBindingRef | dict[str, str] | None = None,
    ) -> MobSpawnResult:
        params: dict[str, Any] = {
            "mob_id": mob_id,
            "profile": profile,
            "agent_identity": agent_identity,
            "initial_message": _wire_value(initial_message),
            "runtime_mode": runtime_mode,
            "backend": backend,
            "labels": _wire_value(labels),
            "context": _wire_value(context),
            "additional_instructions": _wire_value(additional_instructions),
        }
        for key, value in {
            "placement": placement,
            "binding": binding,
            "shell_env": shell_env,
            "auto_wire_parent": auto_wire_parent,
            "launch_mode": launch_mode,
            "tool_access_policy": tool_access_policy,
            "inherited_tool_filter": inherited_tool_filter,
            "override_profile": override_profile,
            "model_override": model_override,
            "auth_binding": auth_binding,
        }.items():
            if value is not None:
                params[key] = _wire_value(value)
        result = await self._request("mob/spawn", params)
        resolved_identity = result.get("agent_identity")
        if not isinstance(resolved_identity, str) or not resolved_identity:
            raise MeerkatError(
                "INVALID_RESPONSE",
                "Invalid mob/spawn response: missing agent_identity",
            )
        member_ref = result.get("member_ref")
        if not isinstance(member_ref, str) or not member_ref:
            raise MeerkatError(
                "INVALID_RESPONSE",
                "Invalid mob/spawn response: missing member_ref",
            )
        resolved_mob_id = result.get("mob_id")
        if not isinstance(resolved_mob_id, str) or not resolved_mob_id:
            raise MeerkatError(
                "INVALID_RESPONSE",
                "Invalid mob/spawn response: missing mob_id",
            )
        return {
            "mob_id": resolved_mob_id,
            "agent_identity": resolved_identity,
            "member_ref": member_ref,
        }

    async def spawn_mob_members(
        self,
        mob_id: str,
        specs: list[MobSpawnSpec],
    ) -> MobSpawnManyResult:
        params = MobSpawnManyParams(
            mob_id=mob_id,
            specs=[MobSpawnSpecParams(**_wire_value(spec)) for spec in specs],
        )
        result: MobSpawnManyResult | dict[str, Any] = await self._request(
            "mob/spawn_many",
            _wire_value(params),
        )
        if isinstance(result, MobSpawnManyResult):  # pragma: no cover - typing aid
            return result
        entries = result.get("results")
        if not isinstance(entries, list):
            raise MeerkatError(
                "INVALID_RESPONSE",
                "Invalid mob/spawn_many response: results must be a list",
            )

        normalized: list[MobSpawnManyResultEntry] = []
        for entry in entries:
            if not isinstance(entry, dict):
                raise MeerkatError(
                    "INVALID_RESPONSE",
                    "Invalid mob/spawn_many response: malformed result entry",
                )
            entry_keys = set(entry.keys())
            if entry_keys - {"status", "result"} or any(
                key in entry for key in ("ok", "error", "agent_identity", "member_ref")
            ):
                raise MeerkatError(
                    "INVALID_RESPONSE",
                    "Invalid mob/spawn_many response: legacy result carrier fields are not allowed",
                )
            status = entry.get("status")
            payload = entry.get("result")
            if status not in ("spawned", "failed"):
                raise MeerkatError(
                    "INVALID_RESPONSE",
                    "Invalid mob/spawn_many response: invalid result status",
                )
            if not isinstance(payload, dict):
                raise MeerkatError(
                    "INVALID_RESPONSE",
                    "Invalid mob/spawn_many response: missing result payload",
                )
            if status == "spawned":
                if set(payload.keys()) - {"agent_identity", "member_ref"}:
                    raise MeerkatError(
                        "INVALID_RESPONSE",
                        "Invalid mob/spawn_many response: spawned result has unknown fields",
                    )
                agent_identity = payload.get("agent_identity")
                member_ref = payload.get("member_ref")
                if not isinstance(agent_identity, str) or not agent_identity:
                    raise MeerkatError(
                        "INVALID_RESPONSE",
                        "Invalid mob/spawn_many response: spawned result missing agent_identity",
                    )
                if not isinstance(member_ref, str) or not member_ref:
                    raise MeerkatError(
                        "INVALID_RESPONSE",
                        "Invalid mob/spawn_many response: spawned result missing member_ref",
                    )
                normalized.append(
                    MobSpawnManyResultEntry(
                        status="spawned",
                        result=MobSpawnManySpawnedResult(
                            agent_identity=agent_identity,
                            member_ref=member_ref,
                        ),
                    )
                )
                continue

            if set(payload.keys()) - {"cause", "message"}:
                raise MeerkatError(
                    "INVALID_RESPONSE",
                    "Invalid mob/spawn_many response: failed result has unknown fields",
                )
            cause = payload.get("cause")
            if (
                not isinstance(cause, str)
                or cause not in _MOB_SPAWN_MANY_FAILURE_CAUSES
            ):
                raise MeerkatError(
                    "INVALID_RESPONSE",
                    "Invalid mob/spawn_many response: failed result has invalid cause",
                )
            message = payload.get("message")
            if not isinstance(message, str) or not message:
                raise MeerkatError(
                    "INVALID_RESPONSE",
                    "Invalid mob/spawn_many response: failed result missing message",
                )
            normalized.append(
                MobSpawnManyResultEntry(
                    status="failed",
                    result=MobSpawnManyFailedResult(cause=cause, message=message),
                )
            )
        return MobSpawnManyResult(results=normalized)

    async def read_mob_events(
        self,
        mob_id: str,
        *,
        after_cursor: int = 0,
        limit: int = 100,
    ) -> MobEventsResult:
        raw = await self._request(
            "mob/events",
            {
                "mob_id": mob_id,
                "after_cursor": after_cursor,
                "limit": limit,
            },
        )
        return {
            "events": MeerkatClient._require_list_field(
                raw, "events", "Invalid mob/events response"
            )
        }

    async def mob_ingress_interaction(self, params: dict[str, Any]) -> dict[str, Any]:
        return await self._request("mob/ingress_interaction", params)

    async def retire_mob_member(self, mob_id: str, agent_identity: str) -> None:
        await self._request(
            "mob/retire", {"mob_id": mob_id, "agent_identity": agent_identity}
        )

    async def respawn_mob_member(
        self,
        mob_id: str,
        agent_identity: str,
        initial_message: str | list[dict] | None = None,
    ) -> MobRespawnResult:
        result = await self._request(
            "mob/respawn",
            {
                "mob_id": mob_id,
                "agent_identity": agent_identity,
                "initial_message": initial_message,
            },
        )
        receipt = result.get("receipt")
        if isinstance(receipt, dict):
            member_ref = receipt.get("member_ref")
            if not isinstance(member_ref, str) or not member_ref:
                raise MeerkatError(
                    "INVALID_RESPONSE",
                    "Invalid mob/respawn response: receipt missing member_ref",
                )
            resolved_identity = receipt.get("identity")
            if not isinstance(resolved_identity, str) or not resolved_identity:
                raise MeerkatError(
                    "INVALID_RESPONSE",
                    "Invalid mob/respawn response: receipt missing identity",
                )
            result = dict(result)
            result["receipt"] = {
                "agent_identity": resolved_identity,
                "member_ref": member_ref,
            }
        return {
            "status": self._parse_wire_respawn_outcome(
                result.get("status"),
                "Invalid mob/respawn response: invalid status",
            ),
            "receipt": result["receipt"],
            **(
                {
                    "failed_peer_ids": [
                        str(peer_id) for peer_id in result["failed_peer_ids"]
                    ]
                }
                if isinstance(result.get("failed_peer_ids"), list)
                else {}
            ),
        }

    async def force_cancel_mob_member(self, mob_id: str, agent_identity: str) -> None:
        await self._request(
            "mob/force_cancel",
            {"mob_id": mob_id, "agent_identity": agent_identity},
        )

    async def grant_mob_scopes(
        self,
        mob_id: str,
        principal: str,
        scopes: list[MobControlScope],
        *,
        expires_at_ms: int | None = None,
    ) -> MobGrantRecord:
        """Replace a principal's control-scope grant and return its record."""
        params = MobGrantScopesParams(
            mob_id=mob_id,
            principal=principal,
            scopes=scopes,
            expires_at_ms=expires_at_ms,
        )
        _rpc_signature: MobGrantScopesParams | MobGrantScopesResult
        result = await self._request("mob/grant_scopes", _wire_params(params))
        context = "Invalid mob/grant_scopes response"
        record = self._require_dict(result.get("record"), "record", context)
        return self._parse_mob_grant_record(record, context)

    async def revoke_mob_scopes(
        self,
        mob_id: str,
        principal: str,
        scopes: list[MobControlScope] | None = None,
    ) -> bool:
        """Revoke selected scopes, or the whole grant when scopes is omitted."""
        params = MobRevokeScopesParams(
            mob_id=mob_id,
            principal=principal,
            scopes=scopes,
        )
        _rpc_signature: MobRevokeScopesParams | MobRevokeScopesResult
        result = await self._request("mob/revoke_scopes", _wire_params(params))
        return self._require_bool_field(
            result,
            "removed",
            "Invalid mob/revoke_scopes response",
        )

    async def list_mob_grants(self, mob_id: str) -> list[MobGrantRecord]:
        """List typed control-scope grants, including optional expiry metadata."""
        params = MobIdParams(mob_id=mob_id)
        _rpc_signature: MobIdParams | MobGrantsResult
        result = await self._request("mob/grants", _wire_params(params))
        context = "Invalid mob/grants response"
        grants = self._require_present_list_field(result, "grants", context)
        return [
            self._parse_mob_grant_record(
                self._require_dict(record, f"grants[{index}]", context),
                f"{context}: grants[{index}]",
            )
            for index, record in enumerate(grants)
        ]

    async def mob_member_history(
        self,
        mob_id: str,
        agent_identity: str,
        *,
        from_index: int | None = None,
        limit: int | None = None,
    ) -> MobMemberHistoryResult:
        """Read one typed transcript page for a local or remote member."""
        params = MobMemberHistoryParams(
            mob_id=mob_id,
            agent_identity=agent_identity,
            from_index=from_index,
            limit=limit,
        )
        result = await self._request("mob/member_history", _wire_params(params))
        return self._parse_mob_member_history_result(result)

    async def mob_hosts(self, mob_id: str) -> list[MobHostStatus]:
        """List tracked member hosts with committed authority facts."""
        params = MobIdParams(mob_id=mob_id)
        _rpc_signature: MobIdParams | MobHostsResult
        result = await self._request("mob/hosts", _wire_params(params))
        context = "Invalid mob/hosts response"
        hosts = self._require_present_list_field(result, "hosts", context)
        return [
            self._parse_mob_host_status(row, f"{context}: hosts[{index}]")
            for index, row in enumerate(hosts)
        ]

    async def mob_route_installs(self, mob_id: str) -> MobRouteInstallsResult:
        """Read outstanding cross-host route-install obligations."""
        params = MobIdParams(mob_id=mob_id)
        result = await self._request("mob/route_installs", _wire_params(params))
        context = "Invalid mob/route_installs response"
        outstanding = self._require_present_list_field(
            result,
            "outstanding",
            context,
        )
        obligations = [
            self._parse_route_install_obligation(
                row,
                f"{context}: outstanding[{index}]",
            )
            for index, row in enumerate(outstanding)
        ]
        return MobRouteInstallsResult(
            complete=self._require_bool_field(result, "complete", context),
            outstanding=obligations,
        )

    async def bind_mob_host(
        self,
        mob_id: str,
        descriptor: WireHostBindingDescriptor | dict[str, Any],
    ) -> MobBindHostResult:
        """Bind a member-host daemon from its typed binding descriptor."""
        params = MobBindHostParams(
            mob_id=mob_id,
            descriptor=cast(WireHostBindingDescriptor, descriptor),
        )
        result = await self._request("mob/bind_host", _wire_params(params))
        context = "Invalid mob/bind_host response"
        return MobBindHostResult(
            authority_epoch=self._require_non_negative_integer_field(
                result,
                "authority_epoch",
                context,
            ),
            capabilities=self._parse_mob_host_capabilities(
                result.get("capabilities"),
                f"{context}: capabilities",
            ),
            host_id=self._require_string_field(result, "host_id", context),
        )

    async def revoke_mob_host(
        self,
        mob_id: str,
        host_id: str,
    ) -> MobRevokeHostResult:
        """Revoke a host and return the identities released from it."""
        params = MobRevokeHostParams(mob_id=mob_id, host_id=host_id)
        result = await self._request("mob/revoke_host", _wire_params(params))
        context = "Invalid mob/revoke_host response"
        return MobRevokeHostResult(
            host_id=self._require_string_field(result, "host_id", context),
            released_members=self._parse_nonempty_string_list(
                result.get("released_members"),
                f"{context}: released_members",
            ),
        )

    async def hard_cancel_mob_member(
        self,
        mob_id: str,
        agent_identity: str,
        reason: str,
    ) -> bool:
        """Hard-cancel a member using immediate user-interrupt authority."""
        params = MobHardCancelParams(
            mob_id=mob_id,
            agent_identity=agent_identity,
            reason=reason,
        )
        _rpc_signature: MobHardCancelParams | MobHardCancelResult
        result = await self._request("mob/hard_cancel_member", _wire_params(params))
        return self._require_bool_field(
            result,
            "cancelled",
            "Invalid mob/hard_cancel_member response",
        )

    async def open_mob_member_live(
        self,
        mob_id: str,
        agent_identity: str,
        *,
        turning_mode: RealtimeTurningMode | None = None,
        transport: MobMemberLiveTransport | None = None,
    ) -> LiveOpenResult:
        """Open a member live channel while preserving its opaque bootstrap."""
        params = MobMemberLiveOpenParams(
            mob_id=mob_id,
            agent_identity=agent_identity,
            turning_mode=turning_mode,
            transport=transport,
        )
        result = await self._request("mob/member_live_open", _wire_params(params))
        return self._parse_live_open_result(
            result,
            "Invalid mob/member_live_open response",
        )

    async def close_mob_member_live(
        self,
        mob_id: str,
        agent_identity: str,
        channel_id: str,
    ) -> LiveCloseResult:
        """Close exactly the named member live channel."""
        params = MobMemberLiveChannelParams(
            mob_id=mob_id,
            agent_identity=agent_identity,
            channel_id=channel_id,
        )
        result = await self._request("mob/member_live_close", _wire_params(params))
        context = "Invalid mob/member_live_close response"
        status = self._require_string_field(result, "status", context)
        if status != "closed":
            raise MeerkatError(
                "INVALID_RESPONSE",
                f"{context}: unsupported status {status!r}",
            )
        return LiveCloseResult(status=cast("LiveCloseStatus", status))

    async def mob_member_live_status(
        self,
        mob_id: str,
        agent_identity: str,
        channel_id: str | None = None,
    ) -> LiveStatusResult:
        """Read a named channel or discover the member's active channel."""
        params = MobMemberLiveStatusParams(
            mob_id=mob_id,
            agent_identity=agent_identity,
            channel_id=channel_id,
        )
        result = await self._request("mob/member_live_status", _wire_params(params))
        return self._parse_live_status_result(
            result,
            "Invalid mob/member_live_status response",
        )

    async def control_mob_member_live(
        self,
        mob_id: str,
        agent_identity: str,
        channel_id: str,
        verb: BridgeLiveControlVerb,
    ) -> BridgeLiveControlOutcome:
        """Drive one typed turn-level control verb on a member channel."""
        requested_verb = verb.get("verb") if isinstance(verb, dict) else None
        if requested_verb not in {"commit_input", "interrupt", "truncate", "refresh"}:
            raise MeerkatError(
                "INVALID_ARGS",
                "mob/member_live_control verb must use the closed live-control vocabulary",
            )
        params = MobMemberLiveControlParams(
            mob_id=mob_id,
            agent_identity=agent_identity,
            channel_id=channel_id,
            verb=verb,
        )
        result = await self._request("mob/member_live_control", _wire_params(params))
        return self._parse_bridge_live_control_outcome(
            result,
            "Invalid mob/member_live_control response",
            requested_verb,
        )

    async def mob_turn_start(
        self,
        mob_id: str,
        agent_identity: str,
        prompt: WireContentInput,
        *,
        skill_refs: list[SkillRef] | None = None,
        turn_tool_overlay: PublicTurnToolOverlay | None = None,
        additional_instructions: list[str] | None = None,
        injected_context: list[WireContentInput] | None = None,
        keep_alive: bool | None = None,
        model: str | None = None,
        provider: str | None = None,
        self_hosted_server_id: str | None = None,
        max_tokens: int | None = None,
        system_prompt: str | None = None,
        output_schema: Any | None = None,
        structured_output_retries: int | None = None,
        provider_params: dict[str, Any] | None = None,
        auth_binding: dict[str, Any] | None = None,
    ) -> dict[str, Any]:
        """Start a turn on a mob member.

        ``injected_context`` carries host-attached ambient context delivered
        as separate typed transcript messages immediately before the turn's
        user message (excluded from semantic-memory indexing).

        ``self_hosted_server_id`` selects the exact configured local server
        for a self-hosted model instead of relying on model identity alone.

        ``provider_params`` and ``auth_binding`` carry the canonical
        Inherit/Set/Clear tri-state exactly as the wire does: pass
        ``{"action": "set", "value": ...}`` (or an untagged value, which the
        server admits as Set), ``{"action": "clear"}``, or omit the argument
        to inherit. The retired split ``clear_*`` keyword form is rejected by
        the server as an unknown field.
        """
        params = MobTurnStartParams(
            mob_id=mob_id,
            agent_identity=agent_identity,
            prompt=prompt,
            skill_refs=_skill_refs_to_wire(skill_refs),
            turn_tool_overlay=turn_tool_overlay,
            additional_instructions=additional_instructions,
            injected_context=injected_context,
            keep_alive=keep_alive,
            model=model,
            provider=provider,
            self_hosted_server_id=self_hosted_server_id,
            max_tokens=max_tokens,
            system_prompt=system_prompt,
            output_schema=output_schema,
            structured_output_retries=structured_output_retries,
            provider_params=provider_params,
            auth_binding=auth_binding,
        )
        return await self._request("mob/turn_start", _wire_value(params))

    async def mob_member_status(
        self, mob_id: str, agent_identity: str
    ) -> MobMemberSnapshot:
        _rpc_signature: MobMemberStatusResult
        params = MobMemberParams(mob_id=mob_id, agent_identity=agent_identity)
        result = await self._request("mob/member_status", _wire_value(params))
        resolved_capabilities = self._parse_resolved_model_capabilities(
            result.get("resolved_capabilities")
        )
        peer_connectivity = self._parse_mob_peer_connectivity(
            result.get("peer_connectivity"),
            "Invalid mob/member_status response",
        )
        kickoff = self._optional_dict_field(
            result,
            "kickoff",
            "Invalid mob/member_status response",
        )
        return {
            "status": self._require_string_field(
                result,
                "status",
                "Invalid mob/member_status response",
            ),
            # The opaque member handle is runtime-owned: read it from the
            # payload rather than synthesizing it client-side from
            # {mob_id, agent_identity}.
            "member_ref": self._require_string_field(
                result,
                "member_ref",
                "Invalid mob/member_status response",
            ),
            **(
                {"output_preview": str(result["output_preview"])}
                if result.get("output_preview") is not None
                else {}
            ),
            **(
                {"error": str(result["error"])}
                if result.get("error") is not None
                else {}
            ),
            "tokens_used": self._require_non_negative_integer_field(
                result,
                "tokens_used",
                "Invalid mob/member_status response",
            ),
            "is_final": self._require_bool_field(
                result,
                "is_final",
                "Invalid mob/member_status response",
            ),
            **(
                {"resolved_capabilities": resolved_capabilities}
                if resolved_capabilities is not None
                else {}
            ),
            **(
                {"peer_connectivity": peer_connectivity}
                if peer_connectivity is not None
                else {}
            ),
            **({"kickoff": kickoff} if kickoff is not None else {}),
            **(
                {"external_member": result["external_member"]}
                if "external_member" in result
                else {}
            ),
            **(
                {
                    "progress": self._parse_member_progress_snapshot(
                        result["progress"],
                        "Invalid mob/member_status response",
                    )
                }
                if result.get("progress") is not None
                else {}
            ),
            # Preserve `current_session_id` as diagnostic status/continuity data only.
            **(
                {"current_session_id": str(result["current_session_id"])}
                if isinstance(result.get("current_session_id"), str)
                else {}
            ),
        }

    async def wait_mob_kickoff(
        self,
        mob_id: str,
        *,
        member_ids: list[str] | None = None,
        timeout_ms: int | None = None,
    ) -> list[MobKickoffMemberSnapshot]:
        params: dict[str, Any] = {"mob_id": mob_id}
        if member_ids is not None:
            params["member_ids"] = member_ids
        if timeout_ms is not None:
            params["timeout_ms"] = timeout_ms
        result = await self._request("mob/wait_kickoff", params)
        members = self._require_list_field(
            result, "members", "Invalid mob/wait_kickoff response"
        )
        normalized: list[MobKickoffMemberSnapshot] = []
        for entry in members:
            if not isinstance(entry, dict):
                raise MeerkatError(
                    "INVALID_RESPONSE",
                    "Invalid mob/wait_kickoff response: member entry must be an object",
                )
            peer_connectivity = self._parse_mob_peer_connectivity(
                entry.get("peer_connectivity"),
                "Invalid mob/wait_kickoff response",
            )
            kickoff = self._optional_dict_field(
                entry,
                "kickoff",
                "Invalid mob/wait_kickoff response",
            )
            normalized.append(
                {
                    "agent_identity": self._require_string_field(
                        entry,
                        "agent_identity",
                        "Invalid mob/wait_kickoff response",
                    ),
                    "status": self._require_string_field(
                        entry,
                        "status",
                        "Invalid mob/wait_kickoff response",
                    ),
                    **(
                        {"output_preview": str(entry["output_preview"])}
                        if entry.get("output_preview") is not None
                        else {}
                    ),
                    **(
                        {"error": str(entry["error"])}
                        if entry.get("error") is not None
                        else {}
                    ),
                    "tokens_used": self._require_non_negative_integer_field(
                        entry,
                        "tokens_used",
                        "Invalid mob/wait_kickoff response",
                    ),
                    "is_final": self._require_bool_field(
                        entry,
                        "is_final",
                        "Invalid mob/wait_kickoff response",
                    ),
                    **(
                        {"peer_connectivity": peer_connectivity}
                        if peer_connectivity is not None
                        else {}
                    ),
                    **({"kickoff": kickoff} if kickoff is not None else {}),
                }
            )
        return normalized

    async def wait_mob_ready(
        self,
        mob_id: str,
        *,
        member_ids: list[str] | None = None,
        timeout_ms: int | None = None,
    ) -> list[MobReadyMemberSnapshot]:
        params: dict[str, Any] = {"mob_id": mob_id}
        if member_ids is not None:
            params["member_ids"] = member_ids
        if timeout_ms is not None:
            params["timeout_ms"] = timeout_ms
        result = await self._request("mob/wait_ready", params)
        members = self._require_list_field(
            result, "members", "Invalid mob/wait_ready response"
        )
        normalized: list[MobReadyMemberSnapshot] = []
        for entry in members:
            if not isinstance(entry, dict):
                raise MeerkatError(
                    "INVALID_RESPONSE",
                    "Invalid mob/wait_ready response: member entry must be an object",
                )
            peer_connectivity = self._parse_mob_peer_connectivity(
                entry.get("peer_connectivity"),
                "Invalid mob/wait_ready response",
            )
            kickoff = self._optional_dict_field(
                entry,
                "kickoff",
                "Invalid mob/wait_ready response",
            )
            normalized.append(
                {
                    "agent_identity": self._require_string_field(
                        entry,
                        "agent_identity",
                        "Invalid mob/wait_ready response",
                    ),
                    "status": self._require_string_field(
                        entry,
                        "status",
                        "Invalid mob/wait_ready response",
                    ),
                    **(
                        {"output_preview": str(entry["output_preview"])}
                        if entry.get("output_preview") is not None
                        else {}
                    ),
                    **(
                        {"error": str(entry["error"])}
                        if entry.get("error") is not None
                        else {}
                    ),
                    "tokens_used": self._require_non_negative_integer_field(
                        entry,
                        "tokens_used",
                        "Invalid mob/wait_ready response",
                    ),
                    "is_final": self._require_bool_field(
                        entry,
                        "is_final",
                        "Invalid mob/wait_ready response",
                    ),
                    **(
                        {"peer_connectivity": peer_connectivity}
                        if peer_connectivity is not None
                        else {}
                    ),
                    **({"kickoff": kickoff} if kickoff is not None else {}),
                }
            )
        return normalized

    async def spawn_mob_helper(
        self,
        mob_id: str,
        prompt: str,
        *,
        agent_identity: str | None = None,
        role_name: str | None = None,
        model_override: str | None = None,
        auth_binding: WireAuthBindingRef | dict[str, str] | None = None,
        runtime_mode: str | None = None,
        backend: str | None = None,
    ) -> MobHelperResult:
        result = await self._request(
            "mob/spawn_helper",
            {
                "mob_id": mob_id,
                "prompt": prompt,
                "agent_identity": agent_identity,
                "role_name": role_name,
                "model_override": model_override,
                "auth_binding": _wire_value(auth_binding),
                "runtime_mode": runtime_mode,
                "backend": backend,
            },
        )
        member_ref = result.get("member_ref")
        if not isinstance(member_ref, str) or not member_ref:
            raise MeerkatError(
                "INVALID_RESPONSE",
                "Invalid mob/spawn_helper response: missing member_ref",
            )
        resolved_identity = result.get("agent_identity")
        if not isinstance(resolved_identity, str) or not resolved_identity:
            raise MeerkatError(
                "INVALID_RESPONSE",
                "Invalid mob/spawn_helper response: missing agent_identity",
            )
        return {
            "output": str(result["output"])
            if result.get("output") is not None
            else None,
            "tokens_used": self._require_non_negative_integer_field(
                result,
                "tokens_used",
                "Invalid mob/spawn_helper response",
            ),
            "agent_identity": resolved_identity,
            "member_ref": member_ref,
        }

    async def fork_mob_helper(
        self,
        mob_id: str,
        source_member_id: str,
        prompt: str,
        *,
        agent_identity: str | None = None,
        role_name: str | None = None,
        model_override: str | None = None,
        auth_binding: WireAuthBindingRef | dict[str, str] | None = None,
        fork_context: dict[str, Any] | None = None,
        runtime_mode: str | None = None,
        backend: str | None = None,
    ) -> MobHelperResult:
        result = await self._request(
            "mob/fork_helper",
            {
                "mob_id": mob_id,
                "source_member_id": source_member_id,
                "prompt": prompt,
                "agent_identity": agent_identity,
                "role_name": role_name,
                "model_override": model_override,
                "auth_binding": _wire_value(auth_binding),
                "fork_context": fork_context,
                "runtime_mode": runtime_mode,
                "backend": backend,
            },
        )
        member_ref = result.get("member_ref")
        if not isinstance(member_ref, str) or not member_ref:
            raise MeerkatError(
                "INVALID_RESPONSE",
                "Invalid mob/fork_helper response: missing member_ref",
            )
        resolved_identity = result.get("agent_identity")
        if not isinstance(resolved_identity, str) or not resolved_identity:
            raise MeerkatError(
                "INVALID_RESPONSE",
                "Invalid mob/fork_helper response: missing agent_identity",
            )
        return {
            "output": str(result["output"])
            if result.get("output") is not None
            else None,
            "tokens_used": self._require_non_negative_integer_field(
                result,
                "tokens_used",
                "Invalid mob/fork_helper response",
            ),
            "agent_identity": resolved_identity,
            "member_ref": member_ref,
        }

    async def create_mob_profile(
        self, name: str, profile: MobProfile
    ) -> StoredMobProfile:
        return await self._request(
            "mob/profile/create", {"name": name, "profile": profile}
        )

    async def get_mob_profile(self, name: str) -> StoredMobProfile | None:
        raw = await self._request("mob/profile/get", {"name": name})
        if raw.get("not_found") is True:
            return None
        return raw

    async def list_mob_profiles(self) -> list[StoredMobProfile]:
        raw = await self._request("mob/profile/list", {})
        profiles = self._require_present_list_field(
            raw, "profiles", "Invalid mob/profile/list response"
        )
        parsed: list[StoredMobProfile] = []
        for idx, profile in enumerate(profiles):
            entry = self._require_dict(
                profile,
                f"profiles[{idx}]",
                "Invalid mob/profile/list response",
            )
            self._require_string_field(
                entry, "name", "Invalid mob/profile/list response"
            )
            parsed.append(entry)  # type: ignore[arg-type]
        return parsed

    async def update_mob_profile(
        self,
        name: str,
        profile: MobProfile,
        expected_revision: int,
    ) -> StoredMobProfile:
        return await self._request(
            "mob/profile/update",
            {
                "name": name,
                "profile": profile,
                "expected_revision": expected_revision,
            },
        )

    async def delete_mob_profile(
        self,
        name: str,
        expected_revision: int,
    ) -> DeletedMobProfile:
        return await self._request(
            "mob/profile/delete",
            {"name": name, "expected_revision": expected_revision},
        )

    async def wire_mob_members(
        self, mob_id: str, member: str, peer: str | dict[str, Any]
    ) -> None:
        payload = (
            {"mob_id": mob_id, "member": member, "peer": {"local": peer}}
            if isinstance(peer, str)
            else {"mob_id": mob_id, "member": member, "peer": peer}
        )
        await self._request("mob/wire", payload)

    async def mob_wire_members_batch(
        self,
        mob_id: str,
        edges: list[MobWireMembersBatchEdgeInput],
    ) -> MobWireMembersBatchResult:
        payload = {
            "mob_id": mob_id,
            "edges": [_normalize_mob_wire_members_batch_edge(edge) for edge in edges],
        }
        result = await self._request("mob/wire_members_batch", payload)
        return _parse_mob_wire_members_batch_report(result)

    async def unwire_mob_members(
        self, mob_id: str, member: str, peer: str | dict[str, Any]
    ) -> None:
        payload = (
            {"mob_id": mob_id, "member": member, "peer": {"local": peer}}
            if isinstance(peer, str)
            else {"mob_id": mob_id, "member": member, "peer": peer}
        )
        await self._request("mob/unwire", payload)

    async def mob_lifecycle(self, mob_id: str, action: MobLifecycleAction) -> None:
        """Drive a mob through a typed lifecycle transition.

        ``action`` must be one of the `MobLifecycleAction` literals
        ("stop", "resume", "complete", "reset", "destroy"). The wire
        contract rejects any other value.
        """
        await self._request("mob/lifecycle", {"mob_id": mob_id, "action": action})

    async def mob_snapshot(self, mob_id: str) -> dict[str, Any]:
        """Point-in-time aggregate of mob status plus member list.

        Wraps the ``mob/snapshot`` RPC (DELETE_ME C2). Returns
        ``{mob_id, status, members}`` in one atomic call so consumers
        do not have to compose ``mob/status`` + ``mob/members`` or fall
        back to event-stream projection for point-in-time state.
        """
        return await self._request("mob/snapshot", {"mob_id": mob_id})

    async def mob_destroy(self, mob_id: str) -> dict[str, Any]:
        """Destroy a mob and surface the structured ``MobDestroyReport``.

        Wraps the ``mob/destroy`` RPC (DELETE_ME C3). Unlike
        ``mob/lifecycle`` with ``action="destroy"``, this dedicated
        endpoint has a predictable response shape
        (``{mob_id, ok, destroy_report}``) that does not require
        branching on an action string.
        """
        return await self._request("mob/destroy", {"mob_id": mob_id})

    async def mob_rotate_supervisor(self, mob_id: str) -> MobRotateSupervisorResult:
        """Rotate the supervisor bridge for all members of a mob.

        Wraps the ``mob/rotate_supervisor`` RPC (DELETE_ME C10). Returns
        the full ``SupervisorRotationReport`` so operators can inspect
        per-member rotation outcomes instead of getting a bare
        ``ok: true``.
        """
        return await self._request("mob/rotate_supervisor", {"mob_id": mob_id})

    async def mob_submit_work(
        self,
        member_ref: MobMemberRef,
        content: Any,
        *,
        work_ref: str | None = None,
        origin: WorkOrigin = "external",
        injected_context: list[WireContentInput] | None = None,
        objective_id: str | None = None,
    ) -> dict[str, Any]:
        """Submit a unit of work to a mob member through the work lane.

        ``member_ref`` is the opaque handle returned by
        ``ensure_member``/``spawn_helper``/``fork_helper`` and member-list
        responses. The server resolves it to the current incarnation, so
        callers never pass raw ``generation`` / ``fence_token`` values.
        ``origin`` is ``"external"`` for user-originated turns and
        ``"internal"`` for mob-orchestration work. When ``work_ref`` is
        omitted the server generates a fresh UUID. ``injected_context``
        carries host-attached ambient context delivered as separate typed
        transcript messages immediately before the work content (queue-mode
        turn-driven delivery only; steer and autonomous-inbox dispatch reject
        it fail-closed).
        """
        params: dict[str, Any] = {
            "member_ref": member_ref,
            "content": content,
            "origin": origin,
        }
        if work_ref is not None:
            params["work_ref"] = work_ref
        if injected_context:
            params["injected_context"] = injected_context
        if objective_id is not None:
            params["objective_id"] = objective_id
        return await self._request("mob/submit_work", params)

    async def mob_conclude_objective(
        self, member_ref: MobMemberRef, objective_id: str, outcome: str
    ) -> MobConcludeObjectiveResult:
        """Conclude a machine-owned kickoff objective."""
        params = MobConcludeObjectiveParams(
            member_ref=member_ref, objective_id=objective_id, outcome=outcome
        )
        result = await self._request("mob/conclude_objective", params.__dict__)
        context = "Invalid mob/conclude_objective response"
        return MobConcludeObjectiveResult(
            member_ref=self._require_string_field(result, "member_ref", context),
            objective_id=self._require_string_field(result, "objective_id", context),
            concluded=self._require_bool_field(result, "concluded", context),
        )

    async def mob_cancel_work(self, mob_id: str, work_ref: str) -> dict[str, Any]:
        """Cancel a previously submitted unit of work.

        Wraps the ``mob/cancel_work`` RPC (DELETE_ME C4).
        """
        result = await self._request(
            "mob/cancel_work", {"mob_id": mob_id, "work_ref": work_ref}
        )
        context = "Invalid mob/cancel_work response"
        return {
            "mob_id": self._require_string_field(result, "mob_id", context),
            "ok": self._require_bool_field(result, "ok", context),
        }

    async def mob_cancel_all_work(self, member_ref: MobMemberRef) -> dict[str, Any]:
        """Cancel all in-flight work for a specific mob member.

        ``member_ref`` is the opaque handle returned by
        ``ensure_member``/``spawn_helper``/``fork_helper`` and member-list
        responses.
        """
        result = await self._request(
            "mob/cancel_all_work",
            {"member_ref": member_ref},
        )
        context = "Invalid mob/cancel_all_work response"
        return {
            "mob_id": self._require_string_field(result, "mob_id", context),
            "ok": self._require_bool_field(result, "ok", context),
        }

    async def append_mob_system_context(
        self,
        mob_id: str,
        agent_identity: str,
        text: str,
        *,
        source: str | None = None,
        idempotency_key: str | None = None,
    ) -> dict[str, Any]:
        result = await self._request(
            "mob/append_system_context",
            {
                "mob_id": mob_id,
                "agent_identity": agent_identity,
                "text": text,
                "source": source,
                "idempotency_key": idempotency_key,
            },
        )
        resolved_identity = result.get("agent_identity")
        if not isinstance(resolved_identity, str) or not resolved_identity:
            raise MeerkatError(
                "INVALID_RESPONSE",
                "Invalid mob/append_system_context response: missing agent_identity",
            )
        return {
            "mob_id": str(result.get("mob_id", mob_id)),
            "agent_identity": resolved_identity,
            "status": self._parse_wire_append_system_context_status(
                result.get("status"),
                "Invalid mob/append_system_context response: invalid status",
            ),
        }

    async def list_mob_flows(self, mob_id: str) -> list[str]:
        result = await self._request("mob/flows", {"mob_id": mob_id})
        context = "Invalid mob/flows response"
        self._require_string_field(result, "mob_id", context)
        flows = self._require_present_list_field(result, "flows", context)
        for idx, flow in enumerate(flows):
            if not isinstance(flow, str) or not flow:
                raise MeerkatError(
                    "INVALID_RESPONSE",
                    f"{context}: flows[{idx}] must be string",
                )
        return flows

    async def run_mob_flow(
        self, mob_id: str, flow_id: str, params: dict[str, Any] | None = None
    ) -> str:
        result = await self._request(
            "mob/flow_run",
            {"mob_id": mob_id, "flow_id": flow_id, "params": params or {}},
        )
        return self._require_string_field(
            result, "run_id", "Invalid mob/flow_run response"
        )

    async def run_mob(
        self,
        mob_id: str,
        params: dict[str, Any] | None = None,
        *,
        prompt: str | None = None,
        flow_id: str | None = None,
    ) -> str:
        payload = MobRunParams(
            mob_id=mob_id,
            params=params or {},
            prompt=prompt,
            flow_id=flow_id,
        )
        result: MobFlowRunResult | dict[str, Any] = await self._request(
            "mob/run",
            _wire_params(payload),
        )
        if isinstance(result, dict):
            return self._require_string_field(
                result, "run_id", "Invalid mob/run response"
            )
        return result.run_id

    async def get_mob_flow_status(
        self, mob_id: str, run_id: str
    ) -> dict[str, Any] | None:
        result = await self._request(
            "mob/flow_status", {"mob_id": mob_id, "run_id": run_id}
        )
        return result.get("run")

    async def get_mob_run_result(
        self, mob_id: str, run_id: str
    ) -> dict[str, Any] | None:
        params = MobRunResultParams(mob_id=mob_id, run_id=run_id)
        result: MobRunResult | dict[str, Any] = await self._request(
            "mob/run_result", _wire_params(params)
        )
        if isinstance(result, dict):
            return result.get("run")
        return result.run

    async def cancel_mob_flow(self, mob_id: str, run_id: str) -> None:
        await self._request("mob/flow_cancel", {"mob_id": mob_id, "run_id": run_id})

    async def subscribe_mob_events(self, mob_id: str) -> EventSubscription:
        return await self._open_event_subscription(
            "mob/stream_open",
            {"mob_id": mob_id},
            "mob/stream_close",
            self._parse_attributed_mob_event,
        )

    async def subscribe_mob_member_events(
        self, mob_id: str, agent_identity: str
    ) -> EventSubscription:
        return await self._open_event_subscription(
            "mob/stream_open",
            {"mob_id": mob_id, "agent_identity": agent_identity},
            "mob/stream_close",
            self._parse_agent_event_envelope,
        )

    async def _open_event_subscription(
        self,
        open_method: str,
        params: dict[str, Any],
        close_method: str,
        parser: Any,
    ) -> EventSubscription:
        if not self._dispatcher:
            raise MeerkatError("NOT_CONNECTED", "Client not connected")
        if open_method == "mob/stream_open" and "agent_identity" in params:
            result = await self._request(open_method, params)
        else:
            result = await self._request(open_method, params)
        stream_id = str(result.get("stream_id", ""))
        if not stream_id:
            raise MeerkatError(
                "INVALID_RESPONSE", f"{open_method} did not return stream_id"
            )
        queue = self._dispatcher.subscribe_stream(stream_id)

        async def _close_remote(active_stream_id: str) -> None:
            # Await stream-close authority before tearing down local dispatch:
            # a rejected close must leave the subscription delivering events.
            await self._request(close_method, {"stream_id": active_stream_id})
            self._dispatcher.unsubscribe_stream(active_stream_id)

        return EventSubscription(
            stream_id=stream_id,
            event_queue=queue,
            dispatcher=self._dispatcher,
            close_remote=_close_remote,
            parse_event_fn=parser,
        )

    @staticmethod
    def _parse_agent_event_envelope(raw: dict[str, Any]) -> EventEnvelope:
        # Only parse a payload that is actually present as an object. An absent
        # or non-object payload leaves `payload=None` (mirrors the TS envelope
        # parser) rather than synthesizing a typeless frame that the
        # fail-closed `parse_event` would (correctly) reject.
        payload = raw.get("payload")
        return EventEnvelope(
            event_id=str(raw.get("event_id", "")),
            source=MeerkatClient._parse_event_source_identity(raw.get("source")),
            seq=int(raw.get("seq", 0)),
            timestamp_ms=int(raw.get("timestamp_ms", 0)),
            payload=parse_event(payload) if isinstance(payload, dict) else None,
        )

    @staticmethod
    def _parse_event_source_identity(raw: Any) -> EventSourceIdentity | None:
        if not isinstance(raw, dict):
            return None
        source_type = raw.get("type")
        if source_type == "session":
            session_id = raw.get("session_id", raw.get("sessionId"))
            return (
                EventSourceIdentity(type="session", session_id=session_id)
                if isinstance(session_id, str)
                else None
            )
        if source_type == "runtime":
            runtime_id = raw.get("runtime_id", raw.get("runtimeId"))
            return (
                EventSourceIdentity(type="runtime", runtime_id=runtime_id)
                if isinstance(runtime_id, str)
                else None
            )
        if source_type == "interaction":
            interaction_id = raw.get("interaction_id", raw.get("interactionId"))
            return (
                EventSourceIdentity(type="interaction", interaction_id=interaction_id)
                if isinstance(interaction_id, str)
                else None
            )
        if source_type == "callback":
            return EventSourceIdentity(type="callback")
        if source_type == "external":
            source_id = raw.get("source_id", raw.get("sourceId"))
            return (
                EventSourceIdentity(type="external", source_id=source_id)
                if isinstance(source_id, str)
                else None
            )
        return None

    @staticmethod
    def _parse_attributed_mob_event(raw: dict[str, Any]) -> AttributedEvent:
        context = "Invalid attributed mob event"
        # The runtime wire shape (meerkat-mob AttributedEvent) carries a typed
        # source ``{identity, generation}`` record (AgentRuntimeId) and a
        # ``role`` (ProfileName) string — NOT a free-form ``source`` string.
        # Mirror the TypeScript/web parsers: validate the source record
        # (require non-empty ``identity`` + non-negative integer
        # ``generation``) and the ``role`` field, raising on
        # absence/malformation. The public ``AttributedEvent`` dataclass
        # exposes ``source: str``, so the validated ``identity`` is projected
        # into it. Nothing is coalesced to "" — malformed frames fail closed.
        source = MeerkatClient._require_dict(raw.get("source"), "source", context)
        identity = MeerkatClient._require_string_field(source, "identity", context)
        generation = MeerkatClient._require_number_field(source, "generation", context)
        if not isinstance(generation, int) or generation < 0:
            raise MeerkatError(
                "INVALID_RESPONSE",
                f"{context}: source generation must be a non-negative integer",
            )
        return AttributedEvent(
            source=identity,
            role=MeerkatClient._require_string_field(raw, "role", context),
            envelope=MeerkatClient._parse_agent_event_envelope(
                MeerkatClient._require_dict(raw.get("envelope"), "envelope", context)
            ),
            source_fence_token=MeerkatClient._optional_number_field(
                raw, "source_fence_token", context
            ),
        )

    # -- Internal methods used by Session ----------------------------------

    async def _start_turn(
        self,
        session_id: str,
        prompt: str | list[ContentBlock],
        *,
        injected_context: list[str | list[ContentBlock]] | None = None,
        skill_refs: list[SkillRef] | None = None,
        turn_tool_overlay: PublicTurnToolOverlay | None = None,
        additional_instructions: list[str] | None = None,
        keep_alive: bool | None = None,
        model: str | None = None,
        provider: str | None = None,
        self_hosted_server_id: str | None = None,
        max_tokens: int | None = None,
        system_prompt: str | None = None,
        output_schema: dict[str, Any] | None = None,
        structured_output_retries: int | None = None,
        provider_params: dict[str, Any] | None = None,
    ) -> RunResult:
        params: dict[str, Any] = {"session_id": session_id, "prompt": prompt}
        if injected_context is not None:
            params["injected_context"] = injected_context
        wire_refs = _skill_refs_to_wire(skill_refs)
        if wire_refs is not None:
            params["skill_refs"] = wire_refs
        if turn_tool_overlay is not None:
            params["turn_tool_overlay"] = _wire_value(turn_tool_overlay)
        if additional_instructions is not None:
            params["additional_instructions"] = additional_instructions
        if keep_alive is not None:
            params["keep_alive"] = keep_alive
        if model is not None:
            params["model"] = model
        if provider is not None:
            params["provider"] = provider
        if self_hosted_server_id is not None:
            params["self_hosted_server_id"] = self_hosted_server_id
        if max_tokens is not None:
            params["max_tokens"] = max_tokens
        if system_prompt is not None:
            params["system_prompt"] = system_prompt
        if output_schema is not None:
            params["output_schema"] = output_schema
        if structured_output_retries is not None:
            params["structured_output_retries"] = structured_output_retries
        if provider_params is not None:
            params["provider_params"] = provider_params
        raw = await self._request("turn/start", params)
        return self._parse_run_result(raw)

    def _start_turn_streaming(
        self,
        session_id: str,
        prompt: str | list[ContentBlock],
        *,
        injected_context: list[str | list[ContentBlock]] | None = None,
        skill_refs: list[SkillRef] | None = None,
        turn_tool_overlay: PublicTurnToolOverlay | None = None,
        additional_instructions: list[str] | None = None,
        keep_alive: bool | None = None,
        model: str | None = None,
        provider: str | None = None,
        self_hosted_server_id: str | None = None,
        max_tokens: int | None = None,
        system_prompt: str | None = None,
        output_schema: dict[str, Any] | None = None,
        structured_output_retries: int | None = None,
        provider_params: dict[str, Any] | None = None,
        _session: Session | None = None,
    ) -> EventStream:
        if not self._dispatcher or not self._process or not self._process.stdin:
            raise MeerkatError("NOT_CONNECTED", "Client not connected")
        self._request_id += 1
        request_id = self._request_id
        event_queue = self._dispatcher.subscribe_events(session_id)
        response_future = self._dispatcher.expect_response(request_id)
        params: dict[str, Any] = {"session_id": session_id, "prompt": prompt}
        if injected_context is not None:
            params["injected_context"] = injected_context
        wire_refs = _skill_refs_to_wire(skill_refs)
        if wire_refs is not None:
            params["skill_refs"] = wire_refs
        if turn_tool_overlay is not None:
            params["turn_tool_overlay"] = _wire_value(turn_tool_overlay)
        if additional_instructions is not None:
            params["additional_instructions"] = additional_instructions
        if keep_alive is not None:
            params["keep_alive"] = keep_alive
        if model is not None:
            params["model"] = model
        if provider is not None:
            params["provider"] = provider
        if self_hosted_server_id is not None:
            params["self_hosted_server_id"] = self_hosted_server_id
        if max_tokens is not None:
            params["max_tokens"] = max_tokens
        if system_prompt is not None:
            params["system_prompt"] = system_prompt
        if output_schema is not None:
            params["output_schema"] = output_schema
        if structured_output_retries is not None:
            params["structured_output_retries"] = structured_output_retries
        if provider_params is not None:
            params["provider_params"] = provider_params
        request = {
            "jsonrpc": "2.0",
            "id": request_id,
            "method": "turn/start",
            "params": params,
        }
        data = (json.dumps(request) + "\n").encode()
        return EventStream(
            session_id=session_id,
            event_queue=event_queue,
            response_future=response_future,
            dispatcher=self._dispatcher,
            parse_result=self._parse_run_result,
            pending_send=(self._process.stdin, data),
            session=_session,
        )

    async def _interrupt(self, session_id: str) -> InterruptResult:
        _rpc_signature: RpcInterruptParams
        return await self._request("turn/interrupt", {"session_id": session_id})

    async def _archive(self, session_id: str) -> None:
        _rpc_signature: RpcArchiveSessionParams
        await self._request("session/archive", {"session_id": session_id})

    async def _send(self, session_id: str, **kwargs: Any) -> CommsSendResult:
        return await self.send(session_id, **kwargs)

    async def _peers(self, session_id: str) -> dict[str, Any]:
        return await self.peers(session_id)

    # Typed literal aliases mirror the Rust `CommsCommandRequest` enum
    # (`meerkat-core/src/comms.rs`). Invalid discriminator values are rejected
    # at the server's typed-serde boundary — these aliases document the
    # closed-world shape for callers.
    _CommsKind = Literal[
        "input", "peer_message", "peer_lifecycle", "peer_request", "peer_response"
    ]
    _HandlingMode = Literal["queue", "steer"]
    _InputSource = Literal["tcp", "uds", "stdin", "webhook", "rpc"]
    _InputStreamMode = Literal["none", "reserve_interaction"]
    _ResponseStatus = Literal["accepted", "completed", "failed"]
    _PeerLifecycleKind = Literal[
        "mob.peer_added", "mob.peer_retired", "mob.peer_unwired"
    ]

    async def send(
        self,
        session_id: str,
        *,
        kind: "MeerkatClient._CommsKind",
        to: str | None = None,
        body: str | None = None,
        blocks: list[ContentBlock] | None = None,
        lifecycle_kind: "MeerkatClient._PeerLifecycleKind | None" = None,
        intent: str | None = None,
        params: dict[str, Any] | None = None,
        in_reply_to: str | None = None,
        status: "MeerkatClient._ResponseStatus | None" = None,
        result: Any = None,
        source: "MeerkatClient._InputSource | None" = None,
        stream: "MeerkatClient._InputStreamMode | None" = None,
        allow_self_session: bool | None = None,
        handling_mode: "MeerkatClient._HandlingMode | None" = None,
    ) -> CommsSendResult:
        """Send a typed comms command.

        Mirrors the Rust `CommsCommandRequest` variants:
        - ``kind="input"`` — inject input into the local session.
        - ``kind="peer_message"`` — fire-and-forget peer message.
        - ``kind="peer_lifecycle"`` — one-way peer topology notification.
        - ``kind="peer_request"`` — request/response with correlated reply.
        - ``kind="peer_response"`` — reply to a prior peer request.

        Invalid discriminator values (``source``, ``stream``, ``handling_mode``,
        ``status``) are rejected at the server's typed-serde boundary.
        """
        fields: dict[str, Any] = {
            "to": to,
            "body": body,
            "blocks": blocks,
            "lifecycle_kind": lifecycle_kind,
            "intent": intent,
            "params": params,
            "in_reply_to": in_reply_to,
            "status": status,
            "result": result,
            "source": source,
            "stream": stream,
            "allow_self_session": allow_self_session,
            "handling_mode": handling_mode,
        }
        payload: dict[str, Any] = {"session_id": session_id, "kind": kind}
        payload.update({k: v for k, v in fields.items() if v is not None})
        result = await self._request("comms/send", payload)
        return self._parse_comms_send_result(result)

    async def peers(self, session_id: str) -> dict[str, Any]:
        result = await self._request("comms/peers", {"session_id": session_id})
        return self._parse_comms_peers_result(result)

    @staticmethod
    def _retired_runtime_session_control_error() -> MeerkatError:
        return MeerkatError(
            "METHOD_NOT_FOUND",
            "Retired runtime session control methods are no longer supported by the public RPC surface.",
        )

    @staticmethod
    def _warn_retired_runtime_session_control() -> None:
        warnings.warn(
            "Retired runtime session control methods are deprecated and always fail before transport.",
            DeprecationWarning,
            stacklevel=2,
        )

    async def runtime_status(self, session_id: str) -> None:
        self._warn_retired_runtime_session_control()
        raise self._retired_runtime_session_control_error()

    async def runtime_submit(
        self, session_id: str, input: dict[str, Any] | ContentInput
    ) -> None:
        self._warn_retired_runtime_session_control()
        raise self._retired_runtime_session_control_error()

    async def runtime_submission(self, session_id: str, input_id: str) -> None:
        self._warn_retired_runtime_session_control()
        raise self._retired_runtime_session_control_error()

    async def runtime_submissions(self, session_id: str) -> None:
        self._warn_retired_runtime_session_control()
        raise self._retired_runtime_session_control_error()

    async def runtime_retire(self, session_id: str) -> None:
        self._warn_retired_runtime_session_control()
        raise self._retired_runtime_session_control_error()

    async def runtime_reset(self, session_id: str) -> None:
        self._warn_retired_runtime_session_control()
        raise self._retired_runtime_session_control_error()

    # ---- Live multimodal adapter (`live/*`) ---------------------------
    #
    # Typed wrappers over the `live/*` JSON-RPC surface (enabled when the
    # server configures at least one live transport). Result shapes come from `meerkat-contracts`
    # (regenerated into `meerkat/generated/types.py` as `LiveOpenResult`,
    # `LiveStatusResult`, etc.). I52 + I53.
    #
    # CC5/CC6: `capabilities` and `continuity` on `LiveOpenResult` are now
    # typed wire mirrors:
    #   * `result["capabilities"]` is shaped like
    #     `meerkat.types.WireLiveChannelCapabilities` — a dict with typed
    #     boolean fields (`image_in`, `video_in`, `transcript_supported`,
    #     `barge_in_supported`, `provider_native_resume`, `audio_in`,
    #     `audio_out`, `text_in`, `text_out`).
    #   * `result["continuity"]` is shaped like
    #     `meerkat.types.WireLiveContinuityMode` — a tagged dict with
    #     `mode: "fresh" | "transcript_only" | "degraded" | "provider_native_resume"`.
    #     The `provider_native_resume` variant additionally carries
    #     `provider_session_id: str`.
    # Consumers route on `result["continuity"]["mode"]` and read
    # capability booleans by name. Static type checkers (mypy / pyright)
    # validate field access via the regenerated types in
    # `meerkat.generated.types`.

    async def live_open(
        self,
        session_id: str,
        turning_mode: Literal["provider_managed", "explicit_commit"] | None = None,
        transport: Literal["websocket", "webrtc"] | None = None,
        seed_max_chars: int | None = None,
    ) -> dict[str, Any]:
        """Open a live audio/text channel with model-gated image input.

        Wraps `live/open`.

        Returns a dict shaped like `meerkat.types.LiveOpenResult` with keys
        `channel_id`, `transport`, `capabilities`, and `continuity`. The
        `capabilities` value is shaped like
        `meerkat.types.WireLiveChannelCapabilities` (typed booleans:
        `audio_in`, `audio_out`, `text_in`, `text_out`, `image_in`,
        `video_in`, `transcript_supported`, `barge_in_supported`,
        `provider_native_resume`). The `continuity` value is shaped like
        `meerkat.types.WireLiveContinuityMode` (tagged on `mode`; the
        `provider_native_resume` variant additionally carries
        `provider_session_id`). CC5/CC6.

        R4-2 (P2): the optional ``turning_mode`` argument selects between
        the provider-managed flow (server VAD owns commits) and the
        explicit-commit flow (client owns commits). The G9 typed text-only
        ``live_commit_input(channel_id, response_modality="text")`` path
        requires ``"explicit_commit"`` because the OpenAI realtime API
        rejects ``input_audio_buffer.commit`` unless the session was
        opened with explicit-commit semantics. Defaults to ``None``,
        which omits the field on the wire — the server treats omitted as
        ``"provider_managed"`` for back-compat.

        ``seed_max_chars`` optionally bounds serialized provider seed messages
        and requests a core-owned, whole-turn suffix. Omitting it preserves
        the full canonical seed. The resolved root prompt is never dropped
        and must fit; an existing compaction summary may be retained as the
        head. Any truncation is reported as degraded continuity. Runtime
        system context and canonical image identity, tombstone, and accounting
        sidecars remain complete outside the message window. When provided,
        the value must be positive; the server rejects zero.
        """
        params: dict[str, Any] = {"session_id": session_id}
        if turning_mode is not None:
            params["turning_mode"] = turning_mode
        if transport is not None:
            params["transport"] = transport
        if seed_max_chars is not None:
            params["seed_max_chars"] = seed_max_chars
        return await self._request("live/open", params)

    async def live_webrtc_answer(
        self,
        channel_id: str,
        token: str,
        offer_sdp: str,
    ) -> dict[str, Any]:
        """Answer a browser-created WebRTC SDP offer for a live channel."""
        return await self._request(
            "live/webrtc/answer",
            {
                "channel_id": channel_id,
                "token": token,
                "offer_sdp": offer_sdp,
            },
        )

    async def live_status(self, channel_id: str) -> dict[str, Any]:
        """Get the status of a live channel. Wraps `live/status`.

        Returns the `LiveStatusResult` shape: `channel_id`, `status`.
        """
        return await self._request("live/status", {"channel_id": channel_id})

    async def live_close(self, channel_id: str) -> LiveCloseResult:
        """Close a live channel. Wraps `live/close`."""
        raw = await self._request("live/close", {"channel_id": channel_id})
        context = "Invalid live/close response"
        raw = self._require_dict(raw, "result", context)
        status = self._require_string_field(raw, "status", context)
        if status != "closed":
            raise MeerkatError(
                "INVALID_RESPONSE",
                f"{context}: unsupported status {status!r}",
            )
        return LiveCloseResult(
            status=cast("LiveCloseStatus", status),
        )

    async def live_send_input_text(self, channel_id: str, text: str) -> dict[str, Any]:
        """Send a text input chunk to a live channel.

        Wraps `live/send_input` with `LiveSendInputParams { channel_id,
        chunk: LiveInputChunkWire::Text { text } }` (the nested-shape form
        per `BREAKING_LIVE_WIRE_FORMAT_V1`).
        """
        return await self._request(
            "live/send_input",
            {
                "channel_id": channel_id,
                "chunk": {"kind": "text", "text": text},
            },
        )

    async def live_send_input_audio(
        self,
        channel_id: str,
        data_base64: str,
        sample_rate_hz: int,
        channels: int,
    ) -> dict[str, Any]:
        """Send a base64-encoded audio chunk to a live channel.

        Wraps `live/send_input` with `LiveSendInputParams { channel_id,
        chunk: LiveInputChunkWire::Audio { data, sample_rate_hz, channels } }`.
        Caller is responsible for encoding the PCM payload as base64; mismatched
        sample rates or channel counts will be rejected by the adapter.
        """
        return await self._request(
            "live/send_input",
            {
                "channel_id": channel_id,
                "chunk": {
                    "kind": "audio",
                    "data": data_base64,
                    "sample_rate_hz": sample_rate_hz,
                    "channels": channels,
                },
            },
        )

    async def live_send_input_image(
        self,
        channel_id: str,
        idempotency_key: str,
        mime: str,
        data_base64: str,
    ) -> dict[str, Any]:
        """Queue a base64-encoded image input chunk on a live channel.

        Wraps `live/send_input` with `LiveSendInputParams { channel_id,
        chunk: LiveInputChunkWire::Image { idempotency_key, mime, data } }`.
        ``idempotency_key`` is a required caller-stable, session-scoped key:
        retrying the same key with the same MIME and bytes is safe, while
        reusing it for different content is rejected. ``mime`` is the IANA
        MIME type; adapter support is provider-specific, and OpenAI Realtime
        currently accepts ``image/png`` and ``image/jpeg``. ``data_base64``
        is the raw image bytes encoded as standard base64.

        A successful result only proves adapter-queue acceptance. A scoped
        adapter/provider rejection may arrive later as ``command_rejected``;
        persistence failure is terminal. Wait for ``user_content_committed``
        carrying the same ``idempotency_key`` before treating the image as
        durable context.

        Check ``live_open(...)["capabilities"]["image_in"]`` before
        sending; adapters or model bindings without image input reject with
        the typed ``LiveAdapterErrorCode::ConfigRejected { reason:
        "image_input_not_implemented" }``.
        """
        return await self._request(
            "live/send_input",
            {
                "channel_id": channel_id,
                "chunk": {
                    "kind": "image",
                    "idempotency_key": idempotency_key,
                    "mime": mime,
                    "data": data_base64,
                },
            },
        )

    async def live_send_input_video_frame(
        self,
        channel_id: str,
        codec: str,
        data_base64: str,
        timestamp_ms: int,
    ) -> dict[str, Any]:
        """Send a base64-encoded video-frame input chunk to a live channel.

        Wraps `live/send_input` with `LiveSendInputParams { channel_id,
        chunk: LiveInputChunkWire::VideoFrame { codec, data,
        timestamp_ms } }`. ``codec`` is the frame encoding (``vp8``,
        ``vp9``, ``h264``, ``image/jpeg`` for keyframe-as-image transports,
        …); ``timestamp_ms`` is the presentation timestamp the adapter
        will stamp into the provider envelope so frames remain ordered.
        Adapters that do not implement video input (today: every provider
        Meerkat ships) reject with the typed
        ``LiveAdapterErrorCode::ConfigRejected { reason:
        "video_frame_input_not_implemented" }``.
        """
        return await self._request(
            "live/send_input",
            {
                "channel_id": channel_id,
                "chunk": {
                    "kind": "video_frame",
                    "codec": codec,
                    "data": data_base64,
                    "timestamp_ms": timestamp_ms,
                },
            },
        )

    async def live_commit_input(
        self,
        channel_id: str,
        response_modality: Literal["audio", "text"] | None = None,
    ) -> dict[str, Any]:
        """Commit any buffered input on a live channel. Wraps `live/commit_input`.

        G9 (P2): the optional ``response_modality`` argument lets the caller
        request a text-only response on an audio-first channel without
        flipping the channel-wide modality. Pass ``"audio"`` (channel
        default for OpenAI realtime) or ``"text"`` to suppress audio output
        for this single response. ``None`` keeps the channel default.
        """
        params: dict[str, Any] = {"channel_id": channel_id}
        if response_modality is not None:
            if response_modality not in ("audio", "text"):
                raise MeerkatError(
                    "INVALID_ARGS",
                    "response_modality must be 'audio' or 'text', "
                    f"got {response_modality!r}",
                )
            params["response_modality"] = {"modality": response_modality}
        return await self._request("live/commit_input", params)

    async def live_interrupt(self, channel_id: str) -> dict[str, Any]:
        """Interrupt the in-progress assistant turn on a live channel.

        Wraps `live/interrupt`.
        """
        return await self._request("live/interrupt", {"channel_id": channel_id})

    async def live_truncate(
        self,
        channel_id: str,
        item_id: str,
        content_index: int,
        audio_played_ms: int,
    ) -> dict[str, Any]:
        """Truncate the assistant output on a live channel at the
        client-tracked playback cursor. Wraps `live/truncate`.
        """
        return await self._request(
            "live/truncate",
            {
                "channel_id": channel_id,
                "item_id": item_id,
                "content_index": content_index,
                "audio_played_ms": audio_played_ms,
            },
        )

    async def live_refresh(self, channel_id: str) -> LiveRefreshResult:
        """Apply mutable session config (instructions / tools / audio) to an
        open live channel. Wraps ``live/refresh``.

        **Does NOT replay canonical history.** Refresh enqueues a single
        ``session.update`` carrying the latest projection snapshot's mutable
        config fields. History seeding is owned by ``live/open``; refresh is
        config-only by design — re-seeding the transcript on every refresh
        would compound the provider conversation by N+1 every call.

        **Identity changes require close + reopen.** Refresh validates that
        ``model_id`` and ``provider_id`` match the channel's currently-open
        identity and rejects swaps with a typed adapter-level error — the
        OpenAI Realtime API does not accept a mutable ``model`` on
        ``session.update``.

        R4-5 (P3): the result is a typed
        :class:`meerkat.generated.types.LiveRefreshResult` carrying the
        generated-authority ``status`` discriminator (today: ``"queued"``).
        This SDK build accepts the generated status set it was built with and
        rejects missing or unknown status values until regenerated for a newer
        contract. Refresh completion is asynchronous (the adapter pump applies
        the ``session.update`` after the host accepts the queued command); the
        realtime stream is the source of truth for the actual outcome (failures
        surface as
        :class:`meerkat.types.WireLiveAdapterObservation` ``error``).
        """
        raw = await self._request(
            "live/refresh",
            {"channel_id": channel_id},
        )
        context = "Invalid live/refresh response"
        raw = self._require_dict(raw, "result", context)
        status = self._require_string_field(raw, "status", context)
        if status != "queued":
            raise MeerkatError(
                "INVALID_RESPONSE",
                f"{context}: unsupported status {status!r}",
            )
        return LiveRefreshResult(
            status=cast("LiveRefreshStatus", status),
        )

    @staticmethod
    def parse_live_observation(raw: dict[str, Any]) -> dict[str, Any]:
        """Type-narrow an inbound live-adapter observation against
        :class:`meerkat.types.WireLiveAdapterObservation`.

        Observations are streamed as JSON objects on the live-channel WS
        transport returned by ``live/open``. Each object carries an
        internally-tagged ``observation`` discriminator (``ready``,
        ``assistant_audio_chunk``, ``command_rejected``, …). The Meerkat
        SDK does not own the WS transport (the browser/Python client opens
        the URL from ``LiveOpenResult.transport`` directly), so this helper
        is a no-op cast that exists so callers can type-narrow inbound
        frames against the regenerated ``WireLiveAdapterObservation``
        TypedDict union without copying the discriminator wiring.

        Example:
            obs = MeerkatClient.parse_live_observation(json.loads(frame))
            if obs["observation"] == "assistant_audio_chunk":
                # Type-narrowed: obs["item_id"], obs["response_id"],
                # obs["content_index"] are all known optional str / int.
                truncate_at(obs.get("item_id"), obs.get("content_index"))
            elif obs["observation"] == "command_rejected":
                # Typed channel-survives error (R5-9).
                handle_rejection(obs["code"], obs["message"])

        FIX-SDK-OBS: closes the verifier gap that left
        ``WireLiveAdapterObservation`` invisible at the SDK boundary.
        Validation is deferred to the static type checker (mypy /
        pyright); the helper raises ``ValueError`` only if the
        ``observation`` discriminator is missing.
        """
        if not isinstance(raw, dict):
            raise ValueError("live observation must be a JSON object")
        if "observation" not in raw:
            raise ValueError("live observation missing `observation` discriminator")
        return raw

    async def mob_ensure_member(
        self, mob_id: str, spec: dict[str, Any]
    ) -> dict[str, Any]:
        """Idempotent spawn: spawns the member or returns the existing entry.

        Wraps `mob/ensure_member`.
        """
        return await self._request(
            "mob/ensure_member",
            {"mob_id": mob_id, "spec": spec},
        )

    async def mob_reconcile(
        self,
        mob_id: str,
        desired: list[dict[str, Any]],
        options: dict[str, Any] | None = None,
    ) -> dict[str, Any]:
        """Declarative reconcile: converge roster to the desired spec list.

        Wraps `mob/reconcile`.
        """
        params: dict[str, Any] = {"mob_id": mob_id, "desired": desired}
        if options is not None:
            params["options"] = options
        return await self._request("mob/reconcile", params)

    async def mob_list_members_matching(
        self, mob_id: str, filter: dict[str, Any]
    ) -> dict[str, Any]:
        """Label-filtered member listing. Wraps `mob/list_members_matching`."""
        return await self._request(
            "mob/list_members_matching",
            {"mob_id": mob_id, "filter": filter},
        )

    # -- Transport ---------------------------------------------------------

    async def _request(self, method: str, params: dict[str, Any]) -> dict[str, Any]:
        if not self._process or not self._process.stdin or not self._dispatcher:
            raise MeerkatError("NOT_CONNECTED", "Client not connected")
        fault = self._dispatcher.transport_fault
        if fault is not None:
            # The read loop has terminated the transport (corrupted frame /
            # closed stream): fail closed with the recorded typed fault instead
            # of writing into a condemned stream and awaiting a response no
            # read loop will deliver.
            raise fault
        self._request_id += 1
        request_id = self._request_id
        request = {
            "jsonrpc": "2.0",
            "id": request_id,
            "method": method,
            "params": params,
        }
        response_future = self._dispatcher.expect_response(request_id)
        self._process.stdin.write((json.dumps(request) + "\n").encode())
        await self._process.stdin.drain()
        return await response_future

    # -- Binary resolution (unchanged from original) -----------------------

    async def _resolve_rkat_binary(self, requested_path: str) -> tuple[str, bool]:
        override = os.environ.get("MEERKAT_BIN_PATH")
        if override:
            override = override.strip()
            if not override:
                raise MeerkatError(
                    "BINARY_NOT_FOUND", "MEERKAT_BIN_PATH is set to an empty path."
                )
            candidate = self._resolve_candidate_path(override)
            if candidate is None:
                raise MeerkatError(
                    "BINARY_NOT_FOUND",
                    f"Binary not found at MEERKAT_BIN_PATH='{override}'. Set MEERKAT_BIN_PATH to a valid executable.",
                )
            return str(candidate), Path(candidate).name == "rkat"

        if requested_path != _MEERKAT_BINARY:
            candidate = self._resolve_candidate_path(requested_path)
            if candidate is None:
                raise MeerkatError(
                    "BINARY_NOT_FOUND",
                    f"Binary not found at '{requested_path}'. Set rkat_path to a valid path or use MEERKAT_BIN_PATH.",
                )
            return str(candidate), Path(candidate).name == "rkat"

        candidate = self._resolve_candidate_path(_MEERKAT_BINARY)
        if candidate is not None:
            return str(candidate), False

        try:
            cached = await self._download_rkat_rpc_binary()
        except MeerkatError:
            if shutil.which("rkat"):
                return "rkat", True
            raise

        if cached is not None:
            return cached, False

        if shutil.which("rkat"):
            return "rkat", True

        raise MeerkatError(
            "BINARY_NOT_FOUND",
            f"Could not find '{_MEERKAT_BINARY}' on PATH and auto-download failed."
            " Set MEERKAT_BIN_PATH to a local executable.",
        )

    @staticmethod
    def _resolve_candidate_path(command_or_path: str) -> str | None:
        if "/" in command_or_path or "\\" in command_or_path:
            candidate = Path(command_or_path).expanduser()
            if candidate.is_file():
                return str(candidate)
            return None
        return shutil.which(command_or_path)

    @staticmethod
    def _platform_target() -> tuple[str, str, str]:
        system = platform.system().lower()
        machine = platform.machine().lower()
        if system == "darwin":
            target = {
                "arm64": "aarch64-apple-darwin",
                "aarch64": "aarch64-apple-darwin",
            }.get(machine)
            if target is None:
                raise MeerkatError(
                    "UNSUPPORTED_PLATFORM",
                    f"Unsupported macOS architecture '{machine}'",
                )
            return target, "tar.gz", _MEERKAT_BINARY
        if system == "linux":
            if machine in {"x86_64", "amd64"}:
                return "x86_64-unknown-linux-gnu", "tar.gz", _MEERKAT_BINARY
            if machine in {"aarch64", "arm64"}:
                return "aarch64-unknown-linux-gnu", "tar.gz", _MEERKAT_BINARY
            raise MeerkatError(
                "UNSUPPORTED_PLATFORM", f"Unsupported Linux architecture '{machine}'."
            )
        if system == "windows":
            if machine in {"x86_64", "amd64"}:
                return "x86_64-pc-windows-msvc", "zip", f"{_MEERKAT_BINARY}.exe"
            raise MeerkatError(
                "UNSUPPORTED_PLATFORM", f"Unsupported Windows architecture '{machine}'."
            )
        raise MeerkatError("UNSUPPORTED_PLATFORM", f"Unsupported platform '{system}'.")

    @staticmethod
    async def _download_rkat_rpc_binary() -> str | None:
        target, archive_ext, binary_name = MeerkatClient._platform_target()
        owner, repo = _MEERKAT_REPO
        version = CONTRACT_VERSION
        artifact = f"{_MEERKAT_BINARY}-v{version}-{target}.{archive_ext}"
        url = (
            f"https://github.com/{owner}/{repo}/releases/download/v{version}/{artifact}"
        )
        cache_dir = _MEERKAT_BINARY_CACHE_ROOT / f"v{version}" / target
        cache_dir.mkdir(parents=True, exist_ok=True)
        cached = cache_dir / binary_name
        if cached.exists():
            return str(cached)

        try:
            with urllib.request.urlopen(url, timeout=30) as response:
                payload = response.read()
        except (URLError, OSError) as exc:
            raise MeerkatError(
                "BINARY_DOWNLOAD_FAILED",
                f"Failed to download {_MEERKAT_BINARY} from {url}: {exc}",
            ) from exc

        with tempfile.TemporaryDirectory() as tmpdir:
            archive_path = Path(tmpdir) / f"{_MEERKAT_BINARY}.{archive_ext}"
            with archive_path.open("wb") as fp:
                fp.write(payload)

            temp_binary = cache_dir / f".{binary_name}.download"
            if archive_ext == "tar.gz":
                with tarfile.open(archive_path, "r:gz") as archive:
                    members = [
                        m
                        for m in archive.getmembers()
                        if Path(m.name).name == binary_name
                    ]
                    if not members:
                        raise MeerkatError(
                            "BINARY_DOWNLOAD_FAILED",
                            f"Downloaded archive does not contain '{binary_name}'.",
                        )
                    source = archive.extractfile(members[0])
                    if source is None:
                        raise MeerkatError(
                            "BINARY_DOWNLOAD_FAILED",
                            f"Could not extract '{binary_name}' from {artifact}.",
                        )
                    with source, temp_binary.open("wb") as out:
                        out.write(source.read())
            else:
                with zipfile.ZipFile(archive_path, "r") as archive:
                    candidate_names = [entry.filename for entry in archive.infolist()]
                    if not any(
                        Path(name).name == binary_name for name in candidate_names
                    ):
                        raise MeerkatError(
                            "BINARY_DOWNLOAD_FAILED",
                            f"Downloaded archive does not contain '{binary_name}'.",
                        )
                    with (
                        archive.open(
                            next(
                                name
                                for name in candidate_names
                                if Path(name).name == binary_name
                            ),
                        ) as source,
                        temp_binary.open("wb") as out,
                    ):
                        out.write(source.read())

            if os.name != "nt":
                temp_binary.chmod(0o755)
            temp_binary.replace(cached)
            return str(cached)

    # -- Helpers -----------------------------------------------------------

    @staticmethod
    def _build_args(
        legacy: bool,
        *,
        isolated: bool,
        realm_id: str | None,
        instance_id: str | None,
        realm_backend: str | None,
        state_root: str | None,
        context_root: str | None,
        user_config_root: str | None,
        live_ws: bool,
        live_webrtc: bool,
    ) -> list[str]:
        if legacy:
            return ["rpc"]
        args: list[str] = []
        if live_ws:
            args.extend(["--live-ws", "127.0.0.1:0"])
        if live_webrtc:
            args.append("--live-webrtc")
        if isolated:
            args.append("--isolated")
        if realm_id:
            args.extend(["--realm", realm_id])
        if instance_id:
            args.extend(["--instance", instance_id])
        if realm_backend:
            args.extend(["--realm-backend", realm_backend])
        if state_root:
            args.extend(["--state-root", state_root])
        if context_root:
            args.extend(["--context-root", context_root])
        if user_config_root:
            args.extend(["--user-config-root", user_config_root])
        return args

    @staticmethod
    def _build_create_params(
        prompt: str | list[ContentBlock],
        *,
        injected_context: list[str | list[ContentBlock]] | None = None,
        model: str | None = None,
        provider: str | None = None,
        auth_binding: dict[str, str] | None = None,
        system_prompt: SystemPromptOverride | None = None,
        max_tokens: int | None = None,
        output_schema: dict[str, Any] | None = None,
        structured_output_retries: int | None = None,
        hooks_override: dict[str, Any] | None = None,
        enable_builtins: bool | None = None,
        enable_shell: bool | None = None,
        enable_memory: bool | None = None,
        enable_schedule: bool | None = None,
        enable_workgraph: bool | None = None,
        enable_mob: bool | None = None,
        enable_web_search: bool | None = None,
        keep_alive: bool | None = None,
        comms_name: str | None = None,
        peer_meta: dict[str, Any] | None = None,
        budget_limits: dict[str, Any] | None = None,
        provider_params: dict[str, Any] | None = None,
        preload_skills: list[SkillRef] | None = None,
        skill_refs: list[SkillRef] | None = None,
        labels: dict[str, str] | None = None,
        additional_instructions: list[str] | None = None,
        app_context: dict[str, Any] | None = None,
        shell_env: dict[str, str] | None = None,
        external_tools: list[dict[str, Any]] | None = None,
    ) -> dict[str, Any]:
        params: dict[str, Any] = {"prompt": prompt}
        if injected_context is not None:
            params["injected_context"] = injected_context
        if model:
            params["model"] = model
        if provider:
            params["provider"] = provider
        if auth_binding is not None:
            params["auth_binding"] = auth_binding
        if system_prompt is not None:
            params["system_prompt"] = system_prompt
        if max_tokens:
            params["max_tokens"] = max_tokens
        if output_schema is not None:
            params["output_schema"] = output_schema
        if structured_output_retries is not None:
            params["structured_output_retries"] = structured_output_retries
        if hooks_override is not None:
            params["hooks_override"] = hooks_override
        if enable_builtins is not None:
            params["enable_builtins"] = enable_builtins
        if enable_shell is not None:
            params["enable_shell"] = enable_shell
        if enable_memory is not None:
            params["enable_memory"] = enable_memory
        if enable_schedule is not None:
            params["enable_schedule"] = enable_schedule
        if enable_workgraph is not None:
            params["enable_workgraph"] = enable_workgraph
        if enable_mob is not None:
            params["enable_mob"] = enable_mob
        if enable_web_search is not None:
            params["enable_web_search"] = enable_web_search
        if keep_alive is not None:
            params["keep_alive"] = keep_alive
        if comms_name:
            params["comms_name"] = comms_name
        if peer_meta is not None:
            params["peer_meta"] = peer_meta
        if budget_limits is not None:
            params["budget_limits"] = budget_limits
        if provider_params is not None:
            params["provider_params"] = provider_params
        if preload_skills is not None:
            params["preload_skills"] = _skill_keys_to_wire(preload_skills)
        wire_refs = _skill_refs_to_wire(skill_refs)
        if wire_refs is not None:
            params["skill_refs"] = wire_refs
        if labels is not None:
            params["labels"] = labels
        if additional_instructions is not None:
            params["additional_instructions"] = additional_instructions
        if app_context is not None:
            params["app_context"] = app_context
        if shell_env is not None:
            params["shell_env"] = shell_env
        if external_tools is not None:
            params["external_tools"] = external_tools
        return params

    @staticmethod
    def _parse_wire_capability_status(raw: Any, context: str) -> str:
        # CapabilityStatus is an externally-tagged Rust enum: either the bare
        # string "Available", or a single-key dict like
        # {"DisabledByPolicy": {...}} / {"NotCompiled": {...}} /
        # {"NotSupportedByProtocol": {...}}. We do NOT whitelist variants (the
        # capability vocabulary evolves), but we fail closed on an absent or
        # otherwise unparseable status rather than fabricating a permissive
        # "Available" default.
        if isinstance(raw, str) and raw:
            return raw
        if isinstance(raw, dict) and raw:
            first_key = next(iter(raw), None)
            if isinstance(first_key, str) and first_key:
                return first_key
        raise MeerkatError(
            "INVALID_RESPONSE",
            f"{context}: missing or invalid capability status",
        )

    @staticmethod
    def _parse_wire_handling_mode(raw: Any, context: str) -> str:
        if isinstance(raw, str) and raw in {"queue", "steer"}:
            return raw
        raise MeerkatError("INVALID_RESPONSE", context)

    @staticmethod
    def _parse_wire_respawn_outcome(raw: Any, context: str) -> str:
        if isinstance(raw, str) and raw in {"completed", "topology_restore_failed"}:
            return raw
        raise MeerkatError("INVALID_RESPONSE", context)

    @staticmethod
    def _parse_wire_append_system_context_status(raw: Any, context: str) -> str:
        if isinstance(raw, str) and raw in {"staged", "duplicate"}:
            return raw
        raise MeerkatError("INVALID_RESPONSE", context)

    @staticmethod
    def _parse_mob_peer_connectivity_snapshot(
        raw: Any, context: str
    ) -> MobPeerConnectivitySnapshot:
        if not isinstance(raw, dict):
            raise MeerkatError(
                "INVALID_RESPONSE",
                f"{context}: peer_connectivity.snapshot must be object",
            )
        unreachable_raw = raw.get("unreachable_peers")
        unreachable_peers: list[MobUnreachablePeer] = []
        if unreachable_raw is not None:
            if not isinstance(unreachable_raw, list):
                raise MeerkatError(
                    "INVALID_RESPONSE",
                    f"{context}: peer_connectivity.snapshot.unreachable_peers must be array",
                )
            for entry in unreachable_raw:
                if not isinstance(entry, dict):
                    raise MeerkatError(
                        "INVALID_RESPONSE",
                        f"{context}: peer_connectivity.snapshot.unreachable_peers "
                        "entry must be object",
                    )
                peer_entry: MobUnreachablePeer = {
                    "peer": MeerkatClient._require_string_field(entry, "peer", context),
                }
                reason = MeerkatClient._optional_string_field(entry, "reason", context)
                if reason is not None:
                    peer_entry["reason"] = reason
                unreachable_peers.append(peer_entry)
        return {
            "reachable_peer_count": MeerkatClient._require_non_negative_integer_field(
                raw,
                "reachable_peer_count",
                context,
                "peer_connectivity.snapshot.reachable_peer_count",
            ),
            "unknown_peer_count": MeerkatClient._require_non_negative_integer_field(
                raw,
                "unknown_peer_count",
                context,
                "peer_connectivity.snapshot.unknown_peer_count",
            ),
            "unreachable_peers": unreachable_peers,
        }

    @staticmethod
    def _parse_mob_peer_connectivity(
        raw: Any, context: str
    ) -> MobPeerConnectivity | None:
        """Fail-closed parser for the tri-state ``peer_connectivity``
        projection. Switches on the variant ``status`` tag and raises on any
        unknown tag rather than coalescing it into a permissive default.
        Absent connectivity (the field was omitted) yields ``None``. Mirrors
        the web SDK ``parseMobPeerConnectivity``.
        """
        if raw is None:
            return None
        if not isinstance(raw, dict):
            raise MeerkatError(
                "INVALID_RESPONSE",
                f"{context}: peer_connectivity must be object",
            )
        status = raw.get("status")
        if status == "not_applicable":
            return {"status": "not_applicable"}
        if status == "probe_timed_out":
            return {"status": "probe_timed_out"}
        if status == "known":
            return {
                "status": "known",
                "snapshot": MeerkatClient._parse_mob_peer_connectivity_snapshot(
                    raw.get("snapshot"), context
                ),
            }
        raise MeerkatError(
            "INVALID_RESPONSE",
            f"{context}: unknown peer_connectivity status {status!r}",
        )

    @staticmethod
    def _require_schedule_literal(
        raw: dict[str, Any],
        field: str,
        allowed: tuple[str, ...],
        context: str,
    ) -> str:
        value = MeerkatClient._require_string_field(raw, field, context)
        if value not in allowed:
            raise MeerkatError(
                "INVALID_RESPONSE",
                f"{context}: invalid {field} {value!r}",
            )
        return value

    @staticmethod
    def _parse_schedule_misfire_policy(raw: Any, context: str) -> dict[str, Any]:
        policy = MeerkatClient._require_dict(raw, "misfire_policy", context)
        kind = MeerkatClient._require_schedule_literal(
            policy, "type", ("skip", "catch_up_within"), context
        )
        if kind == "catch_up_within":
            MeerkatClient._require_non_negative_integer_field(
                policy, "window_seconds", context
            )
        return policy

    @staticmethod
    def _parse_schedule_labels(raw: Any, context: str) -> dict[str, str]:
        if not isinstance(raw, dict):
            raise MeerkatError("INVALID_RESPONSE", f"{context}: labels must be object")
        labels: dict[str, str] = {}
        for key, value in raw.items():
            if not isinstance(key, str) or not isinstance(value, str):
                raise MeerkatError(
                    "INVALID_RESPONSE",
                    f"{context}: labels must contain only string values",
                )
            labels[key] = value
        return labels

    @staticmethod
    def _parse_schedule_string_array(
        raw: dict[str, Any], field: str, context: str
    ) -> list[str]:
        # The public Rust projection uses `serde(default,
        # skip_serializing_if = "BTreeSet::is_empty")`: omission therefore
        # means the canonical empty set, while an explicitly present `null`
        # (or any other non-array shape) is malformed wire data.
        if field not in raw:
            return []
        values = MeerkatClient._require_list_field(raw, field, context)
        parsed: list[str] = []
        for index, value in enumerate(values):
            if not isinstance(value, str) or not value:
                raise MeerkatError(
                    "INVALID_RESPONSE",
                    f"{context}: {field}[{index}] must be string",
                )
            parsed.append(value)
        return parsed

    @staticmethod
    def _parse_schedule_record(raw: Any, context: str) -> ScheduleRecord:
        raw = MeerkatClient._require_dict(raw, "response", context)
        phase = MeerkatClient._require_schedule_literal(
            raw, "phase", ("active", "paused", "deleted"), context
        )
        overlap_policy = MeerkatClient._require_schedule_literal(
            raw,
            "overlap_policy",
            ("allow_concurrent", "skip_if_running"),
            context,
        )
        missing_target_policy = MeerkatClient._require_schedule_literal(
            raw,
            "missing_target_policy",
            ("skip", "mark_misfired"),
            context,
        )
        return cast(
            ScheduleRecord,
            {
                "schedule_id": MeerkatClient._require_string_field(
                    raw, "schedule_id", context
                ),
                "phase": phase,
                "revision": MeerkatClient._require_non_negative_integer_field(
                    raw, "revision", context
                ),
                "trigger": MeerkatClient._require_dict(
                    raw.get("trigger"), "trigger", context
                ),
                "target": MeerkatClient._require_dict(
                    raw.get("target"), "target", context
                ),
                "misfire_policy": MeerkatClient._parse_schedule_misfire_policy(
                    raw.get("misfire_policy"), context
                ),
                "overlap_policy": overlap_policy,
                "missing_target_policy": missing_target_policy,
                "next_occurrence_ordinal": (
                    MeerkatClient._require_non_negative_integer_field(
                        raw, "next_occurrence_ordinal", context
                    )
                ),
                "planning_horizon_days": (
                    MeerkatClient._require_non_negative_integer_field(
                        raw, "planning_horizon_days", context
                    )
                ),
                "planning_horizon_occurrences": (
                    MeerkatClient._require_non_negative_integer_field(
                        raw, "planning_horizon_occurrences", context
                    )
                ),
                "created_at_utc": MeerkatClient._require_string_field(
                    raw, "created_at_utc", context
                ),
                "updated_at_utc": MeerkatClient._require_string_field(
                    raw, "updated_at_utc", context
                ),
                "labels": (
                    MeerkatClient._parse_schedule_labels(raw["labels"], context)
                    if "labels" in raw
                    else {}
                ),
                "superseded_ack_ids": (
                    MeerkatClient._parse_schedule_string_array(
                        raw, "superseded_ack_ids", context
                    )
                ),
                "name": MeerkatClient._optional_string_field(raw, "name", context),
                "description": MeerkatClient._optional_string_field(
                    raw, "description", context
                ),
                "planning_cursor_utc": MeerkatClient._optional_string_field(
                    raw, "planning_cursor_utc", context
                ),
                "deleted_at_utc": MeerkatClient._optional_string_field(
                    raw, "deleted_at_utc", context
                ),
            },
        )

    @staticmethod
    def _parse_schedule_occurrence(raw: Any, context: str) -> ScheduleOccurrenceRecord:
        raw = MeerkatClient._require_dict(raw, "response", context)
        phase = MeerkatClient._require_schedule_literal(
            raw,
            "phase",
            (
                "pending",
                "claimed",
                "dispatching",
                "awaiting_completion",
                "completed",
                "skipped",
                "misfired",
                "superseded",
                "delivery_failed",
            ),
            context,
        )
        overlap_policy = MeerkatClient._require_schedule_literal(
            raw,
            "overlap_policy",
            ("allow_concurrent", "skip_if_running"),
            context,
        )
        missing_target_policy = MeerkatClient._require_schedule_literal(
            raw,
            "missing_target_policy",
            ("skip", "mark_misfired"),
            context,
        )
        superseded_by_revision = raw.get("superseded_by_revision")
        if superseded_by_revision is not None:
            superseded_by_revision = MeerkatClient._require_non_negative_integer_field(
                raw, "superseded_by_revision", context
            )
        return cast(
            ScheduleOccurrenceRecord,
            {
                "occurrence_id": MeerkatClient._require_string_field(
                    raw, "occurrence_id", context
                ),
                "schedule_id": MeerkatClient._require_string_field(
                    raw, "schedule_id", context
                ),
                "schedule_revision": (
                    MeerkatClient._require_non_negative_integer_field(
                        raw, "schedule_revision", context
                    )
                ),
                "occurrence_ordinal": (
                    MeerkatClient._require_non_negative_integer_field(
                        raw, "occurrence_ordinal", context
                    )
                ),
                "phase": phase,
                "due_at_utc": MeerkatClient._require_string_field(
                    raw, "due_at_utc", context
                ),
                "trigger_snapshot": MeerkatClient._require_dict(
                    raw.get("trigger_snapshot"), "trigger_snapshot", context
                ),
                "target_snapshot": MeerkatClient._require_dict(
                    raw.get("target_snapshot"), "target_snapshot", context
                ),
                "misfire_policy": MeerkatClient._parse_schedule_misfire_policy(
                    raw.get("misfire_policy"), context
                ),
                "overlap_policy": overlap_policy,
                "missing_target_policy": missing_target_policy,
                "attempt_count": (
                    MeerkatClient._require_non_negative_integer_field(
                        raw, "attempt_count", context
                    )
                ),
                "created_at_utc": MeerkatClient._require_string_field(
                    raw, "created_at_utc", context
                ),
                "claimed_by": MeerkatClient._optional_string_field(
                    raw, "claimed_by", context
                ),
                "lease_expires_at_utc": MeerkatClient._optional_string_field(
                    raw, "lease_expires_at_utc", context
                ),
                "delivery_correlation_id": (
                    MeerkatClient._optional_string_field(
                        raw, "delivery_correlation_id", context
                    )
                ),
                "last_receipt": MeerkatClient._optional_dict_field(
                    raw, "last_receipt", context
                ),
                "failure_class": MeerkatClient._optional_string_field(
                    raw, "failure_class", context
                ),
                "runtime_outcome": MeerkatClient._optional_dict_field(
                    raw, "runtime_outcome", context
                ),
                "failure_detail": MeerkatClient._optional_string_field(
                    raw, "failure_detail", context
                ),
                "claimed_at_utc": MeerkatClient._optional_string_field(
                    raw, "claimed_at_utc", context
                ),
                "dispatched_at_utc": MeerkatClient._optional_string_field(
                    raw, "dispatched_at_utc", context
                ),
                "completed_at_utc": MeerkatClient._optional_string_field(
                    raw, "completed_at_utc", context
                ),
                "superseded_by_revision": superseded_by_revision,
            },
        )

    @staticmethod
    def _parse_mob_grant_record(
        raw: dict[str, Any],
        context: str,
    ) -> MobGrantRecord:
        principal = MeerkatClient._require_string_field(raw, "principal", context)
        scopes_raw = MeerkatClient._require_present_list_field(raw, "scopes", context)
        scopes: list[MobControlScope] = []
        for index, scope in enumerate(scopes_raw):
            if not isinstance(scope, str) or scope not in _MOB_CONTROL_SCOPES:
                raise MeerkatError(
                    "INVALID_RESPONSE",
                    f"{context}: scopes[{index}] is outside the control-scope vocabulary",
                )
            scopes.append(cast(MobControlScope, scope))
        expires_at_ms: int | None = None
        if raw.get("expires_at_ms") is not None:
            expires_at_ms = MeerkatClient._require_non_negative_integer_field(
                raw,
                "expires_at_ms",
                context,
            )
        return {
            "principal": principal,
            "scopes": scopes,
            **({"expires_at_ms": expires_at_ms} if expires_at_ms is not None else {}),
        }

    @staticmethod
    def _require_present_field(
        raw: dict[str, Any],
        field: str,
        context: str,
    ) -> Any:
        if field not in raw:
            raise MeerkatError("INVALID_RESPONSE", f"{context}: missing {field}")
        return raw[field]

    @staticmethod
    def _validate_optional_response_string(
        raw: dict[str, Any],
        field: str,
        context: str,
    ) -> None:
        if field in raw and not isinstance(raw[field], str):
            raise MeerkatError(
                "INVALID_RESPONSE",
                f"{context}: {field} must be a string",
            )

    @staticmethod
    def _validate_nullable_response_string(
        raw: dict[str, Any],
        field: str,
        context: str,
    ) -> None:
        if field in raw and raw[field] is not None and not isinstance(raw[field], str):
            raise MeerkatError(
                "INVALID_RESPONSE",
                f"{context}: {field} must be a string or null",
            )

    @staticmethod
    def _validate_optional_response_bool(
        raw: dict[str, Any],
        field: str,
        context: str,
    ) -> None:
        if field in raw and not isinstance(raw[field], bool):
            raise MeerkatError(
                "INVALID_RESPONSE",
                f"{context}: {field} must be boolean",
            )

    @staticmethod
    def _validate_wire_content_block(
        raw: Any,
        context: str,
        *,
        system_notice_content: bool = False,
    ) -> dict[str, Any]:
        block = MeerkatClient._require_dict(raw, "content block", context)
        block_type = MeerkatClient._require_string_field(block, "type", context)
        allowed = (
            {"text", "image", "video", "structured", "skill_context"}
            if system_notice_content
            else {"text", "image", "video", "structured", "unknown"}
        )
        if block_type not in allowed:
            raise MeerkatError(
                "INVALID_RESPONSE",
                f"{context}: unsupported content block type {block_type!r}",
            )

        if block_type == "text":
            MeerkatClient._require_present_string_field(block, "text", context)
        elif block_type == "image":
            MeerkatClient._require_present_string_field(block, "media_type", context)
            source = MeerkatClient._require_string_field(block, "source", context)
            if source == "inline":
                MeerkatClient._require_present_string_field(block, "data", context)
            elif source == "blob":
                MeerkatClient._require_string_field(block, "blob_id", context)
            else:
                raise MeerkatError(
                    "INVALID_RESPONSE",
                    f"{context}: unsupported image source {source!r}",
                )
        elif block_type == "video":
            MeerkatClient._require_present_string_field(block, "media_type", context)
            MeerkatClient._require_non_negative_integer_field(
                block,
                "duration_ms",
                context,
            )
            source = MeerkatClient._require_string_field(block, "source", context)
            if source == "inline":
                MeerkatClient._require_present_string_field(block, "data", context)
            elif source == "uri":
                MeerkatClient._require_string_field(block, "uri", context)
            else:
                raise MeerkatError(
                    "INVALID_RESPONSE",
                    f"{context}: unsupported video source {source!r}",
                )
        elif block_type == "structured":
            MeerkatClient._require_present_field(block, "data", context)
        elif block_type == "skill_context":
            skill_key = MeerkatClient._require_dict(
                block.get("skill_key"),
                "skill_key",
                context,
            )
            MeerkatClient._require_string_field(
                skill_key,
                "source_uuid",
                f"{context}: skill_key",
            )
            MeerkatClient._require_string_field(
                skill_key,
                "skill_name",
                f"{context}: skill_key",
            )
            MeerkatClient._require_present_string_field(block, "text", context)
        return block

    @staticmethod
    def _validate_tool_config_status(raw: Any, context: str) -> None:
        status = MeerkatClient._require_dict(raw, "status_info", context)
        kind = MeerkatClient._require_string_field(status, "kind", context)
        if kind == "boundary_applied":
            MeerkatClient._require_bool_field(status, "base_changed", context)
            MeerkatClient._require_non_negative_integer_field(
                status,
                "revision",
                context,
            )
            MeerkatClient._require_bool_field(status, "visible_changed", context)
        elif kind == "deferred_catalog_delta":
            for field in (
                "added_hidden_count",
                "pending_source_count",
                "removed_hidden_count",
            ):
                MeerkatClient._require_non_negative_integer_field(
                    status,
                    field,
                    context,
                )
        elif kind == "warning_failed_closed":
            MeerkatClient._require_present_string_field(status, "error", context)
        elif kind == "external_tool_delta":
            phase = MeerkatClient._require_string_field(status, "phase", context)
            if phase not in {"pending", "applied", "draining", "forced", "failed"}:
                raise MeerkatError(
                    "INVALID_RESPONSE",
                    f"{context}: unsupported external-tool phase {phase!r}",
                )
            MeerkatClient._validate_nullable_response_string(status, "detail", context)
        else:
            raise MeerkatError(
                "INVALID_RESPONSE",
                f"{context}: unsupported tool-config status {kind!r}",
            )

    @staticmethod
    def _validate_tool_config_payload(raw: Any, context: str) -> None:
        payload = MeerkatClient._require_dict(raw, "payload", context)
        operation = MeerkatClient._require_string_field(
            payload,
            "operation",
            context,
        )
        if operation not in {"add", "remove", "reload"}:
            raise MeerkatError(
                "INVALID_RESPONSE",
                f"{context}: unsupported tool-config operation {operation!r}",
            )
        MeerkatClient._require_bool_field(payload, "persisted", context)
        MeerkatClient._require_present_string_field(payload, "target", context)
        MeerkatClient._validate_tool_config_status(
            payload.get("status_info"),
            f"{context}: status_info",
        )
        if "applied_at_turn" in payload and payload["applied_at_turn"] is not None:
            MeerkatClient._require_non_negative_integer_field(
                payload,
                "applied_at_turn",
                context,
            )
        if "domain" in payload and payload["domain"] is not None:
            domain = MeerkatClient._require_string_field(payload, "domain", context)
            if domain not in {"tool_scope", "deferred_catalog"}:
                raise MeerkatError(
                    "INVALID_RESPONSE",
                    f"{context}: unsupported tool-config domain {domain!r}",
                )
        if (
            "deferred_catalog_delta" in payload
            and payload["deferred_catalog_delta"] is not None
        ):
            delta = MeerkatClient._require_dict(
                payload["deferred_catalog_delta"],
                "deferred_catalog_delta",
                context,
            )
            for field in (
                "added_hidden_names",
                "pending_sources",
                "removed_hidden_names",
            ):
                if field in delta:
                    MeerkatClient._parse_nonempty_string_list(
                        delta[field],
                        f"{context}: deferred_catalog_delta.{field}",
                    )

    @staticmethod
    def _validate_system_notice_block(raw: Any, context: str) -> None:
        block = MeerkatClient._require_dict(raw, "system notice block", context)
        block_type = MeerkatClient._require_string_field(block, "type", context)
        if block_type not in {
            "comms",
            "external_event",
            "tool_config",
            "mcp",
            "background_job",
            "auth",
            "runtime_notice",
            "unknown",
        }:
            raise MeerkatError(
                "INVALID_RESPONSE",
                f"{context}: unsupported system-notice block {block_type!r}",
            )

        def validate_content() -> None:
            if "content" not in block:
                return
            content = MeerkatClient._require_present_list_field(
                block,
                "content",
                context,
            )
            for index, content_block in enumerate(content):
                MeerkatClient._validate_wire_content_block(
                    content_block,
                    f"{context}: content[{index}]",
                    system_notice_content=True,
                )

        if block_type == "comms":
            direction = MeerkatClient._require_string_field(
                block,
                "direction",
                context,
            )
            if direction not in {"incoming", "outgoing", "internal"}:
                raise MeerkatError(
                    "INVALID_RESPONSE",
                    f"{context}: unsupported comms direction {direction!r}",
                )
            MeerkatClient._require_present_string_field(block, "kind", context)
            for field in ("intent", "request_id", "status", "summary"):
                MeerkatClient._validate_nullable_response_string(block, field, context)
            if "peer" in block and block["peer"] is not None:
                peer = MeerkatClient._require_dict(block["peer"], "peer", context)
                MeerkatClient._require_string_field(peer, "id", f"{context}: peer")
                MeerkatClient._validate_nullable_response_string(
                    peer,
                    "display_name",
                    f"{context}: peer",
                )
            if "sender_taint" in block and block["sender_taint"] is not None:
                sender_taint = MeerkatClient._require_string_field(
                    block,
                    "sender_taint",
                    context,
                )
                if sender_taint not in {"clean", "tainted"}:
                    raise MeerkatError(
                        "INVALID_RESPONSE",
                        f"{context}: unsupported sender_taint {sender_taint!r}",
                    )
            validate_content()
        elif block_type == "external_event":
            MeerkatClient._require_present_string_field(block, "event_type", context)
            MeerkatClient._require_present_string_field(block, "source", context)
            MeerkatClient._validate_nullable_response_string(block, "body", context)
            MeerkatClient._validate_nullable_response_string(block, "summary", context)
            validate_content()
        elif block_type == "tool_config":
            MeerkatClient._validate_tool_config_payload(
                block.get("payload"),
                f"{context}: payload",
            )
        elif block_type == "mcp":
            for field in ("detail", "server_id"):
                MeerkatClient._validate_nullable_response_string(block, field, context)
            if "operation" in block and block["operation"] is not None:
                operation = MeerkatClient._require_string_field(
                    block,
                    "operation",
                    context,
                )
                if operation not in {"add", "remove", "reload"}:
                    raise MeerkatError(
                        "INVALID_RESPONSE",
                        f"{context}: unsupported MCP operation {operation!r}",
                    )
            if "pending_sources" in block:
                MeerkatClient._parse_nonempty_string_list(
                    block["pending_sources"],
                    f"{context}: pending_sources",
                )
            MeerkatClient._validate_optional_response_bool(block, "persisted", context)
            if "phase" in block and block["phase"] is not None:
                phase = MeerkatClient._require_string_field(block, "phase", context)
                if phase not in {
                    "pending",
                    "applied",
                    "draining",
                    "forced",
                    "failed",
                }:
                    raise MeerkatError(
                        "INVALID_RESPONSE",
                        f"{context}: unsupported MCP phase {phase!r}",
                    )
        elif block_type == "background_job":
            MeerkatClient._require_string_field(block, "job_id", context)
            status = MeerkatClient._require_string_field(block, "status", context)
            if status not in {
                "completed",
                "failed",
                "aborted",
                "cancelled",
                "retired",
                "terminated",
            }:
                raise MeerkatError(
                    "INVALID_RESPONSE",
                    f"{context}: unsupported background-job status {status!r}",
                )
            MeerkatClient._validate_nullable_response_string(block, "detail", context)
            MeerkatClient._validate_nullable_response_string(
                block,
                "display_name",
                context,
            )
        elif block_type == "auth":
            MeerkatClient._require_present_string_field(block, "state", context)
            MeerkatClient._validate_nullable_response_string(block, "binding", context)
            MeerkatClient._validate_nullable_response_string(block, "detail", context)
        elif block_type == "runtime_notice":
            MeerkatClient._require_present_string_field(block, "category", context)
            MeerkatClient._validate_nullable_response_string(block, "detail", context)
        else:
            MeerkatClient._validate_nullable_response_string(block, "summary", context)

    @staticmethod
    def _validate_wire_provider_meta(raw: Any, context: str) -> None:
        meta = MeerkatClient._require_dict(raw, "meta", context)
        provider = MeerkatClient._require_string_field(meta, "provider", context)
        required_payload = {
            "anthropic": "signature",
            "anthropic_redacted": "data",
            "anthropic_compaction": "content",
            "gemini": "thoughtSignature",
            "open_ai_response": "response_id",
        }.get(provider)
        if required_payload is not None:
            MeerkatClient._require_present_string_field(
                meta,
                required_payload,
                context,
            )
        elif provider == "open_ai":
            MeerkatClient._require_present_string_field(meta, "id", context)
            for field in ("encrypted_content", "phase", "response_id"):
                MeerkatClient._validate_nullable_response_string(
                    meta,
                    field,
                    context,
                )
        elif provider != "unknown":
            raise MeerkatError(
                "INVALID_RESPONSE",
                f"{context}: unsupported provider metadata {provider!r}",
            )

    @staticmethod
    def _validate_wire_server_tool_kind(raw: Any, context: str) -> None:
        kind = MeerkatClient._require_dict(raw, "kind", context)
        kind_name = MeerkatClient._require_string_field(kind, "kind", context)
        if kind_name in {"web_search", "google_search"}:
            return
        if kind_name == "provider_native":
            MeerkatClient._require_present_string_field(kind, "name", context)
            return
        if kind_name == "unknown":
            MeerkatClient._require_present_string_field(kind, "debug", context)
            return
        raise MeerkatError(
            "INVALID_RESPONSE",
            f"{context}: unsupported server-tool kind {kind_name!r}",
        )

    @staticmethod
    def _validate_blob_ref(raw: Any, context: str) -> None:
        blob_ref = MeerkatClient._require_dict(raw, "blob_ref", context)
        MeerkatClient._require_present_string_field(blob_ref, "blob_id", context)
        MeerkatClient._require_present_string_field(blob_ref, "media_type", context)

    @staticmethod
    def _validate_provider_image_metadata(raw: Any, context: str) -> None:
        meta = MeerkatClient._require_dict(raw, "meta", context)
        provider = MeerkatClient._require_string_field(meta, "provider", context)
        if provider == "not_emitted":
            return
        if provider == "openai":
            MeerkatClient._require_present_string_field(meta, "target_model", context)
            for field in ("response_id", "image_generation_call_id"):
                MeerkatClient._validate_nullable_response_string(
                    meta,
                    field,
                    context,
                )
            return
        if provider == "gemini":
            MeerkatClient._require_present_string_field(meta, "target_model", context)
            for field in ("response_id", "continuity_ref"):
                MeerkatClient._validate_nullable_response_string(
                    meta,
                    field,
                    context,
                )
            return
        raise MeerkatError(
            "INVALID_RESPONSE",
            f"{context}: unsupported image metadata provider {provider!r}",
        )

    @staticmethod
    def _validate_revised_prompt_disposition(raw: Any, context: str) -> None:
        revised_prompt = MeerkatClient._require_dict(
            raw,
            "revised_prompt",
            context,
        )
        disposition = MeerkatClient._require_string_field(
            revised_prompt,
            "disposition",
            context,
        )
        if disposition in {"not_requested", "unsupported_by_backend", "unchanged"}:
            return
        if disposition != "revised":
            raise MeerkatError(
                "INVALID_RESPONSE",
                f"{context}: unsupported revised-prompt disposition {disposition!r}",
            )
        source = MeerkatClient._require_string_field(
            revised_prompt,
            "source",
            context,
        )
        if source not in {"provider", "meerkat_projection"}:
            raise MeerkatError(
                "INVALID_RESPONSE",
                f"{context}: unsupported revised-prompt source {source!r}",
            )
        text = MeerkatClient._require_dict(
            revised_prompt.get("text"),
            "text",
            context,
        )
        MeerkatClient._require_present_string_field(
            text,
            "content",
            f"{context}: text",
        )

    @staticmethod
    def _validate_wire_assistant_block(raw: Any, context: str) -> None:
        block = MeerkatClient._require_dict(raw, "assistant block", context)
        block_type = MeerkatClient._require_string_field(block, "block_type", context)
        if block_type not in {
            "text",
            "transcript",
            "reasoning",
            "tool_use",
            "server_tool_content",
            "image",
            "unknown",
        }:
            raise MeerkatError(
                "INVALID_RESPONSE",
                f"{context}: unsupported assistant block {block_type!r}",
            )
        if block_type == "unknown":
            return

        data = MeerkatClient._require_dict(block.get("data"), "data", context)
        data_context = f"{context}: data"
        if block_type != "image" and "meta" in data and data["meta"] is not None:
            MeerkatClient._validate_wire_provider_meta(
                data["meta"],
                f"{data_context}: meta",
            )
        if block_type == "text":
            MeerkatClient._require_present_string_field(data, "text", data_context)
        elif block_type == "transcript":
            MeerkatClient._require_present_string_field(data, "text", data_context)
            source = MeerkatClient._require_dict(
                data.get("source"),
                "source",
                data_context,
            )
            source_kind = MeerkatClient._require_string_field(
                source,
                "kind",
                f"{data_context}: source",
            )
            if source_kind == "unknown":
                MeerkatClient._require_present_string_field(
                    source,
                    "debug",
                    f"{data_context}: source",
                )
            elif source_kind != "spoken":
                raise MeerkatError(
                    "INVALID_RESPONSE",
                    f"{context}: unsupported transcript source {source_kind!r}",
                )
        elif block_type == "reasoning":
            MeerkatClient._validate_optional_response_string(
                data,
                "text",
                data_context,
            )
        elif block_type == "tool_use":
            MeerkatClient._require_present_field(data, "args", data_context)
            MeerkatClient._require_string_field(data, "id", data_context)
            MeerkatClient._require_string_field(data, "name", data_context)
        elif block_type == "server_tool_content":
            MeerkatClient._require_present_field(data, "content", data_context)
            MeerkatClient._validate_wire_server_tool_kind(
                data.get("kind"),
                f"{data_context}: kind",
            )
            MeerkatClient._validate_nullable_response_string(data, "id", data_context)
        else:
            MeerkatClient._validate_blob_ref(
                data.get("blob_ref"),
                f"{data_context}: blob_ref",
            )
            MeerkatClient._require_non_negative_integer_field(
                data, "height", data_context
            )
            MeerkatClient._require_string_field(data, "image_id", data_context)
            MeerkatClient._require_present_string_field(
                data, "media_type", data_context
            )
            MeerkatClient._validate_provider_image_metadata(
                data.get("meta"),
                f"{data_context}: meta",
            )
            MeerkatClient._validate_revised_prompt_disposition(
                data.get("revised_prompt"),
                f"{data_context}: revised_prompt",
            )
            MeerkatClient._require_non_negative_integer_field(
                data, "width", data_context
            )

    @staticmethod
    def _validate_wire_tool_result(raw: Any, context: str) -> None:
        result = MeerkatClient._require_dict(raw, "tool result", context)
        MeerkatClient._require_string_field(result, "tool_use_id", context)
        content = MeerkatClient._require_present_field(result, "content", context)
        if content is None:
            raise MeerkatError(
                "INVALID_RESPONSE",
                f"{context}: content must not be null",
            )
        if isinstance(content, list):
            for index, block in enumerate(content):
                MeerkatClient._validate_wire_content_block(
                    block,
                    f"{context}: content[{index}]",
                )
        elif not isinstance(content, str):
            raise MeerkatError(
                "INVALID_RESPONSE",
                f"{context}: content must be a string or block list",
            )
        MeerkatClient._validate_optional_response_bool(result, "is_error", context)

    @staticmethod
    def _validate_wire_history_row(raw: Any, context: str) -> WireHistoryRow:
        row = MeerkatClient._require_dict(raw, "history row", context)
        role = MeerkatClient._require_string_field(row, "role", context)
        MeerkatClient._require_string_field(row, "created_at", context)
        for field in ("interaction_id", "run_id"):
            value = row.get(field)
            if value is not None and (not isinstance(value, str) or not value):
                raise MeerkatError(
                    "INVALID_RESPONSE",
                    f"{context}: {field} must be a non-empty string",
                )

        if role == "system":
            MeerkatClient._require_present_string_field(row, "content", context)
        elif role == "system_notice":
            kind = MeerkatClient._require_string_field(row, "kind", context)
            if kind not in {
                "generic",
                "comms",
                "external_event",
                "mcp_pending",
                "mcp",
                "background_job",
                "tool_scope",
                "tool_scope_warning",
                "auth_reauth_required",
            }:
                raise MeerkatError(
                    "INVALID_RESPONSE",
                    f"{context}: unsupported system-notice kind {kind!r}",
                )
            MeerkatClient._validate_nullable_response_string(row, "body", context)
            MeerkatClient._validate_optional_response_string(row, "content", context)
            if "blocks" in row:
                blocks = MeerkatClient._require_present_list_field(
                    row,
                    "blocks",
                    context,
                )
                for index, block in enumerate(blocks):
                    MeerkatClient._validate_system_notice_block(
                        block,
                        f"{context}: blocks[{index}]",
                    )
        elif role == "user":
            content = MeerkatClient._require_present_field(row, "content", context)
            if content is None:
                raise MeerkatError(
                    "INVALID_RESPONSE",
                    f"{context}: content must not be null",
                )
            if isinstance(content, list):
                for index, block in enumerate(content):
                    MeerkatClient._validate_wire_content_block(
                        block,
                        f"{context}: content[{index}]",
                    )
            elif not isinstance(content, str):
                raise MeerkatError(
                    "INVALID_RESPONSE",
                    f"{context}: content must be a string or block list",
                )
            transcript_role = row.get("transcript_role")
            if transcript_role is not None and transcript_role not in {
                "conversational",
                "compaction_summary",
                "injected_context",
            }:
                raise MeerkatError(
                    "INVALID_RESPONSE",
                    f"{context}: unsupported transcript_role {transcript_role!r}",
                )
        elif role == "block_assistant":
            blocks = MeerkatClient._require_present_list_field(
                row,
                "blocks",
                context,
            )
            for index, block in enumerate(blocks):
                MeerkatClient._validate_wire_assistant_block(
                    block,
                    f"{context}: blocks[{index}]",
                )
            stop_reason = MeerkatClient._require_string_field(
                row,
                "stop_reason",
                context,
            )
            if stop_reason not in {
                "end_turn",
                "tool_use",
                "max_tokens",
                "stop_sequence",
                "content_filter",
                "cancelled",
            }:
                raise MeerkatError(
                    "INVALID_RESPONSE",
                    f"{context}: unsupported stop_reason {stop_reason!r}",
                )
        elif role == "tool_results":
            results = MeerkatClient._require_present_list_field(
                row,
                "results",
                context,
            )
            for index, result in enumerate(results):
                MeerkatClient._validate_wire_tool_result(
                    result,
                    f"{context}: results[{index}]",
                )
        else:
            raise MeerkatError(
                "INVALID_RESPONSE",
                f"{context}: unsupported role {role!r}",
            )
        return cast(WireHistoryRow, row)

    @staticmethod
    def _parse_mob_member_history_result(
        raw: Any,
    ) -> MobMemberHistoryResult:
        context = "Invalid mob/member_history response"
        result = MeerkatClient._require_dict(raw, "result", context)
        page_raw = MeerkatClient._require_dict(result.get("page"), "page", context)
        messages_raw = MeerkatClient._require_present_list_field(
            page_raw,
            "messages",
            f"{context}: page",
        )
        messages = [
            MeerkatClient._validate_wire_history_row(
                row,
                f"{context}: page.messages[{index}]",
            )
            for index, row in enumerate(messages_raw)
        ]
        from_index = MeerkatClient._require_non_negative_integer_field(
            page_raw,
            "from_index",
            f"{context}: page",
        )
        message_count = MeerkatClient._require_non_negative_integer_field(
            page_raw,
            "message_count",
            f"{context}: page",
        )
        complete = MeerkatClient._require_bool_field(
            page_raw,
            "complete",
            f"{context}: page",
        )
        next_index: int | None = None
        if page_raw.get("next_index") is not None:
            next_index = MeerkatClient._require_non_negative_integer_field(
                page_raw,
                "next_index",
                f"{context}: page",
            )
        page_end = from_index + len(messages)
        if page_end > message_count:
            raise MeerkatError(
                "INVALID_RESPONSE",
                f"{context}: page ends beyond message_count",
            )
        if complete != (page_end == message_count):
            raise MeerkatError(
                "INVALID_RESPONSE",
                f"{context}: complete must match whether the page reaches message_count",
            )
        if complete and next_index is not None:
            raise MeerkatError(
                "INVALID_RESPONSE",
                f"{context}: complete page must not carry next_index",
            )
        if not complete:
            if not messages:
                raise MeerkatError(
                    "INVALID_RESPONSE",
                    f"{context}: incomplete page must make progress",
                )
            if next_index != page_end:
                raise MeerkatError(
                    "INVALID_RESPONSE",
                    f"{context}: incomplete page next_index must equal served end",
                )
        provenance = MeerkatClient._require_string_field(
            result,
            "provenance",
            context,
        )
        if provenance not in {"host_claimed", "controlling_host_verified"}:
            raise MeerkatError(
                "INVALID_RESPONSE",
                f"{context}: unsupported provenance {provenance!r}",
            )
        placement = MeerkatClient._optional_string_field(
            result,
            "placement",
            context,
        )
        if placement == "":
            raise MeerkatError(
                "INVALID_RESPONSE",
                f"{context}: placement must be non-empty",
            )
        page = WireMemberHistoryPageBody(
            complete=complete,
            from_index=from_index,
            message_count=message_count,
            messages=messages,
            next_index=next_index,
        )
        return MobMemberHistoryResult(
            generation=MeerkatClient._require_non_negative_integer_field(
                result,
                "generation",
                context,
            ),
            page=page,
            provenance=cast(WireProjectionProvenance, provenance),
            placement=placement,
        )

    @staticmethod
    def _parse_mob_host_capabilities(
        raw: Any,
        context: str,
    ) -> WireHostCapabilityFlags:
        capabilities = MeerkatClient._require_dict(raw, "capabilities", context)
        protocol_min = MeerkatClient._require_non_negative_integer_field(
            capabilities,
            "protocol_min",
            context,
        )
        protocol_max = MeerkatClient._require_non_negative_integer_field(
            capabilities,
            "protocol_max",
            context,
        )
        if protocol_min > protocol_max:
            raise MeerkatError(
                "INVALID_RESPONSE",
                f"{context}: protocol_min exceeds protocol_max",
            )
        live_endpoint = MeerkatClient._optional_string_field(
            capabilities,
            "live_endpoint",
            context,
        )
        if live_endpoint == "":
            raise MeerkatError(
                "INVALID_RESPONSE",
                f"{context}: live_endpoint must be non-empty",
            )
        resolvable_providers: list[str] | None = None
        if "resolvable_providers" in capabilities:
            resolvable_providers = MeerkatClient._parse_nonempty_string_list(
                capabilities["resolvable_providers"],
                f"{context}: resolvable_providers",
            )
        tracked_input_cancel = False
        if "tracked_input_cancel" in capabilities:
            tracked_input_cancel = MeerkatClient._require_bool_field(
                capabilities,
                "tracked_input_cancel",
                context,
            )
        return WireHostCapabilityFlags(
            approval_forwarding=MeerkatClient._require_bool_field(
                capabilities,
                "approval_forwarding",
                context,
            ),
            autonomous_members=MeerkatClient._require_bool_field(
                capabilities,
                "autonomous_members",
                context,
            ),
            durable_sessions=MeerkatClient._require_bool_field(
                capabilities,
                "durable_sessions",
                context,
            ),
            engine_version=MeerkatClient._require_string_field(
                capabilities,
                "engine_version",
                context,
            ),
            hard_cancel_member=MeerkatClient._require_bool_field(
                capabilities,
                "hard_cancel_member",
                context,
            ),
            mcp=MeerkatClient._require_bool_field(capabilities, "mcp", context),
            memory_store=MeerkatClient._require_bool_field(
                capabilities,
                "memory_store",
                context,
            ),
            protocol_max=protocol_max,
            protocol_min=protocol_min,
            live_endpoint=live_endpoint,
            resolvable_providers=resolvable_providers,
            tracked_input_cancel=tracked_input_cancel,
        )

    @staticmethod
    def _parse_mob_host_status(raw: Any, context: str) -> MobHostStatus:
        host = MeerkatClient._require_dict(raw, "host", context)
        bind_phase = MeerkatClient._require_string_field(
            host,
            "bind_phase",
            context,
        )
        if bind_phase not in {"requested", "bound"}:
            raise MeerkatError(
                "INVALID_RESPONSE",
                f"{context}: unsupported bind_phase {bind_phase!r}",
            )
        authority_epoch: int | None = None
        if host.get("authority_epoch") is not None:
            authority_epoch = MeerkatClient._require_non_negative_integer_field(
                host,
                "authority_epoch",
                context,
            )
        endpoint = MeerkatClient._optional_string_field(host, "endpoint", context)
        if endpoint == "":
            raise MeerkatError(
                "INVALID_RESPONSE",
                f"{context}: endpoint must be non-empty",
            )
        capabilities = (
            MeerkatClient._parse_mob_host_capabilities(
                host.get("capabilities"),
                f"{context}: capabilities",
            )
            if host.get("capabilities") is not None
            else None
        )
        if bind_phase == "bound" and (
            authority_epoch is None or endpoint is None or capabilities is None
        ):
            raise MeerkatError(
                "INVALID_RESPONSE",
                f"{context}: bound host missing committed authority facts",
            )
        if bind_phase == "requested" and (
            authority_epoch is not None
            or endpoint is not None
            or capabilities is not None
        ):
            raise MeerkatError(
                "INVALID_RESPONSE",
                f"{context}: requested host must not claim committed authority facts",
            )
        control_reachability = MeerkatClient._optional_string_field(
            host,
            "control_reachability",
            context,
        )
        if control_reachability is not None and control_reachability not in {
            "reachable",
            "stale",
            "unreachable",
            "unknown",
        }:
            raise MeerkatError(
                "INVALID_RESPONSE",
                f"{context}: unsupported control_reachability",
            )
        last_seen_ms: int | None = None
        if host.get("last_seen_ms") is not None:
            last_seen_ms = MeerkatClient._require_non_negative_integer_field(
                host,
                "last_seen_ms",
                context,
            )
        return MobHostStatus(
            bind_phase=cast(WireHostBindPhase, bind_phase),
            host_id=MeerkatClient._require_string_field(host, "host_id", context),
            materialized_member_count=MeerkatClient._require_non_negative_integer_field(
                host,
                "materialized_member_count",
                context,
            ),
            authority_epoch=authority_epoch,
            capabilities=capabilities,
            control_reachability=cast(WireReachability, control_reachability),
            endpoint=endpoint,
            freshness_reason=MeerkatClient._optional_string_field(
                host,
                "freshness_reason",
                context,
            ),
            last_seen_ms=last_seen_ms,
        )

    @staticmethod
    def _parse_route_install_obligation(
        raw: Any,
        context: str,
    ) -> WireRouteInstallObligation:
        obligation = MeerkatClient._require_dict(raw, "obligation", context)
        return WireRouteInstallObligation(
            edge_a=MeerkatClient._require_string_field(
                obligation,
                "edge_a",
                context,
            ),
            edge_b=MeerkatClient._require_string_field(
                obligation,
                "edge_b",
                context,
            ),
            host=MeerkatClient._require_string_field(obligation, "host", context),
        )

    @staticmethod
    def _parse_live_channel_capabilities(
        raw: Any,
        context: str,
    ) -> WireLiveChannelCapabilities:
        capabilities = MeerkatClient._require_dict(raw, "capabilities", context)
        return WireLiveChannelCapabilities(
            audio_in=MeerkatClient._require_bool_field(
                capabilities,
                "audio_in",
                context,
            ),
            audio_out=MeerkatClient._require_bool_field(
                capabilities,
                "audio_out",
                context,
            ),
            barge_in_supported=MeerkatClient._require_bool_field(
                capabilities,
                "barge_in_supported",
                context,
            ),
            image_in=MeerkatClient._require_bool_field(
                capabilities,
                "image_in",
                context,
            ),
            provider_native_resume=MeerkatClient._require_bool_field(
                capabilities,
                "provider_native_resume",
                context,
            ),
            text_in=MeerkatClient._require_bool_field(
                capabilities,
                "text_in",
                context,
            ),
            text_out=MeerkatClient._require_bool_field(
                capabilities,
                "text_out",
                context,
            ),
            transcript_supported=MeerkatClient._require_bool_field(
                capabilities,
                "transcript_supported",
                context,
            ),
            video_in=MeerkatClient._require_bool_field(
                capabilities,
                "video_in",
                context,
            ),
        )

    @staticmethod
    def _parse_live_transport_bootstrap(raw: Any, context: str) -> dict[str, Any]:
        transport = MeerkatClient._require_dict(raw, "transport", context)
        kind = MeerkatClient._require_string_field(
            transport,
            "transport",
            context,
        )
        if kind == "websocket":
            MeerkatClient._require_string_field(transport, "url", context)
            MeerkatClient._require_string_field(transport, "token", context)
        elif kind == "webrtc":
            MeerkatClient._require_string_field(transport, "answer_method", context)
            MeerkatClient._require_string_field(transport, "token", context)
            http_url = MeerkatClient._optional_string_field(
                transport,
                "http_url",
                context,
            )
            if http_url == "":
                raise MeerkatError(
                    "INVALID_RESPONSE",
                    f"{context}: http_url must be non-empty",
                )
        elif kind == "unknown":
            MeerkatClient._require_string_field(transport, "debug", context)
        else:
            raise MeerkatError(
                "INVALID_RESPONSE",
                f"{context}: unsupported transport {kind!r}",
            )
        return transport

    @staticmethod
    def _parse_live_continuity(raw: Any, context: str) -> dict[str, Any]:
        continuity = MeerkatClient._require_dict(raw, "continuity", context)
        mode = MeerkatClient._require_string_field(continuity, "mode", context)
        if mode == "provider_native_resume":
            MeerkatClient._require_string_field(
                continuity,
                "provider_session_id",
                context,
            )
        elif mode == "unknown":
            MeerkatClient._require_string_field(continuity, "debug", context)
        elif mode not in {"fresh", "transcript_only", "degraded"}:
            raise MeerkatError(
                "INVALID_RESPONSE",
                f"{context}: unsupported continuity mode {mode!r}",
            )
        return continuity

    @staticmethod
    def _parse_live_open_result(raw: Any, context: str) -> LiveOpenResult:
        result = MeerkatClient._require_dict(raw, "result", context)
        return LiveOpenResult(
            capabilities=MeerkatClient._parse_live_channel_capabilities(
                result.get("capabilities"),
                f"{context}: capabilities",
            ),
            channel_id=MeerkatClient._require_string_field(
                result,
                "channel_id",
                context,
            ),
            continuity=cast(
                Any,
                MeerkatClient._parse_live_continuity(
                    result.get("continuity"),
                    f"{context}: continuity",
                ),
            ),
            transport=cast(
                Any,
                MeerkatClient._parse_live_transport_bootstrap(
                    result.get("transport"),
                    f"{context}: transport",
                ),
            ),
        )

    @staticmethod
    def _parse_live_adapter_status(raw: Any, context: str) -> dict[str, Any]:
        status = MeerkatClient._require_dict(raw, "status", context)
        state = MeerkatClient._require_string_field(status, "status", context)
        if state == "degraded":
            reason = MeerkatClient._require_dict(
                status.get("reason"),
                "reason",
                context,
            )
            reason_kind = MeerkatClient._require_string_field(
                reason,
                "kind",
                f"{context}: reason",
            )
            if reason_kind not in {
                "rate_limited",
                "provider_throttled",
                "network_unstable",
                "other",
                "unknown",
            }:
                raise MeerkatError(
                    "INVALID_RESPONSE",
                    f"{context}: unsupported degradation reason {reason_kind!r}",
                )
            if reason_kind == "other":
                MeerkatClient._require_present_string_field(
                    reason,
                    "detail",
                    f"{context}: reason",
                )
            elif reason_kind == "unknown":
                MeerkatClient._require_present_string_field(
                    reason,
                    "debug",
                    f"{context}: reason",
                )
        elif state == "unknown":
            MeerkatClient._require_string_field(status, "debug", context)
        elif state not in {"idle", "opening", "ready", "closing", "closed"}:
            raise MeerkatError(
                "INVALID_RESPONSE",
                f"{context}: unsupported live status {state!r}",
            )
        return status

    @staticmethod
    def _parse_live_status_result(raw: Any, context: str) -> LiveStatusResult:
        result = MeerkatClient._require_dict(raw, "result", context)
        return LiveStatusResult(
            channel_id=MeerkatClient._require_string_field(
                result,
                "channel_id",
                context,
            ),
            status=cast(
                Any,
                MeerkatClient._parse_live_adapter_status(
                    result.get("status"),
                    f"{context}: status",
                ),
            ),
        )

    @staticmethod
    def _parse_bridge_live_control_outcome(
        raw: Any,
        context: str,
        expected_verb: str,
    ) -> BridgeLiveControlOutcome:
        outcome = MeerkatClient._require_dict(raw, "result", context)
        verb = MeerkatClient._require_string_field(outcome, "verb", context)
        status = MeerkatClient._require_string_field(outcome, "status", context)
        expected_status = {
            "commit_input": "committed",
            "interrupt": "interrupted",
            "truncate": "truncated",
            "refresh": "queued",
        }.get(verb)
        if (
            expected_status is None
            or status != expected_status
            or verb != expected_verb
        ):
            raise MeerkatError(
                "INVALID_RESPONSE",
                f"{context}: mismatched verb/status outcome",
            )
        return cast(BridgeLiveControlOutcome, outcome)

    @staticmethod
    def _parse_nonempty_string_list(raw: Any, context: str) -> list[str]:
        if not isinstance(raw, list):
            raise MeerkatError(
                "INVALID_RESPONSE",
                f"{context} must be a list",
            )
        parsed: list[str] = []
        for index, value in enumerate(raw):
            if not isinstance(value, str) or not value:
                raise MeerkatError(
                    "INVALID_RESPONSE",
                    f"{context}[{index}] must be a non-empty string",
                )
            parsed.append(value)
        return parsed

    @staticmethod
    def _require_dict(raw: Any, field: str, context: str) -> dict[str, Any]:
        if not isinstance(raw, dict):
            raise MeerkatError("INVALID_RESPONSE", f"{context}: missing {field}")
        return raw

    @staticmethod
    def _require_string_field(raw: dict[str, Any], field: str, context: str) -> str:
        value = raw.get(field)
        if not isinstance(value, str) or not value:
            raise MeerkatError("INVALID_RESPONSE", f"{context}: missing {field}")
        return value

    @staticmethod
    def _require_present_string_field(
        raw: dict[str, Any], field: str, context: str
    ) -> str:
        value = raw.get(field)
        if not isinstance(value, str):
            raise MeerkatError("INVALID_RESPONSE", f"{context}: missing {field}")
        return value

    @staticmethod
    def _require_number_field(
        raw: dict[str, Any],
        field: str,
        context: str,
        display_field: str | None = None,
    ) -> int | float:
        if field not in raw:
            raise MeerkatError(
                "INVALID_RESPONSE",
                f"{context}: missing {display_field or field}",
            )
        value = raw.get(field)
        if not isinstance(value, (int, float)) or isinstance(value, bool):
            raise MeerkatError(
                "INVALID_RESPONSE",
                f"{context}: {display_field or field} must be number",
            )
        return value

    @staticmethod
    def _require_non_negative_integer_field(
        raw: dict[str, Any],
        field: str,
        context: str,
        display_field: str | None = None,
    ) -> int:
        """Return a required wire count/total field (Rust ``usize``/``u64``).

        Delegates the base number check to ``_require_number_field``; a
        present value that is fractional or negative can never be a valid
        unsigned integer, so it is a contract violation and raises
        ``INVALID_RESPONSE`` rather than being accepted as an
        any-finite-number. Mirrors the web SDK
        ``requireNonNegativeIntegerField`` and the TypeScript SDK
        ``requireNonNegativeIntegerField``.
        """
        value = MeerkatClient._require_number_field(raw, field, context, display_field)
        if value < 0 or (isinstance(value, float) and not value.is_integer()):
            raise MeerkatError(
                "INVALID_RESPONSE",
                f"{context}: {display_field or field} must be a non-negative integer",
            )
        return int(value)

    @staticmethod
    def _require_bool_field(raw: dict[str, Any], field: str, context: str) -> bool:
        value = raw.get(field)
        if not isinstance(value, bool):
            raise MeerkatError(
                "INVALID_RESPONSE", f"{context}: {field} must be boolean"
            )
        return value

    @staticmethod
    def _parse_comms_send_result(raw: Any) -> CommsSendResult:
        context = "Invalid comms/send response"
        result = MeerkatClient._require_dict(raw, "result", context)
        kind = MeerkatClient._require_string_field(result, "kind", context)

        if kind == "input_accepted":
            allowed = {"kind", "interaction_id", "stream_reserved"}
            MeerkatClient._require_string_field(result, "interaction_id", context)
            MeerkatClient._require_bool_field(result, "stream_reserved", context)
        elif kind == "peer_message_sent":
            allowed = {"kind", "envelope_id", "delivery"}
            MeerkatClient._require_string_field(result, "envelope_id", context)
            delivery = MeerkatClient._require_string_field(result, "delivery", context)
            if delivery not in {"acked", "handed_off", "queued"}:
                raise MeerkatError("INVALID_RESPONSE", f"{context}: invalid delivery")
        elif kind == "peer_lifecycle_sent":
            allowed = {"kind", "envelope_id"}
            MeerkatClient._require_string_field(result, "envelope_id", context)
        elif kind == "peer_request_sent":
            allowed = {
                "kind",
                "envelope_id",
                "interaction_id",
                "request_id",
                "stream_reserved",
            }
            MeerkatClient._require_string_field(result, "envelope_id", context)
            MeerkatClient._require_string_field(result, "interaction_id", context)
            MeerkatClient._require_string_field(result, "request_id", context)
            MeerkatClient._require_bool_field(result, "stream_reserved", context)
        elif kind == "peer_response_sent":
            allowed = {"kind", "envelope_id", "in_reply_to"}
            MeerkatClient._require_string_field(result, "envelope_id", context)
            MeerkatClient._require_string_field(result, "in_reply_to", context)
        else:
            raise MeerkatError("INVALID_RESPONSE", f"{context}: invalid kind")
        unknown = set(result) - allowed
        if unknown:
            raise MeerkatError(
                "INVALID_RESPONSE",
                f"{context}: unknown field {sorted(unknown)[0]}",
            )
        return cast(CommsSendResult, result)

    @staticmethod
    def _parse_comms_peers_result(raw: Any) -> dict[str, Any]:
        context = "Invalid comms/peers response"
        result = MeerkatClient._require_dict(raw, "result", context)
        MeerkatClient._require_present_list_field(result, "peers", context)
        return result

    @staticmethod
    def _parse_member_progress_snapshot(
        raw: Any, context: str
    ) -> WireMemberProgressSnapshot:
        progress = MeerkatClient._require_dict(raw, "progress", context)
        run_state = MeerkatClient._require_string_field(progress, "run_state", context)
        if run_state not in {"idle", "run_open", "unknown"}:
            raise MeerkatError(
                "INVALID_RESPONSE", f"{context}: invalid progress.run_state"
            )
        last_event = MeerkatClient._require_string_field(
            progress, "last_progress_event", context
        )
        if last_event not in {"execution_advanced", "became_idle", "unchanged"}:
            raise MeerkatError(
                "INVALID_RESPONSE", f"{context}: invalid progress.last_progress_event"
            )
        health = MeerkatClient._require_string_field(progress, "health", context)
        if health not in {"healthy", "degraded", "wedged", "unknown"}:
            raise MeerkatError(
                "INVALID_RESPONSE", f"{context}: invalid progress.health"
            )
        MeerkatClient._require_non_negative_integer_field(
            progress, "in_flight_work", context, "progress.in_flight_work"
        )
        MeerkatClient._require_non_negative_integer_field(
            progress, "last_progress_at_ms", context, "progress.last_progress_at_ms"
        )
        allowed = {
            "run_state",
            "in_flight_work",
            "last_progress_at_ms",
            "last_progress_event",
            "health",
        }
        unknown = set(progress) - allowed
        if unknown:
            raise MeerkatError(
                "INVALID_RESPONSE",
                f"{context}: unknown progress field {sorted(unknown)[0]}",
            )
        return cast(WireMemberProgressSnapshot, progress)

    @staticmethod
    def _optional_string_field(
        raw: dict[str, Any], field: str, context: str
    ) -> str | None:
        """Return an optional wire string, failing closed on malformed shapes.

        An absent or ``null`` field yields ``None``; a field that is
        *present-but-non-string* is a contract violation and raises
        ``INVALID_RESPONSE`` rather than being silently coerced.
        """
        value = raw.get(field)
        if value is None:
            return None
        if not isinstance(value, str):
            raise MeerkatError("INVALID_RESPONSE", f"{context}: {field} must be string")
        return value

    @staticmethod
    def _optional_dict_field(
        raw: dict[str, Any], field: str, context: str
    ) -> dict[str, Any] | None:
        """Return an optional wire object, failing closed on malformed shapes.

        An absent or ``null`` field yields ``None``; a field that is
        *present-but-non-object* is a contract violation and raises
        ``INVALID_RESPONSE`` rather than being silently dropped. Mirrors the
        web SDK ``optionalRecordField`` and the TypeScript SDK
        ``requireRecord``-on-present policy.
        """
        value = raw.get(field)
        if value is None:
            return None
        if not isinstance(value, dict):
            raise MeerkatError("INVALID_RESPONSE", f"{context}: {field} must be object")
        return value

    @staticmethod
    def _optional_number_field(
        raw: dict[str, Any], field: str, context: str
    ) -> int | None:
        """Return an optional wire integer, failing closed on malformed shapes.

        An absent or ``null`` field yields ``None``; a field that is
        *present-but-non-number* (or a bool) is a contract violation and raises
        ``INVALID_RESPONSE`` rather than being silently coerced.
        """
        value = raw.get(field)
        if value is None:
            return None
        if not isinstance(value, (int, float)) or isinstance(value, bool):
            raise MeerkatError("INVALID_RESPONSE", f"{context}: {field} must be number")
        return int(value)

    @staticmethod
    def _optional_non_negative_integer_field(
        raw: dict[str, Any], field: str, context: str
    ) -> int | None:
        """Return an optional unsigned wire integer without coercion.

        Missing and explicit ``null`` both represent ``None`` for optional
        serde fields. Present values must satisfy the same non-negative integer
        contract as required Rust ``usize``/``u64`` fields; booleans,
        fractional numbers, and negative integers are response violations.
        """
        if raw.get(field) is None:
            return None
        return MeerkatClient._require_non_negative_integer_field(
            raw,
            field,
            context,
        )

    @staticmethod
    def _require_list_field(raw: dict[str, Any], field: str, context: str) -> list[Any]:
        """Return a required wire array, failing closed on malformed shapes.

        An absent field is treated as the empty list (the runtime may omit
        empty collections), but a field that is *present-but-non-list* is a
        contract violation and raises ``INVALID_RESPONSE`` rather than being
        silently coerced to ``[]``.
        """
        if field not in raw:
            return []
        value = raw[field]
        if not isinstance(value, list):
            raise MeerkatError("INVALID_RESPONSE", f"{context}: {field} must be a list")
        return value

    @staticmethod
    def _require_present_list_field(
        raw: dict[str, Any], field: str, context: str
    ) -> list[Any]:
        if field not in raw:
            raise MeerkatError("INVALID_RESPONSE", f"{context}: missing {field}")
        return MeerkatClient._require_list_field(raw, field, context)

    @staticmethod
    def _parse_skill_diagnostics(raw: Any) -> SkillRuntimeDiagnostics | None:
        if raw is None:
            return None
        context = "Invalid run result"
        data = MeerkatClient._require_dict(raw, "skill_diagnostics", context)

        source_health_raw = MeerkatClient._require_dict(
            data.get("source_health"),
            "skill_diagnostics.source_health",
            context,
        )
        source_health = SourceHealthSnapshot(
            state=MeerkatClient._require_string_field(
                source_health_raw, "state", context
            ),
            invalid_ratio=MeerkatClient._require_number_field(
                source_health_raw,
                "invalid_ratio",
                context,
                "skill_diagnostics.source_health.invalid_ratio",
            ),
            invalid_count=MeerkatClient._require_non_negative_integer_field(
                source_health_raw,
                "invalid_count",
                context,
                "skill_diagnostics.source_health.invalid_count",
            ),
            total_count=MeerkatClient._require_non_negative_integer_field(
                source_health_raw,
                "total_count",
                context,
                "skill_diagnostics.source_health.total_count",
            ),
            failure_streak=MeerkatClient._require_non_negative_integer_field(
                source_health_raw,
                "failure_streak",
                context,
                "skill_diagnostics.source_health.failure_streak",
            ),
            handshake_failed=MeerkatClient._require_bool_field(
                source_health_raw, "handshake_failed", context
            ),
        )

        quarantined_items: list[SkillQuarantineDiagnostic] = []
        raw_quarantined = MeerkatClient._require_present_list_field(
            data, "quarantined", context
        )
        for idx, item in enumerate(raw_quarantined):
            entry = MeerkatClient._require_dict(
                item,
                f"skill_diagnostics.quarantined[{idx}]",
                context,
            )
            identity = MeerkatClient._require_dict(
                entry.get("identity"),
                f"skill_diagnostics.quarantined[{idx}].identity",
                context,
            )
            quarantined_items.append(
                SkillQuarantineDiagnostic(
                    source_uuid=MeerkatClient._require_string_field(
                        identity, "source_uuid", context
                    ),
                    skill_id=MeerkatClient._require_present_string_field(
                        identity, "raw_id", context
                    ),
                    location=MeerkatClient._require_present_string_field(
                        entry, "location", context
                    ),
                    error_code=MeerkatClient._require_present_string_field(
                        entry, "error_code", context
                    ),
                    error_class=MeerkatClient._require_present_string_field(
                        entry, "error_class", context
                    ),
                    message=MeerkatClient._require_present_string_field(
                        entry, "message", context
                    ),
                    first_seen_unix_secs=MeerkatClient._require_non_negative_integer_field(
                        entry,
                        "first_seen_unix_secs",
                        context,
                        "skill_diagnostics.quarantined.first_seen_unix_secs",
                    ),
                    last_seen_unix_secs=MeerkatClient._require_non_negative_integer_field(
                        entry,
                        "last_seen_unix_secs",
                        context,
                        "skill_diagnostics.quarantined.last_seen_unix_secs",
                    ),
                )
            )

        return SkillRuntimeDiagnostics(
            source_health=source_health,
            quarantined=quarantined_items,
            collection_fault=data.get("collection_fault"),
        )

    @staticmethod
    def _check_version_compatible(server: str, client: str) -> bool:
        # Drive contract-version compatibility off the generated helper
        # (mirrors `ContractVersion::is_compatible_with`) instead of a
        # hand-rolled copy of the rule (dogma row #193).
        return _generated_is_compatible_with(server, client)

    @staticmethod
    def _parse_run_result(data: dict[str, Any]) -> RunResult:
        context = "Invalid run result"
        usage_data = MeerkatClient._require_dict(data.get("usage"), "usage", context)
        usage = Usage(
            input_tokens=MeerkatClient._require_number_field(
                usage_data,
                "input_tokens",
                context,
                "usage.input_tokens",
            ),
            output_tokens=MeerkatClient._require_number_field(
                usage_data,
                "output_tokens",
                context,
                "usage.output_tokens",
            ),
            cache_creation_tokens=(
                MeerkatClient._require_number_field(
                    usage_data,
                    "cache_creation_tokens",
                    context,
                    "usage.cache_creation_tokens",
                )
                if usage_data.get("cache_creation_tokens") is not None
                else None
            ),
            cache_read_tokens=(
                MeerkatClient._require_number_field(
                    usage_data,
                    "cache_read_tokens",
                    context,
                    "usage.cache_read_tokens",
                )
                if usage_data.get("cache_read_tokens") is not None
                else None
            ),
        )
        raw_warnings = data.get("schema_warnings")
        schema_warnings: list[SchemaWarning] | None = None
        if raw_warnings is not None:
            warnings = MeerkatClient._require_list_field(
                {"schema_warnings": raw_warnings},
                "schema_warnings",
                context,
            )
            schema_warnings = []
            for idx, warning in enumerate(warnings):
                entry = MeerkatClient._require_dict(
                    warning, f"schema_warnings[{idx}]", context
                )
                schema_warnings.append(
                    SchemaWarning(
                        provider=MeerkatClient._require_present_string_field(
                            entry, "provider", context
                        ),
                        path=MeerkatClient._require_present_string_field(
                            entry, "path", context
                        ),
                        message=MeerkatClient._require_present_string_field(
                            entry, "message", context
                        ),
                    )
                )
        raw_extraction_error = data.get("extraction_error")
        extraction_error = None
        if raw_extraction_error is not None:
            extraction = MeerkatClient._require_dict(
                raw_extraction_error,
                "extraction_error",
                context,
            )
            extraction_error = ExtractionError(
                last_output=MeerkatClient._require_present_string_field(
                    extraction,
                    "last_output",
                    context,
                ),
                attempts=MeerkatClient._require_number_field(
                    extraction,
                    "attempts",
                    context,
                    "extraction_error.attempts",
                ),
                reason=MeerkatClient._require_present_string_field(
                    extraction,
                    "reason",
                    context,
                ),
            )
        return RunResult(
            session_id=MeerkatClient._require_string_field(data, "session_id", context),
            text=MeerkatClient._require_present_string_field(data, "text", context),
            turns=MeerkatClient._require_number_field(data, "turns", context),
            tool_calls=MeerkatClient._require_number_field(data, "tool_calls", context),
            usage=usage,
            terminal_cause_kind=data.get("terminal_cause_kind")
            if isinstance(data.get("terminal_cause_kind"), str)
            else None,
            session_ref=MeerkatClient._optional_string_field(
                data, "session_ref", context
            ),
            structured_output=data.get("structured_output"),
            extraction_error=extraction_error,
            schema_warnings=schema_warnings,
            skill_diagnostics=MeerkatClient._parse_skill_diagnostics(
                data.get("skill_diagnostics")
            ),
        )

    @staticmethod
    def _parse_session_history(data: Any) -> SessionHistory:
        context = "Invalid session history"
        data = MeerkatClient._require_dict(data, "result", context)
        return SessionHistory(
            session_id=MeerkatClient._require_string_field(data, "session_id", context),
            session_ref=MeerkatClient._optional_string_field(
                data, "session_ref", context
            ),
            message_count=MeerkatClient._require_non_negative_integer_field(
                data, "message_count", context
            ),
            offset=MeerkatClient._require_non_negative_integer_field(
                data, "offset", context
            ),
            limit=MeerkatClient._optional_non_negative_integer_field(
                data, "limit", context
            ),
            has_more=MeerkatClient._require_bool_field(data, "has_more", context),
            messages=[
                MeerkatClient._parse_session_message(message)
                for message in MeerkatClient._require_present_list_field(
                    data, "messages", context
                )
            ],
        )

    @staticmethod
    def _parse_session_transcript_revision(
        data: Any,
    ) -> SessionTranscriptRevision:
        context = "Invalid session transcript revision"
        data = MeerkatClient._require_dict(data, "result", context)
        return SessionTranscriptRevision(
            session_id=MeerkatClient._require_string_field(data, "session_id", context),
            session_ref=MeerkatClient._optional_string_field(
                data, "session_ref", context
            ),
            revision=MeerkatClient._require_string_field(data, "revision", context),
            head_revision=MeerkatClient._require_string_field(
                data, "head_revision", context
            ),
            message_count=MeerkatClient._require_non_negative_integer_field(
                data, "message_count", context
            ),
            offset=MeerkatClient._require_non_negative_integer_field(
                data, "offset", context
            ),
            limit=MeerkatClient._optional_non_negative_integer_field(
                data, "limit", context
            ),
            has_more=MeerkatClient._require_bool_field(data, "has_more", context),
            messages=[
                MeerkatClient._parse_session_message(message)
                for message in MeerkatClient._require_present_list_field(
                    data, "messages", context
                )
            ],
        )

    @staticmethod
    def _parse_session_transcript_revision_list(
        data: Any,
    ) -> SessionTranscriptRevisionList:
        context = "Invalid session transcript revision list"
        data = MeerkatClient._require_dict(data, "result", context)
        entries: list[SessionTranscriptRevisionEntry] = []
        for entry in MeerkatClient._require_present_list_field(
            data,
            "entries",
            context,
        ):
            if not isinstance(entry, dict):
                raise MeerkatError(
                    "INVALID_RESPONSE", f"{context}: entries must be objects"
                )
            entries.append(
                SessionTranscriptRevisionEntry(
                    revision=MeerkatClient._require_string_field(
                        entry, "revision", context
                    ),
                    parent_revision=MeerkatClient._require_string_field(
                        entry, "parent_revision", context
                    ),
                    actor=MeerkatClient._optional_string_field(entry, "actor", context),
                    reason=MeerkatClient._require_string_field(
                        entry, "reason", context
                    ),
                    committed_at=MeerkatClient._require_non_negative_integer_field(
                        entry,
                        "committed_at",
                        context,
                    ),
                )
            )
        return SessionTranscriptRevisionList(
            head_revision=MeerkatClient._require_string_field(
                data, "head_revision", context
            ),
            entries=entries,
        )

    @staticmethod
    def _parse_session_fork_result(data: Any) -> SessionForkResult:
        context = "Invalid session/fork response"
        data = MeerkatClient._require_dict(data, "result", context)
        return SessionForkResult(
            source_session_id=MeerkatClient._require_string_field(
                data, "source_session_id", context
            ),
            session_id=MeerkatClient._require_string_field(data, "session_id", context),
            session_ref=MeerkatClient._optional_string_field(
                data,
                "session_ref",
                context,
            ),
            message_count=MeerkatClient._require_non_negative_integer_field(
                data, "message_count", context
            ),
        )

    @staticmethod
    def _validate_transcript_rewrite_range(raw: Any, context: str) -> None:
        rewrite_range = MeerkatClient._require_dict(raw, "range", context)
        MeerkatClient._require_non_negative_integer_field(
            rewrite_range,
            "start",
            context,
        )
        MeerkatClient._require_non_negative_integer_field(
            rewrite_range,
            "end",
            context,
        )

    @staticmethod
    def _validate_transcript_rewrite_selection(raw: Any, context: str) -> None:
        selection = MeerkatClient._require_dict(raw, "selection", context)
        selection_type = MeerkatClient._require_string_field(
            selection,
            "type",
            context,
        )
        if selection_type == "message_range":
            MeerkatClient._require_non_negative_integer_field(
                selection,
                "start",
                context,
            )
            MeerkatClient._require_non_negative_integer_field(
                selection,
                "end",
                context,
            )
        elif selection_type in {
            "edit_message_range",
            "compaction_message_range",
        }:
            MeerkatClient._validate_transcript_rewrite_range(
                selection.get("range"),
                f"{context}: range",
            )
        else:
            raise MeerkatError(
                "INVALID_RESPONSE",
                f"{context}: unsupported selection type {selection_type!r}",
            )

    @staticmethod
    def _validate_transcript_rewrite_commit(
        raw: Any,
        context: str,
    ) -> dict[str, Any]:
        commit = MeerkatClient._require_dict(raw, "commit", context)
        for field in (
            "parent_revision",
            "revision",
            "original_span_digest",
            "replacement_digest",
        ):
            MeerkatClient._require_string_field(commit, field, context)
        MeerkatClient._validate_nullable_response_string(commit, "actor", context)
        for field in ("messages_before", "messages_after"):
            MeerkatClient._require_non_negative_integer_field(
                commit,
                field,
                context,
            )

        reason = MeerkatClient._require_dict(
            commit.get("reason"),
            "reason",
            context,
        )
        MeerkatClient._require_string_field(reason, "kind", f"{context}: reason")
        MeerkatClient._validate_nullable_response_string(
            reason,
            "note",
            f"{context}: reason",
        )
        MeerkatClient._validate_transcript_rewrite_selection(
            commit.get("selection"),
            f"{context}: selection",
        )

        committed_at = MeerkatClient._require_dict(
            commit.get("committed_at"),
            "committed_at",
            context,
        )
        for field in ("secs_since_epoch", "nanos_since_epoch"):
            MeerkatClient._require_non_negative_integer_field(
                committed_at,
                field,
                f"{context}: committed_at",
            )
        return commit

    @staticmethod
    def _parse_session_transcript_rewrite_result(
        data: Any,
    ) -> SessionTranscriptRewriteResult:
        context = "Invalid session transcript rewrite response"
        data = MeerkatClient._require_dict(data, "result", context)
        commit = MeerkatClient._validate_transcript_rewrite_commit(
            data.get("commit"),
            f"{context}: commit",
        )
        return SessionTranscriptRewriteResult(
            session_id=MeerkatClient._require_string_field(data, "session_id", context),
            parent_revision=MeerkatClient._require_string_field(
                data, "parent_revision", context
            ),
            revision=MeerkatClient._require_string_field(data, "revision", context),
            message_count=MeerkatClient._require_non_negative_integer_field(
                data,
                "message_count",
                context,
            ),
            commit=dict(commit),
        )

    @staticmethod
    def _parse_session_message(data: dict[str, Any]) -> SessionMessage:
        # Transcript truth fails closed: a message without its identity facts
        # (role, created_at) or with malformed collections is a wire-contract
        # violation, never coerced to ""/[] placeholder truth.
        context = "Invalid session message"
        MeerkatClient._validate_wire_history_row(data, context)
        role = MeerkatClient._require_string_field(data, "role", context)
        created_at = MeerkatClient._require_string_field(data, "created_at", context)
        content_value = (
            data.get("body")
            if role == "system_notice" and "content" not in data and "body" in data
            else data.get("content")
        )
        transcript_role = data.get("transcript_role")
        return SessionMessage(
            role=role,
            created_at=created_at,
            kind=data.get("kind") if isinstance(data.get("kind"), str) else None,
            body=data.get("body") if isinstance(data.get("body"), str) else None,
            content=MeerkatClient._parse_content_input(content_value)
            if content_value is not None
            else None,
            transcript_role=cast(
                TranscriptUserRole | None,
                transcript_role,
            ),
            stop_reason=data.get("stop_reason"),
            interaction_id=data.get("interaction_id"),
            run_id=data.get("run_id"),
            blocks=[
                MeerkatClient._parse_session_assistant_block(block)
                for block in MeerkatClient._require_list_field(data, "blocks", context)
            ]
            if role == "block_assistant"
            else [],
            results=[
                MeerkatClient._parse_session_tool_result(result, context)
                for result in MeerkatClient._require_list_field(
                    data, "results", context
                )
            ]
            if role == "tool_results"
            else [],
            raw=dict(data),
        )

    @staticmethod
    def _parse_session_tool_result(result: Any, context: str) -> SessionToolResult:
        MeerkatClient._validate_wire_tool_result(result, context)
        result = cast(dict[str, Any], result)
        is_error = result.get("is_error", False)
        return SessionToolResult(
            tool_use_id=MeerkatClient._require_string_field(
                result, "tool_use_id", context
            ),
            content=MeerkatClient._parse_content_input(result["content"]),
            is_error=is_error,
        )

    @staticmethod
    def _serialize_transcript_rewrite_message(
        message: TranscriptRewriteInputMessage,
    ) -> dict[str, Any]:
        if isinstance(message, SessionMessage):
            payload: dict[str, Any] = dict(message.raw)
            payload.update(
                {
                    "role": message.role,
                    "created_at": message.created_at,
                }
            )
            if message.kind is not None:
                payload["kind"] = message.kind
            if message.role == "user" and message.transcript_role is not None:
                payload["transcript_role"] = message.transcript_role
            if message.role == "system_notice":
                payload.pop("body", None)
                payload.pop("content", None)
                if message.body is not None:
                    payload["body"] = message.body
                elif message.content is not None:
                    payload["content"] = message.content
            else:
                if message.body is not None:
                    payload["body"] = message.body
                if message.content is not None:
                    payload["content"] = message.content
            if message.stop_reason is not None:
                payload["stop_reason"] = message.stop_reason
            if message.blocks:
                payload["blocks"] = [
                    MeerkatClient._serialize_session_assistant_block(block)
                    for block in message.blocks
                ]
            if message.results:
                payload["results"] = [
                    {
                        "tool_use_id": result.tool_use_id,
                        "content": result.content,
                        "is_error": result.is_error,
                    }
                    for result in message.results
                ]
            return payload
        return dict(message)

    @staticmethod
    def _serialize_session_assistant_block(
        block: SessionAssistantBlock,
    ) -> dict[str, Any]:
        raw_data = block.raw.get("data")
        data: dict[str, Any] = dict(raw_data) if isinstance(raw_data, dict) else {}
        if block.text is not None:
            data["text"] = block.text
        if block.id is not None:
            data["id"] = block.id
        if block.name is not None:
            data["name"] = block.name
        if block.args is not None:
            data["args"] = block.args
        if block.image_id is not None:
            data["image_id"] = block.image_id
        if block.blob_id is not None:
            raw_blob_ref = data.get("blob_ref")
            blob_ref = dict(raw_blob_ref) if isinstance(raw_blob_ref, dict) else {}
            blob_ref["blob_id"] = block.blob_id
            data["blob_ref"] = blob_ref
        if block.media_type is not None:
            data["media_type"] = block.media_type
        if block.width is not None:
            data["width"] = block.width
        if block.height is not None:
            data["height"] = block.height
        if block.revised_prompt is not None:
            data["revised_prompt"] = block.revised_prompt
        if block.meta is not None:
            data["meta"] = block.meta
        if block.source is not None:
            data["source"] = block.source
        payload = dict(block.raw)
        payload["block_type"] = block.block_type
        if block.block_type == "unknown" and "data" not in block.raw and not data:
            payload.pop("data", None)
        else:
            payload["data"] = data
        return payload

    @staticmethod
    def _parse_content_input(value: Any) -> SessionContentInput:
        if value is None:
            # Explicit `content: null` is a wire-contract violation (the
            # TypeScript and Rust surfaces reject it); never coalesce it into
            # empty-string transcript truth.
            raise MeerkatError("INVALID_RESPONSE", "content must not be null")
        if isinstance(value, list):
            return [
                MeerkatClient._parse_content_block(
                    block,
                    f"Invalid session content: content[{index}]",
                )
                for index, block in enumerate(value)
            ]
        if isinstance(value, str):
            return value
        raise MeerkatError(
            "INVALID_RESPONSE",
            "Invalid session content: expected string or content-block list",
        )

    @staticmethod
    def _parse_content_block(
        data: Any,
        context: str = "Invalid session content block",
    ) -> SessionContentBlock:
        block = MeerkatClient._validate_wire_content_block(data, context)
        # Keep the validated wire projection byte-for-byte meaningful. In
        # particular structured/unknown variants and media source payloads are
        # never collapsed into fabricated text/inline content.
        return cast(SessionContentBlock, dict(block))

    @staticmethod
    def _parse_session_assistant_block(data: dict[str, Any]) -> SessionAssistantBlock:
        context = "Invalid session assistant block"
        MeerkatClient._validate_wire_assistant_block(data, context)
        block_data = data.get("data", {})
        block_data = cast(dict[str, Any], block_data)
        blob_ref = block_data.get("blob_ref")
        blob_id = blob_ref.get("blob_id") if isinstance(blob_ref, dict) else None
        source_raw = block_data.get("source")
        source = dict(source_raw) if isinstance(source_raw, dict) else None
        return SessionAssistantBlock(
            block_type=MeerkatClient._require_string_field(data, "block_type", context),
            text=block_data.get("text"),
            id=block_data.get("id"),
            name=block_data.get("name"),
            args=block_data.get("args"),
            image_id=block_data.get("image_id"),
            blob_id=str(blob_id) if blob_id is not None else None,
            media_type=block_data.get("media_type"),
            width=block_data.get("width"),
            height=block_data.get("height"),
            revised_prompt=block_data.get("revised_prompt")
            if isinstance(block_data.get("revised_prompt"), dict)
            else None,
            meta=block_data.get("meta"),
            source=source,
            raw=dict(data),
        )

    @staticmethod
    def _parse_int(raw: Any, default: int = 0) -> int:
        try:
            return int(raw)
        except (TypeError, ValueError):
            return default

    @staticmethod
    def _parse_optional_string_map_field(
        raw: dict[str, Any], field: str, context: str
    ) -> dict[str, str]:
        if field not in raw:
            return {}
        value = raw[field]
        if not isinstance(value, dict):
            raise MeerkatError(
                "INVALID_RESPONSE", f"{context}: {field} must be object"
            )
        if not all(
            isinstance(key, str) and isinstance(item, str)
            for key, item in value.items()
        ):
            raise MeerkatError(
                "INVALID_RESPONSE",
                f"{context}: {field} keys and values must be strings",
            )
        return dict(value)

    @staticmethod
    def _parse_session_summary(s: dict[str, Any]) -> SessionSummary:
        context = "Invalid session/list response entry"
        return SessionSummary(
            session_id=MeerkatClient._require_string_field(s, "session_id", context),
            session_ref=MeerkatClient._optional_string_field(s, "session_ref", context),
            created_at=MeerkatClient._require_non_negative_integer_field(
                s, "created_at", context
            ),
            updated_at=MeerkatClient._require_non_negative_integer_field(
                s, "updated_at", context
            ),
            message_count=MeerkatClient._require_non_negative_integer_field(
                s, "message_count", context
            ),
            total_tokens=MeerkatClient._require_non_negative_integer_field(
                s, "total_tokens", context
            ),
            labels=MeerkatClient._parse_optional_string_map_field(
                s, "labels", context
            ),
            is_active=MeerkatClient._require_bool_field(s, "is_active", context),
        )

    @staticmethod
    def _parse_session_details(s: Any) -> SessionDetails:
        context = "Invalid session/read response"
        data = MeerkatClient._require_dict(s, "result", context)
        return SessionDetails(
            session_id=MeerkatClient._require_string_field(
                data, "session_id", context
            ),
            session_ref=MeerkatClient._optional_string_field(
                data, "session_ref", context
            ),
            created_at=MeerkatClient._require_non_negative_integer_field(
                data, "created_at", context
            ),
            updated_at=MeerkatClient._require_non_negative_integer_field(
                data, "updated_at", context
            ),
            message_count=MeerkatClient._require_non_negative_integer_field(
                data, "message_count", context
            ),
            labels=MeerkatClient._parse_optional_string_map_field(
                data, "labels", context
            ),
            is_active=MeerkatClient._require_bool_field(data, "is_active", context),
            model=MeerkatClient._require_string_field(data, "model", context),
            provider=MeerkatClient._require_string_field(data, "provider", context),
            last_assistant_text=MeerkatClient._optional_string_field(
                data, "last_assistant_text", context
            ),
            resolved_capabilities=MeerkatClient._parse_resolved_model_capabilities(
                data.get("resolved_capabilities")
            ),
        )

    @staticmethod
    def _parse_resolved_model_capabilities(
        raw: Any,
    ) -> ResolvedModelCapabilities | None:
        if not isinstance(raw, dict):
            return None
        required = [
            "vision",
            "image_input",
            "image_tool_results",
            "inline_video",
            "realtime",
            "web_search",
            "image_generation",
        ]
        if any(not isinstance(raw.get(field), bool) for field in required):
            return None
        return {
            "vision": raw["vision"],
            "image_input": raw["image_input"],
            "image_tool_results": raw["image_tool_results"],
            "inline_video": raw["inline_video"],
            "realtime": raw["realtime"],
            "web_search": raw["web_search"],
            "image_generation": raw["image_generation"],
        }

    @staticmethod
    def _parse_models_catalog(data: dict[str, Any]) -> ModelsCatalogResponse:
        # Fail closed on malformed shapes instead of silently coalescing them
        # (parity with the TypeScript SDK): a non-list `providers` or a
        # non-object entry is a broken response, not an empty catalog.
        providers_raw = data.get("providers")
        if not isinstance(providers_raw, list):
            raise MeerkatError(
                "INVALID_RESPONSE",
                "Invalid models/catalog response: providers must be a list",
            )
        providers: list[dict[str, Any]] = []
        for provider in providers_raw:
            if not isinstance(provider, dict):
                raise MeerkatError(
                    "INVALID_RESPONSE",
                    "Invalid models/catalog response: provider entries must be objects",
                )
            providers.append(provider)
        contract_version = MeerkatClient._parse_contract_version(
            data.get("contract_version"),
            "Invalid models/catalog response",
        )
        return {
            "contract_version": contract_version,
            "providers": providers,
        }

    @staticmethod
    def _parse_contract_version(raw: Any, context: str) -> dict[str, int]:
        def parse_component(value: Any, field: str) -> int:
            message = (
                f"{context}: contract_version.{field} must be non-negative integer"
            )
            if isinstance(value, bool):
                raise MeerkatError(
                    "INVALID_RESPONSE",
                    message,
                )
            if isinstance(value, int) and value >= 0:
                return value
            if isinstance(value, str) and value.isdigit():
                return int(value)
            raise MeerkatError(
                "INVALID_RESPONSE",
                message,
            )

        if isinstance(raw, dict):
            return {
                "major": parse_component(raw.get("major"), "major"),
                "minor": parse_component(raw.get("minor"), "minor"),
                "patch": parse_component(raw.get("patch"), "patch"),
            }
        if isinstance(raw, str):
            parts = raw.split(".")
            if len(parts) == 3:
                return {
                    "major": parse_component(parts[0], "major"),
                    "minor": parse_component(parts[1], "minor"),
                    "patch": parse_component(parts[2], "patch"),
                }
        raise MeerkatError(
            "INVALID_RESPONSE",
            f"{context}: missing contract_version",
        )
