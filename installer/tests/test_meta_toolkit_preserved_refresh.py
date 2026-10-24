"""Product successor source/history and inert byte-retention controls.

The selected records and SDK/native bytes are synthetic, never qualification.
No selected interpreter, SDK, native program or application is executed.
"""
from copy import deepcopy
import json
import os
from pathlib import Path
import tomllib
import uuid
import zipfile

import pytest

from launchkit import compiler, deployment, deployment_updates as updates
from launchkit import runtime_dependencies as dependencies, runtime_selection as selection
from launchkit.implementation_workspace import ImplementationWorkspace, PROTECTED_APPLICATION_PATHS
from launchkit.projects import fingerprint
from launchkit.templates import install_candidate as adapter
from test_candidate_deployment_updates import installation, packed
from test_meta_toolkit_initial_package import inputs, setup_spec
from test_meta_toolkit_runtime_selection import no_product_execution, json_bytes, digest
import test_meta_toolkit_runtime_selection as specimens
from test_meta_toolkit_runtime_update import catalog_target
from test_reading_runtime_refresh import report


@pytest.fixture
def products(tmp_path, monkeypatch):
    generators = []
    results = []
    for index in range(2):
        owned = tmp_path / ("inert selected product " + str(index))
        owned.mkdir(mode=0o700)
        with monkeypatch.context() as changed:
            changed.setattr(specimens, "GATEWAY_BYTES", ("INERT selected gateway " + str(index) + "\n").encode())
            source = specimens.product.__wrapped__(owned)
            value = next(source)
            generators.append(source)
            catalog_target(value)
            results.append(value)
    yield results
    for source in generators:
        try:
            next(source)
        except StopIteration:
            pass


@pytest.fixture
def crossing(tmp_path, products, inputs):
    old_product, new_product = products
    spec = setup_spec()
    old = ImplementationWorkspace(tmp_path / "original accepted workspace", spec)
    old.seed(runtime_target=old_product.target)
    # The host source is genuinely private; the baseline/export uses the
    # established canonical public text mode while preserving exact bytes.
    for path in inputs.values():
        path.chmod(0o600)
    old.adopt_application_inputs(inputs, require_current=lambda: None)
    ordinary = {"application/__init__.py": "raise RuntimeError('inert app must never execute')\n",
        "application/domain.py": "# Exact accepted application logic.\n",
        "requirements.application.txt": "fixture-dependency==1.0.0\n"}
    for name, value in ordinary.items():
        old.write_file(name, value)
    build = str(uuid.uuid4())
    with old._locked():
        state = old._state()
        files = old._files(state)
        snapshot = old._framework_snapshot(state, files, build_id=build,
            accepted_source_digest=old._digest(files))
    previous = {item.path: (item.as_bytes(), 0o755 if item.mode & 0o111 else 0o644)
        for item in old.export_files(require_verified=False)}
    old_report = report(spec, old.framework_provenance(), build, snapshot.previous_source_digest)
    previous["implementation-report.json"] = (json_bytes(old_report), 0o644)
    successor = ImplementationWorkspace(tmp_path / "successor workspace", spec)
    successor.seed(snapshot, runtime_refresh=True, runtime_target=new_product.target)
    for name, value in ordinary.items():
        successor.write_file(name, value)
    candidate = {item.path: (item.as_bytes(), 0o755 if item.mode & 0o111 else 0o644)
        for item in successor.export_files(require_verified=False)}
    new_report = report(spec, successor.framework_provenance(), str(uuid.uuid4()), "b" * 64)
    candidate["implementation-report.json"] = (json_bytes(new_report), 0o644)
    old_product.successor_workspace = successor
    return old_product, new_product.target, previous, candidate, old_report, new_report


def original_receipt(previous):
    return {"runtime_candidate": json.loads(previous["runtime-candidate.json"][0])}


def product_proof(target, expected):
    return {"source": "meta-toolkit-product", "version": target.meta.version,
        "platform": selection.selected_runtime_platform(target), "assembly_sha256": target.assembly.sha256,
        "catalog_sha256": target.model_catalog.sha256, "sdk": deepcopy(expected["sdk"]),
        "binaries": {"toolkit_gateway": {"binary_sha256": expected["native"]["binary_sha256"]}},
        **{key: value for key, value in expected["native"].items() if key != "binary_sha256"}}


def staged(tmp_path, crossing):
    _, target, previous, candidate, _, new_report = crossing
    directory, destination = installation(tmp_path, crossing)
    receipt = updates.read_receipt(directory)
    receipt.update(original_receipt(previous))
    updates.write_private(directory / "receipt.json", receipt)
    status = updates.stage(directory, packed(candidate), new_report, runtime_target=target)
    assert status["update"]["blockers"] == []
    return directory, destination, status["update"]["update_id"]


def completed(tmp_path, crossing):
    old_product, target, _, _, _, _ = crossing
    directory, destination, identity = staged(tmp_path, crossing)
    updates.activate(directory, identity)
    _, root, current = updates.transaction(directory, identity)
    descriptor = current["runtime_dependency_update"]
    before = product_proof(old_product.target, descriptor["previous_runtime_candidate_artifacts"])
    after = product_proof(target, descriptor["runtime_candidate_artifacts"])
    updates.checkpoint(directory, root, current, status="completed", blockers=[], rollback_available=False,
        runtime_admission_possible=True,
        runtime_files_retained={"format": "mobkit-launch-runtime-retention-v1", "retained": True,
            "inventory_sha256": "e" * 64, "destination": str(destination), "backup": str(root / "runtime-before"),
            "previous_native": before}, runtime_artifacts={"sdk": after["sdk"], "native": after})
    return directory, destination, root, identity, descriptor


def test_actual_archive_product_successor_preserves_policy_viewer_access_and_settings(crossing, tmp_path):
    old_product, target, previous, candidate, _, _ = crossing
    archive = deployment.package_files(packed(candidate))
    assert archive == candidate
    descriptor = updates.runtime_dependency_transition(previous, archive, original_receipt(previous), runtime_target=target)
    assert descriptor is not None and descriptor["version"] == 7
    assert descriptor["previous_framework_version"] == 6
    assert descriptor["previous_runtime_candidate"] == selection.candidate_selection(old_product.target)
    assert descriptor["runtime_candidate"] == selection.candidate_selection(target)
    assert descriptor["previous_runtime_candidate"] != descriptor["runtime_candidate"]
    assert descriptor["protected_application_paths"] == sorted(name for name in PROTECTED_APPLICATION_PATHS if name in previous)
    for name in descriptor["protected_application_paths"]:
        assert candidate[name] == previous[name]
        assert descriptor["protected_application_inventory"][name] == {"sha256": digest(previous[name][0]), "mode": 0o644}
    assert descriptor["accepted_baseline_digest"] != descriptor["compiler_baseline_digest"]
    directory, destination, identity = staged(tmp_path, crossing)
    private_names = (".env", ".mobkit/state/native.db", "data/ledger.db", "workspace/user.txt")
    private = {name: (destination / name).read_bytes() for name in private_names}
    updates.activate(directory, identity)
    assert updates.read_receipt(directory)["runtime_candidate"] == selection.candidate_selection(target)
    assert private == {name: (destination / name).read_bytes() for name in private_names}
    for name in descriptor["protected_application_paths"] + ["application/domain.py", "requirements.application.txt"]:
        assert (destination / name).read_bytes() == previous[name][0]


def test_actual_product_refresh_preserves_host_adopted_announcement_declarations(tmp_path, products, inputs):
    name = compiler.DECLARATIONS_PATH
    source = tmp_path / "selected-announcements.json"
    source.write_bytes(b'{"revision":7,"sources":[],"subscriptions":[]}\r\n')
    selected = {**inputs, name: source}
    value = crossing.__wrapped__(tmp_path, products, selected)
    _, _, previous, candidate, _, _ = value
    assert previous[name] == candidate[name] == (source.read_bytes(), 0o644)
    directory, destination, identity = staged(tmp_path, value)
    _, _, update = updates.transaction(directory, identity)
    assert name in update["runtime_dependency_update"]["protected_application_paths"]
    updates.activate(directory, identity)
    assert (destination / name).read_bytes() == source.read_bytes()


@pytest.mark.parametrize("predecessor", ["compiler-default", "customized"])
def test_installed_update_to_a_refreshed_connect_product_admits_its_console(
        tmp_path, products, inputs, monkeypatch, predecessor):
    """The update authority admits the refreshed Console: a regenerated default or a kept design."""
    real = compiler._console
    spec = compiler._spec(setup_spec())
    default = real(spec)
    earlier = default if predecessor == "compiler-default" else default + "[rail]\nvisible = false\n"
    old_target = products[0].target
    # The predecessor's compiler emitted this Console; the current one refreshes it.
    monkeypatch.setattr(compiler, "_console", lambda value, *, runtime_target=None, legacy_ordinary=False:
        earlier if runtime_target is old_target else real(value, runtime_target=runtime_target,
            legacy_ordinary=legacy_ordinary))
    selected = crossing.__wrapped__(tmp_path, products, inputs)
    old_product, target, previous, candidate, _, _ = selected
    assert previous["config/console.toml"] == (earlier.encode(), 0o644)
    expected = (real(spec, runtime_target=target) if predecessor == "compiler-default" else earlier).encode()
    assert candidate["config/console.toml"] == (expected, 0o644)
    retained = old_product.successor_workspace.framework_provenance()["retained_template_paths"]
    assert ("config/console.toml" in retained) is (predecessor == "customized")
    receipt = original_receipt(previous)
    changes, blockers = updates.compatibility(previous, candidate, receipt, runtime_target=target)
    assert blockers == []
    assert ({"path": "config/console.toml", "change": "changed"} in changes) is (predecessor == "compiler-default")
    # The Console change is admitted only as a product framework path of this refresh.
    descriptor = updates.runtime_dependency_transition(previous, candidate, receipt, runtime_target=target)
    assert descriptor is not None and "config/console.toml" in descriptor["protected_paths"]
    directory, destination, identity = staged(tmp_path, selected)
    updates.activate(directory, identity)
    installed = (destination / "config/console.toml").read_bytes()
    assert installed == expected
    buttons = tomllib.loads(installed.decode()).get("sidebar", {}).get("buttons", [])
    assert [row["id"] for row in buttons] == (["connect-setup"] if predecessor == "compiler-default" else [])


def test_refresh_of_a_product_without_the_connect_route_keeps_its_console(products):
    spec = setup_spec()
    spec["acquisition"] = None
    del spec["agent_classes"][0]["tools"]["rust_bundles"]
    accepted = {item.path: item for item in compiler.generate_package(spec, runtime_target=products[0].target,
        legacy_ordinary=True)}
    refreshed = {item.path: item for item in compiler.generate_runtime_refresh(spec, list(accepted.values()),
        runtime_target=products[1].target, protected_application_paths=())}
    console = refreshed["config/console.toml"].as_bytes()
    assert console == accepted["config/console.toml"].as_bytes() == compiler._console(compiler._spec(spec)).encode()
    assert "sidebar" not in tomllib.loads(console.decode())
    assert "applicationInventory" not in json.loads(refreshed["deployment.json"].content)
    assert "native-inventory.example.json" not in refreshed
    assert "Create application/native-inventory.json" not in refreshed["README.md"].content
    assert "In-place updates are currently unavailable" not in refreshed["README.md"].content


@pytest.mark.parametrize("defect", ["policy", "viewer", "access", "mode", "new-app", "app", "pin", "paths", "receipt", "previous"])
def test_current_product_refresh_refuses_non_runtime_changes(crossing, defect):
    _, target, previous, candidate, _, _ = crossing
    previous, candidate, receipt = deepcopy(previous), deepcopy(candidate), original_receipt(previous)
    if defect in {"policy", "viewer", "access", "mode", "app", "pin"}:
        name = {"policy": "application/tool-policy.json", "viewer": "application/console.json",
            "access": "config/access.toml", "mode": "application/configuration.json",
            "app": "application/domain.py", "pin": "requirements.application.txt"}[defect]
        content, mode = candidate[name]
        candidate[name] = (content if defect == "mode" else content + b"changed\n", 0o600 if defect == "mode" else mode)
    elif defect == "new-app":
        candidate["application/extra.py"] = (b"# New behavior is not runtime refresh.\n", 0o644)
    elif defect == "receipt":
        receipt["runtime_candidate"]["assembly"]["sha256"] = "f" * 64
    elif defect == "previous":
        previous["runtime-candidate.json"] = (b"{}", 0o644)
    else:
        value = json.loads(candidate["implementation-report.json"][0])
        value["framework"]["protected_application_paths"].remove("config/access.toml")
        candidate["implementation-report.json"] = (json_bytes(value), 0o644)
    assert updates.runtime_dependency_transition(previous, candidate, receipt, runtime_target=target) is None


def test_product_snapshot_requires_host_protected_paths_and_current_original_selection(crossing):
    old_product, target, previous, _, _, _ = crossing
    spec = json.loads(previous["blueprint.json"][0])
    from launchkit.generator import PackageFile
    files = [PackageFile(name, content.decode(), 0o600 if name == ".env.example" else mode)
        for name, (content, mode) in previous.items() if name != "SHA256SUMS"]
    with pytest.raises(ValueError, match="protected predecessor"):
        compiler.generate_runtime_refresh(spec, files, runtime_target=target)
    assert updates.runtime_dependency_transition(previous, previous, original_receipt(previous),
        runtime_target=old_product.target) is None


def test_refreshed_product_continuation_retains_full_protected_baseline(crossing, tmp_path):
    old_product, _, previous, candidate, _, _ = crossing
    current = old_product.successor_workspace
    with current._locked():
        state, files = current._state(), current._files(current._state())
        snapshot = current._framework_snapshot(state, files, build_id=str(uuid.uuid4()),
            accepted_source_digest=current._digest(files))
    reopened = ImplementationWorkspace(tmp_path / "same-source continuation", current.blueprint)
    reopened.seed(snapshot)
    preserved = {item.path: (item.as_bytes(), 0o755 if item.mode & 0o111 else 0o644)
        for item in reopened.export_files(require_verified=False)}
    expected = sorted(name for name in PROTECTED_APPLICATION_PATHS if name in previous)
    for name in expected:
        assert preserved[name] == previous[name] == candidate[name]
    provenance = reopened.framework_provenance()
    assert provenance["baseline_digest"] == current.framework_provenance()["baseline_digest"]
    assert provenance["current_compiler_baseline_digest"] == current.framework_provenance()["current_compiler_baseline_digest"]
    assert provenance["protected_application_paths"] == expected
    assert reopened.verification() is None


def test_closed_product_history_uses_archived_source_and_original_both_product_proofs(crossing, tmp_path, monkeypatch):
    directory, _, _, identity, descriptor = completed(tmp_path, crossing)
    def forbidden(*args, **kwargs):
        pytest.fail("Closed history attempted current compiler or artifact interpretation")
    for name in ("generate_package", "generate_runtime_refresh", "product_entrypoint"):
        monkeypatch.setattr(compiler, name, forbidden)
    for name in ("selected_runtime_artifacts", "candidate_selection"):
        monkeypatch.setattr(selection, name, forbidden)
    _, _, historical = updates.transaction(directory, identity)
    assert historical["status"] == "completed"
    assert historical["runtime_dependency_update"] == descriptor


@pytest.mark.parametrize("successor", [False, True])
def test_fixed_v5_product_history_and_v7_successor_preserve_original_inventory(
        tmp_path, products, inputs, monkeypatch, successor):
    # Synthetic old fixed-inventory package, not a historical native result.
    export = compiler.connect_application_finalizer_export
    generate = compiler.generate_package
    def old_generation(*args, **kwargs):
        if successor and kwargs.get("runtime_target") is not products[0].target:
            return generate(*args, **kwargs)
        with monkeypatch.context() as historical:
            historical.setattr(compiler, "connect_application_finalizer_export", lambda: [item
                for item in export() if item.path != "launchkit/connect_member_class_requests.py"])
            return generate(*args, **kwargs)
    with monkeypatch.context() as old:
        old.setattr(compiler, "generate_package", old_generation)
        # Workspace seeding imports this function directly. Both entry points
        # must apply the historical finalizer export to the predecessor.
        old.setattr("launchkit.implementation_workspace.generate_package", old_generation)
        selected = crossing.__wrapped__(tmp_path, products, inputs)
        assert "launchkit/connect_member_class_requests.py" not in selected[2]
        assert ("launchkit/connect_member_class_requests.py" in selected[3]) is successor
        directory, _, _, identity, descriptor = completed(tmp_path, selected)
    assert descriptor["version"] == (7 if successor else 5)
    assert descriptor.get("previous_framework_version", 4) == 4
    assert ("launchkit/connect_member_class_requests.py" in descriptor["protected_paths"]) is successor
    def forbidden(*args, **kwargs):
        pytest.fail("Historical product read attempted current compilation or artifact admission")
    for name in ("generate_package", "generate_runtime_refresh"):
        monkeypatch.setattr(compiler, name, forbidden)
    for name in ("selected_runtime_artifacts", "candidate_selection"):
        monkeypatch.setattr(selection, name, forbidden)
    _, _, historical = updates.transaction(directory, identity)
    assert historical["runtime_dependency_update"] == descriptor


@pytest.mark.parametrize("defect", ["relabel", "v5-relabel", "previous-framework", "protected-hash", "protected-path", "prior-sdk", "prior-native", "new-sdk", "prior-receipt", "source-map", "old-build", "admission"])
def test_completed_product_history_refuses_changed_frozen_bindings(crossing, tmp_path, defect):
    _, _, previous, candidate, _, _ = crossing
    directory, _, root, identity, _ = completed(tmp_path, crossing)
    receipt, _, current = updates.transaction(directory, identity)
    corrupted = deepcopy(current)
    descriptor = corrupted["runtime_dependency_update"]
    if defect == "relabel": descriptor["version"] = 4
    elif defect == "v5-relabel": descriptor["version"] = 5
    elif defect == "previous-framework": descriptor["previous_framework_version"] = 4
    elif defect == "protected-hash": descriptor["protected_application_inventory"]["config/access.toml"]["sha256"] = "f" * 64
    elif defect == "protected-path": descriptor["protected_application_paths"].remove("config/access.toml")
    elif defect == "prior-sdk": corrupted["runtime_files_retained"]["previous_native"]["sdk"]["meta"]["source_payload"][0][1] = "f" * 64
    elif defect == "prior-native": corrupted["runtime_files_retained"]["previous_native"]["resources_sha256"] = "f" * 64
    elif defect == "new-sdk": corrupted["runtime_artifacts"]["sdk"]["toolkit"]["sha256"] = "f" * 64
    elif defect == "source-map": descriptor["required_supervisor_sources"]["launchkit/templates_v2/app.py"] = "f" * 64
    elif defect == "old-build": corrupted["from_build_id"] = str(uuid.uuid4())
    elif defect == "admission": corrupted["runtime_admission_possible"] = False
    else:
        original = json.loads((root / "previous-receipt.json").read_text())
        original["build_id"] = str(uuid.uuid4())
        updates.write_private(root / "previous-receipt.json", original)
    transition = updates._completed_runtime_dependency_transition(previous, candidate, receipt, corrupted, root)
    assert transition is None or transition != descriptor


@pytest.fixture
def retained(tmp_path, products, monkeypatch):
    old, new = products
    root, transaction = tmp_path / "inert installed bytes", tmp_path / "inert retained bytes"
    root.mkdir(mode=0o700); transaction.mkdir(mode=0o700)
    native = root / ".bin"
    native.mkdir(mode=0o700)
    # This is a byte specimen, not an install: copy inert payloads directly.
    for name, (kind, mode, value) in sorted(old.nodes.items()):
        path = native / name
        if kind == "directory": path.mkdir(parents=True, exist_ok=True)
        elif kind == "link":
            path.symlink_to(value)
            if path.lstat().st_mode & 0o777 != mode:
                os.chmod(path, mode, follow_symlinks=False)
        else: path.write_bytes(value)
    for name, (kind, mode, _) in old.nodes.items():
        if kind != "link": (native / name).chmod(mode)
    site = root / ".venv/lib/python-fixture/site-packages"
    # These inert SDK directories model a private installation independently
    # of the validation host's umask. Native specimen modes above stay exact.
    for directory in (root / ".venv", root / ".venv/lib", site.parent, site):
        directory.mkdir(mode=0o700, exist_ok=True)
    (root / ".venv/bin").mkdir(mode=0o700)
    (root / ".venv/bin/python").write_bytes(b"INERT interpreter must never run\n")
    (root / ".venv/bin/python").chmod(0o700)
    for wheel in (old.target.meta_wheel, old.target.toolkit_wheel):
        with zipfile.ZipFile(wheel.path) as archive:
            for name in archive.namelist():
                path = site / name
                parent = site
                for part in Path(name).parts[:-1]:
                    parent = parent / part
                    parent.mkdir(mode=0o700, exist_ok=True)
                path.write_bytes(archive.read(name)); path.chmod(0o600)
    def locations(base, python=None, site_packages=None):
        assert python is None or Path(python) == Path(base) / ".venv/bin/python"
        actual = Path(base) / ".venv/lib/python-fixture/site-packages"
        assert site_packages is None or Path(site_packages) == actual
        return Path(base) / ".venv/bin/python", actual, Path(base) / ".venv/forge-sdk.receipt.json"
    monkeypatch.setattr(adapter, "_sdk_location", locations)
    monkeypatch.setattr(dependencies, "platform_target", lambda: selection.selected_runtime_platform(old.target))
    previous = {"deployment.json": json_bytes({"target": "mac", "mobkitVersion": old.target.meta.version,
        "runtimeCandidate": selection.selected_runtime_marker(old.target)}),
        "runtime-candidate.json": json_bytes(selection.candidate_selection(old.target))}
    (root / "deployment.json").write_bytes(json_bytes({"target": "mac", "mobkitVersion": new.target.meta.version,
        "runtimeCandidate": selection.selected_runtime_marker(new.target)}))
    (root / "runtime-candidate.json").write_bytes(json_bytes(selection.candidate_selection(new.target)))
    yield root, transaction, old, new, previous
    # Only owned temporary specimens are made removable by pytest.
    for base in (root, transaction):
        for directory, children, files in os.walk(base, followlinks=False):
            Path(directory).chmod(0o700)
            for name in files:
                path = Path(directory) / name
                if not path.is_symlink(): path.chmod(0o600)


def retain_options(old, new, previous, require_current=lambda: None):
    return {"expected_version": old.target.meta.version, "runtime_target": old.target,
        "next_runtime_target": new.target, "previous_source": previous, "require_current": require_current}


def test_product_retention_reads_original_prefix_links_and_sdk_pair_after_source_activation(retained):
    root, transaction, old, new, previous = retained
    proof = dependencies.retain_runtime(root, transaction, **retain_options(old, new, previous))
    assert proof["previous_native"]["assembly_sha256"] == old.target.assembly.sha256
    assert proof["previous_native"]["sdk"]["meta"]["sha256"] == old.target.meta.sha256
    assert not (root / ".bin").exists() and not (root / ".venv").exists()
    assert dependencies.retain_runtime(root, transaction, **retain_options(old, new, previous)) == proof
    saved = transaction / "runtime-before/.bin" / specimens.RESOURCE / "python-current"
    assert saved.is_symlink() and os.readlink(saved) == "python"


@pytest.mark.parametrize("defect", ["source", "prefix", "link", "mode", "sdk", "unselected-link", "owner"])
def test_product_retention_refuses_before_moving_changed_original_bytes_or_owner(retained, defect):
    root, transaction, old, new, previous = retained
    require_current = lambda: None
    if defect == "source":
        previous["runtime-candidate.json"] = json_bytes(selection.candidate_selection(new.target))
    elif defect == "prefix":
        path = root / ".bin" / specimens.RESOURCE / "real-file"
        path.chmod(0o600); path.write_bytes(b"changed resources\n")
    elif defect == "link":
        path = root / ".bin" / specimens.RESOURCE / "python-current"
        path.parent.chmod(0o700)
        path.unlink(); path.symlink_to("runtime-link")
        if path.lstat().st_mode & 0o777 != 0o777:
            os.chmod(path, 0o777, follow_symlinks=False)
        path.parent.chmod(0o555)
    elif defect == "mode": (root / ".bin" / specimens.PREFIX / "bin/toolkit_gateway").chmod(0o700)
    elif defect == "sdk": (root / ".venv/lib/python-fixture/site-packages/meerkat_meta/__init__.py").write_bytes(b"changed SDK\n")
    elif defect == "unselected-link": (root / ".bin/arbitrary-link").symlink_to("/")
    else: require_current = lambda: False
    with pytest.raises((dependencies.RuntimeDependencyError, adapter.CandidateArtifactError)):
        dependencies.retain_runtime(root, transaction, **retain_options(old, new, previous, require_current))
    assert (root / ".bin").is_dir() and (root / ".venv").is_dir()
    assert not (transaction / "runtime-retained.json").exists()


def test_product_partial_retention_resumes_same_original_pair_without_overwriting(retained, monkeypatch):
    root, transaction, old, new, previous = retained
    original = dependencies._rename_root
    moves = []
    def interrupted(source, destination):
        if moves: raise dependencies.RuntimeDependencyError("Inert owner interruption")
        original(source, destination); moves.append(str(source))
    monkeypatch.setattr(dependencies, "_rename_root", interrupted)
    with pytest.raises(dependencies.RuntimeDependencyError, match="owner interruption"):
        dependencies.retain_runtime(root, transaction, **retain_options(old, new, previous))
    assert len(moves) == 1
    monkeypatch.setattr(dependencies, "_rename_root", original)
    proof = dependencies.retain_runtime(root, transaction, **retain_options(old, new, previous))
    assert proof["previous_native"]["assembly_sha256"] == old.target.assembly.sha256
    assert dependencies.retain_runtime(root, transaction, **retain_options(old, new, previous)) == proof
