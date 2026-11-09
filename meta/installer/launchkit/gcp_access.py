"""Host-owned SSH key preparation and authenticated fresh-VM host-key bootstrap.

No application settings enter this service. The unencrypted SSH identity stays in
one private operation directory; only its public key enters VM metadata. Guest
attributes establish initial SSH trust, never application health or authorization.
"""
from __future__ import annotations

import asyncio
import base64
from copy import deepcopy
import hashlib
import json
import os
from pathlib import Path
import stat
import struct
import uuid

from .gcp_preflight import ProbeUnavailable
from .projects import DraftConflict
from .remote_deployment import private_file, write_private
from .ssh_transport import _execute, SshTransportError

MAX_KEY = 16384


def _saved(path):
    try:
        value = json.loads(private_file(path, MAX_KEY))
        if not isinstance(value, dict):
            raise ValueError()
        return value
    except (ValueError, UnicodeError, RecursionError):
        raise DraftConflict("The saved cloud SSH evidence is malformed. Existing files were preserved.") from None


def operation_uuid(value):
    if not isinstance(value, str):
        raise ValueError("Choose an exact cloud setup operation.")
    try:
        if str(uuid.UUID(value)) != value:
            raise ValueError()
    except ValueError:
        raise ValueError("Choose an exact cloud setup operation.") from None
    return value


def checked_ed25519(value):
    if not isinstance(value, str) or not 1 <= len(value) <= 512:
        raise DraftConflict("The computer's SSH public key is missing or malformed.")
    try:
        raw = base64.b64decode(value, validate=True)
        kind = b"ssh-ed25519"
        if (base64.b64encode(raw).decode() != value or len(raw) != 4 + len(kind) + 4 + 32
                or raw[:4] != struct.pack(">I", len(kind)) or raw[4:4 + len(kind)] != kind
                or raw[4 + len(kind):8 + len(kind)] != struct.pack(">I", 32)):
            raise ValueError()
    except (ValueError, TypeError):
        raise DraftConflict("The computer's SSH public key is missing or malformed.") from None
    return value


def _scope(binding, destination):
    return hashlib.sha256(json.dumps([binding, destination], sort_keys=True, separators=(",", ":"),
        allow_nan=False).encode()).hexdigest()


class GcpAccessService:
    def __init__(self, root, preflight, provisioning=None, *, key_runner=None):
        self.root = Path(root) / "gcp-access"
        self.preflight, self.provisioning = preflight, provisioning
        self.key_runner = key_runner or _execute
        self.locks = {}

    def directory(self, operation_id):
        return self.root / operation_uuid(operation_id)

    def lock(self, operation_id):
        return self.locks.setdefault(operation_uuid(operation_id), asyncio.Lock())

    def _directory(self, operation_id):
        path = self.directory(operation_id)
        if any(item.is_symlink() for item in (path, *path.parents)):
            raise DraftConflict("The private cloud SSH setup contains a link.")
        self.root.mkdir(parents=True, exist_ok=True, mode=0o700)
        for folder in (self.root, path):
            folder.mkdir(exist_ok=True, mode=0o700)
            info = folder.lstat()
            if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077:
                raise DraftConflict("The private cloud SSH setup is not an owned private directory.")
        return path

    def _record(self, operation_id):
        path = self.directory(operation_id)
        value = _saved(path / "identity.json")
        if value.get("operation_id") != operation_id or value.get("status") != "ready":
            raise DraftConflict("Cloud SSH key preparation is incomplete. Preserve this operation's files and review them before continuing.")
        key = private_file(path / "identity", MAX_KEY)
        public = private_file(path / "identity.pub", MAX_KEY).decode("ascii").strip()
        if (hashlib.sha256(key).hexdigest() != value.get("private_key_sha256")
                or public != value.get("public_key") or not key.startswith(b"-----BEGIN OPENSSH PRIVATE KEY-----\n")
                or not key.rstrip().endswith(b"-----END OPENSSH PRIVATE KEY-----")):
            raise DraftConflict("The cloud SSH identity changed. Its existing files were preserved.")
        pieces = public.split(" ")
        if len(pieces) != 3 or pieces[0] != "ssh-ed25519" or pieces[2] != "mobkit-launch:" + operation_id:
            raise DraftConflict("The cloud SSH identity has an unexpected public key.")
        checked_ed25519(pieces[1])
        return path, value

    async def prepare_access(self, binding, destination, operation_id):
        async with self.lock(operation_id):
            path = self._directory(operation_id)
            expected = _scope(binding, destination)
            if (path / "identity.json").exists():
                _, value = self._record(operation_id)
                if value.get("scope_sha256") != expected:
                    raise DraftConflict("This SSH identity belongs to a different cloud package or destination.")
                return {"user": "mobkit", "public_key": value["public_key"]}
            if any((path / name).exists() or (path / name).is_symlink() for name in ("identity", "identity.pub")):
                raise DraftConflict("An incomplete SSH identity already exists. It will not be overwritten or regenerated.")
            intent = {"operation_id": operation_id, "scope_sha256": expected, "status": "key-generation-pending"}
            write_private(path / "identity.json", intent)
            try:
                _, _, code = await self.key_runner(["/usr/bin/ssh-keygen", "-q", "-t", "ed25519", "-N", "",
                    "-C", "mobkit-launch:" + operation_id, "-f", str(path / "identity")], b"", 15)
                if code != 0:
                    raise DraftConflict("The private SSH identity could not be prepared. Existing operation files were preserved.")
                # OpenSSH writes a public file with ordinary mode; make it private
                # before applying the same bounded source-custody reader.
                pub = path / "identity.pub"
                descriptor = os.open(pub, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
                try:
                    info = os.fstat(descriptor)
                    if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_nlink != 1:
                        raise DraftConflict("The cloud SSH public key is not an owned ordinary file.")
                    os.fchmod(descriptor, 0o600)
                finally:
                    os.close(descriptor)
                private = private_file(path / "identity", MAX_KEY)
                public = private_file(pub, MAX_KEY).decode("ascii").strip()
                value = {**intent, "status": "ready", "user": "mobkit", "public_key": public,
                    "private_key_sha256": hashlib.sha256(private).hexdigest()}
                write_private(path / "identity.json", value)
                self._record(operation_id)
                return {"user": "mobkit", "public_key": public}
            except (OSError, UnicodeError, SshTransportError):
                raise DraftConflict("The private SSH identity could not be verified. Existing operation files were preserved.") from None

    async def identity_file(self, endpoint):
        async with self.lock(endpoint["operation_id"]):
            path, _ = self._record(endpoint["operation_id"])
            return str(path / "identity")

    async def _verify(self, endpoint):
        if self.provisioning is None:
            raise DraftConflict("The owned cloud operation cannot be verified.")
        value = await self.provisioning.verify(endpoint)
        if value != endpoint:
            raise DraftConflict("The cloud computer identity changed. No SSH request was admitted.")
        return value

    async def host_keys(self, endpoint):
        async with self.lock(endpoint["operation_id"]):
            path, _ = self._record(endpoint["operation_id"])
            await self._verify(endpoint)
            cached = path / "host-key.json"
            if cached.exists():
                record = _saved(cached)
                if record.get("endpoint") != endpoint or record.get("key_type") != "ssh-ed25519":
                    raise DraftConflict("The pinned cloud host key belongs to another computer.")
                checked_ed25519(record.get("key"))
                return {"key_type": record["key_type"], "key": record["key"]}
            checks = []
            try:
                _, observed, _ = await self.preflight._accounts(endpoint["account"])
                if not observed or any(row.get("status") != "checked" for row in observed):
                    raise ProbeUnavailable()
                token = await self.preflight._token(endpoint["account"], endpoint["project"], checks)
                if not token:
                    raise ProbeUnavailable()
                try:
                    url = ("https://compute.googleapis.com/compute/v1/projects/" + endpoint["project"] +
                        "/zones/" + endpoint["zone"] + "/instances/" + endpoint["instance"] +
                        "/getGuestAttributes?queryPath=hostkeys%2F")
                    value = await self.preflight._rest("GET", url, None, token)
                finally:
                    token = None
                if not isinstance(value, dict) or value.get("queryPath") != "hostkeys/" or not isinstance(value.get("queryValue"), dict):
                    raise ProbeUnavailable()
                items = value["queryValue"].get("items")
                if (not isinstance(items, list) or not 1 <= len(items) <= 16
                        or any(not isinstance(row, dict) or row.get("namespace") != "hostkeys"
                            or not isinstance(row.get("key"), str) or not 1 <= len(row["key"]) <= 128
                            or not isinstance(row.get("value"), str) or not 1 <= len(row["value"]) <= 16384 for row in items)):
                    raise ProbeUnavailable()
                keys = [row.get("value") for row in items if row.get("namespace") == "hostkeys" and row.get("key") == "ssh-ed25519"]
                if len(keys) != 1:
                    raise ProbeUnavailable()
                key = checked_ed25519(keys[0])
                await self._verify(endpoint)
                write_private(cached, {"endpoint": deepcopy(endpoint), "key_type": "ssh-ed25519", "key": key,
                    "origin": "authenticated-compute-guest-attributes", "application_health_claimed": False})
                return {"key_type": "ssh-ed25519", "key": key}
            except (ProbeUnavailable, ValueError, KeyError, TypeError):
                raise DraftConflict("The new computer's SSH identity is not available yet. Recheck this owned computer; no unverified connection was used.") from None
