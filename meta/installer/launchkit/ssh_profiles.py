"""Private host-selected SSH files for exact Linux endpoints, never browser input."""
from __future__ import annotations

import hashlib
import json
import math
import os
from pathlib import Path
import stat
import tempfile

from .ssh_transport import (SshTransport, SshTransportError, validate_endpoint,
                            _operation_deadline, _remaining)
from .ssh_browser_access import SshBrowserAccessTransport, BrowserAccessError


MAX_PROFILES = 32
MAX_DOCUMENT = 256 * 1024
MAX_IDENTITY = 64 * 1024
MAX_KNOWN_HOSTS = 1024 * 1024
CONFIGURATION_ERROR = (
    "Configured SSH profiles are unavailable or unsafe. Check the private "
    "META_FORGE_SSH_PROFILES files and restart Forge after correcting them. "
    "Remote work may still be running; no replacement connection was admitted."
)


class SshProfilesError(ValueError):
    def __init__(self):
        super().__init__(CONFIGURATION_ERROR)


def _path(value):
    if (not isinstance(value, str) or not 1 <= len(value) <= 4096
            or not value.startswith("/") or any(part in {"", ".", ".."} for part in value.split("/")[1:])
            or any(ord(char) < 32 or ord(char) == 127 for char in value)):
        raise SshProfilesError()
    return Path(value)


def _private_bytes(path, maximum):
    """Walk with no-follow directory descriptors, then read one bounded owned file."""
    directory = descriptor = None
    try:
        directory = os.open("/", os.O_RDONLY | os.O_DIRECTORY)
        for component in path.parts[1:-1]:
            child = os.open(component, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=directory)
            os.close(directory)
            directory = child
        descriptor = os.open(path.name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=directory)
        before = os.fstat(descriptor)
        if (not stat.S_ISREG(before.st_mode) or before.st_uid != os.getuid()
                or before.st_nlink != 1 or before.st_mode & 0o077
                or before.st_mode & 0o7000 or not before.st_mode & 0o400
                or not 0 < before.st_size <= maximum):
            raise SshProfilesError()
        data = bytearray()
        while len(data) <= maximum:
            chunk = os.read(descriptor, min(65536, maximum + 1 - len(data)))
            if not chunk:
                break
            data.extend(chunk)
        after = os.fstat(descriptor)
        if (len(data) != before.st_size or len(data) > maximum
                or (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns, before.st_ctime_ns)
                != (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns, after.st_ctime_ns)):
            raise SshProfilesError()
        return bytes(data)
    except (OSError, ValueError):
        raise SshProfilesError() from None
    finally:
        if descriptor is not None:
            os.close(descriptor)
        if directory is not None:
            os.close(directory)


def _object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise SshProfilesError()
        result[key] = value
    return result


def _invalid_constant(_):
    raise SshProfilesError()


def _key(endpoint):
    value = validate_endpoint(endpoint)
    return value["host"], value["user"], value["ssh_port"]


def _budget(timeout, deadline):
    try:
        if (isinstance(timeout, bool) or not isinstance(timeout, (int, float))
                or not math.isfinite(timeout) or timeout <= 0):
            raise SshTransportError("invalid_request")
    except OverflowError:
        raise SshTransportError("invalid_request") from None
    result = _operation_deadline(timeout, deadline)
    _remaining(result)
    return result


class _SelectedBrowserHandle:
    """Keep the selected private copies until the existing SSH guardian joins."""
    def __init__(self, handle, temporary):
        self._handle, self._temporary = handle, temporary

    async def observe(self):
        return await self._handle.observe()

    async def close(self):
        joined = await self._handle.close()
        if joined and self._temporary is not None:
            self._temporary.cleanup()
            self._temporary = None
        return joined


class SshProfiles:
    def __init__(self, path):
        self._path = _path(path)
        document = _private_bytes(self._path, MAX_DOCUMENT)
        self._document_sha256 = hashlib.sha256(document).digest()
        self._profiles = {}
        try:
            value = json.loads(document, object_pairs_hook=_object, parse_constant=_invalid_constant)
            if (not isinstance(value, dict) or set(value) != {"version", "profiles"}
                    or type(value["version"]) is not int or value["version"] != 1
                    or not isinstance(value["profiles"], list) or len(value["profiles"]) > MAX_PROFILES):
                raise SshProfilesError()
            for row in value["profiles"]:
                if (not isinstance(row, dict) or set(row) != {"endpoint", "identity_file", "known_hosts_file"}
                        or not isinstance(row["endpoint"], dict)
                        or set(row["endpoint"]) != {"host", "user", "ssh_port"}):
                    raise SshProfilesError()
                key = _key(row["endpoint"])
                if key in self._profiles:
                    raise SshProfilesError()
                identity, hosts = _path(row["identity_file"]), _path(row["known_hosts_file"])
                self._profiles[key] = (
                    identity, hashlib.sha256(_private_bytes(identity, MAX_IDENTITY)).digest(),
                    hosts, hashlib.sha256(_private_bytes(hosts, MAX_KNOWN_HOSTS)).digest(),
                )
        except (ValueError, TypeError, UnicodeError, RecursionError, SshTransportError):
            raise SshProfilesError() from None

    def _selection(self, endpoint):
        selected = self._profiles.get(_key(endpoint))
        if selected is None:
            return None
        if hashlib.sha256(_private_bytes(self._path, MAX_DOCUMENT)).digest() != self._document_sha256:
            raise SshProfilesError()
        identity, identity_digest, hosts, hosts_digest = selected
        key_bytes = _private_bytes(identity, MAX_IDENTITY)
        host_bytes = _private_bytes(hosts, MAX_KNOWN_HOSTS)
        if (hashlib.sha256(key_bytes).digest() != identity_digest
                or hashlib.sha256(host_bytes).digest() != hosts_digest):
            raise SshProfilesError()
        return key_bytes, host_bytes

    @staticmethod
    def _copies(selected):
        temporary = tempfile.TemporaryDirectory(prefix="forge-ssh-profile-")
        try:
            directory = Path(temporary.name).resolve()
            # Fixed OpenSSH option values must not gain config/token expansion.
            if any(char.isspace() or char in '%~"\\' or ord(char) < 32 or ord(char) == 127
                   for char in str(directory)):
                raise SshProfilesError()
            paths = (directory / "identity", directory / "known_hosts")
            for path, content in zip(paths, selected):
                descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
                with os.fdopen(descriptor, "wb") as stream:
                    stream.write(content)
            prefix = ("-i", str(paths[0]), "-o", "IdentitiesOnly=yes", "-o", "IdentityAgent=none",
                      "-o", "PreferredAuthentications=publickey", "-o", "AddKeysToAgent=no",
                      "-o", "GlobalKnownHostsFile=/dev/null")
            return temporary, prefix, paths[1]
        except BaseException:
            temporary.cleanup()
            raise

    def browser_transport(self, endpoint):
        return self if _key(endpoint) in self._profiles else None

    async def request(self, endpoint, program, payload, timeout=30, *, expected_fingerprint=None, deadline=None):
        deadline = _budget(timeout, deadline)
        try:
            selected = self._selection(endpoint)
            if selected is None:
                return await SshTransport().request(endpoint, program, payload, timeout,
                    expected_fingerprint=expected_fingerprint, deadline=deadline)
            temporary, prefix, hosts = self._copies(selected)
            if expected_fingerprint is None:
                # Initial trust comes from the selected file. Later requests
                # must leave UserKnownHostsFile to the exact native pin owner.
                prefix += ("-o", "UserKnownHostsFile=" + str(hosts))
        except SshProfilesError:
            raise SshTransportError("configuration_unavailable") from None
        try:
            return await SshTransport(argv_prefix=prefix, known_hosts_paths=(hosts,)).request(
                endpoint, program, payload, _remaining(deadline), expected_fingerprint=expected_fingerprint, deadline=deadline)
        finally:
            temporary.cleanup()

    async def transfer_file(self, endpoint, program, payload, artifact, timeout=30,
                            *, expected_fingerprint=None, deadline=None, require_current=None):
        """Use the same immutable profile identity for a bounded file stream."""
        deadline = _budget(timeout, deadline)
        try:
            selected = self._selection(endpoint)
            if selected is None:
                return await SshTransport().transfer_file(endpoint, program, payload, artifact, timeout,
                    expected_fingerprint=expected_fingerprint, deadline=deadline, require_current=require_current)
            temporary, prefix, hosts = self._copies(selected)
        except SshProfilesError:
            raise SshTransportError("configuration_unavailable") from None
        try:
            return await SshTransport(argv_prefix=prefix, known_hosts_paths=(hosts,)).transfer_file(
                endpoint, program, payload, artifact, _remaining(deadline),
                expected_fingerprint=expected_fingerprint, deadline=deadline, require_current=require_current)
        finally:
            temporary.cleanup()

    async def open(self, endpoint, *, expected_fingerprint, port, timeout=15, deadline=None):
        try:
            deadline = _budget(timeout, deadline)
            selected = self._selection(endpoint)
            if selected is None:
                return await SshBrowserAccessTransport().open(endpoint, expected_fingerprint=expected_fingerprint,
                    port=port, timeout=timeout, deadline=deadline)
            temporary, prefix, hosts = self._copies(selected)
        except SshProfilesError:
            raise BrowserAccessError("configuration-unavailable") from None
        except SshTransportError as error:
            raise BrowserAccessError("invalid-endpoint" if error.code == "invalid_endpoint"
                else "timeout" if error.code == "timeout" else "invalid-request") from None
        try:
            handle = await SshBrowserAccessTransport(argv_prefix=prefix, known_hosts_paths=(hosts,)).open(
                endpoint, expected_fingerprint=expected_fingerprint, port=port, timeout=_remaining(deadline), deadline=deadline)
        except BaseException as error:
            if isinstance(error, BrowserAccessError) and getattr(error, "handle", None) is not None:
                error.handle = _SelectedBrowserHandle(error.handle, temporary)
            else:
                temporary.cleanup()
            if isinstance(error, SshTransportError):
                raise BrowserAccessError("timeout" if error.code == "timeout" else "invalid-request") from None
            raise
        return _SelectedBrowserHandle(handle, temporary)


def configured_ssh_transport():
    selected = os.environ.get("META_FORGE_SSH_PROFILES")
    return None if selected is None else SshProfiles(selected)
