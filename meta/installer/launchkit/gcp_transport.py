"""Source-owned IAP routing over the existing pinned SSH RPC and guardian.

Cloud observations, host keys and private identity paths come from host-owned
callbacks. Neither the endpoint nor remote payload can supply SSH arguments.
"""
from __future__ import annotations

import asyncio
import base64
from collections.abc import Mapping
import hashlib
import inspect
import math
import os
from pathlib import Path
import re
import shlex
import stat
import tempfile
import uuid

from .ssh_transport import (SshTransport, SshTransportError, _fingerprint_digest,
    _operation_deadline, _transfer_current)
from .ssh_browser_access import SshBrowserAccessTransport, BrowserAccessError, _settle_cleanup


FIELDS = frozenset({"transport", "operation_id", "account", "project", "zone", "instance",
                    "instance_id", "ownership_token", "host", "user", "ssh_port"})
ACCOUNT = re.compile(r"[A-Za-z0-9][A-Za-z0-9._+%-]{0,190}@[A-Za-z0-9][A-Za-z0-9.-]{0,190}\Z", re.ASCII)
PROJECT = re.compile(r"[a-z][a-z0-9-]{4,28}[a-z0-9]\Z", re.ASCII)
RESOURCE = re.compile(r"[a-z](?:[a-z0-9-]{0,61}[a-z0-9])?\Z", re.ASCII)
NUMBER = re.compile(r"[1-9][0-9]{0,19}\Z", re.ASCII)
TOKEN = re.compile(r"[a-f0-9]{32}\Z", re.ASCII)
SAFE_PATH = "/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin:/opt/homebrew/bin:/opt/homebrew/share/google-cloud-sdk/bin"


def validate_gcp_endpoint(value):
    if not isinstance(value, Mapping) or set(value) != FIELDS:
        raise SshTransportError("invalid_endpoint")
    endpoint = dict(value)
    try:
        operation = endpoint["operation_id"]
        valid = (isinstance(operation, str) and str(uuid.UUID(operation)) == operation
            and endpoint["transport"] == "gcp-iap"
            and isinstance(endpoint["account"], str) and ACCOUNT.fullmatch(endpoint["account"])
            and isinstance(endpoint["project"], str) and PROJECT.fullmatch(endpoint["project"])
            and isinstance(endpoint["zone"], str) and RESOURCE.fullmatch(endpoint["zone"])
            and isinstance(endpoint["instance"], str) and RESOURCE.fullmatch(endpoint["instance"])
            and isinstance(endpoint["instance_id"], str) and NUMBER.fullmatch(endpoint["instance_id"])
            and int(endpoint["instance_id"]) <= 2**64 - 1
            and isinstance(endpoint["ownership_token"], str) and TOKEN.fullmatch(endpoint["ownership_token"])
            and endpoint["host"] == "compute." + endpoint["instance_id"]
            and endpoint["user"] == "mobkit"
            and type(endpoint["ssh_port"]) is int and endpoint["ssh_port"] == 22)
    except (ValueError, TypeError, AttributeError):
        valid = False
    if not valid:
        raise SshTransportError("invalid_endpoint")
    return endpoint


def _absolute(value):
    try:
        value = os.fspath(value)
    except TypeError:
        raise ValueError("Use a checked absolute host path for IAP transport.") from None
    if (not isinstance(value, str) or not Path(value).is_absolute() or len(value) > 2000
            or any(ord(char) < 32 or ord(char) == 127 for char in value)
            or "%" in value or "~" in value or ".." in Path(value).parts):
        raise ValueError("Use a checked absolute host path for IAP transport.")
    return value


def _identity(value):
    try:
        path = Path(_absolute(value))
        if any(item.is_symlink() for item in (path, *path.parents)):
            raise ValueError
        info, parent = path.lstat(), path.parent.lstat()
        if (not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid()
                or info.st_mode & 0o077 or not 1 <= info.st_size <= 64 * 1024
                or not stat.S_ISDIR(parent.st_mode) or parent.st_uid != os.getuid()
                or parent.st_mode & 0o077):
            raise ValueError
    except (ValueError, TypeError, OSError):
        raise SshTransportError("connection_unavailable") from None
    return str(path)


def checked_host_key(value, alias):
    """Accept one exact canonical Ed25519 guest key, never comments or certs."""
    if (not isinstance(value, Mapping) or set(value) != {"key_type", "key"}
            or value.get("key_type") != "ssh-ed25519" or not isinstance(value.get("key"), str)):
        raise SshTransportError("host_key_unavailable")
    encoded = value["key"]
    try:
        if len(encoded) != 68 or not encoded.isascii():
            raise ValueError
        raw = base64.b64decode(encoded, validate=True)
        if base64.b64encode(raw).decode("ascii") != encoded:
            raise ValueError
        expected = b"\x00\x00\x00\x0bssh-ed25519\x00\x00\x00\x20"
        if len(raw) != 51 or raw[:19] != expected:
            raise ValueError
    except (ValueError, TypeError):
        raise SshTransportError("host_key_unavailable") from None
    fingerprint = "SHA256:" + base64.b64encode(hashlib.sha256(raw).digest()).decode("ascii").rstrip("=")
    return (alias + " ssh-ed25519 " + encoded + "\n").encode("ascii"), fingerprint


async def _call(callback, endpoint):
    result = callback(dict(endpoint)) if callable(callback) else callback
    return await result if inspect.isawaitable(result) else result


def _remaining(deadline):
    remaining = deadline - asyncio.get_running_loop().time()
    if remaining <= 0:
        raise TimeoutError
    return remaining


class GcpIapTransport:
    def __init__(self, verify, host_keys, identity_file, *, gcloud_executable,
                 cli_home, cli_config_directory):
        if not callable(verify) or not callable(host_keys):
            raise ValueError("IAP requires host-owned identity and guest key observations.")
        self.verify, self.host_keys, self.identity_file = verify, host_keys, identity_file
        self.gcloud = _absolute(gcloud_executable)
        self.home, self.config = _absolute(cli_home), _absolute(cli_config_directory)

    def _prefix(self, endpoint, identity):
        command = ["/usr/bin/env", "-i", "HOME=" + self.home, "PATH=" + SAFE_PATH,
            "CLOUDSDK_CONFIG=" + self.config, "CLOUDSDK_CORE_DISABLE_PROMPTS=1", "LANG=C", "LC_ALL=C",
            self.gcloud, "--quiet", "--account=" + endpoint["account"], "--project=" + endpoint["project"],
            "compute", "start-iap-tunnel", endpoint["instance"], "22", "--listen-on-stdin",
            "--zone=" + endpoint["zone"], "--verbosity=error"]
        # OpenSSH expands percent tokens before invoking its shell. Literal
        # percent characters in an ordinary selected account must stay literal.
        proxy = shlex.join(command).replace("%", "%%")
        return ("-o", "ProxyCommand=" + proxy, "-o", "HostKeyAlias=" + endpoint["host"],
                "-o", "IdentityFile=" + identity, "-o", "IdentitiesOnly=yes", "-o", "CheckHostIP=no")

    async def _prepare(self, endpoint, expected, deadline):
        selected = validate_gcp_endpoint(endpoint)
        _remaining(deadline)
        checked = await _call(self.verify, selected)
        if not isinstance(checked, Mapping) or dict(checked) != selected:
            raise SshTransportError("connection_unavailable")
        _remaining(deadline)
        key, fingerprint = checked_host_key(await _call(self.host_keys, selected), selected["host"])
        if expected is not None and expected != fingerprint:
            raise SshTransportError("host_key_unavailable")
        _remaining(deadline)
        identity = _identity(await _call(self.identity_file, selected))
        _remaining(deadline)
        # Key/path observation cannot substitute an earlier VM/account check.
        checked = await _call(self.verify, selected)
        if not isinstance(checked, Mapping) or dict(checked) != selected:
            raise SshTransportError("connection_unavailable")
        _remaining(deadline)
        base = {name: selected[name] for name in ("host", "user", "ssh_port")}
        return base, self._prefix(selected, identity), key, fingerprint

    async def request(self, endpoint, program, payload, timeout=30, *, expected_fingerprint=None, deadline=None):
        if (not isinstance(program, str) or not program or "\0" in program or not isinstance(payload, dict)
                or not _timeout(timeout)
                or expected_fingerprint is not None and _fingerprint_digest(expected_fingerprint) is None):
            raise SshTransportError("invalid_request")
        deadline = _operation_deadline(timeout, deadline)
        try:
            # Ownership/key observations and SSH share one operation budget.
            # The callbacks and SSH transport retain custody during cancellation
            # cleanup; do not start another timer around their cleanup paths.
            async with asyncio.timeout_at(deadline):
                base, prefix, key, fingerprint = await self._prepare(endpoint, expected_fingerprint, deadline)
            with tempfile.TemporaryDirectory(prefix="mobkit-iap-pin-") as temporary:
                pin = Path(temporary) / "known_hosts"
                descriptor = os.open(pin, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
                with os.fdopen(descriptor, "wb") as stream:
                    stream.write(key)
                transport = SshTransport(argv_prefix=prefix, known_hosts_paths=(pin,))
                result = await transport.request(base, program, payload, _remaining(deadline),
                    expected_fingerprint=fingerprint, deadline=deadline)
            _remaining(deadline)
            return result
        except SshTransportError:
            raise
        except TimeoutError:
            raise SshTransportError("timeout") from None
        except (ValueError, LookupError, OSError, RuntimeError):
            raise SshTransportError("connection_unavailable") from None

    async def transfer_file(self, endpoint, program, payload, artifact, timeout=30,
                            *, expected_fingerprint=None, deadline=None, require_current=None):
        """Retain the existing VM, account, guest key and SSH owner for transfer."""
        if (not isinstance(program, str) or not program or "\0" in program or type(payload) is not dict
                or not _timeout(timeout) or _fingerprint_digest(expected_fingerprint) is None):
            raise SshTransportError("invalid_request")
        deadline = _operation_deadline(timeout, deadline)
        try:
            _transfer_current(require_current)
            async with asyncio.timeout_at(deadline):
                base, prefix, key, fingerprint = await self._prepare(endpoint, expected_fingerprint, deadline)
            _transfer_current(require_current)
            with tempfile.TemporaryDirectory(prefix="forge-iap-artifact-pin-") as temporary:
                pin = Path(temporary) / "known_hosts"
                descriptor = os.open(pin, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
                with os.fdopen(descriptor, "wb") as stream:
                    stream.write(key)
                transport = SshTransport(argv_prefix=prefix, known_hosts_paths=(pin,))
                result = await transport.transfer_file(base, program, payload, artifact, _remaining(deadline),
                    expected_fingerprint=fingerprint, deadline=deadline, require_current=require_current)
            _remaining(deadline)
            _transfer_current(require_current)
            return result
        except SshTransportError:
            raise
        except TimeoutError:
            raise SshTransportError("timeout") from None
        except (ValueError, LookupError, OSError, RuntimeError):
            raise SshTransportError("connection_unavailable") from None

    async def open(self, endpoint, *, expected_fingerprint, port, timeout=15):
        if (type(port) is not int or not 1024 <= port <= 65535 or not _timeout(timeout, maximum=60)
                or _fingerprint_digest(expected_fingerprint) is None):
            raise BrowserAccessError("invalid-request")
        deadline = asyncio.get_running_loop().time() + timeout
        handle = None
        try:
            async with asyncio.timeout_at(deadline):
                base, prefix, key, fingerprint = await self._prepare(endpoint, expected_fingerprint, deadline)
            with tempfile.TemporaryDirectory(prefix="mobkit-iap-pin-") as temporary:
                pin = Path(temporary) / "known_hosts"
                descriptor = os.open(pin, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
                with os.fdopen(descriptor, "wb") as stream:
                    stream.write(key)
                transport = SshBrowserAccessTransport(argv_prefix=prefix, known_hosts_paths=(pin,))
                # The guardian copies the pin to its owned temporary directory
                # before open returns, then owns that copy through close().
                handle = await transport.open(base, expected_fingerprint=fingerprint, port=port,
                    timeout=_remaining(deadline), deadline=deadline)
            _remaining(deadline)
            return handle
        except BaseException as error:
            # A local temporary-file cleanup failure after the guardian starts
            # must not lose its real handle or escape the normal close path.
            if handle is not None:
                cleaned, cancelled = await _settle_cleanup(handle)
                if not cleaned:
                    raise BrowserAccessError("cleanup-unavailable", handle=handle) from None
                if cancelled:
                    raise asyncio.CancelledError() from None
            if isinstance(error, BrowserAccessError):
                raise
            if isinstance(error, TimeoutError):
                raise BrowserAccessError("timeout") from None
            if isinstance(error, SshTransportError):
                code = {"host_key_unavailable": "host-key-unavailable", "invalid_endpoint": "invalid-endpoint",
                    "timeout": "timeout"}.get(error.code, "connection-unavailable")
                raise BrowserAccessError(code) from None
            if isinstance(error, (ValueError, LookupError, OSError, RuntimeError)):
                raise BrowserAccessError("connection-unavailable") from None
            raise


def _timeout(value, maximum=None):
    try:
        return (not isinstance(value, bool) and isinstance(value, (int, float))
            and math.isfinite(value) and value > 0 and (maximum is None or value <= maximum))
    except OverflowError:
        return False
