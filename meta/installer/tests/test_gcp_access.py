"""Portable cloud-free custody checks for fresh metadata-SSH access."""
import asyncio
import base64
from copy import deepcopy
import hashlib
import json
import os
from pathlib import Path
import struct
import uuid

import pytest

from launchkit.gcp_access import GcpAccessService, checked_ed25519
from launchkit.gcp_preflight import ProbeUnavailable
from launchkit.projects import DraftConflict
from launchkit.remote_deployment import write_private
from launchkit.ssh_transport import SshTransportError

TOKEN = "transient-cloud-token-CANARY"
PRIVATE = b"-----BEGIN OPENSSH PRIVATE KEY-----\nprivate-key-CANARY\n-----END OPENSSH PRIVATE KEY-----\n"


def public_blob(seed=11):
    kind = b"ssh-ed25519"
    return base64.b64encode(struct.pack(">I", len(kind)) + kind + struct.pack(">I", 32) + bytes([seed]) * 32).decode()


def endpoint(operation):
    return {"transport": "gcp-iap", "operation_id": operation, "account": "operator@example.com",
            "project": "mobkit-cloud-project", "zone": "europe-west1-b", "instance": "atlas-operations",
            "instance_id": "123456789", "ownership_token": uuid.UUID(operation).hex,
            "host": "compute.123456789", "user": "mobkit", "ssh_port": 22}


class Keys:
    def __init__(self):
        self.calls = []
        self.code = 0
        self.error = None
        self.public_mode = 0o644
        self.hold = None
        self.entered = asyncio.Event()

    async def __call__(self, argv, payload, timeout):
        self.calls.append((list(argv), payload, timeout))
        assert argv[:7] == ["/usr/bin/ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-C"]
        assert payload == b"" and timeout == 15
        path = Path(argv[-1])
        path.write_bytes(PRIVATE)
        path.chmod(0o600)
        pub = path.with_name(path.name + ".pub")
        pub.write_text("ssh-ed25519 " + public_blob() + " " + argv[7] + "\n")
        pub.chmod(self.public_mode)
        self.entered.set()
        if self.hold is not None: await self.hold.wait()
        if self.error: raise self.error
        return b"", b"private-keyrunner-error-CANARY", self.code


class Preflight:
    def __init__(self):
        self.calls = []
        self.accounts_checked = True
        self.token = TOKEN
        self.result = {"queryPath": "hostkeys/", "queryValue": {"items": [
            {"namespace": "hostkeys", "key": "ssh-ed25519", "value": public_blob(19)},
            {"namespace": "hostkeys", "key": "ssh-rsa", "value": "ignored-other-algorithm"}]}}
        self.error = None

    async def _accounts(self, account):
        self.calls.append(("accounts", account))
        return [{"account": account}], [{"status": "checked" if self.accounts_checked else "unknown"}], None

    async def _token(self, account, project, checks):
        self.calls.append(("token", account, project))
        return self.token

    async def _rest(self, method, url, body, token):
        self.calls.append(("rest", method, url, body))
        assert token == TOKEN
        if self.error: raise self.error
        return deepcopy(self.result)


class Provisioning:
    def __init__(self):
        self.calls = []
        self.changes = None
        self.failure_at = None

    async def verify(self, value):
        self.calls.append(deepcopy(value))
        if len(self.calls) == self.failure_at:
            raise DraftConflict("The original cloud resource identity changed.")
        return {**value, **(self.changes or {})}


@pytest.fixture
def access(tmp_path):
    keys, preflight, provisioning = Keys(), Preflight(), Provisioning()
    service = GcpAccessService(tmp_path, preflight, provisioning, key_runner=keys)
    operation = str(uuid.uuid4())
    binding = {"guide_id": str(uuid.uuid4()), "guide_version": 4, "build_id": str(uuid.uuid4()),
               "source_digest": "a" * 64, "package_sha256": "b" * 64}
    destination = {"project": "mobkit-cloud-project", "zone": "europe-west1-b", "instance": "atlas-operations"}
    return service, keys, preflight, provisioning, operation, binding, destination


async def prepared(access):
    service, _, _, _, operation, binding, destination = access
    return await service.prepare_access(binding, destination, operation)


async def test_prepare_once_is_exact_scoped_private_and_returns_only_public_data(access):
    service, keys, _, _, operation, binding, destination = access
    result = await prepared(access)
    assert result == {"user": "mobkit", "public_key": "ssh-ed25519 " + public_blob() + " mobkit-launch:" + operation}
    assert await prepared(access) == result and len(keys.calls) == 1
    assert "private-key-CANARY" not in json.dumps(result)
    assert "private-keyrunner-error-CANARY" not in json.dumps(result)
    path = service.directory(operation)
    assert all(item.stat().st_mode & 0o077 == 0 for item in (service.root, path, path / "identity", path / "identity.pub", path / "identity.json"))
    assert await service.identity_file(endpoint(operation)) == str(path / "identity")
    record = json.loads((path / "identity.json").read_text())
    assert record["private_key_sha256"] == hashlib.sha256(PRIVATE).hexdigest()
    assert "private-key-CANARY" not in json.dumps(record)


async def test_concurrent_exact_preparation_has_one_key_generation(access):
    _, keys, _, _, _, _, _ = access
    keys.hold = asyncio.Event()
    first = asyncio.create_task(prepared(access))
    await keys.entered.wait()
    second = asyncio.create_task(prepared(access))
    keys.hold.set()
    assert await first == await second and len(keys.calls) == 1


@pytest.mark.parametrize("field", ["binding", "destination"])
async def test_scope_changes_cannot_reuse_or_overwrite_identity(access, field):
    service, keys, _, _, operation, binding, destination = access
    await prepared(access)
    before = {p.name: p.read_bytes() for p in service.directory(operation).iterdir()}
    binding, destination = deepcopy(binding), deepcopy(destination)
    if field == "binding": binding["package_sha256"] = "c" * 64
    else: destination["zone"] = "europe-west1-c"
    with pytest.raises(DraftConflict, match="different cloud package or destination"):
        await service.prepare_access(binding, destination, operation)
    assert before == {p.name: p.read_bytes() for p in service.directory(operation).iterdir()}
    assert len(keys.calls) == 1


@pytest.mark.parametrize("error", [None, OSError("private-error-CANARY"), SshTransportError("connection_unavailable")])
async def test_failed_generation_preserves_partial_files_and_never_regenerates(access, error):
    service, keys, _, _, operation, _, _ = access
    keys.code, keys.error = 1, error
    with pytest.raises(DraftConflict) as caught:
        await prepared(access)
    assert "private-error-CANARY" not in str(caught.value)
    before = {p.name: p.read_bytes() for p in service.directory(operation).iterdir()}
    keys.error, keys.code = None, 0
    with pytest.raises(DraftConflict, match="incomplete"):
        await prepared(access)
    assert len(keys.calls) == 1
    assert before == {p.name: p.read_bytes() for p in service.directory(operation).iterdir()}


async def test_cancelled_generation_retains_pending_custody(access):
    service, keys, _, _, operation, _, _ = access
    keys.hold = asyncio.Event()
    task = asyncio.create_task(prepared(access))
    await keys.entered.wait()
    task.cancel()
    with pytest.raises(asyncio.CancelledError): await task
    assert json.loads((service.directory(operation) / "identity.json").read_text())["status"] == "key-generation-pending"
    with pytest.raises(DraftConflict, match="incomplete"): await prepared(access)
    assert len(keys.calls) == 1


@pytest.mark.parametrize("problem", ["key-hash", "public-change", "key-mode", "record-mode", "key-link", "public-hardlink", "operation-link", "directory-mode"])
async def test_modified_or_nonprivate_files_block_reuse_without_overwrite(access, tmp_path, problem):
    service, keys, _, _, operation, _, _ = access
    await prepared(access)
    path = service.directory(operation)
    if problem == "key-hash": (path / "identity").write_bytes(PRIVATE.replace(b"CANARY", b"CHANGED"))
    elif problem == "public-change": (path / "identity.pub").write_text("ssh-ed25519 " + public_blob(12) + " mobkit-launch:" + operation)
    elif problem == "key-mode": (path / "identity").chmod(0o644)
    elif problem == "record-mode": (path / "identity.json").chmod(0o644)
    elif problem == "key-link":
        (path / "identity").unlink(); (path / "identity").symlink_to(tmp_path / "missing-key")
    elif problem == "public-hardlink": os.link(path / "identity.pub", tmp_path / "linked-public")
    elif problem == "operation-link":
        moved = tmp_path / "moved"; path.rename(moved); path.symlink_to(moved, target_is_directory=True)
    else: path.chmod(0o755)
    with pytest.raises(DraftConflict): await prepared(access)
    assert len(keys.calls) == 1


@pytest.mark.parametrize("value", [None, 7, "", "%%%%", public_blob() + "=", public_blob() + "\n",
    base64.b64encode(b"not-an-ssh-key").decode(),
    base64.b64encode(b"\x00\x00\x00\x0bssh-ed25519\x00\x00\x00\x21" + bytes(32)).decode()])
def test_ed25519_rejects_noncanonical_or_wrong_wire_key(value):
    with pytest.raises(DraftConflict): checked_ed25519(value)


async def test_host_key_query_brackets_owned_numeric_endpoint_and_cache_is_immutable(access):
    service, _, preflight, provisioning, operation, _, _ = access
    await prepared(access)
    selected = endpoint(operation)
    result = await service.host_keys(selected)
    assert result == {"key_type": "ssh-ed25519", "key": public_blob(19)}
    assert provisioning.calls == [selected, selected]
    rest = [call for call in preflight.calls if call[0] == "rest"]
    assert rest == [("rest", "GET", "https://compute.googleapis.com/compute/v1/projects/mobkit-cloud-project/zones/europe-west1-b/instances/atlas-operations/getGuestAttributes?queryPath=hostkeys%2F", None)]
    cache = service.directory(operation) / "host-key.json"
    before = cache.read_bytes()
    assert TOKEN not in before.decode() and "private-key-CANARY" not in before.decode()
    assert json.loads(before)["application_health_claimed"] is False
    preflight.result["queryValue"]["items"][0]["value"] = public_blob(21)
    assert await service.host_keys(selected) == result
    assert cache.read_bytes() == before and len([call for call in preflight.calls if call[0] == "rest"]) == 1
    assert provisioning.calls == [selected, selected, selected]


@pytest.mark.parametrize("variant", ["flattened", "path", "foreign", "duplicate", "empty", "nonrow", "malformed", "wire", "mixed-foreign"])
async def test_host_key_requires_exact_rest_path_namespace_and_canonical_wire(access, variant):
    service, _, preflight, _, operation, _, _ = access
    await prepared(access)
    row = {"namespace": "hostkeys", "key": "ssh-ed25519", "value": public_blob(19)}
    value = {"queryPath": "hostkeys/", "queryValue": {"items": [row]}}
    if variant == "flattened": value = [row]
    elif variant == "path": value["queryPath"] = "another/"
    elif variant == "foreign": row["namespace"] = "foreign"
    elif variant == "duplicate": value["queryValue"]["items"].append(deepcopy(row))
    elif variant == "empty": value["queryValue"]["items"] = []
    elif variant == "nonrow": value["queryValue"]["items"] = [None]
    elif variant == "malformed": value["queryValue"] = None
    elif variant == "wire": row["value"] = "invalid-canonical-wire"
    else: value["queryValue"]["items"].append({"namespace": "foreign", "key": "ssh-rsa", "value": "foreign-canary"})
    preflight.result = value
    with pytest.raises(DraftConflict): await service.host_keys(endpoint(operation))
    assert not (service.directory(operation) / "host-key.json").exists()


@pytest.mark.parametrize("failure", ["accounts", "token", "rest", "first-verify", "second-verify", "different-id"])
async def test_unavailable_account_or_replaced_vm_never_pins_host_key(access, failure):
    service, _, preflight, provisioning, operation, _, _ = access
    await prepared(access)
    if failure == "accounts": preflight.accounts_checked = False
    elif failure == "token": preflight.token = None
    elif failure == "rest": preflight.error = ProbeUnavailable()
    elif failure == "first-verify": provisioning.failure_at = 1
    elif failure == "second-verify": provisioning.failure_at = 2
    else: provisioning.changes = {"instance_id": "987654321", "host": "compute.987654321"}
    with pytest.raises(DraftConflict) as caught:
        await service.host_keys(endpoint(operation))
    assert TOKEN not in str(caught.value)
    assert not (service.directory(operation) / "host-key.json").exists()
    if failure in {"first-verify", "different-id"}: assert preflight.calls == []


async def test_pinned_endpoint_cannot_cross_cloud_instance_and_bad_cache_cannot_be_used(access):
    service, _, _, provisioning, operation, _, _ = access
    await prepared(access)
    selected = endpoint(operation)
    await service.host_keys(selected)
    cache = service.directory(operation) / "host-key.json"
    before = cache.read_bytes()
    with pytest.raises(DraftConflict, match="another computer"):
        await service.host_keys({**selected, "instance_id": "987654321", "host": "compute.987654321"})
    assert cache.read_bytes() == before
    value = json.loads(before); value["key"] = "invalid"
    write_private(cache, value)
    with pytest.raises(DraftConflict): await service.host_keys(selected)


@pytest.mark.parametrize("record,content", [
    ("identity.json", b"[]"),
    ("identity.json", b"{broken-json"),
    ("host-key.json", b"[]"),
    ("host-key.json", b"{broken-json"),
])
async def test_corrupt_owned_records_raise_controlled_conflict_without_rewriting(access, record, content):
    service, keys, _, _, operation, _, _ = access
    await prepared(access)
    if record == "host-key.json": await service.host_keys(endpoint(operation))
    path = service.directory(operation) / record
    path.write_bytes(content)
    with pytest.raises(DraftConflict):
        if record == "identity.json": await prepared(access)
        else: await service.host_keys(endpoint(operation))
    assert path.read_bytes() == content and len(keys.calls) == 1
