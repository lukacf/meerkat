"""Host-owned gateway startup: inherited descriptors, extra args, factory hook.

A host composes the local gateway process through
``MetaRuntime._make_gateway_transport``. The transport passes exactly the
descriptors it was given (POSIX ``pass_fds``) and appends ``extra_args``
after ``--persistent``; neither is ever serialized into RPC. Each test drives
a stand-in gateway script, so the real launch path runs without a gateway
binary.
"""
from __future__ import annotations

import json
import os
import sys
import textwrap
from pathlib import Path

import pytest

from meerkat_meta._transport import PersistentTransport, TransportTermination
from meerkat_meta.builder import Meta
from meerkat_meta.runtime import MetaRuntime

# Records argv and the token readable on the descriptor named by
# ``--token-fd``, then answers init like an older gateway and serves until
# stdin closes.
_GATEWAY = """
import json, os, pathlib, sys
record = pathlib.Path(sys.argv[0]).with_suffix(".record")
entry = {"argv": sys.argv[1:]}
if "--token-fd" in sys.argv:
    fd = int(sys.argv[sys.argv.index("--token-fd") + 1])
    entry["token"] = os.read(fd, 64).decode()
init = json.loads(sys.stdin.readline())
entry["init_params"] = init.get("params", {})
record.write_text(json.dumps(entry))
sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": init["id"],
    "result": {"http_base_url": "http://127.0.0.1:5"}}) + "\\n")
sys.stdout.flush()
for raw in sys.stdin:
    pass
"""


def _gateway(tmp_path: Path) -> Path:
    script = tmp_path / "gateway.py"
    script.write_text(f"#!{sys.executable}\n{textwrap.dedent(_GATEWAY)}")
    script.chmod(0o755)
    return script


@pytest.mark.parametrize(
    "kwargs",
    [
        {"inherited_fds": (0,)},
        {"inherited_fds": (2,)},
        {"inherited_fds": (True,)},
        {"inherited_fds": ("5",)},
        {"inherited_fds": (5, 5)},
        {"extra_args": ("ok", "bad\x00arg")},
        {"extra_args": (7,)},
    ],
)
def test_startup_capabilities_are_validated_before_any_spawn(kwargs):
    with pytest.raises(ValueError):
        PersistentTransport("unused", **kwargs)


def test_launch_passes_exactly_the_inherited_descriptors(monkeypatch):
    seen = {}

    def fake_popen(argv, **kwargs):
        # Keep only the launch facts under test: the env holds the whole
        # process environment and must never reach failure output.
        seen["argv"] = argv
        seen["close_fds"] = kwargs.get("close_fds")
        seen["pass_fds"] = kwargs.get("pass_fds")
        del kwargs
        raise OSError("stop before a child exists")

    monkeypatch.setattr("meerkat_meta._transport.subprocess.Popen", fake_popen)
    transport = PersistentTransport(
        "/opt/gw", inherited_fds=(7, 9), extra_args=("--token-fd", "7")
    )
    with pytest.raises(OSError):
        transport.start()
    assert seen["argv"] == ["/opt/gw", "--persistent", "--token-fd", "7"]
    assert seen["close_fds"] is True
    assert seen["pass_fds"] == (7, 9)
    # No gateway process was started, so there is no exit to report.
    assert transport.last_termination == TransportTermination(
        "not_started", None, None, "none", "not_negotiated"
    )


def test_default_launch_is_unchanged(monkeypatch):
    seen = {}

    def fake_popen(argv, **kwargs):
        # Keep only the launch facts under test: the env holds the whole
        # process environment and must never reach failure output.
        seen["argv"] = argv
        seen["close_fds"] = kwargs.get("close_fds")
        seen["pass_fds"] = kwargs.get("pass_fds")
        del kwargs
        raise OSError("stop before a child exists")

    monkeypatch.setattr("meerkat_meta._transport.subprocess.Popen", fake_popen)
    with pytest.raises(OSError):
        PersistentTransport("/opt/gw").start()
    assert seen["argv"] == ["/opt/gw", "--persistent"]
    assert seen["pass_fds"] == ()


@pytest.mark.skipif(os.name != "posix", reason="descriptor inheritance is POSIX-only")
@pytest.mark.asyncio
async def test_subclass_factory_is_used_by_connect_and_stopped_by_shutdown(tmp_path):
    script = _gateway(tmp_path)
    read_end, write_end = os.pipe()
    built: list[PersistentTransport] = []

    class HostRuntime(MetaRuntime):
        def _make_gateway_transport(self) -> PersistentTransport:
            transport = PersistentTransport(
                self._config.gateway_bin,
                inherited_fds=(read_end,),
                extra_args=("--token-fd", str(read_end)),
            )
            built.append(transport)
            return transport

    os.write(write_end, b"host-secret")
    os.close(write_end)
    runtime = HostRuntime(Meta.builder().gateway(str(script))._config)
    try:
        await runtime.connect()
        (transport,) = built
        assert runtime._transport is transport
        assert transport.is_running()
    finally:
        await runtime.shutdown()
        os.close(read_end)

    assert not transport.is_running(), "shutdown reaps the factory-built child"
    # The stand-in exits 0 at stdin EOF; it offered no shutdown handshake.
    assert transport.last_termination == TransportTermination(
        "exited", 0, None, "none", "not_negotiated"
    )
    record = json.loads(script.with_suffix(".record").read_text())
    assert record["argv"] == ["--persistent", "--token-fd", str(read_end)]
    assert record["token"] == "host-secret"
    serialized = json.dumps(record["init_params"])
    assert "host-secret" not in serialized and "token-fd" not in serialized


@pytest.mark.asyncio
async def test_default_factory_launches_the_configured_gateway(tmp_path):
    script = _gateway(tmp_path)
    runtime = MetaRuntime(Meta.builder().gateway(str(script))._config)
    try:
        await runtime.connect()
    finally:
        await runtime.shutdown()
    record = json.loads(script.with_suffix(".record").read_text())
    assert record["argv"] == ["--persistent"]
