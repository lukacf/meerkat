"""Inert selected-product source/retention counterexamples, not native acceptance."""
from copy import deepcopy
from dataclasses import asdict, replace
import json
from pathlib import Path
import subprocess
from types import SimpleNamespace
import zipfile

import pytest

from launchkit import compiler, deployment_updates as updates, runtime_dependencies as dependencies
from launchkit import runtime_selection as selection
from launchkit.generator import PackageFile
from launchkit.projects import fingerprint
from launchkit.templates import install_candidate as adapter
from test_meta_toolkit_runtime_selection import (product, no_product_execution, put, pin, json_bytes,
    changed_assembly, digest)
from test_reading_runtime_refresh import previous_framework, inventory, report
from test_candidate_deployment_updates import installation, packed, candidate_proof
from test_candidate_host_integration import inert_pair, PLATFORM
from test_runtime_candidates import Bundle
from test_candidate_deployment_updates import crossing as legacy_crossing
from test_candidate_deployment_updates import completed as completed_legacy


def test_legacy_generated_startup_keeps_actual_selected_crossing_and_closed_history(legacy_crossing, tmp_path):
    _, target, previous, candidate, _, _ = legacy_crossing
    descriptor = updates.runtime_dependency_transition(previous, candidate, {}, runtime_target=target)
    assert descriptor is not None and descriptor["to_version"] == "0.8.48"
    assert digest(candidate["start.sh"][0]) != descriptor["required_supervisor_sources"]["launchkit/templates/start.sh"]
    directory, _, _, expected = completed_legacy(tmp_path, legacy_crossing)
    assert updates.transaction(directory, expected["update_id"])[2] == expected


@pytest.mark.parametrize("defect", ["retained-script", "source-map"])
def test_legacy_generated_startup_refuses_changed_retained_script_or_source_map(legacy_crossing, defect):
    _, target, previous, candidate, _, _ = legacy_crossing
    descriptor = updates.runtime_dependency_transition(previous, candidate, {}, runtime_target=target)
    assert descriptor is not None
    retained, bound = deepcopy(candidate), deepcopy(descriptor)
    if defect == "retained-script":
        content, mode = retained["start.sh"]
        retained["start.sh"] = (content.replace(b"interpreter is unavailable", b"interpreter is available"), mode)
    else:
        bound["required_supervisor_sources"]["launchkit/templates/start.sh"] = "0" * 64
    assert updates._archived_supervisor_sources(retained, bound, 6) is None


def catalog_target(product, model="gpt-6-astra"):
    product.catalog = {"models": [{"id": model, "provider": "openai", "display_name": "Inert model",
        "tier": "fixture", "vision": True, "image_tool_results": True,
        "profile": {"image_input": True, "image_tool_results": True}}], "provider_defaults": []}
    catalog = put(product.target.model_catalog.path, json_bytes(product.catalog))
    product.target = replace(product.target, model_catalog=catalog)
    product.report["cases"][0]["model_catalog"] = pin(catalog)
    verification = put(product.final / "gateway-verification.json", json_bytes(product.report))
    product.assembly["qualification"]["installed_gateway_verification_sha256"] = verification.sha256
    product.target = changed_assembly(product)
    return product.target


@pytest.fixture
def product_crossing(product, monkeypatch):
    target = catalog_target(product)
    spec, snapshot = previous_framework(monkeypatch, "0.8.47")
    old = {item.path: (item.as_bytes(), 0o755 if item.mode & 0o111 else 0o644) for item in snapshot.files}
    old_framework = {"kind": "host-framework-provenance", "version": 1,
        "blueprint_digest": fingerprint(spec), "baseline_digest": snapshot.baseline_digest,
        "current_compiler_baseline_digest": snapshot.baseline_digest, "basis": "current-compiler",
        "retained_template_paths": []}
    old_report = report(spec, old_framework, "inert-predecessor", "a" * 64)
    old["implementation-report.json"] = (json_bytes(old_report), 0o644)
    old["application/__init__.py"] = (b"raise RuntimeError('inert app must never import')\n", 0o644)
    old["requirements.application.txt"] = (b"# inert application with no extra dependencies\n", 0o644)
    source = [PackageFile(name, content.decode(), 0o600 if name == ".env.example" else mode)
        for name, (content, mode) in old.items() if name != "SHA256SUMS"]
    generated = compiler.generate_runtime_refresh(spec, source, runtime_target=target)
    # Today's compiler baseline is measured on the accepted deployment's route.
    ordinary = compiler.generate_package(spec, runtime_target=target, legacy_ordinary=
        compiler.accepted_ordinary_route(spec, json.loads(old["deployment.json"][0]), target))
    baseline, current = inventory(generated), inventory(ordinary)
    retained = sorted(name for name in baseline.keys() | current.keys()
        if name != "SHA256SUMS" and baseline.get(name) != current.get(name))
    binding = {"build_id": old_report["build_id"], "source_digest": old_report["verification"]["digest"]}
    framework = {"kind": "host-framework-provenance", "version": 1, "blueprint_digest": fingerprint(spec),
        "basis": "runtime-refresh", "reason": "explicit-runtime-refresh", "previous": binding, "origin": binding,
        "baseline_digest": fingerprint(baseline), "current_compiler_baseline_digest": fingerprint(current),
        "retained_template_paths": retained, "runtime_candidate": selection.candidate_selection(target)}
    new_report = report(spec, framework, "inert-successor", "b" * 64)
    candidate = {item.path: (item.as_bytes(), 0o755 if item.mode & 0o111 else 0o644) for item in generated}
    for name, value in old.items():
        if name.startswith("application/") or name == "requirements.application.txt":
            candidate[name] = value
    candidate["implementation-report.json"] = (json_bytes(new_report), 0o644)
    return product, target, old, candidate, old_report, new_report


def test_product_compiler_preserves_design_and_refuses_manual_or_unknown_model(product_crossing):
    product, target, old, candidate, _, _ = product_crossing
    for name in ("blueprint.json", "config/mob.toml", "config/console.toml", "config/connections.json", "application/__init__.py"):
        assert candidate[name] == old[name]
    for name in ("install.sh", "start.sh", "install_gateway.py"):
        text = candidate[name][0].decode()
        assert "offered installation, application setup, update or recovery controls" in text
        assert "Shell entrypoints cannot select its native setup inputs" in text
        assert "pip install" not in text and "--doctor" not in text
    assert b"meerkat-meta==0.9.0 --hash=sha256:" in candidate["requirements.txt"][0]
    assert b"meerkat-toolkit==0.9.0 --hash=sha256:" in candidate["requirements.txt"][0]
    assert b"complete native product prefix" in candidate["README.md"][0]
    spec = json.loads(candidate["blueprint.json"][0])
    spec["agent_classes"][0]["model"] = "gpt-6.1-sol"
    with pytest.raises(ValueError, match="catalog"):
        compiler.generate_package(spec, runtime_target=target)


def test_product_descriptor_requires_selected_current_framework_and_offline_bytes(product_crossing):
    product, target, old, candidate, _, _ = product_crossing
    assert updates.runtime_dependency_transition(old, candidate, {}) is None
    descriptor = updates.runtime_dependency_transition(old, candidate, {}, runtime_target=target)
    assert descriptor is not None
    assert descriptor["kind"] == "selected-meta-toolkit-runtime-dependency-update"
    assert descriptor["version"] == 6 and not descriptor["native_registry_update"]
    assert descriptor["runtime_candidate"] == selection.candidate_selection(target)
    assert descriptor["runtime_candidate_artifacts"]["sdk"] == selection._json_paths({"meta": asdict(target.meta), "toolkit": asdict(target.toolkit)})
    assert updates.runtime_dependency_transition(old, candidate, {"execution_mode": "background"}, runtime_target=target) is None
    changed = deepcopy(candidate)
    changed["application/__init__.py"] = (b"changed application\n", 0o644)
    assert updates.runtime_dependency_transition(old, changed, {}, runtime_target=target) is None
    target.model_catalog.path.write_bytes(b"changed catalog")
    assert updates.runtime_dependency_transition(old, candidate, {}, runtime_target=target) is None


def test_closed_product_byte_facts_retain_exact_selection_and_refuse_changes(product_crossing):
    _, target, old, candidate, _, _ = product_crossing
    descriptor = updates.runtime_dependency_transition(old, candidate, {}, runtime_target=target)
    expected = descriptor["runtime_candidate_artifacts"]
    sdk = expected["sdk"]
    native = {"source": "meta-toolkit-product", "version": target.meta.version,
        "platform": selection.selected_runtime_platform(target), "assembly_sha256": target.assembly.sha256,
        "catalog_sha256": target.model_catalog.sha256, "sdk": sdk,
        "binaries": {"toolkit_gateway": {"binary_sha256": expected["native"]["binary_sha256"]}},
        **{name: value for name, value in expected["native"].items() if name != "binary_sha256"}}
    selected = descriptor["runtime_candidate"]
    assert updates._retained_product_proof(native, sdk, selected, expected, "/inert-destination")
    for name in ("assembly_sha256", "catalog_sha256", "resources_sha256", "prefix_inventory_sha256"):
        assert not updates._retained_product_proof({**native, name: "f" * 64}, sdk, selected, expected, "/inert-destination")
    different_sdk = deepcopy(sdk)
    different_sdk["meta"]["source_payload"] = []
    assert not updates._retained_product_proof({**native, "sdk": different_sdk}, different_sdk,
        selected, expected, "/inert-destination")


@pytest.mark.parametrize("value", ["catalog", "candidate"])
def test_nested_verified_product_metadata_cannot_change_under_retained_hash(product_crossing, value):
    from launchkit.native_models import model_registry_lines
    _, target, _, candidate, _, _ = product_crossing
    verified = selection.selected_runtime_artifacts(target)
    if value == "catalog":
        verified.catalog["models"][0]["id"] = "mutated-unverified-model"
    else:
        verified.candidate["binary"]["sha256"] = "f" * 64
    with pytest.raises(ValueError, match="metadata changed"):
        model_registry_lines(json.loads(candidate["blueprint.json"][0]), runtime_target=target,
            verified_artifacts=verified)


@pytest.fixture
def previous_product_crossing(product, legacy_crossing):
    _, _, _, previous, previous_report, old_report = legacy_crossing
    target = catalog_target(product)
    spec = json.loads(previous["blueprint.json"][0])
    source = [PackageFile(name, content.decode(), 0o600 if name == ".env.example" else mode)
        for name, (content, mode) in previous.items() if name != "SHA256SUMS"]
    generated = compiler.generate_runtime_refresh(spec, source, runtime_target=target)
    # Today's compiler baseline is measured on the accepted deployment's route.
    ordinary = compiler.generate_package(spec, runtime_target=target, legacy_ordinary=
        compiler.accepted_ordinary_route(spec, json.loads(previous["deployment.json"][0]), target))
    baseline, current = inventory(generated), inventory(ordinary)
    retained = sorted(name for name in baseline.keys() | current.keys()
        if name != "SHA256SUMS" and baseline.get(name) != current.get(name))
    binding = {"build_id": old_report["build_id"], "source_digest": old_report["verification"]["digest"]}
    framework = {"kind": "host-framework-provenance", "version": 1, "blueprint_digest": fingerprint(spec),
        "basis": "runtime-refresh", "reason": "explicit-runtime-refresh", "previous": binding,
        "origin": binding, "baseline_digest": fingerprint(baseline),
        "current_compiler_baseline_digest": fingerprint(current), "retained_template_paths": retained,
        "runtime_candidate": selection.candidate_selection(target)}
    new_report = report(spec, framework, "inert-product-successor", "e" * 64)
    candidate = {item.path: (item.as_bytes(), 0o755 if item.mode & 0o111 else 0o644) for item in generated}
    for name, value in previous.items():
        if name.startswith("application/") or name == "requirements.application.txt":
            candidate[name] = value
    candidate["implementation-report.json"] = (json_bytes(new_report), 0o644)
    return product, target, previous, candidate, old_report, new_report


@pytest.mark.parametrize("defect", [None, "receipt-selection", "missing-selection", "previous-bytes"])
def test_original_publication_joins_signed_predecessor_to_admitted_old_source(
        previous_product_crossing, tmp_path, defect):
    _, target, previous, candidate, _, new_report = previous_product_crossing
    directory, destination = installation(tmp_path, previous_product_crossing)
    receipt = updates.read_receipt(directory)
    receipt["runtime_candidate"] = json.loads(previous["runtime-candidate.json"][0])
    updates.write_private(directory / "receipt.json", receipt)
    staged = updates.stage(directory, packed(candidate), new_report, runtime_target=target)
    assert staged["update"]["blockers"] == []
    update_id = staged["update"]["update_id"]
    root = updates.update_directory(directory, update_id)
    if defect == "receipt-selection":
        receipt["runtime_candidate"]["accepted"]["source_sha"] = "f" * 40
        updates.write_private(directory / "receipt.json", receipt)
    elif defect == "missing-selection":
        del receipt["runtime_candidate"]
        updates.write_private(directory / "receipt.json", receipt)
    elif defect == "previous-bytes":
        (root / "previous.zip").write_bytes(b"changed original admitted predecessor")
    before = (destination / "deployment.json").read_bytes()
    if defect is not None:
        with pytest.raises(updates.DraftConflict):
            updates.activate(directory, update_id)
        assert (destination / "deployment.json").read_bytes() == before
    else:
        updates.activate(directory, update_id)
        current = updates.read_receipt(directory)
        assert current["runtime_candidate"] == selection.candidate_selection(target)
        assert (destination / "deployment.json").read_bytes() == candidate["deployment.json"][0]


def test_closed_product_transition_keeps_original_json_and_source_history(
        previous_product_crossing, tmp_path, monkeypatch):
    _, target, previous, candidate, _, new_report = previous_product_crossing
    directory, destination = installation(tmp_path, previous_product_crossing)
    receipt = updates.read_receipt(directory)
    receipt["runtime_candidate"] = json.loads(previous["runtime-candidate.json"][0])
    updates.write_private(directory / "receipt.json", receipt)
    staged = updates.stage(directory, packed(candidate), new_report, runtime_target=target)
    identity = staged["update"]["update_id"]
    updates.activate(directory, identity)
    _, root, current = updates.transaction(directory, identity)
    descriptor = current["runtime_dependency_update"]
    old_descriptor = {"runtime_candidate": descriptor["previous_runtime_candidate"],
        "runtime_candidate_artifacts": descriptor["previous_runtime_candidate_artifacts"]}
    _, previous_native = candidate_proof(old_descriptor, destination)
    expected = descriptor["runtime_candidate_artifacts"]
    native = {"source": "meta-toolkit-product", "version": target.meta.version,
        "platform": selection.selected_runtime_platform(target), "assembly_sha256": target.assembly.sha256,
        "catalog_sha256": target.model_catalog.sha256, "sdk": expected["sdk"],
        "binaries": {"toolkit_gateway": {"binary_sha256": expected["native"]["binary_sha256"]}},
        **{name: value for name, value in expected["native"].items() if name != "binary_sha256"}}
    updates.checkpoint(directory, root, current, status="completed", blockers=[], rollback_available=False,
        runtime_admission_possible=True,
        runtime_files_retained={"format": "mobkit-launch-runtime-retention-v1", "retained": True,
            "inventory_sha256": "e" * 64, "destination": str(destination), "backup": str(root / "runtime-before"),
            "previous_native": previous_native},
        runtime_artifacts={"sdk": expected["sdk"], "native": native})

    def forbidden(*args, **kwargs):
        pytest.fail("Closed product history must not read current artifacts or generate later compiler guards")

    monkeypatch.setattr(compiler, "product_entrypoint", forbidden)
    monkeypatch.setattr(selection, "selected_runtime_artifacts", forbidden)
    _, _, historical = updates.transaction(directory, identity)
    assert historical["status"] == "completed" and historical["runtime_dependency_update"] == descriptor
    corrupted = deepcopy(historical)
    corrupted["runtime_artifacts"]["sdk"]["meta"]["source_payload"][0][1] = "f" * 64
    assert updates._completed_runtime_dependency_transition(previous, candidate,
        updates.read_receipt(directory), corrupted, root) is None


LINUX_CP311 = {"python_tag": "cp311", "abi_tag": "cp311", "implementation": "cpython", "system": "Linux",
    "machine": "x86_64", "glibc_minimum": [2, 39], "macos": (0, 0)}


def staged_product_update(crossing, tmp_path):
    _, target, previous, candidate, _, new_report = crossing
    directory, _ = installation(tmp_path, crossing)
    receipt = updates.read_receipt(directory)
    receipt["runtime_candidate"] = json.loads(previous["runtime-candidate.json"][0])
    updates.write_private(directory / "receipt.json", receipt)
    return directory, lambda: updates.stage(directory, packed(candidate), new_report, runtime_target=target)


@pytest.mark.parametrize("host_glibc", [[2, 35], [2, 39]], ids=["glibc-2.35", "glibc-2.39"])
def test_stage_refuses_a_product_update_below_the_declared_glibc_floor_before_staging(
        previous_product_crossing, tmp_path, monkeypatch, host_glibc):
    # Without this, the update is staged and applied, and only the install
    # worker refuses it, after the current runtime has been retained. This
    # fixture's predecessor is an Apple design, so the product's declared
    # floor is modelled at its cheap reader, which reads a real Linux
    # declaration in test_meta_toolkit_runtime_selection.
    from launchkit import deployment, runtime_candidates
    target = previous_product_crossing[1]
    def declared(selected):
        assert selected == target
        return (2, 39)
    monkeypatch.setattr(runtime_candidates, "declared_glibc_floor", declared, raising=False)
    monkeypatch.setattr(deployment, "forge_interpreter", lambda: {**LINUX_CP311, "glibc_minimum": host_glibc},
        raising=False)
    directory, stage = staged_product_update(previous_product_crossing, tmp_path)
    if host_glibc < [2, 39]:
        with pytest.raises(updates.DraftConflict, match=r"glibc 2\.39 or newer, but this computer has glibc 2\.35"):
            stage()
        assert "update" not in updates.read_receipt(directory)
        assert not (directory / "updates").exists() or not any((directory / "updates").iterdir())
    else:
        assert stage()["update"]["status"] == "staged"


def test_stage_refuses_a_product_update_this_cpython_cannot_install_before_staging(
        previous_product_crossing, tmp_path, monkeypatch):
    # The inert closure is pure Python; model a cp311-only compiled closure
    # under a cp312 Forge, as the fresh-install row does.
    from launchkit import deployment
    original = deployment.selected_interpreter_refusal
    def cp311_only(target, interpreter=None):
        compiled = SimpleNamespace(path=Path("/selected/pydantic_core-2.27.2-cp311-cp311-manylinux_2_17_x86_64.whl"))
        return original(SimpleNamespace(dependency_wheels=(compiled,)),
            {**LINUX_CP311, "python_tag": "cp312", "abi_tag": "cp312"})
    monkeypatch.setattr(deployment, "selected_interpreter_refusal", cp311_only, raising=False)
    directory, stage = staged_product_update(previous_product_crossing, tmp_path)
    with pytest.raises(updates.DraftConflict, match=r"built for CPython 3\.11, but this Forge runs CPython 3\.12"):
        stage()
    assert "update" not in updates.read_receipt(directory)


@pytest.fixture
def retained_candidate(tmp_path, product, monkeypatch):
    root, transaction = tmp_path / "inert installed", tmp_path / "inert transaction"
    root.mkdir(mode=0o700); transaction.mkdir(mode=0o700)
    bundle = Bundle(tmp_path / "inert predecessor artifacts")
    target = selection.RuntimeCandidateTarget(bundle.tuple_path, bundle.root, bundle.binding)
    inert_pair(root, bundle, target, candidate=True)
    original = selection.candidate_selection(target)
    previous = {"deployment.json": json_bytes({"mobkitVersion": "0.8.48", "target": "mac",
        "runtimeCandidate": selection.selected_runtime_marker(target)}),
        "runtime-candidate.json": json_bytes(original)}
    # Source is already the successor; retention must use the admitted old ZIP.
    (root / "deployment.json").write_bytes(json_bytes({"mobkitVersion": "0.9.0"}))
    (root / "runtime-candidate.json").write_bytes(json_bytes(selection.candidate_selection(product.target)))
    def stdlib_path(argv, **kwargs):
        assert argv[1:4] == ["-I", "-S", "-c"] and "sysconfig.get_paths" in argv[4]
        site = str(Path(argv[-1]) / "lib/python-fixture/site-packages")
        return subprocess.CompletedProcess(argv, 0, json.dumps({"purelib": site, "platlib": site}))
    monkeypatch.setattr(adapter.subprocess, "run", stdlib_path)
    monkeypatch.setattr(dependencies, "platform_target", lambda: PLATFORM)
    return root, transaction, bundle, target, product.target, previous


def test_retention_uses_original_candidate_source_even_after_activation_and_rechecks_moved_bytes(retained_candidate):
    root, transaction, _, old, new, previous = retained_candidate
    options = {"expected_version": "0.8.48", "runtime_target": old, "next_runtime_target": new,
        "previous_source": previous, "require_current": lambda: None}
    proof = dependencies.retain_runtime(root, transaction, **options)
    assert proof["previous_native"]["source"] == "signed-candidate"
    assert proof["previous_native"]["sdk"]["site_packages"].startswith(str(root / ".venv"))
    assert not (root / ".venv").exists() and not (root / ".bin").exists()
    assert dependencies.retain_runtime(root, transaction, **options) == proof
    (transaction / "runtime-before/.bin/rpc_gateway").write_bytes(b"changed predecessor bytes")
    with pytest.raises(dependencies.RuntimeDependencyError, match="inventory"):
        dependencies.retain_runtime(root, transaction, **options)


@pytest.mark.parametrize("change", ["selection", "sdk", "native", "owner"])
def test_changed_predecessor_or_owner_refuses_before_runtime_move(retained_candidate, change):
    root, transaction, _, old, new, previous = retained_candidate
    current = lambda: None
    if change == "selection":
        document = json.loads(previous["runtime-candidate.json"])
        document["accepted"]["tuple_sha256"] = "f" * 64
        previous["runtime-candidate.json"] = json_bytes(document)
    elif change == "sdk":
        (root / ".venv/lib/python-fixture/site-packages/meerkat_mobkit/__init__.py").write_bytes(b"changed bytes")
    elif change == "native":
        (root / ".bin/rpc_gateway").write_bytes(b"changed predecessor bytes")
    else:
        current = lambda: False
    with pytest.raises((dependencies.RuntimeDependencyError, adapter.CandidateArtifactError)):
        dependencies.retain_runtime(root, transaction, expected_version="0.8.48", runtime_target=old,
            next_runtime_target=new, previous_source=previous, require_current=current)
    assert (root / ".bin").is_dir() and (root / ".venv").is_dir()
    assert not (transaction / "runtime-retained.json").exists()


def test_native_storage_release_boundary_is_exact():
    for before, after in [("0.9.0", "0.9.4"), ("0.9.1", "0.9.4"), ("0.8.48", "0.9.4"), ("0.9.3", "0.9.5")]:
        assert updates.crosses_native_storage(before, after)
    for before, after in [("0.9.0", "0.9.1"), ("0.8.47", "0.9.1"), ("0.9.4", "0.9.4"), ("0.9.4", "0.9.5"),
            ("0.9.4", "0.9.1"), (None, "0.9.4"), ("0.9.1", None), ("latest", "0.9.4"), ("0.9.1", "0.9")]:
        assert not updates.crosses_native_storage(before, after)


def test_runtime_crossing_into_native_storage_is_blocked_before_any_change(product_crossing, monkeypatch):
    product, target, old, candidate, _, _ = product_crossing
    before = deepcopy((old, candidate))
    assert updates.runtime_dependency_transition(old, candidate, {}, runtime_target=target) is not None
    _, blockers = updates.compatibility(old, candidate, {}, runtime_target=target)
    assert not any("storage-migrate-native" in blocker for blocker in blockers)
    # Move the boundary onto this inert 0.8.47 -> 0.9.0 crossing: the refresh is
    # no longer authorized and review names the offline migration it needs.
    monkeypatch.setattr(updates, "NATIVE_STORAGE_META_VERSION", "0.9.0")
    assert updates.native_storage_crossing(old, candidate) == ("0.8.47", "0.9.0")
    assert updates.runtime_dependency_transition(old, candidate, {}, runtime_target=target) is None
    _, blockers = updates.compatibility(old, candidate, {}, runtime_target=target)
    assert blockers[0] == updates.NATIVE_STORAGE_UPDATE_BLOCKER.format("0.8.47", "0.9.0")
    assert "Nothing was changed" in blockers[0] and "`meta_gateway storage-migrate-native`" in blockers[0]
    assert (old, candidate) == before
