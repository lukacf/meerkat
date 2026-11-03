"""Trusted factory model selections through actual private input suppliers.

Install completion, source admission, policy and recovery readers remain real.
Only the existing package/SDK, subprocess and network edges are inert fixtures.
The selected TOML is a custody specimen, not native syntax or model entitlement.
No credentials are inspected, no native setup executes, and no default store is
selected. Platform substitution only exercises the Linux source contract.
"""
from copy import deepcopy
import hashlib
import json
from pathlib import Path
import sys
from types import SimpleNamespace

import pytest

from launchkit import deployment, deployment_worker, remote_product_destination_owner
from launchkit import destination_application_inputs as supplier
from launchkit import connect_application_startup as startup
from launchkit import forge_application_host as application_host
from launchkit import deployment_application_preparation as preparation
from launchkit.connect_installer_caller import CustodyError
from launchkit.destination_application_inputs import (
    create_destination_application_inputs, recover_destination_application_inputs, SOURCE_DIRECTORY)
from launchkit.local_application_factory import LocalApplicationFactory
from launchkit.local_application_preparation_inputs import LocalApplicationPreparationOwner
from launchkit.product_installed_source import open_installed_product_source
from launchkit.templates.storage_setup import project_lock
from test_local_application_factory import (
    local, retain, retaining, prepared, implementation_service, workspace, selected,
    product, no_product_execution, no_processes)
from test_destination_application_inputs import inputs_destination, select_caller, snapshot
from test_destination_application_factory import destination
from test_product_destination_install import installation, destination_source, artifact_custody
from test_product_destination_owner import no_destination_execution


def model_source(tmp_path, monkeypatch, name="original"):
    directory = tmp_path / ("model-owner-" + name)
    directory.mkdir(mode=0o700)
    config = directory / "host.toml"
    content = b'[model]\nselected = "inert-custody-specimen"\n'
    config.write_bytes(content)
    config.chmod(0o600)
    root = directory / "xdg"
    root.mkdir(mode=0o700)
    credentials = root / "opaque-credentials"
    credentials.write_bytes(b"SYNTHETIC_CREDENTIAL_NOT_TO_BE_READ")
    credentials.chmod(0o600)
    previous = startup._read
    def no_credentials(path, limit):
        assert Path(path) != credentials, "Selected root custody cannot read credential contents."
        return previous(path, limit)
    monkeypatch.setattr(startup, "_read", no_credentials)
    # These three feature guards exercise Linux selection. The actual global
    # platform and admitted Mac package/runtime fixture stay unchanged.
    for feature in (startup, application_host, preparation):
        monkeypatch.setattr(feature, "sys", SimpleNamespace(platform="linux"))
    return {"host_config": {"path": str(config), "sha256": hashlib.sha256(content).hexdigest()},
        "credential_config_root": str(root)}


def local_select(f, value):
    f.factory = LocalApplicationFactory(guide_store=f.store, guide_id=f.guide_id,
        runtime_target=f.target, acquisition_selection=f.acquisition,
        viewer_selection=f.viewer, model_configuration=value)
    f.host, f.owner = f.factory(), f.factory.preparation_owner
    f.service.application_setup_retention = f.host.retain_installation_inputs
    f.service.application_setup_selection = f.host._selected_startup
    f.service.application_setup_preparation = f.host.prepare_application
    return f.factory


async def destination_select(f, value):
    select_caller(f)
    async with f.service.lock:
        with project_lock(f.selected.destination) as lease:
            with remote_product_destination_owner.open_stopped_product_source(
                    f.selected.context, project_lease=lease, require_current=f.supplier_current,
                    owner_source=f.selected.source) as source:
                owner = await create_destination_application_inputs(source, deployment=f.service,
                    require_current=f.supplier_current, model_configuration=value)
                # The existing inert Supervisor wrapper joins this real
                # returned factory/host before its original install begins.
                f.factory, f.host = owner.factory, owner.host
                source.require_current()
                return owner


async def destination_install(f, value):
    owner = await destination_select(f, value)
    await owner.factory.install(settings=f.settings)
    f.held = owner.preparation_owner._inputs
    return owner


async def destination_recover(f, service, value):
    select_caller(f)
    async with service.lock:
        with project_lock(f.selected.destination) as lease:
            async with open_installed_product_source(f.directory, project_lease=lease,
                    require_current=f.supplier_current, owner_source=f.selected.source) as source:
                assert source is not None
                result = await recover_destination_application_inputs(source, deployment=service,
                    require_current=f.supplier_current, model_configuration=value)
                source.require_current()
                return result


@pytest.mark.parametrize("explicit", [False, True])
async def test_local_successful_install_retains_only_explicit_model_choice(local, tmp_path, monkeypatch, explicit):
    value = model_source(tmp_path, monkeypatch) if explicit else None
    original = deepcopy(value)
    factory = local_select(local, value)
    if value is not None:
        value["credential_config_root"] = "/caller-mutated-after-admission"
    inputs = await retain(local)
    choices = json.loads(local.owner._choices_pin.read())
    assert inputs.model_configuration == original
    assert inputs._require_model_configuration() == original
    assert factory._selected_model_configuration() == original
    assert choices.get("model_configuration") == original
    assert ("model_configuration" in choices) is explicit
    assert inputs.supervisor is local.host._installation_supervisor
    assert not local.calls and not inputs.startup_selection_path.exists()
    async with local.service.lock:
        chosen = local.host._selected_preparation(local.service, local.guide_id, local.directory.name)
    assert chosen.get("model_configuration") == original
    assert ("model_configuration" in chosen) is explicit


@pytest.mark.parametrize("change", ["nested-selection", "config-bytes", "same-byte-replacement", "root-replacement"])
def test_local_factory_refuses_original_selection_or_custody_drift(local, tmp_path, monkeypatch, change):
    value = model_source(tmp_path, monkeypatch)
    factory = local_select(local, value)
    config, root = Path(value["host_config"]["path"]), Path(value["credential_config_root"])
    if change == "nested-selection":
        factory.model_configuration["host_config"]["sha256"] = "f" * 64
    elif change == "config-bytes":
        config.write_bytes(config.read_bytes() + b"# changed\n")
    elif change == "same-byte-replacement":
        content = config.read_bytes()
        config.rename(config.with_name("original.toml"))
        config.write_bytes(content)
        config.chmod(0o600)
    else:
        root.rename(root.with_name("original-xdg"))
        root.mkdir(mode=0o700)
    with pytest.raises(CustodyError):
        factory.require_current()
    assert not local.setup_root.exists() and not local.calls


@pytest.mark.parametrize("explicit", [False, True])
async def test_destination_standard_supplier_retains_optional_selection(inputs_destination, tmp_path, monkeypatch, explicit):
    f = inputs_destination
    value = model_source(tmp_path, monkeypatch) if explicit else None
    owner = await destination_install(f, value)
    policy = json.loads(owner.preparation_owner._choices_pin.read())
    assert f.held.model_configuration == value
    assert f.held._require_model_configuration() == value
    assert owner._selected_model_configuration() == value
    assert policy.get("model_configuration") == value
    assert ("model_configuration" in policy) is explicit
    assert f.held.supervisor is owner.host._installation_supervisor
    assert f.held.supervisor._product_source_custody._closed
    assert not f.held.startup_selection_path.exists() and not f.inventory_calls
    assert deployment.read_receipt(f.directory)["phase"] == "configure"


async def test_destination_selection_refuses_caller_mutation_across_source_join(inputs_destination, tmp_path, monkeypatch):
    f = inputs_destination
    value = model_source(tmp_path, monkeypatch)
    original = supplier.await_product_validation
    async def changed_after_join(source, function, *args, **kwargs):
        result = await original(source, function, *args, **kwargs)
        if function.__name__ == "_inputs":
            value["credential_config_root"] = "/changed-during-source-join"
        return result
    monkeypatch.setattr(supplier, "await_product_validation", changed_after_join)
    with pytest.raises(CustodyError, match="model configuration choice changed"):
        await destination_select(f, value)
    assert not (f.selected.destination / SOURCE_DIRECTORY).exists()
    assert f.service.application_host_selection is None
    assert not f.inventory_calls


@pytest.mark.parametrize("selected", ["original", "omitted", "other-root"])
async def test_destination_recovery_requires_original_selected_policy_without_replay(
        inputs_destination, tmp_path, monkeypatch, selected):
    f = inputs_destination
    original = model_source(tmp_path, monkeypatch)
    owner = await destination_install(f, original)
    before = snapshot(f.root), snapshot(f.directory)
    value = original if selected == "original" else None
    if selected == "other-root":
        value = model_source(tmp_path, monkeypatch, "different")
    service = deployment.DeploymentService(f.selected.custody.home / ".local/state/meta-forge",
        f.guide, f.implementation)
    def no_retain(*args, **kwargs):
        pytest.fail("Recovery cannot publish new preparation defaults.")
    async def no_command(*args, **kwargs):
        pytest.fail("Recovery cannot run native setup or inventory.")
    monkeypatch.setattr(LocalApplicationPreparationOwner, "retain", no_retain)
    monkeypatch.setattr(deployment_worker.Supervisor, "command", no_command)
    if selected == "original":
        recovered = await destination_recover(f, service, value)
        assert recovered.preparation_owner._inputs.model_configuration == original
        assert recovered.preparation_owner._inputs._require_model_configuration() == original
        assert recovered.preparation_owner._choices_pin.read() == owner.preparation_owner._choices_pin.read()
        assert recovered.factory._startup_pin is None
        assert recovered.host._product_source_custody._closed
    else:
        with pytest.raises(CustodyError, match="policy or activation"):
            await destination_recover(f, service, value)
        assert service.application_host_selection is None
    assert before == (snapshot(f.root), snapshot(f.directory))
    assert not f.inventory_calls
