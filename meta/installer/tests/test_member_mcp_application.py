"""Forge's private participant boundary, not native admission qualification.

The optional Toolkit tests use the selected real SDK and an inert pipe peer.
The application fixture owns its row; Forge never creates a binding store.
"""
import asyncio
import copy
import json
import os
from types import SimpleNamespace

import pytest

from test_application_extension import load_host
from test_application_update_startup import fixture as replacement_fixture


def application(tmp_path, monkeypatch):
    module = load_host(monkeypatch, tmp_path)
    app = module.Application(module.ApplicationContext(tmp_path, {}, asyncio.Event()))
    app.dynamic = True
    app.integration = SimpleNamespace(admission_roster=lambda: {"listed": [], "admitting": []})
    return app


def test_no_participant_does_not_require_new_sdk(tmp_path, monkeypatch):
    app = application(tmp_path, monkeypatch)
    assert app.member_mcp_runtime_options() == {}


@pytest.mark.parametrize("missing", ["retain_member_mcp", "lookup_member_mcp", "admission_roster"])
def test_partial_private_hooks_are_refused(tmp_path, monkeypatch, missing):
    app = application(tmp_path, monkeypatch)
    app.integration.retain_member_mcp = lambda value: None
    app.integration.lookup_member_mcp = lambda selection: None
    delattr(app.integration, missing)
    with pytest.raises(ValueError):
        app.member_mcp_runtime_options()


@pytest.fixture
def sdk():
    # New private SDK contracts are exercised only with a selected development
    # artifact. Shipping 0.9.4 does not silently pretend to implement them.
    return pytest.importorskip("meerkat_toolkit.member_classes")


@pytest.mark.parametrize("refs", [[], ["opaque-one", "opaque-two"]])
async def test_private_retention_and_cold_read_use_exact_application_row(tmp_path, monkeypatch, sdk, refs):
    path = tmp_path / "application-row.json"
    key = {"admission_id": "original-add", "identity": "project-one", "class": "project"}
    # This is an application-owned fixture, not a Forge persistence mechanism.
    path.write_text(json.dumps({"selection": key, "state": "admitting"}))

    def hooks(app):
        async def retain(value):
            row = json.loads(path.read_text())
            if row["selection"] != value["selection"] or row["state"] != "admitting":
                raise ValueError("Application row changed")
            if "bindings" in row and row["bindings"] != value["bindings"]:
                raise ValueError("Retained binding set changed")
            row["bindings"] = value["bindings"]
            path.write_text(json.dumps(row))
        async def lookup(selection):
            row = json.loads(path.read_text())
            if row["selection"] != selection or row["state"] not in {"admitting", "listed"}:
                raise ValueError("Application row unavailable")
            return {"selection": row["selection"], "bindings": row["bindings"]}
        app.integration.retain_member_mcp = retain
        app.integration.lookup_member_mcp = lookup

    app = application(tmp_path, monkeypatch)
    hooks(app)
    participant = app.member_mcp_runtime_options()["member_mcp"]
    assert app.member_mcp_runtime_options()["member_mcp"] is participant
    selection = sdk.MemberMcpAdmissionSelection.from_wire(key)
    value = sdk.RetainedMemberMcpBindings(selection, tuple(refs))
    with pytest.raises(KeyError):
        await participant._lookup(selection)
    await participant._retain(value)
    await participant._retain(value)
    with pytest.raises(ValueError, match="changed"):
        await participant._retain(sdk.RetainedMemberMcpBindings(selection, ("different",)))
    await app.stop()

    # New host/participant, same already-loaded application row, before start.
    cold = application(tmp_path, monkeypatch)
    hooks(cold)
    restored = cold.member_mcp_runtime_options()["member_mcp"]
    assert await restored._lookup(selection) == value
    assert cold.integration.admission_roster() == {"listed": [], "admitting": []}
    row = json.loads(path.read_text())
    for state in ("void", "retired"):
        path.write_text(json.dumps({**row, "state": state}))
        with pytest.raises(ValueError, match="unavailable"):
            await restored._lookup(selection)
    path.write_text(json.dumps({**row, "selection": {**key, "admission_id": "successor"}}))
    with pytest.raises(ValueError, match="unavailable"):
        await restored._lookup(selection)
    await cold.stop()


@pytest.mark.parametrize("change", ["other-admission", "extra-field", "duplicate", "too-many", "missing"])
async def test_unreadable_or_mismatched_lookup_never_becomes_empty(tmp_path, monkeypatch, sdk, change):
    app = application(tmp_path, monkeypatch)
    selection = sdk.MemberMcpAdmissionSelection("add-one", "project-one", "project")
    value = {"selection": selection.wire(), "bindings": []}
    if change == "other-admission":
        value["selection"]["admission_id"] = "another"
    elif change == "extra-field":
        value["grant"] = "not authority"
    elif change == "duplicate":
        value["bindings"] = ["one", "one"]
    elif change == "too-many":
        value["bindings"] = [f"ref-{n}" for n in range(33)]
    else:
        value = None
    app.integration.retain_member_mcp = lambda value: None
    app.integration.lookup_member_mcp = lambda key: copy.deepcopy(value)
    participant = app.member_mcp_runtime_options()["member_mcp"]
    with pytest.raises((ValueError, RuntimeError)):
        await participant._lookup(selection)
    await app.stop()


async def test_stop_joins_entered_write_before_closing_store_despite_cancellation(tmp_path, monkeypatch, sdk):
    from meerkat_toolkit.host_control import PrivateHostClient
    app = application(tmp_path, monkeypatch)
    entered, release = asyncio.Event(), asyncio.Event()
    events = []
    async def retain(value):
        events.append("entered")
        entered.set()
        await release.wait()
        events.append("committed")
    app.integration.retain_member_mcp = retain
    app.integration.lookup_member_mcp = lambda key: None
    app.integration.stop = lambda: events.append("store closed")
    participant = app.member_mcp_runtime_options()["member_mcp"]
    child_read, parent_write = os.pipe()
    parent_read, child_write = os.pipe()
    host = PrivateHostClient(parent_write, parent_read, timeout=2, member_mcp=participant)
    reader = os.fdopen(child_read, "rb")
    writer = os.fdopen(child_write, "wb", buffering=0)
    stopping = None
    try:
        key = sdk.MemberMcpAdmissionSelection("add-one", "project-one", "project")
        frame = {"kind": "member_class_mcp_required", "exchange_id": 1,
                 "selection": key.wire(), "bindings": []}
        writer.write(json.dumps({"version": 1, "id": 0, "result": frame}).encode() + b"\n")
        await asyncio.wait_for(entered.wait(), 2)
        stopping = asyncio.create_task(app.stop())
        await asyncio.sleep(0)
        stopping.cancel()
        await asyncio.sleep(0)
        assert events == ["entered"] and not stopping.done()
        release.set()
        acknowledgement = json.loads(await asyncio.wait_for(asyncio.to_thread(reader.readline), 2))
        assert acknowledgement["command"]["operation"]["retained"]["bindings"] == []
        writer.write(json.dumps({"version": 1, "id": acknowledgement["id"], "result": {
            "kind": "member_class_mcp_acknowledged", "exchange_id": 1}}).encode() + b"\n")
        with pytest.raises(asyncio.CancelledError):
            await asyncio.wait_for(stopping, 2)
        assert events == ["entered", "committed", "store closed"]
        await participant.join()  # runtime shutdown's repeated join is safe
    finally:
        release.set()
        host.close()
        reader.close()
        writer.close()
        if stopping is not None and not stopping.done():
            await asyncio.gather(stopping, return_exceptions=True)


async def test_join_and_store_close_failures_are_both_retained(tmp_path, monkeypatch):
    app = application(tmp_path, monkeypatch)
    async def join():
        raise RuntimeError("callback outcome unknown")
    async def stop():
        raise ValueError("store close failed")
    app._member_mcp = SimpleNamespace(join=join)
    app.integration.stop = stop
    with pytest.raises(BaseExceptionGroup) as error:
        await app.stop()
    assert [str(item) for item in error.value.exceptions] == ["callback outcome unknown", "store close failed"]


@pytest.mark.parametrize("mode", ["fresh", "restore"])
async def test_participant_reaches_fresh_and_restore_before_connect(tmp_path, monkeypatch, mode):
    from test_application_startup import Fixture
    from test_application_restart import RestartFixture
    f = Fixture(tmp_path, monkeypatch) if mode == "fresh" else RestartFixture(tmp_path, monkeypatch)
    module = f.modules["meerkat_toolkit.application_runtime" if mode == "fresh" else "meerkat_toolkit.host_control"]
    name = "ToolkitApplicationRuntime" if mode == "fresh" else "ToolkitMobKitRuntime"
    original = getattr(module, name)
    participant = object()
    calls = []
    def options():
        calls.append("loaded-store participant")
        return {"member_mcp": participant}
    def construct(*args, member_mcp):
        assert member_mcp is participant
        assert calls == ["loaded-store participant"]
        calls.append("native constructor")
        return original(*args)
    f.app.member_mcp_runtime_options = options
    monkeypatch.setattr(module, name, construct)
    await f.owner.prepare(f.app)
    await f.owner.connect(f.config(), f.app)
    assert calls == ["loaded-store participant", "native constructor"]


async def test_participant_reaches_normal_replacement_constructor(replacement_fixture, monkeypatch):
    f = replacement_fixture
    import meerkat_toolkit.application_update as sdk_module
    original = sdk_module.ToolkitApplicationUpdateRuntime
    participant = object()
    f.app.member_mcp_runtime_options = lambda: {"member_mcp": participant}
    def construct(*args, member_mcp):
        assert member_mcp is participant
        return original(*args)
    monkeypatch.setattr(sdk_module, "ToolkitApplicationUpdateRuntime", construct)
    await f.owner.prepare(f.app)
    runtime = await f.owner.connect(f.config(), f.app)
    assert runtime is f.owner.runtime
