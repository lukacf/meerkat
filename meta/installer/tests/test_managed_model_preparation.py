"""Managed model selection through original Prepare, with inert native edges.

The maintained source/host/bridge fixtures create actual accepted source and
parse synthetic exited inventory bytes. No SDK, gateway, provider, credential
store, native process or installed runtime is exercised or qualified here.
"""
import asyncio
from copy import deepcopy
from dataclasses import replace
import json
import uuid

import pytest

from launchkit import (
    connect_application_startup as startup,
    deployment_application_bootstrap as bootstrap,
    deployment_application_preparation as bridge,
    forge_application_host as host,
)
from launchkit.connect_installer_caller import CustodyError
from launchkit.projects import DraftConflict
from launchkit.templates.storage_setup import project_lock
import test_forge_application_host as source_fixture
from test_deployment_application_preparation import (
    prepared, implementation_service, workspace, product, no_product_execution,
    no_processes,
)
from test_forge_prepare_setup import joined, prepare, status


class _FeatureSystem:
    """Delegate original sys semantics except this inert feature platform."""

    def __init__(self, original):
        object.__setattr__(self, "_original", original)
        object.__setattr__(self, "platform", "linux")

    def __getattr__(self, name):
        return getattr(self._original, name)

    def __setattr__(self, name, value):
        if name == "platform":
            object.__setattr__(self, name, value)
        else:
            setattr(self._original, name, value)


@pytest.fixture
def selected(tmp_path, product, monkeypatch, request):
    # Use the same source producer before the original host authenticates it.
    # There is no alternate host or post-admission blueprint mutation.
    if getattr(request, "param", False):
        original = source_fixture.setup_spec

        def managed_spec():
            spec = original()
            spec["agent_classes"][0]["model_auth"] = "managed_host"
            return spec

        monkeypatch.setattr(source_fixture, "setup_spec", managed_spec)
    return source_fixture.selected.__wrapped__(tmp_path, product)


def select_model(f, monkeypatch):
    private = f.root.parent / "inert-original-model-selection"
    private.mkdir(mode=0o700)
    config = source_fixture.put(private / "host.toml",
        b'# Inert source-only configuration, not a usable provider binding.\n')
    root = private / "credential-root"
    root.mkdir(mode=0o700)
    value = {"host_config": config.pin(), "credential_config_root": str(root)}
    # Isolate the Linux feature branches without changing Python's global
    # platform or the authenticated original deployment target. All other sys
    # attributes, including exception and argv, keep their original semantics.
    feature_sys = _FeatureSystem(bridge.sys)
    for module in (startup, host, bridge, bootstrap):
        monkeypatch.setattr(module, "sys", feature_sys)
    monkeypatch.delenv("XDG_CONFIG_HOME", raising=False)
    return value, config, root


async def test_ordinary_prepare_keeps_original_envelope_without_model_selection(joined):
    f = joined
    chosen = f.host._selected_preparation(f.service, f.guide_id, f.receipt["deployment_id"])
    assert "model_configuration" not in chosen
    assert f.inputs.model_configuration is None
    result = await prepare(f)
    assert result["action_request"]["outcome"] == "returned"
    assert "model_configuration" not in json.loads(f.selection_path.read_bytes())
    assert len(f.calls) == 1 and not f.service.launched


@pytest.mark.parametrize("selected", [True], indirect=True)
async def test_managed_prepare_retains_exact_selection_without_process_environment(joined, monkeypatch):
    f = joined
    value, config, root = select_model(f, monkeypatch)
    original = deepcopy(value)
    f.inputs = replace(f.inputs, model_configuration=value)
    # Native persistence may add children; Forge owns directory metadata only.
    f.after_command = lambda: (root / "native-created-child").mkdir(mode=0o700)
    ready = await status(f)
    assert ready["available"] and ready["action"] == "prepare-setup" and not f.calls
    result = await prepare(f)
    fresh = json.loads(f.selection_path.read_bytes())
    assert fresh["model_configuration"] == original == value
    assert result["action_request"]["outcome"] == "returned"
    assert config.read().startswith(b"# Inert source-only configuration")
    assert (root / "native-created-child").is_dir()
    assert len(f.calls) == 1 and not f.service.launched
    assert not (f.directory / "application-bootstrap-entry.json").exists()
    assert f.service._application_preparations == {}
    with project_lock(f.destination):
        pass


@pytest.mark.parametrize("selected", [True], indirect=True)
@pytest.mark.parametrize("entry", ["host", "bridge"])
async def test_managed_without_original_selection_never_offers_or_enters_prepare(joined, entry):
    f = joined
    ready = await status(f)
    assert ready["available"] is False and ready["reason"] == "source_unavailable"
    assert "model configuration" in ready["detail"]
    with pytest.raises((CustodyError, DraftConflict), match="model configuration"):
        if entry == "host":
            await prepare(f)
        else:
            await f.run()
    assert not f.calls and not f.selection_path.exists() and not f.plan_path.exists()
    assert f.service._application_preparations == {} and not f.service.launched


@pytest.mark.parametrize("selected", [True], indirect=True)
@pytest.mark.parametrize("change", ["wire", "file", "root-mode", "non-linux"])
async def test_retained_model_input_drift_cannot_offer_or_enter_inventory(joined, monkeypatch, change):
    f = joined
    value, config, root = select_model(f, monkeypatch)
    f.inputs = replace(f.inputs, model_configuration=value)
    if change == "wire":
        f.inputs.model_configuration["host_config"]["sha256"] = "0" * 64
    elif change == "file":
        config.path.write_bytes(config.read() + b"# Changed original file.\n")
    elif change == "root-mode":
        root.chmod(0o750)
    else:
        monkeypatch.setattr(bridge.sys, "platform", "darwin")
    ready = await status(f)
    assert ready["available"] is False and ready["reason"] == "source_unavailable"
    with pytest.raises((CustodyError, DraftConflict)):
        await prepare(f)
    assert not f.calls and not f.selection_path.exists() and not f.plan_path.exists()
    assert f.service._application_preparations == {}


@pytest.mark.parametrize("selected", [True], indirect=True)
@pytest.mark.parametrize("change", ["wire", "file", "root-mode", "root-replacement"])
async def test_model_custody_drift_after_original_command_cannot_publish_fresh(prepared, monkeypatch, change):
    f = prepared
    value, config, root = select_model(f, monkeypatch)

    def mutate():
        if change == "wire":
            value["host_config"]["sha256"] = "0" * 64
        elif change == "file":
            config.path.write_bytes(config.read() + b"# Changed after original await.\n")
        elif change == "root-mode":
            root.chmod(0o750)
        else:
            root.rename(root.with_name("original-retained-root"))
            root.mkdir(mode=0o700)

    f.after_command = mutate
    with pytest.raises(CustodyError):
        await f.run(model_configuration=value)
    assert len(f.calls) == 1 and not f.selection_path.exists() and not f.plan_path.exists()
    assert f.service._application_preparations == {} and not f.service.launched
    with project_lock(f.destination):
        pass


@pytest.mark.parametrize("selected", [True], indirect=True)
async def test_managed_prepare_cancellation_keeps_original_action_pending_and_cleans(joined, monkeypatch):
    f = joined
    value, _, _ = select_model(f, monkeypatch)
    f.inputs = replace(f.inputs, model_configuration=value)
    entered = asyncio.Event()

    async def held(argv, *args, **kwargs):
        f.calls.append(tuple(argv))
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
    assert repeated["action_request"]["phase"] == "pending"
    assert len(f.calls) == 1 and f.supervisor.stop.is_set()
    assert not f.selection_path.exists() and not f.plan_path.exists()
    assert f.service._application_preparations == {} and not f.service.launched
    with project_lock(f.destination):
        pass
