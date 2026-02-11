"""Meerkat client — spawns rkat rpc subprocess and communicates via JSON-RPC."""

import asyncio
import json
from typing import Optional

from .errors import CapabilityUnavailableError, MeerkatError
from .generated.types import (
    CONTRACT_VERSION,
    CapabilitiesResponse,
    CapabilityEntry,
    WireRunResult,
    WireUsage,
)
from .streaming import StreamingTurn, _StdoutDispatcher


class MeerkatClient:
    """Async client that communicates with a Meerkat runtime via rkat rpc.

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

    def __init__(self, rkat_path: str = "rkat"):
        import shutil

        self._rkat_path = rkat_path
        self._process: Optional[asyncio.subprocess.Process] = None
        self._request_id = 0
        self._capabilities: Optional[CapabilitiesResponse] = None
        self._dispatcher: Optional[_StdoutDispatcher] = None

        if not shutil.which(self._rkat_path):
            raise MeerkatError(
                "BINARY_NOT_FOUND",
                f"rkat binary not found at '{self._rkat_path}'. "
                "Ensure it is installed and on your PATH.",
            )

    async def connect(self) -> "MeerkatClient":
        """Start the rkat rpc subprocess and perform handshake."""
        self._process = await asyncio.create_subprocess_exec(
            self._rkat_path, "rpc",
            stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )
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
        """Terminate the rkat rpc subprocess."""
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
            provider_params: Optional[dict] = None) -> WireRunResult:
        """Create a new session and run the first turn."""
        params = self._build_create_params(prompt, model, provider, system_prompt,
            max_tokens, output_schema, structured_output_retries, hooks_override,
            enable_builtins, enable_shell, enable_subagents, enable_memory,
            host_mode, comms_name, provider_params)
        result = await self._request("session/create", params)
        return self._parse_run_result(result)

    async def start_turn(self, session_id: str, prompt: str) -> WireRunResult:
        """Start a new turn on an existing session."""
        result = await self._request("turn/start", {"session_id": session_id, "prompt": prompt})
        return self._parse_run_result(result)

    # --- Session lifecycle (streaming) ---

    def create_session_streaming(self, prompt: str, model: Optional[str] = None,
            provider: Optional[str] = None, system_prompt: Optional[str] = None,
            max_tokens: Optional[int] = None, output_schema: Optional[dict] = None,
            structured_output_retries: int = 2, hooks_override: Optional[dict] = None,
            enable_builtins: bool = False, enable_shell: bool = False,
            enable_subagents: bool = False, enable_memory: bool = False,
            host_mode: bool = False, comms_name: Optional[str] = None,
            provider_params: Optional[dict] = None) -> StreamingTurn:
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
            host_mode, comms_name, provider_params)
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

    def start_turn_streaming(self, session_id: str, prompt: str) -> StreamingTurn:
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
        request = {"jsonrpc": "2.0", "id": request_id, "method": "turn/start",
                   "params": {"session_id": session_id, "prompt": prompt}}
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
        await self._request("config/set", {"config": config})

    async def patch_config(self, patch: dict) -> dict:
        """Merge-patch runtime configuration. Returns updated config."""
        return await self._request("config/patch", {"patch": patch})

    # --- Internal ---

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
        provider_params: Optional[dict],
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
        if provider_params:
            params["provider_params"] = provider_params
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
            text=data.get("text", ""),
            turns=data.get("turns", 0),
            tool_calls=data.get("tool_calls", 0),
            usage=usage,
            structured_output=data.get("structured_output"),
            schema_warnings=data.get("schema_warnings"),
        )
