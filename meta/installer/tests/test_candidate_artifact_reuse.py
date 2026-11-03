"""Proposed inert per-call reuse controls; no SDK or native program is run."""
from dataclasses import asdict, replace
import importlib.util
import json
import os
from pathlib import Path
import sys
import zipfile

import pytest

from launchkit import runtime_candidates as reader
from launchkit import runtime_dependencies as runtime
from launchkit import runtime_selection as selection
from launchkit.templates import install_candidate as adapter
from test_candidate_runtime_verification import installed, snapshot, verify, write_json
from test_meta_toolkit_runtime_selection import product
from test_runtime_candidates import PLATFORM, VERSION


def instrument(monkeypatch):
    calls = {"full": [], "archives": [], "sdk": [], "installed_native": []}
    full, native = reader.read_candidate_artifacts, reader._native
    sdk, installed_native = adapter.sdk_proof, runtime._hash_executable
    def read(*args, **kwargs):
        calls["full"].append((args, kwargs))
        return full(*args, **kwargs)
    def archive(*args, **kwargs):
        calls["archives"].append(args[2]["binary"])
        return native(*args, **kwargs)
    def payload(*args, **kwargs):
        calls["sdk"].append(args[0].path)
        return sdk(*args, **kwargs)
    def executable(*args, **kwargs):
        calls["installed_native"].append(args[0])
        return installed_native(*args, **kwargs)
    for owner in (reader, selection, adapter):
        monkeypatch.setattr(owner, "read_candidate_artifacts", read)
    monkeypatch.setattr(reader, "_native", archive)
    monkeypatch.setattr(adapter, "sdk_proof", payload)
    monkeypatch.setattr(runtime, "_hash_executable", executable)
    return calls


def test_one_full_tuple_per_call_still_reads_repair_and_both_installed_fences(installed, monkeypatch):
    root, _, bundle, _, queries = installed
    before = snapshot(root), snapshot(bundle.root)
    calls = instrument(monkeypatch)
    first, second = verify(installed), verify(installed)
    assert first == second and len(calls["full"]) == 2
    assert sorted(calls["archives"]) == sorted(list(reader.BINARIES) * 2)
    assert len(calls["sdk"]) == 4 and len(calls["installed_native"]) == 4
    assert len(queries) == 4
    assert (snapshot(root), snapshot(bundle.root)) == before


def test_existing_runtime_target_subclass_keeps_its_admission_semantics(installed):
    class SelectedTarget(selection.RuntimeCandidateTarget):
        pass
    target = installed[3]
    subclass = SelectedTarget(target.tuple_path, target.artifact_root, target.accepted)
    assert verify(installed, runtime_target=subclass) == verify(installed)


def test_required_repair_archive_is_not_skipped_by_runtime_reuse(installed):
    bundle = installed[2]
    repair = next(row for row in bundle.records if row["binary"] == "mobkit_repair")
    path = bundle.native_dir / repair["name"]
    path.write_bytes(path.read_bytes() + b"changed repair archive")
    with pytest.raises(runtime.RuntimeDependencyError):
        verify(installed)
    assert installed[-1] == []


@pytest.mark.parametrize("kind", ["dict", "bare-constructor"])
def test_unpopulated_or_untyped_proof_cannot_replace_full_reader(installed, kind):
    root, _, bundle, _, queries = installed
    artifacts = bundle.read()
    proof = {"verified": True} if kind == "dict" else replace(artifacts)
    with pytest.raises(reader.CandidateArtifactError, match="full reader"):
        adapter.verify_installed_candidate_runtime(root / "runtime-candidate.json",
            expected_native_version=VERSION, expected_platform=PLATFORM, verified_artifacts=proof)
    assert queries == []


@pytest.mark.parametrize("field", ["accepted", "artifact_root", "tuple_path"])
def test_reuse_cannot_substitute_installed_selection_identity_or_locator(installed, field):
    root, _, bundle, _, queries = installed
    artifacts = bundle.read()
    path = root / "runtime-candidate.json"
    value = json.loads(path.read_text())
    if field == "accepted":
        value[field]["source_sha"] = "b" * 40
    else:
        value[field] = str(bundle.root / "another-owned-location")
    write_json(path, value)
    with pytest.raises(reader.CandidateArtifactError):
        adapter.verify_installed_candidate_runtime(path, expected_native_version=VERSION,
            expected_platform=PLATFORM, verified_artifacts=artifacts)
    assert queries == []


@pytest.mark.parametrize("kind", ["tuple", "wheel", "archive", "provenance", "review"])
@pytest.mark.parametrize("boundary", ["runtime-return", "late-sdk-return"])
def test_external_mutation_during_reuse_is_refused_without_another_full_read(
        installed, monkeypatch, kind, boundary):
    root, _, bundle, _, _ = installed
    paths = {"tuple": bundle.tuple_path, "wheel": bundle.wheel_path,
        "archive": bundle.native_dir / bundle.records[0]["name"],
        "provenance": bundle.sdk_dir / "provenance.json", "review": bundle.sdk_dir / "review.json"}
    calls = instrument(monkeypatch)
    if boundary == "runtime-return":
        original = adapter.verify_installed_candidate_runtime
        def changed(*args, **kwargs):
            result = original(*args, **kwargs)
            path = paths[kind]
            path.write_bytes(path.read_bytes() + b"\n")
            return result
        monkeypatch.setattr(adapter, "verify_installed_candidate_runtime", changed)
    else:
        original, count = adapter.sdk_proof, 0
        def changed(*args, **kwargs):
            nonlocal count
            result = original(*args, **kwargs)
            count += 1
            if count == 2:
                path = paths[kind]
                path.write_bytes(path.read_bytes() + b"\n")
            return result
        monkeypatch.setattr(adapter, "sdk_proof", changed)
    with pytest.raises(runtime.RuntimeDependencyError):
        verify(installed)
    assert len(calls["full"]) == 1
    assert not (root / "runtime-inventory.json").exists()


def test_same_bytes_rewritten_after_full_read_cannot_reuse_changed_ctime(installed, monkeypatch):
    original = adapter.verify_installed_candidate_runtime
    def changed(*args, **kwargs):
        result = original(*args, **kwargs)
        path = installed[2].wheel_path
        before = path.stat()
        path.write_bytes(path.read_bytes())
        os.utime(path, ns=(before.st_atime_ns, before.st_mtime_ns))
        assert path.stat().st_ctime_ns != before.st_ctime_ns
        return result
    monkeypatch.setattr(adapter, "verify_installed_candidate_runtime", changed)
    with pytest.raises(runtime.RuntimeDependencyError):
        verify(installed)


@pytest.mark.parametrize("kind", ["sdk", "native", "selection", "deployment"])
def test_reuse_does_not_skip_original_late_installed_checks(installed, monkeypatch, kind):
    root, site, _, _, _ = installed
    original = adapter.verify_installed_candidate_runtime
    def changed(*args, **kwargs):
        result = original(*args, **kwargs)
        paths = {"sdk": site / "meerkat_mobkit/__init__.py", "native": root / ".bin/rpc_gateway",
            "selection": root / "runtime-candidate.json", "deployment": root / "deployment.json"}
        path = paths[kind]
        path.write_bytes(path.read_bytes() + b"\n")
        return result
    monkeypatch.setattr(adapter, "verify_installed_candidate_runtime", changed)
    with pytest.raises(runtime.RuntimeDependencyError):
        verify(installed)


def standalone_pair(tmp_path, monkeypatch):
    # Load only integrity/adapter source. No selected SDK or native program is imported.
    def load(name, source):
        path = tmp_path / (name + ".py")
        path.write_bytes(Path(source).read_bytes())
        spec = importlib.util.spec_from_file_location(name, path)
        module = importlib.util.module_from_spec(spec)
        monkeypatch.setitem(sys.modules, name, module)
        spec.loader.exec_module(module)
        return module
    load("sdk_wheel_integrity", Path(reader.__file__).with_name("sdk_wheel_integrity.py"))
    sibling = load("runtime_candidates", reader.__file__)
    exported = load("reuse_standalone_adapter", adapter.__file__)
    assert exported._candidate_reader is sibling and exported.CandidateBinding is sibling.CandidateBinding
    return sibling, exported


def test_standalone_class_identity_cannot_reuse_host_artifacts(installed, tmp_path, monkeypatch):
    root, _, bundle, _, queries = installed
    _, exported = standalone_pair(tmp_path, monkeypatch)
    with pytest.raises(exported.CandidateArtifactError, match="full reader"):
        exported.load_candidate_selection(root / "runtime-candidate.json",
            expected_native_version=VERSION, expected_platform=PLATFORM, verified_artifacts=bundle.read())
    assert queries == []


def test_older_standalone_reader_without_helper_keeps_default_full_reads_and_refuses_reuse(
        installed, tmp_path, monkeypatch):
    root, site, bundle, _, queries = installed
    sibling, exported = standalone_pair(tmp_path, monkeypatch)
    monkeypatch.delattr(sibling, "recheck_candidate_artifacts")
    original, calls = exported.read_candidate_artifacts, []
    def read(*args, **kwargs):
        calls.append(True)
        return original(*args, **kwargs)
    monkeypatch.setattr(exported, "read_candidate_artifacts", read)
    proof = exported.verify_installed_candidate_runtime(root / "runtime-candidate.json",
        expected_native_version=VERSION, expected_platform=PLATFORM,
        sdk_python=root / ".venv/bin/python", site_packages=site)
    assert proof["source"] == "signed-candidate" and len(calls) == 2 and queries == []
    with pytest.raises(exported.CandidateArtifactError, match="does not support explicit artifact reuse"):
        exported.load_candidate_selection(root / "runtime-candidate.json",
            expected_native_version=VERSION, expected_platform=PLATFORM, verified_artifacts=bundle.read())
    assert len(calls) == 2


@pytest.fixture
def installed_product(product, tmp_path, monkeypatch):
    root = tmp_path / "inert-installed-product"
    root.mkdir(mode=0o700)
    artifacts = reader.read_meta_toolkit_artifacts(product.target)
    adapter.install_meta_toolkit_prefix(artifacts, deployment_root=root, require_current=lambda: None)
    site = root / ".venv/lib/python-fixture/site-packages"
    site.mkdir(parents=True)
    python = root / ".venv/bin/python"
    python.parent.mkdir()
    python.write_bytes(b"inert interpreter; never execute\n")
    python.chmod(0o700)
    for path in (product.target.meta_wheel.path, product.target.toolkit_wheel.path):
        with zipfile.ZipFile(path) as wheel:
            for name in wheel.namelist():
                file = site / name
                file.parent.mkdir(parents=True, exist_ok=True)
                file.write_bytes(wheel.read(name))
    write_json(root / "runtime-candidate.json", selection.candidate_selection(product.target))
    write_json(root / "deployment.json", {"mobkitVersion": product.target.meta.version,
        "runtimeCandidate": selection.selected_runtime_marker(product.target)})
    monkeypatch.setattr(runtime, "platform_target", lambda: selection.selected_runtime_platform(product.target))
    def forbidden(*args, **kwargs):
        pytest.fail("Inert product verification must not dispatch any process.")
    monkeypatch.setattr(adapter.subprocess, "run", forbidden)
    return root, site, python, product


def test_meta_toolkit_runtime_qualifies_full_prefix_once_per_independent_call(installed_product, monkeypatch):
    root, site, python, product = installed_product
    original, calls = selection.read_meta_toolkit_artifacts, []
    def read(*args, **kwargs):
        calls.append(True)
        return original(*args, **kwargs)
    monkeypatch.setattr(selection, "read_meta_toolkit_artifacts", read)
    first = runtime.verify_runtime(root, product.target.meta.version, runtime_target=product.target,
        sdk_python=python, site_packages=site)
    second = runtime.verify_runtime(root, product.target.meta.version, runtime_target=product.target,
        sdk_python=python, site_packages=site)
    assert first == second and len(calls) == 2 and first["source"] == "meta-toolkit-product"


def test_meta_toolkit_original_artifact_fence_still_refuses_during_sdk_comparison(installed_product, monkeypatch):
    root, site, python, product = installed_product
    original = adapter.compare_installed_meta_toolkit_wheels
    def changed(*args, **kwargs):
        facts = original(*args, **kwargs)
        path = product.target.model_catalog.path
        path.write_bytes(path.read_bytes() + b"\n")
        return facts
    monkeypatch.setattr(adapter, "compare_installed_meta_toolkit_wheels", changed)
    with pytest.raises(reader.CandidateArtifactError):
        runtime.verify_runtime(root, product.target.meta.version, runtime_target=product.target,
            sdk_python=python, site_packages=site)
