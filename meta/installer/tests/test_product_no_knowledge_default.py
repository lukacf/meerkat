"""Actual unchanged no-knowledge owners after the optional preparation delta.

The default Blueprint/compiler, original byte Install, claimed Prepare, installed
source readers and Start admission remain production code. Reused SDK/full-prefix,
inventory and completed Bootstrap specimens are explicitly inert. Application
spawn is stopped at its original boundary. No optional helper, native runtime,
Docker, HTTP, Git, cloud or provider executes; no readiness is established.
"""
from contextlib import asynccontextmanager
import json
from pathlib import Path

import pytest

from launchkit import compiler, deployment, deployment_worker as worker
from launchkit import product_installed_source as installed_source
from launchkit.blueprint import starter_blueprint
from launchkit.templates.storage_setup import project_lock
from test_destination_application_inputs import inputs_destination, install, prepare_original
from test_destination_application_factory import destination
from test_product_destination_install import (installation, destination_source,
    artifact_custody, product)
from test_product_destination_owner import no_destination_execution
from test_meta_toolkit_runtime_selection import no_product_execution
from test_product_installed_bootstrap import installed_bootstrap
from test_deployment_application_bootstrap import inputs


class ApplicationBoundary(RuntimeError):
    pass


def forbid_knowledge(monkeypatch):
    async def forbidden(*args, **kwargs):
        pytest.fail("No-knowledge source cannot enter optional preparation, Docker query or new knowledge state.")
    monkeypatch.setattr(worker.Supervisor, "prepare_knowledge", forbidden)
    monkeypatch.setattr(worker.Supervisor, "knowledge_preflight", forbidden)
    monkeypatch.setattr(worker.Supervisor, "query", forbidden)


def test_real_default_blueprint_compiler_has_no_elephant_inputs_connection_or_files():
    blueprint = starter_blueprint()
    assert blueprint.knowledge.mode == "none"
    assert all(row.tools.knowledge is False for row in blueprint.agent_classes)
    environment = compiler.environment_fields(blueprint)
    definition = compiler._runtime(blueprint)
    files = compiler.generate_files(blueprint.model_dump(mode="json"))
    assert definition["knowledge"]["mode"] == "none"
    assert all(row["key"] not in {"ELEPHANT_TOKEN", "ELEPHANT_SPACE_ID", "JWT_SECRET", "SURREALDB_PASS"}
        for row in environment)
    assert all(row["id"] != "elephant" for row in definition["mcpConnections"])
    assert all(not name.startswith("knowledge/") for name in files)
    assert "No knowledge service was selected." in files["README.md"]


async def test_original_selected_install_prepare_and_reopen_need_no_optional_knowledge_state(
        inputs_destination, monkeypatch):
    f = inputs_destination
    forbid_knowledge(monkeypatch)
    assert f.spec["knowledge"]["mode"] == "none"
    assert all(not name.startswith("knowledge/") for name in f.selected.files)
    owner, installer = await install(f)
    original = {name: (f.root / name).read_bytes() for name in f.selected.files}
    assert installer is owner.host._destination_installation_supervisor
    assert deployment.read_receipt(f.directory)["status"] == "stopped"
    assert not deployment.read_receipt(f.directory).get("knowledge_managed")
    assert not (f.root / "knowledge").exists()
    await prepare_original(f, owner, monkeypatch)
    async with f.service.lock:
        async with f.factory._operation(installed=True):
            source = f.factory._source
            assert source._installed_view.knowledge is None
            source.require_current()
            assert json.loads(f.factory._startup_pin.read())["kind"] == "fresh"
    assert original == {name: (f.root / name).read_bytes() for name in f.selected.files}
    assert len(f.inventory_calls) == 1 and f.service._application_preparations == {}
    assert f.held.supervisor.child is None and f.held.supervisor._child_admission is None
    assert f.factory._source is None and source._closed
    assert not (f.root / "knowledge").exists()
    assert not deployment.read_receipt(f.directory).get("knowledge_managed")
    with project_lock(f.root) as lease:
        lease.require_current()


async def test_original_selected_bootstrap_start_keeps_source_proof_and_closes_parent_lease_without_knowledge(
        installed_bootstrap, monkeypatch):
    f = installed_bootstrap
    forbid_knowledge(monkeypatch)
    definition = json.loads(f.selected.files["deployment.json"])
    assert definition["knowledge"]["mode"] == "none"
    original = {name: (f.selected.destination / name).read_bytes() for name in f.selected.files}
    opened, verified, spawned = [], [], []
    original_open = installed_source.open_installed_product_source
    original_verify = worker.Supervisor._verify_candidate_runtime

    @asynccontextmanager
    async def observe(*args, **kwargs):
        async with original_open(*args, **kwargs) as source:
            assert source is not None and source._installed_view.knowledge is None
            opened.append((source, kwargs["project_lease"]))
            yield source

    def verify(supervisor, files, *, source_custody=None, **options):
        assert source_custody is opened[-1][0] and not source_custody._closed
        result = original_verify(supervisor, files, source_custody=source_custody, **options)
        verified.append(result)
        return result

    async def boundary(*args, **options):
        assert len(verified) == 1 and all(source._closed and lease._closed for source, lease in opened)
        assert args[:3] == (str(f.selected.destination / ".venv/bin/python"), "-I", "-c")
        with project_lock(f.selected.destination) as lease:
            lease.require_current()
        spawned.append(args)
        raise ApplicationBoundary("Original selected application spawn stays inert.")

    monkeypatch.setattr(installed_source, "open_installed_product_source", observe)
    monkeypatch.setattr(worker.Supervisor, "_verify_candidate_runtime", verify)
    monkeypatch.setattr(worker, "require_available_console_port", lambda port: {"available": True})
    supervisor = worker.Supervisor(f.directory)
    supervisor._bootstrap_owner_source = f.selected.source
    monkeypatch.setattr(supervisor, "spawn_child", boundary)
    with pytest.raises(ApplicationBoundary, match="spawn stays inert"):
        await supervisor.start(definition, bootstrap_selection=f.pin)
    assert len(opened) == len(verified) == len(spawned) == 1
    assert supervisor.child is None and supervisor._child_admission is None
    assert original == {name: (f.selected.destination / name).read_bytes() for name in f.selected.files}
    assert not (f.selected.destination / "knowledge").exists()
    assert not deployment.read_receipt(f.directory).get("knowledge_managed")


@pytest.mark.parametrize("managed", [None, True])
async def test_source_less_no_knowledge_start_skips_optional_lane_even_with_saved_managed_flag(
        inputs, monkeypatch, managed):
    selected = inputs
    forbid_knowledge(monkeypatch)
    receipt = deployment.update_receipt(selected.directory, knowledge_managed=managed)
    definition = json.loads((selected.destination / "deployment.json").read_bytes())
    assert definition["knowledge"]["mode"] == "none"
    admission, _ = selected.admit("restore")
    before = (selected.destination / ".env").read_bytes()
    opened, spawned = [], []
    original_open = installed_source.open_installed_product_source

    @asynccontextmanager
    async def observe(*args, **kwargs):
        async with original_open(*args, **kwargs) as source:
            assert source is None
            opened.append(kwargs["project_lease"])
            yield source

    async def boundary(*args, **options):
        assert opened and all(lease._closed for lease in opened)
        with project_lock(selected.destination) as lease:
            lease.require_current()
        spawned.append(args)
        raise ApplicationBoundary("Original ordinary application spawn stays inert.")

    monkeypatch.setattr(installed_source, "open_installed_product_source", observe)
    monkeypatch.setattr(worker, "require_available_console_port", lambda port: {"available": True})
    supervisor = worker.Supervisor(selected.directory)
    monkeypatch.setattr(supervisor, "spawn_child", boundary)
    with pytest.raises(ApplicationBoundary, match="spawn stays inert"):
        await supervisor.start(definition, bootstrap_selection=admission)
    assert len(opened) == len(spawned) == 1
    assert supervisor.child is None and supervisor._child_admission is None
    assert not (selected.destination / "knowledge").exists()
    assert before == (selected.destination / ".env").read_bytes()
    assert deployment.read_receipt(selected.directory).get("knowledge_managed") is managed
    assert receipt["runtime_pid"] is None
