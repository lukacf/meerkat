"""Stopped destination source custody, never native or application readiness.

The real artifact receiver supplies a poisoned, inert product closure. These
tests join exact accepted application bytes to that closure without executing
an application, selected SDK, native process, provider or remote connection.
"""
import asyncio
from contextlib import contextmanager, nullcontext
from copy import deepcopy
from dataclasses import replace
import io
import json
import os
from pathlib import Path
import platform
import stat
import sys
import time
from types import SimpleNamespace
import uuid
import zipfile

import pytest

from launchkit import remote_product_destination_owner as owner
from launchkit import remote_product_artifacts as artifacts
from launchkit.remote_interface_bundle import product_destination_owner_bytes
from launchkit.runtime_selection import candidate_selection, selected_runtime_marker
from launchkit.ssh_transport import SshTransportError, _remaining
from launchkit.templates.storage_setup import project_lock
import test_meta_toolkit_runtime_selection as product_fixtures
from test_meta_toolkit_runtime_selection import product, no_product_execution, digest
from test_product_artifact_transfer import (
    GUIDE, BUILD, SOURCE, call as artifact_call, custody as artifact_custody,
    frame, send_complete,
)
from test_product_artifact_transport import ENDPOINT, FINGERPRINT
from test_remote_deployment import longest_loop_gap


MAX_SOURCE = 64 * 1024 * 1024
POISON = b"raise RuntimeError('inert destination source must never execute')\n"


def json_bytes(value):
    return artifacts._json(value)


def package_bytes(files):
    """Preserve all exact source bytes, including empty and binary resources."""
    values = {name: content for name, content in files.items() if name != "SHA256SUMS"}
    values["SHA256SUMS"] = b"".join(
        (digest(content) + "  " + name + "\n").encode()
        for name, content in sorted(values.items()))
    output = io.BytesIO()
    with zipfile.ZipFile(output, "w", zipfile.ZIP_DEFLATED) as archive:
        for name, content in sorted(values.items()):
            info = zipfile.ZipInfo(name)
            info.create_system = 3
            info.external_attr = (0o100000 | (0o755 if name.endswith(".sh") else 0o644)) << 16
            info.compress_type = zipfile.ZIP_DEFLATED
            archive.writestr(info, content)
    return output.getvalue(), values


@pytest.fixture(autouse=True)
def no_destination_execution(monkeypatch):
    def forbidden(*args, **kwargs):
        pytest.fail("Stopped source custody attempted a process or signal operation.")
    monkeypatch.setattr(asyncio, "create_subprocess_exec", forbidden)
    monkeypatch.setattr(asyncio, "create_subprocess_shell", forbidden)
    monkeypatch.setattr(os, "kill", forbidden)
    monkeypatch.setattr(os, "killpg", forbidden)


@pytest.fixture
def destination_source(request, monkeypatch):
    # The shipping product reader supports Linux x86_64. Configure its existing
    # inert fixture before it constructs any selected product bytes.
    target = "linux-x86_64"
    prefix = "meerkat-connect-" + target
    monkeypatch.setattr(product_fixtures, "TARGET", target)
    monkeypatch.setattr(product_fixtures, "PREFIX", prefix)
    monkeypatch.setattr(product_fixtures, "RESOURCE", prefix + "/libexec/meerkat-connect")
    monkeypatch.setattr(platform, "system", lambda: "Linux")
    monkeypatch.setattr(platform, "machine", lambda: "x86_64")
    monkeypatch.setattr(platform, "libc_ver", lambda: ("glibc", "2.31"))
    custody = request.getfixturevalue("artifact_custody")
    send_complete(custody)
    artifact_result = artifact_call(custody, "finish")
    origin = candidate_selection(custody.product.target)
    files = {
        "deployment.json": json_bytes({"schemaVersion": 2, "target": "linux",
            "slug": "inert-destination", "mobkitVersion": custody.product.target.meta.version,
            "runtimeCandidate": selected_runtime_marker(custody.product.target)}),
        "implementation-report.json": json_bytes({"guide_id": GUIDE,
            "guide_version": 3, "build_id": BUILD, "blueprint_hash": "b" * 64,
            "brief_hash": "c" * 64, "verification": {"digest": SOURCE},
            "framework": {"runtime_candidate": origin}}),
        "runtime-candidate.json": json_bytes(origin),
        "app.py": POISON,
        "install.sh": b"#!/bin/sh\nexit 99\n",
        "install_gateway.py": POISON,
        "remote_operations.py": POISON,
        "requirements.txt": b"# Inert source custody, never install\n",
        "config/mob.toml": b"[profiles.guide]\nmodel = 'inert-model'\n",
        "application/tool-policy.json": b'{ "members": [], "literal": "\\u0061" }\n',
        "application/configuration.json": b'{"fields": []}\n',
        "application/console.json": b'{"scope": "inert"}\n',
        "config/access.toml": b"enabled = true\nadmins = ['fixture-owner']\n",
        "README.md": b"INERT SOURCE ONLY - no native setup or readiness\n",
        "assets/empty": b"",
        "assets/binary": bytes(range(256)) * 1025,
    }
    package, complete_files = package_bytes(files)
    source = product_destination_owner_bytes()
    context = {"schema": 1, "deployment_id": str(uuid.uuid4()),
        "guide_id": GUIDE, "build_id": BUILD, "guide_version": 3,
        "source_digest": SOURCE, "package_sha256": digest(package),
        "package_size": len(package), "host": deepcopy(custody.host),
        "artifact_transaction_id": custody.context["transaction_id"],
        "artifact_manifest_sha256": custody.context["manifest_sha256"],
        "artifact_receiver_sha256": custody.context["receiver_sha256"],
        "artifact_selection_sha256": artifact_result["selection_sha256"],
        "owner_sha256": digest(source)}
    return SimpleNamespace(custody=custody, context=context, source=source,
        package=package, files=complete_files, origin=origin,
        destination=Path(custody.host["destination"]))


def receive(selected, action, content=None, *, module=owner, context=None):
    request = {"action": action, "context": context or selected.context}
    payload = json_bytes(request) if content is None else frame(request, content)
    parsed, stream = module.read_request(io.BytesIO(payload))
    return module.receive(parsed, stream, owner_source=selected.source)


def stage(selected, *, module=owner):
    receive(selected, "begin", module=module)
    receive(selected, "package", selected.package, module=module)
    return receive(selected, "finish", module=module)


@pytest.fixture
def gcp_destination_source(destination_source):
    selected = destination_source
    disposition = {"target": "gcp", "project": "forge-cloud-project", "zone": "europe-west1-b",
        "instance": "forge-original-vm", "instance_id": "731902",
        "operation_id": str(uuid.uuid4())}
    definition = json.loads(selected.files["deployment.json"])
    definition["target"] = "gcp"
    disposition["instance"] = definition["slug"]
    files = {**selected.files, "deployment.json": json_bytes(definition),
        "blueprint.json": json_bytes({"target": "gcp", "gcp": {
            "project": disposition["project"], "zone": disposition["zone"],
            "machine_type": "e2-standard-4", "disk_gb": 60}})}
    selected.package, selected.files = package_bytes(files)
    disposition["binding"] = {key: selected.context[key]
        for key in ("guide_id", "guide_version", "build_id", "source_digest")}
    disposition["binding"].update(package_sha256=digest(selected.package),
        blueprint_hash=json.loads(selected.files["implementation-report.json"])["blueprint_hash"])
    selected.context.update(schema=2, source_disposition=disposition,
        package_sha256=digest(selected.package), package_size=len(selected.package))
    return selected


def test_frozen_gcp_intake_retains_original_cloud_plan_and_linux_runtime(gcp_destination_source):
    selected = gcp_destination_source
    original = deepcopy(selected.files)
    with frozen_owner(selected.source) as frozen:
        result = stage(selected, module=frozen)
    assert_stage_only(result)
    assert selected.context["source_disposition"]["operation_id"]
    assert {name: (selected.destination / name).read_bytes() for name in original} == original
    assert json.loads((selected.destination / "deployment.json").read_bytes())["target"] == "gcp"
    with project_lock(selected.destination) as lease:
        with owner.open_stopped_product_source(selected.context, project_lease=lease,
                require_current=lambda: True, owner_source=selected.source) as source:
            source.require_current()
            assert source._context == selected.context
            assert source.origin == selected.custody.plan.origin
            assert source.read_source("blueprint.json") == original["blueprint.json"]
    assert not (selected.destination / ".venv").exists()
    assert not (selected.destination / ".env").exists()


@pytest.mark.parametrize("defect", ["legacy-context", "missing-blueprint", "project", "zone", "target", "vm", "plan"])
def test_gcp_package_cannot_be_reclassified_or_diverted(gcp_destination_source, defect):
    selected = gcp_destination_source
    if defect == "legacy-context":
        selected.context.pop("source_disposition")
        selected.context["schema"] = 1
    elif defect == "vm": selected.context["source_disposition"]["instance"] = "other-original-vm"
    elif defect == "plan": selected.context["source_disposition"]["binding"]["blueprint_hash"] = "e" * 64
    else:
        files = deepcopy(selected.files)
        if defect == "missing-blueprint":
            files.pop("blueprint.json")
        elif defect in {"project", "zone"}:
            blueprint = json.loads(files["blueprint.json"])
            blueprint["gcp"][defect] = "other-cloud-project" if defect == "project" else "us-central1-a"
            files["blueprint.json"] = json_bytes(blueprint)
        else:
            definition = json.loads(files["deployment.json"])
            definition["target"] = "linux"
            files["deployment.json"] = json_bytes(definition)
        selected.package, selected.files = package_bytes(files)
        selected.context.update(package_sha256=digest(selected.package), package_size=len(selected.package))
        selected.context["source_disposition"]["binding"]["package_sha256"] = digest(selected.package)
    receive(selected, "begin")
    receive(selected, "package", selected.package)
    with pytest.raises((ValueError, OSError), match="cloud plan|destination target"):
        receive(selected, "finish")
    assert not selected.destination.exists()


@pytest.mark.parametrize("defect", ["operation", "extra", "instance", "other-build", "other-package"])
def test_gcp_source_disposition_refuses_malformed_identity_before_intake(gcp_destination_source, defect):
    selected = gcp_destination_source
    disposition = selected.context["source_disposition"]
    if defect == "operation": disposition["operation_id"] = "not-an-original-operation"
    elif defect == "extra": disposition["native_ready"] = True
    elif defect == "instance": disposition["instance_id"] = "0"
    elif defect == "other-build": disposition["binding"]["build_id"] = str(uuid.uuid4())
    else: disposition["binding"]["package_sha256"] = "f" * 64
    with pytest.raises((ValueError, OSError)):
        receive(selected, "begin")
    assert not selected.destination.exists()


@contextmanager
def frozen_owner(content):
    """Execute only the exact trusted standalone bundle, never received code."""
    previous = {name: module for name, module in sys.modules.items()
        if name == "launchkit" or name.startswith("launchkit.")}
    namespace = {"__name__": "inert_destination_owner_fixture", "CONTROLLER_SOURCE": content}
    try:
        exec(compile(content, "<trusted-frozen-product-destination>", "exec"), namespace)
        yield SimpleNamespace(**namespace)
    finally:
        for name in list(sys.modules):
            if name == "launchkit" or name.startswith("launchkit."):
                sys.modules.pop(name)
        sys.modules.update(previous)


def assert_stage_only(result):
    assert result["status"] == "product-source-staged"
    assert not any(result.get(name) is True for name in (
        "ready", "installed", "qualified", "native_ready", "setup_completed", "admitted"))
    assert len(json_bytes(result)) < 65536


def test_actual_frozen_owner_stages_exact_original_source_without_native_admission(destination_source):
    selected = destination_source
    original = {pin.path: pin.path.read_bytes() for pin in selected.custody.plan.files}
    with frozen_owner(selected.source) as frozen:
        result = stage(selected, module=frozen)
        assert_stage_only(result)
        assert receive(selected, "observe", module=frozen) == result
    for name, content in selected.files.items():
        path = selected.destination / name
        assert path.read_bytes() == content
        assert stat.S_ISREG(path.stat().st_mode) and path.stat().st_nlink == 1
    assert json.loads((selected.destination / "runtime-candidate.json").read_bytes()) == selected.origin
    assert all(path.read_bytes() == content for path, content in original.items())
    assert not (selected.destination / ".venv").exists()
    assert not (selected.destination / ".bin").exists()
    assert not (selected.destination / ".env").exists()
    assert not (selected.destination / ".mobkit/remote-service.json").exists()
    assert not (selected.destination / ".mobkit/forge-product-source/receipt.json").exists()
    store = selected.custody.home / ".local/state/meta-forge/product-sources" / selected.context["deployment_id"]
    assert (store / "package.zip").read_bytes() == selected.package
    assert (store / "staged.json").read_bytes() == json_bytes(result)


def test_stopped_source_carrier_retains_origin_and_relocated_target_under_original_lease(destination_source):
    selected = destination_source
    assert_stage_only(stage(selected))
    with project_lock(selected.destination) as lease:
        with owner.open_stopped_product_source(selected.context, project_lease=lease,
                require_current=lambda: True, owner_source=selected.source) as source:
            assert source.receipt["destination"] == str(selected.destination)
            assert source.origin == selected.custody.plan.origin
            assert source.relocated != source.origin
            assert source.relocated.artifact_root == selected.custody.store / "inputs"
            assert source.relocated.source_manifest.path.read_bytes() == selected.custody.product.source.path.read_bytes()
            assert source.read_source("application/tool-policy.json") == selected.files["application/tool-policy.json"]
            assert not hasattr(source, "exact_file")
            source.require_owner_current()
            source.require_filesystem_current()
            source.require_current()
            receipt = source.receipt
            receipt["destination"] = "caller changed a detached projection"
            assert source.receipt["destination"] == str(selected.destination)
    with pytest.raises((ValueError, RuntimeError)):
        source.require_current()


@pytest.mark.parametrize("name,mode", [("application/tool-policy.json", 0o644),
    ("install.sh", 0o755), ("assets/empty", 0o644), ("assets/binary", 0o644)])
def test_source_reads_preserve_original_public_modes_and_empty_bytes_without_native_pins(destination_source, name, mode):
    from launchkit.connect_installer_caller import CustodyError, ExactFile
    selected = destination_source
    with frozen_owner(selected.source) as frozen:
        stage(selected, module=frozen)
        with project_lock(selected.destination) as lease:
            with frozen.open_stopped_product_source(selected.context, project_lease=lease,
                    require_current=lambda: True, owner_source=selected.source) as source:
                pin = source.source_file(name)
                assert type(pin) is frozen.ArtifactFile and pin.path == selected.destination / name
                assert pin.size == len(selected.files[name]) and pin.sha256 == digest(selected.files[name])
                assert source.read_source(name) == selected.files[name]
                assert stat.S_IMODE(pin.path.stat().st_mode) == mode
                assert not hasattr(source, "exact_file")
                # The real private native file reader continues to refuse canonical
                # application files. Its future supplier owns the private projection.
                with pytest.raises(CustodyError, match="private, owned and bounded"):
                    ExactFile(pin.path, pin.sha256).read()


def test_source_read_refuses_change_between_full_fence_and_original_descriptor(destination_source, monkeypatch):
    selected = destination_source
    stage(selected)
    target = selected.destination / "application/tool-policy.json"
    original_open = os.open
    def changed_open(path, *args, **kwargs):
        descriptor = original_open(path, *args, **kwargs)
        if Path(path) == target:
            target.write_bytes(selected.files["application/tool-policy.json"] + b"changed at source read")
        return descriptor
    with project_lock(selected.destination) as lease:
        with pytest.raises((ValueError, OSError, RuntimeError), match="source file changed"):
            with owner.open_stopped_product_source(selected.context, project_lease=lease,
                    require_current=lambda: True, owner_source=selected.source) as source:
                monkeypatch.setattr(os, "open", changed_open)
                source.read_source("application/tool-policy.json")


async def test_source_async_currentness_keeps_owner_callback_on_original_loop(destination_source):
    selected = destination_source
    stage(selected)
    loop, task, calls = asyncio.get_running_loop(), asyncio.current_task(), []
    def original_owner():
        assert asyncio.get_running_loop() is loop and asyncio.current_task() is task
        calls.append(True)
    with project_lock(selected.destination) as lease:
        with owner.open_stopped_product_source(selected.context, project_lease=lease,
                require_current=original_owner, owner_source=selected.source) as source:
            await source.require_current_async()
            assert calls


@pytest.mark.parametrize("field", ["guide_id", "build_id", "source_digest", "artifact_transaction_id",
    "artifact_manifest_sha256", "artifact_receiver_sha256", "artifact_selection_sha256", "owner_sha256"])
def test_foreign_source_or_artifact_binding_cannot_join_this_destination(destination_source, field):
    selected = destination_source
    context = deepcopy(selected.context)
    context[field] = str(uuid.uuid4()) if field.endswith("_id") else "f" * 64
    with pytest.raises((ValueError, OSError)):
        receive(selected, "begin", context=context)
    assert not selected.destination.exists()


@pytest.mark.parametrize("field", ["package_sha256", "package_size", "guide_version", "deployment_id"])
def test_started_destination_owner_cannot_rebind_original_package_or_identity(destination_source, field):
    selected = destination_source
    receive(selected, "begin")
    context = deepcopy(selected.context)
    context[field] = ("f" * 64 if field == "package_sha256" else
        str(uuid.uuid4()) if field == "deployment_id" else context[field] + 1)
    with pytest.raises((ValueError, OSError)):
        receive(selected, "package", selected.package, context=context)
    assert not selected.destination.exists()


def test_interrupted_materialization_retains_original_entry_without_retry_or_success(destination_source, monkeypatch):
    selected = destination_source
    receive(selected, "begin")
    receive(selected, "package", selected.package)
    calls = []
    def interrupted(root, files, context, relocated):
        calls.append(context["deployment_id"])
        root.mkdir(mode=0o700)
        (root / "partial-source").write_bytes(b"retained incomplete original source")
        raise OSError("Inert source writer interruption")
    monkeypatch.setattr(owner, "_materialize", interrupted)
    with pytest.raises(OSError, match="writer interruption"):
        receive(selected, "finish")
    store = selected.custody.home / ".local/state/meta-forge/product-sources" / selected.context["deployment_id"]
    original_entry = (store / "entry.json").read_bytes()
    assert not (store / "staged.json").exists()
    for action in ("begin", "finish", "observe"):
        with pytest.raises((ValueError, OSError)):
            receive(selected, action)
    assert calls == [selected.context["deployment_id"]]
    assert (store / "entry.json").read_bytes() == original_entry
    assert (selected.destination / "partial-source").read_bytes() == b"retained incomplete original source"


@pytest.mark.parametrize("problem", ["truncated", "changed", "trailing"])
def test_partial_or_changed_package_has_no_staged_source_and_retains_uncertainty(destination_source, problem):
    selected = destination_source
    receive(selected, "begin")
    payload = frame({"action": "package", "context": selected.context}, selected.package)
    if problem == "truncated": payload = payload[:-1]
    elif problem == "changed": payload = payload[:-1] + bytes([payload[-1] ^ 1])
    else: payload += b"unowned trailer"
    request, stream = owner.read_request(io.BytesIO(payload))
    with pytest.raises((ValueError, OSError)):
        owner.receive(request, stream, owner_source=selected.source)
    with pytest.raises((ValueError, OSError)):
        receive(selected, "finish")
    assert not selected.destination.exists()


def test_finish_without_original_package_cannot_be_reconstructed_as_completed_source(destination_source):
    selected = destination_source
    receive(selected, "begin")
    with pytest.raises((ValueError, OSError)):
        receive(selected, "finish")
    assert not selected.destination.exists()
    # Preserve the original incomplete owner rather than accepting a replacement
    # package or materializing source from a reconstructed success observation.
    with pytest.raises((ValueError, OSError)):
        receive(selected, "finish")
    assert not selected.destination.exists()


@pytest.mark.parametrize("problem", ["relocated-selection", "report-selection", "build", "version", "digest"])
def test_source_zip_cannot_rewrite_original_accepted_selection(destination_source, problem):
    selected = destination_source
    files = dict(selected.files)
    report = json.loads(files["implementation-report.json"])
    if problem == "relocated-selection":
        files["runtime-candidate.json"] = (selected.custody.store / "selection.json").read_bytes()
    elif problem == "report-selection":
        report["framework"]["runtime_candidate"] = json.loads((selected.custody.store / "selection.json").read_bytes())
    elif problem == "build": report["build_id"] = str(uuid.uuid4())
    elif problem == "version": report["guide_version"] += 1
    else: report["verification"]["digest"] = "f" * 64
    files["implementation-report.json"] = json_bytes(report)
    selected.package, selected.files = package_bytes(files)
    selected.context.update(package_sha256=digest(selected.package), package_size=len(selected.package))
    receive(selected, "begin")
    with pytest.raises((ValueError, OSError)):
        receive(selected, "package", selected.package)
        receive(selected, "finish")
    assert not selected.destination.exists()


@pytest.mark.parametrize("problem", ["selected-artifact", "manifest", "selection", "custody", "receiver-owner"])
def test_fresh_join_refuses_changed_completed_artifact_custody(destination_source, problem):
    selected = destination_source
    receive(selected, "begin")
    receive(selected, "package", selected.package)
    store = selected.custody.store
    if problem == "selected-artifact":
        path = store / "inputs" / selected.custody.plan.files[0].path.relative_to(selected.custody.plan.origin.artifact_root)
    else:
        path = store / {"manifest": "manifest.json", "selection": "selection.json",
            "custody": "custody.json", "receiver-owner": "owner.json"}[problem]
    path.write_bytes(path.read_bytes() + b"changed after complete transfer")
    with pytest.raises((ValueError, OSError)):
        receive(selected, "finish")
    assert not selected.destination.exists()


@pytest.mark.parametrize("problem", ["installed-file", "installed-selection", "artifact-file", "extra-source", "source-link",
    "source-owner", "source-owner-access", "intake-package", "intake-owner", "source-hardlink"])
def test_retained_carrier_invalidates_changed_source_or_artifact_closure(destination_source, problem):
    selected = destination_source
    stage(selected)
    with project_lock(selected.destination) as lease:
        # Intake mutation remains a refusal at the original outer custody exit.
        exit_expectation = pytest.raises((ValueError, OSError, RuntimeError)) if problem == "intake-owner" else nullcontext()
        with exit_expectation:
            with owner.open_stopped_product_source(selected.context, project_lease=lease,
                    require_current=lambda: True, owner_source=selected.source) as source:
                path = selected.destination / "README.md"
                if problem == "installed-selection": path = selected.destination / "runtime-candidate.json"
                elif problem == "artifact-file": path = source.relocated.assembly.path
                elif problem in {"source-owner", "source-owner-access"}:
                    path = selected.destination / ".mobkit/forge-product-source/owner.json"
                elif problem in {"intake-package", "intake-owner"}:
                    store = selected.custody.home / ".local/state/meta-forge/product-sources" / selected.context["deployment_id"]
                    path = store / ("package.zip" if problem == "intake-package" else "owner.json")
                if problem == "extra-source":
                    path = selected.destination / "unselected.py"
                    path.write_bytes(POISON)
                    path.chmod(0o600)
                elif problem == "source-link":
                    path.unlink()
                    path.symlink_to(selected.custody.product.source.path)
                elif problem == "source-hardlink":
                    os.link(path, selected.custody.home / "unexpected hardlink")
                elif problem == "source-owner-access":
                    path.chmod(0o644)
                else:
                    path.write_bytes(path.read_bytes() + b"changed")
                with pytest.raises((ValueError, OSError, RuntimeError)):
                    source.require_filesystem_current()


def test_another_project_lease_does_not_authorize_stopped_source(destination_source):
    selected = destination_source
    stage(selected)
    other = selected.custody.home / "another project"
    other.mkdir(mode=0o700)
    with project_lock(other) as lease:
        with pytest.raises((ValueError, RuntimeError)):
            with owner.open_stopped_product_source(selected.context, project_lease=lease,
                    require_current=lambda: True, owner_source=selected.source):
                pytest.fail("A foreign project lease returned usable source custody.")


def test_source_carrier_refuses_original_owner_loss_without_reacquiring_authority(destination_source):
    selected = destination_source
    stage(selected)
    current = {"value": True}
    def original_owner():
        return current["value"]
    with project_lock(selected.destination) as lease:
        with owner.open_stopped_product_source(selected.context, project_lease=lease,
                require_current=original_owner, owner_source=selected.source) as source:
            current["value"] = False
            with pytest.raises((ValueError, RuntimeError)):
                source.require_owner_current()


def test_application_package_context_bound_remains_64_mebibytes(destination_source):
    selected = destination_source
    context = {**selected.context, "package_size": MAX_SOURCE + 1}
    with pytest.raises((ValueError, OSError)):
        receive(selected, "begin", context=context)
    assert not selected.destination.exists()


def test_compressed_application_cannot_expand_beyond_existing_source_bound(destination_source):
    selected = destination_source
    files = {**selected.files, "assets/oversized": b"x" * (MAX_SOURCE + 1)}
    selected.package, selected.files = package_bytes(files)
    assert len(selected.package) < MAX_SOURCE
    selected.context.update(package_sha256=digest(selected.package), package_size=len(selected.package))
    receive(selected, "begin")
    with pytest.raises((ValueError, OSError)):
        receive(selected, "package", selected.package)
        receive(selected, "finish")
    assert not selected.destination.exists()


def test_staged_product_source_does_not_enable_ordinary_remote_install(destination_source):
    from launchkit import remote_destination
    from test_remote_destination import bundle
    selected = destination_source
    stage(selected)
    before = {name: (selected.destination / name).read_bytes() for name in selected.files}
    public = {name: content for name, content in selected.files.items() if name != "SHA256SUMS"}
    public["application_configuration.py"] = POISON
    archive, transferred_digest = bundle(public)
    binding = {"guide_id": GUIDE, "guide_version": selected.context["guide_version"],
        "build_id": BUILD, "accepted_source_digest": SOURCE,
        "package_sha256": selected.context["package_sha256"],
        "transferred_source_digest": transferred_digest, "controller_sha256": "d" * 64}
    with pytest.raises(remote_destination.ControllerError) as refused:
        remote_destination._archive(archive, binding)
    assert refused.value.code == "runtime-unsupported"
    assert before == {name: (selected.destination / name).read_bytes() for name in selected.files}
    assert not (selected.destination / ".mobkit/launch-remote/operation.json").exists()


class AcceptedImplementation:
    """Existing implementation authority with inert accepted bytes only."""

    def __init__(self, selected):
        self.selected, self.target = selected, selected.custody.plan.origin
        self.calls, self.accepted = [], True

    async def status(self, guide_id):
        assert guide_id == GUIDE
        self.calls.append("status")
        return {"build_id": BUILD, "guide_version": 3, "source_digest": SOURCE,
            "current": True, "acceptance_current": self.accepted,
            "acceptance_trusted": self.accepted, "ready_to_package": True,
            "acceptance": {"status": "pass" if self.accepted else "pending"}}

    def selected_runtime_target(self, guide_id, *, build_id, source_digest):
        assert (guide_id, build_id, source_digest) == (GUIDE, BUILD, SOURCE)
        self.calls.append("selected_runtime_target")
        return self.target

    async def package(self, guide_id, build_id):
        assert (guide_id, build_id) == (GUIDE, BUILD)
        self.calls.append("package")
        return self.selected.package, "inert-destination"


class SourceFramedTransport:
    """Same pinned owner transport interface, without creating a connection.

    A real destination runs in its own process; the host only awaits its
    answer. The in-process destination therefore runs in a worker thread, so
    its file reads and syncs are never measured as host event-loop work.
    """

    def __init__(self, selected):
        self.selected = selected
        self.calls, self.deadlines = [], set()
        self.on_package = None
        self.entered, self.block = asyncio.Event(), False

    def admitted(self, endpoint, program, request, timeout, expected_fingerprint, deadline):
        assert endpoint == ENDPOINT and expected_fingerprint == FINGERPRINT
        assert isinstance(program, str) and program
        assert 0 < timeout <= 600 and deadline > asyncio.get_running_loop().time()
        # Exercise the production loop-owned helper, including the actual loop.
        # No mock replaces its get_running_loop contract or monotonic deadline.
        assert _remaining(deadline) > 0
        self.calls.append(request["action"])
        self.deadlines.add(deadline)

    async def request(self, endpoint, program, request, timeout, *, expected_fingerprint, deadline):
        self.admitted(endpoint, program, request, timeout, expected_fingerprint, deadline)
        def destination():
            parsed, stream = owner.read_request(io.BytesIO(json_bytes(request)))
            return owner.receive(parsed, stream, owner_source=self.selected.source)
        result = await asyncio.to_thread(destination)
        return {**result, "_transport": {"host_key_fingerprint": expected_fingerprint}}

    async def transfer_file(self, endpoint, program, request, artifact, timeout, *,
            expected_fingerprint, deadline, require_current):
        self.admitted(endpoint, program, request, timeout, expected_fingerprint, deadline)
        require_current()
        def destination():
            # The real transport also reads the artifact in a worker thread.
            parsed, stream = owner.read_request(io.BytesIO(frame(request, artifact.path.read_bytes())))
            return owner.receive(parsed, stream, owner_source=self.selected.source)
        result = await asyncio.to_thread(destination)
        if self.on_package is not None:
            self.on_package()
        self.entered.set()
        if self.block:
            await asyncio.Future()
        require_current()
        return {**result, "_transport": {"host_key_fingerprint": expected_fingerprint}}


def staging_arguments(selected, tmp_path, transport, *, require_current=lambda: True):
    artifact = artifacts.ProductArtifactCustody(
        selected.context["artifact_transaction_id"], selected.context["artifact_manifest_sha256"],
        selected.context["artifact_receiver_sha256"], selected.custody.plan.origin,
        artifacts.relocate_product_target(selected.custody.plan.manifest, selected.custody.store / "inputs"),
        selected.context["artifact_selection_sha256"])
    return {"build_id": BUILD, "source_digest": SOURCE, "guide_version": 3,
        "deployment_id": selected.context["deployment_id"], "custody": artifact,
        "transaction_directory": tmp_path.resolve() / "host source intake",
        "destination_host": selected.custody.host, "transport": transport,
        "endpoint": ENDPOINT, "expected_fingerprint": FINGERPRINT,
        "require_current": require_current}


async def test_trusted_accepted_source_uses_existing_pinned_transport_and_original_loop(destination_source, tmp_path):
    selected = destination_source
    implementation, transport = AcceptedImplementation(selected), SourceFramedTransport(selected)
    loop, task, calls = asyncio.get_running_loop(), asyncio.current_task(), []
    def original_owner():
        assert asyncio.get_running_loop() is loop and asyncio.current_task() is task
        calls.append(True)
    arguments = staging_arguments(selected, tmp_path, transport, require_current=original_owner)
    result = await owner.stage_product_destination_source(implementation, GUIDE, **arguments)
    assert_stage_only(result)
    assert transport.calls == ["begin", "package", "finish"] and len(transport.deadlines) == 1
    assert implementation.calls[0] == "status" and "package" in implementation.calls
    assert calls
    retained = arguments["transaction_directory"]
    assert (retained / "package.zip").read_bytes() == selected.package
    intent = json.loads((retained / "intent.json").read_bytes())
    assert intent == selected.context
    assert json.loads((retained / "staged.json").read_bytes()) == {
        "context": selected.context, "observation": result}
    assert json.loads((selected.destination / "runtime-candidate.json").read_bytes()) == selected.origin


async def test_changed_implementation_after_package_transfer_has_no_new_stage_or_completion(destination_source, tmp_path):
    selected = destination_source
    implementation, transport = AcceptedImplementation(selected), SourceFramedTransport(selected)
    transport.on_package = lambda: setattr(implementation, "target",
        replace(implementation.target, artifact_root=selected.custody.home / "changed source"))
    arguments = staging_arguments(selected, tmp_path, transport)
    with pytest.raises(ValueError, match="source changed"):
        await owner.stage_product_destination_source(implementation, GUIDE, **arguments)
    assert transport.calls == ["begin", "package"]
    retained = arguments["transaction_directory"]
    assert json.loads((retained / "intent.json").read_bytes()) == selected.context
    assert (retained / "package.zip").read_bytes() == selected.package
    assert not (retained / "staged.json").exists() and not selected.destination.exists()


async def test_cancelled_original_host_intake_keeps_original_context_and_received_package(destination_source, tmp_path):
    selected = destination_source
    implementation, transport = AcceptedImplementation(selected), SourceFramedTransport(selected)
    transport.block = True
    arguments = staging_arguments(selected, tmp_path, transport)
    task = asyncio.create_task(owner.stage_product_destination_source(implementation, GUIDE, **arguments))
    entered = asyncio.create_task(transport.entered.wait())
    try:
        done, _ = await asyncio.wait((task, entered), timeout=5, return_when=asyncio.FIRST_COMPLETED)
        if task in done:
            # Observe the real failure immediately instead of waiting for an
            # event that a failed original staging task can never produce.
            await task
            pytest.fail("Blocked original source transfer unexpectedly completed.")
        assert entered in done, "Original source transfer did not enter within its bounded wait."
        task.cancel()
        with pytest.raises(asyncio.CancelledError):
            await asyncio.wait_for(asyncio.shield(task), timeout=5)
    finally:
        if not entered.done():
            entered.cancel()
        await asyncio.gather(entered, return_exceptions=True)
        if not task.done():
            task.cancel()
        try:
            await asyncio.wait_for(asyncio.shield(task), timeout=5)
        except asyncio.CancelledError:
            pass
    assert transport.calls == ["begin", "package"]
    retained = arguments["transaction_directory"]
    assert json.loads((retained / "intent.json").read_bytes()) == selected.context
    assert (retained / "package.zip").read_bytes() == selected.package
    assert not (retained / "staged.json").exists() and not selected.destination.exists()
    remote_store = selected.custody.home / ".local/state/meta-forge/product-sources" / selected.context["deployment_id"]
    assert json.loads((remote_store / "owner.json").read_bytes()) == selected.context
    assert (remote_store / "package.zip").read_bytes() == selected.package
    assert not (remote_store / "staged.json").exists()


async def test_actual_host_deadline_expiry_after_begin_retains_original_uncertain_intake(destination_source, tmp_path):
    selected = destination_source
    class ExpiringTransport(SourceFramedTransport):
        async def request(self, endpoint, program, request, timeout, *, expected_fingerprint, deadline):
            result = await super().request(endpoint, program, request, timeout,
                expected_fingerprint=expected_fingerprint, deadline=deadline)
            assert request["action"] == "begin"
            await asyncio.sleep(max(0, deadline - asyncio.get_running_loop().time()) + 0.02)
            return result
    implementation, transport = AcceptedImplementation(selected), ExpiringTransport(selected)
    arguments = staging_arguments(selected, tmp_path, transport)
    with pytest.raises(SshTransportError) as failed:
        await owner.stage_product_destination_source(implementation, GUIDE, timeout=1, **arguments)
    assert failed.value.code == "timeout"
    assert transport.calls == ["begin"]
    retained = arguments["transaction_directory"]
    assert json.loads((retained / "intent.json").read_bytes()) == selected.context
    assert not (retained / "staged.json").exists() and not selected.destination.exists()
    remote_store = selected.custody.home / ".local/state/meta-forge/product-sources" / selected.context["deployment_id"]
    assert json.loads((remote_store / "owner.json").read_bytes()) == selected.context
    assert not (remote_store / "package.zip").exists() and not (remote_store / "staged.json").exists()


async def test_unaccepted_source_never_opens_destination_or_local_intake(destination_source, tmp_path):
    selected = destination_source
    implementation, transport = AcceptedImplementation(selected), SourceFramedTransport(selected)
    implementation.accepted = False
    arguments = staging_arguments(selected, tmp_path, transport)
    with pytest.raises(ValueError, match="accepted application"):
        await owner.stage_product_destination_source(implementation, GUIDE, **arguments)
    assert transport.calls == [] and not arguments["transaction_directory"].exists()
    assert not selected.destination.exists()


def slow_host_package_staging(monkeypatch, transport, intake):
    """Slow the host's own package work until it first contacts the destination."""
    original_hashlib, original_retain, original_read = owner.hashlib, artifacts._retain_file, owner._Inputs.read
    class Hashing:
        def __getattr__(self, name):
            return getattr(original_hashlib, name)
        def sha256(self, data=b""):
            if bytes(data[:4]) == b"PK\x03\x04":
                time.sleep(1.0)  # Hashing a large package on a small host.
            return original_hashlib.sha256(data)
    def retain(path, content):
        if path == intake / "package.zip":
            time.sleep(1.0)  # Writing and syncing its copy.
        return original_retain(path, content)
    def read(self, path, *args, **kwargs):
        if path == intake / "package.zip":
            time.sleep(1.0)  # Reading the copy back.
        return original_read(self, path, *args, **kwargs)
    monkeypatch.setattr(owner, "hashlib", Hashing())
    monkeypatch.setattr(artifacts, "_retain_file", retain)
    monkeypatch.setattr(owner._Inputs, "read", read)
    original_request = transport.request
    async def request(*args, **kwargs):
        owner.hashlib = original_hashlib  # The inert destination runs in this process.
        return await original_request(*args, **kwargs)
    transport.request = request


async def test_host_package_staging_leaves_the_event_loop_free(destination_source, tmp_path, monkeypatch):
    selected = destination_source
    implementation, transport = AcceptedImplementation(selected), SourceFramedTransport(selected)
    arguments = staging_arguments(selected, tmp_path, transport)
    slow_host_package_staging(monkeypatch, transport, arguments["transaction_directory"])
    result, gap = await longest_loop_gap(owner.stage_product_destination_source(implementation, GUIDE, **arguments))
    assert_stage_only(result)
    assert transport.calls == ["begin", "package", "finish"]
    assert (arguments["transaction_directory"] / "package.zip").read_bytes() == selected.package
    assert gap < 0.5


async def test_a_slow_destination_is_not_host_event_loop_work(destination_source, tmp_path, monkeypatch):
    # A real destination runs in its own process over SSH, and the host only
    # awaits its answer. The in-process stand-in keeps that work off the host's
    # loop too; otherwise the loop-gap rows measure the destination's disk.
    selected = destination_source
    implementation, transport = AcceptedImplementation(selected), SourceFramedTransport(selected)
    arguments = staging_arguments(selected, tmp_path, transport)
    original_receive = owner.receive
    def slow_receive(*args, **kwargs):
        time.sleep(1.0)  # A slow destination disk, not host work.
        return original_receive(*args, **kwargs)
    monkeypatch.setattr(owner, "receive", slow_receive)
    result, gap = await longest_loop_gap(owner.stage_product_destination_source(implementation, GUIDE, **arguments))
    assert_stage_only(result)
    assert transport.calls == ["begin", "package", "finish"]
    assert gap < 0.5
