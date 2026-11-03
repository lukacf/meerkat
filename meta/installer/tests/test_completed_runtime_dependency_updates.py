"""Completed dependency history survives compiler drift without new admission."""
import copy
import hashlib
import json
import uuid

import pytest

from launchkit import compiler, deployment_updates as updates, generator
from launchkit.deployment import read_receipt, update_receipt
from launchkit.deployment_worker import update_runtime
from launchkit.generator import PackageFile
from launchkit.projects import DraftConflict
from test_runtime_dependency_updates import (canonical_fixture_packages, dependency_staged,
    mock_dependency_runtime, registry_fixture_packages)


async def completed_fixture(tmp_path, monkeypatch, request, *, registry):
    request.getfixturevalue("registry_fixture_packages" if registry else "canonical_fixture_packages")
    service, directory, destination, previous, private, identity = await dependency_staged(tmp_path)
    epoch = {"revision": 1} if registry else None
    runtime, events = mock_dependency_runtime(monkeypatch, directory, destination, registry_epoch=epoch)
    await update_runtime(runtime, "update", update_id=identity)
    # This fixture ends its inert supervisor at the same stopped UI boundary.
    update_receipt(directory, status="stopped", process_live=False, console_url=None, runtime_pid=None)
    receipt, root, update = updates.transaction(directory, identity)
    assert update["status"] == "completed"
    return service, directory, destination, root, update, receipt, events


from historical_runtime_fixture import qualified_runtime_compiler


def history_bytes(root):
    return {name: (root / name).read_bytes() for name in
            ("update.json", "previous.zip", "candidate.zip", "previous-receipt.json")}


def private_bytes(destination):
    return {name: (destination / name).read_bytes() for name in
            (".env", ".mobkit/state/native.db", "data/ledger.db", "workspace/user.txt")}


@pytest.mark.parametrize("registry", [False, True])
async def test_completed_status_uses_frozen_qualification_without_current_compiler(
        tmp_path, monkeypatch, request, registry):
    service, directory, destination, root, update, receipt, events = await completed_fixture(
        tmp_path, monkeypatch, request, registry=registry)
    before, private, executed = history_bytes(root), private_bytes(destination), list(events)

    def no_qualification(*args, **kwargs):
        pytest.fail("completed history must not requalify or regenerate the old host")
    monkeypatch.setattr(compiler, "generate_package", no_qualification)
    monkeypatch.setattr(updates, "current_host_framework", no_qualification)
    observed = await service.status(service.guide.identity)
    assert observed["update"] == update
    assert observed["build_id"] == update["to_build_id"]
    assert observed["package_sha256"] == receipt["package_sha256"]
    assert observed["status"] == "stopped"
    assert history_bytes(root) == before and private_bytes(destination) == private
    assert events == executed


def advance_compiler(monkeypatch):
    original = compiler.generate_package
    def successor(blueprint):
        files = [PackageFile(item.path, item.content + "\n# Later qualified host generation.\n", item.mode)
                 if item.path == "app.py" else item for item in original(blueprint) if item.path != "SHA256SUMS"]
        sums = "".join(f"{hashlib.sha256(item.content.encode()).hexdigest()}  {item.path}\n"
                       for item in sorted(files, key=lambda item: item.path))
        return [*files, PackageFile("SHA256SUMS", sums)]
    monkeypatch.setattr(compiler, "generate_package", successor)


@pytest.mark.parametrize("registry", [False, True])
async def test_stage_successor_after_completed_crossing_keeps_original_history_and_jobs(
        tmp_path, monkeypatch, request, registry):
    service, directory, destination, root, original, receipt, events = await completed_fixture(
        tmp_path, monkeypatch, request, registry=registry)
    before, private, executed = history_bytes(root), private_bytes(destination), list(events)
    advance_compiler(monkeypatch)
    _, archived_candidate = updates.checked_package(root, "candidate.zip", original["package_sha256"])
    assert updates.current_host_framework(archived_candidate) is False
    assert updates.runtime_dependency_transition(
        updates.checked_package(root, "previous.zip", original["previous_package_sha256"])[1],
        archived_candidate, receipt) is None
    service.implementation.identity = str(uuid.uuid4())
    service.implementation.digest = "later-independently-accepted-host-source"
    staged = await service.stage_update(service.guide.identity, receipt["deployment_id"],
        service.guide.state["version"], service.implementation.identity)
    assert staged["update"]["status"] == "staged" and staged["update"]["blockers"] == []
    assert staged["update"]["update_id"] != original["update_id"]
    assert staged["update"]["runtime_dependency_update"] is None
    assert staged["update"]["native_definition_update"] is False
    assert staged["build_id"] == original["to_build_id"]
    assert staged["package_sha256"] == original["package_sha256"]
    assert history_bytes(root) == before and private_bytes(destination) == private
    assert events == executed


@pytest.mark.parametrize("status", ["staged", "blocked", "snapshotting", "applying", "verifying",
    "restoring", "recovering-current", "recovering-candidate", "recovery-needed", "rolled-back"])
async def test_noncompleted_dependency_transactions_keep_current_host_fence(
        tmp_path, monkeypatch, canonical_fixture_packages, status):
    _, directory, _, _, _, identity = await dependency_staged(tmp_path)
    _, root, update = updates.transaction(directory, identity)
    updates.checkpoint(directory, root, update, status=status)
    monkeypatch.setattr(updates, "current_host_framework", lambda files: False)
    with pytest.raises(DraftConflict, match="qualified host"):
        updates.transaction(directory, identity)


@pytest.mark.parametrize("mutation", ["bool-version", "missing-field", "unknown-release", "unknown-path",
    "missing-path", "duplicate-path", "unordered-paths", "wrong-baseline", "wrong-current-baseline",
    "unknown-framework", "retained-host", "bad-mode", "changed-host", "changed-profile", "foreign-build",
    "foreign-source", "foreign-brief", "foreign-previous-build", "foreign-previous-source",
    "missing-retention", "foreign-retention", "missing-native-proof", "foreign-native-proof", "wrong-sdk",
    "false-admission", "rollback-claim", "storage-version", "bool-storage-marker", "missing-activation",
    "bool-pin", "foreign-activation-package", "wrong-epoch", "wrong-native-hash", "wrong-original-package"])
async def test_closed_history_refuses_forged_or_incomplete_retained_authority(
        tmp_path, monkeypatch, request, mutation):
    _, directory, destination, root, saved, receipt, events = await completed_fixture(
        tmp_path, monkeypatch, request, registry=True)
    before, private, executed = history_bytes(root), private_bytes(destination), list(events)
    _, previous = updates.checked_package(root, "previous.zip", saved["previous_package_sha256"])
    _, candidate = updates.checked_package(root, "candidate.zip", saved["package_sha256"])
    update, receipt, previous, candidate = copy.deepcopy((saved, receipt, previous, candidate))
    descriptor = update["runtime_dependency_update"]

    def report_change(files, change):
        report = json.loads(files["implementation-report.json"][0])
        change(report)
        files["implementation-report.json"] = (json.dumps(report).encode(), 0o644)

    if mutation == "bool-version": descriptor["version"] = True
    elif mutation == "missing-field": del descriptor["core_version"]
    elif mutation == "unknown-release": descriptor["to_version"] = "0.8.49"
    elif mutation == "unknown-path": descriptor["protected_paths"].append("application/unauthorized.py")
    elif mutation == "missing-path": descriptor["protected_paths"].remove("app.py")
    elif mutation == "duplicate-path": descriptor["protected_paths"].append("app.py")
    elif mutation == "unordered-paths": descriptor["protected_paths"].reverse()
    elif mutation == "wrong-baseline": report_change(candidate, lambda value: value["framework"].update(baseline_digest="f" * 64))
    elif mutation == "wrong-current-baseline": report_change(candidate, lambda value: value["framework"].update(current_compiler_baseline_digest="f" * 64))
    elif mutation == "unknown-framework": report_change(candidate, lambda value: value["framework"].update(version=True))
    elif mutation == "retained-host": report_change(candidate, lambda value: value["framework"].update(retained_template_paths=["app.py"]))
    elif mutation == "bad-mode": candidate["app.py"] = (candidate["app.py"][0], 0o755)
    elif mutation == "changed-host": candidate["app.py"] = (candidate["app.py"][0] + b"# Foreign host\n", 0o644)
    elif mutation == "changed-profile": candidate["config/mob.toml"] = (candidate["config/mob.toml"][0].replace(b"gpt-6.1-sol", b"gpt-6-luna"), 0o644)
    elif mutation == "foreign-build": update["to_build_id"] = str(uuid.uuid4())
    elif mutation == "foreign-source": update["source_digest"] = "foreign"
    elif mutation == "foreign-brief": update["brief_hash"] = "f" * 64
    elif mutation == "foreign-previous-build": update["from_build_id"] = str(uuid.uuid4())
    elif mutation == "foreign-previous-source": update["previous_source_digest"] = "foreign"
    elif mutation == "missing-retention": del update["runtime_files_retained"]
    elif mutation == "foreign-retention": update["runtime_files_retained"]["destination"] = "/foreign"
    elif mutation == "missing-native-proof": del update["runtime_artifacts"]
    elif mutation == "foreign-native-proof": update["runtime_artifacts"]["native"]["source"] = "override"
    elif mutation == "wrong-sdk": update["runtime_artifacts"]["sdk"]["version"] = "0.8.42"
    elif mutation == "false-admission": update["runtime_admission_possible"] = False
    elif mutation == "rollback-claim": update["rollback_available"] = True
    elif mutation == "storage-version": update["runtime_storage_upgrade"]["to_version"] = "0.8.42"
    elif mutation == "bool-storage-marker": update["runtime_storage_upgrade"]["marker"] = {"format": True,
        "project": json.loads(candidate["deployment.json"][0])["slug"], "version": generator.MOBKIT_VERSION, "phase": "ready"}
    elif mutation == "missing-activation": del update["native_definition_activation"]
    elif mutation == "bool-pin": update["native_activation_expected_revision"] = True
    elif mutation == "foreign-activation-package": update["native_activation_package_sha256"] = "f" * 64
    elif mutation == "wrong-epoch": update["native_definition_activation"]["definition_revision"] = 3
    elif mutation == "wrong-native-hash": update["native_definition_activation"]["native_binary_sha256"] = "f" * 64
    elif mutation == "wrong-original-package": update["native_definition_observation"]["package_sha256"] = "f" * 64
    derived = updates._completed_runtime_dependency_transition(previous, candidate, receipt, update, root)
    assert not updates._same_toml(descriptor, derived)
    assert history_bytes(root) == before and private_bytes(destination) == private and events == executed


async def test_completed_status_alone_cannot_license_unexecuted_transition(
        tmp_path, monkeypatch, canonical_fixture_packages):
    _, directory, _, _, _, identity = await dependency_staged(tmp_path)
    _, root, update = updates.transaction(directory, identity)
    updates.checkpoint(directory, root, update, status="completed", rollback_available=False,
                       runtime_admission_possible=True)
    with pytest.raises(DraftConflict, match="qualified host"):
        updates.transaction(directory, identity)


@pytest.mark.parametrize("name", ["candidate.zip", "previous.zip"])
async def test_closed_history_rechecks_original_archive_hashes(tmp_path, monkeypatch, request, name):
    _, directory, destination, root, update, _, events = await completed_fixture(
        tmp_path, monkeypatch, request, registry=True)
    private, executed = private_bytes(destination), list(events)
    with (root / name).open("ab") as archive:
        archive.write(b"foreign bytes")
    with pytest.raises(DraftConflict, match="package changed"):
        updates.transaction(directory, update["update_id"])
    assert private_bytes(destination) == private and events == executed


async def test_closed_registry_history_accepts_exact_false_carrier_and_forward_recovery_proof(
        tmp_path, monkeypatch, request):
    _, directory, _, root, update, receipt, _ = await completed_fixture(
        tmp_path, monkeypatch, request, registry=True)
    _, previous = updates.checked_package(root, "previous.zip", update["previous_package_sha256"])
    _, candidate = updates.checked_package(root, "candidate.zip", update["package_sha256"])
    activation = update["native_definition_activation"]
    activation["host_system_prompt"] = {"send": False, "dispatch": False}
    assert updates._same_toml(update["runtime_dependency_update"],
        updates._completed_runtime_dependency_transition(previous, candidate, receipt, update, root))
    # A supported recovery can retain the original CAS pin and observe its
    # exact already-applied successor without publishing a second activation.
    update["native_definition_recovery_observation"] = copy.deepcopy(activation)
    del update["native_definition_activation"]
    assert updates._same_toml(update["runtime_dependency_update"],
        updates._completed_runtime_dependency_transition(previous, candidate, receipt, update, root))
