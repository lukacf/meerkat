"""E2E smoke tests for the Python SDK against a live rkat rpc server.

These tests exercise the full Python SDK lifecycle including session
management, capabilities, and error handling through a real rkat rpc subprocess.

Requirements:
  - rkat binary on PATH (cargo build -p meerkat-cli)
  - ANTHROPIC_API_KEY set for live API tests

Run with: pytest tests/test_e2e_smoke.py -v
"""

import json
import os
import shutil
import subprocess

import pytest

# Skip all tests if rkat binary is not available
pytestmark = pytest.mark.skipif(
    shutil.which("rkat") is None,
    reason="rkat binary not found on PATH",
)


def has_api_key():
    return bool(
        os.environ.get("RKAT_ANTHROPIC_API_KEY")
        or os.environ.get("ANTHROPIC_API_KEY")
    )


def rpc_request(proc, method, params=None, request_id=None):
    """Send a JSON-RPC request and return the result (skipping notifications)."""
    if params is None:
        params = {}
    if request_id is None:
        # Use a simple counter based on method name hash
        request_id = abs(hash(method)) % 100000

    request = {
        "jsonrpc": "2.0",
        "id": request_id,
        "method": method,
        "params": params,
    }

    proc.stdin.write((json.dumps(request) + "\n").encode())
    proc.stdin.flush()

    # Read lines until we get a response with matching id
    while True:
        line = proc.stdout.readline()
        if not line:
            raise RuntimeError("rkat rpc process closed unexpectedly")
        response = json.loads(line)
        if "id" in response and response["id"] == request_id:
            if "error" in response and response["error"]:
                err = response["error"]
                raise RuntimeError(
                    f"RPC error {err.get('code')}: {err.get('message')}"
                )
            return response.get("result", {})


def spawn_rpc():
    """Spawn an rkat rpc subprocess."""
    return subprocess.Popen(
        ["rkat", "rpc"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )


# ---------------------------------------------------------------------------
# Scenario 18: Full Python SDK lifecycle
# ---------------------------------------------------------------------------


@pytest.mark.skipif(not has_api_key(), reason="no ANTHROPIC_API_KEY")
def test_smoke_full_lifecycle():
    """Create session, multi-turn, list, archive — full lifecycle."""
    proc = spawn_rpc()
    try:
        # Initialize
        init = rpc_request(proc, "initialize", {}, 1)
        assert "server_info" in init

        # Create session with context
        create = rpc_request(
            proc,
            "session/create",
            {"prompt": "My name is PyBot. Remember that."},
            2,
        )
        session_id = create.get("session_id", "")
        assert session_id, "session_id should be non-empty"
        assert create.get("text", ""), "response text should be non-empty"

        # Follow-up turn — recall context
        turn = rpc_request(
            proc,
            "turn/start",
            {"session_id": session_id, "prompt": "What is my name?"},
            3,
        )
        assert "PyBot" in turn.get("text", ""), (
            f"Should recall PyBot, got: {turn.get('text', '')}"
        )

        # List sessions — should include ours
        sessions = rpc_request(proc, "session/list", {}, 4)
        session_ids = [s.get("session_id", "") for s in sessions.get("sessions", [])]
        assert session_id in session_ids, "Session should appear in list"

        # Archive
        rpc_request(proc, "session/archive", {"session_id": session_id}, 5)

        # List again — should be gone
        sessions = rpc_request(proc, "session/list", {}, 6)
        session_ids = [s.get("session_id", "") for s in sessions.get("sessions", [])]
        assert session_id not in session_ids, "Session should be gone after archive"

    finally:
        proc.terminate()
        proc.wait(timeout=5)


# ---------------------------------------------------------------------------
# Scenario 19: Capabilities + version check
# ---------------------------------------------------------------------------


def test_smoke_capabilities():
    """Verify capabilities endpoint returns correct structure."""
    proc = spawn_rpc()
    try:
        # Initialize
        rpc_request(proc, "initialize", {}, 1)

        # Get capabilities
        caps = rpc_request(proc, "capabilities/get", {}, 2)
        assert caps.get("contract_version"), "Should have contract_version"

        capabilities = caps.get("capabilities", [])
        assert len(capabilities) > 0, "Should have at least one capability"

        # Find sessions capability
        cap_ids = [c.get("id", "") for c in capabilities]
        assert "sessions" in cap_ids, "Should have 'sessions' capability"

        # Verify Available status uses capital A (our fix)
        sessions_cap = next(c for c in capabilities if c.get("id") == "sessions")
        assert sessions_cap.get("status") == "Available", (
            f"Status should be 'Available' (capital A), got: {sessions_cap.get('status')}"
        )

    finally:
        proc.terminate()
        proc.wait(timeout=5)


# ---------------------------------------------------------------------------
# Scenario 20: Error handling
# ---------------------------------------------------------------------------


@pytest.mark.skipif(not has_api_key(), reason="no ANTHROPIC_API_KEY")
def test_smoke_error_handling():
    """Verify errors propagate correctly, and recovery works after error."""
    proc = spawn_rpc()
    try:
        # Initialize
        rpc_request(proc, "initialize", {}, 1)

        # Try to start turn on non-existent session — should error
        with pytest.raises(RuntimeError, match="error"):
            rpc_request(
                proc,
                "turn/start",
                {
                    "session_id": "00000000-0000-0000-0000-000000000000",
                    "prompt": "hello",
                },
                2,
            )

        # Recovery: create a real session after the error
        create = rpc_request(
            proc,
            "session/create",
            {"prompt": "What is 2+2? Reply with just the number."},
            3,
        )
        assert create.get("session_id"), "Should get session_id after recovery"
        text = create.get("text", "")
        assert "4" in text, f"Should contain '4', got: {text}"

    finally:
        proc.terminate()
        proc.wait(timeout=5)
