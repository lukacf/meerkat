from __future__ import annotations

import json
import hashlib
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
import zipfile

import pytest

from launchkit import distribution, dom_check


@pytest.fixture
def bundle_source(tmp_path, monkeypatch):
    repo = tmp_path / "checkout"
    root = repo / "installer"
    contents = {
        "start.sh": Path(__file__).parents[1].joinpath("start.sh").read_text(),
        "README.md": "Public startup instructions. See [the setup walkthrough](GETTING_STARTED.md) and [frontend integration](FRONTEND_INTEGRATION.md).\n",
        "GETTING_STARTED.md": "Public novice setup walkthrough.\n\nReturn to [the reference](README.md).\n",
        "FRONTEND_INTEGRATION.md": "Public frontend integration instructions.\n\nReturn to [the reference](README.md).\n",
        "pyproject.toml": '[project]\nname="meta-forge"\nversion="0.2.0"\ndependencies=["fastapi>=0.115,<1", "meerkat-mobkit==0.8.42"]\n',
        "requirements-standalone.txt": "fastapi==0.141.1\n",
        "launchkit/__init__.py": "",
        "launchkit/server.py": 'VERSION = "0.8.42"\n# Never imported by packaging.\n',
        "launchkit/bootstrap.py": 'VERSION = "0.8.42"\n',
        "launchkit/generator.py": 'MOBKIT_VERSION = "0.8.42"\n',
        "launchkit/distribution.py": Path(distribution.__file__).read_text(),
        "launchkit/templates/setup.py": "# Public application template.\n",
        "launchkit/visualization_references/original.png": "Original public image bytes.",
        "assets/meerkat-transparent.png": "Public asset bytes.",
        "web/freeform-animation.mjs": "export const renderer = true;\n",
        "web/app.jsx": "export const source = true;\n",
        "web/styles.css": "body { color: black; }\n",
        "web/index.html": '<script type="module" src="/app.js"></script>\n',
        "web/dist/app.js": "const compiled = true;\n",
        "web/dist/app.css": "body{color:black}\n",
    }
    for name, content in contents.items():
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content)
    shutil.copyfile(root / "web/index.html", root / "web/dist/index.html")
    (root / "web/dist/assets").mkdir()
    shutil.copyfile(root / "assets/meerkat-transparent.png", root / "web/dist/assets/meerkat-transparent.png")
    for name, source in (("app.js.map", "app.jsx"), ("app.css.map", "styles.css")):
        (root / "web/dist" / name).write_text(json.dumps({"sources": ["../" + source],
            "sourcesContent": [(root / "web" / source).read_text()]}))
    sdk = repo / "sdk/python/meerkat_mobkit"
    sdk.mkdir(parents=True)
    (sdk / "__init__.py").write_text("# Public SDK.\n")
    (sdk / "builder.py").write_text("def gateway_init_timeout(seconds):\n    return seconds\n")
    wheel = tmp_path / "meerkat_mobkit-0.8.42-py3-none-any.whl"
    with zipfile.ZipFile(wheel, "w") as archive:
        for path in sdk.iterdir():
            archive.writestr("meerkat_mobkit/" + path.name, path.read_bytes())
        archive.writestr("meerkat_mobkit-0.8.42.dist-info/METADATA", "Metadata-Version: 2.1\nName: meerkat-mobkit\nVersion: 0.8.42\n")
    authenticated = []
    def prepare(staging, *, source):
        authenticated.append(source)
        directory = staging / "dom-dependencies"
        target = directory / "node_modules/jsdom/package.json"
        target.parent.mkdir(parents=True)
        target.write_text('{"name":"jsdom","version":"24.1.0"}')
        original = source / "node_modules/jsdom/package.json"
        original.parent.mkdir(parents=True, exist_ok=True)
        original.write_bytes(target.read_bytes())
        original.chmod(0o600)
        target.chmod(0o400)
        return directory, "d" * 64
    monkeypatch.setattr(dom_check, "prepare_dependencies", prepare)
    return root, wheel, authenticated


def unpack(root, wheel, output):
    distribution.package(root, wheel, output)
    target = output.with_suffix("")
    with zipfile.ZipFile(output) as archive:
        archive.extractall(target)
    return target


@pytest.fixture
def published_bundle_source(bundle_source, tmp_path, monkeypatch):
    root, _, authenticated = bundle_source
    for name in ("pyproject.toml", "launchkit/bootstrap.py", "launchkit/generator.py", "launchkit/server.py"):
        path = root / name
        path.write_text(path.read_text().replace("0.8.42", "0.8.46"))
    with (root / "launchkit/bootstrap.py").open("a") as handle:
        handle.write('CORE_VERSION = "0.8.51"\n')
    wheel = tmp_path / "meerkat_mobkit-0.8.46-py3-none-any.whl"
    with zipfile.ZipFile(wheel, "w") as archive:
        archive.writestr("meerkat_mobkit/__init__.py", "# Official SDK fixture differs from local source.\n")
        archive.writestr("meerkat_mobkit/builder.py", "def gateway_init_timeout(seconds):\n    return seconds + 1\n")
        archive.writestr("meerkat_mobkit-0.8.46.dist-info/METADATA", "Metadata-Version: 2.4\nName: meerkat-mobkit\nVersion: 0.8.46\n")
        archive.writestr("meerkat_mobkit-0.8.46.dist-info/licenses/LICENSE", "Public license fixture.\n")
    # Tests qualify their synthetic fixture explicitly; production keeps the
    # reviewed official PyPI digest, never a caller-supplied wheel digest.
    monkeypatch.setattr(distribution, "PUBLISHED_SDK", {**distribution.PUBLISHED_SDK,
        "sha256": hashlib.sha256(wheel.read_bytes()).hexdigest()})
    return root, wheel, authenticated


def rewrite_manifest(copied, change):
    path = copied / distribution.MANIFEST
    manifest = json.loads(path.read_text())
    change(manifest)
    path.write_text(json.dumps(manifest))
    return manifest


def test_published_sdk_uses_fixed_provenance_without_claiming_local_source(published_bundle_source, tmp_path):
    root, wheel, _ = published_bundle_source
    copied = unpack(root, wheel, tmp_path / "published.zip")
    data = json.loads((copied / distribution.MANIFEST).read_text())
    assert data["sdk"] == {"version": "0.8.46", "published": True, "wheel": distribution.SDK_WHEEL,
        "provenance": distribution.PUBLISHED_SDK}
    assert data["native"] == distribution.PUBLISHED_NATIVE
    assert "source_sha256" not in data["sdk"]
    assert distribution.verify(copied) == distribution.SDK_WHEEL


def test_production_published_digest_is_the_reviewed_pypi_artifact():
    assert distribution.PUBLISHED_SDK["sha256"] == "e5c1c4cef9a19e945a723c5f9c9a55ff8f457d4c0e76c3ce93094b2646350cc2"
    assert distribution.PUBLISHED_SDK["revision"] == "592c1297e3d4fc5b42db823023c1453cb0daa9f4"


def test_unknown_published_wheel_cannot_be_packaged(published_bundle_source, tmp_path):
    root, wheel, _ = published_bundle_source
    wheel.write_bytes(wheel.read_bytes() + b"Changed release bytes.")
    with pytest.raises(distribution.DistributionError, match="fixed official checksum"):
        distribution.package(root, wheel, tmp_path / "unknown.zip")
    assert not (tmp_path / "unknown.zip").exists()


@pytest.mark.parametrize("metadata", [
    "Metadata-Version: 2.4\nName: other-package\nVersion: 0.8.46\n",
    "Metadata-Version: 2.4\nName: meerkat-mobkit\nVersion: 0.8.42\n",
    "Metadata-Version: 2.4\nName: meerkat-mobkit\nVersion: 0.8.46\nVersion: 0.8.42\n",
])
def test_checksum_selected_wheel_still_requires_unambiguous_package_identity(published_bundle_source, tmp_path, monkeypatch, metadata):
    root, wheel, _ = published_bundle_source
    with zipfile.ZipFile(wheel) as archive:
        files = {name: archive.read(name) for name in archive.namelist()}
    files["meerkat_mobkit-0.8.46.dist-info/METADATA"] = metadata.encode()
    with zipfile.ZipFile(wheel, "w") as archive:
        for name, content in files.items():
            archive.writestr(name, content)
    monkeypatch.setattr(distribution, "PUBLISHED_SDK", {**distribution.PUBLISHED_SDK,
        "sha256": hashlib.sha256(wheel.read_bytes()).hexdigest()})
    with pytest.raises(distribution.DistributionError, match="unexpected package identity"):
        distribution.package(root, wheel, tmp_path / "bad-identity.zip")
    assert not (tmp_path / "bad-identity.zip").exists()


@pytest.mark.parametrize("path", ["unrelated.pth", "meerkat_mobkit/../outside.py"])
def test_checksum_selected_published_wheel_keeps_bounded_public_paths(published_bundle_source, tmp_path, monkeypatch, path):
    root, wheel, _ = published_bundle_source
    with zipfile.ZipFile(wheel, "a") as archive:
        archive.writestr(path, "Unrelated or escaping payload.")
    monkeypatch.setattr(distribution, "PUBLISHED_SDK", {**distribution.PUBLISHED_SDK,
        "sha256": hashlib.sha256(wheel.read_bytes()).hexdigest()})
    with pytest.raises(distribution.DistributionError, match="unexpected files"):
        distribution.package(root, wheel, tmp_path / "bad-path.zip")
    assert not (tmp_path / "bad-path.zip").exists()


def test_changed_published_wheel_cannot_be_rebound_by_manifest(published_bundle_source, tmp_path):
    root, wheel, _ = published_bundle_source
    copied = unpack(root, wheel, tmp_path / "rebound.zip")
    path = copied / distribution.SDK_WHEEL
    path.write_bytes(path.read_bytes() + b"Unqualified wheel change.")
    rewrite_manifest(copied, lambda data: data["files"].update({distribution.SDK_WHEEL: hashlib.sha256(path.read_bytes()).hexdigest()}))
    with pytest.raises(distribution.DistributionError, match="fixed official checksum"):
        distribution.verify(copied)


@pytest.mark.parametrize("field,value", [("registry", "https://other.test/wheel"), ("revision", "a" * 40),
    ("sha256", "f" * 64), ("release", "https://github.com/lukacf/meerkat-mobkit/releases/tag/v0.8.47")])
def test_claimed_published_provenance_is_not_selection_authority(published_bundle_source, tmp_path, field, value):
    root, wheel, _ = published_bundle_source
    copied = unpack(root, wheel, tmp_path / "provenance.zip")
    rewrite_manifest(copied, lambda data: data["sdk"]["provenance"].update({field: value}))
    with pytest.raises(distribution.DistributionError, match="provenance binding is invalid"):
        distribution.verify(copied)


@pytest.mark.parametrize("change", [
    lambda data: data["sdk"].update({"published": False}),
    lambda data: data["sdk"].update({"source_sha256": {"meerkat_mobkit/builder.py": "a" * 64}}),
    lambda data: data["sdk"].update({"version": "0.8.47"}),
    lambda data: data["native"].update({"core_version": "0.8.50"}),
])
def test_invalid_published_manifest_refuses_before_preparation(published_bundle_source, tmp_path, change):
    root, wheel, _ = published_bundle_source
    copied = unpack(root, wheel, tmp_path / "invalid-binding.zip")
    rewrite_manifest(copied, change)
    with pytest.raises(distribution.DistributionError, match="binding is invalid"):
        distribution.verify(copied)
    assert not (copied / ".venv").exists()


@pytest.mark.parametrize("path,old,new", [
    ("pyproject.toml", "0.8.46", "0.8.42"),
    ("launchkit/bootstrap.py", "0.8.46", "0.8.42"),
    ("launchkit/generator.py", "0.8.46", "0.8.42"),
    ("launchkit/server.py", "0.8.46", "0.8.42"),
    ("launchkit/bootstrap.py", "0.8.51", "0.8.50"),
])
def test_mismatched_sdk_native_tuple_refuses_even_if_file_hash_is_updated(published_bundle_source, tmp_path, path, old, new):
    root, wheel, _ = published_bundle_source
    copied = unpack(root, wheel, tmp_path / "mismatched.zip")
    source = copied / path
    source.write_text(source.read_text().replace(old, new))
    rewrite_manifest(copied, lambda data: data["files"].update({path: hashlib.sha256(source.read_bytes()).hexdigest()}))
    with pytest.raises(distribution.DistributionError, match="must match"):
        distribution.verify(copied)


def test_duplicate_manifest_keys_are_rejected(bundle_source, tmp_path):
    root, wheel, _ = bundle_source
    copied = unpack(root, wheel, tmp_path / "duplicate-key.zip")
    path = copied / distribution.MANIFEST
    path.write_text(path.read_text().replace('"published": false', '"published": true, "published": false'))
    with pytest.raises(distribution.DistributionError, match="duplicate keys"):
        distribution.verify(copied)


@pytest.mark.parametrize("content", ["null", "[]", "{invalid JSON"])
def test_malformed_manifest_is_rejected_before_dependency_preparation(bundle_source, tmp_path, content):
    root, wheel, _ = bundle_source
    copied = unpack(root, wheel, tmp_path / "malformed.zip")
    (copied / distribution.MANIFEST).write_text(content)
    with pytest.raises(distribution.DistributionError, match="manifest is invalid"):
        distribution.verify(copied)
    assert not (copied / ".venv").exists()


def test_local_sdk_source_binding_is_checked_after_unpacking(bundle_source, tmp_path):
    root, wheel, _ = bundle_source
    copied = unpack(root, wheel, tmp_path / "local-binding.zip")
    rewrite_manifest(copied, lambda data: data["sdk"]["source_sha256"].update({"meerkat_mobkit/builder.py": "a" * 64}))
    with pytest.raises(distribution.DistributionError, match="local SDK source binding"):
        distribution.verify(copied)


def mock_preparation_python(path, log_variable="PREPARE_LOG"):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text('#!/bin/sh\n'
        f'printf "%s\\n" "$*" >> "${{{log_variable}}}"\n'
        'case "$1:${2:-}" in\n'
        '  -m:venv)\n'
        '    /bin/mkdir -p "$3/bin"\n'
        '    /bin/cp "$0" "$3/bin/python"\n'
        '    exit 0 ;;\n'
        '  -m:pip|-m:ensurepip|-m:launchkit.bootstrap|-m:uvicorn|"-c:from meerkat_mobkit"*) exit 0 ;;\n'
        'esac\n'
        'exec "$PREPARATION_REAL_PYTHON" "$@"\n')
    path.chmod(0o755)


def mock_incomplete_preparation_python(path, failure, partial_python=None):
    mock_preparation_python(path)
    if failure == "missing-venv":
        response = 'if [ "$1:${2:-}" = "-m:venv" ]; then echo "No module named venv" >&2; exit 41; fi\n'
    elif failure == "partial-venv":
        response = ('if [ "$1:${2:-}" = "-m:venv" ]; then\n'
            '  /bin/mkdir -p "$3/bin"\n'
            f'  /bin/cp {shlex.quote(str(partial_python))} "$3/bin/python"\n'
            '  printf "%s\\n" "Keep this partial environment" > "$3/partial-marker"\n'
            '  echo "Pip bootstrap failed after creating the environment" >&2; exit 42\nfi\n')
    else:
        assert failure in {"pip-bootstrap", "pip-still-missing"}
        response = ('if [ "$1:${2:-}:${3:-}" = "-m:pip:--version" ]; then exit 43; fi\n'
            'if [ "$1:${2:-}" = "-m:ensurepip" ]; then '
            + ('echo "No module named ensurepip" >&2; exit 44' if failure == "pip-bootstrap" else 'exit 0')
            + '; fi\n')
    path.write_text(path.read_text().replace('case "$1:${2:-}" in\n', response + 'case "$1:${2:-}" in\n', 1))


@pytest.mark.parametrize("failure", ["missing-venv", "pip-bootstrap", "pip-still-missing"])
@pytest.mark.parametrize("prepare_only", [False, True])
def test_fresh_python_preparation_failure_explains_safe_retry_before_dependencies_or_server(
        bundle_source, tmp_path, explicit_gateway, failure, prepare_only):
    root, wheel, _ = bundle_source
    copied = unpack(root, wheel, tmp_path / "incomplete-python.zip")
    selected = tmp_path / "selected Python" / "python"
    mock_incomplete_preparation_python(selected, failure)
    retained = [copied / ".studio/draft.json", copied / ".env",
        tmp_path / "forge-state/keep", tmp_path / "alias-state/keep", copied / ".venv.previous/keep"]
    for path in retained:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(b"Existing synthetic state must stay intact.\n")
    before = {path: path.read_bytes() for path in retained}
    log = tmp_path / "preparation.txt"
    result = subprocess.run(["/bin/bash", "start.sh", *(["--prepare-only"] if prepare_only else [])], cwd=copied,
        env={"PATH": "/usr/bin:/bin", "PYTHON_BIN": str(selected), "PREPARE_LOG": str(log),
            "PREPARATION_REAL_PYTHON": sys.executable, "MOBKIT_GATEWAY_BIN": str(explicit_gateway),
            "META_FORGE_HOME": str(tmp_path / "forge-state"), "MOBKIT_LAUNCH_HOME": str(tmp_path / "alias-state"),
            "META_FORGE_PORT": "8788", "MOBKIT_LAUNCH_PORT": "8789"},
        text=True, capture_output=True, timeout=20)
    assert result.returncode != 0, (result.stdout, result.stderr)
    assert "complete Python 3.11+" in result.stderr and "venv" in result.stderr and "ensurepip" in result.stderr
    assert "Saved Forge state and settings have not been changed" in result.stderr
    assert "before installing dependencies or starting the server" in result.stderr
    retry = [line for line in result.stderr.splitlines() if line.startswith('  PYTHON_BIN=')]
    assert retry == ['  PYTHON_BIN="/absolute/path/to/python3.11" bash start.sh' + (' --prepare-only' if prepare_only else '')]
    observed = log.read_text().splitlines()
    assert "-m venv .venv" in observed
    assert not any("pip install" in line or "uvicorn" in line or "launchkit.bootstrap" in line for line in observed)
    assert "preparation is complete" not in result.stdout and "Open Meta Forge" not in result.stdout
    assert all(path.read_bytes() == content for path, content in before.items())
    if failure == "missing-venv":
        assert "Python could not create Forge's private .venv" in result.stderr
        assert not (copied / ".venv").exists()
        assert "mv .venv" not in result.stderr
    else:
        assert "Pip could not be prepared in Forge's private .venv" in result.stderr
        assert "choose another unused backup name" in result.stderr
        assert "test ! -e .venv.previous && test ! -L .venv.previous && mv .venv .venv.previous" in result.stderr
        assert (copied / ".venv/bin/python").read_bytes() == selected.read_bytes(), "Failure must not automatically replace the incomplete environment"
        assert "-m ensurepip --upgrade" in observed
        assert observed.count("-m pip --version") == (2 if failure == "pip-still-missing" else 1)


def test_partial_venv_recovery_keeps_environment_until_explicit_backup_and_supported_retry(
        bundle_source, tmp_path, explicit_gateway):
    root, wheel, _ = bundle_source
    copied = unpack(root, wheel, tmp_path / "partial-python.zip")
    partial_python = tmp_path / "partial Python" / "python"
    selected = tmp_path / "incomplete Python" / "python"
    supported = tmp_path / "complete Python" / "python"
    mock_incomplete_preparation_python(partial_python, "pip-bootstrap")
    mock_incomplete_preparation_python(selected, "partial-venv", partial_python)
    mock_preparation_python(supported)
    saved = copied / ".studio/keep.json"
    saved.parent.mkdir(); saved.write_bytes(b'{"idea":"Keep my existing setup"}\n')
    settings = copied / ".env"; settings.write_bytes(b"FICTIONAL_SETTING=retained\n")
    log = tmp_path / "preparation.txt"
    environment = {"PATH": "/usr/bin:/bin", "PYTHON_BIN": str(selected), "PREPARE_LOG": str(log),
        "PREPARATION_REAL_PYTHON": sys.executable, "MOBKIT_GATEWAY_BIN": str(explicit_gateway)}
    def launch():
        return subprocess.run(["/bin/bash", "start.sh", "--prepare-only"], cwd=copied,
            env=environment, text=True, capture_output=True, timeout=20)
    first = launch()
    assert first.returncode != 0
    assert "Python could not create Forge's private .venv" in first.stderr
    partial = copied / ".venv"
    retained = {path.relative_to(partial): path.read_bytes() for path in partial.rglob('*') if path.is_file()}
    before = {path: path.read_bytes() for path in (saved, settings)}
    environment["PYTHON_BIN"] = str(supported)
    first_commands = log.read_text().splitlines()
    second = launch()
    assert second.returncode != 0 and "Pip could not be prepared" in second.stderr
    assert {path.relative_to(partial): path.read_bytes() for path in partial.rglob('*') if path.is_file()} == retained
    assert "-m venv .venv" not in log.read_text().splitlines()[len(first_commands):], "Selecting a complete Python cannot silently rebuild a retained environment"
    assert not (copied / ".venv.previous").exists()
    # Simulate the person's explicit guarded backup entirely inside this tempfile.
    backup = next(line.strip() for line in second.stderr.splitlines() if line.startswith('  test ! -e .venv.previous'))
    backed_up = subprocess.run(["/bin/bash", "-c", backup], cwd=copied, text=True, capture_output=True, timeout=5)
    assert backed_up.returncode == 0, backed_up.stderr
    third = launch()
    assert third.returncode == 0, third.stderr
    assert "preparation is complete" in third.stdout and "web server and native runtime were not started" in third.stdout
    assert (copied / ".venv/bin/python").read_bytes() == supported.read_bytes()
    assert {path.relative_to(copied / ".venv.previous"): path.read_bytes()
        for path in (copied / ".venv.previous").rglob('*') if path.is_file()} == retained
    assert all(path.read_bytes() == content for path, content in before.items())
    observed = log.read_text().splitlines()
    assert observed.count("-m venv .venv") == 2
    assert sum("pip install" in line for line in observed) == 2
    assert not any("uvicorn" in line or "launchkit.bootstrap" in line for line in observed)


@pytest.fixture
def explicit_gateway(tmp_path):
    path = tmp_path / "explicit gateway" / "rpc_gateway"
    path.parent.mkdir()
    path.write_text('#!/bin/sh\nexit 97\n')
    path.chmod(0o755)
    return path


@pytest.mark.parametrize("local_version,uses_local", [("0.8.42", False), ("0.8.46", True)])
def test_source_start_selects_local_sdk_only_when_its_version_matches(bundle_source, tmp_path, explicit_gateway, local_version, uses_local):
    root, _, _ = bundle_source
    project = root / "pyproject.toml"
    project.write_text(project.read_text().replace("0.8.42", "0.8.46"))
    (root.parent / "sdk/python/pyproject.toml").write_text(f'[project]\nversion="{local_version}"\n')
    commands = tmp_path / "commands"
    commands.mkdir()
    npm = commands / "npm"
    npm.write_text('#!/bin/sh\nprintf "npm %s\\n" "$*" >> "$PREPARE_LOG"\n')
    npm.chmod(0o755)
    mock_preparation_python(root / ".venv/bin/python")
    log = tmp_path / "preparation.txt"
    result = subprocess.run(["/bin/bash", "start.sh", "--prepare-only"], cwd=root,
        env={"PATH": str(commands) + ":/usr/bin:/bin", "PREPARE_LOG": str(log),
             "PREPARATION_REAL_PYTHON": sys.executable, "MOBKIT_GATEWAY_BIN": str(explicit_gateway)},
        text=True, capture_output=True, timeout=20)
    assert result.returncode == 0, result.stderr
    observed = log.read_text().splitlines()
    assert ("-m pip install --quiet -e ../sdk/python -e ." in observed) is uses_local
    assert ("-m pip install --quiet -e ." in observed) is not uses_local
    assert not any("uvicorn" in line or "launchkit.bootstrap" in line for line in observed)


def test_bundle_is_deterministic_public_and_self_contained(bundle_source, tmp_path):
    root, wheel, authenticated = bundle_source
    readme = (root / "README.md").read_bytes()
    walkthrough = (root / "GETTING_STARTED.md").read_bytes()
    frontend_integration = (root / "FRONTEND_INTEGRATION.md").read_bytes()
    for name in (".env", ".studio/private.json", ".tools/rpc_gateway", ".venv/key", ".artifacts/private.json",
            "launchkit/__pycache__/private.pyc", "web/node_modules/private"):
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("Private fixture must not be packaged.")
    first, second = tmp_path / "first.zip", tmp_path / "second.zip"
    one = distribution.package(root, wheel, first)
    two = distribution.package(root, wheel, second)
    assert first.read_bytes() == second.read_bytes()
    assert one["sha256"] == two["sha256"] and one["native_executed"] is False
    assert authenticated == [root.parent / "console"] * 2
    with zipfile.ZipFile(first) as archive:
        names = archive.namelist()
        assert "GETTING_STARTED.md" in names
        assert "FRONTEND_INTEGRATION.md" in names
        assert archive.read("README.md") == readme
        assert b"](GETTING_STARTED.md)" in readme
        assert b"](FRONTEND_INTEGRATION.md)" in readme
        assert archive.read("GETTING_STARTED.md") == walkthrough
        assert archive.read("FRONTEND_INTEGRATION.md") == frontend_integration
        assert "web/freeform-animation.mjs" in names
        assert "checker-dependencies/node_modules/jsdom/package.json" in names
        assert all("private" not in name and not name.startswith(".") for name in names)
        assert "launchkit/visualization_references/original.png" in names
        assert "launchkit/templates/setup.py" in names
        data = json.loads(archive.read(distribution.MANIFEST))
        assert data["files"]["GETTING_STARTED.md"] == hashlib.sha256(walkthrough).hexdigest()
        assert data["files"]["FRONTEND_INTEGRATION.md"] == hashlib.sha256(frontend_integration).hexdigest()
        assert data["sdk"]["published"] is False
        assert data["sdk"]["source_sha256"]["meerkat_mobkit/builder.py"]
        assert data["frontend_source_sha256"]["installer/web/app.jsx"]
    copied = unpack(root, wheel, tmp_path / "verify.zip")
    assert distribution.verify(copied) == distribution.LOCAL_SDK_WHEEL
    assert (copied / "GETTING_STARTED.md").read_bytes() == walkthrough
    assert (copied / "FRONTEND_INTEGRATION.md").read_bytes() == frontend_integration
    assert (copied / "checker-dependencies/node_modules/jsdom/package.json").stat().st_mode & 0o777 == 0o600


def test_legacy_standalone_v1_without_walkthrough_still_verifies(bundle_source, tmp_path):
    root, wheel, _ = bundle_source
    (root / "README.md").write_text("Public legacy startup instructions.\n")
    copied = unpack(root, wheel, tmp_path / "legacy.zip")
    for name in ("GETTING_STARTED.md", "FRONTEND_INTEGRATION.md"):
        (copied / name).unlink()
        manifest = rewrite_manifest(copied, lambda data: data["files"].pop(name))
    assert manifest["format"] == "mobkit-launch-standalone-v1"
    assert "GETTING_STARTED.md" not in manifest["files"]
    assert "FRONTEND_INTEGRATION.md" not in manifest["files"]
    assert distribution.verify(copied) == distribution.LOCAL_SDK_WHEEL


@pytest.mark.parametrize("name", ["GETTING_STARTED.md", "FRONTEND_INTEGRATION.md", "web/dist/app.js", distribution.LOCAL_SDK_WHEEL, "web/freeform-animation.mjs",
    "checker-dependencies/node_modules/jsdom/package.json"])
def test_changed_packaged_inputs_are_rejected(bundle_source, tmp_path, name):
    root, wheel, _ = bundle_source
    copied = unpack(root, wheel, tmp_path / "changed.zip")
    (copied / name).write_bytes(b"Different bytes.")
    with pytest.raises(distribution.DistributionError, match="files changed"):
        distribution.verify(copied)


def test_stale_sdk_or_frontend_cannot_be_packaged(bundle_source, tmp_path):
    root, wheel, _ = bundle_source
    builder = root.parent / "sdk/python/meerkat_mobkit/builder.py"
    original = builder.read_text()
    builder.write_text(original + "# Unmatched new source.\n")
    with pytest.raises(distribution.DistributionError, match="SDK source exactly"):
        distribution.package(root, wheel, tmp_path / "stale-sdk.zip")
    builder.write_text(original)
    (root / "web/app.jsx").write_text("export const newer = true;\n")
    with pytest.raises(distribution.DistributionError, match="frontend is stale"):
        distribution.package(root, wheel, tmp_path / "stale-web.zip")
    assert not (tmp_path / "stale-sdk.zip").exists()
    assert not (tmp_path / "stale-web.zip").exists()


def test_installed_package_sources_in_the_frontend_map_are_not_source_inputs(bundle_source, tmp_path):
    root, wheel, _ = bundle_source
    # A dependency's own source map can name files its package does not ship
    # (inline-style-parser 0.2.7 names index.js, the MCP SDK its TypeScript
    # sources and a pnpm store), and esbuild carries those names into app.js.map.
    source_map = root / "web/dist/app.js.map"
    data = json.loads(source_map.read_text())
    data["sources"] += ["../node_modules/inline-style-parser/index.js",
        "../node_modules/node_modules/.pnpm/content-type@1.0.5/node_modules/content-type/index.js"]
    data["sourcesContent"] += ["module.exports = function parse() {};\n", "exports.parse = parse;\n"]
    source_map.write_text(json.dumps(data))
    copied = unpack(root, wheel, tmp_path / "dependencies.zip")
    manifest = json.loads((copied / distribution.MANIFEST).read_text())
    assert sorted(manifest["frontend_source_sha256"]) == ["installer/web/app.jsx", "installer/web/styles.css"]
    # A name spelled through node_modules that resolves to a checkout source is
    # still compared: where it resolves decides, not its spelling.
    (root / "web/node_modules/x").mkdir(parents=True)
    spelled = {"sources": data["sources"] + ["../node_modules/x/../../app.jsx"],
        "sourcesContent": data["sourcesContent"] + ["export const older = true;\n"]}
    source_map.write_text(json.dumps(spelled))
    with pytest.raises(distribution.DistributionError, match="frontend is stale"):
        distribution.package(root, wheel, tmp_path / "spelled-source.zip")
    # The checkout's own sources are still compared, and a missing one is stale.
    data["sources"].append("../missing.jsx")
    data["sourcesContent"].append("export const missing = true;\n")
    source_map.write_text(json.dumps(data))
    with pytest.raises(distribution.DistributionError, match="frontend is stale"):
        distribution.package(root, wheel, tmp_path / "missing-source.zip")
    assert not (tmp_path / "spelled-source.zip").exists()
    assert not (tmp_path / "missing-source.zip").exists()


def test_manifest_path_forgery_and_symlinks_do_not_escape(bundle_source, tmp_path):
    root, wheel, _ = bundle_source
    copied = unpack(root, wheel, tmp_path / "paths.zip")
    path = copied / distribution.MANIFEST
    manifest = json.loads(path.read_text())
    manifest["files"]["../outside"] = "a" * 64
    path.write_text(json.dumps(manifest))
    with pytest.raises(distribution.DistributionError, match="invalid public file"):
        distribution.verify(copied)
    manifest["files"].pop("../outside")
    path.write_text(json.dumps(manifest))
    original = copied / "web/dist/app.js"
    original.unlink()
    original.symlink_to(root / "web/dist/app.js")
    with pytest.raises(distribution.DistributionError, match="ordinary files"):
        distribution.verify(copied)


def test_dependency_lock_cannot_silently_select_latest(bundle_source, tmp_path):
    root, wheel, _ = bundle_source
    (root / "requirements-standalone.txt").write_text("fastapi>=0.115\n")
    with pytest.raises(distribution.DistributionError, match="exact package pins"):
        distribution.package(root, wheel, tmp_path / "unlocked.zip")


def test_extra_wheel_payloads_and_total_bounds_are_rejected(bundle_source, tmp_path, monkeypatch):
    root, wheel, _ = bundle_source
    original = wheel.read_bytes()
    with zipfile.ZipFile(wheel, "a") as archive:
        archive.writestr("unrelated.pth", "Unrelated install payload.")
    with pytest.raises(distribution.DistributionError, match="unexpected files"):
        distribution.package(root, wheel, tmp_path / "extra-wheel.zip")
    wheel.write_bytes(original)
    monkeypatch.setattr(distribution, "MAX_TOTAL", 1)
    with pytest.raises(distribution.DistributionError, match="exceeds its bounds"):
        distribution.package(root, wheel, tmp_path / "oversize.zip")


def test_invalid_checker_modes_are_rejected_before_chmod(bundle_source, tmp_path):
    root, wheel, _ = bundle_source
    copied = unpack(root, wheel, tmp_path / "mode.zip")
    manifest_path = copied / distribution.MANIFEST
    data = json.loads(manifest_path.read_text())
    checker = "checker-dependencies/node_modules/jsdom/package.json"
    original_mode = (copied / checker).stat().st_mode
    data["dom_file_modes"][checker] = 0o777
    manifest_path.write_text(json.dumps(data))
    with pytest.raises(distribution.DistributionError, match="modes are invalid"):
        distribution.verify(copied)
    assert (copied / checker).stat().st_mode == original_mode


def test_hardlinked_checker_file_refuses_without_changing_either_mode(bundle_source, tmp_path):
    root, wheel, _ = bundle_source
    copied = unpack(root, wheel, tmp_path / "hardlink.zip")
    checker = copied / "checker-dependencies/node_modules/jsdom/package.json"
    linked = tmp_path / "other-owned-file"
    os.link(checker, linked)
    original_mode = checker.stat().st_mode
    assert checker.stat().st_nlink == 2
    with pytest.raises(distribution.DistributionError, match="ordinary owned files"):
        distribution.verify(copied)
    assert checker.stat().st_mode == original_mode
    assert linked.stat().st_mode == original_mode


@pytest.mark.parametrize("override", [False, True])
@pytest.mark.parametrize("old_ambient", [False, True])
def test_prepare_only_verifies_then_prepares_without_node_or_runtime(bundle_source, tmp_path, explicit_gateway, override, old_ambient):
    root, wheel, _ = bundle_source
    copied = unpack(root, wheel, tmp_path / "prepare.zip")
    tools = tmp_path / "commands"
    tools.mkdir()
    ambient_log = tmp_path / "ambient-python.txt"
    if old_ambient:
        ambient = tools / "python3"
        ambient.write_text('#!/bin/sh\nprintf "%s\\n" "$*" >> "$AMBIENT_PYTHON_LOG"\n'
            'echo "Python 3.11 or newer is required" >&2\nexit 1\n')
        ambient.chmod(0o755)
    else:
        (tools / "python3").symlink_to(sys.executable)
    npm = tools / "npm"
    npm.write_text("#!/bin/sh\nexit 91\n")
    npm.chmod(0o755)
    venv = copied / ".venv/bin"
    python = venv / "python"
    mock_preparation_python(python)
    log = tmp_path / "commands.txt"
    environment = {"PATH": str(tools) + ":/usr/bin:/bin", "PREPARE_LOG": str(log), "HOME": str(tmp_path),
        "PREPARATION_REAL_PYTHON": sys.executable, "AMBIENT_PYTHON_LOG": str(ambient_log)}
    if override:
        environment["MOBKIT_GATEWAY_BIN"] = str(explicit_gateway)
    result = subprocess.run(["/bin/bash", "start.sh", "--prepare-only"], cwd=copied,
        env=environment, text=True, capture_output=True, timeout=20)
    assert result.returncode == 0, result.stderr
    commands = log.read_text().splitlines()
    assert any("-r requirements-standalone.txt" in command for command in commands)
    assert any("--force-reinstall --no-deps " + distribution.LOCAL_SDK_WHEEL in command for command in commands)
    assert any("pip check" in command for command in commands)
    # The bundle prepares no studio runtime: the guide uses its selected Meta
    # runtime, and every exported package installs and verifies its own.
    assert not any("launchkit.bootstrap" in command for command in commands)
    assert not any("uvicorn" in command for command in commands)
    assert not ambient_log.exists()
    assert "web server and native runtime were not started" in result.stdout


@pytest.mark.parametrize("prepared,explicit", [(False, False), (False, True), (True, True)])
def test_python_selection_prepares_new_and_existing_folders(bundle_source, tmp_path, explicit_gateway, prepared, explicit):
    root, wheel, _ = bundle_source
    copied = unpack(root, wheel, tmp_path / "selected-python.zip")
    tools = tmp_path / "commands"
    tools.mkdir()
    ambient = tools / "python3"
    if explicit:
        ambient.write_text('#!/bin/sh\nprintf "%s\\n" "$*" >> "$AMBIENT_PYTHON_LOG"\n'
            'echo "Python 3.11 or newer is required" >&2\nexit 1\n')
        ambient.chmod(0o755)
    else:
        mock_preparation_python(ambient, "AMBIENT_PYTHON_LOG")
    selected = tmp_path / "selected Python" / "python"
    mock_preparation_python(selected, "SELECTED_PYTHON_LOG")
    venv_python = copied / ".venv/bin/python"
    if prepared:
        mock_preparation_python(venv_python)
    previous_venv = venv_python.read_bytes() if prepared else None
    selected_log, prepare_log, ambient_log = (tmp_path / name for name in
        ["selected-python.txt", "prepare.txt", "ambient-python.txt"])
    environment = {"PATH": str(tools) + ":/usr/bin:/bin", "HOME": str(tmp_path),
        "PREPARATION_REAL_PYTHON": sys.executable, "SELECTED_PYTHON_LOG": str(selected_log),
        "PREPARE_LOG": str(prepare_log), "AMBIENT_PYTHON_LOG": str(ambient_log),
        "MOBKIT_GATEWAY_BIN": str(explicit_gateway)}
    if explicit:
        environment["PYTHON_BIN"] = str(selected)
    result = subprocess.run(["/bin/bash", "start.sh", "--prepare-only"], cwd=copied, env=environment,
        text=True, capture_output=True, timeout=20)
    assert result.returncode == 0, result.stderr
    selection = (selected_log if explicit else ambient_log).read_text().splitlines()
    assert "sys.version_info" in selection[0]
    assert "-B -m launchkit.distribution --bootstrap-inputs ." in selection
    assert ("-m venv .venv" in selection) is not prepared
    preparation = prepare_log.read_text().splitlines() if prepared else selection
    assert any("-r requirements-standalone.txt" in command for command in preparation)
    assert any("pip check" in command for command in preparation)
    assert not any("uvicorn" in command or "launchkit.bootstrap" in command for command in preparation)
    assert not (ambient_log if explicit else selected_log).exists()
    if prepared:
        assert venv_python.read_bytes() == previous_venv


@pytest.mark.parametrize("selection", ["missing", "old"])
@pytest.mark.parametrize("prepared", [False, True])
def test_invalid_explicit_python_stops_before_preparation(bundle_source, tmp_path, explicit_gateway, selection, prepared):
    root, wheel, _ = bundle_source
    copied = unpack(root, wheel, tmp_path / "invalid-python.zip")
    tools = tmp_path / "commands"
    mock_preparation_python(tools / "python3", "AMBIENT_PYTHON_LOG")
    venv_python = copied / ".venv/bin/python"
    if prepared:
        mock_preparation_python(venv_python)
    previous_venv = venv_python.read_bytes() if prepared else None
    selected = tmp_path / "explicit Python" / "python"
    selected_log, prepare_log, ambient_log = (tmp_path / name for name in
        ["selected-python.txt", "prepare.txt", "ambient-python.txt"])
    if selection == "old":
        selected.parent.mkdir()
        selected.write_text('#!/bin/sh\nprintf "%s\\n" "$*" >> "$SELECTED_PYTHON_LOG"\n'
            'echo "Python 3.11 or newer is required" >&2\nexit 1\n')
        selected.chmod(0o755)
    result = subprocess.run(["/bin/bash", "start.sh", "--prepare-only"], cwd=copied,
        env={"PATH": str(tools) + ":/usr/bin:/bin", "HOME": str(tmp_path),
            "PYTHON_BIN": str(selected), "PREPARATION_REAL_PYTHON": sys.executable,
            "SELECTED_PYTHON_LOG": str(selected_log), "PREPARE_LOG": str(prepare_log),
            "AMBIENT_PYTHON_LOG": str(ambient_log), "MOBKIT_GATEWAY_BIN": str(explicit_gateway)},
        text=True, capture_output=True, timeout=20)
    assert result.returncode != 0
    assert "Python 3.11 or newer is required" in result.stderr
    assert not ambient_log.exists() and not prepare_log.exists()
    if selection == "old":
        attempted = selected_log.read_text().splitlines()
        assert len(attempted) == 1 and "sys.version_info" in attempted[0]
    else:
        assert not selected_log.exists()
    if prepared:
        assert venv_python.read_bytes() == previous_venv
    else:
        assert not (copied / ".venv").exists()


@pytest.mark.parametrize("retained", ["unsupported", "broken", "missing"])
def test_supported_explicit_python_refuses_unusable_retained_environment(bundle_source, tmp_path, retained):
    root, wheel, _ = bundle_source
    copied = unpack(root, wheel, tmp_path / "existing Forge folder.zip")
    selected = tmp_path / "supported Python" / "python"
    mock_preparation_python(selected, "SELECTED_PYTHON_LOG")
    venv_python = copied / ".venv/bin/python"
    if retained == "unsupported":
        mock_preparation_python(venv_python)
        venv_python.write_text(venv_python.read_text().replace('case "$1:${2:-}" in\n',
            'case "$1:${2:-}" in\n'
            '  "-c:"*sys.version_info*) echo "Retained Python 3.10 is unsupported" >&2; exit 1 ;;\n'))
    elif retained == "broken":
        venv_python.parent.mkdir(parents=True)
        venv_python.write_text('#!/bin/sh\nprintf "%s\\n" "$*" >> "$PREPARE_LOG"\n'
            'echo "Retained Python cannot execute" >&2\nexit 126\n')
        venv_python.chmod(0o755)
    else:
        venv_python.parent.mkdir(parents=True)
    retained_marker = copied / ".venv/retained-marker"
    retained_marker.write_bytes(b"Existing environment must be preserved.\n")
    draft = copied / ".studio/draft.json"
    draft.parent.mkdir()
    draft.write_bytes(b'{"idea":"Fictional saved setup"}\n')
    settings = copied / ".env"
    settings.write_bytes(b"FICTIONAL_SETTING=retained\n")
    before = {path: path.read_bytes() for path in (retained_marker, draft, settings)}
    previous_python = venv_python.read_bytes() if venv_python.exists() else None
    selected_log, prepare_log = (tmp_path / name for name in ["selected-python.txt", "prepare.txt"])
    result = subprocess.run(["/bin/bash", str(copied / "start.sh"), "--prepare-only"], cwd=tmp_path,
        env={"PATH": "/usr/bin:/bin", "HOME": str(tmp_path), "PYTHON_BIN": str(selected),
            "PREPARATION_REAL_PYTHON": sys.executable, "SELECTED_PYTHON_LOG": str(selected_log),
            "PREPARE_LOG": str(prepare_log)}, text=True, capture_output=True, timeout=20)
    attempted = prepare_log.read_text().splitlines() if prepare_log.exists() else []
    assert result.returncode != 0, (result.stdout, result.stderr, attempted)
    assert "existing .venv" in result.stderr
    assert "Python 3.11 or newer" in result.stderr
    assert "PYTHON_BIN does not upgrade" in result.stderr
    assert "mv .venv .venv.previous" in result.stderr
    retry = [line for line in result.stderr.splitlines() if line.startswith('  PYTHON_BIN=')]
    assert retry == ['  PYTHON_BIN="/absolute/path/to/python3.11" bash start.sh --prepare-only']
    selection = selected_log.read_text().splitlines()
    assert len(selection) == 1 and "sys.version_info" in selection[0]
    assert len(attempted) == (0 if retained == "missing" else 1)
    assert all("sys.version_info" in command for command in attempted)
    assert not any("-m " in command for command in selection + attempted)
    assert not (copied / ".venv.previous").exists()
    assert all(path.read_bytes() == content for path, content in before.items())
    if previous_python is None:
        assert not venv_python.exists()
    else:
        assert venv_python.read_bytes() == previous_python


def test_corrupt_bundle_is_rejected_before_dependency_preparation(bundle_source, tmp_path):
    root, wheel, _ = bundle_source
    copied = unpack(root, wheel, tmp_path / "refused.zip")
    (copied / "web/dist/app.css").write_text("Changed CSS.")
    result = subprocess.run(["/bin/bash", "start.sh", "--prepare-only"], cwd=copied,
        env={"PATH": os.environ["PATH"]}, text=True, capture_output=True, timeout=20)
    assert result.returncode != 0
    assert not (copied / ".venv").exists()


def test_broken_manifest_link_does_not_fall_back_to_source_install(bundle_source, tmp_path):
    root, wheel, _ = bundle_source
    copied = unpack(root, wheel, tmp_path / "broken.zip")
    manifest = copied / distribution.MANIFEST
    manifest.unlink()
    manifest.symlink_to(copied / "missing-manifest.json")
    result = subprocess.run(["/bin/bash", "start.sh", "--prepare-only"], cwd=copied,
        env={"PATH": os.environ["PATH"]}, text=True, capture_output=True, timeout=20)
    assert result.returncode != 0
    assert not (copied / ".venv").exists()


@pytest.mark.parametrize("settings, selected", [
    ({"MOBKIT_LAUNCH_PORT": "8788"}, "8788"),
    ({"META_FORGE_PORT": "8789"}, "8789"),
    ({"META_FORGE_PORT": "8790", "MOBKIT_LAUNCH_PORT": "8788"}, "8790"),
    ({"META_FORGE_PORT": "", "MOBKIT_LAUNCH_PORT": "8788"}, "8788"),
])
def test_standalone_start_uses_selected_port_and_keeps_loopback_binding(bundle_source, tmp_path, explicit_gateway, settings, selected):
    root, wheel, _ = bundle_source
    copied = unpack(root, wheel, tmp_path / "port.zip")
    venv = copied / ".venv/bin"
    python = venv / "python"
    mock_preparation_python(python, "START_LOG")
    log = tmp_path / "commands.txt"
    result = subprocess.run(["/bin/bash", "start.sh"], cwd=copied,
        env={"PATH": os.environ["PATH"], "START_LOG": str(log), "PREPARATION_REAL_PYTHON": sys.executable,
             "MOBKIT_GATEWAY_BIN": str(explicit_gateway), **settings},
        text=True, capture_output=True, timeout=20)
    assert result.returncode == 0, result.stderr
    assert "Open Meta Forge at http://127.0.0.1:" + selected in result.stdout
    assert log.read_text().splitlines()[-1] == (
        "-m uvicorn launchkit.server:app --host 127.0.0.1 --port " + selected
        + " --timeout-graceful-shutdown 20")


@pytest.mark.parametrize("port", ["0", "65536", "-1", "8788x", "08788"])
@pytest.mark.parametrize("setting", ["META_FORGE_PORT", "MOBKIT_LAUNCH_PORT"])
def test_invalid_standalone_port_stops_before_preparation(bundle_source, tmp_path, port, setting):
    root, wheel, _ = bundle_source
    copied = unpack(root, wheel, tmp_path / "invalid-port.zip")
    result = subprocess.run(["/bin/bash", "start.sh"], cwd=copied,
        env={"PATH": os.environ["PATH"], setting: port},
        text=True, capture_output=True, timeout=20)
    assert result.returncode == 1
    assert "port number from 1 to 65535" in result.stderr
    assert not (copied / ".venv").exists()


@pytest.mark.parametrize("selection", ["missing", "non-executable", "directory"])
@pytest.mark.parametrize("prepare_only", [False, True])
def test_invalid_gateway_override_stops_before_preparation(bundle_source, tmp_path, selection, prepare_only):
    root, _, _ = bundle_source
    commands = tmp_path / "commands"
    commands.mkdir()
    npm = commands / "npm"
    npm.write_text('#!/bin/sh\nprintf "npm %s\\n" "$*" >> "$PREPARE_LOG"\n')
    npm.chmod(0o755)
    python = root / ".venv/bin/python"
    mock_preparation_python(python)
    previous_python = python.read_bytes()
    gateway = tmp_path / "private selection" / "rpc_gateway"
    gateway.parent.mkdir()
    if selection == "non-executable":
        gateway.write_text('#!/bin/sh\nexit 97\n')
        gateway.chmod(0o600)
    elif selection == "directory":
        gateway.mkdir()
        gateway.chmod(0o700)
    log = tmp_path / "preparation.txt"
    result = subprocess.run(["/bin/bash", "start.sh", *(["--prepare-only"] if prepare_only else [])], cwd=root,
        env={"PATH": str(commands) + ":/usr/bin:/bin", "PREPARE_LOG": str(log),
             "PREPARATION_REAL_PYTHON": sys.executable, "MOBKIT_GATEWAY_BIN": str(gateway)},
        text=True, capture_output=True, timeout=20)
    assert result.returncode == 1, (result.stdout, result.stderr)
    assert "MOBKIT_GATEWAY_BIN must name an executable file" in result.stderr
    assert "unset MOBKIT_GATEWAY_BIN" in result.stderr
    assert str(gateway) not in result.stdout + result.stderr
    assert "preparation is complete" not in result.stdout
    assert not log.exists()
    assert python.read_bytes() == previous_python


def test_gateway_override_accepts_executable_symlink_without_fetch_or_execution(bundle_source, tmp_path, explicit_gateway):
    root, _, _ = bundle_source
    linked = tmp_path / "gateway link"
    linked.symlink_to(explicit_gateway)
    commands = tmp_path / "commands"
    commands.mkdir()
    npm = commands / "npm"
    npm.write_text('#!/bin/sh\nprintf "npm %s\\n" "$*" >> "$PREPARE_LOG"\n')
    npm.chmod(0o755)
    mock_preparation_python(root / ".venv/bin/python")
    log = tmp_path / "preparation.txt"
    result = subprocess.run(["/bin/bash", "start.sh", "--prepare-only"], cwd=root,
        env={"PATH": str(commands) + ":/usr/bin:/bin", "PREPARE_LOG": str(log),
             "PREPARATION_REAL_PYTHON": sys.executable, "MOBKIT_GATEWAY_BIN": str(linked)},
        text=True, capture_output=True, timeout=20)
    assert result.returncode == 0, result.stderr
    observed = log.read_text().splitlines()
    assert any("-m pip install" in command for command in observed)
    assert any("npm --prefix web run build" in command for command in observed)
    assert not any("uvicorn" in command or "launchkit.bootstrap" in command for command in observed)
    assert "web server and native runtime were not started" in result.stdout


@pytest.fixture
def published_047_source(published_bundle_source, tmp_path, monkeypatch):
    root, wheel, authenticated = published_bundle_source
    for name in ("pyproject.toml", "launchkit/bootstrap.py", "launchkit/generator.py", "launchkit/server.py"):
        path = root / name
        path.write_text(path.read_text().replace("0.8.46", "0.8.47"))
    successor = tmp_path / "meerkat_mobkit-0.8.47-py3-none-any.whl"
    with zipfile.ZipFile(wheel) as previous, zipfile.ZipFile(successor, "w") as current:
        for name in previous.namelist():
            current.writestr(name.replace("0.8.46", "0.8.47"), previous.read(name).replace(b"0.8.46", b"0.8.47"))
    monkeypatch.setattr(distribution, "PUBLISHED_SDK_047", {**distribution.PUBLISHED_SDK_047,
        "sha256": hashlib.sha256(successor.read_bytes()).hexdigest()})
    return root, successor, authenticated


def test_047_official_wheel_provenance_and_native_core_are_bound(published_047_source, tmp_path):
    root, wheel, _ = published_047_source
    copied = unpack(root, wheel, tmp_path / "published-047.zip")
    manifest = json.loads((copied / distribution.MANIFEST).read_text())
    assert manifest["sdk"] == {"version": "0.8.47", "published": True,
        "wheel": distribution.SDK_WHEEL_047, "provenance": distribution.PUBLISHED_SDK_047}
    assert manifest["native"] == distribution.PUBLISHED_NATIVE_047
    assert distribution.verify(copied) == distribution.SDK_WHEEL_047


def test_production_047_artifacts_keep_fixed_primary_release_selection():
    assert distribution.PUBLISHED_SDK_047["sha256"] == "bca838cc3b956949cb13cbe64c91ce5c07738a38385bc791b325ec0a3bdcb85c"
    assert distribution.PUBLISHED_SDK_047["revision"] == "324123936d5a98a14127fbb0c57d56f6a16f71a0"
    assert distribution.PUBLISHED_NATIVE_047["core_version"] == "0.8.51"
    assert distribution.PUBLISHED_SDK["sha256"] == "e5c1c4cef9a19e945a723c5f9c9a55ff8f457d4c0e76c3ce93094b2646350cc2"


@pytest.mark.parametrize("mutation", ["wheel", "revision", "core", "sdk-pin", "native-pin"])
def test_047_does_not_admit_rebound_or_incoherent_artifacts(published_047_source, tmp_path, mutation):
    root, wheel, _ = published_047_source
    if mutation in {"sdk-pin", "native-pin"}:
        path = root / ("pyproject.toml" if mutation == "sdk-pin" else "launchkit/bootstrap.py")
        path.write_text(path.read_text().replace("0.8.47", "0.8.46"))
        with pytest.raises(distribution.DistributionError):
            distribution.package(root, wheel, tmp_path / "incoherent.zip")
        return
    copied = unpack(root, wheel, tmp_path / "published-047.zip")
    if mutation == "wheel":
        path = copied / distribution.SDK_WHEEL_047
        path.write_bytes(path.read_bytes() + b"unqualified wheel bytes")
        rewrite_manifest(copied, lambda data: data["files"].update({distribution.SDK_WHEEL_047: hashlib.sha256(path.read_bytes()).hexdigest()}))
    elif mutation == "revision": rewrite_manifest(copied, lambda data: data["sdk"]["provenance"].update(revision="f" * 40))
    else: rewrite_manifest(copied, lambda data: data["native"].update(core_version="0.8.50"))
    with pytest.raises(distribution.DistributionError):
        distribution.verify(copied)


@pytest.fixture
def dependency_recovery_source(bundle_source):
    root, wheel, authenticated = bundle_source
    # A single selected baseline control can package only the exact retained
    # pre-fix script. This hook is test-owned and never enters product settings.
    if os.environ.get("FORGE_TEST_DEPENDENCY_RECOVERY_BASELINE") == "1":
        source = Path(__file__).parents[1] / ".artifacts/forge-dependency-startup-recovery-20261010/before/installer/start.sh"
        before = source.read_bytes()
        assert hashlib.sha256(before).hexdigest() == "6e95af982df33dc49c15ac5f39060efc34f9c019b61c0a67a5c116620623558d"
        (root / "start.sh").write_bytes(before)
    return root, wheel, authenticated


def dependency_recovery_python(path, *, failure, failed_command):
    """Log selected argv/settings and simulate only the chosen pip failure."""
    mock_preparation_python(path)
    setting_names = ["PYTHON_BIN", "META_FORGE_PORT", "MOBKIT_LAUNCH_PORT", "META_FORGE_HOME",
                     "MOBKIT_LAUNCH_HOME", "MOBKIT_GATEWAY_BIN", "META_FORGE_ORGANIZATION"]
    logger = "import json,os,sys; print(json.dumps({'argv':sys.argv[1:],'settings':{name:os.environ.get(name) for name in " + repr(setting_names) + "}}))"
    prelude = ('"$PREPARATION_REAL_PYTHON" -c ' + shlex.quote(logger) + ' "$@" >> "$DEPENDENCY_RECOVERY_LOG"\n'
        'if [ "${DEPENDENCY_RECOVERY_FAILURE:-}" = ' + shlex.quote(failure) + ' ] && [ "$*" = ' + shlex.quote(failed_command) + ' ]; then\n'
        '  /bin/mkdir -p .venv\n'
        '  printf "%s\\n" "Keep this partially prepared environment" > .venv/dependency-partial-marker\n'
        '  printf "%s\\n" ' + shlex.quote(f"Original mock pip stderr: {failure} failed after partial preparation.") + ' >&2\n'
        '  exit 37\nfi\n')
    path.write_text(path.read_text().replace("#!/bin/sh\n", "#!/bin/sh\n" + prelude, 1))


def dependency_recovery_trial(folder, tmp_path, explicit_gateway, *, failure, failed_command):
    selected = tmp_path / "selected Python $literal;$(printf unwanted) 'quoted'" / "python"
    dependency_recovery_python(selected, failure=failure, failed_command=failed_command)
    gateway = tmp_path / "explicit gateway $literal;$(printf unwanted) 'quoted'" / "rpc_gateway"
    gateway.parent.mkdir(); gateway.write_bytes(explicit_gateway.read_bytes()); gateway.chmod(0o755)
    state = tmp_path / 'Forge state $literal;$(printf unwanted) "quoted"'
    alias = tmp_path / "alias state $literal; 'quoted'"
    settings = {"PYTHON_BIN": str(selected), "META_FORGE_PORT": "8788", "MOBKIT_LAUNCH_PORT": "8789",
        "META_FORGE_HOME": str(state), "MOBKIT_LAUNCH_HOME": str(alias), "MOBKIT_GATEWAY_BIN": str(gateway),
        "META_FORGE_ORGANIZATION": "example-workplace"}
    log = tmp_path / "dependency-recovery.jsonl"
    support = {"PATH": "/usr/bin:/bin", "PREPARATION_REAL_PYTHON": sys.executable,
        "PREPARE_LOG": str(tmp_path / "prepare.log"), "DEPENDENCY_RECOVERY_LOG": str(log)}
    retained = [folder / ".studio/draft.json", folder / ".env", state / "keep.json", alias / "keep.json",
                folder / ".venv.previous/keep.txt"]
    for path in retained:
        path.parent.mkdir(parents=True, exist_ok=True); path.write_bytes(b"Retained synthetic Forge setup.\n")
    return SimpleDependencyRecovery(folder, settings, support, retained, log, failure, failed_command)


class SimpleDependencyRecovery:
    """An inert shell trial scoped to one temporary extracted/source folder."""
    def __init__(self, folder, settings, support, retained, log, failure, failed_command):
        self.folder, self.settings, self.support = folder, settings, support
        self.retained, self.log, self.failure, self.failed_command = retained, log, failure, failed_command
        self.before = {path: path.read_bytes() for path in retained}
        self.retry_settings = settings
        self.launch_port = "8788"

    def launch(self, prepare_only):
        return subprocess.run(["/bin/bash", "start.sh", *(["--prepare-only"] if prepare_only else [])], cwd=self.folder,
            env={**self.support, **self.settings, "DEPENDENCY_RECOVERY_FAILURE": self.failure},
            text=True, capture_output=True, timeout=20)

    def check_failure(self, result, *, label, prepare_only):
        assert result.returncode != 0
        assert f"Original mock pip stderr: {self.failure} failed after partial preparation." in result.stderr
        assert f"Forge dependency preparation did not finish: {label}." in result.stderr
        assert "The web server was not started" in result.stderr
        assert "Dependencies may be partially prepared in .venv" in result.stderr
        assert "Keep .venv, saved Forge state and settings in place" in result.stderr
        assert "before installing dependencies" not in result.stderr
        assert "mv .venv" not in result.stderr
        assert "Open Meta Forge" not in result.stdout and "preparation is complete" not in result.stdout
        observed = [json.loads(line) for line in self.log.read_text().splitlines()]
        failed = shlex.split(self.failed_command)
        assert observed[-1]["argv"] == failed, "No API, frontend, runtime or server command may follow the failed pip step."
        assert not any(row["argv"][:2] in (["-m", "launchkit.bootstrap"], ["-m", "uvicorn"]) for row in observed)
        assert not any("from meerkat_mobkit" in " ".join(row["argv"]) for row in observed)
        assert all(row["settings"] == {name: self.settings.get(name) for name in row["settings"]} for row in observed)
        assert all(path.read_bytes() == value for path, value in self.before.items())
        self.venv_python = (self.folder / ".venv/bin/python").read_bytes()
        assert (self.folder / ".venv/dependency-partial-marker").read_text() == "Keep this partially prepared environment\n"
        cd = [line.strip() for line in result.stderr.splitlines() if line.startswith("  cd -- ")]
        retry = [line.strip() for line in result.stderr.splitlines() if line.startswith("  PYTHON_BIN=")]
        assert len(cd) == len(retry) == 1
        assert retry[0].endswith("bash start.sh" + (" --prepare-only" if prepare_only else ""))
        self.command = cd[0] + "\n" + retry[0]
        self.prior_count = len(observed)

    def retry(self, prepare_only):
        # Clear the simulated pip error and all selected settings. Only the
        # product's printed shell-quoted command can restore that selection.
        result = subprocess.run(["/bin/bash", "-c", self.command], cwd=self.folder.parent,
            env=self.support, text=True, capture_output=True, timeout=20)
        assert result.returncode == 0, (result.stdout, result.stderr)
        observed = [json.loads(line) for line in self.log.read_text().splitlines()][self.prior_count:]
        assert all(row["settings"] == {name: self.retry_settings.get(name) for name in row["settings"]} for row in observed)
        assert not any(row["argv"][:2] == ["-m", "venv"] for row in observed), "Retry must reuse the retained partial environment."
        assert (self.folder / ".venv/bin/python").read_bytes() == self.venv_python
        assert (self.folder / ".venv/dependency-partial-marker").read_text() == "Keep this partially prepared environment\n"
        assert all(path.read_bytes() == value for path, value in self.before.items())
        if prepare_only:
            assert "preparation is complete" in result.stdout
            assert not any(row["argv"][:2] == ["-m", "uvicorn"] for row in observed)
        else:
            assert "Open Meta Forge at http://127.0.0.1:" + self.launch_port in result.stdout
            assert observed[-1]["argv"] == ["-m", "uvicorn", "launchkit.server:app", "--host", "127.0.0.1", "--port", self.launch_port,
                "--timeout-graceful-shutdown", "20"]


@pytest.mark.parametrize("failure,label,failed_command", [
    ("dependencies", "installing pinned dependencies", "-m pip install --quiet -r requirements-standalone.txt"),
    ("sdk", "installing the bundled SDK", "-m pip install --quiet --force-reinstall --no-deps " + distribution.LOCAL_SDK_WHEEL),
    ("check", "checking installed dependencies", "-m pip check"),
])
@pytest.mark.parametrize("prepare_only", [False, True])
def test_dependency_recovery_standalone_failures_and_printed_retry(dependency_recovery_source, tmp_path,
        explicit_gateway, failure, label, failed_command, prepare_only):
    root, wheel, _ = dependency_recovery_source
    folder = unpack(root, wheel, tmp_path / "standalone retry $literal; 'quoted'.zip")
    trial = dependency_recovery_trial(folder, tmp_path, explicit_gateway, failure=failure, failed_command=failed_command)
    trial.check_failure(trial.launch(prepare_only), label=label, prepare_only=prepare_only)
    trial.retry(prepare_only)


@pytest.mark.parametrize("local,label,failed_command", [
    (True, "installing the source checkout and SDK", "-m pip install --quiet -e ../sdk/python -e ."),
    (False, "installing the source checkout", "-m pip install --quiet -e ."),
])
@pytest.mark.parametrize("prepare_only", [False, True])
def test_dependency_recovery_source_local_or_registry_failure(dependency_recovery_source, tmp_path,
        explicit_gateway, local, label, failed_command, prepare_only):
    folder, _, _ = dependency_recovery_source
    (folder.parent / "sdk/python/pyproject.toml").write_text('[project]\nversion="' + ("0.8.42" if local else "0.8.46") + '"\n')
    commands = tmp_path / "mock npm commands"
    commands.mkdir(); npm = commands / "npm"
    npm.write_text('#!/bin/sh\nprintf "npm %s\\n" "$*" >> "$PREPARE_LOG"\n'); npm.chmod(0o755)
    trial = dependency_recovery_trial(folder, tmp_path, explicit_gateway,
        failure="local" if local else "registry", failed_command=failed_command)
    trial.support["PATH"] = str(commands) + ":/usr/bin:/bin"
    trial.check_failure(trial.launch(prepare_only), label=label, prepare_only=prepare_only)
    trial.retry(prepare_only)


def test_dependency_recovery_unset_overrides_and_empty_alias(dependency_recovery_source, tmp_path, explicit_gateway):
    root, wheel, _ = dependency_recovery_source
    folder = unpack(root, wheel, tmp_path / "default settings.zip")
    command = "-m pip install --quiet -r requirements-standalone.txt"
    trial = dependency_recovery_trial(folder, tmp_path, explicit_gateway,
        failure="dependencies", failed_command=command)
    dependency_recovery_python(folder / ".venv/bin/python", failure="dependencies", failed_command=command)
    trial.settings = {"MOBKIT_LAUNCH_HOME": ""}
    trial.retry_settings = {"PYTHON_BIN": ".venv/bin/python", "MOBKIT_LAUNCH_HOME": ""}
    trial.launch_port = "8787"
    result = trial.launch(False)
    trial.check_failure(result, label="installing pinned dependencies", prepare_only=False)
    retry = [line.strip() for line in result.stderr.splitlines() if line.startswith("  PYTHON_BIN=")]
    assert retry == ["PYTHON_BIN=.venv/bin/python MOBKIT_LAUNCH_HOME='' bash start.sh"]
    trial.retry(False)
