"""Real Stage A source/factory/byte Install with inert SDK and Linux birth facts.

These controls do not create a venv, execute pip, import a selected SDK, execute
an application/native binary, connect SSH or touch cloud. The existing poisoned
SDK fixture retains real source/artifact readers, actual byte Install and full
installed comparisons. Linux worker/command liveness is an explicit specimen.
"""
import asyncio
from copy import deepcopy
import hashlib
import io
import json
import os
from pathlib import Path
import sys
import threading
from uuid import uuid4

import pytest

from launchkit import deployment, destination_application_inputs, remote_destination as controller
from launchkit import remote_product_control as control
from launchkit.remote_interface_bundle import controller_bytes
from launchkit.runtime_candidates import ArtifactFile
from launchkit.templates.storage_setup import project_lock
from test_destination_application_inputs import inputs_destination
from test_destination_application_factory import destination
from test_product_destination_install import installation, destination_source, artifact_custody, product
from test_product_destination_owner import no_destination_execution, gcp_destination_source
from test_meta_toolkit_runtime_selection import no_product_execution
from test_selected_dependency_install import inert_wheel
from test_remote_deployment import service


def write(path, content):
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    path.write_bytes(content)
    path.chmod(0o600)
    return ArtifactFile(path, len(content), hashlib.sha256(content).hexdigest())


def selected_control(f):
    root = f.selected.custody.home / "separate-explicit-control-inputs"
    wheels, rows = [], []
    # Deliberately poisoned pure wheel bytes. This validates selection/integrity,
    # not real Pydantic distribution availability or platform qualification.
    for name in ("pydantic", "pydantic-core", "httpx"):
        specimen = inert_wheel(name, "1.0")
        artifact = write(root / specimen.filename, specimen.content)
        wheels.append(artifact)
        rows.append(name + "==1.0 --hash=sha256:" + artifact.sha256)
    requirements = write(root / "requirements.txt", ("\n".join(rows) + "\n").encode())
    python_tag = "cp" + str(sys.version_info.major) + str(sys.version_info.minor)
    return control.ProductControlSelection(runtime_target=f.selected.custody.plan.origin,
        requirements=requirements, wheels=tuple(wheels), python_tag=python_tag, abi_tag=python_tag,
        glibc_minimum=(2, 31), require_current=lambda: f.current)


@pytest.fixture
async def control_destination(inputs_destination, monkeypatch):
    f = inputs_destination
    selection = selected_control(f)
    selection.require_current()
    # The freezer remains real and reads only the trusted repository closure.
    bundle = control.freeze_product_control(selection)
    source = controller_bytes(product_control=bundle)
    root, host = controller._host(str(f.selected.destination))
    owner = {"protocol": 1, "deployment_id": f.selected.context["deployment_id"],
        "host": {key: host[key] for key in ("uid", "home", "destination")}, "product_control": True,
        "binding": {"guide_id": f.selected.context["guide_id"], "guide_version": f.selected.context["guide_version"],
            "build_id": f.selected.context["build_id"], "accepted_source_digest": f.selected.context["source_digest"],
            "package_sha256": f.selected.context["package_sha256"], "transferred_source_digest": "d" * 64,
            "controller_sha256": hashlib.sha256(source).hexdigest()}}
    folder = controller._directory(root)
    folder.mkdir(mode=0o700)
    write(folder / "controller.py", source)
    write(folder / "control-selection.json", control._json(bundle.manifest))
    write(folder / "control-bundle.zip", bundle.content)
    owner["snapshots"] = {name: hashlib.sha256((folder / name).read_bytes()).hexdigest()
        for name in ("controller.py", "control-selection.json", "control-bundle.zip")}
    controller._json_write(folder / "owner.json", owner, exclusive=True)
    files = control._bundle_files(bundle.content, bundle.manifest)
    for name, content in files.items():
        write(folder / "control-host" / name, content)
    operation = str(uuid4())
    birth = {"pid": 987654, "start_ticks": "123", "uid": os.getuid(), "boot_id": str(uuid4()), "state": "S"}
    command = {**birth, "pid": os.getpid()}
    controller._json_write(folder / "operation.json", {"operation_id": operation, "action": "install",
        "status": "installing", "phase": "application-source", "worker": birth, "command": command})
    controller._json_write(folder / "admission.json", {"deployment_id": owner["deployment_id"], "first_operation_id": operation})
    f.live = True
    def process(pid):
        if pid == birth["pid"]:
            return {**birth, "state": "S" if f.live else "Z"}
        if pid == os.getpid():
            return command
        return None
    monkeypatch.setattr(controller, "_process", process)
    monkeypatch.setattr(controller, "PRODUCT_CONTROL_MANIFEST", control._json(bundle.manifest), raising=False)
    monkeypatch.setattr(controller, "PRODUCT_CONTROL_ADAPTER_SOURCE", Path(control.__file__).read_bytes(), raising=False)
    monkeypatch.setattr(controller, "CONTROLLER_SOURCE", source, raising=False)
    original_create = destination_application_inputs.create_destination_application_inputs
    async def retain_actual(source, *, deployment, require_current):
        assert not (root / ".env").exists() and not (root / ".venv").exists()
        f.parent, f.loop, f.thread = asyncio.current_task(), asyncio.get_running_loop(), threading.get_ident()
        actual = await original_create(source, deployment=deployment, require_current=require_current)
        f.factory, f.service = actual.factory, deployment
        f.input_owner = actual
        assert f.service.guide is None and f.service.implementation is None
        return actual
    monkeypatch.setattr(destination_application_inputs, "create_destination_application_inputs", retain_actual)
    f.control_bundle, f.remote_owner, f.remote_operation = bundle, owner, operation
    f.remote_folder, f.remote_source = folder, source
    return f


async def actual_install(f):
    with controller._input_descriptor(control._json(f.settings), f.selected.destination) as descriptor:
        await control.run_initial_install(f.selected.destination, f.remote_operation, os.dup(descriptor))
    # Outer detached-worker completion is an explicit source specimen. The
    # original journal writer records it; no process execution is claimed.
    controller._progress(f.selected.destination, f.remote_owner, f.remote_operation,
        "complete", status="completed", cleanup_confirmed=True)
    f.live = False
    return controller._observe(f.selected.destination, f.remote_owner)


@pytest.mark.parametrize("installation", [None, "gcp"], indirect=True, ids=["linux", "preserved-gcp"])
async def test_stage_a_actual_factory_byte_install_stays_stopped_without_private_prewrites(control_destination, installation):
    f = control_destination
    assert f is installation
    original_context = deepcopy(f.selected.context)
    declared_target = json.loads(f.selected.files["deployment.json"])["target"]
    before = {name: (f.selected.destination / name).read_bytes() for name in f.selected.files}
    value = await actual_install(f)
    receipt = deployment.read_receipt(f.directory)
    assert receipt["status"] == "stopped" and receipt["phase"] == "configure"
    assert receipt["target"] == "local"
    assert f.selected.context == original_context
    if declared_target == "gcp":
        assert original_context["schema"] == 2
        assert original_context["source_disposition"]["target"] == declared_target
    else:
        assert declared_target == "linux"
    assert receipt["runtime_candidate"] == f.selected.origin
    assert value["scope"] == "byte-install-stopped" and value["status"] == "stopped" and not value["ready"]
    assert not value["service_owned"] and not value["service_observed"] and not value["retryable"]
    assert f.service.guide is None and f.service.implementation is None
    assert len(f.supervisors) == 2 and f.input_owner.preparation_owner._inputs is not None
    assert f.factory._source is None and f.factory._pending_scope is None
    assert not f.service.tasks and not f.service.processes and not f.service._application_preparations
    assert before == {name: (f.selected.destination / name).read_bytes() for name in f.selected.files}
    assert not f.input_owner.preparation_owner._inputs.inventory_attempt_path.exists()
    assert not (f.remote_folder / "remote-service.json").exists()
    with project_lock(f.selected.destination):
        pass


@pytest.mark.parametrize("same_uuid", [True, False], ids=["lost-reply-same-uuid", "different-uuid-cannot-reinstall"])
async def test_stage_a_original_install_uuid_reads_actual_completion_without_replay(control_destination, monkeypatch, same_uuid):
    f = control_destination
    await actual_install(f)
    before = ((f.directory / "receipt.json").read_bytes(), (f.selected.destination / ".env").read_bytes(),
        f.input_owner.preparation_owner._choices_pin.read(), len(f.supervisors), deepcopy(f.events))
    def forbidden(*args, **kwargs):
        pytest.fail("A repeated/lost Install reply attempted another original worker or Fresh retention.")
    monkeypatch.setattr(controller, "_spawn", forbidden)
    request = {"action": "install", "deployment_id": f.remote_owner["deployment_id"],
        "operation_id": f.remote_operation if same_uuid else str(uuid4()), "binding": f.remote_owner["binding"],
        "destination": str(f.selected.destination), "expected_host": f.remote_owner["host"], "settings": {}}
    result = controller.handle(request)
    assert "error" not in result and result["operation_id"] == f.remote_operation and result["status"] == "stopped"
    assert before == ((f.directory / "receipt.json").read_bytes(), (f.selected.destination / ".env").read_bytes(),
        f.input_owner.preparation_owner._choices_pin.read(), len(f.supervisors), f.events)
    public = control._json(result)
    assert f.settings["CONSOLE_LOGIN_PASSWORD"].encode() not in public and f.settings["OPENAI_API_KEY"].encode() not in public


async def test_stage_a_control_source_drift_refuses_actual_install_before_settings(control_destination):
    f = control_destination
    source = f.remote_folder / "control-host/launchkit/blueprint.py"
    source.write_bytes(source.read_bytes() + b"\n# private changed fixture\n")
    with controller._input_descriptor(control._json(f.settings), f.selected.destination) as descriptor:
        with pytest.raises(ValueError, match="trusted control input changed"):
            await control.run_initial_install(f.selected.destination, f.remote_operation, os.dup(descriptor))
    assert not (f.selected.destination / ".env").exists() and not f.directory.exists()
    assert not f.supervisors and not (f.remote_folder / "product-install-completed.json").exists()


async def test_stage_a_wrong_guest_abi_refuses_before_control_materialization(control_destination, monkeypatch):
    f = control_destination
    monkeypatch.setattr(control.platform, "machine", lambda: "aarch64")
    before = sorted(str(path) for path in f.remote_folder.rglob("*"))
    with pytest.raises(ValueError, match="interpreter/platform differs"):
        control.receive_bundle(vars(controller), f.selected.destination, f.remote_owner["deployment_id"],
            f.remote_owner["binding"], f.remote_owner["host"], f.remote_source, f.control_bundle.manifest,
            {"transfer_file": f.control_bundle.manifest["bundle"]}, io.BytesIO(f.control_bundle.content))
    assert before == sorted(str(path) for path in f.remote_folder.rglob("*"))
    assert not (f.selected.destination / ".env").exists() and not f.directory.exists()


async def test_stage_a_default_public_product_guard_remains_with_no_transport_or_owned_state(service):
    from launchkit.projects import DraftConflict
    service.implementation.runtime_candidate = {"source": "meta-toolkit-product", "selection": "runtime-candidate.json",
        "assembly_sha256": "c" * 64}
    with pytest.raises(DraftConflict, match="Remote installation is not supported"):
        await service.accepted(service.guide.identity, 4, service.implementation.identity)
    assert not service.root.exists() and not service.transport.calls and not service.tasks
