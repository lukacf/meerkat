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
from dataclasses import asdict, is_dataclass
import json
import os
import platform
import shutil
import tarfile
import tempfile
import warnings
import zipfile
from pathlib import Path
from typing import Any, Literal, NotRequired, TypedDict
from urllib.error import URLError
import urllib.request

from .errors import CapabilityUnavailableError, MeerkatError
from .events import Usage, parse_event
from .generated.types import CONTRACT_VERSION
from .generated.types import (
    McpServerConfig,
    MobDefinitionInput,
    MobSpawnManyFailedResult,
    MobSpawnManyResult,
    MobSpawnManyResultEntry,
    MobSpawnManySpawnedResult,
    MobRotateSupervisorResult,
    MobTurnStartParams,
    RealtimeCapabilitiesResult,
    RealtimeOpenInfo,
    RealtimeOpenRequest,
    RealtimeStatusResult,
    RuntimeRealtimeAttachmentStatusResult,
    WireBudgetSplitPolicy,
    WireAuthBindingRef,
    WireContentInput,
    WireMemberLaunchMode,
    WireMobBackendKind,
    WireMobProfile,
    WireMobRuntimeMode,
    WireRuntimeBinding,
    WireToolAccessPolicy,
    WireToolFilter,
)
from .mob import (
    Mob,
    MobHelperResult,
    MobKickoffMemberSnapshot,
    MobLifecycleAction,
    MobMember,
    MobMemberRef,
    MobMemberSnapshot,
    MobReadyMemberSnapshot,
    MobSpawnResult,
    MobSpawnSpec,
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
    ExternalEventOutcome,
    ContentBlock,
    ContentInput,
    ModelsCatalogResponse,
    MobEventsResult,
    MobProfile,
    PeerCorrelationId,
    PeerId,
    ScheduleListResult,
    ScheduleOccurrencesResult,
    ScheduleRecord,
    ScheduleToolsResult,
    ScheduleToolCall,
    EventEnvelope,
    EventSourceIdentity,
    McpLiveOpResponse,
    RunResult,
    SchemaWarning,
    SessionDetails,
    SessionAssistantBlock,
    SessionHistory,
    SessionSummary,
    SessionMessage,
    SessionToolCall,
    SessionToolResult,
    SkillKey,
    SkillQuarantineDiagnostic,
    SkillRef,
    SkillRuntimeDiagnostics,
    SourceHealthSnapshot,
    StoredMobProfile,
)

_MEERKAT_REPO = ("lukacf", "meerkat")
_MEERKAT_BINARY = "rkat-rpc"
_MEERKAT_BINARY_CACHE_ROOT = Path.home() / ".cache" / "meerkat" / "bin" / _MEERKAT_BINARY
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
        return {
            key: _wire_value(item)
            for key, item in asdict(value).items()
            if item is not None
        }
    if isinstance(value, dict):
        return {
            key: _wire_value(item)
            for key, item in value.items()
        }
    if isinstance(value, list):
        return [_wire_value(item) for item in value]
    return value


def _wire_params(value: Any) -> dict[str, Any]:
    converted = _wire_value(value)
    return converted if isinstance(converted, dict) else dict(converted)


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
                name, fn, description=description, input_schema=input_schema,
            )
            # If already connected, register the new tool with the server immediately.
            if self._process and self._process.stdin:
                asyncio.ensure_future(self._register_tool_with_server(name, description, input_schema))
            return fn
        return decorator

    async def _register_tool_with_server(
        self,
        name: str,
        description: str,
        input_schema: dict[str, Any] | None,
    ) -> None:
        """Best-effort registration of a single tool with the server."""
        try:
            await self._request(
                "tools/register",
                {"tools": [{"name": name, "description": description or f"Tool: {name}",
                             "input_schema": input_schema or {"type": "object"}}]},
            )
        except Exception:
            pass  # Best-effort — server may be shutting down.

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
    ) -> MeerkatClient:
        """Start the rkat-rpc subprocess and perform handshake."""
        if realm_id and isolated:
            raise MeerkatError("INVALID_ARGS", "realm_id and isolated cannot both be set")

        resolved_path, use_legacy = await self._resolve_rkat_binary(self._rkat_path)
        self._rkat_path = resolved_path
        self._legacy_rpc_subcommand = use_legacy

        has_advanced_opts = any([
            isolated, realm_id, instance_id, realm_backend,
            state_root, context_root, user_config_root,
        ])
        if self._legacy_rpc_subcommand and has_advanced_opts:
            raise MeerkatError(
                "LEGACY_BINARY_UNSUPPORTED",
                "Realm/context options require the standalone rkat-rpc binary. "
                "Install rkat-rpc and retry.",
            )

        args = self._build_args(
            use_legacy, isolated=isolated, realm_id=realm_id,
            instance_id=instance_id, realm_backend=realm_backend,
            state_root=state_root, context_root=context_root,
            user_config_root=user_config_root,
        )

        self._process = await asyncio.create_subprocess_exec(
            self._rkat_path, *args,
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

        result = await self._request("initialize", {})
        server_version = result.get("contract_version", "")
        if not self._check_version_compatible(server_version, CONTRACT_VERSION):
            raise MeerkatError(
                "VERSION_MISMATCH",
                f"Server version {server_version} incompatible with SDK {CONTRACT_VERSION}",
            )
        self._methods = {str(method) for method in result.get("methods", [])}

        caps_result = await self._request("capabilities/get", {})
        self._capabilities = [
            Capability(
                id=c.get("id", ""),
                description=c.get("description", ""),
                status=self._normalize_status(c.get("status", "Available")),
            )
            for c in caps_result.get("capabilities", [])
        ]

        # Register callback tools with the server if any were declared.
        if self._tool_registry:
            await self._request(
                "tools/register",
                {"tools": self._tool_registry.definitions()},
            )

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
        return list(result.get("realms", []))

    async def get_realm(self, realm_id: str) -> dict[str, Any]:
        """Fetch one realm's full WireRealmConnectionSet. Delegates to
        `realm/get`."""
        return await self._request("realm/get", {"realm_id": realm_id})

    async def list_auth_profiles(self, realm_id: str) -> dict[str, Any]:
        """List auth profiles / backend profiles / bindings for one
        realm. Delegates to `auth/profile/list`."""
        return await self._request("auth/profile/list", {"realm_id": realm_id})

    async def get_auth_profile(
        self, realm_id: str, binding_id: str, profile_id: str | None = None
    ) -> dict[str, Any]:
        """Fetch a binding-scoped auth profile via `auth/profile/get`."""
        params: dict[str, Any] = {"realm_id": realm_id, "binding_id": binding_id}
        if profile_id is not None:
            params["profile_id"] = profile_id
        return await self._request("auth/profile/get", params)

    async def create_auth_profile(
        self, params: dict[str, Any]
    ) -> dict[str, Any]:
        """Create binding-scoped credentials via `auth/profile/create`."""
        return await self._request("auth/profile/create", params)

    async def delete_auth_profile(
        self, realm_id: str, binding_id: str, profile_id: str | None = None
    ) -> None:
        """Clear a profile's persisted credentials via
        `auth/profile/delete`."""
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
        return await self._request(
            "auth/login/device_start", {"provider": provider}
        )

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
        params: dict[str, Any] = {"realm_id": realm_id, "binding_id": binding_id}
        if profile_id is not None:
            params["profile_id"] = profile_id
        return await self._request("auth/status/get", params)

    async def auth_logout(
        self, realm_id: str, binding_id: str, profile_id: str | None = None
    ) -> dict[str, Any]:
        """Revoke + delete a binding's persisted credentials via
        `auth/logout`."""
        params: dict[str, Any] = {"realm_id": realm_id, "binding_id": binding_id}
        if profile_id is not None:
            params["profile_id"] = profile_id
        return await self._request("auth/logout", params)

    # -- Session lifecycle -------------------------------------------------

    async def create_session(
        self,
        prompt: str | list[ContentBlock],
        *,
        model: str | None = None,
        provider: str | None = None,
        auth_binding: dict[str, str] | None = None,
        system_prompt: str | None = None,
        max_tokens: int | None = None,
        output_schema: dict[str, Any] | None = None,
        structured_output_retries: int | None = None,
        hooks_override: dict[str, Any] | None = None,
        enable_builtins: bool | None = None,
        enable_shell: bool | None = None,
        enable_memory: bool | None = None,
        enable_mob: bool | None = None,
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
        params = self._build_create_params(
            prompt, model=model, provider=provider, auth_binding=auth_binding,
            system_prompt=system_prompt,
            max_tokens=max_tokens, output_schema=output_schema,
            structured_output_retries=structured_output_retries,
            hooks_override=hooks_override, enable_builtins=enable_builtins,
            enable_shell=enable_shell,
            enable_memory=enable_memory, enable_mob=enable_mob,
            keep_alive=keep_alive,
            comms_name=comms_name, peer_meta=peer_meta,
            budget_limits=budget_limits, provider_params=provider_params,
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
        model: str | None = None,
        provider: str | None = None,
        auth_binding: dict[str, str] | None = None,
        system_prompt: str | None = None,
        max_tokens: int | None = None,
        output_schema: dict[str, Any] | None = None,
        structured_output_retries: int | None = None,
        hooks_override: dict[str, Any] | None = None,
        enable_builtins: bool | None = None,
        enable_shell: bool | None = None,
        enable_memory: bool | None = None,
        enable_mob: bool | None = None,
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
        if not self._dispatcher or not self._process or not self._process.stdin:
            raise MeerkatError("NOT_CONNECTED", "Client not connected")
        params = self._build_create_params(
            prompt, model=model, provider=provider, auth_binding=auth_binding,
            system_prompt=system_prompt,
            max_tokens=max_tokens, output_schema=output_schema,
            structured_output_retries=structured_output_retries,
            hooks_override=hooks_override, enable_builtins=enable_builtins,
            enable_shell=enable_shell,
            enable_memory=enable_memory, enable_mob=enable_mob,
            keep_alive=keep_alive,
            comms_name=comms_name, peer_meta=peer_meta,
            budget_limits=budget_limits, provider_params=provider_params,
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
        request = {"jsonrpc": "2.0", "id": request_id, "method": "session/create", "params": params}
        data = (json.dumps(request) + "\n").encode()
        return EventStream(
            session_id="",
            event_queue=event_queue,
            response_future=response_future,
            dispatcher=self._dispatcher,
            parse_result=self._parse_run_result,
            pending_send=(self._process.stdin, data),
        )

    async def create_deferred_session(
        self,
        prompt: str | list[ContentBlock],
        *,
        model: str | None = None,
        provider: str | None = None,
        auth_binding: dict[str, str] | None = None,
        system_prompt: str | None = None,
        max_tokens: int | None = None,
        output_schema: dict[str, Any] | None = None,
        structured_output_retries: int | None = None,
        hooks_override: dict[str, Any] | None = None,
        enable_builtins: bool | None = None,
        enable_shell: bool | None = None,
        enable_memory: bool | None = None,
        enable_mob: bool | None = None,
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
        params = self._build_create_params(
            prompt, model=model, provider=provider, auth_binding=auth_binding,
            system_prompt=system_prompt,
            max_tokens=max_tokens, output_schema=output_schema,
            structured_output_retries=structured_output_retries,
            hooks_override=hooks_override, enable_builtins=enable_builtins,
            enable_shell=enable_shell,
            enable_memory=enable_memory, enable_mob=enable_mob,
            keep_alive=keep_alive,
            comms_name=comms_name, peer_meta=peer_meta,
            budget_limits=budget_limits, provider_params=provider_params,
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
        return DeferredSession(
            self,
            session_id=raw.get("session_id", ""),
            session_ref=raw.get("session_ref"),
        )

    # -- Session queries ---------------------------------------------------

    async def list_sessions(
        self,
        *,
        labels: dict[str, str] | None = None,
        limit: int | None = None,
        offset: int | None = None,
    ) -> list[SessionSummary]:
        """List active sessions."""
        params: dict[str, Any] = {}
        if labels is not None:
            params["labels"] = labels
        if limit is not None:
            params["limit"] = limit
        if offset is not None:
            params["offset"] = offset
        result = await self._request("session/list", params)
        return [
            self._parse_session_summary(s)
            for s in result.get("sessions", [])
        ]

    async def read_session(self, session_id: str) -> SessionDetails:
        """Read detailed session state."""
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
        params = {
            "session_id": session_id,
            "text": text,
        }
        if source:
            params["source"] = source
        if idempotency_key:
            params["idempotency_key"] = idempotency_key
        return await self._request("session/inject_context", params)

    async def read_session_history(
        self,
        session_id: str,
        *,
        offset: int = 0,
        limit: int | None = None,
    ) -> SessionHistory:
        """Read paginated session transcript history."""
        params: dict[str, Any] = {"session_id": session_id, "offset": offset}
        if limit is not None:
            params["limit"] = limit
        raw = await self._request("session/history", params)
        return self._parse_session_history(raw)

    async def get_blob(self, blob_id: str) -> BlobPayload:
        raw = await self._request("blob/get", {"blob_id": blob_id})
        return BlobPayload(
            blob_id=str(raw.get("blob_id", blob_id)),
            media_type=str(raw.get("media_type", "")),
            data_base64=str(raw.get("data", "")),
        )

    async def get_runtime_host_info(self) -> dict[str, Any]:
        return await self._request("runtime/host_info", {})

    async def get_runtime_host_capabilities(self) -> dict[str, Any]:
        return await self._request("runtime/capabilities", {})

    async def get_runtime_host_health(self) -> dict[str, Any]:
        return await self._request("runtime/health", {})

    async def request_approval(self, params: dict[str, Any]) -> dict[str, Any]:
        return await self._request("approval/request", params)

    async def list_approvals(
        self, params: dict[str, Any] | None = None
    ) -> dict[str, Any]:
        return await self._request("approval/list", params or {})

    async def get_approval(self, params: dict[str, Any]) -> dict[str, Any]:
        return await self._request("approval/get", params)

    async def decide_approval(self, params: dict[str, Any]) -> dict[str, Any]:
        return await self._request("approval/decide", params)

    async def list_artifacts(
        self, params: dict[str, Any] | None = None
    ) -> dict[str, Any]:
        return await self._request("artifact/list", params or {})

    async def get_artifact(self, params: dict[str, Any]) -> dict[str, Any]:
        return await self._request("artifact/get", params)

    async def download_artifact(self, params: dict[str, Any]) -> dict[str, Any]:
        return await self._request("artifact/download", params)

    async def latest_event_cursor(self, params: dict[str, Any]) -> dict[str, Any]:
        return await self._request("events/latest_cursor", params)

    async def list_events_since(self, params: dict[str, Any]) -> dict[str, Any]:
        return await self._request("events/list_since", params)

    async def event_snapshot(self, params: dict[str, Any]) -> dict[str, Any]:
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
        return any(
            c.id == capability_id and c.available
            for c in self.capabilities
        )

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
    ) -> ConfigEnvelope:
        params: dict[str, Any] = {"config": config}
        if expected_generation is not None:
            params["expected_generation"] = expected_generation
        return await self._request("config/set", params)

    async def patch_config(
        self,
        patch: dict[str, Any],
        *,
        expected_generation: int | None = None,
    ) -> ConfigEnvelope:
        params: dict[str, Any] = {"patch": patch}
        if expected_generation is not None:
            params["expected_generation"] = expected_generation
        return await self._request("config/patch", params)

    async def get_models_catalog(self) -> ModelsCatalogResponse:
        raw = await self._request("models/catalog", {})
        return self._parse_models_catalog(raw)

    async def create_schedule(self, request: dict[str, Any]) -> ScheduleRecord:
        return await self._request("schedule/create", request)

    async def get_schedule(self, schedule_id: str) -> ScheduleRecord:
        return await self._request("schedule/get", {"schedule_id": schedule_id})

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
        schedules = raw.get("schedules", [])
        return {
            "schedules": schedules if isinstance(schedules, list) else [],
        }

    async def update_schedule(self, request: dict[str, Any]) -> ScheduleRecord:
        return await self._request("schedule/update", request)

    async def pause_schedule(self, schedule_id: str) -> ScheduleRecord:
        return await self._request("schedule/pause", {"schedule_id": schedule_id})

    async def resume_schedule(self, schedule_id: str) -> ScheduleRecord:
        return await self._request("schedule/resume", {"schedule_id": schedule_id})

    async def delete_schedule(self, schedule_id: str) -> ScheduleRecord:
        return await self._request("schedule/delete", {"schedule_id": schedule_id})

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
        occurrences = raw.get("occurrences", [])
        return {
            "occurrences": occurrences if isinstance(occurrences, list) else [],
        }

    async def list_schedule_tools(self) -> ScheduleToolsResult:
        raw = await self._request("schedule/tools", {})
        tools = raw.get("tools", [])
        return {"tools": tools if isinstance(tools, list) else []}

    async def call_schedule_tool(self, request: ScheduleToolCall) -> dict[str, Any]:
        return await self._request("schedule/call", dict(request))

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
            raise MeerkatError("INVALID_RESPONSE", "Invalid mcp response: missing session_id")
        if operation not in {"add", "remove", "reload"}:
            raise MeerkatError("INVALID_RESPONSE", "Invalid mcp response: invalid operation")
        if not isinstance(status, str) or not status:
            raise MeerkatError("INVALID_RESPONSE", "Invalid mcp response: missing status")
        if not isinstance(persisted, bool):
            raise MeerkatError(
                "INVALID_RESPONSE",
                "Invalid mcp response: persisted must be boolean",
            )
        return McpLiveOpResponse(**raw)

    async def list_skills(self) -> list[dict[str, Any]]:
        result = await self._request("skills/list", {})
        return result.get("skills", [])

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
        result = await self._request("mob/create", {"definition": _wire_params(definition)})
        return Mob(self, str(result.get("mob_id", "")))

    def mob(self, mob_id: str) -> Mob:
        return Mob(self, mob_id)

    async def list_mobs(self) -> list[dict[str, Any]]:
        self.require_capability("mob")
        result = await self._request("mob/list", {})
        return result.get("mobs", [])

    async def mob_status(self, mob_id: str) -> dict[str, Any]:
        result = await self._request("mob/status", {"mob_id": mob_id})
        raw_status = result.get("status")
        if isinstance(raw_status, str) and raw_status:
            status = raw_status
        elif isinstance(raw_status, dict) and raw_status:
            status = next(iter(raw_status))
        else:
            raise MeerkatError("INVALID_RESPONSE", "Invalid mob/status response: missing status")
        return {"mob_id": str(result.get("mob_id", mob_id)), "status": status}

    async def list_mob_members(self, mob_id: str) -> list[MobMember]:
        result = await self._request("mob/members", {"mob_id": mob_id})
        members = result.get("members", [])
        if not isinstance(members, list):
            return []
        normalized: list[MobMember] = []
        for entry in members:
            if not isinstance(entry, dict):
                continue
            member_ref = entry.get("member_ref")
            if not isinstance(member_ref, str) or not member_ref:
                raise MeerkatError(
                    "INVALID_RESPONSE",
                    "Invalid mob/members response: entry missing member_ref",
                )
            normalized.append(
                {
                    "agent_identity": (
                        str(entry.get("agent_identity", ""))
                        if entry.get("agent_identity") is not None
                        else ""
                    ),
                    "member_ref": member_ref,
                    "profile": str(
                        entry.get("profile_name")
                        or entry.get("profile")
                        or entry.get("role")
                        or ""
                    ),
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
                        {"state": str(entry["state"])}
                        if entry.get("state") is not None
                        else {}
                    ),
                    **(
                        {
                            "wired_to": [
                                str(peer) for peer in entry["wired_to"]
                            ]
                        }
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
        resolved_identity = result.get("agent_identity")
        if not isinstance(resolved_identity, str) or not resolved_identity:
            raise MeerkatError(
                "INVALID_RESPONSE",
                "Invalid mob/member_send response: missing agent_identity",
            )
        return {
            "agent_identity": resolved_identity,
            "member_ref": member_ref,
            "handling_mode": (
                receipt_handling_mode
                if receipt_handling_mode in {"queue", "steer"}
                else handling_mode
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
        binding: WireRuntimeBinding | dict[str, Any] | None = None,
        shell_env: dict[str, str] | None = None,
        auto_wire_parent: bool | None = None,
        launch_mode: WireMemberLaunchMode | dict[str, Any] | None = None,
        tool_access_policy: WireToolAccessPolicy | dict[str, Any] | None = None,
        budget_split_policy: WireBudgetSplitPolicy | dict[str, Any] | None = None,
        inherited_tool_filter: WireToolFilter | dict[str, list[str]] | None = None,
        override_profile: WireMobProfile | dict[str, Any] | None = None,
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
            "binding": binding,
            "shell_env": shell_env,
            "auto_wire_parent": auto_wire_parent,
            "launch_mode": launch_mode,
            "tool_access_policy": tool_access_policy,
            "budget_split_policy": budget_split_policy,
            "inherited_tool_filter": inherited_tool_filter,
            "override_profile": override_profile,
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
        return {
            "mob_id": str(result.get("mob_id", mob_id)),
            "agent_identity": resolved_identity,
            "member_ref": member_ref,
        }


    async def spawn_mob_members(
        self,
        mob_id: str,
        specs: list[MobSpawnSpec],
    ) -> MobSpawnManyResult:
        params: dict[str, Any] = {
            "mob_id": mob_id,
            "specs": _wire_value(specs),
        }
        result = await self._request("mob/spawn_many", params)
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
            if not isinstance(cause, str) or cause not in _MOB_SPAWN_MANY_FAILURE_CAUSES:
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
        events = raw.get("events", [])
        return {"events": events if isinstance(events, list) else []}

    async def mob_ingress_interaction(
        self, params: dict[str, Any]
    ) -> dict[str, Any]:
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
            "status": (
                "topology_restore_failed"
                if result.get("status") == "topology_restore_failed"
                else "completed"
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

    async def mob_turn_start(
        self,
        mob_id: str,
        agent_identity: str,
        prompt: WireContentInput,
        *,
        skill_refs: list[SkillRef] | None = None,
        flow_tool_overlay: dict[str, Any] | None = None,
        additional_instructions: list[str] | None = None,
        keep_alive: bool | None = None,
        model: str | None = None,
        provider: str | None = None,
        max_tokens: int | None = None,
        system_prompt: str | None = None,
        output_schema: Any | None = None,
        structured_output_retries: int | None = None,
        provider_params: Any | None = None,
        clear_provider_params: bool | None = None,
        auth_binding: WireAuthBindingRef | dict[str, str] | None = None,
        clear_auth_binding: bool | None = None,
    ) -> dict[str, Any]:
        params = MobTurnStartParams(
            mob_id=mob_id,
            agent_identity=agent_identity,
            prompt=prompt,
            skill_refs=_skill_refs_to_wire(skill_refs),
            flow_tool_overlay=flow_tool_overlay,
            additional_instructions=additional_instructions,
            keep_alive=keep_alive,
            model=model,
            provider=provider,
            max_tokens=max_tokens,
            system_prompt=system_prompt,
            output_schema=output_schema,
            structured_output_retries=structured_output_retries,
            provider_params=provider_params,
            clear_provider_params=clear_provider_params,
            auth_binding=auth_binding,
            clear_auth_binding=clear_auth_binding,
        )
        return await self._request("mob/turn_start", _wire_value(params))

    async def mob_member_status(
        self, mob_id: str, agent_identity: str
    ) -> MobMemberSnapshot:
        result = await self._request(
            "mob/member_status",
            {"mob_id": mob_id, "agent_identity": agent_identity},
        )
        return {
            "status": self._require_string_field(
                result,
                "status",
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
            "tokens_used": self._require_number_field(
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
                {"realtime_attachment_status": str(result["realtime_attachment_status"])}
                if result.get("realtime_attachment_status") is not None
                else {}
            ),
            **(
                {"peer_connectivity": result["peer_connectivity"]}
                if isinstance(result.get("peer_connectivity"), dict)
                else {}
            ),
            **(
                {"kickoff": result["kickoff"]}
                if isinstance(result.get("kickoff"), dict)
                else {}
            ),
            # Phase 5G/T5i identity-first realtime routing: preserve
            # `current_session_id` so callers (including
            # `RealtimeChannel.mob_member`) can resolve a mob member
            # into a `session_target` for `realtime/open_info`.
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
        members = result.get("members", [])
        if not isinstance(members, list):
            return []
        normalized: list[MobKickoffMemberSnapshot] = []
        for entry in members:
            if not isinstance(entry, dict):
                continue
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
                    "tokens_used": self._require_number_field(
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
                        {"peer_connectivity": entry["peer_connectivity"]}
                        if isinstance(entry.get("peer_connectivity"), dict)
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
        members = result.get("members", [])
        if not isinstance(members, list):
            return []
        normalized: list[MobReadyMemberSnapshot] = []
        for entry in members:
            if not isinstance(entry, dict):
                continue
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
                    "tokens_used": self._require_number_field(
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
                        {"peer_connectivity": entry["peer_connectivity"]}
                        if isinstance(entry.get("peer_connectivity"), dict)
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

    async def spawn_mob_helper(
        self,
        mob_id: str,
        prompt: str,
        *,
        agent_identity: str | None = None,
        role_name: str | None = None,
        runtime_mode: str | None = None,
        backend: str | None = None,
    ) -> MobHelperResult:
        result = await self._request("mob/spawn_helper", {
            "mob_id": mob_id,
            "prompt": prompt,
            "agent_identity": agent_identity,
            "role_name": role_name,
            "runtime_mode": runtime_mode,
            "backend": backend,
        })
        member_ref = result.get("member_ref")
        if not isinstance(member_ref, str) or not member_ref:
            raise MeerkatError(
                "INVALID_RESPONSE",
                "Invalid mob/spawn_helper response: missing member_ref",
            )
        return {
            "output": str(result["output"]) if result.get("output") is not None else None,
            "tokens_used": int(result.get("tokens_used", 0)),
            "agent_identity": (
                result["agent_identity"]
                if isinstance(result.get("agent_identity"), str)
                and result["agent_identity"]
                else (agent_identity or "")
            ),
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
        fork_context: dict[str, Any] | None = None,
        runtime_mode: str | None = None,
        backend: str | None = None,
    ) -> MobHelperResult:
        result = await self._request("mob/fork_helper", {
            "mob_id": mob_id,
            "source_member_id": source_member_id,
            "prompt": prompt,
            "agent_identity": agent_identity,
            "role_name": role_name,
            "fork_context": fork_context,
            "runtime_mode": runtime_mode,
            "backend": backend,
        })
        member_ref = result.get("member_ref")
        if not isinstance(member_ref, str) or not member_ref:
            raise MeerkatError(
                "INVALID_RESPONSE",
                "Invalid mob/fork_helper response: missing member_ref",
            )
        return {
            "output": str(result["output"]) if result.get("output") is not None else None,
            "tokens_used": int(result.get("tokens_used", 0)),
            "agent_identity": (
                result["agent_identity"]
                if isinstance(result.get("agent_identity"), str)
                and result["agent_identity"]
                else (agent_identity or "")
            ),
            "member_ref": member_ref,
        }

    async def create_mob_profile(self, name: str, profile: MobProfile) -> StoredMobProfile:
        return await self._request("mob/profile/create", {"name": name, "profile": profile})

    async def get_mob_profile(self, name: str) -> StoredMobProfile | None:
        raw = await self._request("mob/profile/get", {"name": name})
        if raw.get("not_found") is True:
            return None
        return raw

    async def list_mob_profiles(self) -> list[StoredMobProfile]:
        raw = await self._request("mob/profile/list", {})
        profiles = raw.get("profiles", [])
        return profiles if isinstance(profiles, list) else []

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

    async def wire_mob_members(self, mob_id: str, member: str, peer: str | dict[str, Any]) -> None:
        payload = (
            {"mob_id": mob_id, "member": member, "peer": {"local": peer}}
            if isinstance(peer, str)
            else {"mob_id": mob_id, "member": member, "peer": peer}
        )
        await self._request("mob/wire", payload)

    async def unwire_mob_members(self, mob_id: str, member: str, peer: str | dict[str, Any]) -> None:
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
    ) -> dict[str, Any]:
        """Submit a unit of work to a mob member through the work lane.

        ``member_ref`` is the opaque handle returned by
        ``ensure_member``/``spawn_helper``/``fork_helper`` and member-list
        responses. The server resolves it to the current incarnation, so
        callers never pass raw ``generation`` / ``fence_token`` values.
        ``origin`` is ``"external"`` for user-originated turns and
        ``"internal"`` for mob-orchestration work. When ``work_ref`` is
        omitted the server generates a fresh UUID.
        """
        params: dict[str, Any] = {
            "member_ref": member_ref,
            "content": content,
            "origin": origin,
        }
        if work_ref is not None:
            params["work_ref"] = work_ref
        return await self._request("mob/submit_work", params)

    async def mob_cancel_work(self, mob_id: str, work_ref: str) -> dict[str, Any]:
        """Cancel a previously submitted unit of work.

        Wraps the ``mob/cancel_work`` RPC (DELETE_ME C4).
        """
        return await self._request(
            "mob/cancel_work", {"mob_id": mob_id, "work_ref": work_ref}
        )

    async def mob_cancel_all_work(self, member_ref: MobMemberRef) -> dict[str, Any]:
        """Cancel all in-flight work for a specific mob member.

        ``member_ref`` is the opaque handle returned by
        ``ensure_member``/``spawn_helper``/``fork_helper`` and member-list
        responses.
        """
        return await self._request(
            "mob/cancel_all_work",
            {"member_ref": member_ref},
        )

    async def append_mob_system_context(
        self,
        mob_id: str,
        agent_identity: str,
        text: str,
        *,
        source: str | None = None,
        idempotency_key: str | None = None,
    ) -> dict[str, Any]:
        result = await self._request("mob/append_system_context", {
            "mob_id": mob_id,
            "agent_identity": agent_identity,
            "text": text,
            "source": source,
            "idempotency_key": idempotency_key,
        })
        resolved_identity = result.get("agent_identity")
        if not isinstance(resolved_identity, str) or not resolved_identity:
            raise MeerkatError(
                "INVALID_RESPONSE",
                "Invalid mob/append_system_context response: missing agent_identity",
            )
        return {
            "mob_id": str(result.get("mob_id", mob_id)),
            "agent_identity": resolved_identity,
            "status": (
                "duplicate"
                if result.get("status") == "duplicate"
                else "staged"
            ),
        }

    async def list_mob_flows(self, mob_id: str) -> list[str]:
        result = await self._request("mob/flows", {"mob_id": mob_id})
        return result.get("flows", [])

    async def run_mob_flow(self, mob_id: str, flow_id: str, params: dict[str, Any] | None = None) -> str:
        result = await self._request("mob/flow_run", {"mob_id": mob_id, "flow_id": flow_id, "params": params or {}})
        return str(result.get("run_id", ""))

    async def get_mob_flow_status(self, mob_id: str, run_id: str) -> dict[str, Any] | None:
        result = await self._request("mob/flow_status", {"mob_id": mob_id, "run_id": run_id})
        return result.get("run")

    async def cancel_mob_flow(self, mob_id: str, run_id: str) -> None:
        await self._request("mob/flow_cancel", {"mob_id": mob_id, "run_id": run_id})

    async def subscribe_mob_events(self, mob_id: str) -> EventSubscription:
        return await self._open_event_subscription(
            "mob/stream_open",
            {"mob_id": mob_id},
            "mob/stream_close",
            self._parse_attributed_mob_event,
        )

    async def subscribe_mob_member_events(self, mob_id: str, agent_identity: str) -> EventSubscription:
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
            raise MeerkatError("INVALID_RESPONSE", f"{open_method} did not return stream_id")
        queue = self._dispatcher.subscribe_stream(stream_id)
        async def _close_remote(active_stream_id: str) -> None:
            self._dispatcher.unsubscribe_stream(active_stream_id)
            await self._request(close_method, {"stream_id": active_stream_id})
        return EventSubscription(
            stream_id=stream_id,
            event_queue=queue,
            dispatcher=self._dispatcher,
            close_remote=_close_remote,
            parse_event_fn=parser,
        )

    @staticmethod
    def _parse_agent_event_envelope(raw: dict[str, Any]) -> EventEnvelope:
        payload = raw.get("payload", {})
        return EventEnvelope(
            event_id=str(raw.get("event_id", "")),
            source=MeerkatClient._parse_event_source_identity(raw.get("source")),
            source_id=str(raw.get("source_id", "")),
            seq=int(raw.get("seq", 0)),
            timestamp_ms=int(raw.get("timestamp_ms", 0)),
            payload=parse_event(payload if isinstance(payload, dict) else {}),
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
        envelope = raw.get("envelope", {})
        return AttributedEvent(
            source=str(raw.get("source", "")),
            role=str(raw.get("role", "")),
            envelope=MeerkatClient._parse_agent_event_envelope(
                envelope if isinstance(envelope, dict) else {}
            ),
        )

    # -- Internal methods used by Session ----------------------------------

    async def _start_turn(
        self,
        session_id: str,
        prompt: str | list[ContentBlock],
        *,
        skill_refs: list[SkillRef] | None = None,
        flow_tool_overlay: dict[str, Any] | None = None,
        additional_instructions: list[str] | None = None,
        keep_alive: bool | None = None,
        model: str | None = None,
        provider: str | None = None,
        max_tokens: int | None = None,
        system_prompt: str | None = None,
        output_schema: dict[str, Any] | None = None,
        structured_output_retries: int | None = None,
        provider_params: dict[str, Any] | None = None,
    ) -> RunResult:
        params: dict[str, Any] = {"session_id": session_id, "prompt": prompt}
        wire_refs = _skill_refs_to_wire(skill_refs)
        if wire_refs is not None:
            params["skill_refs"] = wire_refs
        if flow_tool_overlay is not None:
            params["flow_tool_overlay"] = flow_tool_overlay
        if additional_instructions is not None:
            params["additional_instructions"] = additional_instructions
        if keep_alive is not None:
            params["keep_alive"] = keep_alive
        if model is not None:
            params["model"] = model
        if provider is not None:
            params["provider"] = provider
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
        skill_refs: list[SkillRef] | None = None,
        flow_tool_overlay: dict[str, Any] | None = None,
        additional_instructions: list[str] | None = None,
        keep_alive: bool | None = None,
        model: str | None = None,
        provider: str | None = None,
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
        wire_refs = _skill_refs_to_wire(skill_refs)
        if wire_refs is not None:
            params["skill_refs"] = wire_refs
        if flow_tool_overlay is not None:
            params["flow_tool_overlay"] = flow_tool_overlay
        if additional_instructions is not None:
            params["additional_instructions"] = additional_instructions
        if keep_alive is not None:
            params["keep_alive"] = keep_alive
        if model is not None:
            params["model"] = model
        if provider is not None:
            params["provider"] = provider
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
        request = {"jsonrpc": "2.0", "id": request_id, "method": "turn/start", "params": params}
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

    async def _interrupt(self, session_id: str) -> None:
        await self._request("turn/interrupt", {"session_id": session_id})

    async def _archive(self, session_id: str) -> None:
        await self._request("session/archive", {"session_id": session_id})

    async def _send(self, session_id: str, **kwargs: Any) -> dict[str, Any]:
        return await self.send(session_id, **kwargs)

    async def _peers(self, session_id: str) -> dict[str, Any]:
        return await self.peers(session_id)

    # Typed literal aliases mirror the Rust `CommsCommandRequest` enum
    # (`meerkat-core/src/comms.rs`). Invalid discriminator values are rejected
    # at the server's typed-serde boundary — these aliases document the
    # closed-world shape for callers.
    _CommsKind = Literal["input", "peer_message", "peer_lifecycle", "peer_request", "peer_response"]
    _HandlingMode = Literal["queue", "steer"]
    _InputSource = Literal["tcp", "uds", "stdin", "webhook", "rpc"]
    _InputStreamMode = Literal["none", "reserve_interaction"]
    _ResponseStatus = Literal["accepted", "completed", "failed"]
    _PeerLifecycleKind = Literal["mob.peer_added", "mob.peer_retired", "mob.peer_unwired"]

    async def send(
        self,
        session_id: str,
        *,
        kind: "MeerkatClient._CommsKind",
        to: str | None = None,
        body: str | None = None,
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
    ) -> dict[str, Any]:
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
        return await self._request("comms/send", payload)

    async def peers(self, session_id: str) -> dict[str, Any]:
        return await self._request("comms/peers", {"session_id": session_id})

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

    async def runtime_realtime_attachment_status(
        self, session_id: str
    ) -> RuntimeRealtimeAttachmentStatusResult:
        raw = await self._request(
            "session/realtime_attachment_status",
            {"session_id": session_id},
        )
        return RuntimeRealtimeAttachmentStatusResult(**raw)

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

    async def realtime_open_info(
        self, request: RealtimeOpenRequest | dict[str, Any]
    ) -> RealtimeOpenInfo:
        raw = await self._request("realtime/open_info", _wire_params(request))
        return RealtimeOpenInfo(**raw)

    async def realtime_status(self, params: dict[str, Any]) -> RealtimeStatusResult:
        raw = await self._request("realtime/status", _wire_params(params))
        return RealtimeStatusResult(**raw)

    async def realtime_capabilities(
        self, params: dict[str, Any]
    ) -> RealtimeCapabilitiesResult:
        raw = await self._request("realtime/capabilities", _wire_params(params))
        return RealtimeCapabilitiesResult(**raw)

    # -- Transport ---------------------------------------------------------

    async def _request(self, method: str, params: dict[str, Any]) -> dict[str, Any]:
        if not self._process or not self._process.stdin or not self._dispatcher:
            raise MeerkatError("NOT_CONNECTED", "Client not connected")
        self._request_id += 1
        request_id = self._request_id
        request = {"jsonrpc": "2.0", "id": request_id, "method": method, "params": params}
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
                raise MeerkatError("BINARY_NOT_FOUND", "MEERKAT_BIN_PATH is set to an empty path.")
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
            target = {"arm64": "aarch64-apple-darwin", "aarch64": "aarch64-apple-darwin"}.get(machine)
            if target is None:
                raise MeerkatError("UNSUPPORTED_PLATFORM", f"Unsupported macOS architecture '{machine}'")
            return target, "tar.gz", _MEERKAT_BINARY
        if system == "linux":
            if machine in {"x86_64", "amd64"}:
                return "x86_64-unknown-linux-gnu", "tar.gz", _MEERKAT_BINARY
            if machine in {"aarch64", "arm64"}:
                return "aarch64-unknown-linux-gnu", "tar.gz", _MEERKAT_BINARY
            raise MeerkatError("UNSUPPORTED_PLATFORM", f"Unsupported Linux architecture '{machine}'.")
        if system == "windows":
            if machine in {"x86_64", "amd64"}:
                return "x86_64-pc-windows-msvc", "zip", f"{_MEERKAT_BINARY}.exe"
            raise MeerkatError("UNSUPPORTED_PLATFORM", f"Unsupported Windows architecture '{machine}'.")
        raise MeerkatError("UNSUPPORTED_PLATFORM", f"Unsupported platform '{system}'.")

    @staticmethod
    async def _download_rkat_rpc_binary() -> str | None:
        target, archive_ext, binary_name = MeerkatClient._platform_target()
        owner, repo = _MEERKAT_REPO
        version = CONTRACT_VERSION
        artifact = f"{_MEERKAT_BINARY}-v{version}-{target}.{archive_ext}"
        url = f"https://github.com/{owner}/{repo}/releases/download/v{version}/{artifact}"
        cache_dir = _MEERKAT_BINARY_CACHE_ROOT / f"v{version}" / target
        cache_dir.mkdir(parents=True, exist_ok=True)
        cached = cache_dir / binary_name
        if cached.exists():
            return str(cached)

        try:
            with urllib.request.urlopen(url, timeout=30) as response:
                payload = response.read()
        except (URLError, OSError) as exc:
            raise MeerkatError("BINARY_DOWNLOAD_FAILED", f"Failed to download {_MEERKAT_BINARY} from {url}: {exc}") from exc

        with tempfile.TemporaryDirectory() as tmpdir:
            archive_path = Path(tmpdir) / f"{_MEERKAT_BINARY}.{archive_ext}"
            with archive_path.open("wb") as fp:
                fp.write(payload)

            temp_binary = cache_dir / f".{binary_name}.download"
            if archive_ext == "tar.gz":
                with tarfile.open(archive_path, "r:gz") as archive:
                    members = [m for m in archive.getmembers() if Path(m.name).name == binary_name]
                    if not members:
                        raise MeerkatError("BINARY_DOWNLOAD_FAILED", f"Downloaded archive does not contain '{binary_name}'.")
                    source = archive.extractfile(members[0])
                    if source is None:
                        raise MeerkatError("BINARY_DOWNLOAD_FAILED", f"Could not extract '{binary_name}' from {artifact}.")
                    with source, temp_binary.open("wb") as out:
                        out.write(source.read())
            else:
                with zipfile.ZipFile(archive_path, "r") as archive:
                    candidate_names = [entry.filename for entry in archive.infolist()]
                    if not any(Path(name).name == binary_name for name in candidate_names):
                        raise MeerkatError("BINARY_DOWNLOAD_FAILED", f"Downloaded archive does not contain '{binary_name}'.")
                    with archive.open(
                        next(name for name in candidate_names if Path(name).name == binary_name),
                    ) as source, temp_binary.open("wb") as out:
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
    ) -> list[str]:
        if legacy:
            return ["rpc"]
        args: list[str] = ["--realtime-ws", "127.0.0.1:0"]
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
        model: str | None = None,
        provider: str | None = None,
        auth_binding: dict[str, str] | None = None,
        system_prompt: str | None = None,
        max_tokens: int | None = None,
        output_schema: dict[str, Any] | None = None,
        structured_output_retries: int | None = None,
        hooks_override: dict[str, Any] | None = None,
        enable_builtins: bool | None = None,
        enable_shell: bool | None = None,
        enable_memory: bool | None = None,
        enable_mob: bool | None = None,
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
        if model:
            params["model"] = model
        if provider:
            params["provider"] = provider
        if auth_binding is not None:
            params["auth_binding"] = auth_binding
        if system_prompt:
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
        if enable_mob is not None:
            params["enable_mob"] = enable_mob
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
    def _normalize_status(raw: Any) -> str:
        if isinstance(raw, str):
            return raw
        if isinstance(raw, dict):
            # Rust can emit externally-tagged enums for status:
            # {"DisabledByPolicy": {"description": "..."}}
            return next(iter(raw), "Unknown")
        return str(raw)

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
    def _require_number_field(
        raw: dict[str, Any],
        field: str,
        context: str,
        display_field: str | None = None,
    ) -> int | float:
        value = raw.get(field)
        if not isinstance(value, (int, float)) or isinstance(value, bool):
            raise MeerkatError(
                "INVALID_RESPONSE",
                f"{context}: {display_field or field} must be number",
            )
        return value

    @staticmethod
    def _require_bool_field(raw: dict[str, Any], field: str, context: str) -> bool:
        value = raw.get(field)
        if not isinstance(value, bool):
            raise MeerkatError("INVALID_RESPONSE", f"{context}: {field} must be boolean")
        return value

    @staticmethod
    def _parse_skill_diagnostics(raw: Any) -> SkillRuntimeDiagnostics | None:
        if not isinstance(raw, dict):
            return None

        source_health_raw = raw.get("source_health")
        source_health = SourceHealthSnapshot()
        if isinstance(source_health_raw, dict):
            source_health = SourceHealthSnapshot(
                state=str(source_health_raw.get("state", "")),
                invalid_ratio=float(source_health_raw.get("invalid_ratio", 0.0)),
                invalid_count=int(source_health_raw.get("invalid_count", 0)),
                total_count=int(source_health_raw.get("total_count", 0)),
                failure_streak=int(source_health_raw.get("failure_streak", 0)),
                handshake_failed=bool(source_health_raw.get("handshake_failed", False)),
            )

        quarantined_items: list[SkillQuarantineDiagnostic] = []
        raw_quarantined = raw.get("quarantined")
        if isinstance(raw_quarantined, list):
            for item in raw_quarantined:
                if not isinstance(item, dict):
                    continue
                quarantined_items.append(
                    SkillQuarantineDiagnostic(
                        source_uuid=str(item.get("source_uuid", "")),
                        skill_id=str(item.get("skill_id", "")),
                        location=str(item.get("location", "")),
                        error_code=str(item.get("error_code", "")),
                        error_class=str(item.get("error_class", "")),
                        message=str(item.get("message", "")),
                        first_seen_unix_secs=int(item.get("first_seen_unix_secs", 0)),
                        last_seen_unix_secs=int(item.get("last_seen_unix_secs", 0)),
                    )
                )

        return SkillRuntimeDiagnostics(
            source_health=source_health,
            quarantined=quarantined_items,
        )

    @staticmethod
    def _check_version_compatible(server: str, client: str) -> bool:
        try:
            s_parts = [int(x) for x in server.split(".")]
            c_parts = [int(x) for x in client.split(".")]
            if s_parts[0] == 0 and c_parts[0] == 0:
                return s_parts[1] == c_parts[1]
            return s_parts[0] == c_parts[0]
        except (ValueError, IndexError):
            return False

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
            schema_warnings = [
                SchemaWarning(
                    provider=w.get("provider", ""),
                    path=w.get("path", ""),
                    message=w.get("message", ""),
                )
                for w in raw_warnings
            ]
        return RunResult(
            session_id=data.get("session_id", ""),
            text=data.get("text", ""),
            turns=MeerkatClient._require_number_field(data, "turns", context),
            tool_calls=MeerkatClient._require_number_field(data, "tool_calls", context),
            usage=usage,
            terminal_cause_kind=data.get("terminal_cause_kind")
            if isinstance(data.get("terminal_cause_kind"), str)
            else None,
            session_ref=data.get("session_ref"),
            structured_output=data.get("structured_output"),
            schema_warnings=schema_warnings,
            skill_diagnostics=MeerkatClient._parse_skill_diagnostics(
                data.get("skill_diagnostics")
            ),
        )

    @staticmethod
    def _parse_session_history(data: dict[str, Any]) -> SessionHistory:
        return SessionHistory(
            session_id=data.get("session_id", ""),
            session_ref=data.get("session_ref"),
            message_count=data.get("message_count", 0),
            offset=data.get("offset", 0),
            limit=data.get("limit"),
            has_more=bool(data.get("has_more", False)),
            messages=[
                MeerkatClient._parse_session_message(message)
                for message in data.get("messages", [])
            ],
        )

    @staticmethod
    def _parse_session_message(data: dict[str, Any]) -> SessionMessage:
        role = data.get("role", "")
        return SessionMessage(
            role=role,
            created_at=str(data.get("created_at", "")),
            content=MeerkatClient._parse_content_input(data["content"]) if "content" in data else None,
            tool_calls=[
                SessionToolCall(
                    id=tool_call.get("id", ""),
                    name=tool_call.get("name", ""),
                    args=tool_call.get("args"),
                )
                for tool_call in data.get("tool_calls", [])
            ],
            stop_reason=data.get("stop_reason"),
            blocks=[
                MeerkatClient._parse_session_assistant_block(block)
                for block in data.get("blocks", [])
            ],
            results=[
                SessionToolResult(
                    tool_use_id=result.get("tool_use_id", ""),
                    content=MeerkatClient._parse_content_input(result.get("content", "")),
                    is_error=bool(result.get("is_error", False)),
                )
                for result in data.get("results", [])
            ],
        )

    @staticmethod
    def _parse_content_input(value: Any) -> ContentInput:
        if isinstance(value, list):
            return [
                MeerkatClient._parse_content_block(block)
                for block in value
                if isinstance(block, dict)
            ]
        return "" if value is None else str(value)

    @staticmethod
    def _parse_content_block(data: dict[str, Any]) -> ContentBlock:
        if data.get("type") == "image":
            source = data.get("source", "inline")
            if source == "blob":
                return {
                    "type": "image",
                    "media_type": str(data.get("media_type", "")),
                    "source": "blob",
                    "blob_id": str(data.get("blob_id", "")),
                }
            return {
                "type": "image",
                "media_type": str(data.get("media_type", "")),
                "source": "inline",
                "data": str(data.get("data", "")),
            }
        if data.get("type") == "video":
            return {
                "type": "video",
                "media_type": str(data.get("media_type", "")),
                "duration_ms": int(data.get("duration_ms", 0) or 0),
                "source": "inline",
                "data": str(data.get("data", "")),
            }
        return {
            "type": "text",
            "text": str(data.get("text", "")),
        }

    @staticmethod
    def _parse_session_assistant_block(data: dict[str, Any]) -> SessionAssistantBlock:
        block_data = data.get("data", {})
        if not isinstance(block_data, dict):
            block_data = {}
        blob_ref = block_data.get("blob_ref")
        blob_id = blob_ref.get("blob_id") if isinstance(blob_ref, dict) else None
        return SessionAssistantBlock(
            block_type=data.get("block_type", ""),
            text=block_data.get("text"),
            id=block_data.get("id"),
            name=block_data.get("name"),
            args=block_data.get("args"),
            image_id=block_data.get("image_id"),
            blob_id=str(blob_id) if blob_id is not None else None,
            media_type=block_data.get("media_type"),
            width=MeerkatClient._parse_int(block_data.get("width")) if "width" in block_data else None,
            height=MeerkatClient._parse_int(block_data.get("height")) if "height" in block_data else None,
            revised_prompt=block_data.get("revised_prompt") if isinstance(block_data.get("revised_prompt"), dict) else None,
            meta=block_data.get("meta"),
        )

    @staticmethod
    def _parse_int(raw: Any, default: int = 0) -> int:
        try:
            return int(raw)
        except (TypeError, ValueError):
            return default

    @staticmethod
    def _parse_string_map(raw: Any) -> dict[str, str]:
        if not isinstance(raw, dict):
            return {}
        result: dict[str, str] = {}
        for key, value in raw.items():
            result[str(key)] = str(value)
        return result

    @staticmethod
    def _parse_session_summary(s: dict[str, Any]) -> SessionSummary:
        return SessionSummary(
            session_id=str(s.get("session_id", "")),
            session_ref=s.get("session_ref"),
            created_at=MeerkatClient._parse_int(s.get("created_at"), 0),
            updated_at=MeerkatClient._parse_int(s.get("updated_at"), 0),
            message_count=MeerkatClient._parse_int(s.get("message_count"), 0),
            total_tokens=MeerkatClient._parse_int(s.get("total_tokens"), 0),
            labels=MeerkatClient._parse_string_map(s.get("labels")),
            is_active=bool(s.get("is_active", False)),
        )

    @staticmethod
    def _parse_session_details(s: dict[str, Any]) -> SessionDetails:
        return SessionDetails(
            session_id=str(s.get("session_id", "")),
            session_ref=s.get("session_ref"),
            created_at=MeerkatClient._parse_int(s.get("created_at"), 0),
            updated_at=MeerkatClient._parse_int(s.get("updated_at"), 0),
            message_count=MeerkatClient._parse_int(s.get("message_count"), 0),
            labels=MeerkatClient._parse_string_map(s.get("labels")),
            is_active=bool(s.get("is_active", False)),
            model=str(s.get("model", "")),
            provider=str(s.get("provider", "")),
            last_assistant_text=(
                str(s["last_assistant_text"])
                if s.get("last_assistant_text") is not None
                else None
            ),
        )

    @staticmethod
    def _parse_models_catalog(data: dict[str, Any]) -> ModelsCatalogResponse:
        providers_raw = data.get("providers", [])
        providers: list[dict[str, Any]] = []
        if isinstance(providers_raw, list):
            for provider in providers_raw:
                if isinstance(provider, dict):
                    providers.append(provider)
        contract_version_raw = data.get("contract_version")
        if isinstance(contract_version_raw, dict):
            contract_version = {
                "major": MeerkatClient._parse_int(contract_version_raw.get("major"), 0),
                "minor": MeerkatClient._parse_int(contract_version_raw.get("minor"), 0),
                "patch": MeerkatClient._parse_int(contract_version_raw.get("patch"), 0),
            }
        elif isinstance(contract_version_raw, str):
            try:
                major, minor, patch = contract_version_raw.split(".", 2)
                contract_version = {
                    "major": MeerkatClient._parse_int(major, 0),
                    "minor": MeerkatClient._parse_int(minor, 0),
                    "patch": MeerkatClient._parse_int(patch, 0),
                }
            except ValueError:
                contract_version = {"major": 0, "minor": 0, "patch": 0}
        else:
            contract_version = {"major": 0, "minor": 0, "patch": 0}
        return {
            "contract_version": contract_version,
            "providers": providers,
        }
