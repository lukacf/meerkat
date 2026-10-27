"""Trusted, bounded SSH RPC observation without inferring remote lifecycle state."""
from __future__ import annotations

import asyncio
import base64
from collections.abc import Mapping, Sequence
import ipaddress
import json
import math
import os
from pathlib import Path
import pwd
import re
import shlex
import signal
import subprocess
import tempfile
import hashlib
import inspect
import stat
import struct


FRAME_PREFIX = "MOBKIT_REMOTE_V1 "
MAX_STDOUT_BYTES = 256 * 1024
MAX_STDERR_BYTES = 64 * 1024
FORCE_REAP_TIMEOUT = 5.0
MAX_TRANSFER_HEADER_BYTES = 32 * 1024 * 1024
MAX_TRANSFER_FILE_BYTES = 3 * 1024 * 1024 * 1024
TRANSFER_CHUNK_BYTES = 1024 * 1024

_MESSAGES = {
    "configuration_unavailable": "Configured SSH profiles are unavailable or unsafe. Check the private META_FORGE_SSH_PROFILES files and restart Forge after correcting them. Remote work may still be running; no replacement connection was admitted.",
    "invalid_endpoint": "The SSH destination is invalid. Remote observation is unavailable.",
    "invalid_request": "The SSH observation request is invalid. Remote observation is unavailable.",
    "connection_unavailable": "The SSH connection did not provide an observation. Remote observation is unavailable; remote work may still be running.",
    "timeout": "The SSH observation timed out. Remote observation is unavailable; remote work may still be running.",
    "output_limit": "The SSH observation exceeded its output limit. Remote observation is unavailable; remote work may still be running.",
    "invalid_response": "The SSH connection returned an invalid observation. Remote observation is unavailable; remote work may still be running.",
    "cleanup_unavailable": "The local SSH process did not confirm exit within its cleanup limit. Remote observation is unavailable; remote work may still be running.",
    "host_key_unavailable": "The previously verified SSH server key could not be pinned. Remote observation is unavailable; no new remote program was admitted.",
}
_HOST_ALIAS = re.compile(r"[A-Za-z0-9](?:[A-Za-z0-9_-]|\.(?=[A-Za-z0-9]))*(?:\.)?", re.ASCII)
_USER = re.compile(r"[A-Za-z0-9_][A-Za-z0-9_.-]{0,63}", re.ASCII)
_SCOPE = re.compile(r"[A-Za-z0-9_][A-Za-z0-9_.-]{0,63}", re.ASCII)
_HOST_KEY = re.compile(rb"debug1: Server host key: ([a-zA-Z0-9@._+-]{1,80}) (SHA256:[A-Za-z0-9+/]{43}=?)")
_KNOWN_KEY = re.compile(rb"debug1: Host '[^'\r\n]{1,512}' is known and matches the ([A-Z0-9-]{1,32}) host key\.")
_KEY_NAMES = {
    b"ssh-ed25519": b"ED25519", b"ssh-rsa": b"RSA", b"ssh-dss": b"DSA",
    b"ecdsa-sha2-nistp256": b"ECDSA", b"ecdsa-sha2-nistp384": b"ECDSA",
    b"ecdsa-sha2-nistp521": b"ECDSA", b"sk-ssh-ed25519@openssh.com": b"ED25519-SK",
    b"sk-ecdsa-sha2-nistp256@openssh.com": b"ECDSA-SK",
}


class SshTransportError(RuntimeError):
    """A fixed public error, with no subprocess output or remote-state assertion."""

    def __init__(self, code: str):
        if code not in _MESSAGES:
            raise ValueError("Unknown SSH transport error code.")
        self.code = code
        super().__init__(_MESSAGES[code])


def validate_endpoint(endpoint: Mapping) -> dict:
    """Return an independent host/user/ssh_port record before process creation."""
    if not isinstance(endpoint, Mapping) or set(endpoint) - {"host", "user", "ssh_port"}:
        raise SshTransportError("invalid_endpoint")
    host, user, port = endpoint.get("host"), endpoint.get("user"), endpoint.get("ssh_port", 22)
    if (not isinstance(host, str) or not host.isascii() or not 1 <= len(host) <= 253
            or not isinstance(user, str) or not _USER.fullmatch(user)
            or type(port) is not int or not 1 <= port <= 65535):
        raise SshTransportError("invalid_endpoint")
    address = host
    if "%" in host:
        address, scope = host.rsplit("%", 1)
        if not _SCOPE.fullmatch(scope):
            raise SshTransportError("invalid_endpoint")
    try:
        parsed = ipaddress.ip_address(address)
    except ValueError:
        if "%" in host or not _HOST_ALIAS.fullmatch(host):
            raise SshTransportError("invalid_endpoint") from None
    else:
        if "%" in host and parsed.version != 6:
            raise SshTransportError("invalid_endpoint")
    return {"host": host, "user": user, "ssh_port": port}


def _environment() -> dict[str, str]:
    # OpenSSH still uses ordinary identity files, known_hosts, and agent auth.
    # Provider credentials and unrelated local runtime settings are not inherited.
    result = {key: os.environ[key] for key in ("HOME", "USER", "PATH", "SSH_AUTH_SOCK") if key in os.environ}
    result.setdefault("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
    return result


def _fingerprint_digest(value) -> bytes | None:
    if not isinstance(value, str) or not re.fullmatch(r"SHA256:[A-Za-z0-9+/]{43}=?", value, re.ASCII):
        return None
    encoded = value.removeprefix("SHA256:")
    try:
        digest = base64.b64decode(encoded + "=" * (-len(encoded) % 4), validate=True)
    except ValueError:
        return None
    return digest if len(digest) == 32 else None


def _verified_fingerprint(stderr: bytes) -> str | None:
    candidate = None
    # Select the initial verified exchange. Later banners or remote diagnostics
    # cannot replace that value by printing debug-shaped text.
    for line in stderr.split(b"\n"):
        if candidate is None:
            match = _HOST_KEY.fullmatch(line.rstrip(b"\r"))
            if match:
                candidate = match.groups()
            continue
        match = _KNOWN_KEY.fullmatch(line.rstrip(b"\r"))
        if match and _KEY_NAMES.get(candidate[0]) == match[1]:
            value = candidate[1].decode("ascii")
            return value if _fingerprint_digest(value) else None
        if line.startswith((b"Authenticated to ", b"debug1: Authentication succeeded", b"debug1: Server host key:")):
            return None
    return None


def _unique_object(items):
    result = {}
    for key, value in items:
        if key in result:
            raise ValueError("Duplicate JSON member.")
        result[key] = value
    return result


def _invalid_number(_):
    raise ValueError("Invalid JSON number.")


def _finite_float(text):
    value = float(text)
    if not math.isfinite(value):
        raise ValueError("Invalid JSON number.")
    return value


def _response(stdout: bytes) -> dict:
    prefix = FRAME_PREFIX.encode("ascii")
    if (not stdout.startswith(prefix) or not stdout.endswith(b"\n")
            or stdout.count(b"\n") != 1 or b"\r" in stdout):
        raise SshTransportError("invalid_response")
    try:
        text = stdout[len(prefix):-1].decode("utf-8")
        value = json.loads(text, object_pairs_hook=_unique_object,
                           parse_constant=_invalid_number, parse_float=_finite_float)
    except (UnicodeError, ValueError, RecursionError):
        raise SshTransportError("invalid_response") from None
    if not isinstance(value, dict) or "_transport" in value:
        raise SshTransportError("invalid_response")
    return value


async def _read(stream, *, stdout: bool) -> bytes:
    kept = bytearray()
    while chunk := await asyncio.to_thread(stream.read, 4096):
        if stdout and len(kept) + len(chunk) > MAX_STDOUT_BYTES:
            raise SshTransportError("output_limit")
        maximum = MAX_STDOUT_BYTES if stdout else MAX_STDERR_BYTES
        kept.extend(chunk[:max(0, maximum - len(kept))])
    return bytes(kept)


async def _write(stream, payload: bytes) -> None:
    def write():
        try:
            view = memoryview(payload)
            while view:
                count = stream.write(view)
                if not count:
                    raise BrokenPipeError
                view = view[count:]
        finally:
            stream.close()
    await _joined_writer(write)


async def _joined_writer(write):
    """Keep the original entered pipe writer until its worker has stopped."""
    pending = asyncio.create_task(asyncio.to_thread(write))
    try:
        await asyncio.shield(pending)
    except asyncio.CancelledError:
        while not pending.done():
            try:
                await asyncio.shield(pending)
            except asyncio.CancelledError:
                continue
            except Exception:
                break
        if not pending.cancelled():
            pending.exception()
        raise


async def _exit(process) -> int:
    # No background wait/reaper is installed. Until this final observation, the
    # owned leader remains an unreaped process-group anchor, even if it exits
    # while a descendant keeps a pipe open. Cleanup signals before any reap.
    while True:
        try:
            return process.wait(timeout=0)
        except subprocess.TimeoutExpired:
            await asyncio.sleep(0.01)


async def _cleanup(process, tasks) -> None:
    unavailable = False
    if process.returncode is None:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        except OSError:
            unavailable = True
    try:
        await asyncio.to_thread(process.wait, timeout=FORCE_REAP_TIMEOUT)
    except (subprocess.TimeoutExpired, OSError):
        unavailable = True
    finally:
        # Close the entered writer's pipe before asking its coroutine to join
        # the original thread. The source and selected profile stay retained
        # until that thread observes EOF/error and finishes its readback.
        try:
            process.stdin.close()
        except (OSError, ValueError):
            unavailable = True
        for task in tasks:
            task.cancel()
        await asyncio.gather(*tasks, return_exceptions=True)
        for stream in (process.stdin, process.stdout, process.stderr):
            stream.close()
    if unavailable:
        raise SshTransportError("cleanup_unavailable") from None


async def _execute_stream(argv, body_writer, timeout, *, require_current=None):
    _transfer_current(require_current)
    try:
        # Synchronous creation avoids an await/cancellation gap that could
        # leave a newly spawned process without an owner. No shell is used.
        process = subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.PIPE, env=_environment(), start_new_session=True, bufsize=0)
    except (OSError, ValueError):
        raise SshTransportError("connection_unavailable") from None
    tasks = [asyncio.create_task(_read(process.stdout, stdout=True)),
             asyncio.create_task(_read(process.stderr, stdout=False)),
             asyncio.create_task(body_writer(process.stdin))]
    async def observe():
        stdout, stderr, _ = await asyncio.gather(*tasks)
        return stdout, stderr, await _exit(process)
    observer = asyncio.create_task(observe())
    try:
        deadline = asyncio.get_running_loop().time() + timeout
        while not observer.done():
            # The original owner may depend on this event loop and calling
            # task. Workers only read/write bytes; they never invoke it.
            _transfer_current(require_current)
            remaining = _remaining(deadline)
            await asyncio.wait((observer,), timeout=min(remaining, 0.05) if require_current is not None else remaining)
        _transfer_current(require_current)
        return observer.result()
    except (OSError, ValueError):
        raise SshTransportError("connection_unavailable") from None
    finally:
        # Shield cleanup from the request's cancellation. A second cancel
        # still waits for this bounded local cleanup before propagating.
        cleanup = asyncio.create_task(_cleanup(process, [*tasks, observer]))
        cancelled = False
        while not cleanup.done():
            try:
                await asyncio.shield(cleanup)
            except asyncio.CancelledError:
                cancelled = True
        cleanup.result()
        if cancelled:
            raise asyncio.CancelledError


async def _execute(argv, body, timeout):
    return await _execute_stream(argv, lambda stream: _write(stream, body), timeout)


def _transfer_current(callback):
    if callback is None:
        return
    if not callable(callback):
        raise SshTransportError("invalid_request")
    result = callback()
    if inspect.isawaitable(result):
        if inspect.iscoroutine(result):
            result.close()
        raise SshTransportError("invalid_request")
    if result is False:
        raise SshTransportError("invalid_request")


def _transfer_identity(info):
    return (info.st_dev, info.st_ino, info.st_mode, info.st_uid, info.st_nlink,
        info.st_size, info.st_mtime_ns, info.st_ctime_ns)


def _transfer_source(artifact, require_current=None):
    from .runtime_candidates import ArtifactFile
    if (type(artifact) is not ArtifactFile or type(artifact.size) is not int
            or not 0 < artifact.size <= MAX_TRANSFER_FILE_BYTES
            or not isinstance(artifact.sha256, str) or not re.fullmatch(r"[a-f0-9]{64}", artifact.sha256)):
        raise SshTransportError("invalid_request")
    path = artifact.path
    if (type(path) is not Path and not isinstance(path, Path)) or not path.is_absolute() or ".." in path.parts:
        raise SshTransportError("invalid_request")
    if any(item.is_symlink() for item in (path, *path.parents)):
        raise SshTransportError("invalid_request")
    descriptor, handle = None, None
    try:
        before = path.lstat()
        descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        handle = os.fdopen(descriptor, "rb")
        descriptor = None
        observed = os.fstat(handle.fileno())
        if (not stat.S_ISREG(observed.st_mode) or observed.st_uid != os.getuid()
                or observed.st_nlink != 1 or observed.st_size != artifact.size
                or _transfer_identity(before) != _transfer_identity(observed)):
            handle.close()
            raise SshTransportError("invalid_request")
        identity = _transfer_identity(observed)
        hashed = hashlib.sha256()
        while chunk := handle.read(TRANSFER_CHUNK_BYTES):
            _transfer_current(require_current)
            hashed.update(chunk)
        if (hashed.hexdigest() != artifact.sha256
                or _transfer_identity(os.fstat(handle.fileno())) != identity
                or _transfer_identity(path.lstat()) != identity):
            handle.close()
            raise SshTransportError("invalid_request")
        handle.seek(0)
        return handle, identity
    except BaseException as error:
        if handle is not None:
            handle.close()
        if descriptor is not None:
            os.close(descriptor)
        if isinstance(error, OSError):
            raise SshTransportError("invalid_request") from None
        raise


async def _observe_transfer_source(artifact, require_current):
    pending = asyncio.create_task(asyncio.to_thread(_transfer_source, artifact))
    try:
        while not pending.done():
            _transfer_current(require_current)
            await asyncio.wait((pending,), timeout=0.05)
        _transfer_current(require_current)
        return pending.result()
    except BaseException:
        while not pending.done():
            try:
                await asyncio.shield(pending)
            except asyncio.CancelledError:
                continue
            except Exception:
                break
        if not pending.cancelled() and pending.exception() is None:
            pending.result()[0].close()
        raise


async def _write_transfer(stream, header, handle, artifact, identity, require_current):
    def write():
        def send(data):
            view = memoryview(data)
            while view:
                count = stream.write(view)
                if not count:
                    raise BrokenPipeError
                view = view[count:]
        try:
            send(struct.pack("!I", len(header)))
            send(header)
            hashed, remaining = hashlib.sha256(), artifact.size
            while remaining:
                chunk = handle.read(min(TRANSFER_CHUNK_BYTES, remaining))
                if not chunk:
                    raise SshTransportError("invalid_request")
                remaining -= len(chunk)
                hashed.update(chunk)
                send(chunk)
            if (handle.read(1) or hashed.hexdigest() != artifact.sha256
                    or _transfer_identity(os.fstat(handle.fileno())) != identity
                    or _transfer_identity(artifact.path.lstat()) != identity):
                raise SshTransportError("invalid_request")
        finally:
            stream.close()
    # Cancellation cannot end a thread's write. Retain the one entered writer
    # until the SSH cleanup closes its pipe and the source observation settles.
    await _joined_writer(write)


def _remaining(deadline):
    value = deadline - asyncio.get_running_loop().time()
    if value <= 0:
        raise SshTransportError("timeout")
    return value


def _operation_deadline(timeout, caller_deadline):
    deadline = asyncio.get_running_loop().time() + timeout
    if caller_deadline is not None:
        try:
            if (isinstance(caller_deadline, bool) or not isinstance(caller_deadline, (int, float))
                    or not math.isfinite(caller_deadline)):
                raise SshTransportError("invalid_request")
        except OverflowError:
            raise SshTransportError("invalid_request") from None
        deadline = min(deadline, caller_deadline)
    return deadline


def _default_known_hosts():
    # OpenSSH obtains the local account home from passwd, not an arbitrary HOME
    # override. These are the default files for our config-free SSH invocation.
    home = Path(pwd.getpwuid(os.getuid()).pw_dir)
    return (home / ".ssh/known_hosts", home / ".ssh/known_hosts2",
            Path("/etc/ssh/ssh_known_hosts"), Path("/etc/ssh/ssh_known_hosts2"))


def _key_record(line):
    parts = line.split()
    marker = b""
    if not parts or parts[0].startswith(b"#"):
        return None
    if parts[0].startswith(b"@"):
        marker, parts = parts[0], parts[1:]
    if len(parts) < 3 or marker not in {b"", b"@revoked", b"@cert-authority"}:
        raise SshTransportError("host_key_unavailable")
    key_type, encoded = parts[1:3]
    try:
        blob = base64.b64decode(encoded, validate=True)
    except ValueError:
        raise SshTransportError("host_key_unavailable") from None
    size = int.from_bytes(blob[:4], "big")
    if len(blob) < 4 or not size or size > 80 or blob[4:4+size] != key_type or len(blob) <= 4+size:
        raise SshTransportError("host_key_unavailable")
    return marker, key_type, encoded, hashlib.sha256(blob).digest()


class SshTransport:
    """Run machine-owned Python RPC through SSH, never a user-supplied command.

    executable/argv_prefix/known_hosts_paths are trusted constructor-only seams.
    Host-selected profiles may compose them; endpoint and payload cannot change argv.
    """

    def __init__(self, *, executable: str = "/usr/bin/ssh", argv_prefix: Sequence[str] = (), known_hosts_paths=None):
        self._executable = executable
        self._argv_prefix = tuple(argv_prefix)
        self._known_hosts_paths = tuple(known_hosts_paths) if known_hosts_paths is not None else None

    async def _pin(self, target, expected, deadline):
        lookup = target["host"] if target["ssh_port"] == 22 else f"[{target['host']}]:{target['ssh_port']}"
        matches, revoked = set(), set()
        for path in self._known_hosts_paths if self._known_hosts_paths is not None else _default_known_hosts():
            path = Path(path)
            try:
                if not path.exists():
                    continue
                if not path.is_file():
                    raise SshTransportError("host_key_unavailable")
                stdout, _, status = await _execute(["/usr/bin/ssh-keygen", "-F", lookup, "-f", str(path)], b"", _remaining(deadline))
            except SshTransportError:
                raise SshTransportError("host_key_unavailable") from None
            except OSError:
                raise SshTransportError("host_key_unavailable") from None
            if status not in {0, 1}:
                raise SshTransportError("host_key_unavailable")
            if status == 1 and stdout:
                raise SshTransportError("host_key_unavailable")
            for line in stdout.split(b"\n"):
                record = _key_record(line)
                if record is None:
                    continue
                marker, key_type, encoded, digest = record
                row = lookup.encode("ascii") + b" " + key_type + b" " + encoded + b"\n"
                if marker == b"@revoked":
                    revoked.add(b"@revoked " + row)
                    if digest == expected:
                        raise SshTransportError("host_key_unavailable")
                elif marker == b"" and key_type in _KEY_NAMES and digest == expected:
                    matches.add(row)
        if not matches:
            raise SshTransportError("host_key_unavailable")
        return b"".join(sorted(revoked | matches))

    async def request(self, endpoint: Mapping, program: str, payload: dict, timeout: float = 30,
                      *, expected_fingerprint: str | None = None, deadline: float | None = None) -> dict:
        target = validate_endpoint(endpoint)
        if (not isinstance(program, str) or not program or "\0" in program or not isinstance(payload, dict)
                or isinstance(timeout, bool) or not isinstance(timeout, (int, float))
                or timeout <= 0):
            raise SshTransportError("invalid_request")
        try:
            if not math.isfinite(timeout):
                raise SshTransportError("invalid_request")
        except OverflowError:
            raise SshTransportError("invalid_request") from None
        expected = _fingerprint_digest(expected_fingerprint) if expected_fingerprint is not None else None
        if expected_fingerprint is not None and expected is None:
            raise SshTransportError("invalid_request")
        # A composing transport can shorten this budget with its absolute
        # deadline. Encoding and pin lookup must not grant renewed time.
        deadline = _operation_deadline(timeout, deadline)
        _remaining(deadline)
        try:
            body = json.dumps(payload, ensure_ascii=True, allow_nan=False, separators=(",", ":")).encode("ascii") + b"\n"
        except (TypeError, ValueError, RecursionError):
            raise SshTransportError("invalid_request") from None
        _remaining(deadline)
        pinned = await self._pin(target, expected, deadline) if expected is not None else None
        argv = [self._executable, *self._argv_prefix, "-F", "/dev/null", "-v", "-T",
                "-o", "StrictHostKeyChecking=yes", "-o", "BatchMode=yes", "-o", "UpdateHostKeys=no",
                "-o", "ForwardAgent=no", "-o", "PermitLocalCommand=no", "-o", "ClearAllForwardings=yes",
                "-o", "ControlMaster=no", "-o", "ControlPath=none", "-o", "ConnectTimeout=10",
                "-o", "FingerprintHash=sha256", "-p", str(target["ssh_port"]), "-l", target["user"],
                "--", target["host"], "python3 -I -c " + shlex.quote(program)]
        if pinned is None:
            stdout, stderr, status = await _execute(argv, body, _remaining(deadline))
        else:
            try:
                with tempfile.TemporaryDirectory(prefix="mobkit-ssh-pin-") as temporary:
                    path = Path(temporary) / "known_hosts"
                    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
                    with os.fdopen(descriptor, "wb") as stream:
                        stream.write(pinned)
                    # These options are command-line-owned. Ambient trust cannot
                    # authorize a replacement server key before the program runs.
                    argv[1+len(self._argv_prefix):1+len(self._argv_prefix)] = [
                        "-o", "UserKnownHostsFile=" + str(path), "-o", "GlobalKnownHostsFile=/dev/null"]
                    stdout, stderr, status = await _execute(argv, body, _remaining(deadline))
            except OSError:
                raise SshTransportError("connection_unavailable") from None
        if status:
            raise SshTransportError("connection_unavailable")
        result = _response(stdout)
        fingerprint = _verified_fingerprint(stderr)
        if expected is not None and _fingerprint_digest(fingerprint) != expected:
            raise SshTransportError("invalid_response")
        if fingerprint:
            result["_transport"] = {"host_key_fingerprint": fingerprint}
        return result

    async def transfer_file(self, endpoint, program, payload, artifact, timeout=30,
                            *, expected_fingerprint=None, deadline=None, require_current=None):
        """Stream one exact owner-selected file to a fixed trusted receiver.

        File framing is transport only. The receiver owns destination custody;
        this method never establishes installation or native setup authority.
        """
        target = validate_endpoint(endpoint)
        if (not isinstance(program, str) or not program or "\0" in program
                or type(payload) is not dict or "transfer_file" in payload
                or isinstance(timeout, bool) or not isinstance(timeout, (int, float))
                or timeout <= 0):
            raise SshTransportError("invalid_request")
        try:
            if not math.isfinite(timeout):
                raise SshTransportError("invalid_request")
        except OverflowError:
            raise SshTransportError("invalid_request") from None
        expected = _fingerprint_digest(expected_fingerprint)
        # Artifact writes require the previously observed destination key.
        if expected is None:
            raise SshTransportError("invalid_request")
        deadline = _operation_deadline(timeout, deadline)
        _transfer_current(require_current)
        handle, identity = await _observe_transfer_source(artifact, require_current)
        try:
            try:
                header = json.dumps({**payload, "transfer_file": {
                    "size": artifact.size, "sha256": artifact.sha256}}, ensure_ascii=True,
                    allow_nan=False, separators=(",", ":")).encode("ascii")
            except (TypeError, ValueError, RecursionError):
                raise SshTransportError("invalid_request") from None
            if len(header) > MAX_TRANSFER_HEADER_BYTES:
                raise SshTransportError("invalid_request")
            pinned = await self._pin(target, expected, deadline)
            _transfer_current(require_current)
            if (_transfer_identity(os.fstat(handle.fileno())) != identity
                    or _transfer_identity(artifact.path.lstat()) != identity):
                raise SshTransportError("invalid_request")
            argv = [self._executable, *self._argv_prefix, "-F", "/dev/null", "-v", "-T",
                "-o", "StrictHostKeyChecking=yes", "-o", "BatchMode=yes", "-o", "UpdateHostKeys=no",
                "-o", "ForwardAgent=no", "-o", "PermitLocalCommand=no", "-o", "ClearAllForwardings=yes",
                "-o", "ControlMaster=no", "-o", "ControlPath=none", "-o", "ConnectTimeout=10",
                "-o", "FingerprintHash=sha256", "-p", str(target["ssh_port"]), "-l", target["user"],
                "--", target["host"], "python3 -I -c " + shlex.quote(program)]
            try:
                with tempfile.TemporaryDirectory(prefix="forge-ssh-artifact-pin-") as temporary:
                    path = Path(temporary) / "known_hosts"
                    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
                    with os.fdopen(descriptor, "wb") as stream:
                        stream.write(pinned)
                    argv[1+len(self._argv_prefix):1+len(self._argv_prefix)] = [
                        "-o", "UserKnownHostsFile=" + str(path), "-o", "GlobalKnownHostsFile=/dev/null"]
                    _transfer_current(require_current)
                    stdout, stderr, status = await _execute_stream(argv,
                        lambda stream: _write_transfer(stream, header, handle, artifact, identity, require_current),
                        _remaining(deadline), require_current=require_current)
            except OSError:
                raise SshTransportError("connection_unavailable") from None
            _remaining(deadline)
            _transfer_current(require_current)
            if (_transfer_identity(os.fstat(handle.fileno())) != identity
                    or _transfer_identity(artifact.path.lstat()) != identity):
                raise SshTransportError("invalid_request")
            if status:
                raise SshTransportError("connection_unavailable")
            result = _response(stdout)
            fingerprint = _verified_fingerprint(stderr)
            if _fingerprint_digest(fingerprint) != expected:
                raise SshTransportError("invalid_response")
            result["_transport"] = {"host_key_fingerprint": fingerprint}
            return result
        except OSError:
            raise SshTransportError("invalid_request") from None
        finally:
            handle.close()
