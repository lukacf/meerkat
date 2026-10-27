"""Real recovery observation readers with inert source specimens.

Small cases exercise the actual initial tree reader and completion reader.
The enclosing case reuses the real original Install/restart/action fixture.
Filesystem currentness is not independent Build or native acceptance, and no
speedup is claimed from these controls. External SDK/worker boundaries remain
explicitly inert in the original recovery fixture.
"""
import os
import pytest

from launchkit import local_application_recovery as recovery
from launchkit.connect_application_update_startup import _file_identity
from launchkit.connect_installer_caller import CustodyError
from test_local_application_recovery import (no_processes, no_product_execution,
    product, registry, restarted_installation, stop)


def _put(path, content=b"first-source"):
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    path.write_bytes(content)
    path.chmod(0o600)
    return path


@pytest.fixture
def observation(tmp_path):
    directory = tmp_path / "original-build"
    directory.mkdir(mode=0o700)
    (directory / "workspace").mkdir(mode=0o700)
    (directory / "acceptance").mkdir(mode=0o700)
    _put(directory / "input.json", b"{}")
    _put(directory / "receipt.json", b"{}")
    _put(directory / "workspace/nested/source.txt")
    _put(directory / "acceptance/round/source.txt", b"accepted-source")
    observed = recovery._build_observation(directory)
    assert {path for path, _ in observed} == {
        directory / "input.json", directory / "receipt.json",
        directory / "workspace", directory / "workspace/nested",
        directory / "workspace/nested/source.txt", directory / "acceptance",
        directory / "acceptance/round", directory / "acceptance/round/source.txt"}
    return directory, observed


def test_original_observation_rechecks_without_a_second_tree_walk(observation, monkeypatch):
    directory, observed = observation
    before = {path: path.read_bytes() for path, _ in observed if path.is_file()}

    def no_second_walk(*args, **kwargs):
        pytest.fail("The completion reader cannot walk the admitted Build trees again.")

    monkeypatch.setattr(recovery, "_build_observation", no_second_walk)
    monkeypatch.setattr(recovery.os, "walk", no_second_walk)
    assert recovery._require_build_observation(directory, observed) is None
    assert tuple((path, _file_identity(path)) for path, _ in observed) == observed
    assert {path: path.read_bytes() for path in before} == before


def test_original_rows_cannot_authorize_a_foreign_build(observation, tmp_path):
    directory, observed = observation
    foreign = tmp_path / "foreign-build"
    foreign.mkdir(mode=0o700)
    # All retained original rows are unchanged; only the selected Build is foreign.
    assert tuple((path, _file_identity(path)) for path, _ in observed) == observed
    with pytest.raises(CustodyError, match="original Build/source custody changed"):
        recovery._require_build_observation(foreign, observed)
    assert tuple((path, _file_identity(path)) for path, _ in observed) == observed


def test_private_build_parent_is_checked_outside_retained_rows(observation):
    directory, observed = observation
    assert directory not in {path for path, _ in observed}
    directory.chmod(0o755)
    # The initial input/receipt _pin checks this private enclosing directory.
    # Its mode is not one of the retained rows, so the explicit guard is needed.
    assert tuple((path, _file_identity(path)) for path, _ in observed) == observed
    with pytest.raises(CustodyError, match="private and owned"):
        recovery._require_build_observation(directory, observed)


@pytest.mark.parametrize("change", [
    "added-file", "added-directory", "removed-file", "replaced-file",
    "removed-directory", "replaced-directory", "restored-mtime-bytes",
    "file-mode", "directory-mode", "file-link", "directory-link", "file-hardlink",
    "removed-input", "removed-receipt",
])
def test_retained_observation_refuses_tree_or_identity_mutation(observation, tmp_path, change):
    directory, observed = observation
    folder = directory / "workspace/nested"
    path = folder / "source.txt"
    if change == "added-file":
        _put(folder / "unselected.txt", b"not part of the original tree")
    elif change == "added-directory":
        (directory / "acceptance/unselected").mkdir(mode=0o700)
    elif change == "removed-file":
        path.unlink()
    elif change == "replaced-file":
        replacement = _put(tmp_path / "replacement.txt", path.read_bytes())
        replacement.replace(path)
    elif change == "removed-directory":
        path.unlink()
        folder.rmdir()
    elif change == "replaced-directory":
        content = path.read_bytes()
        folder.rename(tmp_path / "original-nested")
        _put(path, content)
    elif change == "restored-mtime-bytes":
        before = path.stat()
        content = path.read_bytes()
        path.write_bytes(b"X" + content[1:])
        os.utime(path, ns=(before.st_atime_ns, before.st_mtime_ns))
        assert path.stat().st_size == before.st_size
        assert path.stat().st_mtime_ns == before.st_mtime_ns
        assert path.stat().st_ctime_ns != before.st_ctime_ns
    elif change == "file-mode":
        path.chmod(0o644)
    elif change == "directory-mode":
        folder.chmod(0o755)
    elif change == "file-link":
        content = path.read_bytes()
        path.unlink()
        path.symlink_to(_put(tmp_path / "linked.txt", content))
    elif change == "directory-link":
        target = tmp_path / "linked-nested"
        folder.rename(target)
        folder.symlink_to(target, target_is_directory=True)
    elif change == "file-hardlink":
        content = path.read_bytes()
        path.unlink()
        target = _put(tmp_path / "hardlinked.txt", content)
        os.link(target, path)
    elif change in {"removed-input", "removed-receipt"}:
        (directory / ("input.json" if change == "removed-input" else "receipt.json")).unlink()
    else:
        pytest.fail("Unknown explicit filesystem mutation.")
    with pytest.raises(CustodyError):
        recovery._require_build_observation(directory, observed)


async def test_real_recovered_admission_uses_original_observation_reader(
        restarted_installation, monkeypatch):
    f = restarted_installation
    await stop(f)
    chosen = f.registry._builds[(f.guide_id, f.installed["build_id"])]
    admission = chosen.factory._installed_admission
    assert type(admission) is recovery._InstalledLocalAdmission
    assert admission.registry is f.registry and admission.builds
    assert chosen.host is f.registry.selected_host(f.guide_id, f.directory.name)
    original_reader = recovery._require_build_observation
    calls = []

    def observe_original_reader(directory, observed):
        calls.append((directory, observed))
        return original_reader(directory, observed)

    monkeypatch.setattr(recovery, "_require_build_observation", observe_original_reader)
    admission.require_current()
    expected = [(f.implementation._directory(f.guide_id, row[0]), row[4])
        for row in admission.builds]
    assert calls == expected
    before = tuple(admission.builds)
    _put(f.original_build / "workspace/unselected-after-admission.txt")
    calls.clear()
    with pytest.raises(CustodyError, match="original Build/source custody changed"):
        admission.require_current()
    assert calls == expected[:1]
    assert tuple(admission.builds) == before
