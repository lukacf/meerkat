"""The generated project lease preserves one real flock and descriptor custody."""
import importlib.util
import os
from pathlib import Path
import subprocess
import sys

import pytest


@pytest.fixture
def storage():
    path = Path(__file__).parents[1] / "launchkit/templates/storage_setup.py"
    spec = importlib.util.spec_from_file_location("project_lock_storage_setup", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def competing_process(lock):
    return subprocess.run([sys.executable, "-c", """
import fcntl, os, sys
descriptor = os.open(sys.argv[1], os.O_RDWR | os.O_NOFOLLOW)
try:
    fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
except BlockingIOError:
    print('blocked')
    sys.exit(23)
print('acquired')
""", str(lock)], text=True, capture_output=True, timeout=10, check=False)


def test_current_lease_keeps_real_flock_and_expires_at_context_exit(tmp_path, storage):
    with storage.project_lock(tmp_path) as lease:
        assert isinstance(lease, storage.ProjectLockLease)
        assert lease.root == tmp_path
        lock = tmp_path / ".mobkit/launch.lock"
        assert lock.stat().st_mode & 0o777 == 0o600
        assert lock.parent.stat().st_mode & 0o777 == 0o700
        lease.require_current()
        blocked = competing_process(lock)
        assert (blocked.returncode, blocked.stdout.strip()) == (23, "blocked")
        (tmp_path / "ordinary-workspace-data").write_text("ordinary edits keep the lease current")
        lease.require_current()
    with pytest.raises(RuntimeError, match="no longer current"):
        lease.require_current()
    released = competing_process(lock)
    assert (released.returncode, released.stdout.strip()) == (0, "acquired")
    with storage.project_lock(tmp_path):
        pass


def test_replaced_lock_refuses_old_lease_without_substituting_the_new_flock(tmp_path, storage):
    with storage.project_lock(tmp_path) as lease:
        lock = tmp_path / ".mobkit/launch.lock"
        original = lock.with_name("retained-original.lock")
        lock.rename(original)
        lock.touch(mode=0o600)
        assert competing_process(original).returncode == 23
        assert competing_process(lock).returncode == 0
        with pytest.raises(RuntimeError, match="no longer current"):
            lease.require_current()
        lock.unlink()
        original.rename(lock)
        with pytest.raises(RuntimeError, match="no longer current"):
            lease.require_current()
        assert competing_process(lock).returncode == 23


@pytest.mark.parametrize("target", ["root", "directory", "lock"])
def test_symlink_replacement_during_lease_is_refused_and_retained_state_is_preserved(
        tmp_path, storage, target):
    root = tmp_path / "project"
    root.mkdir()
    with storage.project_lock(root) as lease:
        state = root / ".mobkit/state"
        state.mkdir(mode=0o700)
        data = state / "continuity.sqlite3"
        data.write_bytes(b"retained private native history")
        selected = {"root": root, "directory": root / ".mobkit",
                    "lock": root / ".mobkit/launch.lock"}[target]
        retained = selected.with_name(selected.name + "-retained")
        selected.rename(retained)
        selected.symlink_to(retained, target_is_directory=target != "lock")
        with pytest.raises(RuntimeError, match="no longer current"):
            lease.require_current()
        assert data.read_bytes() == b"retained private native history"


@pytest.mark.parametrize("target", ["root", "directory"])
def test_directory_replacement_during_lease_is_refused(tmp_path, storage, target):
    root = tmp_path / "project"
    root.mkdir()
    with storage.project_lock(root) as lease:
        selected = root if target == "root" else root / ".mobkit"
        selected.rename(selected.with_name(selected.name + "-retained"))
        selected.mkdir(mode=0o700)
        with pytest.raises(RuntimeError, match="no longer current"):
            lease.require_current()


@pytest.mark.parametrize("target", ["root", "directory", "lock"])
def test_lost_private_permissions_refuse_lease_without_repairing_owner_data(tmp_path, storage, target):
    with storage.project_lock(tmp_path) as lease:
        selected = {"root": tmp_path, "directory": tmp_path / ".mobkit",
                    "lock": tmp_path / ".mobkit/launch.lock"}[target]
        selected.chmod({"root": 0o777, "directory": 0o755, "lock": 0o644}[target])
        with pytest.raises(RuntimeError, match="no longer current"):
            lease.require_current()
        assert selected.stat().st_mode & 0o077


def test_hardlinked_lock_refuses_currentness(tmp_path, storage):
    with storage.project_lock(tmp_path) as lease:
        os.link(tmp_path / ".mobkit/launch.lock", tmp_path / "lock-alias")
        with pytest.raises(RuntimeError, match="no longer current"):
            lease.require_current()


@pytest.mark.parametrize("attribute", ["_descriptor", "_guard_descriptor", "_directory_descriptor", "_root_descriptor"])
def test_closed_held_descriptor_refuses_currentness_and_releases_remaining_descriptors(
        tmp_path, storage, attribute):
    with storage.project_lock(tmp_path) as lease:
        held = [lease._descriptor, lease._guard_descriptor,
                lease._directory_descriptor, lease._root_descriptor]
        os.close(getattr(lease, attribute))
        with pytest.raises(RuntimeError, match="no longer current"):
            lease.require_current()
    for descriptor in held:
        with pytest.raises(OSError):
            os.fstat(descriptor)
    with storage.project_lock(tmp_path):
        pass


def test_descriptor_reuse_for_another_inode_refuses_currentness(tmp_path, storage):
    with storage.project_lock(tmp_path) as lease:
        descriptor = lease._descriptor
        os.close(descriptor)
        replacement = tmp_path / "another-file"
        other = os.open(replacement, os.O_RDWR | os.O_CREAT, 0o600)
        try:
            if other != descriptor:
                os.dup2(other, descriptor)
            with pytest.raises(RuntimeError, match="no longer current"):
                lease.require_current()
        finally:
            if other != descriptor:
                os.close(other)
    assert os.fstat(descriptor).st_ino == replacement.stat().st_ino
    os.close(descriptor)
    assert replacement.exists()


def test_same_inode_descriptor_reuse_retains_the_original_flock_owner(tmp_path, storage):
    with storage.project_lock(tmp_path) as lease:
        lock = tmp_path / ".mobkit/launch.lock"
        original = lease._descriptor
        os.close(original)
        reopened = os.open(lock, os.O_RDWR | os.O_NOFOLLOW)
        try:
            if reopened != original:
                os.dup2(reopened, original)
            lease.require_current()
            assert competing_process(lock).returncode == 23
        finally:
            if reopened != original:
                os.close(reopened)
    assert competing_process(lock).returncode == 0


def test_group_writable_project_root_is_refused_without_creating_lock_state(tmp_path, storage):
    tmp_path.chmod(0o770)
    with pytest.raises(RuntimeError, match="Unsafe project lock"):
        with storage.project_lock(tmp_path):
            pytest.fail("Group-writable project root was admitted")
    assert not (tmp_path / ".mobkit").exists()
    assert tmp_path.stat().st_mode & 0o777 == 0o770


@pytest.mark.parametrize("target", ["root", "ancestor", "directory", "lock"])
def test_preexisting_symlinks_are_refused_without_writing_to_their_targets(tmp_path, storage, target):
    root = tmp_path / "project"
    root.mkdir()
    retained = tmp_path / "retained"
    retained.mkdir(mode=0o700)
    data = retained / "owner-data"
    data.write_bytes(b"unchanged owner data")
    if target == "root":
        root.rmdir()
        root.symlink_to(retained, target_is_directory=True)
    elif target == "ancestor":
        alias = tmp_path / "alias"
        alias.symlink_to(tmp_path, target_is_directory=True)
        root = alias / "project"
    elif target == "directory":
        (root / ".mobkit").symlink_to(retained, target_is_directory=True)
    else:
        (root / ".mobkit").mkdir(mode=0o700)
        (root / ".mobkit/launch.lock").symlink_to(data)
    with pytest.raises(RuntimeError, match="Unsafe project lock"):
        with storage.project_lock(root):
            pytest.fail("Symlinked lock custody was admitted")
    assert data.read_bytes() == b"unchanged owner data"
    assert sorted(path.name for path in retained.iterdir()) == ["owner-data"]


@pytest.mark.parametrize("target", ["directory", "lock", "hardlink", "fifo"])
def test_unsafe_existing_lock_location_is_not_modified(tmp_path, storage, target):
    directory = tmp_path / ".mobkit"
    directory.mkdir(mode=0o700)
    directory.chmod(0o755 if target == "directory" else 0o700)
    lock = directory / "launch.lock"
    if target == "fifo":
        os.mkfifo(lock, 0o600)
    else:
        lock.write_bytes(b"preexisting lock bytes")
        lock.chmod(0o644 if target == "lock" else 0o600)
        if target == "hardlink":
            os.link(lock, tmp_path / "lock-alias")
    before = (directory.stat().st_mode, lock.stat().st_mode, lock.stat().st_ino)
    with pytest.raises(RuntimeError, match="Unsafe project lock"):
        with storage.project_lock(tmp_path):
            pytest.fail("Unsafe lock custody was admitted")
    assert before == (directory.stat().st_mode, lock.stat().st_mode, lock.stat().st_ino)
    if target != "fifo":
        assert lock.read_bytes() == b"preexisting lock bytes"


def test_child_cannot_use_inherited_parent_lease(tmp_path, storage):
    with storage.project_lock(tmp_path) as lease:
        child = os.fork()
        if child == 0:
            try:
                lease.require_current()
            except RuntimeError:
                os._exit(0)
            os._exit(1)
        _, status = os.waitpid(child, 0)
        assert os.waitstatus_to_exitcode(status) == 0
        lease.require_current()
