"""E2E tests for the Python SDK against a live rkat-rpc server.

These tests require:
  - rkat-rpc binary on PATH (cargo build -p meerkat-rpc)
  - ANTHROPIC_API_KEY set in environment

Run with: pytest tests/test_e2e.py -v (skipped by default if rkat-rpc not found)
"""

import asyncio
from pathlib import Path
import shutil
import subprocess

import pytest

WORKSPACE_BIN = Path(__file__).resolve().parents[3] / "target-codex" / "debug" / "rkat-rpc"


# Skip all tests in this file if no usable rpc binary is available.
pytestmark = pytest.mark.skipif(
    not WORKSPACE_BIN.exists()
    and shutil.which("rkat-rpc") is None
    and shutil.which("rkat") is None,
    reason="rkat-rpc binary not found on PATH",
)


def test_rkat_rpc_starts_and_responds():
    """Verify rkat-rpc subprocess starts and responds to initialize."""
    import json

    if WORKSPACE_BIN.exists():
        binary = str(WORKSPACE_BIN)
        args = [binary]
    else:
        binary = "rkat-rpc" if shutil.which("rkat-rpc") else "rkat"
        args = [binary] + (["rpc"] if binary == "rkat" else [])
    proc = subprocess.Popen(
        args,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )

    try:
        request = json.dumps({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {},
        }) + "\n"

        proc.stdin.write(request.encode())
        proc.stdin.flush()

        response_line = proc.stdout.readline()
        assert response_line, "rkat-rpc should respond to initialize"

        response = json.loads(response_line)
        assert "result" in response or "error" not in response
        assert response.get("id") == 1
    finally:
        proc.terminate()
        proc.wait(timeout=5)


def test_rkat_capabilities_command():
    """Verify rkat capabilities command produces valid JSON."""
    import json

    result = subprocess.run(
        ["rkat", "capabilities"],
        capture_output=True,
        text=True,
        timeout=10,
    )

    assert result.returncode == 0, f"rkat capabilities failed: {result.stderr}"

    data = json.loads(result.stdout)
    assert "contract_version" in data
    assert "capabilities" in data
    assert len(data["capabilities"]) > 0

    # Verify at least Sessions is present
    cap_ids = [c["id"] for c in data["capabilities"]]
    assert "sessions" in cap_ids


@pytest.mark.asyncio
async def test_python_sdk_live_mob_stream_explicit_close():
    """Verify the packaged Python SDK observes explicit_close on a live mob stream."""
    from meerkat import MeerkatClient

    rpc_path = str(WORKSPACE_BIN) if WORKSPACE_BIN.exists() else (
        "rkat-rpc" if shutil.which("rkat-rpc") else "rkat"
    )

    async with MeerkatClient(rpc_path) as client:
        created = await client.call_mob_tool("mob_create", {"prefab": "coding_swarm"})
        mob_id = str(created.get("mob_id", ""))
        assert mob_id

        sub = await client.subscribe_mob_events(mob_id)
        await sub.close()

        for _ in range(10):
            outcome = sub.terminal_outcome
            if outcome and outcome.get("outcome") == "explicit_close":
                break
            await asyncio.sleep(0.02)

        assert sub.terminal_outcome is not None
        assert sub.terminal_outcome.get("outcome") == "explicit_close"
