"""Inert candidate artifact counterexamples; no real runtime or SDK is used."""
import base64
import copy
import csv
from dataclasses import FrozenInstanceError, replace
import gzip
import hashlib
import io
import json
import os
from pathlib import Path
import socket
import stat
import subprocess
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
def bundle(tmp_path):
    return Bundle(tmp_path / "external-artifacts")


def test_exact_inert_bundle_returns_frozen_honest_metadata_without_execution_or_changes(bundle, monkeypatch):
    def forbidden(*args, **kwargs):
        raise AssertionError("Artifacts must not be extracted, executed, or sent over the network")
    for owner, name in ((subprocess, "run"), (subprocess, "Popen"), (os, "system"),
            (socket, "create_connection"), (zipfile.ZipFile, "extract"), (zipfile.ZipFile, "extractall"),
            (tarfile.TarFile, "extract"), (tarfile.TarFile, "extractall")):
        monkeypatch.setattr(owner, name, forbidden)
    def snapshot():
        return {str(path): (path.read_bytes(), path.stat().st_mode, path.stat().st_mtime_ns, path.stat().st_ino)
            for path in bundle.root.rglob("*") if path.is_file()}
    before = snapshot()
    result = bundle.read()
    assert snapshot() == before
    assert result.identity == bundle.binding and result.native_origin == "signed-candidate"
    assert {item.binary for item in result.native} == set(candidates.BINARIES)
    assert result.sdk.payload == tuple(sorted(bundle.sdk["source_payload"].items()))
    assert result.sdk.origin == "local-build-from-exact-frozen-source"
    assert result.sdk.signed is result.sdk.published is False
    assert all(item.archive.path.parent == bundle.native_dir for item in result.native)
    assert all(item.path.is_relative_to(bundle.root) for item in result.evidence)
    with pytest.raises(FrozenInstanceError):
        result.identity = None
    with pytest.raises(FrozenInstanceError):
        result.sdk.wheel.size = 0


def test_a_json_verified_flag_cannot_replace_an_independent_binding(bundle):
    bundle.value["verified"] = True
    bundle.value["installation_admitted"] = True
    dump(bundle.tuple_path, bundle.value)
    with pytest.raises(candidates.CandidateArtifactError, match="bytes"):
        bundle.read()
    with pytest.raises(candidates.CandidateArtifactError, match="explicit"):
        bundle.read(accepted={"verified": True})


@pytest.mark.parametrize("field,value", [("source_sha", "b" * 40), ("native_version", "0.8.49"),
    ("core_version", "0.8.50"), ("platform", "x86_64-apple-darwin"), ("native_run_id", 24),
    ("native_run_attempt", 2), ("native_manifest_sha256", "0" * 64), ("sdk_wheel_sha256", "0" * 64)])
def test_caller_binding_identity_drift_is_rejected(bundle, field, value):
    with pytest.raises(candidates.CandidateArtifactError):
        bundle.read(accepted=replace(bundle.binding, **{field: value}))


@pytest.mark.parametrize("object_name,field,value", [
    ("manifest", "source_sha", "b" * 40), ("manifest", "version", "0.8.49"),
    ("manifest", "run_id", 24), ("manifest", "run_attempt", True),
    ("intake", "core_version", "0.8.50"), ("intake", "target", "x86_64-apple-darwin"),
    ("selection", "run_id", 24), ("selection", "run_attempt", True),
    ("provenance", "source_sha", "b" * 40), ("review", "source_sha", "b" * 40)])
def test_rehashed_proof_cannot_drift_from_exact_tuple_binding(bundle, object_name, field, value):
    getattr(bundle, object_name)[field] = value
    bundle.seal()
    with pytest.raises(candidates.CandidateArtifactError):
        bundle.read()


@pytest.mark.parametrize("field,value", [("native_run_attempt", True), ("native_run_id", 0),
    ("platform", "x86_64-pc-windows-msvc"), ("source_sha", "HEAD"),
    ("repository", "other/repository"), ("native_version", "latest")])
def test_binding_has_canonical_types_and_supported_platform(bundle, field, value):
    with pytest.raises(candidates.CandidateArtifactError):
        bundle.read(accepted=replace(bundle.binding, **{field: value}))


@pytest.mark.parametrize("change", ["duplicate-target", "duplicate-binary", "missing-binary", "inner-hash", "inner-size", "archive-name", "artifact-id", "workflow", "boolean-identity", "sdk-run", "sdk-version", "sdk-inventory", "sdk-source-size", "sdk-source-duplicate", "sdk-signed"])
def test_manifest_native_and_sdk_binding_counterexamples(bundle, change):
    if change == "duplicate-target":
        bundle.manifest["artifacts"].append(copy.deepcopy(bundle.manifest["artifacts"][0]))
    elif change == "duplicate-binary":
        bundle.records[1] = copy.deepcopy(bundle.records[0])
    elif change == "missing-binary":
        bundle.records.pop()
    elif change == "inner-hash":
        bundle.records[0]["inner"]["sha256"] = "0" * 64
    elif change == "inner-size":
        bundle.records[0]["inner"]["size"] += 1
    elif change == "archive-name":
        bundle.records[0]["name"] = "../outside.tar.gz"
    elif change == "artifact-id":
        bundle.target["id"] += 1
    elif change == "workflow":
        bundle.manifest["workflow"]["sha"] = "b" * 40
    elif change == "boolean-identity":
        bundle.identity["native_run_attempt"] = True
    elif change == "sdk-run":
        bundle.provenance["native_candidate_reference"]["run_id"] += 1
    elif change == "sdk-version":
        bundle.provenance["sdk"]["version"] = "0.8.47"
    elif change == "sdk-inventory":
        bundle.sdk["source_payload"].pop("meerkat_mobkit/py.typed")
    elif change == "sdk-source-size":
        bundle.provenance["source_files"][0]["bytes"] += 1
    elif change == "sdk-source-duplicate":
        bundle.provenance["source_files"].append(copy.deepcopy(bundle.provenance["source_files"][0]))
    elif change == "sdk-signed":
        bundle.sdk["signed"] = True
    bundle.seal()
    with pytest.raises(candidates.CandidateArtifactError):
        bundle.read()


@pytest.mark.parametrize("change", ["name", "nested", "symlink", "hardlink", "directory", "duplicate", "not-executable", "corrupt"])
def test_native_archive_is_one_flat_inert_payload(bundle, change):
    binary = bundle.records[0]["binary"]
    kwargs = {"name": "other"} if change == "name" else {"name": "nested/" + binary} if change == "nested" else {}
    if change in ("symlink", "hardlink", "directory"):
        kwargs["kind"] = {"symlink": tarfile.SYMTYPE, "hardlink": tarfile.LNKTYPE, "directory": tarfile.DIRTYPE}[change]
    if change == "duplicate":
        kwargs["duplicate"] = True
    if change == "not-executable":
        kwargs["executable"] = False
    data = b"not a gzip archive" if change == "corrupt" else native_bytes(binary, bundle.binary_payloads[binary], **kwargs)
    bundle.replace_native(data)
    with pytest.raises(candidates.CandidateArtifactError):
        bundle.read()


@pytest.mark.parametrize("change", ["metadata-version", "metadata-duplicate", "wheel-tag", "record-omitted", "record-hash", "record-duplicate", "payload", "unexpected", "traversal", "link", "duplicate", "corrupt"])
def test_wheel_structure_record_and_exact_source_payload(bundle, change):
    values, kwargs = dict(bundle.wheel_values), {}
    if change == "metadata-version":
        values[PREFIX + "METADATA"] = values[PREFIX + "METADATA"].replace(b"0.8.48", b"0.8.47")
    elif change == "metadata-duplicate":
        values[PREFIX + "METADATA"] = b"Name: meerkat-mobkit\nName: meerkat-mobkit\nVersion: 0.8.48\n"
    elif change == "wheel-tag":
        values[PREFIX + "WHEEL"] = values[PREFIX + "WHEEL"].replace(b"py3-none-any", b"cp314-macos")
    elif change == "record-omitted":
        kwargs["record"] = b""
    elif change == "record-hash":
        kwargs["record"] = b"broken,sha256=bad,1\n"
    elif change == "record-duplicate":
        row = (PREFIX + "RECORD,,\n").encode()
        kwargs["record"] = row + row
    elif change == "payload":
        values["meerkat_mobkit/__init__.py"] += b"# unrelated source\n"
    elif change in ("unexpected", "traversal"):
        values["other.py" if change == "unexpected" else "meerkat_mobkit/../escape.py"] = b"x"
    elif change == "link":
        kwargs["link"] = "meerkat_mobkit/__init__.py"
    elif change == "duplicate":
        kwargs["duplicate"] = "meerkat_mobkit/__init__.py"
    bundle.wheel_path.write_bytes(b"not a ZIP" if change == "corrupt" else wheel_bytes(values, **kwargs))
    bundle.seal()
    with pytest.raises(candidates.CandidateArtifactError):
        bundle.read()


@pytest.mark.parametrize("change", ["file-link", "parent-link", "hardlink", "outside-root", "dotdot", "missing", "directory", "fifo", "bytes"])
def test_external_paths_are_owned_ordinary_and_exact(bundle, tmp_path, change):
    path = bundle.wheel_path
    if change == "file-link":
        renamed = path.with_suffix(".retained")
        path.rename(renamed)
        path.symlink_to(renamed)
    elif change == "parent-link":
        renamed = bundle.sdk_dir.with_name("real-sdk")
        bundle.sdk_dir.rename(renamed)
        bundle.sdk_dir.symlink_to(renamed, target_is_directory=True)
    elif change == "hardlink":
        os.link(path, path.with_suffix(".second"))
    elif change in ("outside-root", "dotdot"):
        external = tmp_path / path.name
        external.write_bytes(path.read_bytes())
        bundle.value["sdk"]["wheel"] = str(external) if change == "outside-root" else str(bundle.root / ".." / path.name)
        bundle.binding = replace(bundle.binding, tuple_sha256=dump(bundle.tuple_path, bundle.value))
    elif change == "missing":
        path.unlink()
    elif change == "directory":
        path.unlink()
        path.mkdir()
    elif change == "fifo":
        path.unlink()
        os.mkfifo(path)
    elif change == "bytes":
        path.write_bytes(path.read_bytes() + b"drift")
    with pytest.raises(candidates.CandidateArtifactError):
        bundle.read()


@pytest.mark.parametrize("bound,value", [("MAX_ARCHIVE_BYTES", 8), ("MAX_BINARY_BYTES", 8),
    ("MAX_WHEEL_BYTES", 8), ("MAX_WHEEL_TOTAL", 8), ("MAX_WHEEL_FILES", 1), ("MAX_METADATA_BYTES", 8)])
def test_existing_artifact_bounds_fail_before_unbounded_reads(bundle, monkeypatch, bound, value):
    monkeypatch.setattr(candidates, bound, value)
    with pytest.raises(candidates.CandidateArtifactError):
        bundle.read()


def test_bounded_gzip_expansion_includes_tar_padding(bundle, monkeypatch):
    binary = bundle.records[0]["binary"]
    bundle.replace_native(native_bytes(binary, bundle.binary_payloads[binary], padding=20_000))
    monkeypatch.setattr(candidates, "MAX_TAR_OVERHEAD", 1024)
    monkeypatch.setattr(candidates, "MAX_BINARY_BYTES", 100)
    with pytest.raises(candidates.CandidateArtifactError, match="expands"):
        bundle.read()


def test_native_payload_above_512_mib_is_streamed_within_its_bound(bundle):
    # Above the former 512 MiB bound; the payload streams through gzip, tar and
    # SHA-256 in bounded chunks, so the bound is a sanity check, not memory.
    record, size = bundle.records[0], 520 * 1024 * 1024

    class Zeros(io.RawIOBase):
        left = size

        def readable(self):
            return True

        def readinto(self, buffer):
            count = min(len(buffer), self.left)
            buffer[:count] = bytes(count)
            self.left -= count
            return count

    path = bundle.native_dir / record["name"]
    with gzip.GzipFile(path, "wb", compresslevel=1, mtime=0) as compressed:
        with tarfile.open(fileobj=compressed, mode="w|") as archive:
            member = tarfile.TarInfo(record["binary"])
            member.size, member.mode = size, 0o755
            archive.addfile(member, Zeros())
    payload, chunk = hashlib.sha256(), bytes(1024 * 1024)
    for _ in range(size // len(chunk)):
        payload.update(chunk)
    record.update({"size": path.stat().st_size, "sha256": digest(path.read_bytes()),
        "inner": {"name": record["binary"], "size": size, "sha256": payload.hexdigest()}})
    bundle.seal()
    [native] = [item for item in bundle.read().native if item.binary == record["binary"]]
    assert (native.inner_size, native.inner_sha256) == (size, payload.hexdigest())
    record["inner"]["size"] = candidates.MAX_BINARY_BYTES + 1
    bundle.seal()
    with pytest.raises(candidates.CandidateArtifactError, match="identity or size is invalid"):
        bundle.read()


def test_replaced_proof_during_read_is_rejected_at_finish(bundle, monkeypatch):
    original = candidates._wheel
    def mutate_after_read(*args):
        result = original(*args)
        (bundle.root / "intake.json").write_bytes(b"changed after initial check")
        return result
    monkeypatch.setattr(candidates, "_wheel", mutate_after_read)
    with pytest.raises(candidates.CandidateArtifactError, match="changed"):
        bundle.read()


def test_duplicate_json_keys_and_wrong_owner_fail_closed(bundle, monkeypatch):
    content = bundle.tuple_path.read_bytes().replace(b'"format":', b'"format": "other", "format":', 1)
    bundle.tuple_path.write_bytes(content)
    binding = replace(bundle.binding, tuple_sha256=digest(content))
    with pytest.raises(candidates.CandidateArtifactError, match="duplicate"):
        bundle.read(accepted=binding)
    actual_uid = os.getuid()
    monkeypatch.setattr(candidates.os, "getuid", lambda: actual_uid + 1)
    with pytest.raises(candidates.CandidateArtifactError, match="owned"):
        bundle.read()
