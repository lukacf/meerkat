"""Actual completed Install ownership across later live proposal changes.

The reused completion and inventory boundaries retain synthetic exited bytes.
No SDK, provider, gateway or native setup executes or qualifies here. The real
GuideStore and installation files belong only to the disposable fixture.
"""
from copy import deepcopy

import pytest

from launchkit import deployment, deployment_worker
from launchkit.connect_installer_caller import CustodyError
from launchkit.implementation_workspace import ImplementationWorkspace
from test_local_application_factory import (local, retain, prepare, source_bytes,
    retaining, prepared, implementation_service, workspace, selected, product,
    no_product_execution, no_processes)


def revise_guide(f):
    state = f.store.get(f.guide_id)
    state["proposal"] = deepcopy(state["proposal"])
    state["proposal"]["project"]["name"] += " revised after Install"
    return f.store.save(state, state["version"])


async def installed_with_draft(f, monkeypatch):
    f.select()
    await retain(f)
    written = await prepare(f, monkeypatch)
    selected_pin = f.owner.startup_selection(f.guide_id, f.directory.name)
    assert selected_pin == written
    assert f.factory() is f.host
    assert f.owner.preparation_inputs(f.guide_id, f.directory.name) is f.inputs
    assert len(f.calls) == 1
    return selected_pin


async def test_completed_install_preserves_original_setup_and_rejects_changed_build(local, monkeypatch, tmp_path):
    f = local
    original_pin = await installed_with_draft(f, monkeypatch)
    original_host, original_inputs, original_supervisor = f.host, f.inputs, f.inputs.supervisor
    before = (source_bytes(f.root), source_bytes(f.setup_root),
        (f.directory / "receipt.json").read_bytes(), (f.destination / ".env").read_bytes())
    newer = revise_guide(f)
    assert newer["version"] > f.saved["version"]
    assert newer["proposal"] != f.spec

    def no_constructor(*args, **kwargs):
        pytest.fail("A live draft edit cannot replace the retained installation Supervisor.")

    async def no_inventory(*args, **kwargs):
        pytest.fail("Read-only setup cannot replay inventory after a live draft edit.")

    monkeypatch.setattr(deployment_worker.Supervisor, "__init__", no_constructor)
    monkeypatch.setattr(original_supervisor, "command", no_inventory)
    for _ in range(2):
        assert f.factory() is original_host
        original_host.require_current()
        assert f.owner.preparation_inputs(f.guide_id, f.directory.name) is original_inputs
        assert f.owner.startup_selection(f.guide_id, f.directory.name) is original_pin
        assert original_host._installation_supervisor is original_supervisor
        status = await original_host.setup_status(f.service, f.guide_id, f.directory.name)
        assert status["available"] and status["kind"] == "fresh"
        assert status["scope"] == "selected-stopped-application-inputs"
        assert status["deployment_id"] == f.directory.name
        assert status["label"] == "Finish application setup"
        assert "action" not in status
    assert before == (source_bytes(f.root), source_bytes(f.setup_root),
        (f.directory / "receipt.json").read_bytes(), (f.destination / ".env").read_bytes())
    assert len(f.calls) == 1

    # The original installed owner stays usable, but cannot supply its protected
    # source to a genuinely changed new Build owned by the current proposal.
    changed = ImplementationWorkspace(tmp_path / "changed-build", newer["proposal"])
    changed.seed(runtime_target=f.target)
    unadopted = source_bytes(changed.project)
    with pytest.raises(CustodyError, match="workspace differs from the host-selected application"):
        original_host.prepare_workspace(f.guide_id, "new-changed-build", changed)
    assert source_bytes(changed.project) == unadopted
    assert f.owner.startup_selection(f.guide_id, f.directory.name) is original_pin
    assert before == (source_bytes(f.root), source_bytes(f.setup_root),
        (f.directory / "receipt.json").read_bytes(), (f.destination / ".env").read_bytes())


@pytest.mark.parametrize("change", ["proposal", "running-operation"])
async def test_uninstalled_owner_still_refuses_live_guide_drift(local, change):
    f = local
    f.select()
    assert f.host._retained_preparation_inputs is None
    assert f.host._installation_supervisor is None and not f.setup_root.exists()
    before = source_bytes(f.root), (f.directory / "receipt.json").read_bytes()
    if change == "proposal":
        revise_guide(f)
        expected = "current local guide differs"
    else:
        f.store.save_operation(f.guide_id, {"status": "running"})
        expected = "unavailable or still changing"
    for read in (f.factory, f.host.require_current):
        with pytest.raises(CustodyError, match=expected):
            read()
    assert not f.setup_root.exists() and not f.calls
    assert f.host._retained_preparation_inputs is None and f.host._installation_supervisor is None
    assert before == (source_bytes(f.root), (f.directory / "receipt.json").read_bytes())


@pytest.mark.parametrize("change", ["policy", "viewer", "receipt"])
async def test_installed_owner_never_adopts_changed_source_or_installation(local, monkeypatch, change):
    f = local
    original_pin = await installed_with_draft(f, monkeypatch)
    revise_guide(f)
    assert f.factory() is f.host
    assert f.owner.startup_selection(f.guide_id, f.directory.name) is original_pin
    if change == "receipt":
        deployment.update_receipt(f.directory, package_sha256="f" * 64)
        expected = "installed application source association changed"
    else:
        pin = {"policy": f.policy, "viewer": f.viewer}[change]
        pin.path.write_bytes(pin.read() + b" ")
        expected = "selected bytes"
    changed = (source_bytes(f.root), source_bytes(f.setup_root),
        (f.directory / "receipt.json").read_bytes(), (f.destination / ".env").read_bytes())
    for read in (f.factory,
            lambda: f.owner.preparation_inputs(f.guide_id, f.directory.name),
            lambda: f.owner.startup_selection(f.guide_id, f.directory.name)):
        with pytest.raises(CustodyError, match=expected):
            read()
    assert changed == (source_bytes(f.root), source_bytes(f.setup_root),
        (f.directory / "receipt.json").read_bytes(), (f.destination / ".env").read_bytes())
    assert f.owner._fresh_pin is original_pin and len(f.calls) == 1
