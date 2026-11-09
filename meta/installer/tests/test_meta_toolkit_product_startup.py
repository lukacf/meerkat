"""Genuine producer modes in inert admitted update/child custody specimens."""
import asyncio
from copy import deepcopy
import json
import os
from pathlib import Path
import threading
import tomllib
from types import SimpleNamespace

import pytest

from launchkit import deployment_application_startup as startup, deployment_updates as updates
from launchkit import deployment_worker as worker, runtime_selection as selection
from launchkit.connect_installer_caller import CustodyError, ExactFile, ProductGatewayFile
from launchkit.templates import install_candidate as adapter
from launchkit.templates.storage_setup import project_lock
from test_deployment_application_startup import Inputs, write
from test_meta_toolkit_runtime_update import product_crossing
from test_meta_toolkit_runtime_selection import product, no_product_execution, json_bytes
from test_candidate_deployment_updates import installation, packed


@pytest.fixture
def product_inputs(product_crossing, tmp_path):
    _, target, previous, candidate, _, report = product_crossing
    deployment = json.loads(candidate["deployment.json"][0])
    policy = {"provider_id": "application", "policy_id": "inert-selected-tools", "revision": 1,
        "members": [{"member_identity": row["identity"], "grants": []} for row in deployment["agents"]]}
    policy_bytes = json_bytes(policy)
    previous["application/tool-policy.json"] = candidate["application/tool-policy.json"] = (policy_bytes, 0o644)
    directory, destination = installation(tmp_path, product_crossing)
    receipt = updates.read_receipt(directory)
    parent = tmp_path / receipt["guide_id"]
    parent.mkdir(mode=0o700)
    relocated = parent / receipt["deployment_id"]
    directory.rename(relocated)
    directory = relocated
    staged = updates.stage(directory, packed(candidate), report, runtime_target=target)
    assert not staged["update"]["blockers"]
    f = Inputs(directory, destination, staged["update"]["update_id"])
    definition = candidate["config/mob.toml"][0].decode()
    mob_id = tomllib.loads(definition)["mob"]["id"]
    rows = [{"identity": row["identity"], "profile": row["profile"]} for row in deployment["agents"]]
    members = [{"member": {"mob_id": mob_id, "role": row["profile"],
        "member": "inert-native-" + str(index)}} for index, row in enumerate(rows)]
    native_policy = deepcopy(policy)
    for row, member in zip(native_policy["members"], members):
        row["member_identity"] = member["member"]["member"]
    native_policy["source"] = {"source_id": report["build_id"], "source_digest": "sha256:" + updates.digest(policy_bytes)}
    application = f.plan_value["application"]
    application.update(definition={"format": "toml", "text": definition}, roster=rows,
        members=members, helper=members[0], tool_policy=native_policy)
    f.repin_application(application)
    artifacts = selection.selected_runtime_artifacts(target)
    enclosing = tmp_path / "original selected product"
    enclosing.mkdir(mode=0o700)
    gateway = adapter.install_meta_toolkit_prefix(artifacts, deployment_root=enclosing, require_current=lambda: None)
    f.gateway = ExactFile(gateway, artifacts.candidate["binary"]["sha256"])
    f.value["gateway"] = f.gateway.pin()
    return f, target, artifacts


async def test_original_admission_and_child_factory_retain_exact_product_gateway(product_inputs):
    f, target, artifacts = product_inputs
    admission = f.admission()
    selected = f.selected(admission)
    _, observed = startup.selected_inputs(f.value, directory=f.directory, binding=selected["binding"],
        installation=selected["installation"], retain_observation=True)
    assert type(observed.gateway) is ProductGatewayFile
    assert observed.gateway.pin() == f.gateway.pin()
    assert set(observed.gateway.pin()) == {"path", "sha256"}
    assert observed.gateway.path.stat().st_mode & 0o777 == 0o755
    observed.require_current()
    updates.activate(f.directory, f.identity, expected_binding=selected["binding"],
        expected_installation=selected["installation"])
    attempt = startup.retain_attempt(f.directory, admission, selected)
    factory = startup.ReplacementFactory(f.directory, "update", admission, attempt)
    with project_lock(f.destination) as lease:
        async with factory(lease) as owner:
            assert type(owner.gateway) is ProductGatewayFile
            assert owner.gateway.pin() == f.gateway.pin()
            assert owner.gateway.artifacts.selection == target
            owner._read_inputs()
            owner._require_current()
        assert factory.source._closed


@pytest.mark.parametrize("defect", ["writable-bin", "gateway-mode", "gateway-bytes", "gateway-link",
    "gateway-hardlink", "resource-bytes", "public-enclosing", "changed-selection", "unowned-gateway"])
def test_admitted_product_startup_refuses_changed_producer_or_enclosing_custody(product_inputs, defect, monkeypatch):
    f, target, artifacts = product_inputs
    admission = f.admission()
    selected = f.selected(admission)
    gateway = f.gateway.path
    if defect == "writable-bin":
        gateway.parent.chmod(0o777)
    elif defect == "gateway-mode":
        gateway.chmod(0o700)
    elif defect == "gateway-bytes":
        gateway.write_bytes(b"changed inert gateway")
    elif defect == "gateway-link":
        other = gateway.parent.parent.parent / "unselected gateway"
        other.write_bytes(gateway.read_bytes()); other.chmod(0o755)
        gateway.unlink(); gateway.symlink_to(other)
    elif defect == "gateway-hardlink":
        os.link(gateway, gateway.parent.parent.parent / "unselected hardlink")
    elif defect == "resource-bytes":
        resource = gateway.parent.parent / "libexec/meerkat-connect/resources.json"
        resource.chmod(0o644); resource.write_bytes(b"changed inert resource"); resource.chmod(0o444)
    elif defect == "public-enclosing":
        gateway.parent.parent.parent.chmod(0o755)
    elif defect == "unowned-gateway":
        original = Path.lstat
        def unowned(path, *args, **kwargs):
            actual = original(path, *args, **kwargs)
            if path == gateway:
                values = list(actual)
                values[4] = os.getuid() + 1
                return os.stat_result(values)
            return actual
        monkeypatch.setattr(Path, "lstat", unowned)
    else:
        target.assembly.path.write_bytes(b"changed retained producer assembly")
    with pytest.raises(ValueError):
        startup.selected_inputs(f.value, directory=f.directory, binding=selected["binding"],
            installation=selected["installation"])


def test_ordinary_exact_file_keeps_private_permission_requirement(product_inputs):
    f, _, _ = product_inputs
    with pytest.raises(CustodyError, match="private"):
        f.gateway.verify(executable=True)


def test_retained_gateway_refuses_replaced_private_enclosing_bundle(product_inputs):
    f, _, _ = product_inputs
    selected = f.selected(f.admission())
    _, observed = startup.selected_inputs(f.value, directory=f.directory, binding=selected["binding"],
        installation=selected["installation"], retain_observation=True)
    enclosing = observed.gateway.enclosing[0]
    retained = enclosing.with_name("retained-original-private-bin")
    enclosing.rename(retained)
    enclosing.mkdir(mode=0o700)
    with pytest.raises((ValueError, OSError)):
        observed.require_current()


def test_retained_gateway_refuses_changed_private_enclosing_mode(product_inputs):
    f, _, _ = product_inputs
    selected = f.selected(f.admission())
    _, observed = startup.selected_inputs(f.value, directory=f.directory, binding=selected["binding"],
        installation=selected["installation"], retain_observation=True)
    observed.gateway.enclosing[0].chmod(0o500)
    with pytest.raises(CustodyError, match="custody changed"):
        observed.require_current()


async def test_cancelled_product_mutation_stops_fence_and_joins_entered_worker():
    supervisor = SimpleNamespace(stop=asyncio.Event())
    entered, release, settled = threading.Event(), threading.Event(), threading.Event()
    effects = []

    def mutation():
        effects.append("initial owned effect")
        entered.set()
        try:
            assert release.wait(3), "The inert joined-worker test did not release its own worker"
            if supervisor.stop.is_set():
                raise asyncio.CancelledError
            effects.append("forbidden subsequent effect")
        finally:
            settled.set()

    task = asyncio.create_task(worker._joined_product_mutation(supervisor, mutation))
    assert await asyncio.to_thread(entered.wait, 30)
    task.cancel()
    await asyncio.sleep(0)
    assert supervisor.stop.is_set() and not task.done()
    release.set()
    with pytest.raises(asyncio.CancelledError):
        await task
    assert settled.is_set() and effects == ["initial owned effect"]
