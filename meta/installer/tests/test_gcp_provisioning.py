"""Cloud-free coordinator observations and durable uncertain-create semantics."""
import asyncio
import base64
from copy import deepcopy
import fcntl
import hashlib
import io
import json
import os
from pathlib import Path
import uuid
import zipfile
from types import SimpleNamespace

import pytest

from launchkit.gcp_preflight import CommandResult, ProbeUnavailable
from launchkit.gcp_access import GcpAccessService
from launchkit.gcp_bootstrap import BOOTSTRAP_SCRIPT
from launchkit.gcp_provisioning import GcpProvisioningService
from launchkit.gcp_transport import validate_gcp_endpoint
from launchkit.projects import DraftConflict
from test_gcp_preflight import slow_cloud_package_work
from test_remote_deployment import longest_loop_gap

ACCOUNT = "operator@example.com"
PROJECT = "mobkit-guided-project"
KEY = "ssh-ed25519 " + base64.b64encode(b"\x00\x00\x00\x0bssh-ed25519\x00\x00\x00\x20" + b"x" * 32).decode()


class Preflight:
    def __init__(self):
        self.guide = str(uuid.uuid4())
        self.binding = {"guide_id": self.guide, "guide_version": 7, "build_id": "accepted-build",
            "source_digest": "a" * 64, "package_sha256": "b" * 64, "blueprint_hash": "c" * 64}
        self.destination = {"project": PROJECT, "instance": "maintenance", "zone": "europe-west1-b",
            "machine_type": "e2-standard-2", "disk_gb": 30, "console_port": 8787}
        self.supported = True
        self.guided = True
        self.account = ACCOUNT
        self.checks = 0
        self.drift_at = None
        self.quota_full = False
        self.cloud = None
        self.definition = {"target": "gcp", "classes": [], "mcpConnections": []}
        self.local = SimpleNamespace(implementation=SimpleNamespace(package=self.package))
        self.refresh_package()

    def refresh_package(self):
        stream = io.BytesIO()
        with zipfile.ZipFile(stream, 'w') as archive:
            for name, content in {"deployment.json": json.dumps(self.definition), "app.py": "# fixture\n",
                                  "install.sh": "# fixture\n", "implementation-report.json": "{}"}.items():
                archive.writestr(name, content)
        self.payload = stream.getvalue()
        self.binding["package_sha256"] = hashlib.sha256(self.payload).hexdigest()

    async def package(self, guide_id, build_id):
        assert guide_id == self.guide and build_id == 'accepted-build'
        return self.payload, 'maintenance'

    async def _snapshot(self, guide_id, build_id, version):
        assert guide_id == self.guide and build_id == "accepted-build"
        return deepcopy(self.binding), deepcopy(self.destination), self.supported

    async def check_guided(self, guide_id, version, build_id, account, project):
        self.checks += 1
        if self.drift_at == self.checks:
            self.binding["source_digest"] = "d" * 64
        rows = [{"id": "package-deployer", "status": "checked", "required": True},
            {"id": "guest-host-keys", "status": "checked", "required": True},
            {"id": "instance-collision", "status": "blocked" if self.cloud.vm else "checked", "required": True},
            {"id": "firewall-collision", "status": "blocked" if self.cloud.firewall else "checked", "required": True}]
        if self.quota_full:
            rows.extend({"id": kind, "status": "blocked", "required": True} for kind in ["regional-quota", "project-quota"])
        return {"binding": deepcopy(self.binding), "destination": deepcopy(self.destination), "account": self.account,
            "project": project, "guided_supported": self.guided, "checks": rows,
            "ready_for_plan": all(row["status"] == "checked" for row in rows)}

    async def _accounts(self, account):
        return [{"id": self.account}], [{"id": "account", "status": "checked"}], False


class Cloud:
    def __init__(self, preflight):
        self.preflight = preflight
        preflight.cloud = self
        self.firewall = self.vm = self.disk = None
        self.calls = []
        self.cleanup_pending = False
        self.lose = None
        self.fail_list = False
        self.delay = None
        self.mutation_started = asyncio.Event()
        self.delete_delay = None
        self.delete_started = asyncio.Event()
        self.delete_lose = None
        self.delete_preserve = False
        self.delete_output = ""

    def base(self):
        return "https://www.googleapis.com/compute/v1/projects/" + PROJECT

    async def command(self, args):
        self.calls.append(list(args))
        assert "--account=" + ACCOUNT in args
        assert "--project=" + PROJECT in args
        for flag in ["--quiet", "--verbosity=error", "--no-log-http"]:
            assert flag in args
        assert "print-access-token" not in args and "ssh" not in args
        kind = "firewall" if args[:2] == ["compute", "firewall-rules"] else "vm" if args[:2] == ["compute", "instances"] else "disk"
        if args[2] == "list":
            if self.fail_list:
                return CommandResult(1, "PRIVATE_CLI_CANARY")
            row = getattr(self, kind)
            return CommandResult(0, json.dumps([row] if row else []))
        if args[2] == "delete":
            self.delete_started.set()
            assert getattr(self, kind) is not None
            if kind == "vm":
                assert "--keep-disks=all" in args
                self.vm["disks"][0]["autoDelete"] = False
            if self.delete_delay is not None:
                await self.delete_delay.wait()
            if self.delete_preserve is True or self.delete_preserve == kind:
                raise ProbeUnavailable("PRIVATE_DELETE_RESPONSE_CANARY")
            if kind == "disk":
                assert self.disk["users"] == []
            setattr(self, kind, None)
            if kind == "vm":
                self.disk["users"] = []
            if self.delete_lose == kind:
                raise ProbeUnavailable("PRIVATE_DELETE_RESPONSE_CANARY")
            return CommandResult(0, self.delete_output)
        assert args[2] == "create" and kind in {"firewall", "vm"}
        self.mutation_started.set()
        if self.delay is not None:
            await self.delay.wait()
        if self.lose == "unknown-spawn":
            raise ProbeUnavailable("PRIVATE_SPAWN_CANARY")
        d, base = self.preflight.destination, self.base()
        if kind == "firewall":
            token = next(item.removeprefix("--description=MobKit Launch deployment ") for item in args if item.startswith("--description="))
            self.firewall = {"id": "111", "name": d["instance"] + "-iap-ssh", "selfLink": base + "/global/firewalls/" + d["instance"] + "-iap-ssh",
                "description": "MobKit Launch deployment " + token, "network": base + "/global/networks/default", "direction": "INGRESS",
                "disabled": False, "priority": 1000, "targetTags": [d["instance"] + "-iap"], "sourceRanges": ["35.235.240.0/20"],
                "allowed": [{"IPProtocol": "tcp", "ports": ["22"]}]}
        else:
            token = next(item.removeprefix("--labels=mobkit_launch=") for item in args if item.startswith("--labels="))
            instance = base + "/zones/" + d["zone"] + "/instances/" + d["instance"]
            disk = base + "/zones/" + d["zone"] + "/disks/" + d["instance"]
            paths = next(item.removeprefix("--metadata-from-file=") for item in args if item.startswith("--metadata-from-file="))
            metadata = [{"key": "enable-guest-attributes", "value": "TRUE"}]
            for pair in paths.split(","):
                name, path = pair.split("=", 1)
                metadata.append({"key": name, "value": Path(path).read_text()})
                assert Path(path).stat().st_mode & 0o077 == 0
            self.vm = {"id": "222", "name": d["instance"], "selfLink": instance, "zone": base + "/zones/" + d["zone"],
                "machineType": base + "/zones/" + d["zone"] + "/machineTypes/" + d["machine_type"], "status": "RUNNING",
                "labels": {"mobkit_launch": token}, "tags": {"items": [d["instance"] + "-iap"]}, "serviceAccounts": [],
                "metadata": {"items": metadata},
                "shieldedInstanceConfig": {"enableSecureBoot": True},
                "networkInterfaces": [{"network": base + "/global/networks/default", "accessConfigs": [{"type": "ONE_TO_ONE_NAT", "natIP": "192.0.2.10"}]}],
                "disks": [{"source": disk, "boot": True, "autoDelete": True}]}
            self.disk = {"id": "333", "name": d["instance"], "selfLink": disk, "zone": base + "/zones/" + d["zone"],
                "type": base + "/zones/" + d["zone"] + "/diskTypes/pd-balanced", "sizeGb": "30", "status": "READY", "users": [instance],
                "sourceImage": "https://www.googleapis.com/compute/v1/projects/debian-cloud/global/images/debian-12-bookworm-v20260928"}
        if self.lose == kind:
            raise ProbeUnavailable("PRIVATE_RESPONSE_CANARY")
        return CommandResult(0, "{}")

    def mutations(self, kind=None):
        return [args for args in self.calls if args[2] == "create" and (kind is None or args[1] == kind)]

    def deletions(self, kind=None):
        return [args for args in self.calls if args[2] == "delete" and (kind is None or args[1] == kind)]


def setup(tmp_path):
    preflight = Preflight()
    cloud = Cloud(preflight)
    service = GcpProvisioningService(tmp_path / "private", preflight, command=cloud)
    async def prepare(_):
        return {"user": "mobkit", "public_key": KEY}
    service.prepare_access = prepare
    async def prepare_removal(_):
        return True
    service.prepare_removal = prepare_removal
    identity = str(uuid.uuid4())
    async def create(account=ACCOUNT, project=PROJECT, request_id=identity, wait=True):
        result = await service.create(preflight.guide, 7, "accepted-build", account, project, request_id)
        return await settled(service, preflight.guide, request_id) if wait else result
    return service, preflight, cloud, identity, create


async def settled(service, guide, identity):
    task = service.tasks.get(identity)
    if task is not None:
        await asyncio.shield(task)
    return await service.status(guide, identity)


async def resumed(service, guide, identity):
    await service.resume(guide, identity)
    return await settled(service, guide, identity)


@pytest.mark.asyncio
async def test_ready_means_observed_vm_not_installed_application_and_fixed_metadata(tmp_path):
    service, preflight, cloud, identity, create = setup(tmp_path)
    result = await create()
    assert result["status"] == "vm-ready" and result["vm_verified"]
    assert result["application_installed"] is False and result["application_status"] == "not-installed"
    assert len(cloud.mutations()) == 2
    vm = cloud.mutations("instances")[0]
    assert "--metadata=enable-guest-attributes=TRUE" in vm
    assert any(arg.startswith("--metadata-from-file=ssh-keys=") for arg in vm)
    assert {row["key"]: row["value"] for row in cloud.vm["metadata"]["items"]} == {
        "enable-guest-attributes": "TRUE", "ssh-keys": "mobkit:" + KEY, "startup-script": BOOTSTRAP_SCRIPT}
    assert "--no-service-account" in vm and "--no-scopes" in vm
    assert "--image-family=debian-12" in vm and "--boot-disk-type=pd-balanced" in vm
    assert "--network=default" in vm
    assert "--labels=mobkit_launch=" + uuid.UUID(identity).hex in vm
    assert result["endpoint"]["instance_id"] == "222"
    preflight.binding["guide_version"] = 99
    assert await service.verify(result["endpoint"]) == result["endpoint"]
    raw = next((tmp_path / "private" / "gcp-provisioning" / preflight.guide).glob(identity + ".json"))
    assert raw.stat().st_mode & 0o077 == 0
    assert "PRIVATE" not in raw.read_text() and "public_key" not in json.dumps(result)


@pytest.mark.asyncio
async def test_new_guest_script_uses_exact_package_grants_and_is_receipt_bound(tmp_path):
    service, preflight, cloud, identity, create = setup(tmp_path)
    preflight.definition.update(classes=[{"tools": {"mcp": [{"connection_id": "selected"}]}}],
        mcpConnections=[{"id": "selected", "transport": "stdio", "command": "npx"},
                        {"id": "unused", "transport": "stdio", "command": "uvx"}])
    preflight.refresh_package()
    result = await create()
    assert result['status'] == 'vm-ready'
    raw = json.loads((service._directory(preflight.guide) / (identity + '.json')).read_text())
    script = raw['bootstrap_script']
    assert '\ninstall(True, False)\n' in script
    assert raw['bootstrap_sha256'] == hashlib.sha256(script.encode()).hexdigest()
    assert {row['key']: row['value'] for row in cloud.vm['metadata']['items']}['startup-script'] == script


@pytest.mark.asyncio
async def test_package_changed_after_cloud_snapshot_refuses_before_access_or_mutation(tmp_path):
    service, preflight, cloud, _, create = setup(tmp_path)
    preflight.payload += b'changed-package'
    async def no_access(_): raise AssertionError('Access must not be prepared')
    service.prepare_access = no_access
    with pytest.raises(DraftConflict, match='package changed'): await create()
    assert not cloud.calls


@pytest.mark.asyncio
async def test_historical_resume_keeps_original_bootstrap_without_deriving_new_script(tmp_path, monkeypatch):
    service, preflight, cloud, identity, create = setup(tmp_path)
    await create()
    path = service._directory(preflight.guide) / (identity + '.json')
    before = path.read_bytes()
    restored = GcpProvisioningService(tmp_path / 'private', preflight, command=cloud)
    async def forbidden(_): raise AssertionError('Historical receipt must retain its script')
    monkeypatch.setattr(restored, '_package_bootstrap', forbidden)
    assert (await restored.create(preflight.guide, 7, 'accepted-build', ACCOUNT, PROJECT, identity))['status'] == 'vm-ready'
    assert path.read_bytes() == before
    assert json.loads(before)['bootstrap_script'] == BOOTSTRAP_SCRIPT
    assert len(cloud.mutations()) == 2


@pytest.mark.asyncio
async def test_duplicate_concurrent_request_and_lost_http_reply_never_create_twice(tmp_path):
    service, preflight, cloud, identity, create = setup(tmp_path)
    cloud.delay = asyncio.Event()
    first = asyncio.create_task(create())
    await cloud.mutation_started.wait()
    second = asyncio.create_task(create())
    assert (await service.status(preflight.guide))["status"] == "provisioning"
    cloud.delay.set()
    assert (await first)["endpoint"] == (await second)["endpoint"]
    restored = GcpProvisioningService(tmp_path / "private", preflight, command=cloud)
    assert (await restored.create(preflight.guide, 7, "accepted-build", ACCOUNT, PROJECT, identity))["status"] == "vm-ready"
    assert len(cloud.mutations()) == 2
    with pytest.raises(DraftConflict):
        await create(account="another@example.com")
    with pytest.raises(DraftConflict):
        await create(request_id=str(uuid.uuid4()))


@pytest.mark.asyncio
async def test_admission_returns_durable_identity_before_slow_cloud_result_and_cancelled_observer(tmp_path):
    service, preflight, cloud, identity, create = setup(tmp_path)
    cloud.delay = asyncio.Event()
    admitted = await create(wait=False)
    assert admitted["operation_id"] == identity and admitted["status"] == "provisioning"
    assert admitted["account"] == ACCOUNT and admitted["destination"]["project"] == PROJECT
    assert not cloud.mutations()
    await cloud.mutation_started.wait()
    observer = asyncio.create_task(settled(service, preflight.guide, identity))
    await asyncio.sleep(0)
    observer.cancel()
    with pytest.raises(asyncio.CancelledError): await observer
    assert identity in service.tasks and not service.tasks[identity].done()
    assert (await create(wait=False))["operation_id"] == identity
    cloud.delay.set()
    assert (await settled(service, preflight.guide, identity))["status"] == "vm-ready"
    assert len(cloud.mutations()) == 2


@pytest.mark.asyncio
async def test_second_coordinator_contention_cannot_deadlock_or_rewrite_owned_pending_receipt(tmp_path):
    service, preflight, cloud, identity, create = setup(tmp_path)
    cloud.delay = asyncio.Event()
    await create(wait=False)
    await cloud.mutation_started.wait()
    restored = GcpProvisioningService(tmp_path / "private", preflight, command=cloud)
    with pytest.raises(DraftConflict, match="still owned"):
        await asyncio.wait_for(restored.resume(preflight.guide, identity), 0.5)
    with pytest.raises(DraftConflict, match="still owned"):
        await asyncio.wait_for(restored.status(preflight.guide, identity), 0.5)
    current = await service.status(preflight.guide, identity)
    assert current["status"] == "provisioning" and current["phase"] == "creating-firewall"
    cloud.delay.set()
    assert (await settled(service, preflight.guide, identity))["status"] == "vm-ready"
    assert len(cloud.mutations()) == 2


@pytest.mark.asyncio
@pytest.mark.parametrize("lost", ["firewall", "vm"])
async def test_lost_cli_result_needs_explicit_resume_of_exact_owned_resources(tmp_path, lost):
    service, preflight, cloud, identity, create = setup(tmp_path)
    cloud.lose = lost
    result = await create()
    assert result["status"] == "resume-required"
    before = len(cloud.mutations())
    assert (await create())["status"] == "resume-required"
    assert len(cloud.mutations()) == before
    cloud.lose = None
    assert (await resumed(service, preflight.guide, identity))["status"] == "vm-ready"
    assert len(cloud.mutations("firewall-rules")) == len(cloud.mutations("instances")) == 1


@pytest.mark.asyncio
async def test_unknown_spawn_with_positive_absence_never_issues_replacement_create(tmp_path):
    service, preflight, cloud, identity, create = setup(tmp_path)
    cloud.lose = "unknown-spawn"
    assert (await create())["status"] == "resume-required"
    cloud.lose = None
    assert (await resumed(service, preflight.guide, identity))["status"] == "resume-required"
    assert len(cloud.mutations()) == 1 and cloud.firewall is None


@pytest.mark.asyncio
async def test_lost_firewall_reply_then_lost_vm_reply_retains_prior_resource_and_recovers_exact_pair(tmp_path):
    service, preflight, cloud, identity, create = setup(tmp_path)
    cloud.lose = "firewall"
    assert (await create())["status"] == "resume-required"
    restored = GcpProvisioningService(tmp_path / "private", preflight, command=cloud)
    cloud.lose = "vm"
    second = await resumed(restored, preflight.guide, identity)
    assert second["status"] == "resume-required" and second["phase"] == "creating-vm"
    assert second["resources"]["firewall"]["id"] == "111"
    again = GcpProvisioningService(tmp_path / "private", preflight, command=cloud)
    cloud.lose = None
    assert (await resumed(again, preflight.guide, identity))["status"] == "vm-ready"
    assert len(cloud.mutations("firewall-rules")) == len(cloud.mutations("instances")) == 1


async def interrupted_spawn(tmp_path, kind):
    service, preflight, cloud, identity, create = setup(tmp_path)
    command = cloud.command
    group = "firewall-rules" if kind == "firewall" else "instances"
    async def uncertain(arguments):
        if arguments[:3] == ["compute", group, "create"]:
            cloud.calls.append(list(arguments))
            raise ProbeUnavailable("PRIVATE_UNKNOWN_SPAWN_CANARY")
        return await command(arguments)
    cloud.command = uncertain
    assert (await create())["status"] == "resume-required"
    cloud.command = command
    directory = tmp_path / "private" / "gcp-provisioning" / preflight.guide
    return service, preflight, cloud, identity, directory


@pytest.mark.asyncio
@pytest.mark.parametrize("kind", ["firewall", "vm"])
@pytest.mark.parametrize("corruption", ["missing", "cleared", "list", "phase", "time", "reset-phase"])
async def test_pending_damage_cannot_refund_mutation_after_reconstruction(tmp_path, kind, corruption):
    service, preflight, cloud, identity, directory = await interrupted_spawn(tmp_path, kind)
    path = directory / (identity + ".json")
    original = path.read_bytes()
    markers = {marker.name: marker.read_bytes() for marker in directory.glob("*.attempt")}
    receipt = json.loads(original)
    if corruption == "missing": receipt.pop("pending")
    if corruption == "cleared": receipt["pending"] = None
    if corruption == "list": receipt["pending"] = []
    if corruption == "phase": receipt["pending"]["phase"] = "PRIVATE_PHASE_CANARY"
    if corruption == "time": receipt["pending"]["started_at"] = "PRIVATE_TIME_CANARY"
    if corruption == "reset-phase": receipt.update(phase="checking", pending=None, resources={})
    path.write_text(json.dumps(receipt))
    restored = GcpProvisioningService(tmp_path / "private", preflight, command=cloud)
    admitted_count = len(cloud.mutations())
    if corruption == "reset-phase":
        assert (await resumed(restored, preflight.guide, identity))["status"] == "resume-required"
    else:
        with pytest.raises(DraftConflict) as error:
            await restored.resume(preflight.guide, identity)
        assert "PRIVATE" not in str(error.value)
    assert len(cloud.mutations()) == admitted_count == (1 if kind == "firewall" else 2)
    assert {marker.name: marker.read_bytes() for marker in directory.glob("*.attempt")} == markers
    path.write_bytes(original)
    assert (await resumed(restored, preflight.guide, identity))["status"] == "resume-required"
    assert len(cloud.mutations()) == admitted_count


@pytest.mark.asyncio
@pytest.mark.parametrize("kind", ["firewall", "vm"])
@pytest.mark.parametrize("corruption", ["missing", "json", "source", "request", "generation", "action", "protocol", "public-mode"])
async def test_missing_or_changed_mutation_marker_blocks_recovery_without_replacement(tmp_path, kind, corruption):
    service, preflight, cloud, identity, directory = await interrupted_spawn(tmp_path, kind)
    phase = "creating-firewall" if kind == "firewall" else "creating-vm"
    marker = directory / (identity + "." + phase + ".attempt")
    receipt_before = (directory / (identity + ".json")).read_bytes()
    if corruption == "missing": marker.unlink()
    elif corruption == "json": marker.write_text("{PRIVATE_MARKER_CANARY")
    elif corruption == "public-mode": marker.chmod(0o644)
    else:
        value = json.loads(marker.read_text())
        if corruption == "source": value["binding"]["source_digest"] = "d" * 64
        if corruption == "request": value["request_fingerprint"] = "e" * 64
        if corruption == "generation": value["ownership_token"] = "f" * 32
        if corruption == "action": value["resource"]["name"] = "different-resource"
        if corruption == "protocol": value["protocol"] = True
        marker.write_text(json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n")
    restored = GcpProvisioningService(tmp_path / "private", preflight, command=cloud)
    before = len(cloud.mutations())
    with pytest.raises(DraftConflict) as error:
        await restored.status(preflight.guide, identity)
    assert "PRIVATE" not in str(error.value)
    with pytest.raises(DraftConflict): await restored.resume(preflight.guide, identity)
    assert len(cloud.mutations()) == before
    assert (directory / (identity + ".json")).read_bytes() == receipt_before


@pytest.mark.asyncio
async def test_mutation_intent_and_pending_are_durable_before_each_cli_create(tmp_path, monkeypatch):
    service, preflight, cloud, identity, create = setup(tmp_path)
    command, fsync = cloud.command, os.fsync
    synchronized = []
    def synchronize(descriptor):
        info = os.fstat(descriptor)
        synchronized.append((info.st_dev, info.st_ino))
        return fsync(descriptor)
    monkeypatch.setattr(os, "fsync", synchronize)
    checked = []
    async def observed(arguments):
        if arguments[2] == "create":
            phase = "creating-firewall" if arguments[1] == "firewall-rules" else "creating-vm"
            directory = tmp_path / "private" / "gcp-provisioning" / preflight.guide
            marker = directory / (identity + "." + phase + ".attempt")
            info, parent = marker.stat(), directory.stat()
            marker_sync = synchronized.index((info.st_dev, info.st_ino))
            assert (parent.st_dev, parent.st_ino) in synchronized[marker_sync + 1:]
            assert info.st_mode & 0o077 == 0 and info.st_nlink == 1
            receipt = json.loads((directory / (identity + ".json")).read_text())
            intent = json.loads(marker.read_text())
            assert receipt["phase"] == receipt["pending"]["phase"] == intent["phase"] == phase
            assert intent["operation_id"] == identity and intent["binding"] == preflight.binding
            assert intent["request_fingerprint"] == receipt["request_fingerprint"]
            assert intent["ownership_token"] == uuid.UUID(identity).hex
            checked.append(phase)
        return await command(arguments)
    cloud.command = observed
    assert (await create())["status"] == "vm-ready"
    assert checked == ["creating-firewall", "creating-vm"]


@pytest.mark.asyncio
async def test_shutdown_waits_for_admission_and_never_starts_work_after_gated_access_returns(tmp_path):
    service, preflight, cloud, identity, create = setup(tmp_path)
    entered, release = asyncio.Event(), asyncio.Event()
    async def prepare(_):
        entered.set()
        await release.wait()
        return {"user": "mobkit", "public_key": KEY}
    service.prepare_access = prepare
    admission = asyncio.create_task(create(wait=False))
    await asyncio.wait_for(entered.wait(), 30)
    stopping = asyncio.create_task(service.shutdown())
    await asyncio.sleep(0)
    assert service.closed and not stopping.done() and not service.tasks and not cloud.calls
    release.set()
    with pytest.raises(DraftConflict, match="shutting down"):
        await asyncio.wait_for(admission, 1)
    await asyncio.wait_for(stopping, 1)
    assert not service.tasks and not cloud.mutations()
    assert not list((tmp_path / "private" / "gcp-provisioning" / preflight.guide).glob("*.json"))
    with pytest.raises(DraftConflict, match="shutting down"):
        await create(wait=False)


@pytest.mark.asyncio
async def test_source_drift_between_mutations_keeps_partial_firewall_and_stops_vm(tmp_path):
    service, preflight, cloud, identity, create = setup(tmp_path)
    preflight.drift_at = 2
    assert (await create())["status"] == "blocked"
    assert cloud.firewall is not None and cloud.vm is None
    assert len(cloud.mutations()) == 1
    assert (await resumed(service, preflight.guide, identity))["status"] == "blocked"
    assert len(cloud.mutations()) == 1


@pytest.mark.asyncio
@pytest.mark.parametrize("failure", ["guided", "account", "source", "foreign-firewall", "foreign-vm", "unknown-list", "no-access"])
async def test_failed_prerequisites_or_foreign_resources_never_admit_creation(tmp_path, failure):
    service, preflight, cloud, identity, create = setup(tmp_path)
    if failure == "guided": preflight.guided = False
    if failure == "account": preflight.account = "another@example.com"
    if failure == "source": preflight.drift_at = 1
    if failure == "unknown-list": cloud.fail_list = True
    if failure == "no-access": service.prepare_access = None
    if failure == "foreign-firewall":
        cloud.firewall = {"id": "111", "name": "maintenance-iap-ssh", "selfLink": cloud.base() + "/global/firewalls/maintenance-iap-ssh", "description": "Foreign"}
    if failure == "foreign-vm":
        cloud.vm = {"id": "222", "name": "maintenance", "selfLink": cloud.base() + "/zones/europe-west1-b/instances/maintenance", "labels": {"mobkit_launch": "foreign"}}
    if failure == "no-access":
        with pytest.raises(DraftConflict): await create()
    else:
        result = await create()
        assert result["status"] in {"blocked", "resume-required"}
        assert "PRIVATE" not in json.dumps(result)
    assert not cloud.mutations()


@pytest.mark.asyncio
async def test_replaced_vm_identity_and_access_scope_cannot_verify_saved_endpoint(tmp_path):
    service, preflight, cloud, identity, create = setup(tmp_path)
    result = await create()
    cloud.vm["id"] = "999"
    with pytest.raises(DraftConflict): await service.verify(result["endpoint"])
    cloud.vm["id"] = "222"
    cloud.firewall["allowed"] = [{"IPProtocol": "tcp", "ports": ["22", "443"]}]
    with pytest.raises(DraftConflict): await service.verify(result["endpoint"])


@pytest.mark.asyncio
async def test_observed_u64_resource_boundary_is_ready_and_transport_compatible(tmp_path):
    service, preflight, cloud, identity, create = setup(tmp_path)
    command = cloud.command
    maximum = str(2 ** 64 - 1)
    async def observed(arguments):
        result = await command(arguments)
        if arguments[2] == "create":
            for row in [cloud.firewall, cloud.vm, cloud.disk]:
                if row is not None:
                    row["id"] = maximum
        return result
    cloud.command = observed
    result = await create()
    assert result["status"] == "vm-ready" and result["vm_verified"] is True
    assert {row["id"] for row in result["resources"].values()} == {maximum}
    assert validate_gcp_endpoint(result["endpoint"]) == result["endpoint"]
    assert await service.verify(result["endpoint"]) == result["endpoint"]
    assert len(cloud.mutations()) == 2


@pytest.mark.asyncio
@pytest.mark.parametrize("kind", ["firewall", "vm", "disk"])
@pytest.mark.parametrize("resource_id", [str(2 ** 64), "9" * 24])
async def test_out_of_range_observed_resource_cannot_publish_ready_or_replacement(tmp_path, kind, resource_id):
    service, preflight, cloud, identity, create = setup(tmp_path)
    command = cloud.command
    async def observed(arguments):
        result = await command(arguments)
        if arguments[2] == "create" and getattr(cloud, kind) is not None:
            getattr(cloud, kind)["id"] = resource_id
        return result
    cloud.command = observed
    result = await create()
    assert result["status"] == "blocked" and result["vm_verified"] is False
    assert "endpoint" not in result
    assert all(row["id"] != resource_id for row in result["resources"].values())
    admitted_count = len(cloud.mutations())
    assert admitted_count == (1 if kind == "firewall" else 2)
    assert (await create())["status"] == "blocked"
    assert (await resumed(service, preflight.guide, identity))["status"] == "blocked"
    assert len(cloud.mutations()) == admitted_count


@pytest.mark.asyncio
@pytest.mark.parametrize("kind", ["firewall", "vm", "disk"])
async def test_restored_ready_receipt_rejects_out_of_range_resource_identity(tmp_path, kind):
    service, preflight, cloud, identity, create = setup(tmp_path)
    await create()
    path = tmp_path / "private" / "gcp-provisioning" / preflight.guide / (identity + ".json")
    receipt = json.loads(path.read_text())
    receipt["resources"][kind]["id"] = str(2 ** 64)
    if kind == "vm":
        receipt["endpoint"]["instance_id"] = str(2 ** 64)
        receipt["endpoint"]["host"] = "compute." + str(2 ** 64)
    path.write_text(json.dumps(receipt))
    with pytest.raises(DraftConflict):
        await service.status(preflight.guide, identity)
    assert len(cloud.mutations()) == 2


@pytest.mark.asyncio
async def test_server_shutdown_records_uncertain_phase_without_new_mutation(tmp_path):
    service, preflight, cloud, identity, create = setup(tmp_path)
    cloud.delay = asyncio.Event()
    task = asyncio.create_task(create())
    await cloud.mutation_started.wait()
    await service.shutdown()
    with pytest.raises(asyncio.CancelledError): await task
    result = await service.status(preflight.guide, identity)
    assert result["status"] == "resume-required" and result["phase"] == "creating-firewall"
    assert len(cloud.mutations()) == 1


@pytest.mark.asyncio
async def test_owned_command_cleanup_blocks_every_new_cli_probe(tmp_path):
    service, preflight, cloud, identity, create = setup(tmp_path)
    cloud.cleanup_pending = True
    assert (await create())["status"] == "resume-required"
    assert not cloud.calls
    with pytest.raises(DraftConflict): await service.shutdown()


@pytest.mark.asyncio
async def test_owned_ready_vm_needs_no_additional_quota_but_partial_resume_still_does(tmp_path):
    service, preflight, cloud, identity, create = setup(tmp_path)
    result = await create()
    preflight.quota_full = True
    assert (await resumed(service, preflight.guide, identity))["status"] == "vm-ready"
    assert len(cloud.mutations()) == 2
    assert await service.verify(result["endpoint"]) == result["endpoint"]
    other, p2, c2, id2, create2 = setup(tmp_path / "second")
    c2.lose = "firewall"
    assert (await create2())["status"] == "resume-required"
    c2.lose = None
    p2.quota_full = True
    assert (await resumed(other, p2.guide, id2))["status"] == "blocked"
    assert not c2.mutations("instances")


@pytest.mark.asyncio
async def test_latest_source_rejection_does_not_revoke_original_ready_endpoint_observation(tmp_path):
    service, preflight, cloud, identity, create = setup(tmp_path)
    original = await create()
    preflight.binding["source_digest"] = "e" * 64
    rejected = await resumed(service, preflight.guide, identity)
    assert rejected["status"] == "blocked" and not rejected["vm_verified"]
    assert await service.verify(original["endpoint"]) == original["endpoint"]
    assert len(cloud.mutations()) == 2


@pytest.mark.asyncio
@pytest.mark.parametrize("change", ["bootstrap", "os-login-override", "invalid-ipv4", "disk-image", "foreign-link", "missing-disk"])
async def test_modified_owned_resource_is_not_promoted_to_ready_or_recreated(tmp_path, change):
    service, preflight, cloud, identity, create = setup(tmp_path)
    result = await create()
    if change == "bootstrap": cloud.vm["metadata"]["items"][2]["value"] += "\nPRIVATE_BOOTSTRAP_CANARY\n"
    if change == "os-login-override": cloud.vm["metadata"]["items"].append({"key": "enable-oslogin", "value": "FALSE"})
    if change == "invalid-ipv4": cloud.vm["networkInterfaces"][0]["accessConfigs"][0]["natIP"] = "PRIVATE_ADDRESS_CANARY"
    if change == "disk-image": cloud.disk["sourceImage"] = cloud.base() + "/global/images/foreign"
    if change == "foreign-link": cloud.vm["selfLink"] = "PRIVATE_LINK_CANARY" + cloud.vm["selfLink"]
    if change == "missing-disk": cloud.disk = None
    with pytest.raises(DraftConflict) as error:
        await service.verify(result["endpoint"])
    assert "PRIVATE" not in str(error.value)
    assert (await resumed(service, preflight.guide, identity))["status"] == "blocked"
    assert len(cloud.mutations()) == 2


@pytest.mark.asyncio
@pytest.mark.parametrize("corruption", ["selector-json", "selector-missing", "receipt-json", "status", "phase", "source", "endpoint", "account"])
async def test_corrupt_private_receipt_fails_closed_without_private_detail_or_replacement(tmp_path, corruption):
    service, preflight, cloud, identity, create = setup(tmp_path)
    await create()
    directory = tmp_path / "private" / "gcp-provisioning" / preflight.guide
    path = directory / (identity + ".json")
    value = json.loads(path.read_text())
    if corruption == "selector-json": (directory / "latest.json").write_text("{PRIVATE_SELECTOR_CANARY")
    if corruption == "selector-missing": (directory / "latest.json").write_text(json.dumps({"operation_id": str(uuid.uuid4())}))
    if corruption == "receipt-json": path.write_text("{PRIVATE_RECEIPT_CANARY")
    if corruption == "status": value["status"] = {"PRIVATE_STATUS_CANARY": True}
    if corruption == "phase": value["phase"] = "PRIVATE_PHASE_CANARY"
    if corruption == "source": value["binding"]["source_digest"] = "PRIVATE_SOURCE_CANARY"
    if corruption == "endpoint": value["endpoint"]["instance_id"] = "PRIVATE_ENDPOINT_CANARY"
    if corruption == "account": value["account"] = "PRIVATE_ACCOUNT_CANARY"
    if corruption not in {"selector-json", "selector-missing", "receipt-json"}: path.write_text(json.dumps(value))
    with pytest.raises(DraftConflict) as error: await service.status(preflight.guide)
    assert "PRIVATE" not in str(error.value)
    with pytest.raises(DraftConflict): await create(request_id=str(uuid.uuid4()))
    assert len(cloud.mutations()) == 2


@pytest.mark.asyncio
async def test_real_access_service_comment_joins_exact_operation_and_metadata_keeps_canonical_wire_key(tmp_path):
    service, preflight, cloud, identity, create = setup(tmp_path)
    generated = []
    async def key_runner(arguments, payload, timeout):
        generated.append(arguments)
        path = Path(arguments[-1])
        path.write_text("-----BEGIN OPENSSH PRIVATE KEY-----\nOFFLINE_PRIVATE_KEY_CANARY\n-----END OPENSSH PRIVATE KEY-----\n")
        path.chmod(0o600)
        path.with_name("identity.pub").write_text(KEY + " " + arguments[7] + "\n")
        return b"", b"", 0
    access = GcpAccessService(tmp_path / "private", preflight, service, key_runner=key_runner)
    async def prepare(value):
        return await access.prepare_access(value["binding"], value["destination"], value["operation_id"])
    service.prepare_access = prepare
    result = await create()
    assert result["status"] == "vm-ready" and len(generated) == 1
    assert (access.directory(identity) / "identity.pub").read_text().strip() == KEY + " mobkit-launch:" + identity
    items = {row["key"]: row["value"] for row in cloud.vm["metadata"]["items"]}
    assert items["ssh-keys"] == "mobkit:" + KEY
    assert "OFFLINE_PRIVATE_KEY_CANARY" not in json.dumps(result)
    assert len(cloud.mutations()) == 2


@pytest.mark.asyncio
async def test_foreign_key_comment_cannot_bind_access_to_another_operation(tmp_path):
    service, preflight, cloud, identity, create = setup(tmp_path)
    async def prepare(_):
        return {"user": "mobkit", "public_key": KEY + " mobkit-launch:" + str(uuid.uuid4())}
    service.prepare_access = prepare
    with pytest.raises(DraftConflict): await create()
    assert not cloud.calls


@pytest.mark.asyncio
@pytest.mark.parametrize("terminal", ["unknown", "blocked", "cancelled"])
async def test_terminal_status_write_still_excludes_second_coordinator_ownership(tmp_path, terminal):
    service, preflight, cloud, identity, create = setup(tmp_path)
    saved = service._save
    contested = []
    def observe_saved(directory, value):
        if value["status"] in {"resume-required", "blocked"}:
            descriptor = os.open(directory / "provision.lock", os.O_RDWR)
            try:
                with pytest.raises(BlockingIOError):
                    fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
                contested.append(value["status"])
            finally:
                os.close(descriptor)
        saved(directory, value)
    service._save = observe_saved
    if terminal == "unknown": cloud.lose = "firewall"
    if terminal == "blocked": preflight.drift_at = 1
    if terminal == "cancelled":
        cloud.delay = asyncio.Event()
        await create(wait=False)
        await cloud.mutation_started.wait()
        await service.shutdown()
        result = await service.status(preflight.guide, identity)
    else:
        result = await create()
    assert contested == ["blocked" if terminal == "blocked" else "resume-required"]
    assert result["status"] == contested[0]
    assert len(cloud.mutations()) == (0 if terminal == "blocked" else 1)


@pytest.mark.asyncio
@pytest.mark.parametrize("output", ["", "null", '{"kind":"compute#operation","status":"DONE"}'])
async def test_removal_uses_original_receipt_after_source_destination_drift_and_observes_absence(tmp_path, output):
    service, preflight, cloud, identity, create = setup(tmp_path)
    ready = await create()
    directory = service._directory(preflight.guide)
    original = service._read(directory, identity)
    attempts = {path.name: path.read_bytes() for path in directory.glob("*.attempt")}
    assert "removal" not in ready and await service.lifecycle(ready["endpoint"]) is None
    preflight.binding["source_digest"] = "e" * 64
    preflight.destination["instance"] = "another-destination"
    async def no_current_source(*_):
        raise AssertionError("Removal must not rederive current source or destination")
    preflight._snapshot = no_current_source
    cloud.delete_output = output
    request_id = str(uuid.uuid4())
    admitted = await service.remove(preflight.guide, identity, request_id, True)
    assert admitted["status"] == "removing" and admitted["removal"]["request_id"] == request_id
    with pytest.raises(DraftConflict): await service.verify(ready["endpoint"])
    with pytest.raises(DraftConflict): await service.resume(preflight.guide, identity)
    result = await settled(service, preflight.guide, identity)
    assert result["status"] == "removed"
    assert result["removal"] == {"request_id": request_id, "acknowledge_data_loss": True,
        "resources": {kind: {"status": "absent"} for kind in ("vm", "disk", "firewall")}}
    assert not result["vm_verified"] and not result["resume_required"] and not result["application_installed"]
    saved = service._read(directory, identity)
    for key in ("phase", "pending", "resources", "endpoint", "request", "binding", "destination"):
        assert saved[key] == original[key]
    assert all((directory / name).read_bytes() == content for name, content in attempts.items())
    assert [args[1] for args in cloud.deletions()] == ["instances", "disks", "firewall-rules"]
    assert all(args[3].startswith("maintenance") for args in cloud.deletions())
    assert "--keep-disks=all" in cloud.deletions("instances")[0]
    assert await service.lifecycle(ready["endpoint"]) == "removed"
    assert (await service.remove(preflight.guide, identity, request_id, True))["status"] == "removed"
    assert len(cloud.deletions()) == 3 and len(cloud.mutations()) == 2
    with pytest.raises(DraftConflict): await service.remove(preflight.guide, identity, str(uuid.uuid4()), True)
    with pytest.raises(DraftConflict): await create(request_id=str(uuid.uuid4()))
    assert "PRIVATE" not in json.dumps(result) and "public_key" not in json.dumps(result)
    for kind in ("vm", "disk", "firewall"):
        attempt = directory / (identity + ".removing-" + kind + ".attempt")
        assert attempt.stat().st_mode & 0o077 == 0
        assert json.loads(attempt.read_text())["resource"] == original["resources"][kind]


@pytest.mark.asyncio
async def test_removal_active_replay_returns_same_intent_without_duplicate_delete(tmp_path):
    service, preflight, cloud, identity, create = setup(tmp_path)
    await create()
    request_id = str(uuid.uuid4())
    cloud.delete_delay = asyncio.Event()
    first = await service.remove(preflight.guide, identity, request_id, True)
    assert first["status"] == "removing"
    await cloud.delete_started.wait()
    replay = await asyncio.wait_for(service.remove(preflight.guide, identity, request_id, True), 0.5)
    assert replay["status"] == "removing" and replay["removal"]["request_id"] == request_id
    assert len(cloud.deletions()) == 1
    with pytest.raises(DraftConflict): await service.remove(preflight.guide, identity, str(uuid.uuid4()), True)
    cloud.delete_delay.set()
    assert (await settled(service, preflight.guide, identity))["status"] == "removed"
    assert len(cloud.deletions()) == 3


@pytest.mark.asyncio
async def test_queued_old_creation_cannot_run_after_second_coordinator_admits_removal(tmp_path):
    service, preflight, cloud, identity, create = setup(tmp_path)
    entered, release = asyncio.Event(), asyncio.Event()
    original_run = service._run
    async def queued(directory, value):
        entered.set()
        await release.wait()
        return await original_run(directory, value)
    service._run = queued
    await create(wait=False)
    await entered.wait()
    remover = GcpProvisioningService(service.root, preflight, command=cloud)
    remover.prepare_removal = service.prepare_removal
    request_id = str(uuid.uuid4())
    await remover.remove(preflight.guide, identity, request_id, True)
    assert (await settled(remover, preflight.guide, identity))["status"] == "removed"
    release.set()
    assert (await settled(service, preflight.guide, identity))["status"] == "removed"
    assert not cloud.mutations() and not cloud.deletions()


@pytest.mark.asyncio
@pytest.mark.parametrize("ack", [False, None, 1, "true"])
async def test_removal_requires_literal_acknowledgement_before_intent(tmp_path, ack):
    service, preflight, cloud, identity, create = setup(tmp_path)
    await create()
    with pytest.raises(ValueError): await service.remove(preflight.guide, identity, str(uuid.uuid4()), ack)
    assert "removal" not in await service.status(preflight.guide, identity)
    assert not cloud.deletions()


@pytest.mark.asyncio
@pytest.mark.parametrize("confirmation", [None, False, "true", 1, "unassigned"])
async def test_removal_persists_intent_but_needs_positive_local_coordination_before_cloud_commands(tmp_path, confirmation):
    service, preflight, cloud, identity, create = setup(tmp_path)
    ready = await create()
    before = len(cloud.calls)
    observed = []
    async def prepare(value):
        assert value == service._read(service._directory(preflight.guide), identity)
        assert await service.lifecycle(ready["endpoint"]) == "removing"
        descriptor = os.open(service._directory(preflight.guide) / "provision.lock", os.O_RDWR)
        try:
            fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
        finally:
            os.close(descriptor)
        observed.append(value["removal"]["request_id"])
        return confirmation
    service.prepare_removal = None if confirmation == "unassigned" else prepare
    request_id = str(uuid.uuid4())
    await service.remove(preflight.guide, identity, request_id, True)
    result = await settled(service, preflight.guide, identity)
    assert result["status"] == "removal-required" and observed == ([] if confirmation == "unassigned" else [request_id])
    assert len(cloud.calls) == before and not cloud.deletions()


@pytest.mark.asyncio
async def test_removal_requires_original_selected_account_not_current_source(tmp_path):
    service, preflight, cloud, identity, create = setup(tmp_path)
    await create()
    preflight.account = "another@example.com"
    await service.remove(preflight.guide, identity, str(uuid.uuid4()), True)
    assert (await settled(service, preflight.guide, identity))["status"] == "removal-required"
    assert not cloud.deletions()


@pytest.mark.asyncio
async def test_removal_refuses_cloud_commands_until_owned_command_cleanup_is_confirmed(tmp_path):
    service, preflight, cloud, identity, create = setup(tmp_path)
    await create()
    before = len(cloud.calls)
    cloud.cleanup_pending = True
    await service.remove(preflight.guide, identity, str(uuid.uuid4()), True)
    assert (await settled(service, preflight.guide, identity))["status"] == "removal-required"
    assert len(cloud.calls) == before and not cloud.deletions()


@pytest.mark.asyncio
@pytest.mark.parametrize("foreign", ["vm-id", "firewall-id", "token", "extra-disk", "attached-disk"])
async def test_removal_never_deletes_foreign_replacement_or_attachments(tmp_path, foreign):
    service, preflight, cloud, identity, create = setup(tmp_path)
    await create()
    if foreign == "vm-id": cloud.vm["id"] = "444"
    if foreign == "firewall-id": cloud.firewall["id"] = "444"
    if foreign == "token": cloud.vm["labels"]["mobkit_launch"] = "f" * 32
    if foreign == "extra-disk": cloud.vm["disks"].append({"source": "foreign", "boot": False, "autoDelete": True})
    if foreign == "attached-disk": cloud.vm = None; cloud.disk["users"] = ["foreign"]
    await service.remove(preflight.guide, identity, str(uuid.uuid4()), True)
    result = await settled(service, preflight.guide, identity)
    assert result["status"] == "removal-required" and not cloud.deletions()
    assert "PRIVATE" not in json.dumps(result)


@pytest.mark.asyncio
async def test_removal_of_known_partial_firewall_preserves_creation_phase_and_pending(tmp_path):
    service, preflight, cloud, identity, create = setup(tmp_path)
    preflight.drift_at = 2
    assert (await create())["status"] == "blocked"
    before = service._read(service._directory(preflight.guide), identity)
    assert set(before["resources"]) == {"firewall"}
    await service.remove(preflight.guide, identity, str(uuid.uuid4()), True)
    result = await settled(service, preflight.guide, identity)
    assert result["status"] == "removed" and result["phase"] == before["phase"]
    after = service._read(service._directory(preflight.guide), identity)
    assert after["pending"] == before["pending"] and after["resources"] == before["resources"]
    assert len(cloud.deletions()) == 1 and cloud.deletions()[0][1] == "firewall-rules"
    assert "endpoint" not in result


@pytest.mark.asyncio
@pytest.mark.parametrize("lost", ["firewall", "vm"])
async def test_removal_can_join_original_unrecorded_resource_after_lost_create_reply(tmp_path, lost):
    service, preflight, cloud, identity, create = setup(tmp_path)
    cloud.lose = lost
    assert (await create())["status"] == "resume-required"
    directory = service._directory(preflight.guide)
    before = service._read(directory, identity)
    await service.remove(preflight.guide, identity, str(uuid.uuid4()), True)
    result = await settled(service, preflight.guide, identity)
    after = service._read(directory, identity)
    assert result["status"] == "removed" and after["pending"] == before["pending"] and after["phase"] == before["phase"]
    assert len(cloud.deletions()) == (1 if lost == "firewall" else 3)
    assert len(cloud.mutations()) == (1 if lost == "firewall" else 2)


@pytest.mark.asyncio
@pytest.mark.parametrize("unjoined", ["empty-create", "orphan-disk"])
async def test_removal_never_calls_unrecorded_lost_create_absence_or_orphan_disk_removed(tmp_path, unjoined):
    service, preflight, cloud, identity, create = setup(tmp_path)
    cloud.lose = "unknown-spawn" if unjoined == "empty-create" else "vm"
    await create()
    if unjoined == "orphan-disk": cloud.vm = None; cloud.disk["users"] = []
    await service.remove(preflight.guide, identity, str(uuid.uuid4()), True)
    result = await settled(service, preflight.guide, identity)
    assert result["status"] == "removal-required" and not cloud.deletions()
    assert any(row["status"] != "absent" for row in result["removal"]["resources"].values())


@pytest.mark.asyncio
@pytest.mark.parametrize("preserved", [False, True])
@pytest.mark.parametrize("kind", ["vm", "disk", "firewall"])
async def test_lost_delete_reply_only_absence_resolves_spent_attempt(tmp_path, preserved, kind):
    service, preflight, cloud, identity, create = setup(tmp_path)
    await create()
    cloud.delete_lose, cloud.delete_preserve = kind, kind if preserved else False
    request_id = str(uuid.uuid4())
    await service.remove(preflight.guide, identity, request_id, True)
    assert (await settled(service, preflight.guide, identity))["status"] == "removal-required"
    attempted = ("vm", "disk", "firewall").index(kind) + 1
    assert len(cloud.deletions()) == attempted
    if preserved and kind == "vm":
        assert cloud.vm["disks"][0]["autoDelete"] is False
    cloud.delete_lose, cloud.delete_preserve = None, False
    await service.remove(preflight.guide, identity, request_id, True)
    result = await settled(service, preflight.guide, identity)
    assert result["status"] == ("removal-required" if preserved else "removed")
    group = {"vm": "instances", "disk": "disks", "firewall": "firewall-rules"}[kind]
    assert len(cloud.deletions(group)) == 1
    assert len(cloud.deletions()) == (attempted if preserved else 3)
    if preserved:
        assert result["removal"]["resources"][kind]["status"] == "removing"
    assert "PRIVATE" not in json.dumps(result)


@pytest.mark.asyncio
async def test_spent_delete_attempt_stays_required_after_repeated_present_observation(tmp_path):
    service, preflight, cloud, identity, create = setup(tmp_path)
    await create()
    cloud.delete_preserve = True
    request_id = str(uuid.uuid4())
    for _ in range(2):
        await service.remove(preflight.guide, identity, request_id, True)
        result = await settled(service, preflight.guide, identity)
        assert result["status"] == "removal-required"
        assert result["removal"]["resources"]["vm"]["status"] == "removing"
    attempt = service._directory(preflight.guide) / (identity + ".removing-vm.attempt")
    attempt.unlink()
    with pytest.raises(DraftConflict): await service.remove(preflight.guide, identity, request_id, True)
    assert len(cloud.deletions()) == 1


@pytest.mark.asyncio
@pytest.mark.parametrize("absence", ["vm", "all"])
async def test_removal_observes_known_recorded_absence_without_replacing_resources(tmp_path, absence):
    service, preflight, cloud, identity, create = setup(tmp_path)
    await create()
    cloud.vm = None
    cloud.disk["users"] = []
    if absence == "all": cloud.disk = cloud.firewall = None
    await service.remove(preflight.guide, identity, str(uuid.uuid4()), True)
    assert (await settled(service, preflight.guide, identity))["status"] == "removed"
    assert len(cloud.deletions()) == (2 if absence == "vm" else 0)
    assert not cloud.deletions("instances") and len(cloud.mutations()) == 2


@pytest.mark.asyncio
async def test_shutdown_and_restart_keep_delete_intent_and_do_not_repeat_uncertain_mutation(tmp_path):
    service, preflight, cloud, identity, create = setup(tmp_path)
    ready = await create()
    cloud.delete_delay = asyncio.Event()
    request_id = str(uuid.uuid4())
    await service.remove(preflight.guide, identity, request_id, True)
    await cloud.delete_started.wait()
    await service.shutdown()
    assert (await service.status(preflight.guide, identity))["status"] == "removal-required"
    restored = GcpProvisioningService(service.root, preflight, command=cloud)
    restored.prepare_removal = service.prepare_removal
    cloud.delete_delay = None
    await restored.remove(preflight.guide, identity, request_id, True)
    assert (await settled(restored, preflight.guide, identity))["status"] == "removal-required"
    assert len(cloud.deletions()) == 1 and await restored.lifecycle(ready["endpoint"]) == "removal-required"
    cloud.vm = None
    cloud.disk["users"] = []
    await restored.remove(preflight.guide, identity, request_id, True)
    assert (await settled(restored, preflight.guide, identity))["status"] == "removed"
    assert len(cloud.deletions()) == 3 and len(cloud.deletions("instances")) == 1


@pytest.mark.asyncio
@pytest.mark.parametrize("endpoint", [None, {}, {"operation_id": "not-a-uuid"}])
async def test_lifecycle_requires_exact_saved_endpoint_and_makes_no_authority_call(tmp_path, endpoint):
    service, preflight, cloud, _, create = setup(tmp_path)
    await create()
    before = len(cloud.calls)
    with pytest.raises(DraftConflict): await service.lifecycle(endpoint)
    assert len(cloud.calls) == before


@pytest.mark.asyncio
@pytest.mark.parametrize("corrupt", ["ack", "request", "extra", "missing-kind", "status", "removed-unknown", "attempt"])
async def test_corrupt_removal_receipt_or_attempt_refuses_observation_without_new_delete(tmp_path, corrupt):
    service, preflight, cloud, identity, create = setup(tmp_path)
    if corrupt == "removed-unknown": cloud.lose = "unknown-spawn"
    await create()
    cloud.delete_preserve = True
    request_id = str(uuid.uuid4())
    await service.remove(preflight.guide, identity, request_id, True)
    await settled(service, preflight.guide, identity)
    directory = service._directory(preflight.guide)
    path = directory / (identity + ".json")
    value = json.loads(path.read_text())
    if corrupt == "ack": value["removal"]["acknowledge_data_loss"] = 1
    if corrupt == "request": value["removal"]["request_id"] = "PRIVATE_UUID_CANARY"
    if corrupt == "extra": value["removal"]["detail"] = "PRIVATE_DETAIL_CANARY"
    if corrupt == "missing-kind": del value["removal"]["resources"]["disk"]
    if corrupt == "status": value["removal"]["resources"]["vm"]["status"] = "deleted"
    if corrupt == "removed-unknown":
        value["status"] = "removed"
        for row in value["removal"]["resources"].values(): row["status"] = "absent"
    if corrupt == "attempt":
        attempt = directory / (identity + ".removing-vm.attempt")
        attempt.write_text(attempt.read_text().replace('"222"', '"444"'))
    else:
        path.write_text(json.dumps(value))
    before = len(cloud.deletions())
    with pytest.raises(DraftConflict) as error: await service.status(preflight.guide, identity)
    assert "PRIVATE" not in str(error.value)
    with pytest.raises(DraftConflict): await service.remove(preflight.guide, identity, request_id, True)
    assert len(cloud.deletions()) == before


@pytest.mark.asyncio
async def test_cloud_computer_creation_leaves_the_event_loop_free(tmp_path, monkeypatch):
    from launchkit import gcp_provisioning
    service, preflight, cloud, identity, create = setup(tmp_path)
    slow_cloud_package_work(monkeypatch, gcp_provisioning)
    result, gap = await longest_loop_gap(create())
    assert result["status"] == "vm-ready" and result["vm_verified"]
    assert gap < 0.5
