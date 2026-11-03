"""Host candidate verification uses inert installed bytes, never an installer."""
from dataclasses import asdict, replace
import hashlib
import json
from pathlib import Path
import stat
import subprocess
import sys
import zipfile

import pytest

from launchkit import runtime_candidates as reader
from launchkit import runtime_dependencies as runtime
from launchkit.runtime_selection import RuntimeCandidateTarget, candidate_selection
from launchkit.templates import install_candidate as adapter
from test_runtime_candidates import Bundle, PLATFORM, PREFIX, VERSION


def hashed(data):
    return hashlib.sha256(data).hexdigest()


def write_json(path, value, *, private=False):
    path.write_text(json.dumps(value, sort_keys=True) + "\n")
    path.chmod(0o600 if private else 0o644)


def fields(binding):
    return {"candidate_binding": asdict(binding), "candidate_tuple_sha256": binding.tuple_sha256,
        "sdk_wheel_sha256": binding.sdk_wheel_sha256, "source_sha": binding.source_sha,
        "native_run_id": binding.native_run_id, "native_run_attempt": binding.native_run_attempt}


@pytest.fixture
def installed(tmp_path, monkeypatch):
    bundle = Bundle(tmp_path / "external-artifacts")
    target = RuntimeCandidateTarget(bundle.tuple_path, bundle.root, bundle.binding)
    root = tmp_path / "application"
    root.mkdir(mode=0o700)
    (root / ".bin").mkdir(mode=0o700)
    site = root / ".venv/lib/python-fixture/site-packages"
    site.mkdir(parents=True)
    (root / ".venv/bin").mkdir()
    python = root / ".venv/bin/python"
    python.write_bytes(b"Inert selected interpreter; the stdlib location response is mocked.\n")
    python.chmod(0o700)
    # Manual fixture copying is not an SDK or native installation call.
    with zipfile.ZipFile(bundle.wheel_path) as wheel:
        for name in wheel.namelist():
            path = site / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(wheel.read(name))
    write_json(root / "runtime-candidate.json", candidate_selection(target))
    write_json(root / "deployment.json", {"target": "mac", "mobkitVersion": VERSION,
        "runtimeCandidate": {"source": "signed-candidate", "selection": "runtime-candidate.json",
            "tuple_sha256": bundle.binding.tuple_sha256}})
    sdk = {"format": "forge-installed-candidate-sdk-v1", "source": "local-build-from-exact-frozen-source",
        "signed": False, "published": False, "version": VERSION, "site_packages": str(site),
        "payload_sha256": dict(bundle.sdk["source_payload"]),
        "metadata_sha256": {name: hashed(bundle.wheel_values[PREFIX + name])
            for name in ("METADATA", "WHEEL", "top_level.txt")}, **fields(bundle.binding)}
    write_json(root / ".venv/forge-sdk.receipt.json", sdk, private=True)
    for record in bundle.records:
        name = record["binary"]
        path = root / ".bin" / name
        path.write_bytes(bundle.binary_payloads[name])
        path.chmod(0o700)
        write_json(root / ".bin" / (name + ".receipt.json"), {
            "source": "signed-candidate", "version": VERSION, "platform": PLATFORM,
            "archive": record["name"], "archive_sha256": record["sha256"],
            "binary_sha256": record["inner"]["sha256"], **fields(bundle.binding)}, private=True)
    (root / ".env").write_bytes(b"PRIVATE_FIXTURE=preserved\n")
    (root / "data").mkdir()
    (root / "data/retained.txt").write_bytes(b"Original state must not change.\n")
    calls = []
    def stdlib_location(argv, **kwargs):
        assert argv == [str(python), "-I", "-S", "-c",
            "import json, sysconfig, sys; print(json.dumps(sysconfig.get_paths(scheme='posix_venv' if 'posix_venv' in sysconfig.get_scheme_names() else 'posix_prefix', vars={'base': sys.argv[1], 'platbase': sys.argv[1]})))",
            str(root / ".venv")]
        assert kwargs == {"check": True, "capture_output": True, "text": True,
            "timeout": adapter.STDLIB_CHECK_TIMEOUT}
        calls.append(argv)
        return subprocess.CompletedProcess(argv, 0, json.dumps({"purelib": str(site), "platlib": str(site)}))
    monkeypatch.setattr(adapter.subprocess, "run", stdlib_location)
    monkeypatch.setattr(runtime, "platform_target", lambda: PLATFORM)
    def forbidden(*args, **kwargs):
        pytest.fail("Verification must not install SDK/dependencies/native tools.")
    for name in ("install_candidate_sdk", "install_candidate_requirements", "install_candidate_native_tools"):
        monkeypatch.setattr(adapter, name, forbidden)
    for name in adapter.OVERRIDES:
        monkeypatch.delenv(name, raising=False)
    return root, site, bundle, target, calls


def verify(installed, **kwargs):
    root, _, _, target, _ = installed
    return runtime.verify_runtime(root, VERSION, runtime_target=kwargs.get("runtime_target", target))


def snapshot(root):
    return {str(path.relative_to(root)): (path.read_bytes(), stat.S_IMODE(path.stat().st_mode), path.stat().st_ino)
            for path in root.rglob("*") if path.is_file() and not path.is_symlink()}


def test_explicit_host_target_rehashes_actual_sdk_and_both_native_receipts_without_changes(installed):
    root, site, bundle, target, calls = installed
    before = snapshot(root), snapshot(bundle.root)
    proof = verify(installed)
    assert proof == {"version": VERSION, "platform": PLATFORM, "core_version": "0.8.51",
        "source": "signed-candidate", "binaries": {record["binary"]: {
            "receipt_sha256": hashed((root / ".bin" / (record["binary"] + ".receipt.json")).read_bytes()),
            "binary_sha256": record["inner"]["sha256"], "archive": record["name"],
            "archive_sha256": record["sha256"]} for record in bundle.records if record["binary"] in runtime.BINARIES},
        **fields(target.accepted), "sdk": json.loads((root / ".venv/forge-sdk.receipt.json").read_text())}
    assert proof["sdk"]["payload_sha256"] == bundle.sdk["source_payload"]
    assert proof["sdk"]["site_packages"] == str(site)
    assert len(calls) == 2 and calls[0] == calls[1]
    assert (snapshot(root), snapshot(bundle.root)) == before
    assert all("release_url" not in value for value in proof["binaries"].values())


def test_installed_candidate_cannot_admit_itself_without_an_explicit_host_target(installed):
    with pytest.raises(runtime.RuntimeDependencyError, match="official release"):
        verify(installed, runtime_target=None)
    assert installed[-1] == []


def test_none_keeps_existing_official_proof_and_does_not_use_candidate_adapter(installed, monkeypatch):
    root, _, _, _, calls = installed
    for name, prefix in runtime.BINARIES.items():
        write_json(root / ".bin" / (name + ".receipt.json"), {"version": "0.8.47",
            "archive": f"{prefix}-0.8.47-{PLATFORM}.tar.gz", "archive_sha256": "f" * 64,
            "release_url": "https://github.com/lukacf/meerkat-mobkit/releases/download/v0.8.47",
            "binary_sha256": hashed((root / ".bin" / name).read_bytes())}, private=True)
    def forbidden(*args, **kwargs):
        pytest.fail("The ordinary branch must not invoke candidate verification.")
    monkeypatch.setattr(adapter, "verify_installed_candidate_runtime", forbidden)
    result = runtime.verify_runtime(root, "0.8.47")
    assert result["source"] == "official-release" and result["core_version"] == "0.8.51"
    assert all("release_url" in value for value in result["binaries"].values()) and calls == []


def test_untyped_host_claim_cannot_replace_the_explicit_target(installed):
    with pytest.raises(runtime.RuntimeDependencyError):
        verify(installed, runtime_target={"source": "signed-candidate", "verified": True})
    assert installed[-1] == []


@pytest.mark.parametrize("version", ["0.8.47", "0.8.49"])
def test_requested_version_must_match_the_explicit_candidate_before_sdk_access(installed, version):
    root, _, _, target, calls = installed
    with pytest.raises(runtime.RuntimeDependencyError, match="native version, Core and local platform"):
        runtime.verify_runtime(root, version, runtime_target=target)
    assert calls == []


def test_real_local_platform_must_match_the_explicit_candidate_before_sdk_access(installed, monkeypatch):
    monkeypatch.setattr(runtime, "platform_target", lambda: "x86_64-apple-darwin")
    with pytest.raises(runtime.RuntimeDependencyError, match="native version, Core and local platform"):
        verify(installed)
    assert installed[-1] == []


@pytest.mark.parametrize("field,value", [("source_sha", "b" * 40), ("tuple_sha256", "f" * 64),
    ("repository", "someone/else"), ("native_version", "0.8.49"), ("core_version", "0.8.52"),
    ("platform", "x86_64-apple-darwin"), ("native_run_id", 24), ("native_run_attempt", 2),
    ("native_manifest_sha256", "f" * 64), ("sdk_wheel_sha256", "f" * 64)])
def test_host_binding_drift_cannot_reuse_existing_candidate_bytes(installed, field, value):
    target = installed[3]
    changed = replace(target, accepted=replace(target.accepted, **{field: value}))
    with pytest.raises(runtime.RuntimeDependencyError):
        verify(installed, runtime_target=changed)
    assert installed[-1] == []


@pytest.mark.parametrize("locator", ["tuple_path", "artifact_root"])
def test_host_locator_drift_is_refused(installed, tmp_path, locator):
    target = installed[3]
    with pytest.raises(runtime.RuntimeDependencyError):
        verify(installed, runtime_target=replace(target, **{locator: tmp_path / "foreign-location"}))
    assert installed[-1] == []


@pytest.mark.parametrize("mutation", ["source", "tuple", "root", "run-bool", "extra", "duplicate"])
def test_installed_selection_cannot_substitute_a_source_or_locator(installed, mutation):
    root, _, _, _, calls = installed
    path = root / "runtime-candidate.json"
    value = json.loads(path.read_text())
    if mutation == "source": value["accepted"]["source_sha"] = "b" * 40
    elif mutation == "tuple": value["tuple_path"] = str(root / "other-tuple.json")
    elif mutation == "root": value["artifact_root"] = str(root)
    elif mutation == "run-bool": value["accepted"]["native_run_attempt"] = True
    elif mutation == "extra": value["verified"] = True
    if mutation == "duplicate":
        path.write_text(path.read_text().rstrip()[:-1] + ', "format": "forge-runtime-candidate-selection-v1"}\n')
    else:
        write_json(path, value)
    with pytest.raises(runtime.RuntimeDependencyError): verify(installed)
    assert calls == []


@pytest.mark.parametrize("field,value", [("source", "official-release"), ("selection", "other.json"),
    ("tuple_sha256", "f" * 64), ("verified", True), ("mobkitVersion", "0.8.47"), ("target", "gcp")])
def test_installed_deployment_must_select_the_exact_local_candidate(installed, field, value):
    path = installed[0] / "deployment.json"
    document = json.loads(path.read_text())
    (document if field in {"mobkitVersion", "target"} else document["runtimeCandidate"])[field] = value
    write_json(path, document)
    with pytest.raises(runtime.RuntimeDependencyError): verify(installed)
    assert installed[-1] == []


@pytest.mark.parametrize("mutation", ["payload", "extra-payload", "metadata", "wheel-metadata", "receipt-payload",
    "receipt-origin", "receipt-published", "receipt-run-bool", "receipt-binding", "receipt-mode"])
def test_actual_installed_sdk_tamper_cannot_pass_same_version_verification(installed, mutation):
    root, site, _, _, _ = installed
    receipt = root / ".venv/forge-sdk.receipt.json"
    if mutation == "payload": (site / "meerkat_mobkit/__init__.py").write_bytes(b"Changed SDK source\n")
    elif mutation == "extra-payload": (site / "meerkat_mobkit/extra.py").write_bytes(b"Unexpected source\n")
    elif mutation == "metadata": (site / PREFIX / "METADATA").write_bytes(b"Same version, changed metadata\n")
    elif mutation == "wheel-metadata": (site / PREFIX / "WHEEL").write_bytes(b"Changed wheel metadata\n")
    elif mutation == "receipt-mode": receipt.chmod(0o644)
    else:
        value = json.loads(receipt.read_text())
        if mutation == "receipt-payload": value["payload_sha256"]["meerkat_mobkit/__init__.py"] = "f" * 64
        elif mutation == "receipt-origin": value["source"] = "official-release"
        elif mutation == "receipt-published": value["published"] = True
        elif mutation == "receipt-run-bool": value["native_run_attempt"] = True
        else: value["candidate_binding"]["source_sha"] = "b" * 40
        write_json(receipt, value, private=True)
    with pytest.raises(runtime.RuntimeDependencyError): verify(installed)


@pytest.mark.parametrize("name", ["rpc_gateway", "mobkit_gateway"])
@pytest.mark.parametrize("mutation", ["binary", "receipt-hash", "receipt-binding", "receipt-source", "receipt-run-bool", "link"])
def test_actual_native_tamper_is_refused_for_each_required_tool(installed, name, mutation):
    root = installed[0]
    binary, receipt = root / ".bin" / name, root / ".bin" / (name + ".receipt.json")
    if mutation == "binary": binary.write_bytes(b"Changed inert native bytes\n")
    elif mutation == "link":
        moved = root / (name + "-outside")
        binary.rename(moved)
        binary.symlink_to(moved)
    else:
        value = json.loads(receipt.read_text())
        if mutation == "receipt-hash": value["binary_sha256"] = "f" * 64
        elif mutation == "receipt-binding": value["candidate_binding"]["native_run_attempt"] = 2
        elif mutation == "receipt-source": value["source"] = "official-release"
        else: value["native_run_attempt"] = True
        write_json(receipt, value, private=True)
    with pytest.raises(runtime.RuntimeDependencyError): verify(installed)


@pytest.mark.parametrize("name", adapter.OVERRIDES)
def test_candidate_gateway_override_is_refused_before_any_selected_python_call(installed, monkeypatch, name):
    monkeypatch.setenv(name, str(installed[0] / ".bin/rpc_gateway"))
    with pytest.raises(runtime.RuntimeDependencyError): verify(installed)
    assert installed[-1] == []


@pytest.mark.parametrize("path", ["runtime-candidate.json", "deployment.json", ".bin/rpc_gateway.receipt.json"])
def test_protected_selection_or_receipt_change_during_actual_verification_is_refused(installed, monkeypatch, path):
    original = adapter.verify_installed_candidate_runtime
    def changed(*args, **kwargs):
        value = original(*args, **kwargs)
        file = installed[0] / path
        file.write_bytes(file.read_bytes() + b"\n")
        return value
    monkeypatch.setattr(adapter, "verify_installed_candidate_runtime", changed)
    with pytest.raises(runtime.RuntimeDependencyError): verify(installed)


@pytest.mark.parametrize("kind", ["wheel", "native"])
def test_external_selected_artifact_drift_is_rechecked_before_installed_sdk_access(installed, kind):
    bundle = installed[2]
    path = bundle.wheel_path if kind == "wheel" else bundle.native_dir / bundle.records[0]["name"]
    path.write_bytes(path.read_bytes() + b"changed external artifact")
    with pytest.raises(runtime.RuntimeDependencyError): verify(installed)
    assert installed[-1] == []


def test_host_import_uses_the_canonical_reader_without_a_standalone_alias():
    assert adapter.__package__ == "launchkit.templates"
    assert adapter.CandidateBinding is reader.CandidateBinding
    assert adapter.read_candidate_artifacts is reader.read_candidate_artifacts
    assert adapter.CandidateBinding.__module__ == "launchkit.runtime_candidates"
    assert "runtime_candidates" not in sys.modules


def test_exported_standalone_import_uses_its_actual_sibling_reader(tmp_path):
    # This fresh Python imports source only. No SDK, gateway or installer action runs.
    (tmp_path / "install_candidate.py").write_bytes(Path(adapter.__file__).read_bytes())
    (tmp_path / "runtime_candidates.py").write_bytes(Path(reader.__file__).read_bytes())
    (tmp_path / "sdk_wheel_integrity.py").write_bytes(Path(reader.__file__).with_name("sdk_wheel_integrity.py").read_bytes())
    code = """import json, sys
from pathlib import Path
sys.path.insert(0, sys.argv[1])
import install_candidate
assert 'sdk_wheel_integrity' in sys.modules
assert Path(sys.modules['sdk_wheel_integrity'].__file__).parent == Path(sys.argv[1])
print(json.dumps({'package': install_candidate.__package__, 'binding': install_candidate.CandidateBinding.__module__, 'host_reader_loaded': 'launchkit.runtime_candidates' in sys.modules}))
"""
    result = subprocess.run([sys.executable, "-I", "-c", code, str(tmp_path)],
        check=True, capture_output=True, text=True)
    assert json.loads(result.stdout) == {"package": "", "binding": "runtime_candidates", "host_reader_loaded": False}
