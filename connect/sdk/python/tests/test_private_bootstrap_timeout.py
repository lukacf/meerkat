"""Private acknowledgement budgets on real pipes; no native Ready/settlement claim."""
import dataclasses
import hashlib
import json
import os
import select
import time
from concurrent.futures import ThreadPoolExecutor

import pytest
from meerkat_meta._transport import PersistentTransport
from meerkat_meta.builder import MetaBuilderConfig
from meerkat_toolkit.application_runtime import ToolkitApplicationRuntime
from meerkat_toolkit.application_update import ToolkitApplicationUpdateRuntime
from meerkat_toolkit.host_control import (
    ApplicationPreparation, ApplicationUpdateStartup, HostBootstrapSelection,
    HostReplyCode, HostReplyError, PrivateHostClient, SelectedHostArtifact,
    ToolkitMobKitRuntime, ToolkitPersistentTransport,
)


def _artifact(tmp_path):
    path = tmp_path / "selection.json"
    raw = b'{"fixture":"private startup only"}\n'
    path.write_bytes(raw)
    return SelectedHostArtifact(str(path), hashlib.sha256(raw).hexdigest())


def _runtime(kind, config, tmp_path):
    artifact = _artifact(tmp_path)
    if kind == "update":
        source = ApplicationUpdateStartup(artifact, artifact, str(tmp_path / "associations"))
        return ToolkitApplicationUpdateRuntime(config, source), source
    source = HostBootstrapSelection(str(tmp_path), artifact, artifact, artifact)
    return ToolkitMobKitRuntime(config, source), source


def _pipe_peer(monkeypatch):
    streams, calls, clients = {}, [], []
    start_call = PrivateHostClient._start_call

    def before_process(owner):
        read_fd, write_fd = owner._inherited_fds
        streams["read"] = os.fdopen(os.dup(read_fd), "rb")
        streams["write"] = os.fdopen(os.dup(write_fd), "wb", buffering=0)

    def observed_call(client, command, **kwargs):
        before = time.monotonic()
        result = start_call(client, command, **kwargs)
        clients.append(client)
        calls.append((command["kind"], kwargs.get("timeout"), result[1] - before, client._timeout))
        return result

    # Keep real Toolkit pipe creation, private writer/reader and correlation.
    # Only the external native child is replaced by this explicit protocol peer.
    monkeypatch.setattr(PersistentTransport, "start", before_process)
    monkeypatch.setattr(PrivateHostClient, "_start_call", observed_call)
    return streams, calls, clients


def _receive(streams):
    # The worker may still be constructing its two pipe ends.
    deadline = time.monotonic() + 3
    while "read" not in streams and time.monotonic() < deadline:
        time.sleep(0.001)
    assert "read" in streams, "private pipe was not created"
    assert select.select([streams["read"]], [], [], max(0, deadline - time.monotonic()))[0]
    return json.loads(streams["read"].readline())


def _reply(streams, request, result):
    streams["write"].write(json.dumps({"version": 1, "id": request["id"], "result": result}).encode() + b"\n")


def _close(streams, clients, transport, pool=None):
    # Wake the real waiters before joining the test worker, even on assertion loss.
    owned = {id(client): client for client in clients}
    if transport.host_control is not None:
        owned[id(transport.host_control)] = transport.host_control
    for client in owned.values():
        client.close()
    for stream in streams.values():
        stream.close()
    if pool is not None:
        pool.shutdown(wait=True)


@pytest.mark.parametrize("kind", ["restore", "update"])
@pytest.mark.parametrize("budget", [60.0, 17.0, 900.0])
@pytest.mark.parametrize("refused", [False, True])
def test_runtime_private_ack_budget_preserves_ordinary_budgets_and_refusal(kind, budget, refused, tmp_path, monkeypatch):
    config = MetaBuilderConfig(gateway_bin="/inert/not-spawned", gateway_init_timeout=budget, init_deadline=3.0)
    runtime, source = _runtime(kind, config, tmp_path)
    transport = runtime._make_gateway_transport()
    streams, calls, clients = _pipe_peer(monkeypatch)
    pool = ThreadPoolExecutor(max_workers=1)
    pending = pool.submit(transport.start)
    try:
        request = _receive(streams)
        command = {"kind": "bootstrap", "selection": dataclasses.asdict(source)} if kind == "restore" else {
            "kind": "replace_application", "startup": dataclasses.asdict(source)}
        assert request == {"version": 1, "id": 1, "command": command}
        _reply(streams, request, {"kind": "error", "error": "denied"} if refused else {"kind": "bootstrap_accepted"})
        if refused:
            with pytest.raises(HostReplyError) as failure:
                pending.result(timeout=3)
            assert failure.value.code is HostReplyCode.DENIED
            assert transport.host_control is None
        else:
            assert pending.result(timeout=3) is None
            client = transport.host_control
            assert client is not None
            opened = pool.submit(client.open_component, "selected-component")
            request = _receive(streams)
            assert request["id"] == 2 and request["command"] == {"kind": "open_component", "component": "selected-component"}
            _reply(streams, request, {"kind": "component", "observation": {
                "incarnation": "00000000-0000-0000-0000-000000000001", "component": "selected-component", "revision": 1, "state": "open"}})
            opened.result(timeout=3)
            assert calls[1][1] is None
            assert calls[1][2] == pytest.approx(60.0, abs=1.0)
        assert calls[0][1] == budget and calls[0][2] == pytest.approx(budget, abs=1.0)
        assert all(row[3] == 60.0 for row in calls)
        assert transport.request_timeout == 60.0
        assert config.gateway_init_timeout == budget and config.init_deadline == 3.0
        assert len(calls) == (1 if refused else 2)
        assert transport._extra_args == transport._inherited_fds == ()
    finally:
        _close(streams, clients, transport, pool)


@pytest.mark.parametrize("kind", ["restore", "update"])
def test_private_ack_timeout_preserves_unknown_outcome_and_never_replays(kind, tmp_path, monkeypatch):
    config = MetaBuilderConfig(gateway_bin="/inert/not-spawned", gateway_init_timeout=0.25, init_deadline=900.0)
    runtime, _ = _runtime(kind, config, tmp_path)
    transport = runtime._make_gateway_transport()
    streams, calls, clients = _pipe_peer(monkeypatch)
    pool = ThreadPoolExecutor(max_workers=1)
    pending = pool.submit(transport.start)
    try:
        assert _receive(streams)["id"] == 1
        # No acknowledgement is supplied. This is the real private wait timeout.
        with pytest.raises(TimeoutError, match="native host observation timed out; settlement is unknown"):
            pending.result(timeout=3)
        assert transport.host_control is None and len(calls) == 1
        assert select.select([streams["read"]], [], [], 3)[0], "timeout must close its writer"
        assert streams["read"].read(1) == b"", "timeout must close without a second command"
        assert transport.request_timeout == 60.0 and config.init_deadline == 900.0
    finally:
        _close(streams, clients, transport, pool)


def test_application_preparation_keeps_its_explicit_observation_budget(tmp_path, monkeypatch):
    config = MetaBuilderConfig(gateway_bin="/inert/not-spawned", gateway_init_timeout=900.0, init_deadline=3.0)
    source = ApplicationPreparation(_artifact(tmp_path), timeout=7.0)
    runtime = ToolkitApplicationRuntime(config, source, lambda _: pytest.fail("no finalizer is admitted by this test"))
    transport = runtime._make_gateway_transport()
    streams, calls, clients = _pipe_peer(monkeypatch)
    try:
        transport.start()
        assert _receive(streams) == {"version": 1, "id": 1, "command": {
            "kind": "prepare_application", "plan": dataclasses.asdict(source.plan)}}
        assert calls[0][1] == 7.0 and calls[0][2] == pytest.approx(7.0, abs=1.0)
        assert transport.request_timeout == calls[0][3] == 60.0
        assert transport.preparation is not None and len(calls) == 1
    finally:
        _close(streams, clients, transport)


@pytest.mark.parametrize("budget", [True, 0, -1, float("nan"), float("inf"), "900"])
def test_invalid_private_bootstrap_budget_refuses_before_native_transport(budget, monkeypatch):
    entered = []
    monkeypatch.setattr(PersistentTransport, "__init__", lambda *_a, **_k: entered.append(True))
    with pytest.raises(ValueError, match="private bootstrap timeout must be positive and finite"):
        ToolkitPersistentTransport("/inert/not-spawned", object(), bootstrap_timeout=budget)
    assert entered == []
