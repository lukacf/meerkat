"""Distinct sterile-helper and late-SDK counterexamples, no live runtime.

The real helper case creates an isolated stdlib-only virtual environment without
pip. SDK/native fixture content is inert and never imported or executed.
"""
from copy import deepcopy
import json
from pathlib import Path
import subprocess
import sys
from types import SimpleNamespace
import venv

import pytest

from launchkit import runtime_dependencies as runtime
from launchkit.templates import install_candidate as adapter
from test_candidate_runtime_verification import installed, verify


QUERY = "import json, sysconfig, sys; print(json.dumps(sysconfig.get_paths(scheme='posix_venv' if 'posix_venv' in sysconfig.get_scheme_names() else 'posix_prefix', vars={'base': sys.argv[1], 'platbase': sys.argv[1]})))"


@pytest.fixture
def hardened(installed, monkeypatch):
    root, site, _, _, calls = installed
    python = root / ".venv/bin/python"
    def sterile_location(argv, **kwargs):
        assert argv == [str(python), "-I", "-S", "-c", QUERY, str(root / ".venv")]
        assert kwargs == {"check": True, "capture_output": True, "text": True,
            "timeout": adapter.STDLIB_CHECK_TIMEOUT}
        calls.append(argv)
        return subprocess.CompletedProcess(argv, 0, json.dumps({"purelib": str(site), "platlib": str(site)}))
    monkeypatch.setattr(adapter.subprocess, "run", sterile_location)
    return installed


@pytest.mark.parametrize("legacy_scheme", [False, True])
def test_real_sterile_stdlib_query_skips_hostile_site_startup_and_sdk_source(tmp_path, monkeypatch, legacy_scheme):
    root = tmp_path / "isolated-application"
    root.mkdir(mode=0o700)
    environment = root / ".venv"
    venv.EnvBuilder(with_pip=False, symlinks=True).create(environment)
    site = environment / f"lib/python{sys.version_info.major}.{sys.version_info.minor}/site-packages"
    site.mkdir(parents=True, exist_ok=True)
    sentinels = {name: root / (name + "-executed") for name in ("pth", "sitecustomize", "sdk")}
    (site / "hostile.pth").write_text(
        f"import pathlib; pathlib.Path({str(sentinels['pth'])!r}).write_text('Unexpected site startup')\n")
    (site / "sitecustomize.py").write_text(
        f"from pathlib import Path\nPath({str(sentinels['sitecustomize'])!r}).write_text('Unexpected sitecustomize')\n")
    package = site / "meerkat_mobkit"
    package.mkdir()
    (package / "__init__.py").write_text(
        f"from pathlib import Path\nPath({str(sentinels['sdk'])!r}).write_text('Unexpected SDK import')\n")
    if legacy_scheme:
        # Exercise the pre-posix_venv scheme branch with the real sterile
        # interpreter; this is not a claim of an installed Python 3.10 trial.
        execute = subprocess.run
        def old_schemes(argv, **kwargs):
            assert argv == [str(environment / "bin/python"), "-I", "-S", "-c", QUERY, str(environment)]
            command = list(argv)
            command[4] = "import sysconfig; sysconfig.get_scheme_names=lambda: ('posix_prefix',); " + QUERY
            return execute(command, **kwargs)
        monkeypatch.setattr(adapter.subprocess, "run", old_schemes)
    python, actual_site, receipt = adapter.sdk_location(SimpleNamespace(root=root))
    assert python == environment / "bin/python" and actual_site == site
    assert receipt == environment / "forge-sdk.receipt.json"
    assert not any(path.exists() for path in sentinels.values())
    assert not (site / "__pycache__").exists() and not (package / "__pycache__").exists()


@pytest.mark.parametrize("version", ["3.10", "3.11", "3.12", "3.13", "3.14"])
def test_owned_venv_base_vars_are_explicit_for_each_supported_path_version(tmp_path, monkeypatch, version):
    root = tmp_path / "application"
    site = root / f".venv/lib/python{version}/site-packages"
    site.mkdir(parents=True)
    captured = []
    def location(argv, **kwargs):
        captured.append(argv)
        assert argv == [str(root / ".venv/bin/python"), "-I", "-S", "-c", QUERY, str(root / ".venv")]
        assert kwargs == {"check": True, "capture_output": True, "text": True,
            "timeout": adapter.STDLIB_CHECK_TIMEOUT}
        return subprocess.CompletedProcess(argv, 0, json.dumps({"purelib": str(site), "platlib": str(site)}))
    monkeypatch.setattr(adapter.subprocess, "run", location)
    _, actual, _ = adapter.sdk_location(SimpleNamespace(root=root))
    assert actual == site and len(captured) == 1


def test_host_verifier_reuses_canonical_sdk_proof_late_before_publication(hardened):
    proof = verify(hardened)
    assert len(hardened[-1]) == 2
    assert proof["sdk"] == json.loads((hardened[0] / ".venv/forge-sdk.receipt.json").read_text())


def test_sterile_sdk_path_query_timeout_refuses_candidate_proof(installed, monkeypatch):
    def timed_out(argv, **options):
        assert argv[:4] == [str(installed[0] / ".venv/bin/python"), "-I", "-S", "-c"]
        assert options["timeout"] == adapter.STDLIB_CHECK_TIMEOUT
        raise subprocess.TimeoutExpired(argv, options["timeout"])
    monkeypatch.setattr(adapter.subprocess, "run", timed_out)
    with pytest.raises(runtime.RuntimeDependencyError, match="could not be verified"):
        verify(installed)


@pytest.mark.parametrize("change", ["payload", "metadata", "receipt"])
def test_late_sdk_drift_after_initial_adapter_proof_is_refused(hardened, monkeypatch, change):
    root, site, bundle, _, _ = hardened
    original = adapter.verify_installed_candidate_runtime
    def changed(*args, **kwargs):
        proof = original(*args, **kwargs)
        if change == "payload":
            (site / "meerkat_mobkit/__init__.py").write_bytes(b"Changed source after initial SDK check\n")
        elif change == "metadata":
            (site / "meerkat_mobkit-0.8.48.dist-info/METADATA").write_bytes(b"Changed metadata after initial SDK check\n")
        else:
            path = root / ".venv/forge-sdk.receipt.json"
            value = json.loads(path.read_text())
            value["payload_sha256"]["meerkat_mobkit/__init__.py"] = "0" * 64
            path.write_text(json.dumps(value))
        return proof
    monkeypatch.setattr(adapter, "verify_installed_candidate_runtime", changed)
    with pytest.raises(runtime.RuntimeDependencyError):
        verify(hardened)


@pytest.mark.parametrize("field,value", [("source", "official-release"), ("site_packages", "/another/site"),
    ("payload_sha256", {}), ("metadata_sha256", {}), ("native_run_attempt", True),
    ("signed", True), ("published", True)])
def test_full_late_sdk_proof_must_equal_first_proof_with_typed_fields(hardened, monkeypatch, field, value):
    original = adapter.verify_installed_candidate_sdk
    count = 0
    def drift(*args, **kwargs):
        nonlocal count
        proof = original(*args, **kwargs)
        count += 1
        if count == 2:
            proof = deepcopy(proof)
            proof[field] = value
        return proof
    monkeypatch.setattr(adapter, "verify_installed_candidate_sdk", drift)
    with pytest.raises(runtime.RuntimeDependencyError, match="payload or metadata changed"):
        verify(hardened)
    assert count == 2


def test_ordinary_branch_does_not_invoke_the_new_late_sdk_consumer(hardened, monkeypatch):
    root = hardened[0]
    for name, prefix in runtime.BINARIES.items():
        path = root / ".bin" / (name + ".receipt.json")
        record = json.loads(path.read_text())
        record = {key: record[key] for key in ("version", "archive", "archive_sha256", "binary_sha256")}
        record["release_url"] = "https://github.com/lukacf/meerkat-mobkit/releases/download/v0.8.48"
        path.write_text(json.dumps(record))
    def forbidden(*args, **kwargs):
        raise AssertionError("Ordinary verification cannot invoke the candidate SDK consumer")
    monkeypatch.setattr(adapter, "verify_installed_candidate_sdk", forbidden)
    assert runtime.verify_runtime(root, "0.8.48")["source"] == "official-release"
    assert hardened[-1] == []
