"""E2E tests for the Python SDK against a live rkat-rpc server.

These tests require:
  - rkat-rpc binary on PATH (cargo build -p meerkat-rpc)
  - ANTHROPIC_API_KEY set in environment

Run with: pytest tests/test_e2e.py -v (skipped by default if rkat-rpc not found)
"""

import shutil
import subprocess

import pytest

# Skip all tests in this file if rkat-rpc is not available
pytestmark = pytest.mark.skipif(
    shutil.which("rkat-rpc") is None and shutil.which("rkat") is None,
    reason="rkat-rpc binary not found on PATH",
)


def test_rkat_rpc_starts_and_responds():
    """Verify rkat-rpc subprocess starts and responds to initialize."""
    import json

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
