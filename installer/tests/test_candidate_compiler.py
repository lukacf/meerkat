"""Candidate compilation and accepted-design preservation, using inert bytes.

No candidate admission, native execution, provider call or installation is
inferred from these tests. Installation and stopped-update acceptance are
separate consumers of the small protected dependency binding.
"""
import ast
import copy
from dataclasses import replace
import hashlib
import json
from pathlib import Path
import subprocess
import sys

import pytest

from launchkit import compiler, generator
from launchkit.blueprint import starter_blueprint
from launchkit.runtime_selection import RuntimeCandidateTarget, candidate_selection
from test_reading_runtime_refresh import previous_framework
from test_runtime_candidates import Bundle


@pytest.fixture
def selected(tmp_path):
    bundle = Bundle(tmp_path / "external-artifacts")
    return bundle, RuntimeCandidateTarget(bundle.tuple_path, bundle.root, bundle.binding)


def source_map(files):
    return {item.path: (item.content, item.mode) for item in files}


@pytest.mark.parametrize("target", ["mac", "linux", "gcp"])
def test_default_package_keeps_the_public_release_path(target):
    spec = starter_blueprint().model_dump(mode="json")
    spec["target"] = target
    if target == "gcp":
        spec["gcp"]["project"] = "forge-candidate-fixture"
    files = source_map(compiler.generate_package(spec))
    assert not {"runtime-candidate.json", "runtime_candidates.py", "sdk_wheel_integrity.py", "install_candidate.py"} & files.keys()
    deployment = json.loads(files["deployment.json"][0])
    assert deployment["mobkitVersion"] == "0.8.47" and "runtimeCandidate" not in deployment
    assert files["requirements.txt"][0].splitlines()[0] == "meerkat-mobkit==0.8.47"
    assert "pip install --disable-pip-version-check -r requirements.txt" in files["install.sh"][0]
    assert "install_candidate" not in files["install_gateway.py"][0]
    assert compiler.MOBKIT_VERSION == generator.MOBKIT_VERSION == "0.8.47"


def test_candidate_compilation_binds_source_outside_unchanged_user_design(selected):
    bundle, target = selected
    spec = starter_blueprint().model_dump(mode="json")
    original = copy.deepcopy(spec)
    files = compiler.generate_package(spec, runtime_target=target)
    generated = {item.path: item for item in files}
    assert json.loads(generated["blueprint.json"].content) == spec == original
    deployment = json.loads(generated["deployment.json"].content)
    assert deployment["mobkitVersion"] == "0.8.48"
    assert deployment["runtimeCandidate"] == {"source": "signed-candidate",
        "selection": "runtime-candidate.json", "tuple_sha256": bundle.binding.tuple_sha256}
    document = json.loads(generated["runtime-candidate.json"].content)
    assert document == candidate_selection(target, deployment_target="mac")
    assert document["accepted"]["source_sha"] == bundle.binding.source_sha
    assert document["accepted"]["native_run_id"] == bundle.binding.native_run_id
    assert document["accepted"]["sdk_wheel_sha256"] == bundle.binding.sdk_wheel_sha256
    assert generated["requirements.txt"].content.splitlines()[0] == "meerkat-mobkit==0.8.48"
    assert generated["sdk_wheel_integrity.py"].content == (
        Path(__file__).parents[1] / "launchkit/sdk_wheel_integrity.py").read_text()
    assert generated["sdk_wheel_integrity.py"].mode == 0o644
    assert "unsigned and unpublished" in generated["README.md"].content
    assert compiler.MOBKIT_VERSION == generator.MOBKIT_VERSION == "0.8.47"
    assert not any(item.path.endswith((".whl", ".tar.gz", ".zip")) for item in files)
    for payload in bundle.binary_payloads.values():
        assert all(payload not in item.as_bytes() for item in files), "Native executables stay in the selected external bundle"


def test_candidate_sdk_precedes_registry_and_gateway_install_with_no_official_fallback(selected):
    _, target = selected
    files = source_map(compiler.generate_package(starter_blueprint(), runtime_target=target))
    install = files["install.sh"][0]
    assert install.index("install_candidate.py sdk-install") < install.index("install_candidate.py requirements-install")
    assert install.index("install_candidate.py requirements-install") < install.index("requirements.application.txt")
    assert install.index("requirements.application.txt") < install.index(".venv/bin/python install_gateway.py")
    assert "pip install --disable-pip-version-check -r requirements.txt" not in install
    gateway = files["install_gateway.py"][0]
    parsed = ast.parse(gateway)
    # The final binding of main and verified_cache is the candidate overlay.
    main = [item for item in parsed.body if isinstance(item, ast.FunctionDef) and item.name == "main"][-1]
    cache = [item for item in parsed.body if isinstance(item, ast.FunctionDef) and item.name == "verified_cache"][-1]
    assert "install_candidate_native_tools" in ast.unparse(main)
    assert "download(" not in ast.unparse(main)
    assert "verify_installed_candidate_runtime" in ast.unparse(cache)
    assert gateway.count('if __name__ == "__main__":') == 1
    for name, (content, _) in files.items():
        if name.endswith(".py"):
            ast.parse(content, filename=name)


def test_exported_candidate_reader_imports_its_local_checker_without_installer_or_sdk(selected, tmp_path):
    _, target = selected
    files = source_map(compiler.generate_package(starter_blueprint(), runtime_target=target))
    root = tmp_path / "exported-reader"
    root.mkdir()
    for name in ("runtime_candidates.py", "sdk_wheel_integrity.py"):
        (root / name).write_text(files[name][0])
    result = subprocess.run([sys.executable, "-I", "-S", "-B", "-c",
        "import pathlib, sys; root = pathlib.Path(sys.argv[1]); sys.path.insert(0, str(root)); "
        "import runtime_candidates; "
        "assert 'sdk_wheel_integrity' in sys.modules; sdk_wheel_integrity = sys.modules['sdk_wheel_integrity']; "
        "assert pathlib.Path(runtime_candidates.__file__).parent == root; "
        "assert pathlib.Path(sdk_wheel_integrity.__file__).parent == root; "
        "assert 'launchkit' not in sys.modules; "
        "assert not any(n.startswith(('meerkat_mobkit', 'meerkat_meta', 'meerkat_toolkit')) for n in sys.modules)",
        str(root)], capture_output=True, text=True, timeout=10)
    assert result.returncode == 0, result.stderr


@pytest.mark.parametrize("version", ["0.8.42", "0.8.46", "0.8.47"])
def test_candidate_refresh_preserves_exact_accepted_design_and_other_dependency_pins(selected, monkeypatch, version):
    bundle, target = selected
    spec, snapshot = previous_framework(monkeypatch, version)
    original = source_map(snapshot.files)
    files = source_map(compiler.generate_runtime_refresh(spec, snapshot.files, runtime_target=target))
    for name in compiler._RUNTIME_DESIGN_PATHS:
        assert files[name] == original[name]
    assert files["requirements.txt"] == (
        original["requirements.txt"][0].replace(f"meerkat-mobkit=={version}", "meerkat-mobkit==0.8.48"), 0o644)
    old, new = json.loads(original["deployment.json"][0]), json.loads(files["deployment.json"][0])
    assert old.pop("mobkitVersion") == version
    assert new.pop("mobkitVersion") == "0.8.48"
    assert new.pop("runtimeCandidate")["tuple_sha256"] == bundle.binding.tuple_sha256
    assert new == old and "applicationProfiles" not in new
    assert source_map(snapshot.files) == original
    assert "sdk_wheel_integrity.py" in files
    assert compiler.MOBKIT_VERSION == generator.MOBKIT_VERSION == "0.8.47"


@pytest.mark.parametrize("target", ["linux", "gcp"])
def test_local_candidate_cannot_be_exported_to_a_different_host_platform(selected, target):
    _, selected_target = selected
    spec = starter_blueprint().model_dump(mode="json")
    spec["target"] = target
    if target == "gcp":
        spec["gcp"]["project"] = "forge-candidate-fixture"
    reason = "platform" if target == "linux" else "GCP product source.*Linux x86_64"
    with pytest.raises(ValueError, match=reason):
        compiler.generate_package(spec, runtime_target=selected_target)


@pytest.mark.parametrize("field,value", [("native_version", "0.8.49"), ("core_version", "0.8.52"),
    ("source_sha", "b" * 40), ("native_run_attempt", 2), ("sdk_wheel_sha256", "f" * 64)])
def test_candidate_target_cannot_change_the_accepted_exact_tuple(selected, field, value):
    _, target = selected
    changed = replace(target, accepted=replace(target.accepted, **{field: value}))
    with pytest.raises(ValueError):
        compiler.generate_package(starter_blueprint(), runtime_target=changed)


def test_candidate_compile_checks_artifact_bytes_again_after_selection(selected):
    bundle, target = selected
    candidate_selection(target)
    bundle.wheel_path.write_bytes(bundle.wheel_path.read_bytes() + b"unaccepted change")
    with pytest.raises(ValueError):
        compiler.generate_package(starter_blueprint(), runtime_target=target)


def test_candidate_generated_manifest_binds_every_protected_dependency_file(selected):
    _, target = selected
    files = source_map(compiler.generate_package(starter_blueprint(), runtime_target=target))
    manifest = dict(line.split("  ", 1)[::-1] for line in files["SHA256SUMS"][0].splitlines())
    assert set(manifest) == set(files) - {"SHA256SUMS"}
    assert all(manifest[name] == hashlib.sha256(content.encode()).hexdigest()
        for name, (content, _) in files.items() if name != "SHA256SUMS")
    assert "sdk_wheel_integrity.py" in manifest


def test_same_candidate_refresh_changes_only_base_host_and_keeps_runtime_design(selected):
    _, target = selected
    spec = starter_blueprint().model_dump(mode="json")
    accepted = compiler.generate_package(spec, runtime_target=target)
    # The accepted older host has no base shell. Its independently accepted
    # application and candidate installers must stay byte for byte intact.
    accepted = [item for item in accepted if item.path not in {
        "console_connect.py", "console_presentation.py", "console_shell.py", "console_shell.mjs", "console_shell.css"}]
    accepted = [generator.PackageFile(item.path, "# Older accepted host\n", item.mode)
                if item.path in {"app.py", "console_auth.py"} else item for item in accepted]
    accepted += [generator.PackageFile("application/__init__.py", "raise RuntimeError('never import application')\n")]
    before = source_map(accepted)
    refreshed = source_map(compiler.generate_runtime_refresh(spec, accepted, runtime_target=target))
    changed = {name for name in before.keys() | refreshed.keys()
               if before.get(name) != refreshed.get(name)}
    assert changed == set(compiler.MAINTAINED_HOST_PATHS) | {"SHA256SUMS"}
    for name in ("deployment.json", "requirements.txt", "runtime-candidate.json", "runtime_candidates.py", "sdk_wheel_integrity.py",
                 "install_candidate.py", "install_gateway.py", "mcp_connections.py", "application/__init__.py"):
        assert refreshed[name] == before[name]
    assert source_map(accepted) == before
    assert refreshed["console_shell.mjs"][0] and refreshed["console_shell.css"][0]
    with pytest.raises(ValueError):
        compiler.generate_runtime_refresh(spec, accepted)
    foreign = copy.deepcopy(accepted)
    index = next(i for i, item in enumerate(foreign) if item.path == "runtime-candidate.json")
    document = json.loads(foreign[index].content)
    document["accepted"]["native_run_attempt"] += 1
    foreign[index] = generator.PackageFile("runtime-candidate.json", json.dumps(document))
    with pytest.raises(ValueError, match="exact accepted candidate"):
        compiler.generate_runtime_refresh(spec, foreign, runtime_target=target)
