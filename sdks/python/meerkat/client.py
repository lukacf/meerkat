"""Meerkat client — spawns rkat rpc subprocess and communicates via JSON-RPC."""

import asyncio
import json
import subprocess
import sys
from typing import AsyncIterator, Optional

from .errors import CapabilityUnavailableError, MeerkatError
from .generated.types import (
    CONTRACT_VERSION,
    CapabilitiesResponse,
    CapabilityEntry,
    WireEvent,
    WireRunResult,
    WireUsage,
)


class MeerkatClient:
    """Async client that communicates with a Meerkat runtime via rkat rpc."""

    def __init__(self, rkat_path: str = "rkat"):
        self._rkat_path = rkat_path
        self._process: Optional[subprocess.Popen] = None
        self._request_id = 0
        self._capabilities: Optional[CapabilitiesResponse] = None

    async def connect(self) -> "MeerkatClient":
        """Start the rkat rpc subprocess and perform handshake."""
        self._process = subprocess.Popen(
            [self._rkat_path, "rpc"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )

        # Initialize handshake
        result = await self._request("initialize", {})
        server_version = result.get("contract_version", "")
        if not self._check_version_compatible(server_version, CONTRACT_VERSION):
            raise MeerkatError(
                "VERSION_MISMATCH",
                f"Server version {server_version} incompatible with SDK {CONTRACT_VERSION}",
            )

        # Fetch capabilities
        caps_result = await self._request("capabilities/get", {})
        self._capabilities = CapabilitiesResponse(
            contract_version=caps_result.get("contract_version", ""),
            capabilities=[
                CapabilityEntry(
                    id=c.get("id", ""),
                    description=c.get("description", ""),
                    status=c.get("status", "available"),
                )
                for c in caps_result.get("capabilities", [])
            ],
        )

        return self

    async def close(self) -> None:
        """Terminate the rkat rpc subprocess."""
        if self._process:
            if self._process.stdin:
                self._process.stdin.close()
            self._process.terminate()
            self._process.wait(timeout=5)
            self._process = None

    async def create_session(
        self,
        prompt: str,
        model: Optional[str] = None,
        system_prompt: Optional[str] = None,
        max_tokens: Optional[int] = None,
    ) -> WireRunResult:
        """Create a new session and run the first turn."""
        params = {"prompt": prompt}
        if model:
            params["model"] = model
        if system_prompt:
            params["system_prompt"] = system_prompt
        if max_tokens:
            params["max_tokens"] = max_tokens

        result = await self._request("session/create", params)
        return self._parse_run_result(result)

    async def start_turn(
        self,
        session_id: str,
        prompt: str,
    ) -> WireRunResult:
        """Start a new turn on an existing session."""
        result = await self._request(
            "turn/start", {"session_id": session_id, "prompt": prompt}
        )
        return self._parse_run_result(result)

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
            c.id == capability_id and c.status == "available"
            for c in self._capabilities.capabilities
        )

    def require_capability(self, capability_id: str) -> None:
        """Raise CapabilityUnavailableError if capability is not available."""
        if not self.has_capability(capability_id):
            raise CapabilityUnavailableError(
                "CAPABILITY_UNAVAILABLE",
                f"Capability '{capability_id}' is not available",
            )

    # --- Internal ---

    async def _request(self, method: str, params: dict) -> dict:
        """Send a JSON-RPC request and return the result."""
        if not self._process or not self._process.stdin or not self._process.stdout:
            raise MeerkatError("NOT_CONNECTED", "Client not connected")

        self._request_id += 1
        request = {
            "jsonrpc": "2.0",
            "id": self._request_id,
            "method": method,
            "params": params,
        }

        line = json.dumps(request) + "\n"
        self._process.stdin.write(line.encode())
        self._process.stdin.flush()

        # Read response (skip notifications)
        while True:
            response_line = self._process.stdout.readline()
            if not response_line:
                raise MeerkatError("CONNECTION_CLOSED", "rkat rpc process closed")

            response = json.loads(response_line)
            if "id" in response and response["id"] == self._request_id:
                if "error" in response and response["error"]:
                    err = response["error"]
                    raise MeerkatError(
                        str(err.get("code", "UNKNOWN")),
                        err.get("message", "Unknown error"),
                        err.get("data"),
                    )
                return response.get("result", {})

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
