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
import json
import os
import platform
import shutil
import tarfile
import tempfile
import zipfile
from pathlib import Path
from typing import Any, Literal, NotRequired, TypedDict
from urllib.error import URLError
import urllib.request

from .errors import CapabilityUnavailableError, MeerkatError
from .events import Usage, parse_event
from .generated.types import CONTRACT_VERSION
from .generated.types import (
    RuntimeAcceptResult,
    RuntimeResetResult,
    RuntimeRetireResult,
    RuntimeStateResult,
    WireInputState,
    WireInputStateHistoryEntry,
)
from .mob import Mob, MobHelperResult, MobMemberSnapshot
from .session import DeferredSession, Session, _normalize_skill_ref
from .streaming import EventStream, EventSubscription, _StdoutDispatcher
from .tools import ToolRegistry
from .types import (
    AttributedEvent,
    Capability,
    EventEnvelope,
    McpLiveOpResponse,
    RunResult,
    SchemaWarning,
    SessionAssistantBlock,
    SessionHistory,
    SessionInfo,
    SessionMessage,
    SessionToolCall,
    SessionToolResult,
    SkillKey,
    SkillQuarantineDiagnostic,
    SkillRef,
    SkillRuntimeDiagnostics,
    SourceHealthSnapshot,
)

_MEERKAT_REPO = ("lukacf", "meerkat")
_MEERKAT_BINARY = "rkat-rpc"
_MEERKAT_BINARY_CACHE_ROOT = Path.home() / ".cache" / "meerkat" / "bin" / _MEERKAT_BINARY

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


def _skill_refs_to_wire(refs: list[SkillRef] | None) -> list[dict[str, str]] | None:
    """Convert a list of SkillRef to the wire format for JSON-RPC."""
    if refs is None:
        return None
    return [
        {
            "source_uuid": _normalize_skill_ref(r).source_uuid,
            "skill_name": _normalize_skill_ref(r).skill_name,
        }
        for r in refs
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

    # -- Session lifecycle -------------------------------------------------

    async def create_session(
        self,
        prompt: str | list[dict],
        *,
        model: str | None = None,
        provider: str | None = None,
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
        preload_skills: list[str] | None = None,
        skill_refs: list[SkillRef] | None = None,
        skill_references: list[str] | None = None,
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
            prompt, model=model, provider=provider, system_prompt=system_prompt,
            max_tokens=max_tokens, output_schema=output_schema,
            structured_output_retries=structured_output_retries,
            hooks_override=hooks_override, enable_builtins=enable_builtins,
            enable_shell=enable_shell,
            enable_memory=enable_memory, enable_mob=enable_mob,
            keep_alive=keep_alive,
            comms_name=comms_name, peer_meta=peer_meta,
            budget_limits=budget_limits, provider_params=provider_params,
            preload_skills=preload_skills,
            skill_refs=skill_refs, skill_references=skill_references,
        )
        raw = await self._request("session/create", params)
        result = self._parse_run_result(raw)
        return Session(self, result)

    def create_session_streaming(
        self,
        prompt: str | list[dict],
        *,
        model: str | None = None,
        provider: str | None = None,
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
        preload_skills: list[str] | None = None,
        skill_refs: list[SkillRef] | None = None,
        skill_references: list[str] | None = None,
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
            prompt, model=model, provider=provider, system_prompt=system_prompt,
            max_tokens=max_tokens, output_schema=output_schema,
            structured_output_retries=structured_output_retries,
            hooks_override=hooks_override, enable_builtins=enable_builtins,
            enable_shell=enable_shell,
            enable_memory=enable_memory, enable_mob=enable_mob,
            keep_alive=keep_alive,
            comms_name=comms_name, peer_meta=peer_meta,
            budget_limits=budget_limits, provider_params=provider_params,
            preload_skills=preload_skills,
            skill_refs=skill_refs, skill_references=skill_references,
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
        prompt: str | list[dict],
        *,
        model: str | None = None,
        provider: str | None = None,
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
        preload_skills: list[str] | None = None,
        skill_refs: list[SkillRef] | None = None,
        skill_references: list[str] | None = None,
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
            prompt, model=model, provider=provider, system_prompt=system_prompt,
            max_tokens=max_tokens, output_schema=output_schema,
            structured_output_retries=structured_output_retries,
            hooks_override=hooks_override, enable_builtins=enable_builtins,
            enable_shell=enable_shell,
            enable_memory=enable_memory, enable_mob=enable_mob,
            keep_alive=keep_alive,
            comms_name=comms_name, peer_meta=peer_meta,
            budget_limits=budget_limits, provider_params=provider_params,
            preload_skills=preload_skills,
            skill_refs=skill_refs, skill_references=skill_references,
        )
        params["initial_turn"] = "deferred"
        raw = await self._request("session/create", params)
        return DeferredSession(
            self,
            session_id=raw.get("session_id", ""),
            session_ref=raw.get("session_ref"),
        )

    # -- Session queries ---------------------------------------------------

    async def list_sessions(self) -> list[SessionInfo]:
        """List active sessions."""
        result = await self._request("session/list", {})
        return [
            SessionInfo(
                session_id=s.get("session_id", ""),
                session_ref=s.get("session_ref"),
                created_at=s.get("created_at", ""),
                updated_at=s.get("updated_at", ""),
                message_count=s.get("message_count", 0),
                total_tokens=s.get("total_tokens", 0),
                is_active=s.get("is_active", False),
            )
            for s in result.get("sessions", [])
        ]

    async def read_session(self, session_id: str) -> dict[str, Any]:
        """Read detailed session state."""
        return await self._request("session/read", {"session_id": session_id})

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

    # -- Capabilities ------------------------------------------------------

    @property
    def capabilities(self) -> list[Capability]:
        """Runtime capabilities discovered during handshake."""
        return self._capabilities or []

    def has_capability(self, capability_id: str) -> bool:
        """Check if a capability is available."""
        if capability_id == "mob":
            return bool(
                {"mob/create", "mob/list", "mob/call"} & self._methods
            )
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

    async def get_config(self) -> dict[str, Any]:
        return await self._request("config/get", {})

    async def set_config(
        self,
        config: dict[str, Any],
        *,
        expected_generation: int | None = None,
    ) -> dict[str, Any]:
        params: dict[str, Any] = {"config": config}
        if expected_generation is not None:
            params["expected_generation"] = expected_generation
        return await self._request("config/set", params)

    async def patch_config(
        self,
        patch: dict[str, Any],
        *,
        expected_generation: int | None = None,
    ) -> dict[str, Any]:
        params: dict[str, Any] = {"patch": patch}
        if expected_generation is not None:
            params["expected_generation"] = expected_generation
        return await self._request("config/patch", params)

    async def mcp_add(
        self,
        session_id: str,
        server_name: str,
        server_config: dict[str, Any],
        *,
        persisted: bool = False,
    ) -> McpLiveOpResponse:
        raw = await self._request(
            "mcp/add",
            {
                "session_id": session_id,
                "server_name": server_name,
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

    async def inspect_skill(
        self,
        skill_id: str,
        *,
        source: str | None = None,
    ) -> dict[str, Any]:
        params: dict[str, Any] = {"id": skill_id}
        if source is not None:
            params["source"] = source
        return await self._request("skills/inspect", params)

    async def list_mob_prefabs(self) -> list[dict[str, Any]]:
        result = await self._request("mob/prefabs", {})
        return result.get("prefabs", [])

    async def list_mob_tools(self) -> list[dict[str, Any]]:
        result = await self._request("mob/tools", {})
        return result.get("tools", [])

    async def call_mob_tool(
        self,
        name: str,
        arguments: dict[str, Any] | None = None,
    ) -> dict[str, Any]:
        return await self._request(
            "mob/call",
            {
                "name": name,
                "arguments": arguments or {},
            },
        )

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
        prefab: str | None = None,
        definition: dict[str, Any] | None = None,
    ) -> Mob:
        self.require_capability("mob")
        result = await self._request("mob/create", {"prefab": prefab, "definition": definition})
        return Mob(self, str(result.get("mob_id", "")))

    def mob(self, mob_id: str) -> Mob:
        return Mob(self, mob_id)

    async def list_mobs(self) -> list[dict[str, Any]]:
        self.require_capability("mob")
        result = await self._request("mob/list", {})
        return result.get("mobs", [])

    async def mob_status(self, mob_id: str) -> dict[str, Any]:
        return await self._request("mob/status", {"mob_id": mob_id})

    async def list_mob_members(self, mob_id: str) -> list[dict[str, Any]]:
        result = await self._request("mob/members", {"mob_id": mob_id})
        return result.get("members", [])

    async def spawn_mob_member(
        self,
        mob_id: str,
        *,
        profile: str,
        meerkat_id: str,
        initial_message: str | list[dict] | None = None,
        runtime_mode: str | None = None,
        backend: str | None = None,
        resume_session_id: str | None = None,
        labels: dict[str, str] | None = None,
        context: dict[str, Any] | None = None,
        additional_instructions: list[str] | None = None,
    ) -> dict[str, Any]:
        return await self._request("mob/spawn", {
            "mob_id": mob_id,
            "profile": profile,
            "meerkat_id": meerkat_id,
            "initial_message": initial_message,
            "runtime_mode": runtime_mode,
            "backend": backend,
            "resume_session_id": resume_session_id,
            "labels": labels,
            "context": context,
            "additional_instructions": additional_instructions,
        })

    async def retire_mob_member(self, mob_id: str, meerkat_id: str) -> None:
        await self._request("mob/retire", {"mob_id": mob_id, "meerkat_id": meerkat_id})

    async def respawn_mob_member(
        self,
        mob_id: str,
        meerkat_id: str,
        initial_message: str | list[dict] | None = None,
    ) -> dict[str, Any]:
        return await self._request(
            "mob/respawn",
            {"mob_id": mob_id, "meerkat_id": meerkat_id, "initial_message": initial_message},
        )

    async def force_cancel_mob_member(self, mob_id: str, meerkat_id: str) -> None:
        await self._request("mob/force_cancel", {"mob_id": mob_id, "meerkat_id": meerkat_id})

    async def mob_member_status(self, mob_id: str, meerkat_id: str) -> MobMemberSnapshot:
        return await self._request("mob/member_status", {"mob_id": mob_id, "meerkat_id": meerkat_id})

    async def spawn_mob_helper(
        self,
        mob_id: str,
        prompt: str,
        *,
        meerkat_id: str | None = None,
        profile_name: str | None = None,
        runtime_mode: str | None = None,
        backend: str | None = None,
    ) -> MobHelperResult:
        return await self._request("mob/spawn_helper", {
            "mob_id": mob_id,
            "prompt": prompt,
            "meerkat_id": meerkat_id,
            "profile_name": profile_name,
            "runtime_mode": runtime_mode,
            "backend": backend,
        })

    async def fork_mob_helper(
        self,
        mob_id: str,
        source_member_id: str,
        prompt: str,
        *,
        meerkat_id: str | None = None,
        profile_name: str | None = None,
        fork_context: dict[str, Any] | None = None,
        runtime_mode: str | None = None,
        backend: str | None = None,
    ) -> MobHelperResult:
        return await self._request("mob/fork_helper", {
            "mob_id": mob_id,
            "source_member_id": source_member_id,
            "prompt": prompt,
            "meerkat_id": meerkat_id,
            "profile_name": profile_name,
            "fork_context": fork_context,
            "runtime_mode": runtime_mode,
            "backend": backend,
        })

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

    async def mob_lifecycle(self, mob_id: str, action: str) -> None:
        await self._request("mob/lifecycle", {"mob_id": mob_id, "action": action})

    async def send_mob_member_content(
        self,
        mob_id: str,
        meerkat_id: str,
        content: str | list[dict[str, Any]],
        *,
        handling_mode: Literal["queue", "steer"] = "queue",
        render_metadata: RenderMetadata | None = None,
    ) -> dict[str, Any]:
        result = await self._request(
            "mob/send",
            {
                "mob_id": mob_id,
                "meerkat_id": meerkat_id,
                "content": content,
                "handling_mode": handling_mode,
                "render_metadata": render_metadata,
            },
        )
        session_id = result.get("session_id")
        if not isinstance(session_id, str) or not session_id:
            raise MeerkatError("INVALID_RESPONSE", "Invalid mob/send response: missing session_id")
        return result

    async def append_mob_system_context(
        self,
        mob_id: str,
        meerkat_id: str,
        text: str,
        *,
        source: str | None = None,
        idempotency_key: str | None = None,
    ) -> dict[str, Any]:
        return await self._request("mob/append_system_context", {
            "mob_id": mob_id,
            "meerkat_id": meerkat_id,
            "text": text,
            "source": source,
            "idempotency_key": idempotency_key,
        })

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

    async def subscribe_mob_member_events(self, mob_id: str, meerkat_id: str) -> EventSubscription:
        return await self._open_event_subscription(
            "mob/stream_open",
            {"mob_id": mob_id, "member_id": meerkat_id},
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
            source_id=str(raw.get("source_id", "")),
            seq=int(raw.get("seq", 0)),
            timestamp_ms=int(raw.get("timestamp_ms", 0)),
            payload=parse_event(payload if isinstance(payload, dict) else {}),
        )

    @staticmethod
    def _parse_attributed_mob_event(raw: dict[str, Any]) -> AttributedEvent:
        envelope = raw.get("envelope", {})
        return AttributedEvent(
            source=str(raw.get("source", "")),
            profile=str(raw.get("profile", "")),
            envelope=MeerkatClient._parse_agent_event_envelope(
                envelope if isinstance(envelope, dict) else {}
            ),
        )

    # -- Internal methods used by Session ----------------------------------

    async def _start_turn(
        self,
        session_id: str,
        prompt: str | list[dict],
        *,
        skill_refs: list[SkillRef] | None = None,
        skill_references: list[str] | None = None,
        flow_tool_overlay: dict[str, Any] | None = None,
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
        if skill_references is not None:
            params["skill_references"] = skill_references
        if flow_tool_overlay is not None:
            params["flow_tool_overlay"] = flow_tool_overlay
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
        prompt: str | list[dict],
        *,
        skill_refs: list[SkillRef] | None = None,
        skill_references: list[str] | None = None,
        flow_tool_overlay: dict[str, Any] | None = None,
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
        if skill_references is not None:
            params["skill_references"] = skill_references
        if flow_tool_overlay is not None:
            params["flow_tool_overlay"] = flow_tool_overlay
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

    async def send(self, session_id: str, **kwargs: Any) -> dict[str, Any]:
        params: dict[str, Any] = {"session_id": session_id, **kwargs}
        return await self._request("comms/send", params)

    async def peers(self, session_id: str) -> dict[str, Any]:
        return await self._request("comms/peers", {"session_id": session_id})

    async def runtime_state(self, session_id: str) -> RuntimeStateResult:
        raw = await self._request("runtime/state", {"session_id": session_id})
        return RuntimeStateResult(**raw)

    async def runtime_accept(
        self,
        session_id: str,
        input: dict[str, Any],
    ) -> RuntimeAcceptResult:
        raw = await self._request("runtime/accept", {"session_id": session_id, "input": input})
        state = raw.get("state")
        if isinstance(state, dict):
            raw = dict(raw)
            raw["state"] = self._parse_wire_input_state(state)
        return RuntimeAcceptResult(**raw)

    async def runtime_retire(self, session_id: str) -> RuntimeRetireResult:
        raw = await self._request("runtime/retire", {"session_id": session_id})
        return RuntimeRetireResult(**raw)

    async def runtime_reset(self, session_id: str) -> RuntimeResetResult:
        raw = await self._request("runtime/reset", {"session_id": session_id})
        return RuntimeResetResult(**raw)

    async def input_state(self, session_id: str, input_id: str) -> WireInputState | None:
        raw = await self._request("input/state", {"session_id": session_id, "input_id": input_id})
        if raw is None:
            return None
        return self._parse_wire_input_state(raw)

    async def input_list(self, session_id: str) -> list[str]:
        result = await self._request("input/list", {"session_id": session_id})
        input_ids = result.get("input_ids", [])
        return [str(input_id) for input_id in input_ids]

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

    @staticmethod
    def _parse_wire_input_state(raw: dict[str, Any]) -> WireInputState:
        history_raw = raw.get("history", [])
        history: list[WireInputStateHistoryEntry] = []
        if isinstance(history_raw, list):
            for entry in history_raw:
                if not isinstance(entry, dict):
                    continue
                history.append(
                    WireInputStateHistoryEntry(
                        from_=str(entry.get("from", "")),
                        reason=(
                            str(entry["reason"])
                            if entry.get("reason") is not None
                            else None
                        ),
                        timestamp=str(entry.get("timestamp", "")),
                        to=str(entry.get("to", "")),
                    )
                )

        payload = dict(raw)
        payload["history"] = history
        return WireInputState(**payload)

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
        args: list[str] = []
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
        prompt: str | list[dict],
        *,
        model: str | None = None,
        provider: str | None = None,
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
        preload_skills: list[str] | None = None,
        skill_refs: list[SkillRef] | None = None,
        skill_references: list[str] | None = None,
    ) -> dict[str, Any]:
        params: dict[str, Any] = {"prompt": prompt}
        if model:
            params["model"] = model
        if provider:
            params["provider"] = provider
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
            params["preload_skills"] = preload_skills
        wire_refs = _skill_refs_to_wire(skill_refs)
        if wire_refs is not None:
            params["skill_refs"] = wire_refs
        if skill_references is not None:
            params["skill_references"] = skill_references
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
        usage_data = data.get("usage", {})
        usage = Usage(
            input_tokens=usage_data.get("input_tokens", 0),
            output_tokens=usage_data.get("output_tokens", 0),
            cache_creation_tokens=usage_data.get("cache_creation_tokens"),
            cache_read_tokens=usage_data.get("cache_read_tokens"),
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
            turns=data.get("turns", 0),
            tool_calls=data.get("tool_calls", 0),
            usage=usage,
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
            content=data.get("content"),
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
                    content=result.get("content", ""),
                    is_error=bool(result.get("is_error", False)),
                )
                for result in data.get("results", [])
            ],
        )

    @staticmethod
    def _parse_session_assistant_block(data: dict[str, Any]) -> SessionAssistantBlock:
        block_data = data.get("data", {})
        return SessionAssistantBlock(
            block_type=data.get("block_type", ""),
            text=block_data.get("text"),
            id=block_data.get("id"),
            name=block_data.get("name"),
            args=block_data.get("args"),
            meta=block_data.get("meta"),
        )
