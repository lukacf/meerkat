"""Private byte custody for the existing owner's admitted application ZIP.

The deployment owner supplies its retained receipt, current selection check and
held project lease. This projection does not verify acceptance, compile native
facts, launch a process or change installed source. Failed projections remain
private evidence; no carrier is returned for them.
"""
from __future__ import annotations

import asyncio
from copy import deepcopy
from dataclasses import dataclass
import errno
import hashlib
import os
from pathlib import Path
import stat

from .connect_installer_caller import (
    CustodyError, ExactFile, MAX_INPUT_BYTES, MAX_RECEIPT_BYTES, _joined_filesystem, _json,
)
from .deployment import DeploymentService, MAX_PACKAGE_BYTES, OWNER_FILE, ownership, package_files
from .deployment_sources import read_package_source
from .deployment_updates import _same_toml, update_binding


def _path(value):
    path = Path(value)
    if (not path.is_absolute() or ".." in path.parts
            or str(path) != os.path.abspath(path)):
        raise CustodyError("Accepted source custody needs an absolute canonical path.")
    return path


def _open_directory(path):
    """Traverse actual components without resolving a replaced link."""
    descriptor = os.open(path.anchor, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        for part in path.parts[1:]:
            child = os.open(part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW,
                            dir_fd=descriptor)
            os.close(descriptor)
            descriptor = child
        return descriptor
    except BaseException:
        os.close(descriptor)
        raise


def _identity(info):
    return info.st_dev, info.st_ino


def _file_identity(info):
    return (_identity(info), info.st_mode, info.st_uid, info.st_nlink,
            info.st_size, info.st_mtime_ns, info.st_ctime_ns)


def _directory_identity(info):
    # Journal publication legitimately changes a retained directory's times.
    return _identity(info), info.st_mode, info.st_uid


def _close(descriptor, identity):
    try:
        if _identity(os.fstat(descriptor)) == identity:
            os.close(descriptor)
    except OSError as error:
        if error.errno != errno.EBADF:
            raise


def _named_info(path):
    parent = _open_directory(path.parent)
    try:
        return os.stat(path.name, dir_fd=parent, follow_symlinks=False)
    finally:
        os.close(parent)


@dataclass(frozen=True)
class _File:
    path: Path
    identity: tuple
    sha256: str
    size: int

    def require_current(self):
        if _file_identity(_named_info(self.path)) != self.identity:
            raise CustodyError("An original or private application source file changed.")


def _read(path, limit, *, private=False, retained=None):
    parent = _open_directory(path.parent)
    descriptor = None
    try:
        descriptor = os.open(path.name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK,
                             dir_fd=parent)
        before = os.fstat(descriptor)
        if (not stat.S_ISREG(before.st_mode) or before.st_uid != os.getuid()
                or before.st_nlink != 1 or before.st_size > limit
                or private and stat.S_IMODE(before.st_mode) != 0o600):
            raise CustodyError("Application source must be an owned, ordinary bounded file.")
        content = bytearray()
        while chunk := os.read(descriptor, min(1024 * 1024, limit - len(content) + 1)):
            content.extend(chunk)
            if len(content) > limit:
                raise CustodyError("Application source exceeded its retained byte bound.")
        after = os.fstat(descriptor)
        named = os.stat(path.name, dir_fd=parent, follow_symlinks=False)
        if (len(content) != before.st_size or _file_identity(before) != _file_identity(after)
                or _file_identity(named) != _file_identity(after)):
            raise CustodyError("Application source changed during its descriptor read.")
        current_parent = _open_directory(path.parent)
        try:
            if _identity(os.fstat(current_parent)) != _identity(os.fstat(parent)):
                raise CustodyError("An application source parent changed during its read.")
        finally:
            os.close(current_parent)
        value = bytes(content)
        pin = _File(path, _file_identity(after), hashlib.sha256(value).hexdigest(), len(value))
        if retained is not None:
            retained.append((descriptor, _identity(after)))
            descriptor = None
        return value, pin
    finally:
        if descriptor is not None:
            os.close(descriptor)
        os.close(parent)


def _hold_directory(path, held, *, private):
    descriptor = _open_directory(path)
    info = os.fstat(descriptor)
    if (info.st_uid != os.getuid() or not stat.S_ISDIR(info.st_mode)
            or private and stat.S_IMODE(info.st_mode) != 0o700):
        os.close(descriptor)
        raise CustodyError("Application custody directories must be owned and private.")
    held.append((descriptor, _identity(info)))
    return descriptor, _directory_identity(info)


def _receipt(directory):
    content, _ = _read(directory / "receipt.json", MAX_RECEIPT_BYTES, private=True)
    value = _json(content)
    if not isinstance(value, dict):
        raise CustodyError("The retained installation receipt is not an object.")
    return value


def _binding(receipt):
    # Keep the original installation/source/lifecycle fences. Only the update
    # uses its existing immutable admission projection: legitimate checkpoints
    # must not invalidate retained source through connection and cleanup. Keep
    # no-update and malformed-value types distinct rather than filling defaults.
    binding = DeploymentService._configuration_binding(receipt)
    if isinstance(binding["update"], dict):
        binding["update"] = update_binding(binding["update"])
    from .deployment_application_bootstrap import restored_service
    if restored_service(receipt):
        from .deployment_service import service_binding
        binding["service"] = service_binding(binding["service"])
    return binding


def _directories(files, root):
    return {root, *(parent for name in files for parent in (root / name).parents
                    if parent == root or root in parent.parents)}


def _directory_pins(paths, *, private):
    result = {}
    for path in paths:
        descriptor = _open_directory(path)
        try:
            info = os.fstat(descriptor)
            if (info.st_uid != os.getuid()
                    or private and stat.S_IMODE(info.st_mode) != 0o700):
                raise CustodyError("Application source directory permissions changed.")
            result[path] = _directory_identity(info)
        finally:
            os.close(descriptor)
    return result


def _check_directories(pins):
    for path, expected in pins.items():
        descriptor = _open_directory(path)
        try:
            if _directory_identity(os.fstat(descriptor)) != expected:
                raise CustodyError("An application source directory was replaced or changed.")
        finally:
            os.close(descriptor)


def _write(root, files, parent, held):
    """Create only exclusive entries anchored to the retained owner directory."""
    owner_descriptor = parent
    os.mkdir(root.name, mode=0o700, dir_fd=parent)
    created = os.stat(root.name, dir_fd=parent, follow_symlinks=False)
    descriptor = os.open(root.name, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=parent)
    info = os.fstat(descriptor)
    held.append((descriptor, _identity(info)))
    if (info.st_uid != os.getuid() or stat.S_IMODE(info.st_mode) != 0o700
            or _directory_identity(info) != _directory_identity(created)
            or _directory_identity(os.stat(root.name, dir_fd=parent, follow_symlinks=False)) != _directory_identity(created)):
        raise CustodyError("The fresh private source root was replaced or is not private.")
    directories = {root: descriptor}
    for name, (content, _) in files.items():
        path = root / name
        parts = name.split("/")
        selected = root
        for part in parts[:-1]:
            child = selected / part
            if child not in directories:
                os.mkdir(part, mode=0o700, dir_fd=directories[selected])
                created = os.stat(part, dir_fd=directories[selected], follow_symlinks=False)
                descriptor = os.open(part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW,
                                     dir_fd=directories[selected])
                info = os.fstat(descriptor)
                held.append((descriptor, _identity(info)))
                if (info.st_uid != os.getuid() or stat.S_IMODE(info.st_mode) != 0o700
                        or _directory_identity(info) != _directory_identity(created)
                        or _directory_identity(os.stat(part, dir_fd=directories[selected], follow_symlinks=False)) != _directory_identity(created)):
                    raise CustodyError("A fresh private source directory was replaced or is not private.")
                directories[child] = descriptor
            selected = child
        parent = directories[selected]
        descriptor = None
        try:
            parent_info = os.fstat(parent)
            if parent_info.st_uid != os.getuid() or stat.S_IMODE(parent_info.st_mode) != 0o700:
                raise CustodyError("A private source parent changed before writing.")
            descriptor = os.open(path.name, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,
                                 0o600, dir_fd=parent)
            os.fchmod(descriptor, 0o600)
            view = memoryview(content)
            while view:
                written = os.write(descriptor, view)
                if not written:
                    raise CustodyError("A private source write did not complete.")
                view = view[written:]
            os.fsync(descriptor)
        finally:
            if descriptor is not None:
                os.close(descriptor)
    for selected, descriptor in sorted(directories.items(), key=lambda entry: len(entry[0].parts), reverse=True):
        os.fsync(descriptor)
    os.fsync(owner_descriptor)
    return directories[root], _directory_identity(os.fstat(directories[root]))


def _closure(root, expected_files, expected_directories):
    files, directories = set(), {root}
    pending = [root]
    while pending:
        path = pending.pop()
        descriptor = _open_directory(path)
        try:
            for name in os.listdir(descriptor):
                selected = path / name
                info = os.stat(name, dir_fd=descriptor, follow_symlinks=False)
                if stat.S_ISDIR(info.st_mode):
                    directories.add(selected)
                    pending.append(selected)
                elif stat.S_ISREG(info.st_mode):
                    files.add(selected)
                else:
                    raise CustodyError("Private source custody contains a link or special file.")
                if len(files) + len(directories) > len(expected_files) + len(expected_directories):
                    raise CustodyError("The private application source inventory changed.")
        finally:
            os.close(descriptor)
    if files != expected_files or directories != expected_directories:
        raise CustodyError("The private application source inventory changed.")


class AcceptedApplicationSource:
    """Retained deployment bytes, valid only under the supplied original lease.

    Empty and large resources remain in the full source closure. Selecting an
    ExactFile for a native input applies that existing caller's narrower bounds.
    This carrier does not assert native authority or installed readiness.
    """

    def __init__(self, directory, root, receipt, source, project_lease, require_owner_current,
                 held, roots, package, installed, marker, private, directories):
        self.directory, self.root = directory, root
        self._receipt, self._source = deepcopy(receipt), deepcopy(source)
        self._binding = _binding(receipt)
        self._lease, self._owner_current = project_lease, require_owner_current
        self._held, self._roots, self._package = held, roots, package
        self._installed, self._marker, self._private = installed, marker, private
        self._directories = directories
        self._closed = self._invalidated = False

    @property
    def source(self):
        return deepcopy(self._source)

    @property
    def receipt(self):
        return deepcopy(self._receipt)

    @property
    def files(self):
        return tuple(self._private)

    def _lease_current(self):
        if self._closed or self._invalidated:
            raise CustodyError("Accepted application source custody is no longer current.")
        if _path(self._lease.root) != _path(self._receipt["destination"]):
            raise CustodyError("The retained source belongs to another held project.")
        self._lease.require_current()

    def _owner(self):
        self._lease_current()
        self._owner_current()

    def _filesystem(self):
        for path, (descriptor, expected) in self._roots.items():
            if _directory_identity(os.fstat(descriptor)) != expected:
                raise CustodyError("A retained application directory descriptor changed.")
            _check_directories({path: expected})
        for descriptor, identity in self._held:
            if _identity(os.fstat(descriptor)) != identity:
                raise CustodyError("A retained application descriptor was lost.")
        if not _same_toml(_binding(_receipt(self.directory)), self._binding):
            raise CustodyError("The admitted application source or installation owner changed.")
        self._package.require_current()
        self._marker.require_current()
        _check_directories(self._directories)
        for pin in (*self._installed.values(), *self._private.values()):
            pin.require_current()
        _closure(self.root, {pin.path for pin in self._private.values()},
                 {path for path in self._directories if path == self.root or self.root in path.parents})

    def require_owner_current(self):
        """Loop-side owner component, without the retained source traversal.

        This component checks the original lease and selection callback. It does
        not establish filesystem currentness or native authority by itself.
        """
        try:
            self._owner()
        except (OSError, ValueError, RuntimeError):
            self._invalidated = True
            raise

    def require_filesystem_current(self):
        """Worker-side component; never call the loop-owned selection callback.

        Validate the original lease and full retained filesystem closure. The
        caller must fence its joined worker with require_owner_current before
        and after it. Filesystem validation alone is not sufficient selection
        currentness or native authority. A failure invalidates custody here,
        including when worker settlement subsequently reports cancellation.
        """
        try:
            self._lease_current()
            self._filesystem()
            self._lease_current()
        except (OSError, ValueError, RuntimeError):
            self._invalidated = True
            raise

    def require_current(self):
        """Existing synchronous caller fence; no hashing or application execution."""
        self.require_owner_current()
        self.require_filesystem_current()

    async def require_current_async(self):
        """Join full source validation off-loop with loop-side owner fences."""
        try:
            self.require_owner_current()
            _, cancelled = await _joined_filesystem(self.require_filesystem_current)
            if cancelled:
                raise asyncio.CancelledError
            self.require_owner_current()
        except (OSError, ValueError, RuntimeError):
            self._invalidated = True
            raise

    def exact_file(self, name):
        """Select one retained native input without omitting other source files."""
        self.require_current()
        if self._closed or self._invalidated or name not in self._private:
            raise CustodyError("Select a file from this current retained application source.")
        pin = self._private[name]
        if not 0 < pin.size <= MAX_INPUT_BYTES:
            raise CustodyError("This resource exceeds the existing native input file contract.")
        pin.require_current()
        return ExactFile(pin.path, pin.sha256)

    def close(self):
        """Release descriptors, preserving exact private evidence and installed data."""
        if not self._closed:
            self._closed = True
            for descriptor, identity in reversed(self._held):
                _close(descriptor, identity)


def _retain(directory, receipt, root, lease, owner_current):
    held, roots = [], {}
    try:
        roots[directory] = _hold_directory(directory, held, private=True)
        if not _same_toml(_binding(_receipt(directory)), _binding(receipt)):
            raise CustodyError("A different application is retained by the installation owner.")
        payload, package = _read(directory / "package.zip", MAX_PACKAGE_BYTES, private=True, retained=held)
        if package.sha256 != receipt["package_sha256"]:
            raise CustodyError("The retained accepted package bytes changed.")
        files = package_files(payload)
        source = read_package_source(files, receipt)
        if source["source_kind"] != "application" or source["brief_hash"] != receipt.get("brief_hash"):
            raise CustodyError("The retained package is not this admitted application source.")
        destination = _path(receipt["destination"])
        roots[destination] = _hold_directory(destination, held, private=False)
        marker_content, marker = _read(destination / OWNER_FILE, MAX_RECEIPT_BYTES, private=True)
        if not _same_toml(_json(marker_content), ownership(receipt)):
            raise CustodyError("The retained source has no matching installation ownership marker.")
        installed = {}
        for name, (content, _) in files.items():
            observed, pin = _read(destination / name, MAX_PACKAGE_BYTES)
            if observed != content:
                raise CustodyError("Installed source differs from its admitted application package.")
            installed[name] = pin
        directory_pins = _directory_pins(_directories(files, destination), private=False)
        package.require_current()
        marker.require_current()
        for pin in installed.values():
            pin.require_current()
        _check_directories(directory_pins)
        if not _same_toml(_binding(_receipt(directory)), _binding(receipt)):
            raise CustodyError("Application admission changed before its private projection.")
        roots[root] = _write(root, files, roots[directory][0], held)
        private = {}
        for name, (content, _) in files.items():
            observed, pin = _read(root / name, MAX_PACKAGE_BYTES, private=True)
            if observed != content:
                raise CustodyError("A private application file differs from its accepted bytes.")
            private[name] = pin
        directory_pins.update(_directory_pins(_directories(files, root), private=True))
        result = AcceptedApplicationSource(directory, root, receipt, source, lease, owner_current,
            held, roots, package, installed, marker, private, directory_pins)
        result._filesystem()
        return result
    except BaseException:
        for descriptor, identity in reversed(held):
            _close(descriptor, identity)
        raise


async def retain_accepted_application_source(installation_directory, receipt, *, project_lease,
        require_owner_current, custody_directory):
    """Project the exact retained deployment under its existing child/factory lease.

    require_owner_current is the trusted deployment owner's synchronous current
    selection check and runs on its event loop. Creation joins its filesystem
    worker even on repeated cancellation. No partially returned carrier survives
    cancellation, and no existing path is overwritten or removed.
    """
    directory, root = _path(installation_directory), _path(custody_directory)
    selected = deepcopy(receipt)
    if (not isinstance(selected, dict) or root.parent != directory
            or directory.name != selected.get("deployment_id")
            or directory.parent.name != selected.get("guide_id")
            or selected.get("source_kind", "application") != "application"
            or not callable(require_owner_current) or not callable(getattr(project_lease, "require_current", None))
            or _path(project_lease.root) != _path(selected["destination"])):
        raise CustodyError("Private source custody requires this retained installation and its held project lease.")
    project_lease.require_current()
    require_owner_current()
    result, cancelled = await _joined_filesystem(_retain, directory, selected, root,
                                                project_lease, require_owner_current)
    if cancelled:
        result.close()
        raise asyncio.CancelledError
    try:
        await result.require_current_async()
    except BaseException:
        result.close()
        raise
    return result
