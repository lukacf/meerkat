"""The runtime reports the gateway shutdown horizon its transport accepted."""
from unittest.mock import MagicMock, patch

import pytest

from meerkat_meta._transport import PersistentTransport
from meerkat_meta.runtime import MetaRuntime


def initialized(result):
    transport = PersistentTransport("unused")
    transport._ensure_running = lambda: None
    transport._send_sync_running = MagicMock(
        return_value={"jsonrpc": "2.0", "id": "init", "result": result}
    )
    transport.send_sync(
        {"jsonrpc": "2.0", "id": "init", "method": "meta/init", "params": {}}
    )
    return transport


@pytest.mark.parametrize(
    "result, expected",
    [
        ({"stdio_shutdown_handshake": True, "stdio_shutdown_horizon_ms": 392_000}, 392_000),
        ({"stdio_shutdown_handshake": True, "stdio_shutdown_horizon_ms": 432_000}, 432_000),
        # A value the transport refused leaves it on its own default, which
        # is not what the gateway advertised.
        ({"stdio_shutdown_handshake": True, "stdio_shutdown_horizon_ms": "392000"}, None),
        ({"stdio_shutdown_handshake": True}, None),
        ({"stdio_shutdown_horizon_ms": 392_000}, None),
    ],
    ids=["392s", "432s", "invalid", "not-advertised", "no-handshake"],
)
def test_runtime_reports_the_gateway_shutdown_horizon_it_accepted(result, expected):
    runtime = MetaRuntime(None, initialized(result))
    assert runtime.gateway_shutdown_horizon_ms == expected


def test_runtime_without_a_negotiated_gateway_reports_no_horizon():
    assert MetaRuntime(None, None).gateway_shutdown_horizon_ms is None
    assert MetaRuntime(None, PersistentTransport("unused")).gateway_shutdown_horizon_ms is None


def test_a_replacement_gateway_does_not_inherit_the_previous_horizon():
    transport = initialized(
        {"stdio_shutdown_handshake": True, "stdio_shutdown_horizon_ms": 432_000}
    )
    process = MagicMock()
    process.poll.return_value = None
    process.stdout.readline.return_value = b""
    with patch("meerkat_meta._transport.subprocess.Popen", return_value=process):
        transport.start()
    assert MetaRuntime(None, transport).gateway_shutdown_horizon_ms is None
    transport._process = None
