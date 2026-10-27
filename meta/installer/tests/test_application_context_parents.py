"""Real confined directory effects, with no SDK, grants or native execution."""
import os
from pathlib import Path

import pytest

from launchkit.connect_application_startup import _prepare_context_parents
from launchkit.connect_installer_caller import CustodyError
from launchkit.templates.storage_setup import project_lock


@pytest.fixture
def root(tmp_path):
    tmp_path.chmod(0o700)
    return tmp_path


def prepare(root, *, initialize=True, check=None):
    with project_lock(root) as lease:
        _prepare_context_parents(root, lease, check or lease.require_current, initialize=initialize)


def test_initialize_prepares_only_private_context_parents(root):
    prepare(root)
    assert sorted(str(p.relative_to(root / "shared")) for p in (root / "shared").rglob("*")) == ["control", "data"]
    assert all((root / name).stat().st_mode & 0o777 == 0o700
        for name in ("shared", "shared/data", "shared/control"))
    assert not (root / "shared/data/toolkit-native").exists()
    assert not (root / "shared/control/deny.lock").exists()


def test_restore_requires_existing_parents_without_creating_them(root):
    with pytest.raises(CustodyError):
        prepare(root, initialize=False)
    assert not (root / "shared").exists()


def test_restore_preserves_existing_parents_and_native_outputs(root):
    prepare(root)
    output = root / "shared/control/deny.json"
    output.write_bytes(b"inert existing output")
    output.chmod(0o644)
    paths = [root / name for name in ("shared", "shared/data", "shared/control")]
    before = [(p.stat().st_ino, p.stat().st_mtime_ns, p.stat().st_mode) for p in paths]
    prepare(root, initialize=False)
    assert output.read_bytes() == b"inert existing output"
    assert before == [(p.stat().st_ino, p.stat().st_mtime_ns, p.stat().st_mode) for p in paths]


@pytest.mark.parametrize("name", ["shared", "shared/data", "shared/control"])
@pytest.mark.parametrize("change", ["link", "file", "unsafe", "readonly"])
def test_unsafe_existing_parent_is_refused_without_repair(root, name, change):
    prepare(root)
    path = root / name
    outside = root.parent / (root.name + "-outside")
    outside.mkdir(mode=0o700)
    sentinel = outside / "untouched"
    sentinel.write_bytes(b"outside")
    if change in {"link", "file"}:
        if name == "shared":
            (path / "data").rmdir()
            (path / "control").rmdir()
        path.rmdir()
        if change == "link":
            path.symlink_to(outside, target_is_directory=True)
        else:
            path.write_bytes(b"inert conflicting file")
    else:
        path.chmod(0o720 if change == "unsafe" else 0o500)
    before = path.lstat()
    with pytest.raises(CustodyError):
        prepare(root)
    after = path.lstat()
    assert (before.st_ino, before.st_mode, before.st_size) == (after.st_ino, after.st_mode, after.st_size)
    assert sentinel.read_bytes() == b"outside" and sorted(p.name for p in outside.iterdir()) == ["untouched"]


def test_lost_original_owner_precedes_any_context_effect(root):
    with pytest.raises(CustodyError):
        prepare(root, check=lambda: False)
    assert not (root / "shared").exists()


def test_async_owner_fence_is_refused_without_effect(root):
    async def invalid():
        return None
    with pytest.raises(CustodyError, match="synchronous"):
        prepare(root, check=invalid)
    assert not (root / "shared").exists()


def test_owner_change_during_preparation_stops_before_native_state(root):
    calls = []
    def current():
        calls.append(True)
        return len(calls) < 5
    with pytest.raises(CustodyError):
        prepare(root, check=current)
    assert not (root / "shared/data/toolkit-native").exists()
    assert not (root / "shared/control/deny.lock").exists()


def test_closed_original_lease_is_refused(root):
    with project_lock(root) as lease:
        pass
    with pytest.raises(RuntimeError):
        _prepare_context_parents(root, lease, lease.require_current, initialize=True)
    assert not (root / "shared").exists()
