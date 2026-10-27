"""Inert first-install publication faults preserve retry and destination custody."""
import hashlib
import io
import json
import os
from types import SimpleNamespace
import stat
import zipfile

import pytest

from launchkit import deployment
from launchkit.projects import DraftConflict


@pytest.fixture
def pending(tmp_path):
    root = tmp_path / "journal"
    destination = tmp_path / "team"
    root.mkdir(mode=0o700)
    destination.mkdir(mode=0o700)
    owner = destination / deployment.OWNER_FILE
    owner.parent.mkdir(mode=0o700)
    payload = io.BytesIO()
    with zipfile.ZipFile(payload, "w") as archive:
        for name, content in {"app.py": b"complete checked source\n", "deployment.json": b"{}",
                "install.sh": b"exit 0\n", "implementation-report.json": b"{}",
                "application/nested.py": b"nested checked source\n"}.items():
            item = zipfile.ZipInfo(name)
            item.external_attr = (stat.S_IFREG | (0o755 if name == "install.sh" else 0o644)) << 16
            archive.writestr(item, content)
    receipt = {"deployment_id": "owned-install", "guide_id": "owned-guide", "build_id": "owned-build",
        "destination": str(destination), "package_sha256": hashlib.sha256(payload.getvalue()).hexdigest()}
    (root / "receipt.json").write_text(json.dumps(receipt))
    (root / "package.zip").write_bytes(payload.getvalue())
    owner.write_text(json.dumps(deployment.ownership(receipt)))
    owner.chmod(0o600)
    settings = destination / ".env"
    settings.write_bytes(b"PRIVATE_FIXTURE=preserved\n")
    settings.chmod(0o600)
    history = owner.parent / "history.json"
    history.write_bytes(b"retained private history\n")
    return root, destination, receipt, {path: path.read_bytes() for path in (owner, settings, history)}


def preserved_private(pending):
    _, destination, _, private = pending
    assert all(path.read_bytes() == content for path, content in private.items())
    assert stat.S_IMODE((destination / ".env").stat().st_mode) == 0o600


class FaultStream:
    def __init__(self, stream, fault):
        self.stream, self.fault = stream, fault

    def __enter__(self):
        self.stream.__enter__()
        return self

    def __exit__(self, *args):
        return self.stream.__exit__(*args)

    def __getattr__(self, name):
        return getattr(self.stream, name)

    def write(self, content):
        if self.fault == "write":
            self.stream.write(content[:3])
            raise OSError("inert write failure")
        if self.fault == "short":
            return self.stream.write(content[:3])
        if self.fault == "corrupt":
            self.stream.write(b"x" * len(content))
            return len(content)
        return self.stream.write(content)

    def flush(self):
        if self.fault == "flush":
            raise OSError("inert flush failure")
        return self.stream.flush()


@pytest.mark.parametrize("fault", ["write", "short", "corrupt", "flush", "fsync"])
def test_incomplete_write_never_publishes_and_exact_retry_succeeds(pending, monkeypatch, fault):
    root, destination, receipt, _ = pending
    fdopen = deployment.os.fdopen
    with monkeypatch.context() as patch:
        if fault == "fsync":
            patch.setattr(deployment.os, "fsync", lambda fd: (_ for _ in ()).throw(OSError("inert fsync failure")))
        else:
            patch.setattr(deployment.os, "fdopen", lambda fd, mode: FaultStream(fdopen(fd, mode), fault))
        with pytest.raises(OSError):
            deployment.materialize(root)
    assert not (destination / "app.py").exists()
    assert not list(destination.glob(".forge-source-*"))
    preserved_private(pending)
    files = deployment.materialize(root)
    deployment.verify_destination(receipt, files)
    assert (destination / "app.py").read_bytes() == b"complete checked source\n"
    preserved_private(pending)


def test_successful_publication_has_complete_fsynced_bytes_and_legacy_umask_mode(pending, monkeypatch):
    root, destination, _, _ = pending
    fsync, publish = deployment.os.fsync, deployment._rename_new_source
    flushed = set()
    published = []

    def record_fsync(fd):
        info = os.fstat(fd)
        flushed.add((info.st_dev, info.st_ino))
        return fsync(fd)

    def inspect_publish(stage, parent, target):
        source = "source"
        assert stat.S_IMODE(os.fstat(stage).st_mode) == 0o700
        info = os.stat(source, dir_fd=stage)
        assert (info.st_dev, info.st_ino) in flushed
        if target == "app.py":
            assert not (destination / target).exists()
        published.append(target)
        return publish(stage, parent, target)

    monkeypatch.setattr(deployment.os, "fsync", record_fsync)
    monkeypatch.setattr(deployment, "_rename_new_source", inspect_publish)
    mask = os.umask(0o077)
    try:
        files = deployment.materialize(root)
    finally:
        os.umask(mask)
    assert len(published) == len(files)
    assert stat.S_IMODE((destination / "app.py").stat().st_mode) == 0o600
    assert stat.S_IMODE((destination / "install.sh").stat().st_mode) == 0o700
    assert all((destination / name).read_bytes() == content for name, (content, _) in files.items())
    assert not list(destination.rglob(".forge-source-*"))
    preserved_private(pending)


def test_later_file_failure_retains_already_published_source_for_retry(pending, monkeypatch):
    root, destination, _, _ = pending
    fsync = deployment.os.fsync
    calls = 0

    def fail_second(fd):
        nonlocal calls
        calls += 1
        if calls == 2:
            raise OSError("inert later file failure")
        fsync(fd)

    with monkeypatch.context() as patch:
        patch.setattr(deployment.os, "fsync", fail_second)
        with pytest.raises(OSError, match="later file"):
            deployment.materialize(root)
    first = destination / "app.py"
    identity = first.stat().st_ino
    assert first.read_bytes() == b"complete checked source\n"
    assert not (destination / "deployment.json").exists()
    deployment.materialize(root)
    assert first.stat().st_ino == identity
    preserved_private(pending)


@pytest.mark.parametrize("collision", ["different", "matching", "link"])
def test_publication_collision_does_not_replace_newly_existing_entry(pending, monkeypatch, collision):
    root, destination, _, _ = pending
    publish = deployment._rename_new_source
    unrelated = destination / "unrelated.txt"
    unrelated.write_bytes(b"unrelated original")

    def collide(stage, parent, target):
        final = destination / target
        if collision == "link":
            final.symlink_to(unrelated)
        else:
            final.write_bytes(b"complete checked source\n" if collision == "matching" else b"other writer")
        return publish(stage, parent, target)

    with monkeypatch.context() as patch:
        patch.setattr(deployment, "_rename_new_source", collide)
        with pytest.raises(FileExistsError):
            deployment.materialize(root)
    if collision == "link":
        assert (destination / "app.py").is_symlink()
    else:
        assert (destination / "app.py").read_bytes() == (b"complete checked source\n" if collision == "matching" else b"other writer")
    assert unrelated.read_bytes() == b"unrelated original"
    assert not list(destination.glob(".forge-source-*"))
    preserved_private(pending)
    if collision == "matching":
        deployment.materialize(root)
    else:
        with pytest.raises(DraftConflict):
            deployment.materialize(root)


@pytest.mark.parametrize("tamper", ["source", "owner", "package"])
def test_existing_tamper_refuses_before_staging(pending, monkeypatch, tamper):
    root, destination, _, _ = pending
    if tamper == "source":
        (destination / "app.py").write_bytes(b"existing modified source")
    elif tamper == "owner":
        (destination / deployment.OWNER_FILE).write_text("{}")
    else:
        (root / "package.zip").write_bytes(b"modified package")
    monkeypatch.setattr(deployment, "_rename_new_source", lambda *a, **k: pytest.fail("tamper must not publish"))
    with pytest.raises(DraftConflict):
        deployment.materialize(root)
    assert not list(destination.glob(".forge-source-*"))
    assert not (destination / "deployment.json").exists()
    if tamper != "owner":
        preserved_private(pending)
    assert (destination / ".env").read_bytes() == b"PRIVATE_FIXTURE=preserved\n"


def test_retry_ignores_other_attempt_private_residue(pending):
    root, destination, _, _ = pending
    residue = destination / ".forge-source-interrupted-attempt"
    residue.mkdir(mode=0o700)
    (residue / "source").write_bytes(b"incomplete other attempt")
    deployment.materialize(root)
    assert (residue / "source").read_bytes() == b"incomplete other attempt"
    assert stat.S_IMODE(residue.stat().st_mode) == 0o700
    preserved_private(pending)


def test_cleanup_failure_after_publish_leaves_complete_source_retryable(pending, monkeypatch):
    root, destination, _, _ = pending
    with monkeypatch.context() as patch:
        patch.setattr(deployment.os, "rmdir", lambda *a, **k: (_ for _ in ()).throw(OSError("inert cleanup failure")))
        deployment.materialize(root)
    residue = list(destination.glob(".forge-source-*"))
    assert residue and all(stat.S_IMODE(path.stat().st_mode) == 0o700 for path in residue)
    assert (destination / "app.py").read_bytes() == b"complete checked source\n"
    assert (destination / "app.py").stat().st_nlink == 1
    assert all(not (path / "source").exists() for path in residue)
    deployment.materialize(root)
    assert all(path.exists() for path in residue)
    preserved_private(pending)


def test_cleanup_does_not_delete_replaced_staging_file(pending, monkeypatch):
    root, destination, _, _ = pending
    publish = deployment._rename_new_source
    residue = []

    def replace_after_publish(stage, parent, target):
        result = publish(stage, parent, target)
        fd = os.open("source", os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600, dir_fd=stage)
        with os.fdopen(fd, "wb") as stream:
            stream.write(b"another writer's file")
        residue.append(target)
        return result

    monkeypatch.setattr(deployment, "_rename_new_source", replace_after_publish)
    deployment.materialize(root)
    stages = list(destination.rglob(".forge-source-*"))
    assert len(stages) == len(residue)
    assert all((stage / "source").read_bytes() == b"another writer's file" for stage in stages)
    preserved_private(pending)


def test_changed_ownership_before_publish_preserves_new_owner_and_no_source(pending, monkeypatch):
    root, destination, _, _ = pending
    fsync = deployment.os.fsync
    owner = destination / deployment.OWNER_FILE

    def change_owner(fd):
        fsync(fd)
        owner.write_text("{}")

    monkeypatch.setattr(deployment.os, "fsync", change_owner)
    with pytest.raises(DraftConflict, match="another deployment"):
        deployment.materialize(root)
    assert owner.read_text() == "{}"
    assert not (destination / "app.py").exists()
    assert not list(destination.glob(".forge-source-*"))
    assert (destination / ".env").read_bytes() == b"PRIVATE_FIXTURE=preserved\n"


@pytest.mark.parametrize("platform,function,flags", [("linux", "renameat2", 1), ("darwin", "renameatx_np", 0x4)])
def test_platform_exclusive_rename_receives_original_directory_fds(monkeypatch, platform, function, flags):
    calls = []

    def rename(*args):
        calls.append(args)
        return 0

    monkeypatch.setattr(deployment.sys, "platform", platform)
    monkeypatch.setattr(deployment.ctypes, "CDLL", lambda *a, **k: SimpleNamespace(**{function: rename}))
    deployment._rename_new_source(31, 32, "checked.py")
    assert calls == [(31, b"source", 32, b"checked.py", flags)]
    assert rename.restype is deployment.ctypes.c_int
    assert rename.argtypes == (deployment.ctypes.c_int, deployment.ctypes.c_char_p,
        deployment.ctypes.c_int, deployment.ctypes.c_char_p, deployment.ctypes.c_uint)


@pytest.mark.parametrize("platform", ["linux", "darwin", "unsupported"])
def test_missing_atomic_exclusive_primitive_fails_without_publishing(pending, monkeypatch, platform):
    root, destination, _, _ = pending
    with monkeypatch.context() as patch:
        patch.setattr(deployment.sys, "platform", platform)
        patch.setattr(deployment.ctypes, "CDLL", lambda *a, **k: SimpleNamespace())
        with pytest.raises(OSError, match="atomically"):
            deployment.materialize(root)
    assert not (destination / "app.py").exists()
    assert not list(destination.glob(".forge-source-*"))
    deployment.materialize(root)
    preserved_private(pending)


def test_atomic_rename_error_never_falls_back_to_replacing_rename(pending, monkeypatch):
    root, destination, _, _ = pending

    def fail(*args):
        deployment.ctypes.set_errno(95)
        return -1

    with monkeypatch.context() as patch:
        patch.setattr(deployment.sys, "platform", "linux")
        patch.setattr(deployment.ctypes, "CDLL", lambda *a, **k: SimpleNamespace(renameat2=fail))
        patch.setattr(deployment.os, "replace", lambda *a, **k: pytest.fail("no replacing fallback"))
        with pytest.raises(OSError) as error:
            deployment.materialize(root)
        assert error.value.errno == 95
    assert not (destination / "app.py").exists()
    deployment.materialize(root)
    preserved_private(pending)
