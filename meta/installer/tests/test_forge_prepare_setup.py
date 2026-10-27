"""Original action/host/bridge joins with inert retained inventory bytes only.

The existing product fixture forbids processes and SDK imports. These controls
do not prepare a native owner, finalize Fresh, select a real host or run agents.
"""
import asyncio
import ast
import hashlib
import json
from pathlib import Path
import uuid

import pytest

from launchkit import deployment
from launchkit.connect_installer_caller import CustodyError, ExactFile
from launchkit.forge_application_host import ApplicationPreparationInputs, ForgeApplicationHost
from launchkit.projects import DraftConflict
from test_deployment_application_preparation import (prepared, implementation_service,
    workspace, selected, product, no_product_execution, no_processes)
from test_forge_application_host import put, startup
from test_forge_application_host_server import api, select


@pytest.fixture
def joined(prepared):
    f = prepared
    f.inputs = ApplicationPreparationInputs(**{name: value for name, value in f.kwargs.items()
        if name != "application_host"})
    f.provider_calls = []
    def inputs(guide, installation):
        assert (guide, installation) == (f.guide_id, f.receipt["deployment_id"])
        f.provider_calls.append((guide, installation))
        return f.inputs
    def original_startup(guide, installation):
        assert (guide, installation) == (f.guide_id, f.receipt["deployment_id"])
        return ExactFile(f.selection_path, hashlib.sha256(f.selection_path.read_bytes()).hexdigest())
    f.host._preparation_inputs = inputs
    f.host._startup_selection = original_startup
    f.service.application_setup_selection = f.host._selected_startup
    f.service.application_setup_preparation = f.host.prepare_application
    return f


async def prepare(f, request_id=None, **extra):
    return await f.service.action(f.guide_id, f.receipt["deployment_id"], "prepare-setup",
        action_request_id=request_id or str(uuid.uuid4()), **extra)


async def status(f):
    return await f.host.setup_status(f.service, f.guide_id, f.receipt["deployment_id"])


async def test_actual_action_retains_original_envelope_then_existing_finish_owner(joined, monkeypatch):
    f = joined
    before = deployment.read_receipt(f.directory)
    projection = await status(f)
    assert projection["available"] and projection["action"] == "prepare-setup"
    assert not f.calls and deployment.read_receipt(f.directory) == before
    assert not f.selection_path.exists() and not f.plan_path.exists()
    request_id = str(uuid.uuid4())
    result = await asyncio.wait_for(prepare(f, request_id), 10)
    assert result["status"] == "stopped" and result["phase"] == "configure"
    assert result["action_request"]["phase"] == "settled"
    assert result["action_request"]["outcome"] == "returned"
    assert len(f.calls) == 1 and not f.service.launched
    original = f.host._startup_selection(f.guide_id, f.receipt["deployment_id"])
    assert original.path == f.selection_path and original.read() == f.selection_path.read_bytes()
    ready = await status(f)
    assert ready["available"] and ready["kind"] == "fresh" and "action" not in ready
    assert not (f.directory / "application-bootstrap-entry.json").exists()
    for _ in range(2):
        repeated = await prepare(f, request_id)
        assert repeated["action_request"] == result["action_request"]
    assert len(f.calls) == 1
    with pytest.raises(CustodyError, match="already retained"):
        await prepare(f)
    assert len(f.calls) == 1
    starts = []
    async def locked(guide, installation, *, application_startup):
        assert f.service.lock.locked()
        starts.append(application_startup)
        return {**before, "status": "queued"}
    monkeypatch.setattr(f.service, "_start_application_locked", locked)
    finished = await f.service.action(f.guide_id, f.receipt["deployment_id"], "finish-setup",
        action_request_id=str(uuid.uuid4()))
    assert finished["status"] == "queued" and starts == [json.loads(original.read())]


@pytest.mark.parametrize("change", ["active", "source", "public-parent", "typed-input", "template", "child"])
async def test_ineligible_readback_and_action_never_enter_inventory(joined, change):
    f = joined
    if change == "active":
        deployment.update_receipt(f.directory, status="running", process_live=True)
    elif change == "source":
        current = deployment.read_receipt(f.directory)["runtime_candidate"]
        current["assembly"]["sha256"] = "0" * 64
        deployment.update_receipt(f.directory, runtime_candidate=current)
    elif change == "public-parent":
        f.private.chmod(0o755)
    elif change == "typed-input":
        f.host._preparation_inputs = lambda *args: dict(f.kwargs)
    elif change == "template":
        f.template.path.write_bytes(b'{"changed":true}\n')
    else:
        f.supervisor.child = object()
    value = await status(f)
    assert value["available"] is False and value["reason"] == "source_unavailable"
    with pytest.raises((CustodyError, DraftConflict, ValueError)):
        await prepare(f)
    assert not f.calls and not f.selection_path.exists()


@pytest.mark.parametrize("change", ["unreadable", "foreign"])
async def test_existing_output_cannot_be_reinterpreted_as_prepare_readiness(joined, change):
    f = joined
    if change == "unreadable":
        f.selection_path.write_bytes(b"unreadable retained bytes")
        f.selection_path.chmod(0o600)
    else:
        await prepare(f)
        value = json.loads(f.selection_path.read_bytes())
        value["source_policy"] = put(f.private / "unselected-policy.json", f.policy.read()).pin()
        copied = put(f.private / "unselected-fresh.json", value)
        f.host._startup_selection = lambda *args: copied
    calls = len(f.calls)
    value = await status(f)
    assert value["available"] is False and value["reason"] == "source_unavailable"
    with pytest.raises(CustodyError):
        await prepare(f)
    assert len(f.calls) == calls


async def test_original_startup_supplier_must_select_exact_written_path(joined):
    f = joined
    def wrong(*args):
        if not f.selection_path.exists():
            raise FileNotFoundError(f.selection_path)
        return put(f.private / "foreign-selection.json", f.selection_path.read_bytes())
    f.host._startup_selection = wrong
    with pytest.raises(CustodyError, match="exact prepared Fresh envelope"):
        await prepare(f)
    assert f.selection_path.exists() and len(f.calls) == 1
    assert (await status(f))["available"] is True
    assert not (f.directory / "application-bootstrap-entry.json").exists()


async def test_retained_fresh_is_recovery_and_actual_restore_still_uses_original_supplier(joined):
    f = joined
    await prepare(f)
    put(f.directory / "application-bootstrap-entry.json", {"original_retained_attempt": True})
    value = await status(f)
    assert not value["available"] and value["reason"] == "previous_attempt"
    before = len(f.calls)
    with pytest.raises((CustodyError, DraftConflict)):
        await prepare(f)
    assert len(f.calls) == before
    startup(f, "restore")
    restored = await status(f)
    assert restored["available"] and restored["kind"] == "restore" and "action" not in restored


@pytest.mark.parametrize("kind", ["fresh", "restore"])
async def test_retained_startup_does_not_consult_unavailable_preparation_inputs(joined, monkeypatch, kind):
    f = joined
    if kind == "fresh":
        await prepare(f)
        expected = json.loads(f.selection_path.read_bytes())
    else:
        expected = startup(f, "restore")
    def obsolete(*args):
        pytest.fail("A retained startup selection must not read preparation-only inputs.")
    f.host._preparation_inputs = obsolete
    before = len(f.calls)
    ready = await status(f)
    assert ready["available"] and ready["kind"] == kind and "action" not in ready
    starts = []
    async def locked(guide, installation, *, application_startup):
        assert f.service.lock.locked()
        starts.append(application_startup)
        return {**f.receipt, "status": "queued"}
    monkeypatch.setattr(f.service, "_start_application_locked", locked)
    finished = await f.service.action(f.guide_id, f.receipt["deployment_id"], "finish-setup",
        action_request_id=str(uuid.uuid4()))
    assert finished["status"] == "queued" and starts == [expected] and len(f.calls) == before


async def test_absent_supplier_cannot_hide_existing_unreadable_planned_output(joined):
    f = joined
    put(f.selection_path, b"unreadable retained bytes")
    f.host._startup_selection = lambda *args: None
    value = await status(f)
    assert not value["available"] and value["reason"] == "source_unavailable" and not f.calls
    with pytest.raises(CustodyError):
        await prepare(f)
    assert not f.calls


async def test_missing_private_settings_remain_distinct_from_host_capability(joined):
    f = joined
    saved = (f.destination / ".env").read_bytes()
    (f.destination / ".env").write_bytes(b"OPENAI_API_KEY=inert-not-a-provider-key\n")
    value = await status(f)
    assert not value["available"] and value["reason"] == "private_settings_missing"
    assert value["missing_fields"] and not f.calls
    (f.destination / ".env").write_bytes(saved)
    assert (await status(f))["action"] == "prepare-setup"


async def test_readonly_capability_cannot_enter_the_preparation_helper(joined, monkeypatch):
    from launchkit import deployment_application_preparation
    f = joined
    async def forbidden(*args, **kwargs):
        pytest.fail("A setup GET cannot enter the preparation helper.")
    monkeypatch.setattr(deployment_application_preparation, "prepare_application", forbidden)
    before = deployment.read_receipt(f.directory)
    result = await status(f)
    assert result["action"] == "prepare-setup" and not f.calls
    assert deployment.read_receipt(f.directory) == before
    assert f.inputs.supervisor is f.supervisor and not f.selection_path.exists()


async def test_startup_supplier_cannot_publish_readiness_after_owner_changes(joined):
    f = joined
    def changed(*args):
        content = f.selection_path.read_bytes()
        deployment.update_receipt(f.directory, status="running", process_live=True)
        return ExactFile(f.selection_path, hashlib.sha256(content).hexdigest())
    f.host._startup_selection = changed
    with pytest.raises(DraftConflict, match="stopped"):
        await prepare(f)
    assert len(f.calls) == 1 and f.selection_path.exists()
    assert not (f.directory / "application-bootstrap-entry.json").exists()


async def test_cancelled_action_stays_pending_and_exact_uuid_never_reenters(joined, monkeypatch):
    f = joined
    entered = asyncio.Event()
    async def held(*args, **kwargs):
        f.calls.append(("inert-held-command",))
        entered.set()
        await asyncio.Event().wait()
    monkeypatch.setattr(f.supervisor, "command", held)
    request_id = str(uuid.uuid4())
    task = asyncio.create_task(prepare(f, request_id))
    await asyncio.wait_for(entered.wait(), 10)
    task.cancel()
    with pytest.raises(asyncio.CancelledError):
        await task
    repeated = await prepare(f, request_id)
    assert repeated["action_request"]["phase"] == "pending" and len(f.calls) == 1
    assert not f.selection_path.exists() and not f.service.launched


@pytest.mark.parametrize("extra", [{"action_request_id": None}, {"update_id": "foreign-update"}])
async def test_prepare_requires_original_action_uuid_without_update(joined, extra):
    f = joined
    with pytest.raises(ValueError, match="UUID"):
        await f.service.action(f.guide_id, f.receipt["deployment_id"], "prepare-setup", **extra)
    assert not f.calls


def test_server_selection_retains_original_preparation_callback_and_http_is_id_only(api):
    select(api)
    assert api.deployment.application_setup_preparation == api.selected.host.prepare_application
    request_id = str(uuid.uuid4())
    body = {"deployment_id": "original-installation", "action": "prepare-setup",
        "action_request_id": request_id}
    assert api.client.post("/api/guide/original-guide/deployment/action", json=body).status_code == 200
    assert api.calls == [("action", ("original-guide", "original-installation", "prepare-setup"),
        {"action_request_id": request_id})]
    api.calls.clear()
    for field in ("authorities", "startup_selection_path", "activation_template", "supervisor"):
        assert api.client.post("/api/guide/original-guide/deployment/action",
            json={**body, field: "browser-selected"}).status_code == 422
    assert api.calls == []
    api.deployment.application_setup_preparation = lambda *args: None
    with pytest.raises(DraftConflict, match="callbacks changed"):
        select(api)


@pytest.mark.parametrize("invalid", ["no-startup", "async", "wrong-arity"])
def test_constructor_rejects_incomplete_or_async_supplier_before_invoking_it(selected, invalid):
    options = {"startup_selection": lambda *args: None}
    if invalid == "no-startup":
        options = {}
        supplier = lambda *args: pytest.fail("An incomplete supplier must not run.")
    elif invalid == "async":
        async def supplier(*args):
            pytest.fail("An async supplier must not run.")
    else:
        def supplier():
            pytest.fail("A wrong-arity supplier must not run.")
    with pytest.raises(CustodyError):
        ForgeApplicationHost(guide_id=selected.host.guide_id, runtime_target=selected.target,
            acquisition_selection=selected.acquisition, viewer_selection=selected.viewer,
            require_current=lambda: None, preparation_inputs=supplier, **options)


def test_stock_binding_successor_changes_only_optional_provider_forwarding():
    artifacts = Path(__file__).resolve().parent / "fixtures/stock_host_binding"
    original = artifacts / "original.py"
    successor = artifacts / "successor.py"
    before, after = ast.parse(original.read_text()), ast.parse(successor.read_text())
    builder = next(node for node in after.body if isinstance(node, ast.FunctionDef) and node.name == "build_stock_host")
    assert builder.args.kwonlyargs[-1].arg == "preparation_inputs"
    assert isinstance(builder.args.kw_defaults[-1], ast.Constant) and builder.args.kw_defaults[-1].value is None
    constructor = builder.body[-1].value
    assert isinstance(constructor, ast.Call) and constructor.func.id == "ForgeApplicationHost"
    assert constructor.keywords[-1].arg == "preparation_inputs" and constructor.keywords[-1].value.id == "preparation_inputs"
    builder.args.kwonlyargs.pop()
    builder.args.kw_defaults.pop()
    constructor.keywords.pop()
    assert ast.dump(after) == ast.dump(before), "Original source/currentness and source pins must remain exact."
