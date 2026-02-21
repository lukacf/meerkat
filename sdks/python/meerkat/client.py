"""Meerkat client — spawns rkat-rpc subprocess and communicates via JSON-RPC."""

import asyncio
import os
import json
import platform
import shutil
import tarfile
import tempfile
from pathlib import Path
from urllib.error import URLError
from typing import Optional
import urllib.request
import zipfile

from .errors import CapabilityUnavailableError, MeerkatError
from .generated.types import (
    CONTRACT_VERSION,
    CapabilitiesResponse,
    CapabilityEntry,
    WireRunResult,
    WireUsage,
)
from .streaming import StreamingTurn, _StdoutDispatcher

_MEERKAT_REPO = ("lukacf", "meerkat")
_MEERKAT_BINARY = "rkat-rpc"
_MEERKAT_BINARY_CACHE_ROOT = Path.home() / ".cache" / "meerkat" / "bin" / _MEERKAT_BINARY


class MeerkatClient:
    """Async client that communicates with a Meerkat runtime via rkat-rpc.

    A background dispatcher multiplexes stdout so that JSON-RPC responses
    and streaming event notifications can be consumed concurrently.

    Usage::

        client = MeerkatClient()
        await client.connect()
        result = await client.create_session("Hello!")
        print(result.text)
        await client.close()

    Streaming::

        async with client.create_session_streaming("Hello!") as stream:
            async for event in stream:
                if event["type"] == "text_delta":
                    print(event["delta"], end="", flush=True)
            result = stream.result
    """

    def __init__(self, rkat_path: str = "rkat-rpc"):
        self._rkat_path = rkat_path
        self._legacy_rpc_subcommand = False
        self._process: Optional[asyncio.subprocess.Process] = None
        self._request_id = 0
        self._capabilities: Optional[CapabilitiesResponse] = None
        self._dispatcher: Optional[_StdoutDispatcher] = None

    async def connect(
        self,
        realm_id: Optional[str] = None,
        instance_id: Optional[str] = None,
        realm_backend: Optional[str] = None,
        isolated: bool = False,
        state_root: Optional[str] = None,
        context_root: Optional[str] = None,
        user_config_root: Optional[str] = None,
    ) -> "MeerkatClient":
        """Start the rkat-rpc subprocess and perform handshake."""
        if realm_id and isolated:
            raise MeerkatError(
                "INVALID_ARGS",
                "realm_id and isolated cannot both be set",
            )
        resolved_path, use_legacy_rpc = await self._resolve_rkat_binary(self._rkat_path)
        self._rkat_path = resolved_path
        self._legacy_rpc_subcommand = use_legacy_rpc

        legacy_requires_new_binary = any(
            [
                isolated,
                realm_id is not None,
                instance_id is not None,
                realm_backend is not None,
                state_root is not None,
                context_root is not None,
                user_config_root is not None,
            ]
        )
        if self._legacy_rpc_subcommand and legacy_requires_new_binary:
            raise MeerkatError(
                "LEGACY_BINARY_UNSUPPORTED",
                "Realm/context options require the standalone rkat-rpc binary. "
                "Install rkat-rpc and retry.",
            )
        args = []
        if self._legacy_rpc_subcommand:
            # Legacy fallback path supports only `rkat rpc` with defaults.
            args = ["rpc"]
        else:
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
        self._process = await asyncio.create_subprocess_exec(
            self._rkat_path, *args,
            stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )
        assert self._process.stdout is not None, "stdout pipe not available"
        self._dispatcher = _StdoutDispatcher(self._process.stdout)
        self._dispatcher.start()

        result = await self._request("initialize", {})
        server_version = result.get("contract_version", "")
        if not self._check_version_compatible(server_version, CONTRACT_VERSION):
            raise MeerkatError("VERSION_MISMATCH",
                f"Server version {server_version} incompatible with SDK {CONTRACT_VERSION}")

        caps_result = await self._request("capabilities/get", {})
        self._capabilities = CapabilitiesResponse(
            contract_version=caps_result.get("contract_version", ""),
            capabilities=[
                CapabilityEntry(id=c.get("id", ""), description=c.get("description", ""),
                    status=self._normalize_status(c.get("status", "Available")))
                for c in caps_result.get("capabilities", [])
            ],
        )
        return self

    async def close(self) -> None:
        """Terminate the rkat-rpc subprocess."""
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

    # --- Session lifecycle (non-streaming) ---

    async def create_session(self, prompt: str, model: Optional[str] = None,
            provider: Optional[str] = None, system_prompt: Optional[str] = None,
            max_tokens: Optional[int] = None, output_schema: Optional[dict] = None,
            structured_output_retries: int = 2, hooks_override: Optional[dict] = None,
            enable_builtins: bool = False, enable_shell: bool = False,
            enable_subagents: bool = False, enable_memory: bool = False,
            host_mode: bool = False, comms_name: Optional[str] = None,
            peer_meta: Optional[dict] = None,
            provider_params: Optional[dict] = None,
            preload_skills: Optional[list[str]] = None,
            skill_refs: Optional[list[dict]] = None,
            skill_references: Optional[list[str]] = None) -> WireRunResult:
        """Create a new session and run the first turn."""
        params = self._build_create_params(prompt, model, provider, system_prompt,
            max_tokens, output_schema, structured_output_retries, hooks_override,
            enable_builtins, enable_shell, enable_subagents, enable_memory,
            host_mode, comms_name, peer_meta, provider_params, preload_skills, skill_refs, skill_references)
        result = await self._request("session/create", params)
        return self._parse_run_result(result)

    async def start_turn(
        self,
        session_id: str,
        prompt: str,
        skill_refs: Optional[list[dict]] = None,
        skill_references: Optional[list[str]] = None,
    ) -> WireRunResult:
        """Start a new turn on an existing session."""
        params: dict = {"session_id": session_id, "prompt": prompt}
        if skill_refs is not None:
            params["skill_refs"] = skill_refs
        if skill_references is not None:
            params["skill_references"] = skill_references
        result = await self._request("turn/start", params)
        return self._parse_run_result(result)

    # --- Session lifecycle (streaming) ---

    def create_session_streaming(self, prompt: str, model: Optional[str] = None,
            provider: Optional[str] = None, system_prompt: Optional[str] = None,
            max_tokens: Optional[int] = None, output_schema: Optional[dict] = None,
            structured_output_retries: int = 2, hooks_override: Optional[dict] = None,
            enable_builtins: bool = False, enable_shell: bool = False,
            enable_subagents: bool = False, enable_memory: bool = False,
            host_mode: bool = False, comms_name: Optional[str] = None,
            peer_meta: Optional[dict] = None,
            provider_params: Optional[dict] = None,
            preload_skills: Optional[list[str]] = None,
            skill_refs: Optional[list[dict]] = None,
            skill_references: Optional[list[str]] = None) -> StreamingTurn:
        """Create a new session and stream events from the first turn.

        Returns a StreamingTurn async context manager. The request is sent
        when entering the context. Events are raw dicts matching the Rust
        ``AgentEvent`` serde-tagged enum.

        Usage::

            async with client.create_session_streaming("Hello!") as stream:
                async for event in stream:
                    if event["type"] == "text_delta":
                        print(event["delta"], end="", flush=True)
                result = stream.result
        """
        if not self._dispatcher or not self._process or not self._process.stdin:
            raise MeerkatError("NOT_CONNECTED", "Client not connected")
        params = self._build_create_params(prompt, model, provider, system_prompt,
            max_tokens, output_schema, structured_output_retries, hooks_override,
            enable_builtins, enable_shell, enable_subagents, enable_memory,
            host_mode, comms_name, peer_meta, provider_params, preload_skills, skill_refs, skill_references)
        self._request_id += 1
        request_id = self._request_id
        event_queue = self._dispatcher.subscribe_pending_stream(request_id)
        response_future = self._dispatcher.expect_response(request_id)
        request = {"jsonrpc": "2.0", "id": request_id, "method": "session/create", "params": params}
        data = (json.dumps(request) + "\n").encode()
        return StreamingTurn(session_id="", event_queue=event_queue,
            response_future=response_future, dispatcher=self._dispatcher,
            parse_result=self._parse_run_result,
            pending_send=(self._process.stdin, data))

    def start_turn_streaming(self, session_id: str, prompt: str,
            skill_refs: Optional[list[dict]] = None,
            skill_references: Optional[list[str]] = None) -> StreamingTurn:
        """Start a new turn on an existing session and stream events.

        Usage::

            async with client.start_turn_streaming(session_id, "Follow up") as stream:
                async for event in stream:
                    print(event)
                result = stream.result
        """
        if not self._dispatcher or not self._process or not self._process.stdin:
            raise MeerkatError("NOT_CONNECTED", "Client not connected")
        self._request_id += 1
        request_id = self._request_id
        event_queue = self._dispatcher.subscribe_events(session_id)
        response_future = self._dispatcher.expect_response(request_id)
        params: dict = {"session_id": session_id, "prompt": prompt}
        if skill_refs is not None:
            params["skill_refs"] = skill_refs
        if skill_references is not None:
            params["skill_references"] = skill_references
        request = {"jsonrpc": "2.0", "id": request_id, "method": "turn/start",
                   "params": params}
        data = (json.dumps(request) + "\n").encode()
        return StreamingTurn(session_id=session_id, event_queue=event_queue,
            response_future=response_future, dispatcher=self._dispatcher,
            parse_result=self._parse_run_result,
            pending_send=(self._process.stdin, data))

    # --- Other operations ---

    async def interrupt(self, session_id: str) -> None:
        """Interrupt a running turn."""
        await self._request("turn/interrupt", {"session_id": session_id})

    async def list_sessions(self) -> list:
        """List active sessions."""
        result = await self._request("session/list", {})
        return result.get("sessions", [])

    async def read_session(self, session_id: str) -> dict:
        """Read session state."""
        return await self._request("session/read", {"session_id": session_id})

    async def archive_session(self, session_id: str) -> None:
        """Archive (remove) a session."""
        await self._request("session/archive", {"session_id": session_id})

    async def get_capabilities(self) -> CapabilitiesResponse:
        """Get runtime capabilities."""
        if self._capabilities:
            return self._capabilities
        result = await self._request("capabilities/get", {})
        return CapabilitiesResponse(
            contract_version=result.get("contract_version", ""),
            capabilities=[],
        )

    def has_capability(self, capability_id: str) -> bool:
        """Check if a capability is available."""
        if not self._capabilities:
            return False
        return any(
            c.id == capability_id and c.status == "Available"
            for c in self._capabilities.capabilities
        )

    def require_capability(self, capability_id: str) -> None:
        """Raise CapabilityUnavailableError if capability is not available."""
        if not self.has_capability(capability_id):
            raise CapabilityUnavailableError(
                "CAPABILITY_UNAVAILABLE",
                f"Capability '{capability_id}' is not available",
            )

    # --- Config ---

    async def get_config(self) -> dict:
        """Get runtime configuration."""
        return await self._request("config/get", {})

    async def set_config(self, config: dict) -> None:
        """Replace runtime configuration."""
        await self._request("config/set", config)

    async def patch_config(self, patch: dict) -> dict:
        """Merge-patch runtime configuration. Returns updated config."""
        return await self._request("config/patch", patch)

    async def send(self, session_id: str, **kwargs) -> dict:
        """Send a canonical comms command to a session.

        Args:
            session_id: Target session ID.
            **kwargs: Command fields (kind, to, body, intent, params, etc.)

        Returns:
            dict with receipt info on success.
        """
        params = {"session_id": session_id, **kwargs}
        return await self._request("comms/send", params)

    async def peers(self, session_id: str) -> dict:
        """List peers visible to a session's comms runtime.

        Args:
            session_id: Target session ID.

        Returns:
            dict with {"peers": [...]} on success.
        """
        return await self._request("comms/peers", {"session_id": session_id})

    # --- Internal ---
    async def _resolve_rkat_binary(self, requested_path: str) -> tuple[str, bool]:
        override = os.environ.get("MEERKAT_BIN_PATH")
        if override:
            override = override.strip()
            if not override:
                raise MeerkatError(
                    "BINARY_NOT_FOUND",
                    "MEERKAT_BIN_PATH is set to an empty path.",
                )
            candidate = self._resolve_candidate_path(override)
            if candidate is None:
                raise MeerkatError(
                    "BINARY_NOT_FOUND",
                    f"Binary not found at MEERKAT_BIN_PATH='{override}'. "
                    "Set MEERKAT_BIN_PATH to a valid executable.",
                )
            return str(candidate), Path(candidate).name == "rkat"

        if requested_path != _MEERKAT_BINARY:
            candidate = self._resolve_candidate_path(requested_path)
            if candidate is None:
                raise MeerkatError(
                    "BINARY_NOT_FOUND",
                    f"Binary not found at '{requested_path}'. "
                    "Set rkat_path to a valid path or use MEERKAT_BIN_PATH.",
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
    def _resolve_candidate_path(command_or_path: str) -> Optional[str]:
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
                raise MeerkatError("UNSUPPORTED_PLATFORM", f"Unsupported macOS architecture '{machine}'")
            return target, "tar.gz", _MEERKAT_BINARY

        if system == "linux":
            if machine in {"x86_64", "amd64"}:
                return "x86_64-unknown-linux-gnu", "tar.gz", _MEERKAT_BINARY
            if machine in {"aarch64", "arm64"}:
                return "aarch64-unknown-linux-gnu", "tar.gz", _MEERKAT_BINARY
            raise MeerkatError(
                "UNSUPPORTED_PLATFORM",
                f"Unsupported Linux architecture '{machine}'.",
            )

        if system == "windows":
            if machine in {"x86_64", "amd64"}:
                return "x86_64-pc-windows-msvc", "zip", f"{_MEERKAT_BINARY}.exe"
            raise MeerkatError(
                "UNSUPPORTED_PLATFORM",
                f"Unsupported Windows architecture '{machine}'.",
            )

        raise MeerkatError("UNSUPPORTED_PLATFORM", f"Unsupported platform '{system}'.")

    @staticmethod
    async def _download_rkat_rpc_binary() -> Optional[str]:
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
                    members = [m for m in archive.getmembers() if Path(m.name).name == binary_name]
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
                    if not any(Path(name).name == binary_name for name in candidate_names):
                        raise MeerkatError(
                            "BINARY_DOWNLOAD_FAILED",
                            f"Downloaded archive does not contain '{binary_name}'.",
                        )
                    with archive.open(
                        next(name for name in candidate_names if Path(name).name == binary_name),
                    ) as source, temp_binary.open("wb") as out:
                        out.write(source.read())

            if os.name != "nt":
                temp_binary.chmod(0o755)
            temp_binary.replace(cached)
            return str(cached)

    async def _request(self, method: str, params: dict) -> dict:
        """Send a JSON-RPC request and return the result.

        Uses the background dispatcher to wait for the matching response.
        Any notifications that arrive during the wait are routed to their
        respective event queues (or silently dropped if no subscriber).
        """
        if not self._process or not self._process.stdin or not self._dispatcher:
            raise MeerkatError("NOT_CONNECTED", "Client not connected")
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

    @staticmethod
    def _build_create_params(
        prompt: str,
        model: Optional[str],
        provider: Optional[str],
        system_prompt: Optional[str],
        max_tokens: Optional[int],
        output_schema: Optional[dict],
        structured_output_retries: int,
        hooks_override: Optional[dict],
        enable_builtins: bool,
        enable_shell: bool,
        enable_subagents: bool,
        enable_memory: bool,
        host_mode: bool,
        comms_name: Optional[str],
        peer_meta: Optional[dict],
        provider_params: Optional[dict],
        preload_skills: Optional[list[str]] = None,
        skill_refs: Optional[list[dict]] = None,
        skill_references: Optional[list[str]] = None,
    ) -> dict:
        """Build the params dict for session/create."""
        params: dict = {"prompt": prompt}
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
        if skill_refs is not None:
            params["skill_refs"] = skill_refs
        if skill_references is not None:
            params["skill_references"] = skill_references
        return params

    @staticmethod
    def _normalize_status(raw) -> str:
        """Normalize a CapabilityStatus from the wire.

        On the wire, Available is the string ``"Available"`` but other variants
        are externally-tagged objects like ``{"DisabledByPolicy": {"description": "..."}}``.
        We normalize to the variant name string for simple comparison.
        """
        if isinstance(raw, str):
            return raw
        if isinstance(raw, dict):
            return next(iter(raw), "Unknown")
        return str(raw)

    @staticmethod
    def _check_version_compatible(server: str, client: str) -> bool:
        """Check version compatibility (same major for 1.0+, same minor for 0.x)."""
        try:
            s_parts = [int(x) for x in server.split(".")]
            c_parts = [int(x) for x in client.split(".")]
            if s_parts[0] == 0 and c_parts[0] == 0:
                return s_parts[1] == c_parts[1]
            return s_parts[0] == c_parts[0]
        except (ValueError, IndexError):
            return False

    @staticmethod
    def _parse_run_result(data: dict) -> WireRunResult:
        """Parse a run result from JSON-RPC response."""
        usage_data = data.get("usage", {})
        usage = WireUsage(
            input_tokens=usage_data.get("input_tokens", 0),
            output_tokens=usage_data.get("output_tokens", 0),
            total_tokens=usage_data.get("total_tokens", 0),
            cache_creation_tokens=usage_data.get("cache_creation_tokens"),
            cache_read_tokens=usage_data.get("cache_read_tokens"),
        )
        return WireRunResult(
            session_id=data.get("session_id", ""),
            session_ref=data.get("session_ref"),
            text=data.get("text", ""),
            turns=data.get("turns", 0),
            tool_calls=data.get("tool_calls", 0),
            usage=usage,
            structured_output=data.get("structured_output"),
            schema_warnings=data.get("schema_warnings"),
        )
