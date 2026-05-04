"""E2E tests for the Python SDK against a live rkat-rpc server.

These tests require:
  - rkat-rpc binary on PATH (cargo build -p meerkat-rpc)
  - ANTHROPIC_API_KEY set in environment

Run with: pytest tests/test_e2e.py -v (skipped by default if rkat-rpc not found)
"""

import asyncio
import subprocess

import pytest

from .live_smoke_support import resolve_rkat_rpc_path


# Skip all tests in this file if no usable rpc binary is available.
pytestmark = pytest.mark.skipif(
    resolve_rkat_rpc_path() is None,
    reason="rkat-rpc binary not found on PATH",
)


def test_rkat_rpc_starts_and_responds():
    """Verify rkat-rpc subprocess starts and responds to initialize."""
    import json

    binary = resolve_rkat_rpc_path()
    assert binary is not None
    args = [binary]
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


def test_rkat_rpc_capabilities_method():
    """Verify rkat-rpc capabilities/get produces valid JSON."""
    import json

    binary = resolve_rkat_rpc_path()
    assert binary is not None
    proc = subprocess.Popen(
        [binary],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )

    try:
        for request in [
            {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}},
            {"jsonrpc": "2.0", "id": 2, "method": "capabilities/get", "params": {}},
        ]:
            proc.stdin.write((json.dumps(request) + "\n").encode())
            proc.stdin.flush()

        response_line = proc.stdout.readline()
        assert response_line, "rkat-rpc should respond to initialize"
        response_line = proc.stdout.readline()
        assert response_line, "rkat-rpc should respond to capabilities/get"
        response = json.loads(response_line)
        data = response["result"]
        assert "capabilities" in data
        assert len(data["capabilities"]) > 0

        # Verify at least Sessions is present.
        cap_ids = [c["id"] for c in data["capabilities"]]
        assert "sessions" in cap_ids
    finally:
        proc.terminate()
        proc.wait(timeout=5)


@pytest.mark.asyncio
async def test_python_sdk_live_mob_stream_explicit_close():
    """Verify the packaged Python SDK observes explicit_close on a live mob stream."""
    from meerkat import MeerkatClient

    rpc_path = resolve_rkat_rpc_path()
    assert rpc_path is not None

    async with MeerkatClient(rpc_path) as client:
        if not client.has_capability("mob"):
            pytest.skip("packaged binary does not advertise mob capability")

        mob = await client.create_mob(
            definition={
                "id": "coding_swarm",
                "orchestrator": {"profile": "lead"},
                "profiles": {
                    "lead": {"model": "claude-opus-4-6"},
                    "worker": {"model": "claude-sonnet-4-6"},
                },
            }
        )
        mob_id = mob.id
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
