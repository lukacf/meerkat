"""Candidate installed adapter tests use inert wheels/archives and mocked pip only."""
import base64
import copy
import csv
from dataclasses import asdict
import gzip
import importlib.util
import hashlib
import io
import json
import os
from pathlib import Path
import socket
import stat
import subprocess
import sys
import tarfile
import warnings
import zipfile

import pytest

from launchkit import runtime_candidates as candidates


VERSION = "0.8.48"
PLATFORM = "aarch64-apple-darwin"
PREFIX = f"meerkat_mobkit-{VERSION}.dist-info/"


def digest(value):
    return hashlib.sha256(value).hexdigest()


def dump(path, value):
    path.write_text(json.dumps(value, sort_keys=True) + "\n")
    return digest(path.read_bytes())


def native_bytes(binary, payload, *, name=None, kind=tarfile.REGTYPE, duplicate=False, executable=True, padding=0):
    raw = io.BytesIO()
    with tarfile.open(fileobj=raw, mode="w") as archive:
        member = tarfile.TarInfo(name or binary)
        member.type, member.mode = kind, 0o755 if executable else 0o644
        member.size = len(payload) if kind == tarfile.REGTYPE else 0
        member.linkname = "outside" if kind != tarfile.REGTYPE else ""
        archive.addfile(member, io.BytesIO(payload) if member.size else None)
        if duplicate:
            archive.addfile(member, io.BytesIO(payload) if member.size else None)
    return gzip.compress(raw.getvalue() + b"\0" * padding, mtime=0)


def wheel_bytes(values, *, record=None, duplicate=None, link=None):
    rows = []
    for name, value in values.items():
        hashed = base64.urlsafe_b64encode(hashlib.sha256(value).digest()).decode().rstrip("=")
        rows.append([name, "sha256=" + hashed, str(len(value))])
    rows.append([PREFIX + "RECORD", "", ""])
    output = io.StringIO()
    csv.writer(output, lineterminator="\n").writerows(rows)
    content = dict(values, **{PREFIX + "RECORD": output.getvalue().encode() if record is None else record})
    result = io.BytesIO()
    with warnings.catch_warnings(), zipfile.ZipFile(result, "w", zipfile.ZIP_DEFLATED) as archive:
        warnings.simplefilter("ignore", UserWarning)
        for name, value in content.items():
            entry = zipfile.ZipInfo(name)
            entry.external_attr = (stat.S_IFLNK | 0o777 if name == link else stat.S_IFREG | 0o644) << 16
            archive.writestr(entry, value)
        if duplicate:
            archive.writestr(duplicate, content[duplicate])
    return result.getvalue()


class Bundle:
    def __init__(self, root):
        self.root = root.resolve()
        self.root.mkdir()
        self.native_dir, self.sdk_dir = self.root / "native", self.root / "sdk"
        self.native_dir.mkdir()
        self.sdk_dir.mkdir()
        self.tuple_path = self.root / "tuple.json"
        self.payload = {"meerkat_mobkit/__init__.py": b"raise RuntimeError('SDK must never be imported')\n",
            "meerkat_mobkit/py.typed": b""}
        self.wheel_values = dict(self.payload, **{
            PREFIX + "METADATA": b"Metadata-Version: 2.1\nName: meerkat-mobkit\nVersion: 0.8.48\n\n",
            PREFIX + "WHEEL": b"Wheel-Version: 1.0\nRoot-Is-Purelib: true\nTag: py3-none-any\n\n",
            PREFIX + "top_level.txt": b"meerkat_mobkit\n"})
        self.wheel_path = self.sdk_dir / f"meerkat_mobkit-{VERSION}-py3-none-any.whl"
        self.wheel_path.write_bytes(wheel_bytes(self.wheel_values))
        self.records = []
        self.binary_payloads = {}
        for binary, prefix in candidates.BINARIES.items():
            payload = b"#!/bin/sh\nprintf 'NEVER EXECUTE THIS'\n" + binary.encode()
            self.binary_payloads[binary] = payload
            name = f"{prefix}-{VERSION}-{PLATFORM}.tar.gz"
            data = native_bytes(binary, payload)
            (self.native_dir / name).write_bytes(data)
            self.records.append({"binary": binary, "name": name, "target": PLATFORM, "size": len(data),
                "sha256": digest(data), "inner": {"name": binary, "size": len(payload), "sha256": digest(payload)}})
        for name in ("metadata.zip", "target.zip"):
            with zipfile.ZipFile(self.root / name, "w") as archive:
                archive.writestr("inert-proof.txt", b"Signature admission belongs to the caller.")
        for path in (self.root / "native-provenance.jsonl", self.native_dir / "provenance.jsonl"):
            path.write_bytes(b'{"inert":true}\n')
        self.metadata = {"id": 11, "manifest_path": str(self.root / "manifest.json"),
            "original_provenance_path": str(self.root / "native-provenance.jsonl"),
            "original_provenance_sha256": digest((self.root / "native-provenance.jsonl").read_bytes()),
            "path": str(self.root / "metadata.zip"), "outer_sha256": digest((self.root / "metadata.zip").read_bytes())}
        self.target = {"id": 12, "path": str(self.root / "target.zip"), "bytes": (self.root / "target.zip").stat().st_size,
            "outer_sha256": digest((self.root / "target.zip").read_bytes()), "archives": self.records,
            "original_provenance_path": str(self.native_dir / "provenance.jsonl"),
            "original_provenance_sha256": digest((self.native_dir / "provenance.jsonl").read_bytes())}
        self.identity = {"repository": "lukacf/meerkat-mobkit", "source_sha": "a" * 40,
            "native_version": VERSION, "core_version": "0.8.51", "platform": PLATFORM,
            "native_run_id": 23, "native_run_attempt": 1}
        self.manifest = {"kind": "candidate-manifest", "schema_version": 1,
            "repository": self.identity["repository"], "source_sha": self.identity["source_sha"],
            "version": VERSION, "tag": "v" + VERSION, "run_id": 23, "run_attempt": 1,
            "workflow": {"path": ".github/workflows/release.yml", "ref": "refs/heads/main", "sha": "a" * 40},
            "artifacts": [{"target": PLATFORM, "id": 12, "digest": self.target["outer_sha256"], "archives": self.records}]}
        self.intake = {"repository": self.identity["repository"], "source_sha": "a" * 40,
            "native_version": VERSION, "core_version": "0.8.51", "target": PLATFORM,
            "run_id": 23, "run_attempt": 1, "workflow": copy.deepcopy(self.manifest["workflow"]),
            "selection_path": str(self.root / "selection.json")}
        self.selection = {"kind": "accepted-candidate", "schema_version": 1,
            "repository": self.identity["repository"], "run_id": 23, "run_attempt": 1}
        self.sdk = {"origin": "local-build-from-exact-frozen-source", "signed": False, "published": False,
            "wheel": str(self.wheel_path), "source_payload": {name: digest(value) for name, value in self.payload.items()},
            "provenance": str(self.sdk_dir / "provenance.json"), "independent_review": str(self.sdk_dir / "review.json")}
        self.provenance = {"format": "forge-local-candidate-sdk-preparation-v1",
            "repository": self.identity["repository"], "source_sha": "a" * 40,
            "native_candidate_reference": {"run_id": 23, "run_attempt": 1},
            "sdk": {"name": "meerkat-mobkit", "version": VERSION, "origin": self.sdk["origin"],
                "signed": False, "published": False, "native_executable_included": False},
            "source_files": [{"path": "sdk/python/" + name, "sha256": digest(value), "bytes": len(value)}
                for name, value in self.payload.items()]}
        self.review = {"state": "inert test proof, not admission", "source_sha": "a" * 40}
        self.seal()

    def seal(self):
        self.identity["native_manifest_sha256"] = dump(self.root / "manifest.json", self.manifest)
        self.metadata["manifest_sha256"] = self.identity["native_manifest_sha256"]
        self.intake["metadata_artifact"], self.intake["target_artifact"] = copy.deepcopy(self.metadata), copy.deepcopy(self.target)
        self.selection["manifest_artifact"] = {"id": self.metadata["id"], "digest": self.metadata["outer_sha256"],
            "manifest_sha256": self.metadata["manifest_sha256"]}
        native = {"origin": "github-actions-signed-candidate", "metadata": self.metadata, "target": self.target,
            "intake": str(self.root / "intake.json"), "selection": str(self.root / "selection.json"),
            "intake_sha256": dump(self.root / "intake.json", self.intake),
            "selection_sha256": dump(self.root / "selection.json", self.selection)}
        wheel = self.wheel_path.read_bytes()
        self.identity["sdk_wheel_sha256"] = self.sdk["wheel_sha256"] = digest(wheel)
        self.sdk["bytes"] = len(wheel)
        self.provenance["sdk"].update({"sha256": digest(wheel), "bytes": len(wheel)})
        self.sdk["provenance_sha256"] = dump(self.sdk_dir / "provenance.json", self.provenance)
        self.review.update({"provenance_sha256": self.sdk["provenance_sha256"], "wheel_sha256": digest(wheel)})
        self.sdk["independent_review_sha256"] = dump(self.sdk_dir / "review.json", self.review)
        self.value = {"format": "forge-private-consumer-tuple-preparation-v1", "installation_admitted": False,
            "identity": self.identity, "native": native, "sdk": self.sdk}
        hashed = dump(self.tuple_path, self.value)
        self.binding = candidates.CandidateBinding(tuple_sha256=hashed, **self.identity)

    def read(self, **kwargs):
        return candidates.read_candidate_artifacts(self.tuple_path, accepted=kwargs.get("accepted", self.binding),
            artifact_root=kwargs.get("artifact_root", self.root))

    def replace_native(self, data):
        record = self.records[0]
        (self.native_dir / record["name"]).write_bytes(data)
        record.update({"size": len(data), "sha256": digest(data)})
        self.seal()


@pytest.fixture
def installed(tmp_path, monkeypatch):
    bundle = Bundle(tmp_path / "external-artifacts")
    root = tmp_path / "application"
    root.mkdir(mode=0o700)
    source = Path(__file__).resolve().parents[1] / "launchkit/templates/install_candidate.py"
    (root / "install_candidate.py").write_bytes(source.read_bytes())
    monkeypatch.setitem(sys.modules, "runtime_candidates", candidates)
    spec = importlib.util.spec_from_file_location("isolated_candidate_adapter", root / "install_candidate.py")
    module = importlib.util.module_from_spec(spec)
    monkeypatch.setitem(sys.modules, spec.name, module)
    spec.loader.exec_module(module)
    monkeypatch.setattr(module, "platform_target", lambda: PLATFORM)
    for name in module.OVERRIDES:
        monkeypatch.delenv(name, raising=False)
    selection = root / "runtime-candidate.json"
    dump(selection, {"format": "forge-runtime-candidate-selection-v1", "tuple_path": str(bundle.tuple_path),
        "artifact_root": str(bundle.root), "accepted": asdict(bundle.binding)})
    dump(root / "deployment.json", {"mobkitVersion": VERSION, "runtimeCandidate": {
        "source": "signed-candidate", "selection": selection.name, "tuple_sha256": bundle.binding.tuple_sha256}})
    (root / "requirements.txt").write_text(f"meerkat-mobkit=={VERSION}\nhttpx==0.28.1\n")
    site = root / ".venv/lib/python-fixture/site-packages"
    site.mkdir(parents=True)
    (root / ".venv/bin").mkdir()
    (root / ".venv/bin/python").symlink_to(sys.executable)
    # These inputs are ordinary application files. No install operation may change them.
    for name, value in ((".env", "PRIVATE_FIXTURE=kept\n"), ("application.py", "raise RuntimeError('do not execute')\n")):
        (root / name).write_text(value)
    (root / "data").mkdir()
    (root / "data/retained.txt").write_text("Retained state remains exact.\n")
    calls = []
    stdlib_run = subprocess.run
    def mock_pip(argv, **kwargs):
        if argv[1:3] != ["-m", "pip"]:
            assert argv[:3] == [str(root / ".venv/bin/python"), "-I", "-S"]
            return stdlib_run(argv, **kwargs)
        calls.append(argv)
        assert argv[:3] == [str(root / ".venv/bin/python"), "-m", "pip"]
        assert "install" in argv
        if "--no-index" in argv:
            assert "--no-deps" in argv and "--no-compile" in argv
            wheel = Path(argv[-1])
            assert wheel.read_bytes() == bundle.wheel_path.read_bytes()
            assert stat.S_IMODE(wheel.stat().st_mode) == 0o600
            # Inert file copying emulates pip installation; SDK code is never imported.
            with zipfile.ZipFile(wheel) as archive:
                for name in archive.namelist():
                    destination = site / name
                    destination.parent.mkdir(parents=True, exist_ok=True)
                    destination.write_bytes(archive.read(name))
        else:
            assert "-r" in argv
            requirements = Path(argv[-1])
            assert requirements.read_text() == "httpx==0.28.1\n"
            assert stat.S_IMODE(requirements.stat().st_mode) == 0o600
        return subprocess.CompletedProcess(argv, 0)
    monkeypatch.setattr(module.subprocess, "run", mock_pip)
    return module, bundle, root, site, calls


def sdk_install(installed):
    module, _, root, site, _ = installed
    return module.install_candidate_sdk(root / "runtime-candidate.json", site_packages=site)


def native_install(installed):
    module, _, root, site, _ = installed
    sdk_install(installed)
    return module.install_candidate_native_tools(root / "runtime-candidate.json", site_packages=site)


def test_exact_local_sdk_and_native_install_have_honest_binding_without_execution(installed, monkeypatch):
    module, bundle, root, site, calls = installed
    preserved = {name: (root / name).read_bytes() for name in (".env", "application.py", "data/retained.txt")}
    sdk = sdk_install(installed)
    assert sdk["source"] == "local-build-from-exact-frozen-source"
    assert sdk["signed"] is sdk["published"] is False
    assert sdk["candidate_binding"] == asdict(bundle.binding)
    assert sdk["payload_sha256"] == bundle.sdk["source_payload"]
    module.install_candidate_requirements(root / "runtime-candidate.json", site_packages=site)
    assert len(calls) == 2
    report = module.install_candidate_native_tools(root / "runtime-candidate.json", site_packages=site)
    assert report["source"] == "signed-candidate" and set(report["native"]) == set(module.BINARIES)
    assert report["candidate_tuple_sha256"] == bundle.binding.tuple_sha256
    for name, receipt in report["native"].items():
        assert receipt["source"] == "signed-candidate"
        assert receipt["candidate_binding"] == asdict(bundle.binding)
        assert receipt["sdk_wheel_sha256"] == bundle.binding.sdk_wheel_sha256
        assert "release_url" not in receipt
        assert (root / ".bin" / name).read_bytes() == bundle.binary_payloads[name]
        assert stat.S_IMODE((root / ".bin" / name).stat().st_mode) == 0o700
        assert stat.S_IMODE((root / ".bin" / (name + ".receipt.json")).stat().st_mode) == 0o600
    assert stat.S_IMODE((root / ".bin").stat().st_mode) == 0o700
    assert stat.S_IMODE((root / ".venv/forge-sdk.receipt.json").stat().st_mode) == 0o600
    assert not (root / ".bin/mobkit_repair").exists()
    monkeypatch.setattr(module.subprocess, "run", lambda *a, **kw: pytest.fail("Verifier must not run pip or native executables"))
    assert module.verify_installed_candidate_runtime(root / "runtime-candidate.json", site_packages=site) == report
    assert preserved == {name: (root / name).read_bytes() for name in preserved}


@pytest.mark.parametrize("defect", ["duplicate", "extra-field", "missing-accepted", "wrong-format", "wrong-filename",
    "relative-tuple", "traversal-tuple", "relative-root", "selection-symlink", "selection-hardlink", "selection-writable",
    "deployment-missing", "deployment-source", "deployment-digest", "deployment-version", "deployment-extra",
    "sdk-wheel", "source", "version", "core", "platform", "run", "run-attempt", "manifest"])
def test_selection_and_protected_deployment_refuse_before_pip_or_native_writes(installed, defect):
    module, _, root, site, calls = installed
    path = root / "runtime-candidate.json"
    value = json.loads(path.read_text())
    deployment_path = root / "deployment.json"
    deployment = json.loads(deployment_path.read_text())
    if defect == "duplicate":
        path.write_text(path.read_text().replace('{', '{"format":"bad",', 1))
    elif defect in {"selection-symlink", "selection-hardlink"}:
        other = root / "selection-copy.json"
        path.replace(other)
        path.symlink_to(other) if defect.endswith("symlink") else path.hardlink_to(other)
    elif defect == "selection-writable":
        path.chmod(0o666)
    elif defect == "wrong-filename":
        other = root / "alternate.json"
        other.write_bytes(path.read_bytes())
        path = other
    elif defect.startswith("deployment-"):
        key = defect.removeprefix("deployment-")
        if key == "missing":deployment.pop("runtimeCandidate")
        elif key == "source":deployment["runtimeCandidate"]["source"] = "official-release"
        elif key == "digest":deployment["runtimeCandidate"]["tuple_sha256"] = "0" * 64
        elif key == "version":deployment["mobkitVersion"] = "0.8.47"
        else:deployment["runtimeCandidate"]["verified"] = True
        dump(deployment_path, deployment)
    else:
        if defect == "extra-field":value["verified"] = True
        elif defect == "missing-accepted":value.pop("accepted")
        elif defect == "wrong-format":value["format"] = "accepted-candidate"
        elif defect == "relative-tuple":value["tuple_path"] = "tuple.json"
        elif defect == "traversal-tuple":value["tuple_path"] = value["artifact_root"] + "/../external-artifacts/tuple.json"
        elif defect == "relative-root":value["artifact_root"] = "external-artifacts"
        else:
            field, changed = {"sdk-wheel": ("sdk_wheel_sha256", "0" * 64), "source": ("source_sha", "b" * 40),
                "version": ("native_version", "0.8.47"), "core": ("core_version", "0.8.52"),
                "platform": ("platform", "x86_64-apple-darwin"), "run": ("native_run_id", 24),
                "run-attempt": ("native_run_attempt", 2), "manifest": ("native_manifest_sha256", "0" * 64)}[defect]
            value["accepted"][field] = changed
        dump(path, value)
    with pytest.raises((candidates.CandidateArtifactError, OSError)):
        module.install_candidate_sdk(path, site_packages=site)
    assert calls == [] and not (root / ".bin").exists()


@pytest.mark.parametrize("name", ["MOBKIT_RPC_GATEWAY_BIN", "MOBKIT_GATEWAY_BIN", "META_FORGE_RPC_GATEWAY_BIN"])
def test_gateway_override_never_enables_candidate_install(installed, monkeypatch, name):
    module, _, root, site, calls = installed
    monkeypatch.setenv(name, "/unrelated/executable")
    with pytest.raises(candidates.CandidateArtifactError, match="overrides"):
        module.install_candidate_sdk(root / "runtime-candidate.json", site_packages=site)
    assert calls == []


@pytest.mark.parametrize("defect", ["payload", "extra-source", "metadata", "wheel-metadata", "top-level", "duplicate-distribution",
    "package-link", "payload-link", "payload-hardlink", "receipt-link", "receipt-hardlink", "receipt-writable",
    "receipt-origin", "receipt-sdk", "receipt-source", "receipt-run", "receipt-version", "wheel-tamper"])
def test_installed_sdk_verifier_rechecks_actual_bytes_and_honest_receipt(installed, defect):
    module, bundle, root, site, _ = installed
    sdk_install(installed)
    payload = site / "meerkat_mobkit/__init__.py"
    receipt = root / ".venv/forge-sdk.receipt.json"
    if defect == "payload":payload.write_text("Changed source\n")
    elif defect == "extra-source":(payload.parent / "unexpected.py").write_text("Unexpected module\n")
    elif defect in {"metadata", "wheel-metadata", "top-level"}:
        field = {"metadata": "METADATA", "wheel-metadata": "WHEEL", "top-level": "top_level.txt"}[defect]
        (site / PREFIX / field).write_text("Changed metadata\n")
    elif defect == "duplicate-distribution":(site / "meerkat_mobkit-0.8.47.dist-info").mkdir()
    elif defect == "package-link":
        payload.parent.rename(site / "outside-package")
        (site / "meerkat_mobkit").symlink_to(site / "outside-package", target_is_directory=True)
    elif defect in {"payload-link", "payload-hardlink", "receipt-link", "receipt-hardlink"}:
        target = receipt if defect.startswith("receipt") else payload
        other = root / ("outside-" + target.name)
        target.replace(other)
        target.symlink_to(other) if defect.endswith("-link") else target.hardlink_to(other)
    elif defect == "receipt-writable":receipt.chmod(0o644)
    elif defect == "wheel-tamper":bundle.wheel_path.write_bytes(b"Changed selected wheel")
    else:
        value = json.loads(receipt.read_text())
        if defect == "receipt-origin":value["source"] = "official-release"
        elif defect == "receipt-sdk":value["sdk_wheel_sha256"] = "0" * 64
        elif defect == "receipt-source":value["candidate_binding"]["source_sha"] = "b" * 40
        elif defect == "receipt-run":value["candidate_binding"]["native_run_id"] = 24
        else:value["version"] = "0.8.47"
        dump(receipt, value)
    with pytest.raises(candidates.CandidateArtifactError):
        module.verify_installed_candidate_sdk(root / "runtime-candidate.json", site_packages=site)


@pytest.mark.parametrize("defect", ["binary", "receipt", "version", "platform", "run", "source", "sdk", "binary-link", "receipt-link",
    "binary-hardlink", "receipt-hardlink", "binary-public", "receipt-public", "nonexecutable", "directory-public", "directory-link"])
def test_native_verifier_rechecks_installed_hashes_permissions_and_exact_binding(installed, defect):
    module, _, root, site, _ = installed
    native_install(installed)
    directory = root / ".bin"
    binary, receipt = directory / "rpc_gateway", directory / "rpc_gateway.receipt.json"
    if defect == "binary":binary.write_bytes(b"Changed native bytes")
    elif defect in {"binary-link", "receipt-link", "binary-hardlink", "receipt-hardlink"}:
        target = receipt if defect.startswith("receipt") else binary
        other = root / ("outside-" + target.name)
        target.replace(other)
        target.symlink_to(other) if defect.endswith("-link") else target.hardlink_to(other)
    elif defect == "directory-link":
        directory.replace(root / "outside-tools")
        directory.symlink_to(root / "outside-tools", target_is_directory=True)
    elif defect == "directory-public":directory.chmod(0o755)
    elif defect.endswith("-public"):(receipt if defect.startswith("receipt") else binary).chmod(0o755)
    elif defect == "nonexecutable":binary.chmod(0o600)
    else:
        value = json.loads(receipt.read_text())
        if defect == "receipt":value["archive_sha256"] = "0" * 64
        elif defect == "version":value["version"] = "0.8.47"
        elif defect == "platform":value["platform"] = "x86_64-apple-darwin"
        elif defect == "run":value["candidate_binding"]["native_run_attempt"] = 2
        elif defect == "source":value["source"] = "official-release"
        else:value["sdk_wheel_sha256"] = "0" * 64
        dump(receipt, value)
    with pytest.raises(candidates.CandidateArtifactError):
        module.verify_installed_candidate_runtime(root / "runtime-candidate.json", site_packages=site)


def test_all_native_archives_and_destinations_checked_before_any_replacement(installed):
    module, bundle, root, site, _ = installed
    native_install(installed)
    saved = {p.name: p.read_bytes() for p in (root / ".bin").iterdir()}
    target = next(record for record in bundle.records if record["binary"] == "rpc_gateway")
    (bundle.native_dir / target["name"]).write_bytes(b"Changed second archive")
    with pytest.raises(candidates.CandidateArtifactError):
        module.install_candidate_native_tools(root / "runtime-candidate.json", site_packages=site)
    assert saved == {p.name: p.read_bytes() for p in (root / ".bin").iterdir()}


@pytest.mark.parametrize("text", ["httpx==0.28.1\n", "meerkat-mobkit==0.8.47\n", "meerkat-mobkit==0.8.48\nmeerkat-mobkit==0.8.48\n",
    "meerkat_mobkit==0.8.48\n", "meerkat-mobkit==0.8.48\n--index-url https://unrelated.invalid\n",
    "meerkat-mobkit==0.8.48\nhttpx>=0.28\n"])
def test_requirements_do_not_replace_selected_sdk_or_expand_resolver_input(installed, text):
    module, _, root, site, calls = installed
    sdk_install(installed)
    calls.clear()
    (root / "requirements.txt").write_text(text)
    with pytest.raises(candidates.CandidateArtifactError):
        module.install_candidate_requirements(root / "runtime-candidate.json", site_packages=site)
    assert calls == []


def test_ordinary_requirements_that_tamper_with_sdk_are_not_accepted(installed, monkeypatch):
    module, _, root, site, _ = installed
    sdk_install(installed)
    def tampering_pip(argv, **kwargs):
        assert "-r" in argv
        (site / "meerkat_mobkit/__init__.py").write_text("Dependency resolver changed SDK\n")
        return subprocess.CompletedProcess(argv, 0)
    monkeypatch.setattr(module.subprocess, "run", tampering_pip)
    with pytest.raises(candidates.CandidateArtifactError, match="payload"):
        module.install_candidate_requirements(root / "runtime-candidate.json", site_packages=site)


def test_native_install_requires_sdk_receipt_and_selected_local_environment(installed):
    module, _, root, site, calls = installed
    with pytest.raises(OSError):
        module.install_candidate_native_tools(root / "runtime-candidate.json", site_packages=site)
    with pytest.raises(candidates.CandidateArtifactError, match="selected virtual"):
        module.install_candidate_sdk(root / "runtime-candidate.json", sdk_python=Path(sys.executable), site_packages=site)
    assert calls == [] and not (root / ".bin").exists()


def test_verified_matching_bytecode_is_allowed_but_tampered_bytecode_is_rejected(installed):
    module, _, root, site, _ = installed
    sdk_install(installed)
    path = site / "meerkat_mobkit/__init__.py"
    cache = Path(importlib.util.cache_from_source(str(path)))
    cache.parent.mkdir()
    code = compile(path.read_bytes(), str(path), "exec", dont_inherit=True)
    cache.write_bytes(importlib.util.MAGIC_NUMBER + b"\0" * 12 + __import__("marshal").dumps(code))
    module.verify_installed_candidate_sdk(root / "runtime-candidate.json", site_packages=site)
    changed = compile("raise RuntimeError('tampered cached code')", str(path), "exec", dont_inherit=True)
    cache.write_bytes(importlib.util.MAGIC_NUMBER + b"\0" * 12 + __import__("marshal").dumps(changed))
    with pytest.raises(candidates.CandidateArtifactError, match="bytecode differs"):
        module.verify_installed_candidate_sdk(root / "runtime-candidate.json", site_packages=site)


def test_bytecode_format_comes_from_selected_python_not_host_interpreter(installed, monkeypatch):
    module, _, root, site, _ = installed
    sdk_install(installed)
    path = site / "meerkat_mobkit/__init__.py"
    cache = Path(importlib.util.cache_from_source(str(path)))
    cache.parent.mkdir()
    code = compile(path.read_bytes(), str(path), "exec", dont_inherit=True)
    cache.write_bytes(importlib.util.MAGIC_NUMBER + b"\0" * 12 + __import__("marshal").dumps(code))
    # A caller with a different bytecode format must not reject deployed caches.
    monkeypatch.setattr(module.importlib.util, "MAGIC_NUMBER", b"HOST")
    monkeypatch.setattr(module, "compile", lambda *a, **kw: pytest.fail("Caller must not compile deployed bytecode"), raising=False)
    module.verify_installed_candidate_sdk(root / "runtime-candidate.json", site_packages=site)


def cached_sources(tmp_path, count):
    from launchkit.templates import install_candidate as module
    site = tmp_path / "site"
    package = site / "inert_sdk"
    package.mkdir(parents=True)
    sentinel = tmp_path / "source-was-executed"
    expected, entries = {}, {}
    for index in range(count):
        path = package / f"module_{index}.py"
        source = (f"assert False\nfrom pathlib import Path\nPath({str(sentinel)!r}).write_text('Forbidden execution')\n").encode()
        path.write_bytes(source)
        optimize = index % 3
        cache = Path(importlib.util.cache_from_source(str(path), optimization=str(optimize) if optimize else ""))
        cache.parent.mkdir(exist_ok=True)
        cache.write_bytes(importlib.util.MAGIC_NUMBER + b"\0" * 12 + __import__("marshal").dumps(
            compile(source, str(path), "exec", dont_inherit=True, optimize=optimize)))
        expected[path.relative_to(site).as_posix()] = digest(source)
        entries[str(path)] = {"cache": cache, "source": source, "optimize": optimize}
    return module, site, expected, entries, sentinel


def test_cache_inventory_uses_bounded_batches_with_every_exact_input(tmp_path, monkeypatch):
    module, site, expected, entries, sentinel = cached_sources(tmp_path, 65)
    selected_python = tmp_path / "already-selected-python"
    batches = []
    def selected(argv, **options):
        assert argv[:4] == [str(selected_python), "-I", "-S", "-c"]
        assert options["timeout"] == module.STDLIB_CHECK_TIMEOUT
        rows = json.loads(options["input"])
        batches.append(rows if isinstance(rows, list) else [rows])
        return subprocess.CompletedProcess(argv, 0, '{"valid": true}')
    monkeypatch.setattr(module.subprocess, "run", selected)
    module._compare_sdk_payload(site, selected_python, "inert_sdk", expected)
    assert list(map(len, batches)) == [64, 1]
    rows = [row for batch in batches for row in batch]
    assert {row["filename"] for row in rows} == entries.keys() and len(rows) == len(entries)
    for row in rows:
        original = entries[row["filename"]]
        assert base64.b64decode(row["cache"]) == original["cache"].read_bytes()
        assert base64.b64decode(row["source"]) == original["source"]
        assert row["optimize"] == original["optimize"]
    assert not sentinel.exists()


@pytest.mark.parametrize("change", ["none", "magic", "source", "filename", "optimization"])
def test_selected_interpreter_checks_every_cache_in_one_batch(tmp_path, monkeypatch, change):
    module, site, expected, entries, sentinel = cached_sources(tmp_path, 3)
    filename, row = list(entries.items())[-1]
    if change == "magic":
        value = row["cache"].read_bytes()
        row["cache"].write_bytes(b"FAKE" + value[4:])
    elif change != "none":
        code = compile(b"raise RuntimeError('Changed cache')\n" if change == "source" else row["source"],
            filename + ".foreign" if change == "filename" else filename, "exec", dont_inherit=True,
            optimize=0 if change == "optimization" else row["optimize"])
        row["cache"].write_bytes(importlib.util.MAGIC_NUMBER + b"\0" * 12 + __import__("marshal").dumps(code))
    original, calls = module.subprocess.run, []
    def sterile(argv, **options):
        calls.append(argv)
        assert argv[:4] == [sys.executable, "-I", "-S", "-c"]
        assert len(json.loads(options["input"])) == 3
        assert options["timeout"] == module.STDLIB_CHECK_TIMEOUT
        return original(argv, **options)
    monkeypatch.setattr(module.subprocess, "run", sterile)
    if change in {"none", "filename"}:
        # Original code-object equality permits relocated cached filenames.
        module._compare_sdk_payload(site, Path(sys.executable), "inert_sdk", expected)
    else:
        with pytest.raises(candidates.CandidateArtifactError, match="bytecode differs"):
            module._compare_sdk_payload(site, Path(sys.executable), "inert_sdk", expected)
    assert len(calls) == 1 and not sentinel.exists()


def test_bytecode_batch_size_keeps_single_larger_file_valid(tmp_path, monkeypatch):
    module, site, expected, entries, _ = cached_sources(tmp_path, 2)
    monkeypatch.setattr(module, "BYTECODE_BATCH_BYTES", 1)
    batches = []
    def selected(argv, **options):
        request = json.loads(options["input"])
        assert isinstance(request, dict)
        batches.append(request)
        return subprocess.CompletedProcess(argv, 0, '{"valid": true}')
    monkeypatch.setattr(module.subprocess, "run", selected)
    module._compare_sdk_payload(site, Path(sys.executable), "inert_sdk", expected)
    assert len(batches) == 2 and {row["filename"] for row in batches} == entries.keys()


def test_batched_product_checks_keep_the_original_selected_executor(tmp_path, monkeypatch):
    module, site, expected, entries, _ = cached_sources(tmp_path, 3)
    calls = []
    def forbidden(*args, **options):
        pytest.fail("The original Supervisor executor owns selected product checks")
    monkeypatch.setattr(module.subprocess, "run", forbidden)
    def original_owner(argv, *, input, pip):
        assert argv[:4] == [sys.executable, "-I", "-S", "-c"] and pip is False
        rows = json.loads(input)
        assert {row["filename"] for row in rows} == entries.keys()
        calls.append(rows)
        return {"returncode": 0, "stdout": b'{"valid": true}', "stderr": b""}
    module._compare_sdk_payload(site, Path(sys.executable), "inert_sdk", expected,
        execute_command=original_owner)
    assert len(calls) == 1


def test_empty_cache_inventory_never_launches_a_bytecode_child(tmp_path, monkeypatch):
    module, site, expected, entries, _ = cached_sources(tmp_path, 1)
    for row in entries.values():
        row["cache"].unlink()
    def forbidden(*args, **options):
        pytest.fail("No bytecode means no comparison child")
    monkeypatch.setattr(module.subprocess, "run", forbidden)
    module._compare_sdk_payload(site, Path(sys.executable), "inert_sdk", expected)


def test_bytecode_timeout_reaps_its_exact_direct_sterile_child(tmp_path, monkeypatch):
    module, site, expected, _, sentinel = cached_sources(tmp_path, 1)
    selected_python = tmp_path / "delayed-stdlib-python"
    selected_python.write_text(f"#!{sys.executable}\nimport time\ntime.sleep(60)\n")
    selected_python.chmod(0o700)
    monkeypatch.setattr(module, "STDLIB_CHECK_TIMEOUT", 0.05)
    original, children = module.subprocess.Popen, []
    def record_child(*args, **options):
        child = original(*args, **options)
        children.append(child)
        return child
    monkeypatch.setattr(module.subprocess, "Popen", record_child)
    with pytest.raises(subprocess.TimeoutExpired):
        module._compare_sdk_payload(site, selected_python, "inert_sdk", expected)
    assert len(children) == 1 and children[0].poll() is not None and not sentinel.exists()
