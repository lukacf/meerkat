"""Explicit candidate update/history counterexamples with inert artifact bytes.

No SDK payload, native executable, installed application or provider is run.
The Bundle's signature admission is deliberately synthetic test authority.
"""
import copy
from dataclasses import replace
import hashlib
import io
import json
import os
from pathlib import Path
import shutil
import uuid
import zipfile

import pytest

from launchkit import compiler, generator, runtime_candidates, runtime_selection
from launchkit import deployment_updates as updates
from launchkit.deployment import OWNER_FILE, ownership, read_receipt, update_receipt
from launchkit.generator import PackageFile
from launchkit.projects import DraftConflict, fingerprint
from launchkit.templates.storage_setup import project_lock
from test_reading_runtime_refresh import previous_framework
from test_runtime_candidates import Bundle
from test_runtime_dependency_updates import native_fixture_proof


def inventory(files):
    return {item.path: {"sha256": updates.digest(item.content.encode()), "mode": item.mode} for item in files}


def packed(files):
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w") as archive:
        for name, (content, mode) in files.items():
            entry = zipfile.ZipInfo(name)
            entry.external_attr = (0o100000 | mode) << 16
            archive.writestr(entry, content)
    return buffer.getvalue()


def document(files, name, change):
    value = json.loads(files[name][0])
    change(value)
    files[name] = (json.dumps(value).encode(), files[name][1])


@pytest.fixture
def crossing(tmp_path, monkeypatch):
    bundle = Bundle(tmp_path / "external-artifacts")
    target = runtime_selection.RuntimeCandidateTarget(bundle.tuple_path, bundle.root, bundle.binding)
    spec, snapshot = previous_framework(monkeypatch, "0.8.47")
    old = {item.path: (item.content.encode(), 0o755 if item.mode & 0o111 else 0o644) for item in snapshot.files}
    old_framework = {"kind": "host-framework-provenance", "version": 1,
        "blueprint_digest": fingerprint(spec), "baseline_digest": snapshot.baseline_digest,
        "current_compiler_baseline_digest": snapshot.baseline_digest, "basis": "current-compiler",
        "retained_template_paths": []}
    old_report = {"build_id": str(uuid.uuid4()), "guide_version": 1, "blueprint_hash": fingerprint(spec),
        "brief_hash": "b" * 64, "verification": {"digest": "c" * 64}, "unresolved": [], "framework": old_framework}
    old["implementation-report.json"] = (json.dumps(old_report).encode(), 0o644)
    old["application/__init__.py"] = (b"raise RuntimeError('application must never be imported')\n", 0o644)
    old["requirements.application.txt"] = (b"# Exact accepted application dependency pin\n", 0o644)
    source = [PackageFile(name, content.decode(), 0o600 if name == ".env.example" else mode)
        for name, (content, mode) in old.items() if name != "SHA256SUMS"]
    generated = compiler.generate_runtime_refresh(spec, source, runtime_target=target)
    ordinary = compiler.generate_package(spec, runtime_target=target)
    baseline, current = inventory(generated), inventory(ordinary)
    retained = sorted(name for name in baseline.keys() | current.keys()
        if name != "SHA256SUMS" and baseline.get(name) != current.get(name))
    previous_binding = {"build_id": old_report["build_id"], "source_digest": old_report["verification"]["digest"]}
    framework = {"kind": "host-framework-provenance", "version": 1, "blueprint_digest": fingerprint(spec),
        "basis": "runtime-refresh", "reason": "explicit-runtime-refresh",
        "previous": previous_binding, "origin": previous_binding,
        "baseline_digest": fingerprint(baseline), "current_compiler_baseline_digest": fingerprint(current),
        "retained_template_paths": retained, "runtime_candidate": runtime_selection.candidate_selection(target)}
    candidate = {item.path: (item.content.encode(), 0o755 if item.mode & 0o111 else 0o644) for item in generated}
    for name, value in old.items():
        if name.startswith("application/") or name == "requirements.application.txt":
            candidate[name] = value
    new_report = {**old_report, "build_id": str(uuid.uuid4()), "verification": {"digest": "d" * 64}, "framework": framework}
    candidate["implementation-report.json"] = (json.dumps(new_report).encode(), 0o644)
    return bundle, target, old, candidate, old_report, new_report


def installation(tmp_path, crossing):
    _, _, previous, _, old_report, _ = crossing
    directory, destination = tmp_path / "journal", tmp_path / "application"
    directory.mkdir(mode=0o700)
    destination.mkdir(mode=0o700)
    payload = packed(previous)
    (directory / "package.zip").write_bytes(payload)
    (directory / "package.zip").chmod(0o600)
    receipt = {"deployment_id": str(uuid.uuid4()), "guide_id": str(uuid.uuid4()),
        "target": "local", "execution_mode": "interactive", "status": "stopped",
        "destination": str(destination), "package_sha256": updates.digest(payload),
        **{key: old_report[key] for key in updates.BOUND_KEYS if key != "package_sha256"}}
    (directory / "receipt.json").write_text(json.dumps(receipt))
    (directory / "receipt.json").chmod(0o600)
    for name, (content, mode) in previous.items():
        path = destination / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(content)
        path.chmod(mode)
    (destination / OWNER_FILE).parent.mkdir(parents=True, exist_ok=True)
    (destination / ".mobkit").chmod(0o700)
    (destination / OWNER_FILE).write_text(json.dumps(ownership(receipt)))
    (destination / OWNER_FILE).chmod(0o600)
    for name, content in {".env": b"SYNTHETIC_PRIVATE=preserve\n", ".mobkit/state/native.db": b"inert native state",
            "data/ledger.db": b"inert application state", "workspace/user.txt": b"inert user reference"}.items():
        path = destination / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(content)
        path.chmod(0o600)
    return directory, destination


def candidate_proof(descriptor, destination):
    selection = descriptor["runtime_candidate"]
    common = updates._candidate_binding_fields(selection)
    expected = descriptor["runtime_candidate_artifacts"]
    sdk = {"format": "forge-installed-candidate-sdk-v1", "source": "local-build-from-exact-frozen-source",
        "signed": False, "published": False, "version": "0.8.48",
        "site_packages": str(destination / ".venv/lib/python3.14/site-packages"), **common, **copy.deepcopy(expected["sdk"])}
    native = {"version": "0.8.48", "core_version": "0.8.51", "platform": selection["accepted"]["platform"],
        "source": "signed-candidate", **common, "sdk": sdk,
        "binaries": {name: {key: value for key, value in item.items() if key != "binary_bytes"}
                     for name, item in expected["native"].items()}}
    return sdk, native


def completed(tmp_path, crossing):
    _, target, _, candidate, _, report = crossing
    directory, destination = installation(tmp_path, crossing)
    staged = updates.stage(directory, packed(candidate), report, runtime_target=target)
    identity = staged["update"]["update_id"]
    updates.activate(directory, identity)
    receipt, root, update = updates.transaction(directory, identity)
    sdk, native = candidate_proof(update["runtime_dependency_update"], destination)
    update = updates.checkpoint(directory, root, update, status="completed", blockers=[],
        rollback_available=False, runtime_admission_possible=True,
        runtime_files_retained={"format": "mobkit-launch-runtime-retention-v1", "retained": True,
            "inventory_sha256": "e" * 64, "destination": str(destination), "backup": str(root / "runtime-before"),
            "previous_native": native_fixture_proof("0.8.47")},
        runtime_artifacts={"sdk": sdk, "native": native})
    return directory, destination, root, update


def test_explicit_crossing_keeps_default_design_and_frozen_members(crossing):
    bundle, target, previous, candidate, _, _ = crossing
    assert compiler.MOBKIT_VERSION == generator.MOBKIT_VERSION == "0.8.47"
    assert updates.runtime_dependency_transition(previous, candidate, {}) is None
    assert updates.compatibility(previous, candidate, {})[1]
    descriptor = updates.runtime_dependency_transition(previous, candidate, {}, runtime_target=target)
    assert descriptor["runtime_candidate"] == runtime_selection.candidate_selection(target)
    assert descriptor["definition_unchanged"] and not descriptor["native_registry_update"]
    assert updates.CANDIDATE_PATHS <= set(descriptor["protected_paths"])
    assert set(descriptor["runtime_candidate_artifacts"]["native"]) == {"rpc_gateway", "mobkit_gateway"}
    assert descriptor["runtime_candidate_artifacts"]["sdk"]["payload_sha256"] == bundle.sdk["source_payload"]
    assert not updates.compatibility(previous, candidate, {}, runtime_target=target)[1]


def test_current_candidate_pins_wheel_helper_in_package_and_worker(crossing):
    _, target, previous, candidate, _, _ = crossing
    descriptor = updates.runtime_dependency_transition(previous, candidate, {}, runtime_target=target)
    assert descriptor is not None
    assert "sdk_wheel_integrity.py" in descriptor["protected_paths"]
    sources = descriptor["required_supervisor_sources"]
    assert sources["launchkit/sdk_wheel_integrity.py"] == updates.digest(candidate["sdk_wheel_integrity.py"][0])
    assert updates.CANDIDATE_PATHS == {"runtime-candidate.json", "runtime_candidates.py", "install_candidate.py"}
    for paths in (updates._RUNTIME_FRAMEWORK_V2_SUPERVISOR_PATHS,
                  updates._RUNTIME_FRAMEWORK_V3_SUPERVISOR_PATHS,
                  updates._RUNTIME_FRAMEWORK_V4_SUPERVISOR_PATHS):
        assert "launchkit/sdk_wheel_integrity.py" not in paths


@pytest.mark.parametrize("defect", ["missing-package-helper", "missing-worker-helper", "changed-worker-helper"])
def test_dependent_candidate_reader_refuses_missing_or_changed_wheel_helper(crossing, defect):
    _, target, previous, candidate, _, _ = crossing
    descriptor = updates.runtime_dependency_transition(previous, candidate, {}, runtime_target=target)
    selected = copy.deepcopy(candidate)
    sources = copy.deepcopy(descriptor["required_supervisor_sources"])
    if defect == "missing-package-helper":
        selected.pop("sdk_wheel_integrity.py")
        with pytest.raises(ValueError, match="lost its SDK wheel dependency"):
            updates._runtime_framework_paths(selected, 4)
        with pytest.raises(ValueError, match="lost its SDK wheel dependency"):
            updates._archived_supervisor_sources(selected, {"required_supervisor_sources": sources}, 4)
    else:
        if defect == "missing-worker-helper":
            sources.pop("launchkit/sdk_wheel_integrity.py")
        else:
            sources["launchkit/sdk_wheel_integrity.py"] = "f" * 64
        assert updates._archived_supervisor_sources(selected, {"required_supervisor_sources": sources}, 4) is None


@pytest.mark.parametrize("version", [2, 3, 4])
def test_retained_self_contained_reader_keeps_original_worker_inventory(crossing, version):
    _, _, _, candidate, _, _ = crossing
    selected = copy.deepcopy(candidate)
    selected["runtime_candidates.py"] = (b"import zipfile\n# Retained self-contained reader source; never executed.\n", 0o644)
    selected.pop("sdk_wheel_integrity.py")
    sources = updates._runtime_history_supervisor_sources(version)
    sources.pop("launchkit/sdk_wheel_integrity.py")
    sources["launchkit/runtime_candidates.py"] = updates.digest(selected["runtime_candidates.py"][0])
    assert "sdk_wheel_integrity.py" not in updates._runtime_framework_paths(selected, version)
    assert updates._archived_supervisor_sources(selected, {"required_supervisor_sources": sources}, version) == sources
    assert set(updates._runtime_supervisor_paths(version, sources=sources)) == set(sources)
    sources["launchkit/runtime_candidates.py"] = "f" * 64
    assert updates._archived_supervisor_sources(selected, {"required_supervisor_sources": sources}, version) is None


def test_current_worker_pinset_cannot_omit_helper_even_when_host_has_it(crossing):
    from launchkit import deployment_service as service
    _, target, previous, candidate, _, _ = crossing
    descriptor = updates.runtime_dependency_transition(previous, candidate, {}, runtime_target=target)
    frozen = service.supervisor_metadata({}, service.supervisor_source_files())
    descriptor["required_supervisor_sources"].pop("launchkit/sdk_wheel_integrity.py")
    with pytest.raises(DraftConflict, match="exact current runtime history and compiler closure"):
        service.require_candidate_update_protocol(frozen, {"runtime_dependency_update": descriptor})


def test_transition_expands_one_verified_tuple_and_independent_fence_reads_fresh(crossing, monkeypatch):
    _, target, previous, candidate, _, _ = crossing
    calls = []
    original = runtime_candidates._native

    def observed(*args, **kwargs):
        calls.append(args[2]["binary"])
        return original(*args, **kwargs)

    monkeypatch.setattr(runtime_candidates, "_native", observed)
    descriptor = updates.runtime_dependency_transition(previous, candidate, {}, runtime_target=target)
    assert descriptor is not None and descriptor["definition_unchanged"]
    assert descriptor["runtime_candidate"]["accepted"]["tuple_sha256"] == target.accepted.tuple_sha256
    assert sorted(calls) == sorted(runtime_candidates.BINARIES)
    independent = updates.runtime_dependency_transition(previous, candidate, {}, runtime_target=target)
    assert independent == descriptor and len(calls) == 2 * len(runtime_candidates.BINARIES)


def test_transition_refuses_owned_artifact_changed_after_full_read(crossing, monkeypatch):
    bundle, target, previous, candidate, _, _ = crossing
    original = runtime_candidates.read_candidate_artifacts

    def raced(*args, **kwargs):
        verified = original(*args, **kwargs)
        path = bundle.wheel_path
        info = path.stat()
        path.write_bytes(path.read_bytes())
        os.utime(path, ns=(info.st_atime_ns, info.st_mtime_ns))
        return verified

    monkeypatch.setattr(runtime_candidates, "read_candidate_artifacts", raced)
    assert updates.runtime_dependency_transition(previous, candidate, {}, runtime_target=target) is None
    assert all(candidate[name] == previous[name] for name in compiler._RUNTIME_DESIGN_PATHS)
    assert candidate["application/__init__.py"] == previous["application/__init__.py"]


@pytest.mark.parametrize("mutation", ["source", "wheel", "manifest", "attempt", "locator", "native-platform"])
def test_equal_version_cannot_substitute_the_caller_selected_tuple(crossing, mutation):
    bundle, target, previous, candidate, _, _ = crossing
    if mutation == "locator":
        target = replace(target, tuple_path=bundle.root / "another-tuple.json")
    else:
        key, value = {"source": ("source_sha", "f" * 40), "wheel": ("sdk_wheel_sha256", "f" * 64),
            "manifest": ("native_manifest_sha256", "f" * 64), "attempt": ("native_run_attempt", 2),
            "native-platform": ("platform", "x86_64-apple-darwin")}[mutation]
        target = replace(target, accepted=replace(target.accepted, **{key: value}))
    assert updates.runtime_dependency_transition(previous, candidate, {}, runtime_target=target) is None


@pytest.mark.parametrize("mutation", ["selection", "deployment-extra", "framework", "application", "native",
    "requirements", "candidate-mode", "missing-adapter", "adapter-code"])
def test_selected_candidate_does_not_mask_unauthorized_source_changes(crossing, mutation):
    _, target, previous, candidate, _, _ = crossing
    candidate = copy.deepcopy(candidate)
    if mutation == "selection":
        document(candidate, "runtime-candidate.json", lambda value: value["accepted"].update(native_run_attempt=True))
    elif mutation == "deployment-extra":
        document(candidate, "deployment.json", lambda value: value.update(unknown_authority=True))
    elif mutation == "framework":
        document(candidate, "implementation-report.json", lambda value: value["framework"].pop("runtime_candidate"))
    elif mutation == "application":
        candidate["application/__init__.py"] = (b"# changed accepted application\n", 0o644)
    elif mutation == "native":
        candidate["config/mob.toml"] = (candidate["config/mob.toml"][0] + b"\n# changed native\n", 0o644)
    elif mutation == "requirements":
        candidate["requirements.application.txt"] = (b"new-authority==1.0\n", 0o644)
    elif mutation == "candidate-mode":
        candidate["runtime-candidate.json"] = (candidate["runtime-candidate.json"][0], 0o755)
    elif mutation == "missing-adapter":
        candidate.pop("install_candidate.py")
    elif mutation == "adapter-code":
        candidate["install_candidate.py"] = (candidate["install_candidate.py"][0] + b"# foreign adapter\n", 0o644)
    assert updates.runtime_dependency_transition(previous, candidate, {}, runtime_target=target) is None
    assert updates.compatibility(previous, candidate, {}, runtime_target=target)[1]


def test_current_transaction_rechecks_artifacts_but_never_reselects(crossing, tmp_path):
    bundle, target, _, candidate, _, report = crossing
    directory, destination = installation(tmp_path, crossing)
    receipt = updates.stage(directory, packed(candidate), report, runtime_target=target)
    descriptor = receipt["update"]["runtime_dependency_update"]
    assert descriptor is not None and "runtime_candidate" not in receipt
    before = (directory / "package.zip").read_bytes()
    private = (destination / ".env").read_bytes()
    bundle.wheel_path.write_bytes(bundle.wheel_path.read_bytes() + b"foreign bytes")
    with pytest.raises(DraftConflict, match="qualified host"):
        updates.transaction(directory, receipt["update"]["update_id"])
    assert (directory / "package.zip").read_bytes() == before and (destination / ".env").read_bytes() == private


def test_source_activation_and_forward_retry_publish_the_checked_carrier(crossing, tmp_path):
    _, target, _, candidate, _, report = crossing
    directory, destination = installation(tmp_path, crossing)
    private = {name: (destination / name).read_bytes() for name in (".env", "data/ledger.db", ".mobkit/state/native.db", "workspace/user.txt")}
    staged = updates.stage(directory, packed(candidate), report, runtime_target=target)
    identity = staged["update"]["update_id"]
    updates.activate(directory, identity)
    receipt, root, update = updates.transaction(directory, identity)
    assert receipt["runtime_candidate"] == update["runtime_dependency_update"]["runtime_candidate"]
    updates.checkpoint(directory, root, update, status="recovery-needed", runtime_admission_possible=True)
    updates.retry_candidate(directory, identity)
    receipt = read_receipt(directory)
    assert receipt["runtime_candidate"] == staged["update"]["runtime_dependency_update"]["runtime_candidate"]
    assert {name: (destination / name).read_bytes() for name in private} == private
    with pytest.raises(DraftConflict, match="downgrad"):
        updates.restore(directory, identity, restore_data=False)


@pytest.mark.parametrize("existing", ["equal", "different", "null"])
def test_ordinary_predecessor_carrier_is_not_replaced_by_staged_authority(crossing, tmp_path, existing):
    _, target, _, candidate, _, report = crossing
    directory, destination = installation(tmp_path, crossing)
    staged = updates.stage(directory, packed(candidate), report, runtime_target=target)
    carrier = copy.deepcopy(staged["update"]["runtime_dependency_update"]["runtime_candidate"])
    if existing == "different":
        carrier["accepted"]["source_sha"] = "f" * 40
    elif existing == "null":
        carrier = None
    update_receipt(directory, runtime_candidate=carrier)
    original = (destination / "application/__init__.py").read_bytes()
    package = (directory / "package.zip").read_bytes()
    marker = (destination / OWNER_FILE).read_bytes()
    with pytest.raises(DraftConflict, match="ordinary predecessor"):
        updates.activate(directory, staged["update"]["update_id"])
    assert (destination / "application/__init__.py").read_bytes() == original
    assert (directory / "package.zip").read_bytes() == package
    assert (destination / OWNER_FILE).read_bytes() == marker
    assert read_receipt(directory)["runtime_candidate"] == carrier
    assert (destination / ".mobkit/state/native.db").read_bytes() == b"inert native state"


@pytest.mark.parametrize("existing", ["different", "null", "missing"])
def test_forward_retry_does_not_repair_an_unbound_candidate_receipt(crossing, tmp_path, existing):
    _, target, _, candidate, _, report = crossing
    directory, destination = installation(tmp_path, crossing)
    staged = updates.stage(directory, packed(candidate), report, runtime_target=target)
    identity = staged["update"]["update_id"]
    updates.activate(directory, identity)
    receipt, root, update = updates.transaction(directory, identity)
    updates.checkpoint(directory, root, update, status="recovery-needed", runtime_admission_possible=True)
    receipt = read_receipt(directory)
    if existing == "different":
        receipt["runtime_candidate"]["accepted"]["source_sha"] = "f" * 40
    elif existing == "null":
        receipt["runtime_candidate"] = None
    else:
        receipt.pop("runtime_candidate")
    (directory / "receipt.json").write_text(json.dumps(receipt))
    package = (directory / "package.zip").read_bytes()
    marker = (destination / OWNER_FILE).read_bytes()
    with pytest.raises(DraftConflict, match="matching admitted host selection"):
        updates.retry_candidate(directory, identity)
    assert (directory / "package.zip").read_bytes() == package
    assert (destination / OWNER_FILE).read_bytes() == marker
    assert read_receipt(directory) == receipt
    assert (destination / ".mobkit/state/native.db").read_bytes() == b"inert native state"


def test_pre_admission_source_restore_removes_carrier_without_ordinary_null_key(crossing, tmp_path):
    _, target, _, candidate, _, report = crossing
    directory, _ = installation(tmp_path, crossing)
    staged = updates.stage(directory, packed(candidate), report, runtime_target=target)
    updates.activate(directory, staged["update"]["update_id"])
    assert "runtime_candidate" in read_receipt(directory)
    updates.restore(directory, staged["update"]["update_id"], restore_data=True)
    assert "runtime_candidate" not in read_receipt(directory)


def test_same_runtime_application_source_keeps_existing_admitted_carrier(crossing, tmp_path):
    directory, destination, _, update = completed(tmp_path, crossing)
    _, _, _, candidate, _, _ = crossing
    receipt = read_receipt(directory)
    admitted = copy.deepcopy(receipt["runtime_candidate"])
    changed = copy.deepcopy(candidate)
    changed["application/__init__.py"] = (b"# inert application successor\n", 0o644)
    with project_lock(destination) as lease:
        result = updates._publish_source_receipt(directory, changed, {"runtime_dependency_update": None},
            project_lease=lease, detail="inert same-runtime source")
    assert result["runtime_candidate"] == admitted
    document(changed, "runtime-candidate.json", lambda value: value["accepted"].update(source_sha="f" * 40))
    with project_lock(destination) as lease:
        with pytest.raises((DraftConflict, ValueError)):
            updates._publish_source_receipt(directory, changed, {"runtime_dependency_update": None}, project_lease=lease)
    assert read_receipt(directory)["runtime_candidate"] == admitted


def same_runtime_application(directory, crossing):
    candidate = copy.deepcopy(crossing[3])
    candidate["application/__init__.py"] = (b"# inert application successor\n", 0o644)
    report = json.loads(candidate["implementation-report.json"][0])
    report.update(build_id=str(uuid.uuid4()), verification={"digest": "a" * 64})
    candidate["implementation-report.json"] = (json.dumps(report).encode(), 0o644)
    staged = updates.stage(directory, packed(candidate), report, runtime_target=crossing[1])
    assert staged["update"]["runtime_dependency_update"] is None
    assert not staged["update"]["blockers"]
    return staged, candidate


def test_later_application_activation_preserves_checked_candidate_source(crossing, tmp_path):
    directory, destination, _, _ = completed(tmp_path, crossing)
    admitted = read_receipt(directory)["runtime_candidate"]
    staged, candidate = same_runtime_application(directory, crossing)
    updates.activate(directory, staged["update"]["update_id"])
    assert read_receipt(directory)["runtime_candidate"] == admitted
    assert (destination / "application/__init__.py").read_bytes() == candidate["application/__init__.py"][0]
    assert (destination / ".mobkit/state/native.db").read_bytes() == b"inert native state"


def test_missing_receipt_carrier_refuses_before_application_replacement(crossing, tmp_path):
    directory, destination, _, _ = completed(tmp_path, crossing)
    staged, _ = same_runtime_application(directory, crossing)
    receipt = read_receipt(directory)
    receipt.pop("runtime_candidate")
    (directory / "receipt.json").write_text(json.dumps(receipt))
    original = (destination / "application/__init__.py").read_bytes()
    journal = (directory / "package.zip").read_bytes()
    with pytest.raises(DraftConflict, match="matching admitted host selection"):
        updates.activate(directory, staged["update"]["update_id"])
    assert (destination / "application/__init__.py").read_bytes() == original
    assert (directory / "package.zip").read_bytes() == journal


@pytest.mark.parametrize("mode", [0o755, 0o770])
def test_candidate_source_activation_refuses_nonprivate_project_lock(crossing, tmp_path, mode):
    _, target, _, candidate, _, report = crossing
    directory, destination = installation(tmp_path, crossing)
    private = destination / ".mobkit"
    assert private.stat().st_mode & 0o777 == 0o700
    staged = updates.stage(directory, packed(candidate), report, runtime_target=target)
    private.chmod(mode)
    before = {name: (directory / name).read_bytes() for name in ("package.zip", "receipt.json")}
    source = (destination / "app.py").read_bytes()
    state = (private / "state/native.db").read_bytes()
    with pytest.raises(RuntimeError, match="Unsafe project lock"):
        updates.activate(directory, staged["update"]["update_id"])
    assert {name: (directory / name).read_bytes() for name in before} == before
    assert (destination / "app.py").read_bytes() == source
    assert (private / "state/native.db").read_bytes() == state
    assert private.stat().st_mode & 0o777 == mode


@pytest.fixture
def maintained_crossing(crossing, tmp_path, monkeypatch):
    from launchkit.implementation_workspace import FrameworkSnapshot, ImplementationWorkspace
    bundle, target, _, admitted, _, accepted_report = crossing
    names = updates._archived_framework_paths(admitted)
    framework = [PackageFile(name, content.decode(), 0o600 if name == ".env.example" else mode)
        for name, (content, mode) in admitted.items() if name in names]
    framework = [PackageFile(item.path, "# Older independently accepted host\n", item.mode)
                 if item.path in {"app.py", "console_auth.py"} else item for item in framework]
    sums = "".join(f"{hashlib.sha256(item.as_bytes()).hexdigest()}  {item.path}\n"
                   for item in sorted(framework, key=lambda item: item.path))
    framework.append(PackageFile("SHA256SUMS", sums))
    baseline = fingerprint(inventory(framework))
    old_report = copy.deepcopy(accepted_report)
    old_report["framework"].update(baseline_digest=baseline)
    old = {item.path: (item.as_bytes(), 0o755 if item.mode & 0o111 else 0o644) for item in framework}
    old.update({name: value for name, value in admitted.items()
                if name.startswith("application/") or name == "requirements.application.txt"})
    old["implementation-report.json"] = (json.dumps(old_report).encode(), 0o644)
    spec = json.loads(old["blueprint.json"][0])
    origin = {"build_id": old_report["build_id"], "source_digest": old_report["verification"]["digest"]}
    snapshot = FrameworkSnapshot(fingerprint(spec), tuple(framework), baseline, origin["build_id"],
        origin["source_digest"], origin, runtime_selection.candidate_selection(target))
    workspace = ImplementationWorkspace(tmp_path / "fresh-host-workspace", spec)
    workspace.seed(snapshot, runtime_refresh=True, runtime_target=target)
    for name, (content, _) in old.items():
        if name.startswith("application/") or name == "requirements.application.txt":
            workspace.write_file(name, content.decode())
    candidate = {item.path: (item.as_bytes(), 0o755 if item.mode & 0o111 else 0o644)
        for item in workspace.export_files(require_verified=False)}
    new_report = {**old_report, "build_id": str(uuid.uuid4()), "verification": {"digest": "e" * 64},
        "framework": workspace.framework_provenance()}
    candidate["implementation-report.json"] = (json.dumps(new_report).encode(), 0o644)
    assert workspace.verification() is None
    return bundle, target, old, candidate, old_report, new_report


def maintained_installation(tmp_path, crossing):
    directory, destination = installation(tmp_path, crossing)
    update_receipt(directory, runtime_candidate=runtime_selection.candidate_selection(crossing[1]))
    return directory, destination


@pytest.fixture
def repeated_maintained_crossing(maintained_crossing, tmp_path, monkeypatch):
    from launchkit.implementation_workspace import FrameworkSnapshot, ImplementationWorkspace
    _, target, _, previous, _, previous_report = maintained_crossing
    source_root = tmp_path / "next-maintained-source"
    for name in updates._maintained_supervisor_paths():
        source = Path(updates.__file__).parent.joinpath(*Path(name).parts[1:])
        destination = source_root / name
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, destination)
    templates = source_root / "launchkit/templates_v2"
    shutil.copytree(compiler.TEMPLATES_V2, templates, dirs_exist_ok=True)
    shell = templates / "console_shell.css"
    shell.write_bytes(shell.read_bytes() + b"\n/* Next independently maintained Console host. */\n")
    monkeypatch.setattr(compiler, "TEMPLATES_V2", templates)
    monkeypatch.setattr(updates, "__file__", str(source_root / "launchkit/deployment_updates.py"))
    names = sorted(set(updates._archived_framework_paths(previous)) | set(compiler.MAINTAINED_HOST_PATHS))
    framework = [PackageFile(name, previous[name][0].decode(),
        0o600 if name == ".env.example" else previous[name][1]) for name in names]
    framework.append(PackageFile("SHA256SUMS", "".join(
        f"{hashlib.sha256(item.as_bytes()).hexdigest()}  {item.path}\n" for item in framework)))
    baseline = fingerprint(inventory(framework))
    assert baseline == previous_report["framework"]["baseline_digest"]
    spec = json.loads(previous["blueprint.json"][0])
    origin = {"build_id": previous_report["build_id"],
        "source_digest": previous_report["verification"]["digest"]}
    snapshot = FrameworkSnapshot(fingerprint(spec), tuple(framework), baseline,
        origin["build_id"], origin["source_digest"], origin, runtime_selection.candidate_selection(target))
    workspace = ImplementationWorkspace(tmp_path / "second-fresh-host-workspace", spec)
    workspace.seed(snapshot, runtime_refresh=True, runtime_target=target)
    for name, (content, _) in previous.items():
        if name.startswith("application/") or name == "requirements.application.txt":
            workspace.write_file(name, content.decode())
    candidate = {item.path: (item.as_bytes(), 0o755 if item.mode & 0o111 else 0o644)
        for item in workspace.export_files(require_verified=False)}
    report = {**previous_report, "build_id": str(uuid.uuid4()), "verification": {"digest": "f" * 64},
        "framework": workspace.framework_provenance()}
    candidate["implementation-report.json"] = (json.dumps(report).encode(), 0o644)
    return target, previous, candidate, report


def test_second_maintained_host_crossing_authenticates_exact_extended_previous_inventory(repeated_maintained_crossing):
    target, previous, candidate, report = repeated_maintained_crossing
    receipt = {"runtime_candidate": runtime_selection.candidate_selection(target)}
    descriptor = updates.maintained_host_transition(previous, candidate, receipt, runtime_target=target)
    assert descriptor is not None
    assert descriptor["previous"]["build_id"] == json.loads(previous["implementation-report.json"][0])["build_id"]
    assert descriptor["candidate"]["build_id"] == report["build_id"]
    assert descriptor["protected_paths"] == sorted(compiler.MAINTAINED_HOST_PATHS)
    assert descriptor["runtime_candidate"] == receipt["runtime_candidate"]
    assert previous["console_shell.css"] != candidate["console_shell.css"]
    assert previous["config/mob.toml"] == candidate["config/mob.toml"]
    assert previous["application/__init__.py"] == candidate["application/__init__.py"]


def test_two_successive_maintained_host_activations_keep_private_state_and_first_history(maintained_crossing, tmp_path, monkeypatch):
    _, target, _, first_candidate, _, first_report = maintained_crossing
    directory, destination = maintained_installation(tmp_path, maintained_crossing)
    names = (".env", "data/ledger.db", ".mobkit/state/native.db", "workspace/user.txt")
    retained = {name: (destination / name).read_bytes() for name in names}
    first = updates.stage(directory, packed(first_candidate), first_report, runtime_target=target)["update"]
    updates.activate(directory, first["update_id"])
    _, first_root, first = updates.transaction(directory, first["update_id"])
    updates.checkpoint(directory, first_root, first, status="completed", blockers=[],
        rollback_available=True, runtime_admission_possible=True)
    frozen_first = {str(path.relative_to(first_root)): path.read_bytes()
        for path in first_root.rglob("*") if path.is_file()}
    target, previous, candidate, report = repeated_maintained_crossing.__wrapped__(
        maintained_crossing, tmp_path, monkeypatch)
    second = updates.stage(directory, packed(candidate), report, runtime_target=target)["update"]
    assert not second["blockers"] and second["runtime_dependency_update"] is None
    assert second["native_definition_update"] is False and second["host_instruction_update"] is False
    updates.activate(directory, second["update_id"])
    _, second_root, second = updates.transaction(directory, second["update_id"])
    updates.checkpoint(directory, second_root, second, status="completed", blockers=[],
        rollback_available=True, runtime_admission_possible=True)
    assert {name: (destination / name).read_bytes() for name in names} == retained
    assert {str(path.relative_to(first_root)): path.read_bytes()
        for path in first_root.rglob("*") if path.is_file()} == frozen_first
    assert read_receipt(directory)["runtime_candidate"] == runtime_selection.candidate_selection(target)
    assert (destination / "console_shell.css").read_bytes() == candidate["console_shell.css"][0]
    assert (destination / "application/__init__.py").read_bytes() == previous["application/__init__.py"][0]


@pytest.mark.parametrize("mutation", ["missing-host", "host-mode", "baseline", "unknown-inventory", "selection", "application",
    "present-template-annotation", "next-template-annotation", "unsafe-absent-annotation", "unsafe-knowledge-annotation",
    "non-string-annotation", "control-annotation"])
def test_second_maintained_host_crossing_refuses_unproven_previous_inventory(repeated_maintained_crossing, mutation):
    target, previous, candidate, _ = repeated_maintained_crossing
    previous, candidate = copy.deepcopy(previous), copy.deepcopy(candidate)
    receipt = {"runtime_candidate": runtime_selection.candidate_selection(target)}
    if mutation == "missing-host":
        previous.pop("console_shell.py")
    elif mutation == "host-mode":
        previous["console_shell.mjs"] = (previous["console_shell.mjs"][0], 0o755)
    elif mutation == "baseline":
        document(previous, "implementation-report.json", lambda value: value["framework"].update(baseline_digest="0" * 64))
    elif mutation == "unknown-inventory":
        previous["future_host.py"] = (b"# Unqualified future inventory\n", 0o644)
        names = sorted(set(updates._archived_framework_paths(previous)) | set(compiler.MAINTAINED_HOST_PATHS) | {"future_host.py"})
        framework = [PackageFile(name, previous[name][0].decode(),
            0o600 if name == ".env.example" else previous[name][1]) for name in names]
        framework.append(PackageFile("SHA256SUMS", "".join(
            f"{hashlib.sha256(item.as_bytes()).hexdigest()}  {item.path}\n" for item in framework)))
        document(previous, "implementation-report.json", lambda value: value["framework"].update(baseline_digest=fingerprint(inventory(framework))))
    elif mutation == "selection":
        receipt["runtime_candidate"]["accepted"]["source_sha"] = "0" * 40
    elif mutation in {"present-template-annotation", "next-template-annotation", "unsafe-absent-annotation",
            "unsafe-knowledge-annotation", "non-string-annotation"}:
        name = {"unsafe-absent-annotation": "../unknown_template.py",
            "unsafe-knowledge-annotation": "knowledge/../unknown_template.py",
            "non-string-annotation": 1}.get(mutation, "unknown_template.py")
        document(previous, "implementation-report.json", lambda value: value["framework"]["retained_template_paths"].insert(0, name))
        if mutation == "present-template-annotation":
            previous[name] = candidate[name] = (b"# Unqualified optional host\n", 0o644)
        elif mutation == "next-template-annotation":
            candidate[name] = (b"# Unqualified optional host\n", 0o644)
    elif mutation == "control-annotation":
        document(previous, "implementation-report.json", lambda value: value["framework"]["retained_template_paths"].insert(0, "unknown\n_template.py"))
    else:
        candidate["application/__init__.py"] = (b"# Unaccepted application change\n", 0o644)
    assert updates.maintained_host_transition(previous, candidate, receipt, runtime_target=target) is None


def test_maintained_host_activation_and_retry_preserve_admitted_runtime_and_all_private_state(maintained_crossing, tmp_path, monkeypatch):
    _, target, previous, candidate, _, report = maintained_crossing
    directory, destination = maintained_installation(tmp_path, maintained_crossing)
    retained_names = (".env", "data/ledger.db", ".mobkit/state/native.db", "workspace/user.txt")
    private = {name: (destination / name).read_bytes() for name in retained_names}
    admitted = copy.deepcopy(read_receipt(directory)["runtime_candidate"])
    def forbidden(*args, **kwargs):
        pytest.fail("A base host crossing must not generate optional setup/Toolkit or run a runtime installer.")
    monkeypatch.setattr(compiler, "generate_package", forbidden)
    staged = updates.stage(directory, packed(candidate), report, runtime_target=target)
    update = staged["update"]
    assert not update["blockers"] and update["runtime_dependency_update"] is None
    assert update["native_definition_update"] is False and update["host_instruction_update"] is False
    assert update["maintained_host_update"]["protected_paths"] == sorted(compiler.MAINTAINED_HOST_PATHS)
    assert update["maintained_host_update"]["runtime_candidate"] == admitted
    for name in previous.keys() - set(compiler.MAINTAINED_HOST_PATHS) - {"implementation-report.json", "SHA256SUMS"}:
        assert previous[name] == candidate[name]
    updates.activate(directory, update["update_id"])
    receipt, root, checkpoint = updates.transaction(directory, update["update_id"])
    updates.checkpoint(directory, root, checkpoint, status="recovery-needed", runtime_admission_possible=True)
    updates.retry_candidate(directory, update["update_id"])
    assert read_receipt(directory)["runtime_candidate"] == admitted
    assert {name: (destination / name).read_bytes() for name in private} == private
    assert (destination / "application/__init__.py").read_bytes() == previous["application/__init__.py"][0]
    assert all((destination / name).read_bytes() == candidate[name][0] for name in compiler.MAINTAINED_HOST_PATHS)


@pytest.mark.parametrize("mutation", ["application", "native", "dependency", "candidate-adapter", "host-code",
    "host-mode", "origin", "baseline", "receipt-selection", "missing-independent-target"])
def test_maintained_host_refuses_foreign_runtime_design_application_and_source(maintained_crossing, mutation):
    _, target, previous, candidate, _, _ = maintained_crossing
    candidate = copy.deepcopy(candidate)
    receipt = {"runtime_candidate": runtime_selection.candidate_selection(target)}
    if mutation in {"application", "native", "dependency", "candidate-adapter", "host-code"}:
        name = {"application": "application/__init__.py", "native": "config/mob.toml", "dependency": "requirements.txt",
                "candidate-adapter": "install_candidate.py", "host-code": "console_shell.mjs"}[mutation]
        candidate[name] = (candidate[name][0] + b"\n# unaccepted change\n", candidate[name][1])
        if mutation == "host-code":
            names = sorted(set(updates._archived_framework_paths(previous)) | set(compiler.MAINTAINED_HOST_PATHS))
            source = [PackageFile(name, candidate[name][0].decode(),
                0o600 if name == ".env.example" else candidate[name][1]) for name in names]
            sums = "".join(f"{hashlib.sha256(item.as_bytes()).hexdigest()}  {item.path}\n" for item in source)
            source.append(PackageFile("SHA256SUMS", sums))
            document(candidate, "implementation-report.json",
                lambda value: value["framework"].update(baseline_digest=fingerprint(inventory(source))))
    elif mutation == "host-mode":
        candidate["console_shell.mjs"] = (candidate["console_shell.mjs"][0], 0o755)
    elif mutation == "origin":
        document(candidate, "implementation-report.json", lambda value: value["framework"]["previous"].update(source_digest="f" * 64))
    elif mutation == "baseline":
        document(candidate, "implementation-report.json", lambda value: value["framework"].update(baseline_digest="f" * 64))
    elif mutation == "receipt-selection":
        receipt["runtime_candidate"]["accepted"]["source_sha"] = "f" * 40
    else:
        target = None
    assert updates.maintained_host_transition(previous, candidate, receipt, runtime_target=target) is None
    assert updates.compatibility(previous, candidate, receipt, runtime_target=target)[1]


def test_maintained_host_refuses_selected_connect_before_stage_and_before_recovery(maintained_crossing, tmp_path):
    _, target, _, candidate, _, report = maintained_crossing
    directory, destination = maintained_installation(tmp_path, maintained_crossing)
    original_package = (directory / "package.zip").read_bytes()
    original_app = (destination / "app.py").read_bytes()
    private = destination / ".env"
    original_private = private.read_bytes()
    private.write_bytes(original_private + b'MOBKIT_CONNECT_BIN="/synthetic/selected-connect"\n')
    with pytest.raises(DraftConflict, match="already selected Connect"):
        updates.stage(directory, packed(candidate), report, runtime_target=target)
    assert not (directory / "updates").exists()
    assert (directory / "package.zip").read_bytes() == original_package
    private.write_bytes(original_private)
    staged = updates.stage(directory, packed(candidate), report, runtime_target=target)
    private.write_bytes(original_private + b'MOBKIT_CONNECT_BIN="/synthetic/selected-connect"\n')
    with pytest.raises(DraftConflict, match="already selected Connect"):
        updates.activate(directory, staged["update"]["update_id"])
    assert (destination / "app.py").read_bytes() == original_app
    assert (directory / "package.zip").read_bytes() == original_package
    assert read_receipt(directory)["update"]["status"] == "staged"


def test_maintained_host_closed_history_survives_bundle_and_compiler_drift_but_retry_does_not(maintained_crossing, tmp_path, monkeypatch):
    bundle, target, _, candidate, _, report = maintained_crossing
    directory, _ = maintained_installation(tmp_path, maintained_crossing)
    staged = updates.stage(directory, packed(candidate), report, runtime_target=target)
    identity = staged["update"]["update_id"]
    updates.activate(directory, identity)
    _, root, update = updates.transaction(directory, identity)
    updates.checkpoint(directory, root, update, status="completed", blockers=[], rollback_available=True,
        runtime_admission_possible=True)
    shutil.rmtree(bundle.root)
    def forbidden(*args, **kwargs):
        pytest.fail("Closed history cannot borrow a current compiler or artifact admission.")
    monkeypatch.setattr(compiler, "generate_package", forbidden)
    monkeypatch.setattr(compiler, "maintained_host_export", forbidden)
    assert updates.transaction(directory, identity)[2]["status"] == "completed"
    update = json.loads((root / "update.json").read_text())
    updates.checkpoint(directory, root, update, status="recovery-needed")
    with pytest.raises(DraftConflict, match="qualified host"):
        updates.retry_candidate(directory, identity)


@pytest.fixture
def maintained_independent_readers(maintained_crossing, tmp_path):
    bundle, target, previous, candidate, old_report, report = copy.deepcopy(maintained_crossing)
    reader = b"import zipfile\n# Independently retained self-contained package reader; never executed.\n"
    for files in (previous, candidate):
        files["runtime_candidates.py"] = (reader, 0o644)
        files.pop("sdk_wheel_integrity.py")
        names = set(updates._archived_framework_paths(previous))
        if files is candidate:
            names.update(compiler.MAINTAINED_HOST_PATHS)
        source = [PackageFile(name, files[name][0].decode(),
            0o600 if name == ".env.example" else files[name][1]) for name in sorted(names)]
        sums = "".join(f"{hashlib.sha256(item.as_bytes()).hexdigest()}  {item.path}\n" for item in source)
        source.append(PackageFile("SHA256SUMS", sums))
        files["SHA256SUMS"] = (sums.encode(), 0o644)
        document(files, "implementation-report.json", lambda value:
            value["framework"].update(baseline_digest=fingerprint(inventory(source))))
    old_report = json.loads(previous["implementation-report.json"][0])
    report = json.loads(candidate["implementation-report.json"][0])
    crossing = bundle, target, previous, candidate, old_report, report
    directory, _ = maintained_installation(tmp_path, crossing)
    update = updates.stage(directory, packed(candidate), report, runtime_target=target)["update"]
    assert update["blockers"] == [] and update["maintained_host_update"] is not None
    updates.activate(directory, update["update_id"])
    receipt, root, update = updates.transaction(directory, update["update_id"])
    descriptor = copy.deepcopy(update["maintained_host_update"])
    descriptor["required_supervisor_sources"].pop("launchkit/sdk_wheel_integrity.py")
    assert descriptor["required_supervisor_sources"]["launchkit/runtime_candidates.py"] != updates.digest(reader)
    update = updates.checkpoint(directory, root, update, status="completed", blockers=[],
        rollback_available=True, runtime_admission_possible=True, maintained_host_update=descriptor)
    return directory, root, update, previous, candidate


def test_completed_maintained_history_keeps_independent_reader_owners(maintained_independent_readers, monkeypatch):
    directory, root, update, _, _ = maintained_independent_readers
    retained = {name: (root / name).read_bytes() for name in
        ("previous.zip", "candidate.zip", "previous-receipt.json", "update.json")}
    def forbidden(*args, **kwargs):
        pytest.fail("Closed history cannot use current exports or candidate artifact admission.")
    monkeypatch.setattr(compiler, "generate_package", forbidden)
    monkeypatch.setattr(compiler, "maintained_host_export", forbidden)
    monkeypatch.setattr(runtime_selection, "candidate_selection", forbidden)
    observed, _, actual = updates.inspect_transaction(directory, update["update_id"])
    assert actual == update and observed["update"]["status"] == "completed"
    assert {name: (root / name).read_bytes() for name in retained} == retained


@pytest.mark.parametrize("defect", ["missing-worker-reader", "invalid-worker-reader", "missing-required-helper", "changed-refreshed-source"])
def test_completed_maintained_independent_readers_refuse_incomplete_closure(maintained_independent_readers, defect):
    directory, _, update, previous, candidate = maintained_independent_readers
    descriptor = copy.deepcopy(update["maintained_host_update"])
    sources = descriptor["required_supervisor_sources"]
    if defect == "missing-worker-reader":
        sources.pop("launchkit/runtime_candidates.py")
    elif defect == "invalid-worker-reader":
        sources["launchkit/runtime_candidates.py"] = "not-an-exact-source-hash"
    elif defect == "missing-required-helper":
        for files in (previous, candidate):
            files["runtime_candidates.py"] = (b"from sdk_wheel_integrity import read_wheel_members\n", 0o644)
        with pytest.raises(ValueError, match="lost its SDK wheel dependency"):
            updates._candidate_reader_helper_paths(candidate)
    else:
        sources["launchkit/templates_v2/app.py"] = "f" * 64
    assert updates.maintained_host_transition(previous, candidate, read_receipt(directory), archived=descriptor) is None


def test_maintained_host_requires_exact_frozen_supervisor_source_and_base_assets(maintained_crossing):
    from launchkit import deployment_service as service
    _, target, previous, candidate, _, _ = maintained_crossing
    descriptor = updates.maintained_host_transition(previous, candidate,
        {"runtime_candidate": runtime_selection.candidate_selection(target)}, runtime_target=target)
    source = service.supervisor_source_files()
    frozen = service.supervisor_metadata({}, source)
    update = {"runtime_dependency_update": None, "maintained_host_update": descriptor}
    service.require_candidate_update_protocol(frozen, update)
    for name in ("launchkit/deployment_updates.py", "launchkit/templates_v2/console_shell.mjs"):
        stale = copy.deepcopy(frozen)
        stale["host_files"][name] = "f" * 64
        with pytest.raises(DraftConflict, match="exact maintained Console"):
            service.require_candidate_update_protocol(stale, update)
    unsupported = copy.deepcopy(frozen)
    unsupported.pop("candidate_startup_protocol")
    with pytest.raises(DraftConflict, match="selected candidate runtime"):
        service.require_candidate_update_protocol(unsupported, update)


def test_completed_candidate_history_survives_removed_bundle_and_compiler_drift(crossing, tmp_path, monkeypatch):
    bundle, _, _, _, _, _ = crossing
    directory, destination, root, saved = completed(tmp_path, crossing)
    before = {name: (root / name).read_bytes() for name in ("previous.zip", "candidate.zip", "previous-receipt.json", "update.json")}
    def forbidden(*args, **kwargs):
        pytest.fail("closed candidate history must not compile, inspect artifacts, install or run")
    monkeypatch.setattr(compiler, "generate_package", forbidden)
    monkeypatch.setattr(compiler, "generate_runtime_refresh", forbidden)
    monkeypatch.setattr(runtime_candidates, "read_candidate_artifacts", forbidden)
    monkeypatch.setattr(runtime_selection, "candidate_selection", forbidden)
    monkeypatch.setattr(updates, "_candidate_artifact_expectation", forbidden)
    shutil.rmtree(bundle.root)
    receipt, same_root, observed = updates.transaction(directory, saved["update_id"])
    assert observed == saved and same_root == root
    assert receipt["runtime_candidate"] == saved["runtime_dependency_update"]["runtime_candidate"]
    assert {name: (root / name).read_bytes() for name in before} == before


@pytest.mark.parametrize("mutation", ["signed", "published", "sdk-origin", "payload", "metadata", "site",
    "receipt-sha", "binary-sha", "archive-sha", "archive-name", "native-source", "native-binding",
    "sdk-binding", "proof-attempt-bool", "receipt-selection", "descriptor-selection", "missing-path",
    "framework-binding", "extra-activation", "foreign-observation", "old-native"])
def test_closed_candidate_history_refuses_changed_member_or_admission_proof(crossing, tmp_path, mutation):
    directory, _, root, saved = completed(tmp_path, crossing)
    _, _, previous, candidate, _, _ = crossing
    receipt = read_receipt(directory)
    update, receipt, candidate = copy.deepcopy((saved, receipt, candidate))
    artifacts = update["runtime_artifacts"]
    sdk, native = artifacts["sdk"], artifacts["native"]
    descriptor = update["runtime_dependency_update"]
    if mutation in {"signed", "published"}: sdk[mutation] = True
    elif mutation == "sdk-origin": sdk["source"] = "official-release"
    elif mutation == "payload": sdk["payload_sha256"][next(iter(sdk["payload_sha256"]))] = "f" * 64
    elif mutation == "metadata": sdk["metadata_sha256"]["METADATA"] = "f" * 64
    elif mutation == "site": sdk["site_packages"] = "/foreign/site-packages"
    elif mutation in {"receipt-sha", "binary-sha", "archive-sha", "archive-name"}:
        key = {"receipt-sha": "receipt_sha256", "binary-sha": "binary_sha256", "archive-sha": "archive_sha256", "archive-name": "archive"}[mutation]
        native["binaries"]["rpc_gateway"][key] = "f" * 64
    elif mutation == "native-source": native["source"] = "official-release"
    elif mutation == "native-binding": native["candidate_binding"]["source_sha"] = "f" * 40
    elif mutation == "sdk-binding": sdk["sdk_wheel_sha256"] = "f" * 64
    elif mutation == "proof-attempt-bool": native["native_run_attempt"] = True
    elif mutation == "receipt-selection": receipt["runtime_candidate"]["artifact_root"] += "-foreign"
    elif mutation == "descriptor-selection": descriptor["runtime_candidate"]["tuple_path"] += ".foreign"
    elif mutation == "missing-path": descriptor["protected_paths"].remove("runtime_candidates.py")
    elif mutation == "framework-binding": document(candidate, "implementation-report.json", lambda value: value["framework"].pop("runtime_candidate"))
    elif mutation == "extra-activation": update["native_definition_activation"] = {"definition_revision": 2}
    elif mutation == "foreign-observation": update["native_definition_observation"] = {"definition_revision": 1, "package_sha256": "f" * 64}
    elif mutation == "old-native": update["runtime_files_retained"]["previous_native"]["source"] = "signed-candidate"
    result = updates._completed_runtime_dependency_transition(previous, candidate, receipt, update, root)
    assert not updates._same_toml(descriptor, result)


def test_recovery_cannot_borrow_closed_history_when_current_bundle_is_missing(crossing, tmp_path):
    bundle, _, _, _, _, _ = crossing
    directory, _, root, update = completed(tmp_path, crossing)
    updates.checkpoint(directory, root, update, status="recovery-needed")
    shutil.rmtree(bundle.root)
    with pytest.raises(DraftConflict, match="qualified host"):
        updates.transaction(directory, update["update_id"])
