"""Explicit recovery of an original build selection, without worker admission."""
from __future__ import annotations

from contextlib import contextmanager
from dataclasses import dataclass
from datetime import datetime
import fcntl
import os
from pathlib import Path
import stat
import tempfile
import uuid

from .artifact_binding import frozen_matches
from .connect_accepted_source import _File, _directory_identity, _file_identity, _open_directory, _read
from .connect_installer_caller import CustodyError, MAX_INPUT_BYTES, MAX_RECEIPT_BYTES, _json
from .deployment import _publish_new_source
from .implementation_workspace import (
    ImplementationWorkspace, MAX_FILE_BYTES, MAX_PROJECT_BYTES, MAX_PROJECT_FILES, _safe_name,
)
from .projects import DraftConflict, fingerprint

MAX_SAVED_ENTRIES = 256
MAX_RECOVERY_BYTES = 2 * MAX_PROJECT_BYTES + MAX_INPUT_BYTES + MAX_RECEIPT_BYTES
ERROR = "The saved build changed or is unavailable. No selection or source was replaced."


def canonical_id(value):
    try:
        if str(uuid.UUID(value)) != value:
            raise ValueError
    except (ValueError, TypeError, AttributeError):
        raise DraftConflict(ERROR) from None
    return value


def _directory(path, *, private=False):
    descriptor = _open_directory(path)
    info = os.fstat(descriptor)
    if (info.st_uid != os.getuid() or stat.S_IMODE(info.st_mode) & (0o077 if private else 0o022)
            or info.st_mode & (stat.S_ISUID | stat.S_ISGID | stat.S_ISVTX)):
        os.close(descriptor)
        raise CustodyError(ERROR)
    return descriptor, _directory_identity(info)


def _require_directory(path, identity):
    descriptor, observed = _directory(path)
    try:
        if observed != identity:
            raise CustodyError(ERROR)
    finally:
        os.close(descriptor)


def require_absent(parent):
    try:
        os.stat("latest.json", dir_fd=parent, follow_symlinks=False)
    except FileNotFoundError:
        return
    raise DraftConflict("A build selection already exists or is unreadable. Reopen it before recovering another selection.")


@contextmanager
def _lock(directory, name, *, create=False, optional=False):
    flags = os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK
    if create:
        flags = os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW | os.O_NONBLOCK
    descriptor = None
    try:
        try:
            descriptor = os.open(name, flags, 0o600, dir_fd=directory)
        except FileNotFoundError:
            if not optional:
                raise
        if descriptor is not None:
            before = os.fstat(descriptor)
            if (not stat.S_ISREG(before.st_mode) or before.st_uid != os.getuid()
                    or before.st_nlink != 1 or stat.S_IMODE(before.st_mode) != 0o600
                    or before.st_size != 0):
                raise CustodyError(ERROR)
            # A held native/source owner stays held. This metadata operation
            # never waits indefinitely for, stops, or adopts that owner.
            fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
            named = os.stat(name, dir_fd=directory, follow_symlinks=False)
            if _file_identity(named) != _file_identity(before):
                raise CustodyError(ERROR)
        yield
    finally:
        if descriptor is not None:
            os.close(descriptor)


def _held(path, limit, files, *, private=False, budget=None):
    if budget is not None:
        limit = min(limit, budget[0])
    content, pin = _read(path, limit, private=private)
    if pin.identity[1] & (stat.S_ISUID | stat.S_ISGID | stat.S_ISVTX):
        raise CustodyError(ERROR)
    files.append(pin)
    if budget is not None:
        budget[0] -= len(content)
    return content


def _entries(descriptor, limit):
    result = []
    with os.scandir(descriptor) as entries:
        for entry in entries:
            if len(result) >= limit:
                raise CustodyError(ERROR)
            result.append(entry.name)
    return sorted(result)


def _project(path, files, directories, budget):
    result = {}
    size = 0
    members = 0

    def visit(folder, prefix=""):
        nonlocal size, members
        descriptor, identity = _directory(folder)
        directories.append((folder, identity))
        try:
            for name in _entries(descriptor, MAX_PROJECT_FILES * 2 - members):
                members += 1
                if members > MAX_PROJECT_FILES * 2:
                    raise CustodyError(ERROR)
                relative = _safe_name(prefix + name)
                info = os.stat(name, dir_fd=descriptor, follow_symlinks=False)
                if stat.S_ISDIR(info.st_mode):
                    visit(folder / name, relative + "/")
                else:
                    if len(result) >= MAX_PROJECT_FILES:
                        raise CustodyError(ERROR)
                    content = _held(folder / name, MAX_FILE_BYTES, files, budget=budget)
                    size += len(content)
                    if size > MAX_PROJECT_BYTES:
                        raise CustodyError(ERROR)
                    result[relative] = (content, stat.S_IMODE(files[-1].identity[1]))
            _require_directory(folder, identity)
        finally:
            os.close(descriptor)

    visit(path)
    return result


@dataclass
class Candidate:
    build_id: str
    receipt: dict
    token: str
    files: list[_File]
    directories: list[tuple[Path, tuple]]

    def require_current(self):
        for path, identity in self.directories:
            _require_directory(path, identity)
        for retained in self.files:
            retained.require_current()


@contextmanager
def candidate(parent_path, guide_id, build_id, *, restoring=False, budget=None):
    """Hold original source locks and validate a bounded, inert source copy.

    The original workspace owns baseline/framework validation. This copy is
    solely a safe read surface for its existing validators, never a checker,
    accepted package, worker or replacement workspace.
    """
    canonical_id(guide_id)
    canonical_id(build_id)
    path = parent_path / build_id
    directory, identity = _directory(path, private=True)
    files = []
    directories = [(path, identity)]
    try:
        with _lock(directory, ".receipt.lock", create=restoring, optional=not restoring):
            workspace_path = path / "workspace"
            workspace, workspace_identity = _directory(workspace_path, private=True)
            directories.append((workspace_path, workspace_identity))
            try:
                with _lock(workspace, "workspace.lock"):
                    # Incomplete batch publication belongs to the existing
                    # workspace recovery owner, not selection restoration.
                    try:
                        os.stat("batch-transaction.json", dir_fd=workspace, follow_symlinks=False)
                    except FileNotFoundError:
                        pass
                    else:
                        raise CustodyError(ERROR)
                    frozen = _json(_held(path / "input.json", MAX_INPUT_BYTES, files, private=True, budget=budget))
                    receipt = _json(_held(path / "receipt.json", MAX_RECEIPT_BYTES, files, private=True, budget=budget))
                    state_bytes = _held(workspace_path / "workspace-state.json", MAX_INPUT_BYTES, files, private=True, budget=budget)
                    key_bytes = _held(workspace_path / "verification-key", 4096, files, private=True, budget=budget)
                    if (not isinstance(frozen, dict) or not isinstance(receipt, dict)
                            or frozen.get("guide_id") != guide_id or receipt.get("build_id") != build_id
                            or type(frozen.get("brief")) is not dict
                            or type(frozen.get("blueprint")) is not dict
                            or type(frozen.get("guide_version")) is not int or frozen["guide_version"] < 1
                            or receipt.get("status") not in {"queued", "running", "checking", "completed", "failed", "interrupted"}
                            or not isinstance(receipt.get("created_at"), str) or len(receipt["created_at"]) > 100
                            or not frozen_matches(receipt, frozen, {"id": guide_id,
                                "version": frozen["guide_version"], "blueprint": frozen["blueprint"],
                                "brief": frozen["brief"], "organization": frozen.get("organization")})
                            or receipt.get("request") != frozen.get("request")
                            or fingerprint(receipt.get("framework")) != fingerprint(frozen.get("framework"))):
                        raise CustodyError(ERROR)
                    datetime.fromisoformat(receipt["created_at"])
                    source = _project(workspace_path / "project", files, directories, budget)
                    with tempfile.TemporaryDirectory(prefix="forge-selection-read-") as temporary:
                        copy = ImplementationWorkspace(Path(temporary) / "workspace", frozen["blueprint"])
                        copy.project.mkdir(mode=0o700)
                        for name, content in (("workspace-state.json", state_bytes), ("verification-key", key_bytes)):
                            target = copy.root / name
                            target.write_bytes(content)
                            target.chmod(0o600)
                        for name, (content, mode) in source.items():
                            target = copy.project / name
                            target.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
                            target.write_bytes(content)
                            target.chmod(mode)
                        stored = copy._state()
                        copy._files(stored)
                        framework = {key: value for key, value in (stored.get("framework") or {}).items()
                                     if key != "signature"}
                        if fingerprint(framework) != fingerprint(frozen.get("framework")):
                            raise CustodyError(ERROR)
                    held = Candidate(build_id, receipt, fingerprint({
                        "guide_id": guide_id, "build_id": build_id,
                        "directories": [(str(p.relative_to(parent_path)), i) for p, i in directories],
                        "files": [(str(f.path.relative_to(parent_path)), f.identity, f.sha256, f.size) for f in files],
                    }), files, directories)
                    held.require_current()
                    yield held
                    held.require_current()
            finally:
                os.close(workspace)
    finally:
        os.close(directory)


def candidates(root, guide_id):
    canonical_id(guide_id)
    path = root / guide_id
    try:
        parent, identity = _directory(path)
    except FileNotFoundError:
        return [], 0
    try:
        require_absent(parent)
        entries = _entries(parent, MAX_SAVED_ENTRIES)
        found, unavailable = [], 0
        budget = [MAX_RECOVERY_BYTES]
        for name in entries:
            try:
                with candidate(path, guide_id, name, budget=budget) as held:
                    found.append(held)
            except (CustodyError, DraftConflict, OSError, ValueError, TypeError, KeyError):
                unavailable += 1
        _require_directory(path, identity)
        require_absent(parent)
        return found, unavailable
    finally:
        os.close(parent)


def publish(root, guide_id, build_id, token, require_current):
    """Publish only a fully checked original selection, exclusively."""
    path = root / canonical_id(guide_id)
    parent, identity = _directory(path)
    try:
        require_absent(parent)
        with candidate(path, guide_id, build_id, restoring=True) as held:
            if not isinstance(token, str) or token != held.token:
                raise DraftConflict(ERROR)

            def fence():
                require_current()
                held.require_current()
                _require_directory(path, identity)
                require_absent(parent)

            fence()
            # Reuse the maintained complete-write/readback/no-replace primitive.
            # It neither changes the chosen receipt nor overwrites another
            # service's newly published pointer, including across a crash.
            content = ('{\n  "build_id": "' + build_id + '"\n}\n').encode("ascii")
            _publish_new_source(path / "latest.json", content, 0o600,
                                exact_mode=True, require_current=fence)
            os.fsync(parent)
    except (CustodyError, OSError, ValueError, TypeError, KeyError):
        raise DraftConflict(ERROR) from None
    finally:
        os.close(parent)
