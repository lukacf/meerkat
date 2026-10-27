"""Actual byte-copy source checks with no native execution or credentials."""
import json
import os
from pathlib import Path

import pytest

from launchkit import deployment
from launchkit.projects import DraftConflict


@pytest.fixture
def source(tmp_path):
    root = tmp_path / "application"
    root.mkdir(mode=0o700)
    (root / ".mobkit").mkdir(mode=0o700)
    files = {"app.py": (b"x" * 1024 * 1024, 0o644), "empty.py": (b"", 0o644)}
    for name, (content, mode) in files.items():
        (root / name).write_bytes(content)
        (root / name).chmod(mode)
    receipt = {"destination": str(root), "deployment_id": "inert-deployment",
        "guide_id": "inert-guide", "build_id": "inert-build", "package_sha256": "a" * 64}
    marker = root / deployment.OWNER_FILE
    marker.write_text(json.dumps(deployment.ownership(receipt)))
    marker.chmod(0o600)
    return root, files, receipt


def test_repeated_checks_do_not_reread_the_unchanged_source(source, monkeypatch):
    root, files, receipt = source
    reads = []
    original = Path.read_bytes
    def read(path):
        reads.append(path)
        return original(path)
    monkeypatch.setattr(Path, "read_bytes", read)
    observed = deployment._DestinationSourceObservation(receipt, files)
    assert reads == [root / name for name in files]
    reads.clear()
    for _ in range(200):
        observed.require_current(receipt, files)
    assert reads == []
    deployment.verify_destination(receipt, files)
    assert reads == [root / name for name in files]


@pytest.mark.parametrize("change", ["bytes", "replace", "mode", "link", "hardlink", "missing", "owner"])
def test_changed_source_refuses_the_original_operation(source, change):
    root, files, receipt = source
    observed = deployment._DestinationSourceObservation(receipt, files)
    path = root / "app.py"
    if change == "bytes":
        stamp = path.stat()
        path.write_bytes(b"y" * len(files["app.py"][0]))
        os.utime(path, ns=(stamp.st_atime_ns, stamp.st_mtime_ns))
    elif change == "replace":
        path.unlink()
        path.write_bytes(files["app.py"][0])
    elif change == "mode":
        path.chmod(0o600)
    elif change == "link":
        path.unlink()
        path.symlink_to(root / "empty.py")
    elif change == "hardlink":
        os.link(path, root / "second-name")
    elif change == "missing":
        path.unlink()
    else:
        (root / deployment.OWNER_FILE).write_text("{}")
    with pytest.raises((DraftConflict, ValueError)):
        observed.require_current(receipt, files)


@pytest.mark.parametrize("change", ["bytes", "mode", "extra", "missing", "receipt"])
def test_current_inputs_cannot_replace_the_original_selection(source, change):
    root, files, receipt = source
    observed = deployment._DestinationSourceObservation(receipt, files)
    selected = dict(files)
    if change == "bytes":
        selected["app.py"] = (bytes(bytearray(files["app.py"][0])), 0o644)
    elif change == "mode":
        selected["app.py"] = (files["app.py"][0], 0o600)
    elif change == "extra":
        selected["extra.py"] = (b"extra", 0o644)
    elif change == "missing":
        del selected["empty.py"]
    else:
        receipt = {**receipt, "build_id": "another-build"}
    with pytest.raises(DraftConflict):
        observed.require_current(receipt, selected)


def test_changes_during_initial_full_validation_are_refused(source, monkeypatch):
    root, files, receipt = source
    original = deployment.verify_destination
    def verify(*args, **kwargs):
        result = original(*args, **kwargs)
        (root / "app.py").write_bytes(b"changed after the byte check")
        return result
    monkeypatch.setattr(deployment, "verify_destination", verify)
    with pytest.raises(DraftConflict):
        deployment._DestinationSourceObservation(receipt, files)


def test_runtime_private_directories_do_not_invalidate_source(source):
    root, files, receipt = source
    observed = deployment._DestinationSourceObservation(receipt, files)
    (root / ".venv").mkdir(mode=0o700)
    (root / ".venv" / "inert-file").write_bytes(b"not application source")
    observed.require_current(receipt, files)


def test_a_fresh_observation_requires_full_original_bytes(source):
    root, files, receipt = source
    deployment._DestinationSourceObservation(receipt, files)
    (root / "app.py").write_bytes(b"wrong bytes after process restart")
    with pytest.raises(DraftConflict):
        deployment._DestinationSourceObservation(receipt, files)


@pytest.mark.parametrize("entry", [
    (bytearray(b"x"), 0o644), [b"x", 0o644], (b"x",),
    (b"x", True), (b"x", "644"), (b"x", -1), (b"x", 0o10000),
])
def test_observation_refuses_mutable_or_malformed_source_before_read(source, monkeypatch, entry):
    root, files, receipt = source
    def forbidden(*args, **kwargs):
        pytest.fail("Invalid source reached a disk byte read.")
    monkeypatch.setattr(deployment, "verify_destination", forbidden)
    with pytest.raises(DraftConflict, match="immutable checked bytes"):
        deployment._DestinationSourceObservation(receipt, {**files, "app.py": entry})
