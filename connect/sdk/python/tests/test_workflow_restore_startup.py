"""Actual SDK runtime/pipe/startup boundaries, without native Ready or R5 claims."""
import dataclasses
import hashlib
import json
import os
import select
import threading
from concurrent.futures import ThreadPoolExecutor
from types import SimpleNamespace

import pytest
from meerkat_meta._transport import PersistentTransport
from meerkat_toolkit.host_control import (
    ApplicationPreparation, ApplicationUpdateStartup, HostBootstrapSelection,
    HostControlError, HostReplyError, HostReplyCode, PrivateHostClient,
    SelectedHostArtifact, ToolkitMobKitRuntime, ToolkitPersistentTransport,
)
from meerkat_toolkit.application_runtime import ToolkitApplicationRuntime


def selected():
    return HostBootstrapSelection("/actual-selection/application",
        SelectedHostArtifact("/actual-selection/activation.json", "a" * 64),
        SelectedHostArtifact("/actual-selection/manifest.json", "b" * 64),
        SelectedHostArtifact("/actual-selection/policy.json", "c" * 64))


@pytest.mark.parametrize("enabled", [False, True])
@pytest.mark.parametrize("refused", [False, True])
def test_restore_launch_preserves_actual_bootstrap_bytes_and_native_refusal(enabled, refused, monkeypatch):
    # Real runtime -> real Toolkit transport -> actual two anonymous pipes ->
    # real PrivateHostClient.bootstrap. Only the external child is a peer here.
    config = SimpleNamespace(gateway_bin="/inert/not-spawned")
    runtime = ToolkitMobKitRuntime(config, selected(), private_workflow_launch=enabled)
    transport = runtime._make_gateway_transport()
    transport._timeout = 2
    ready = threading.Event(); peer = {}; launches = []
    def before_native_process(owner):
        launches.append(owner._extra_args)
        read, write = owner._inherited_fds
        peer["read"] = os.fdopen(os.dup(read), "rb")
        peer["write"] = os.fdopen(os.dup(write), "wb", buffering=0)
        ready.set()
    monkeypatch.setattr(PersistentTransport, "start", before_native_process)
    pool = ThreadPoolExecutor(max_workers=1)
    pending = pool.submit(transport.start)
    try:
        assert ready.wait(2)
        assert select.select([peer["read"]], [], [], 2)[0], "actual bootstrap pipe did not receive a request"
        request = json.loads(peer["read"].readline())
        assert request["command"] == {"kind":"bootstrap", "selection":dataclasses.asdict(selected())}
        reply = {"kind":"error", "error":"denied"} if refused else {"kind":"bootstrap_accepted"}
        peer["write"].write(json.dumps({"version":1,"id":request["id"],"result":reply}).encode()+b"\n")
        if refused:
            with pytest.raises(HostReplyError) as error: pending.result(timeout=3)
            assert error.value.code == HostReplyCode.DENIED
            assert transport.host_control is None
        else:
            assert pending.result(timeout=3) is None
            assert isinstance(transport.host_control, PrivateHostClient)
            transport.host_control.close()
        assert launches[0].count("--private-workflow-launch") == int(enabled)
        assert "--initialize-native-owner" not in launches[0]
        assert transport._selection == selected() and transport.preparation is None
        assert transport._extra_args == transport._inherited_fds == ()
        assert vars(config) == {"gateway_bin":"/inert/not-spawned"}
        assert peer["read"].read() == b"", "no retry, prepare, replacement or extra owner command"
    finally:
        if transport.host_control is not None: transport.host_control.close()
        for stream in peer.values(): stream.close()
        pool.shutdown(wait=True)


@pytest.mark.parametrize("enabled", [False, True])
def test_existing_preparation_keeps_exact_plan_and_one_launch_choice(enabled, tmp_path, monkeypatch):
    raw=b'{"schema":4,"fixture":"inert startup only"}\n'
    path=tmp_path/"plan.json";path.write_bytes(raw)
    selection=ApplicationPreparation(SelectedHostArtifact(str(path),hashlib.sha256(raw).hexdigest()),timeout=2)
    calls=[]
    def finalizer(_): calls.append("unexpected finalizer"); raise AssertionError("not a native preparation test")
    runtime=ToolkitApplicationRuntime(SimpleNamespace(gateway_bin="/inert/not-spawned"),selection,finalizer,
        private_workflow_launch=enabled)
    transport=runtime._make_gateway_transport()
    launches=[]
    def stop_before_spawn(owner): launches.append(owner._extra_args); raise RuntimeError("inert before spawn")
    monkeypatch.setattr(PersistentTransport,"start",stop_before_spawn)
    with pytest.raises(RuntimeError,match="before spawn"): transport.start()
    assert launches[0].count("--private-workflow-launch")==int(enabled)
    assert transport._selection is selection and runtime._application_plan==raw and calls==[]
    assert transport._extra_args==transport._inherited_fds==()
    with pytest.raises(HostControlError,match="already started"):runtime._make_gateway_transport()


@pytest.mark.parametrize("bad", [None, 1, "true", {}, []])
def test_nonboolean_opt_in_refuses_before_runtime_or_process_construction(bad, monkeypatch):
    entered=[]
    monkeypatch.setattr(PersistentTransport,"__init__",lambda *_a,**_k:entered.append("transport"))
    with pytest.raises(HostControlError,match="explicit boolean"):
        ToolkitPersistentTransport("/inert/not-spawned",selected(),private_workflow_launch=bad)
    with pytest.raises(HostControlError,match="explicit boolean"):
        ToolkitMobKitRuntime(SimpleNamespace(gateway_bin="/inert/not-spawned"),selected(),private_workflow_launch=bad)
    assert entered==[]


@pytest.mark.parametrize("case", ["update", "foreign", "initialize"])
def test_opt_in_cannot_select_update_foreign_or_legacy_initialization(case, monkeypatch):
    source=selected()
    if case=="update":source=ApplicationUpdateStartup(SelectedHostArtifact("/must-not-read","a"*64),
        SelectedHostArtifact("/prior","b"*64),"/associations")
    elif case=="foreign":source={"selection":dataclasses.asdict(source),"private_workflow_launch":True}
    entered=[]
    monkeypatch.setattr(PersistentTransport,"__init__",lambda *_a,**_k:entered.append("transport"))
    for factory in (lambda:ToolkitPersistentTransport("/inert/not-spawned",source,private_workflow_launch=True,
                    initialize_native_owner=case=="initialize"),
                    lambda:ToolkitMobKitRuntime(SimpleNamespace(gateway_bin="/inert/not-spawned"),source,
                    private_workflow_launch=True,initialize_native_owner=case=="initialize")):
        with pytest.raises(HostControlError,match="ordinary installed Restore"):factory()
    assert entered==[]


def test_model_configuration_cannot_enable_private_workflow_startup():
    # A similarly named opaque model/config field is not the constructor option.
    config=SimpleNamespace(gateway_bin="/inert/not-spawned",private_workflow_launch=True)
    runtime=ToolkitMobKitRuntime(config,selected())
    assert runtime._make_gateway_transport()._private_workflow_launch is False
    assert vars(config)=={"gateway_bin":"/inert/not-spawned","private_workflow_launch":True}
