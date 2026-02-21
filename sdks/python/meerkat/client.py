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
from typing import Any
from urllib.error import URLError
import urllib.request

from .errors import CapabilityUnavailableError, MeerkatError
from .events import Usage
from .generated.types import CONTRACT_VERSION
from .session import Session, _normalize_skill_ref
from .streaming import EventStream, _StdoutDispatcher
from .types import Capability, RunResult, SchemaWarning, SessionInfo, SkillKey, SkillRef

_MEERKAT_REPO = ("lukacf", "meerkat")
_MEERKAT_BINARY = "rkat-rpc"
_MEERKAT_BINARY_CACHE_ROOT = Path.home() / ".cache" / "meerkat" / "bin" / _MEERKAT_BINARY


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
        self._dispatcher: _StdoutDispatcher | None = None

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
        self._dispatcher.start()

        result = await self._request("initialize", {})
        server_version = result.get("contract_version", "")
        if not self._check_version_compatible(server_version, CONTRACT_VERSION):
            raise MeerkatError(
                "VERSION_MISMATCH",
                f"Server version {server_version} incompatible with SDK {CONTRACT_VERSION}",
            )

        caps_result = await self._request("capabilities/get", {})
        self._capabilities = [
            Capability(
                id=c.get("id", ""),
                description=c.get("description", ""),
                status=self._normalize_status(c.get("status", "Available")),
            )
            for c in caps_result.get("capabilities", [])
        ]
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
        prompt: str,
        *,
        model: str | None = None,
        provider: str | None = None,
        system_prompt: str | None = None,
        max_tokens: int | None = None,
        output_schema: dict[str, Any] | None = None,
        structured_output_retries: int = 2,
        hooks_override: dict[str, Any] | None = None,
        enable_builtins: bool = False,
        enable_shell: bool = False,
        enable_subagents: bool = False,
        enable_memory: bool = False,
        host_mode: bool = False,
        comms_name: str | None = None,
        peer_meta: dict[str, Any] | None = None,
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
            enable_shell=enable_shell, enable_subagents=enable_subagents,
            enable_memory=enable_memory, host_mode=host_mode,
            comms_name=comms_name, peer_meta=peer_meta,
            provider_params=provider_params, preload_skills=preload_skills,
            skill_refs=skill_refs, skill_references=skill_references,
        )
        raw = await self._request("session/create", params)
        result = self._parse_run_result(raw)
        return Session(self, result)

    def create_session_streaming(
        self,
        prompt: str,
        *,
        model: str | None = None,
        provider: str | None = None,
        system_prompt: str | None = None,
        max_tokens: int | None = None,
        output_schema: dict[str, Any] | None = None,
        structured_output_retries: int = 2,
        hooks_override: dict[str, Any] | None = None,
        enable_builtins: bool = False,
        enable_shell: bool = False,
        enable_subagents: bool = False,
        enable_memory: bool = False,
        host_mode: bool = False,
        comms_name: str | None = None,
        peer_meta: dict[str, Any] | None = None,
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
            enable_shell=enable_shell, enable_subagents=enable_subagents,
            enable_memory=enable_memory, host_mode=host_mode,
            comms_name=comms_name, peer_meta=peer_meta,
            provider_params=provider_params, preload_skills=preload_skills,
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

    # -- Capabilities ------------------------------------------------------

    @property
    def capabilities(self) -> list[Capability]:
        """Runtime capabilities discovered during handshake."""
        return self._capabilities or []

    def has_capability(self, capability_id: str) -> bool:
        """Check if a capability is available."""
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

    async def set_config(self, config: dict[str, Any]) -> None:
        await self._request("config/set", config)

    async def patch_config(self, patch: dict[str, Any]) -> dict[str, Any]:
        return await self._request("config/patch", patch)

    # -- Internal methods used by Session ----------------------------------

    async def _start_turn(
        self,
        session_id: str,
        prompt: str,
        *,
        skill_refs: list[SkillRef] | None = None,
        skill_references: list[str] | None = None,
    ) -> RunResult:
        params: dict[str, Any] = {"session_id": session_id, "prompt": prompt}
        wire_refs = _skill_refs_to_wire(skill_refs)
        if wire_refs is not None:
            params["skill_refs"] = wire_refs
        if skill_references is not None:
            params["skill_references"] = skill_references
        raw = await self._request("turn/start", params)
        return self._parse_run_result(raw)

    def _start_turn_streaming(
        self,
        session_id: str,
        prompt: str,
        *,
        skill_refs: list[SkillRef] | None = None,
        skill_references: list[str] | None = None,
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
        params: dict[str, Any] = {"session_id": session_id, **kwargs}
        return await self._request("comms/send", params)

    async def _peers(self, session_id: str) -> dict[str, Any]:
        return await self._request("comms/peers", {"session_id": session_id})

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
        prompt: str,
        *,
        model: str | None = None,
        provider: str | None = None,
        system_prompt: str | None = None,
        max_tokens: int | None = None,
        output_schema: dict[str, Any] | None = None,
        structured_output_retries: int = 2,
        hooks_override: dict[str, Any] | None = None,
        enable_builtins: bool = False,
        enable_shell: bool = False,
        enable_subagents: bool = False,
        enable_memory: bool = False,
        host_mode: bool = False,
        comms_name: str | None = None,
        peer_meta: dict[str, Any] | None = None,
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
        if output_schema:
            params["output_schema"] = output_schema
        if structured_output_retries != 2:
            params["structured_output_retries"] = structured_output_retries
        if hooks_override:
            params["hooks_override"] = hooks_override
        if enable_builtins:
            params["enable_builtins"] = True
        if enable_shell:
            params["enable_shell"] = True
        if enable_subagents:
            params["enable_subagents"] = True
        if enable_memory:
            params["enable_memory"] = True
        if host_mode:
            params["host_mode"] = True
        if comms_name:
            params["comms_name"] = comms_name
        if peer_meta is not None:
            params["peer_meta"] = peer_meta
        if provider_params:
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
            return next(iter(raw), "Unknown")
        return str(raw)

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
            skill_diagnostics=data.get("skill_diagnostics"),
        )
