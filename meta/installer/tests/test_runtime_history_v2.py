"""Current runtime history closes through retained source, without fresh admission.

Only inert packages, mocked worker phases and ordinary Python source are used.
No native executable, installed application, provider or service manager runs.
"""
import copy
import importlib
import importlib.util
import hashlib
import json
from pathlib import Path
import shutil
import sys

import pytest

from launchkit import compiler, generator, runtime_candidates, runtime_selection
from launchkit import deployment_service as service
from launchkit import deployment_updates as updates
from launchkit.deployment import read_receipt
from launchkit.deployment_worker import update_runtime
from launchkit.generator import PackageFile
from launchkit.projects import DraftConflict
from test_candidate_deployment_updates import (
    completed, crossing, document, installation, packed,
)
from test_deployment_updates import canonical_update_package, prepared
from historical_runtime_fixture import retained_runtime_v2
from test_runtime_dependency_updates import (
    canonical_fixture_packages, registry_fixture_packages, dependency_packages, dependency_staged,
    mock_dependency_runtime, native_fixture_proof,
)


def forbidden_admission(*args, **kwargs):
    pytest.fail("Closed runtime history must not compile or read live candidate artifacts.")


def forbid_fresh_admission(monkeypatch):
    monkeypatch.setattr(compiler, "generate_package", forbidden_admission)
    monkeypatch.setattr(compiler, "generate_runtime_refresh", forbidden_admission)
    monkeypatch.setattr(runtime_candidates, "read_candidate_artifacts", forbidden_admission)
    monkeypatch.setattr(runtime_candidates, "recheck_candidate_artifacts", forbidden_admission)
    monkeypatch.setattr(runtime_selection, "candidate_selection", forbidden_admission)
    monkeypatch.setattr(updates, "_candidate_artifact_expectation", forbidden_admission)


def closed_inputs(directory, root, update):
    receipt = read_receipt(directory)
    _, previous = updates.checked_package(root, "previous.zip", update["previous_package_sha256"])
    _, candidate = updates.checked_package(root, "candidate.zip", update["package_sha256"])
    return receipt, previous, candidate


V3_ADDITIONS = frozenset({"native_controller_review.py", "native_console_login.py",
    "controller_review.mjs", "controller_review.css"})
V4_ADDITIONS = frozenset({"launchkit/__init__.py", "launchkit/connect_application_finalizer.py",
    "launchkit/connect_application_policy.py", "launchkit/connect_installer_caller.py",
    "launchkit/connect_policy_input.py", "launchkit/connect_application_startup.py",
    "launchkit/connect_application_callbacks.py"})


@pytest.fixture
def current_packages(monkeypatch):
    # Importing the historical fixture module does not opt this suite into its
    # module-local autouse .46 version substitution.
    assert compiler.MOBKIT_VERSION == generator.MOBKIT_VERSION == "0.8.47"
    return dependency_packages(monkeypatch)


@pytest.fixture
async def ordinary_closed(tmp_path, monkeypatch, canonical_fixture_packages):
    _, directory, destination, _, private, identity = await dependency_staged(tmp_path)
    runtime, events = mock_dependency_runtime(monkeypatch, directory, destination,
        target_version="0.8.47")
    await update_runtime(runtime, "update", update_id=identity)
    receipt, root, update = updates.transaction(directory, identity)
    assert events == ["retain", "install", "native", "sdk", "storage", "doctor",
        "application-check", "native-final", "start"]
    assert update["status"] == "completed"
    assert receipt["update"] == update
    assert (destination / ".env").read_bytes() == private
    return directory, destination, root, update


def test_current_ordinary_and_explicit_candidate_use_distinct_v4_closures(current_packages, crossing):
    previous, candidate = current_packages
    ordinary = updates.runtime_dependency_transition(previous, candidate, {})
    _, target, old, selected, _, _ = crossing
    explicit = updates.runtime_dependency_transition(old, selected, {}, runtime_target=target)
    assert ordinary is not None and explicit is not None
    assert ordinary["version"] == explicit["version"] == 4
    assert ordinary["to_version"] == "0.8.47"
    assert explicit["to_version"] == "0.8.48"
    assert ordinary["definition_unchanged"] is explicit["definition_unchanged"] is True
    assert previous["config/mob.toml"] == candidate["config/mob.toml"]
    assert all(old[name] == selected[name] for name in compiler._RUNTIME_DESIGN_PATHS)
    assert len(updates._RUNTIME_FRAMEWORK_V2_PATHS) == 64
    assert len(updates._RUNTIME_FRAMEWORK_V3_PATHS) == 68
    assert set(ordinary["protected_paths"]) == set(updates._RUNTIME_FRAMEWORK_V3_PATHS) | V4_ADDITIONS
    assert set(explicit["protected_paths"]) == (
        set(ordinary["protected_paths"]) | updates.CANDIDATE_PATHS | {"sdk_wheel_integrity.py"})
    assert selected["sdk_wheel_integrity.py"] == (
        Path(updates.__file__).with_name("sdk_wheel_integrity.py").read_bytes(), 0o644)
    assert "runtime_candidate" not in ordinary
    assert explicit["runtime_candidate"] == runtime_selection.candidate_selection(target)
    assert ordinary["required_supervisor_sources"] == updates._runtime_history_supervisor_sources(4)
    assert explicit["required_supervisor_sources"] == ordinary["required_supervisor_sources"]
    copied = {name: "launchkit/templates_v2/" + name for name in V3_ADDITIONS}
    assert len(updates._RUNTIME_FRAMEWORK_V2_SUPERVISOR_PATHS) == 105
    assert len(updates._RUNTIME_FRAMEWORK_V2_COPIED_SOURCES) == 48
    assert len(updates._RUNTIME_FRAMEWORK_V3_SUPERVISOR_PATHS) == 109
    assert len(updates._RUNTIME_FRAMEWORK_V3_COPIED_SOURCES) == 52
    assert updates._RUNTIME_FRAMEWORK_V3_PATHS == updates._RUNTIME_FRAMEWORK_V2_PATHS | V3_ADDITIONS
    assert set(updates._RUNTIME_FRAMEWORK_V3_SUPERVISOR_PATHS) == (
        set(updates._RUNTIME_FRAMEWORK_V2_SUPERVISOR_PATHS) | set(copied.values()))
    assert updates._RUNTIME_FRAMEWORK_V3_COPIED_SOURCES == {
        **updates._RUNTIME_FRAMEWORK_V2_COPIED_SOURCES, **copied}
    for files, descriptor in ((candidate, ordinary), (selected, explicit)):
        for name, source in copied.items():
            assert files[name] == ((compiler.TEMPLATES_V2 / name).read_bytes(), 0o644)
            assert descriptor["required_supervisor_sources"][source] == hashlib.sha256(files[name][0]).hexdigest()
        assert files["launchkit/__init__.py"] == (b"", 0o644)
        helpers = V4_ADDITIONS - {"launchkit/__init__.py"}
        for name in helpers:
            source = Path(updates.__file__).parent / Path(name).name
            assert files[name] == (source.read_bytes(), 0o644)
            assert descriptor["required_supervisor_sources"][name] == hashlib.sha256(files[name][0]).hexdigest()
        assert set(descriptor["required_supervisor_sources"]) == (
            set(updates._RUNTIME_FRAMEWORK_V3_SUPERVISOR_PATHS) | helpers | {"launchkit/sdk_wheel_integrity.py"})
        assert descriptor["required_supervisor_sources"]["launchkit/sdk_wheel_integrity.py"] == (
            hashlib.sha256(Path(updates.__file__).with_name("sdk_wheel_integrity.py").read_bytes()).hexdigest())
        assert updates._archived_supervisor_sources(files, descriptor, 4) == descriptor["required_supervisor_sources"]
        assert updates._archived_supervisor_sources(files, descriptor, 3) is None
    assert updates._runtime_history_supervisor_sources() == updates._runtime_history_supervisor_sources(3)
    assert len(compiler.MAINTAINED_HOST_PATHS) == 7
    assert "console_shell.mjs" not in updates._RETAINED_FRAMEWORK_PATHS
    assert "toolkit-ui/index.js" not in updates._RETAINED_FRAMEWORK_PATHS


@pytest.mark.parametrize("version", [True, False, 0, 5, -1, "4", 4.0, None])
def test_framework_reader_version_is_strict_and_closed(current_packages, version):
    _, candidate = current_packages
    with pytest.raises(ValueError):
        updates._runtime_framework_paths(candidate, version)


def test_v1_inventory_stays_historical_without_admitting_current_export(current_packages):
    _, candidate = current_packages
    names = set(updates._runtime_framework_paths(candidate, 1))
    assert names == set(updates._RETAINED_FRAMEWORK_PATHS) | {"native_definition_continuity.py"}
    assert "console_shell.mjs" not in names and "toolkit-ui/index.js" not in names
    assert names < set(updates._runtime_framework_paths(candidate, 2))
    assert set(updates._runtime_framework_paths(candidate, 2)) < set(updates._runtime_framework_paths(candidate, 3))
    assert not updates._archived_framework_provenance(candidate, updates._runtime_framework_paths(candidate, 2))
    assert not updates._archived_framework_provenance(candidate, updates._runtime_framework_paths(candidate, 3))


@pytest.mark.parametrize("mode, additions", [
    ("none", set()),
    ("remote", {"knowledge/settings.json", "knowledge/manage.py", "knowledge/README.md", "knowledge/.gitignore"}),
    ("local", {"knowledge/settings.json", "knowledge/manage.py", "knowledge/README.md", "knowledge/.gitignore",
        "knowledge/compose.json", "knowledge/Dockerfile", "knowledge/Dockerfile.dockerignore"}),
])
@pytest.mark.parametrize("version", [2, 3, 4])
def test_versioned_knowledge_inventory_is_fixed_by_recorded_mode(current_packages, mode, additions, version):
    _, candidate = copy.deepcopy(current_packages)
    document(candidate, "deployment.json", lambda value: value["knowledge"].update(mode=mode))
    base = {2: updates._RUNTIME_FRAMEWORK_V2_PATHS, 3: updates._RUNTIME_FRAMEWORK_V3_PATHS,
        4: updates._RUNTIME_FRAMEWORK_V4_PATHS}[version]
    assert set(updates._runtime_framework_paths(candidate, version)) == set(base) | additions
    # A public application addition is never a new protected framework member.
    candidate["application/extra.py"] = (b"VERSION = 'accepted'\n", 0o644)
    assert "application/extra.py" not in updates._runtime_framework_paths(candidate, version)


@pytest.mark.parametrize("mode", ["future", True, None])
def test_framework_reader_refuses_unknown_knowledge_mode(current_packages, mode):
    _, candidate = copy.deepcopy(current_packages)
    document(candidate, "deployment.json", lambda value: value["knowledge"].update(mode=mode))
    with pytest.raises(ValueError):
        updates._runtime_framework_paths(candidate, 3)


@pytest.mark.parametrize("name", sorted(V3_ADDITIONS))
@pytest.mark.parametrize("mutation", ["missing", "bytes", "mode"])
def test_current_v4_refuses_missing_or_changed_review_member(current_packages, name, mutation):
    previous, candidate = copy.deepcopy(current_packages)
    if mutation == "missing":
        candidate.pop(name)
    elif mutation == "bytes":
        candidate[name] = (candidate[name][0] + b"\n# Changed copied resource\n", 0o644)
    else:
        candidate[name] = (candidate[name][0], 0o755)
    assert updates.runtime_dependency_transition(previous, candidate, {}) is None
    assert updates.compatibility(previous, candidate, {})[1]


@pytest.mark.parametrize("name", sorted(V3_ADDITIONS))
def test_archived_v4_refuses_changed_copied_review_source(current_packages, name):
    previous, candidate = current_packages
    descriptor = updates.runtime_dependency_transition(previous, candidate, {})
    assert descriptor is not None and descriptor["version"] == 4
    descriptor["required_supervisor_sources"]["launchkit/templates_v2/" + name] = "f" * 64
    assert updates._archived_supervisor_sources(candidate, descriptor, 4) is None


@pytest.mark.parametrize("name", sorted(V4_ADDITIONS))
@pytest.mark.parametrize("mutation", ["missing", "bytes", "mode"])
def test_current_v4_requires_exact_exported_startup_helpers(current_packages, name, mutation):
    previous, candidate = copy.deepcopy(current_packages)
    assert updates.runtime_dependency_transition(previous, candidate, {}) is not None
    if mutation == "missing":
        candidate.pop(name)
    else:
        content, mode = candidate[name]
        candidate[name] = (content + b"\n# Different helper\n", mode) if mutation == "bytes" else (content, 0o755)
    assert updates.runtime_dependency_transition(previous, candidate, {}) is None
    assert updates.compatibility(previous, candidate, {})[1]


@pytest.mark.parametrize("name", sorted(V4_ADDITIONS - {"launchkit/__init__.py"}))
def test_archived_v4_requires_each_exported_helper_source_pin(current_packages, name):
    previous, candidate = current_packages
    descriptor = updates.runtime_dependency_transition(previous, candidate, {})
    assert descriptor is not None and descriptor["version"] == 4
    descriptor["required_supervisor_sources"][name] = "f" * 64
    assert updates._archived_supervisor_sources(candidate, descriptor, 4) is None


@pytest.mark.parametrize("mutation", ["missing", "bytes", "mode"])
def test_archived_v4_requires_generated_empty_init_without_copying_supervisor_init(current_packages, mutation):
    previous, candidate = copy.deepcopy(current_packages)
    descriptor = updates.runtime_dependency_transition(previous, candidate, {})
    assert descriptor is not None and descriptor["version"] == 4
    assert Path(updates.__file__).with_name("__init__.py").read_bytes() != b""
    assert descriptor["required_supervisor_sources"]["launchkit/__init__.py"] != hashlib.sha256(b"").hexdigest()
    if mutation == "missing":
        candidate.pop("launchkit/__init__.py")
    else:
        candidate["launchkit/__init__.py"] = (b"# Different initializer\n", 0o644) if mutation == "bytes" else (b"", 0o755)
    assert updates._archived_supervisor_sources(candidate, descriptor, 4) is None


def test_unknown_future_compiler_export_needs_its_own_history_contract(current_packages, monkeypatch):
    previous, candidate = current_packages
    specification = json.loads(candidate["blueprint.json"][0])
    original = compiler.generate_package

    def future_export(*args, **kwargs):
        return [*original(*args, **kwargs), PackageFile("future_host.py", "# Future protected authority\n")]

    monkeypatch.setattr(compiler, "generate_package", future_export)
    future = canonical_update_package(specification)
    future = {name: (content, 0o755 if mode & 0o111 else 0o644)
        for name, (content, mode) in future.items()}
    assert updates.runtime_dependency_transition(previous, future, {}) is None
    assert updates.compatibility(previous, future, {})[1]


def test_current_compiler_relabeled_046_cannot_claim_the_historical_v1_closure(
        current_packages, monkeypatch):
    previous, current = current_packages
    specification = json.loads(current["blueprint.json"][0])
    # Deliberately create a negative counterexample: these current wider host
    # bytes are not evidence of either accepted historical .46 generation.
    monkeypatch.setattr(compiler, "MOBKIT_VERSION", "0.8.46")
    monkeypatch.setattr(generator, "MOBKIT_VERSION", "0.8.46")
    relabeled = canonical_update_package(specification)
    relabeled = {name: (content, 0o755 if mode & 0o111 else 0o644)
        for name, (content, mode) in relabeled.items()}
    assert json.loads(relabeled["deployment.json"][0])["mobkitVersion"] == "0.8.46"
    assert "console_shell.mjs" in relabeled and "toolkit-ui/index.js" in relabeled
    assert updates.runtime_dependency_transition(previous, relabeled, {}) is None
    assert updates.compatibility(previous, relabeled, {})[1]


def retained_v2_closed_inputs(tmp_path, monkeypatch, retained):
    """Construct inert original-v2 history with its original admission reader."""
    def original_export(value, **kwargs):
        retained.compiler.MOBKIT_VERSION = compiler.MOBKIT_VERSION
        return retained.compiler.generate_package(value, **kwargs)

    with monkeypatch.context() as original:
        original.setattr(compiler, "generate_package", original_export)
        previous, candidate = dependency_packages(original)
    for files, identity, digest in ((previous, "retained-v2-before", "c" * 64),
                                   (candidate, "retained-v2-after", "d" * 64)):
        report = json.loads(files["implementation-report.json"][0])
        report.update(build_id=identity, guide_version=1,
            blueprint_hash=report["framework"]["blueprint_digest"], brief_hash="b" * 64,
            verification={"digest": digest}, unresolved=[])
        files["implementation-report.json"] = (json.dumps(report).encode(), 0o644)
    descriptor = retained.updates.runtime_dependency_transition(previous, candidate, {})
    assert descriptor is not None and descriptor["version"] == 2
    assert len(descriptor["protected_paths"]) == 64
    assert descriptor["required_supervisor_sources"] == {
        name: value["sha256"] for name, value in retained.metadata["files"].items()}
    assert not V3_ADDITIONS.intersection(candidate)
    root = tmp_path / "synthetic-retained-v2-history"
    root.mkdir()
    archives = {"previous.zip": packed(previous), "candidate.zip": packed(candidate)}
    for name, content in archives.items():
        (root / name).write_bytes(content)
    old, new = (json.loads(files["implementation-report.json"][0]) for files in (previous, candidate))
    old_bound = {key: updates.digest(archives["previous.zip"]) if key == "package_sha256"
        else old[key] for key in updates.BOUND_KEYS}
    receipt = {key: updates.digest(archives["candidate.zip"]) if key == "package_sha256"
        else new[key] for key in updates.BOUND_KEYS}
    receipt["destination"] = str(tmp_path / "inert-v2-destination")
    (root / "previous-receipt.json").write_text(json.dumps(old_bound))
    update = {"status": "completed", "blockers": [], "rollback_available": False,
        "runtime_dependency_update": descriptor, "runtime_admission_possible": True,
        "native_definition_update": False, "host_instruction_update": False,
        "from_build_id": old["build_id"], "to_build_id": new["build_id"],
        "previous_source_digest": old["verification"]["digest"], "source_digest": new["verification"]["digest"],
        "previous_package_sha256": old_bound["package_sha256"], "package_sha256": receipt["package_sha256"],
        "guide_version": new["guide_version"], "blueprint_hash": new["blueprint_hash"], "brief_hash": new["brief_hash"],
        "runtime_files_retained": {"format": "mobkit-launch-runtime-retention-v1", "retained": True,
            "destination": receipt["destination"], "backup": str(root / "runtime-before"),
            "inventory_sha256": "e" * 64, "previous_native": native_fixture_proof("0.8.42")},
        "runtime_artifacts": {"native": native_fixture_proof("0.8.47"),
            "sdk": {"package": "meerkat-mobkit", "version": "0.8.47"}}}
    return previous, candidate, receipt, update, root


@pytest.mark.parametrize("mutation", [None, "copied-source", "missing-inventory"])
def test_retained_original_v2_closed_history_survives_v3_compiler(
        tmp_path, monkeypatch, retained_runtime_v2, mutation):
    previous, candidate, receipt, update, root = retained_v2_closed_inputs(
        tmp_path, monkeypatch, retained_runtime_v2)
    expected = copy.deepcopy(update["runtime_dependency_update"])
    assert updates._RUNTIME_FRAMEWORK_V2_PATHS == retained_runtime_v2.updates._RUNTIME_FRAMEWORK_V2_PATHS
    assert updates._RUNTIME_FRAMEWORK_V2_SUPERVISOR_PATHS == retained_runtime_v2.updates._RUNTIME_FRAMEWORK_V2_SUPERVISOR_PATHS
    assert updates._RUNTIME_FRAMEWORK_V2_COPIED_SOURCES == retained_runtime_v2.updates._RUNTIME_FRAMEWORK_V2_COPIED_SOURCES
    before = {name: (root / name).read_bytes()
        for name in ("previous.zip", "candidate.zip", "previous-receipt.json")}
    if mutation == "copied-source":
        update["runtime_dependency_update"]["required_supervisor_sources"]["launchkit/templates_v2/console_shell.mjs"] = "f" * 64
    elif mutation == "missing-inventory":
        update["runtime_dependency_update"]["protected_paths"].remove("console_shell.mjs")
    frozen = copy.deepcopy((previous, candidate, receipt, update))
    forbid_fresh_admission(monkeypatch)
    monkeypatch.setattr(retained_runtime_v2.compiler, "generate_package", forbidden_admission)
    actual = updates._completed_runtime_dependency_transition(previous, candidate, receipt, update, root)
    if mutation is None:
        assert actual == expected
    else:
        assert not updates._same_toml(update["runtime_dependency_update"], actual)
    assert (previous, candidate, receipt, update) == frozen
    assert {name: (root / name).read_bytes() for name in before} == before


async def test_ordinary_v4_closed_readback_uses_only_retained_bytes(ordinary_closed, monkeypatch):
    directory, destination, root, saved = ordinary_closed
    assert saved["runtime_dependency_update"]["version"] == 4
    before = {name: (root / name).read_bytes()
        for name in ("previous.zip", "candidate.zip", "previous-receipt.json", "update.json")}
    # Private settings can select a later Connect while historical readback
    # still authenticates the exact completed runtime crossing.
    with (destination / ".env").open("a") as stream:
        stream.write('MOBKIT_CONNECT_BIN="/future/connect"\n')
    forbid_fresh_admission(monkeypatch)
    receipt, same_root, observed = updates.transaction(directory, saved["update_id"])
    assert observed == saved and same_root == root
    assert receipt["update"] == saved
    assert {name: (root / name).read_bytes() for name in before} == before


@pytest.mark.parametrize("mutation", [None, "copied-source", "new-helper-inventory", "new-helper-source"])
async def test_fixed_v3_closed_contract_remains_readable_without_successor_helpers(
        ordinary_closed, monkeypatch, mutation):
    # This is an inert fixed-v3 history fixture, not historical native admission.
    directory, _, root, saved = ordinary_closed
    receipt, previous, candidate = copy.deepcopy(closed_inputs(directory, root, saved))
    update = copy.deepcopy(saved)
    descriptor = update["runtime_dependency_update"]
    names = updates._runtime_framework_paths(candidate, 3)
    for name in V4_ADDITIONS:
        candidate.pop(name)
    inventory = {name: {"sha256": updates.digest(candidate[name][0]),
        "mode": 0o600 if name == ".env.example" else candidate[name][1]} for name in names}
    sums = "".join(f"{inventory[name]['sha256']}  {name}\n" for name in names).encode()
    inventory["SHA256SUMS"] = {"sha256": updates.digest(sums), "mode": 0o644}
    candidate["SHA256SUMS"] = (sums, 0o644)
    baseline = updates.digest(json.dumps(inventory, ensure_ascii=True, sort_keys=True,
        separators=(",", ":")).encode())
    document(candidate, "implementation-report.json", lambda report: report["framework"].update(
        baseline_digest=baseline, current_compiler_baseline_digest=baseline))
    descriptor.update(version=3, protected_paths=names, accepted_baseline_digest=baseline,
        compiler_baseline_digest=baseline, required_supervisor_sources={name: value
            for name, value in descriptor["required_supervisor_sources"].items()
            if name in updates._RUNTIME_FRAMEWORK_V3_SUPERVISOR_PATHS})
    update["package_sha256"] = receipt["package_sha256"] = updates.digest(packed(candidate))
    expected = copy.deepcopy(descriptor)
    if mutation == "copied-source":
        descriptor["required_supervisor_sources"]["launchkit/templates_v2/app.py"] = "f" * 64
    elif mutation == "new-helper-inventory":
        descriptor["protected_paths"].append("launchkit/connect_application_startup.py")
    elif mutation == "new-helper-source":
        descriptor["required_supervisor_sources"]["launchkit/connect_application_startup.py"] = "f" * 64
    frozen = copy.deepcopy((previous, candidate, receipt, update))
    forbid_fresh_admission(monkeypatch)
    result = updates._completed_runtime_dependency_transition(previous, candidate, receipt, update, root)
    assert result == expected if mutation is None else not updates._same_toml(descriptor, result)
    assert (previous, candidate, receipt, update) == frozen


@pytest.mark.parametrize("name", sorted(V4_ADDITIONS))
async def test_closed_v4_rebaselining_cannot_substitute_exported_startup_source(
        ordinary_closed, monkeypatch, name):
    directory, _, root, saved = ordinary_closed
    receipt, previous, candidate = copy.deepcopy(closed_inputs(directory, root, saved))
    update = copy.deepcopy(saved)
    content, mode = candidate[name]
    candidate[name] = (content + b"\n# Different retained helper\n", mode)
    names = updates._runtime_framework_paths(candidate, 4)
    inventory = {item: {"sha256": updates.digest(candidate[item][0]),
        "mode": 0o600 if item == ".env.example" else candidate[item][1]} for item in names}
    sums = "".join(f"{inventory[item]['sha256']}  {item}\n" for item in names).encode()
    inventory["SHA256SUMS"] = {"sha256": updates.digest(sums), "mode": 0o644}
    candidate["SHA256SUMS"] = (sums, 0o644)
    baseline = updates.digest(json.dumps(inventory, ensure_ascii=True, sort_keys=True,
        separators=(",", ":")).encode())
    document(candidate, "implementation-report.json", lambda report: report["framework"].update(
        baseline_digest=baseline, current_compiler_baseline_digest=baseline))
    descriptor = update["runtime_dependency_update"]
    descriptor.update(accepted_baseline_digest=baseline, compiler_baseline_digest=baseline)
    update["package_sha256"] = receipt["package_sha256"] = updates.digest(packed(candidate))
    assert updates._archived_framework_provenance(candidate, names)
    forbid_fresh_admission(monkeypatch)
    assert updates._completed_runtime_dependency_transition(previous, candidate, receipt, update, root) is None


def test_explicit_candidate_v4_closes_after_retained_proof_without_live_bundle(crossing, tmp_path, monkeypatch):
    bundle, _, _, _, _, _ = crossing
    directory, _, root, saved = completed(tmp_path, crossing)
    assert saved["runtime_dependency_update"]["version"] == 4
    assert saved["runtime_dependency_update"]["to_version"] == "0.8.48"
    before = {name: (root / name).read_bytes()
        for name in ("previous.zip", "candidate.zip", "previous-receipt.json", "update.json")}
    shutil.rmtree(bundle.root)
    forbid_fresh_admission(monkeypatch)
    receipt, same_root, observed = updates.transaction(directory, saved["update_id"])
    assert observed == saved and same_root == root
    assert receipt["runtime_candidate"] == saved["runtime_dependency_update"]["runtime_candidate"]
    assert {name: (root / name).read_bytes() for name in before} == before


def changed_history(saved, candidate, mutation):
    saved, candidate = copy.deepcopy((saved, candidate))
    descriptor = saved["runtime_dependency_update"]
    if mutation == "version-bool":
        descriptor["version"] = True
    elif mutation == "version-unknown":
        descriptor["version"] = 5
    elif mutation == "version-v3":
        descriptor["version"] = 3
    elif mutation == "accepted-baseline":
        descriptor["accepted_baseline_digest"] = "f" * 64
    elif mutation == "compiler-baseline":
        document(candidate, "implementation-report.json",
            lambda value: value["framework"].update(current_compiler_baseline_digest="f" * 64))
    elif mutation == "framework-mode":
        candidate["console_shell.mjs"] = (candidate["console_shell.mjs"][0], 0o755)
    elif mutation == "missing-member":
        candidate.pop("toolkit-ui/index.js")
    elif mutation == "missing-inventory":
        descriptor["protected_paths"].remove("console_shell.mjs")
    elif mutation == "extra-inventory":
        descriptor["protected_paths"].append("future_host.py")
    elif mutation == "source-extra":
        descriptor["required_supervisor_sources"]["launchkit/future_reader.py"] = "f" * 64
    return saved, candidate


HISTORY_MUTATIONS = ["version-bool", "version-unknown", "version-v3", "accepted-baseline", "compiler-baseline",
    "framework-mode", "missing-member", "missing-inventory", "extra-inventory", "source-extra"]


@pytest.mark.parametrize("mutation", HISTORY_MUTATIONS)
async def test_closed_ordinary_v4_refuses_changed_source_or_descriptor(ordinary_closed, mutation):
    directory, _, root, saved = ordinary_closed
    receipt, previous, candidate = closed_inputs(directory, root, saved)
    changed, candidate = changed_history(saved, candidate, mutation)
    result = updates._completed_runtime_dependency_transition(previous, candidate, receipt, changed, root)
    assert not updates._same_toml(changed["runtime_dependency_update"], result)


@pytest.mark.parametrize("mutation", HISTORY_MUTATIONS)
def test_closed_candidate_v4_refuses_changed_source_or_descriptor(crossing, tmp_path, mutation):
    directory, _, root, saved = completed(tmp_path, crossing)
    receipt, previous, candidate = closed_inputs(directory, root, saved)
    changed, candidate = changed_history(saved, candidate, mutation)
    result = updates._completed_runtime_dependency_transition(previous, candidate, receipt, changed, root)
    assert not updates._same_toml(changed["runtime_dependency_update"], result)


async def test_connect_selection_refuses_ordinary_v4_stage_before_journal_or_copy(
        tmp_path, canonical_fixture_packages):
    host, directory, destination, previous, _ = await prepared(tmp_path)
    payload, _ = await host.implementation.package(host.guide.identity, host.implementation.identity)
    candidate = updates.package_files(payload)
    report = json.loads(candidate["implementation-report.json"][0])
    original = (directory / "package.zip").read_bytes()
    original_host = (destination / "app.py").read_bytes()
    with (destination / ".env").open("a") as stream:
        stream.write('MOBKIT_CONNECT_BIN="/selected/connect"\n')
    with pytest.raises(DraftConflict, match="Connect|CONNECT"):
        updates.stage(directory, payload, report)
    assert not (directory / "updates").exists()
    assert (directory / "package.zip").read_bytes() == original
    assert (destination / "app.py").read_bytes() == original_host
    assert read_receipt(directory)["build_id"] == previous["build_id"]


async def test_connect_selected_after_staging_refuses_before_source_copy(
        tmp_path, canonical_fixture_packages):
    _, directory, destination, _, _, identity = await dependency_staged(tmp_path)
    original_host = (destination / "app.py").read_bytes()
    original_package = (directory / "package.zip").read_bytes()
    with (destination / ".env").open("a") as stream:
        stream.write('MOBKIT_CONNECT_BIN="/selected/connect"\n')
    with pytest.raises(DraftConflict, match="Connect|CONNECT"):
        updates.activate(directory, identity)
    assert (directory / "package.zip").read_bytes() == original_package
    assert (destination / "app.py").read_bytes() == original_host
    assert (destination / ".mobkit/state/native.db").read_bytes() == b"native durable sessions"


@pytest.mark.parametrize("after_stage", [False, True])
def test_connect_selection_refuses_explicit_candidate_stage_or_source_copy(crossing, tmp_path, after_stage):
    _, target, _, candidate, _, report = crossing
    directory, destination = installation(tmp_path, crossing)
    original_host = (destination / "app.py").read_bytes()
    original_package = (directory / "package.zip").read_bytes()
    if after_stage:
        staged = updates.stage(directory, packed(candidate), report, runtime_target=target)
    with (destination / ".env").open("a") as stream:
        stream.write('MOBKIT_CONNECT_BIN="/selected/connect"\n')
    with pytest.raises(DraftConflict, match="Connect|CONNECT"):
        if after_stage:
            updates.activate(directory, staged["update"]["update_id"])
        else:
            updates.stage(directory, packed(candidate), report, runtime_target=target)
    assert (directory / "package.zip").read_bytes() == original_package
    assert (destination / "app.py").read_bytes() == original_host
    assert "runtime_candidate" not in read_receipt(directory)
    assert (destination / ".mobkit/state/native.db").read_bytes() == b"inert native state"


@pytest.mark.parametrize("late", [False, True])
async def test_connect_cannot_enter_worker_install_or_start_and_retry_has_no_second_effect(
        tmp_path, monkeypatch, canonical_fixture_packages, late):
    _, directory, destination, _, _, identity = await dependency_staged(tmp_path)
    runtime, events = mock_dependency_runtime(monkeypatch, directory, destination,
        target_version="0.8.47")
    if late:
        command = runtime.command

        async def selected_during_application_check(args, phase, *other, **kwargs):
            result = await command(args, phase, *other, **kwargs)
            if args[-1] == "--application-check":
                with (destination / ".env").open("a") as stream:
                    stream.write('MOBKIT_CONNECT_BIN="/selected/connect"\n')
            return result

        monkeypatch.setattr(runtime, "command", selected_during_application_check)
    else:
        with (destination / ".env").open("a") as stream:
            stream.write('MOBKIT_CONNECT_BIN="/selected/connect"\n')
    with pytest.raises((DraftConflict, RuntimeError), match="Connect|CONNECT"):
        await update_runtime(runtime, "update", update_id=identity)
    assert "start" not in events
    assert events.count("install") == (1 if late else 0)
    assert events.count("retain") == (1 if late else 0)
    current_native = (destination / ".mobkit/state/native.db").read_bytes()
    current_application = (destination / "data/ledger.db").read_bytes()
    retry, retry_events = mock_dependency_runtime(monkeypatch, directory, destination,
        target_version="0.8.47")
    with pytest.raises((DraftConflict, RuntimeError), match="Connect|CONNECT"):
        await update_runtime(retry, "recover-current" if late else "update", update_id=identity)
    assert not retry_events
    assert (destination / ".mobkit/state/native.db").read_bytes() == current_native
    assert (destination / "data/ledger.db").read_bytes() == current_application


@pytest.mark.parametrize("mutation", ["missing-reader", "stale-reader", "stale-worker", "extra-source",
    "missing-login-adapter", "stale-review-asset", "missing-startup-helper", "stale-startup-helper"])
def test_ordinary_v4_requires_exact_frozen_supervisor_even_without_candidate(current_packages, mutation):
    previous, candidate = current_packages
    descriptor = updates.runtime_dependency_transition(previous, candidate, {})
    frozen = service.supervisor_metadata({}, service.supervisor_source_files())
    update = {"runtime_dependency_update": descriptor, "maintained_host_update": None}
    assert "runtime_candidate" not in descriptor
    service.require_candidate_update_protocol(frozen, update)
    if mutation == "missing-reader":
        frozen["host_files"].pop("launchkit/deployment_updates.py")
    elif mutation == "stale-reader":
        frozen["host_files"]["launchkit/deployment_updates.py"] = "f" * 64
    elif mutation == "stale-worker":
        frozen["host_files"]["launchkit/deployment_worker.py"] = "f" * 64
        frozen["update_worker_sha256"] = frozen["candidate_worker_sha256"] = "f" * 64
    elif mutation == "missing-login-adapter":
        frozen["host_files"].pop("launchkit/templates_v2/native_console_login.py")
    elif mutation == "stale-review-asset":
        frozen["host_files"]["launchkit/templates_v2/controller_review.mjs"] = "f" * 64
    elif mutation == "missing-startup-helper":
        frozen["host_files"].pop("launchkit/connect_application_startup.py")
    elif mutation == "stale-startup-helper":
        frozen["host_files"]["launchkit/connect_application_startup.py"] = "f" * 64
    else:
        descriptor["required_supervisor_sources"]["launchkit/future_reader.py"] = "f" * 64
        frozen["host_files"]["launchkit/future_reader.py"] = "f" * 64
    with pytest.raises(DraftConflict):
        service.require_candidate_update_protocol(frozen, update)


@pytest.mark.parametrize("version", [True, False, 0, 5, -1, "4", 4.0, None])
def test_background_source_protocol_refuses_unknown_or_noninteger_history(current_packages, version):
    previous, candidate = current_packages
    descriptor = updates.runtime_dependency_transition(previous, candidate, {})
    frozen = service.supervisor_metadata({}, service.supervisor_source_files())
    descriptor["version"] = version
    with pytest.raises(DraftConflict, match="supported frozen history contract"):
        service.require_candidate_update_protocol(frozen,
            {"runtime_dependency_update": descriptor, "maintained_host_update": None})


def test_exact_new_snapshot_imports_its_own_compiler_and_current_history_reader(
        current_packages, tmp_path, monkeypatch):
    previous, candidate = current_packages
    source = service.supervisor_source_files()
    frozen = service.supervisor_metadata({}, source)
    descriptor = updates.runtime_dependency_transition(previous, candidate, {})
    service.require_candidate_update_protocol(frozen,
        {"runtime_dependency_update": descriptor, "maintained_host_update": None})
    snapshot = tmp_path / "qualified-supervisor"
    for name, content in source.items():
        path = snapshot / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(content)
    namespace = "_qualified_runtime_history_v2"
    package = snapshot / "launchkit"
    spec = importlib.util.spec_from_file_location(namespace, package / "__init__.py",
        submodule_search_locations=[str(package)])
    module = importlib.util.module_from_spec(spec)
    monkeypatch.setitem(sys.modules, namespace, module)
    spec.loader.exec_module(module)
    forbid_fresh_admission(monkeypatch)
    try:
        snapshot_compiler = importlib.import_module(namespace + ".compiler")
        snapshot_updates = importlib.import_module(namespace + ".deployment_updates")
        assert Path(snapshot_compiler.__file__).parent == package
        assert Path(snapshot_updates.__file__).parent == package
        exported = snapshot_compiler.generate_package(json.loads(candidate["blueprint.json"][0]))
        assert {item.path: (item.as_bytes(), 0o755 if item.mode & 0o111 else 0o644)
            for item in exported} == {name: entry for name, entry in candidate.items()
                if name != "implementation-report.json"}
        assert snapshot_updates.runtime_dependency_transition(previous, candidate, {}) == descriptor
    finally:
        for name in list(sys.modules):
            if name.startswith(namespace + "."):
                sys.modules.pop(name, None)


async def test_selected_connect_refuses_before_cold_native_registry_observation(
        tmp_path, monkeypatch, registry_fixture_packages):
    _, directory, destination, _, _, identity = await dependency_staged(tmp_path)
    descriptor = read_receipt(directory)["update"]["runtime_dependency_update"]
    assert descriptor["version"] == 4
    assert descriptor["native_registry_update"] is True
    with (destination / ".env").open("a") as stream:
        stream.write('MOBKIT_CONNECT_BIN="/selected/connect"\n')
    runtime, events = mock_dependency_runtime(monkeypatch, directory, destination,
        registry_epoch={"revision": 1}, target_version="0.8.47")
    with pytest.raises(DraftConflict, match="Connect|CONNECT"):
        await update_runtime(runtime, "update", update_id=identity)
    assert events == [], "Selected Connect must refuse before any cold native connection"


async def test_late_selected_connect_refuses_before_new_registry_activation(
        tmp_path, monkeypatch, registry_fixture_packages):
    _, directory, destination, _, _, identity = await dependency_staged(tmp_path)
    descriptor = read_receipt(directory)["update"]["runtime_dependency_update"]
    assert descriptor["version"] == 4
    assert descriptor["native_registry_update"] is True
    runtime, events = mock_dependency_runtime(monkeypatch, directory, destination,
        registry_epoch={"revision": 1}, target_version="0.8.47")
    command = runtime.command

    async def select_after_application_check(args, phase, *other, **kwargs):
        result = await command(args, phase, *other, **kwargs)
        if args[-1] == "--application-check":
            with (destination / ".env").open("a") as stream:
                stream.write('MOBKIT_CONNECT_BIN="/selected/connect"\n')
        return result

    monkeypatch.setattr(runtime, "command", select_after_application_check)
    with pytest.raises((DraftConflict, RuntimeError), match="Connect|CONNECT"):
        await update_runtime(runtime, "update", update_id=identity)
    assert "start" not in events
    assert (destination / ".mobkit/state/native.db").read_bytes().endswith(
        b"; current runtime observation")
    assert (destination / "data/ledger.db").read_bytes().endswith(
        b"; current application record")
    assert "activation" not in events, "A new native activation must refuse a newly selected Connect"
