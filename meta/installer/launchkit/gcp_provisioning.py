"""Durable, fixed-plan cloud admission with explicit uncertain-result recovery.

CLI success is never application readiness. Observations join the exact billed
resources to the original generation; no existing foreign resource is adopted.
"""
from __future__ import annotations

import asyncio
import base64
from contextlib import contextmanager
from datetime import datetime, timezone
import fcntl
import hashlib
import ipaddress
import json
import os
from pathlib import Path
import re
import stat
import uuid

from .gcp_preflight import (CommandResult, GcloudReadOnly,
    _account, _project, _json)
from .gcp_bootstrap import bootstrap_script
from .deployment import await_validation, package_files
from .projects import DraftConflict, fingerprint
from .remote_deployment import private_file, write_bytes, write_private

MAX_RECEIPT = 64 * 1024
MAX_RESOURCES = 2
NAME = re.compile(r"[a-z][a-z0-9-]{0,61}[a-z0-9]\Z")
ZONE = re.compile(r"[a-z]+-[a-z0-9]+[0-9]-[a-z]\Z")
MACHINE = re.compile(r"[a-z0-9][a-z0-9-]{0,62}\Z")
NUMBER = re.compile(r"[1-9][0-9]{0,19}\Z")
SHA = re.compile(r"[0-9a-f]{64}\Z")
DETAILS = {
    "provisioning": "Preparing the cloud VM. The application has not been installed.",
    "resume-required": "The cloud result could not be confirmed. Keep this request and explicitly resume its observation before continuing.",
    "blocked": "The saved source, selected account, cloud prerequisites or resource ownership could not be verified. No replacement resource was requested.",
    "vm-ready": "The owned cloud VM and access rule were observed. The application has not been installed or tested.",
    "removing": "Removing this saved cloud trial. Its application data will be deleted.",
    "removal-required": "Cloud removal could not be confirmed. Preserve this request and recheck it. An uncertain delete is not repeated.",
    "removed": "The original cloud trial resources were independently observed absent. The saved receipt is preserved.",
}
REMOVAL_STATUSES = {"removing", "removal-required", "removed"}
REMOVAL_KINDS = ("vm", "disk", "firewall")


def _now():
    return datetime.now(timezone.utc).isoformat()


def _resource_id(value):
    return isinstance(value, str) and NUMBER.fullmatch(value) is not None and int(value) <= 2 ** 64 - 1


def _uuid(value):
    try:
        identity = uuid.UUID(value)
        if str(identity) != value or identity.version != 4:
            raise ValueError()
        return value
    except (ValueError, TypeError, AttributeError):
        raise ValueError("Use the same client-generated UUID for this cloud request.") from None


def _folder(path):
    path = Path(path)
    for parent in (*reversed(path.parents), path):
        if parent.is_symlink():
            raise DraftConflict("The cloud receipt folder must be ordinary and owned.")
    path.mkdir(parents=True, exist_ok=True, mode=0o700)
    info = path.stat()
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077:
        raise DraftConflict("The cloud receipt folder must be private and owned.")
    return path


@contextmanager
def _lock(directory):
    path = directory / "provision.lock"
    descriptor = os.open(path, os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    try:
        info = os.fstat(descriptor)
        if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_nlink != 1 or info.st_mode & 0o077:
            raise DraftConflict("The cloud receipt lock must be private and owned.")
        try:
            fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            raise DraftConflict("This cloud request is still owned by another coordinator.") from None
        yield
    finally:
        os.close(descriptor)


def _access(value, operation_id=None):
    if not isinstance(value, dict) or set(value) != {"user", "public_key"} or value.get("user") != "mobkit":
        raise DraftConflict("Prepare the host-owned SSH identity before creating the VM.")
    key = value.get("public_key")
    if not isinstance(key, str) or len(key) > 4096 or not key.startswith("ssh-ed25519 "):
        raise DraftConflict("The prepared SSH public key could not be verified.")
    parts = key.split(" ")
    try:
        if len(parts) == 3:
            if parts[2] != "mobkit-launch:" + _uuid(operation_id):
                raise ValueError()
            parts = parts[:2]
        if len(parts) != 2 or base64.b64encode(base64.b64decode(parts[1], validate=True)).decode() != parts[1]:
            raise ValueError()
        body = base64.b64decode(parts[1], validate=True)
        if len(body) != 51 or body[:19] != b"\x00\x00\x00\x0bssh-ed25519\x00\x00\x00\x20":
            raise ValueError()
    except (ValueError, TypeError):
        raise DraftConflict("The prepared SSH public key could not be verified.") from None
    return {"user": "mobkit", "public_key": " ".join(parts)}


def _link(value, suffix):
    return value in {"https://www.googleapis.com/compute/v1" + suffix,
        "https://compute.googleapis.com/compute/v1" + suffix}


class _Unknown(RuntimeError):
    pass


class GcpProvisioningService:
    def __init__(self, root, preflight, *, command=None):
        self.root = Path(root)
        self.preflight = preflight
        self.command = command if command is not None else GcloudReadOnly(timeout=180)
        self.prepare_access = None
        self.prepare_removal = None
        self.tasks = {}
        self.admission = asyncio.Lock()
        self.closed = False

    def _directory(self, guide_id):
        _uuid(guide_id)
        _folder(self.root)
        _folder(self.root / "gcp-provisioning")
        return _folder(self.root / "gcp-provisioning" / guide_id)

    def _read(self, directory, operation_id=None):
        try:
            selected = operation_id is None
            if selected:
                latest = directory / "latest.json"
                if not latest.exists():
                    saved = []
                    for path in directory.glob("*.json"):
                        if path.name != "latest.json":
                            saved.append(path.stem)
                        if len(saved) > 1:
                            raise ValueError()
                    if not saved:
                        return None
                    operation_id = saved[0]
                else:
                    selector = json.loads(private_file(latest, 4096))
                    if not isinstance(selector, dict) or set(selector) != {"operation_id"}:
                        raise ValueError()
                    operation_id = selector["operation_id"]
            _uuid(operation_id)
            path = directory / (operation_id + ".json")
            if not path.exists():
                if selected:
                    raise ValueError()
                return None
            value = json.loads(private_file(path, MAX_RECEIPT))
            self._saved(value, directory.name, operation_id)
            spent = self._attempts(directory, value)
            self._removal_attempts(directory, value)
            if value["status"] == "removed" and any(
                    ("creating-firewall" if kind == "firewall" else "creating-vm") in spent and kind not in value["resources"]
                    for kind in REMOVAL_KINDS):
                raise ValueError()
            return value
        except (ValueError, TypeError, KeyError, AttributeError, OSError, RecursionError):
            raise DraftConflict("The saved cloud receipt could not be verified. Existing resources are not replaced.") from None

    def _saved(self, value, guide_id, operation_id):
        if (not isinstance(value, dict) or type(value.get("protocol")) is not int or value["protocol"] != 1 or
                value.get("operation_id") != operation_id or value.get("guide_id") != guide_id or
                value.get("status") not in DETAILS or value.get("phase") not in {"checking", "creating-firewall", "creating-vm", "vm-ready"}):
            raise ValueError()
        request, binding, destination = value["request"], value["binding"], value["destination"]
        if (not isinstance(request, dict) or set(request) != {"guide_id", "version", "build_id", "account", "project"} or
                request["guide_id"] != guide_id or type(request["version"]) is not int or request["version"] < 1 or
                not isinstance(request["build_id"], str) or not 1 <= len(request["build_id"]) <= 128 or
                any(ord(c) < 33 or ord(c) > 126 for c in request["build_id"]) or
                _account(request["account"]) != value["account"] or _project(request["project"]) != destination["project"] or
                fingerprint(request) != value["request_fingerprint"] or
                fingerprint({"account": value["account"], "project": destination["project"]}) != value["account_fingerprint"] or
                value["ownership_token"] != uuid.UUID(operation_id).hex):
            raise ValueError()
        if (not isinstance(binding, dict) or set(binding) != {"guide_id", "guide_version", "build_id", "blueprint_hash", "source_digest", "package_sha256"} or
                binding["guide_id"] != guide_id or type(binding["guide_version"]) is not int or binding["guide_version"] != request["version"] or
                binding["build_id"] != request["build_id"] or
                any(not isinstance(binding[key], str) or not SHA.fullmatch(binding[key]) for key in ["blueprint_hash", "source_digest", "package_sha256"])):
            raise ValueError()
        self._destination(destination)
        access = _access(value["access"])
        script = value["bootstrap_script"]
        if (not isinstance(script, str) or not 1 <= len(script) <= 8192 or
                value["bootstrap_sha256"] != hashlib.sha256(script.encode()).hexdigest() or
                value["public_metadata_sha256"] != fingerprint({"enable-guest-attributes": "TRUE",
                    "ssh-keys": access["user"] + ":" + access["public_key"], "startup-script": script})):
            raise ValueError()
        resources = value["resources"]
        if not isinstance(resources, dict) or set(resources) - {"firewall", "vm", "disk"}:
            raise ValueError()
        phase, pending = value["phase"], value["pending"]
        if pending is not None:
            if (not isinstance(pending, dict) or set(pending) != {"phase", "started_at"} or
                    phase not in {"creating-firewall", "creating-vm"} or pending["phase"] != phase or
                    not isinstance(pending["started_at"], str) or len(pending["started_at"]) > 64 or
                    datetime.fromisoformat(pending["started_at"]).utcoffset() is None):
                raise ValueError()
        if (phase == "checking" and resources or
                phase == "creating-firewall" and pending is None and "firewall" not in resources or
                phase == "creating-vm" and ("firewall" not in resources or pending is None and set(resources) != {"firewall", "vm", "disk"}) or
                phase == "vm-ready" and (pending is not None or "endpoint" not in value) or
                value["status"] == "vm-ready" and phase != "vm-ready"):
            raise ValueError()
        for kind, row in resources.items():
            expected_name = destination["instance"] + "-iap-ssh" if kind == "firewall" else destination["instance"]
            suffix = "/projects/" + destination["project"] + ("/global/firewalls/" + expected_name if kind == "firewall" else
                "/zones/" + destination["zone"] + "/" + ("instances/" if kind == "vm" else "disks/") + expected_name)
            if (not isinstance(row, dict) or set(row) != {"id", "name", "self_link"} or
                    not _resource_id(row.get("id")) or
                    row.get("name") != expected_name or not _link(row.get("self_link"), suffix)):
                raise ValueError()
        if value["status"] == "vm-ready" or "endpoint" in value:
            if set(resources) != {"firewall", "vm", "disk"} or value.get("endpoint") != self._endpoint(value, resources):
                raise ValueError()
        removal = value.get("removal")
        if removal is not None:
            if (not isinstance(removal, dict) or set(removal) != {"request_id", "acknowledge_data_loss", "resources"} or
                    _uuid(removal.get("request_id")) != removal["request_id"] or removal.get("acknowledge_data_loss") is not True or
                    value["status"] not in REMOVAL_STATUSES or not isinstance(removal.get("resources"), dict) or
                    set(removal["resources"]) != set(REMOVAL_KINDS)):
                raise ValueError()
            for item in removal["resources"].values():
                if (not isinstance(item, dict) or set(item) != {"status"} or
                        item["status"] not in {"waiting", "checking", "removing", "absent"}):
                    raise ValueError()
            if value["status"] == "removed" and any(item["status"] != "absent" for item in removal["resources"].values()):
                raise ValueError()
        elif "removal" in value or value["status"] in REMOVAL_STATUSES:
            raise ValueError()

    def _save(self, directory, value):
        value["updated_at"] = _now()
        write_private(directory / (value["operation_id"] + ".json"), value)
        descriptor = os.open(directory, os.O_RDONLY)
        try:
            os.fsync(descriptor)
        finally:
            os.close(descriptor)

    def _public(self, value):
        if value is None:
            return None
        keys = ("protocol", "operation_id", "guide_id", "account", "status", "phase", "binding", "destination", "resources", "endpoint", "removal")
        result = {key: value[key] for key in keys if key in value}
        result.update(detail=DETAILS[value["status"]], resume_required=value["status"] == "resume-required",
            vm_verified=value["status"] == "vm-ready", application_installed=False,
            application_status="not-installed")
        return json.loads(json.dumps(result))

    async def create(self, guide_id, version, build_id, account, project, request_id):
        request_id, account, project = _uuid(request_id), _account(account), _project(project)
        request = {"guide_id": guide_id, "version": version, "build_id": build_id, "account": account, "project": project}
        if (type(version) is not int or version < 1 or not isinstance(build_id, str) or
                not 1 <= len(build_id) <= 128 or any(ord(c) < 33 or ord(c) > 126 for c in build_id)):
            raise ValueError("Use the current accepted application source for cloud preparation.")
        async with self.admission:
            if self.closed:
                raise DraftConflict("Cloud preparation is shutting down.")
            directory = self._directory(guide_id)
            active = self.tasks.get(request_id)
            if active is not None:
                old = self._read(directory, request_id)
                if not old or old["request_fingerprint"] != fingerprint(request):
                    raise DraftConflict("This cloud request ID already belongs to a different source or account.")
                task = active
            else:
                task = None
            if task is None:
                task = await self._admit_create(directory, request, request_id)
        # Admission does not wait for VM creation. The durable identity survives
        # a lost HTTP response, and status observes the owned background task.
        return self._public(self._read(directory, request_id))

    async def _admit_create(self, directory, request, request_id):
        guide_id, version, build_id, account, project = (request[key] for key in ("guide_id", "version", "build_id", "account", "project"))
        with _lock(directory):
            old = self._read(directory, request_id)
            if old:
                if old["request_fingerprint"] != fingerprint(request):
                    raise DraftConflict("This cloud request ID already belongs to a different source or account.")
                task = self.tasks.get(request_id)
                if task is None:
                    self._interrupted(directory, old)
                    return None
            else:
                latest = self._read(directory)
                if latest:
                    raise DraftConflict("Resume the saved cloud request before creating any other resources for this setup.")
                binding, destination, supported = await self.preflight._snapshot(guide_id, build_id, version)
                if self.closed:
                    raise DraftConflict("Cloud preparation is shutting down.")
                if not supported or destination.get("project") != project:
                    raise DraftConflict("The accepted package does not match this fixed cloud plan.")
                self._destination(destination)
                script = await self._package_bootstrap(binding)
                if self.prepare_access is None:
                    raise DraftConflict("Prepare the host-owned SSH identity before creating the VM.")
                access = _access(await self.prepare_access({"binding": binding, "destination": destination, "operation_id": request_id}), request_id)
                if self.closed:
                    raise DraftConflict("Cloud preparation is shutting down.")
                value = {"protocol": 1, "operation_id": request_id, "guide_id": guide_id,
                    "request": request, "request_fingerprint": fingerprint(request), "binding": binding,
                    "destination": destination, "account": account, "account_fingerprint": fingerprint({"account": account, "project": project}),
                    "ownership_token": uuid.UUID(request_id).hex, "access": access,
                    "bootstrap_script": script, "bootstrap_sha256": hashlib.sha256(script.encode()).hexdigest(),
                    "public_metadata_sha256": fingerprint({"enable-guest-attributes": "TRUE", "ssh-keys": access["user"] + ":" + access["public_key"],
                        "startup-script": script}),
                    "status": "provisioning", "phase": "checking", "pending": None, "created_at": _now(), "resources": {}}
                self._save(directory, value)
                write_private(directory / "latest.json", {"operation_id": request_id})
                task = self._start(directory, value)
        return task

    async def _package_bootstrap(self, binding):
        """Derive new guest setup only from the exact admitted package bytes."""
        payload, _ = await self.preflight.local.implementation.package(binding["guide_id"], binding["build_id"])
        # Hashing and unpacking the whole package must not hold the event loop.
        if await await_validation(lambda: hashlib.sha256(payload).hexdigest()) != binding["package_sha256"]:
            raise DraftConflict("The accepted package changed before guest preparation. No cloud resources were requested.")
        try:
            definition = json.loads((await await_validation(package_files, payload))["deployment.json"][0])
            if definition.get("target") != "gcp":
                raise ValueError()
            script = bootstrap_script(definition)
            if len(script.encode()) > 8192:
                raise ValueError()
            return script
        except (ValueError, TypeError, KeyError, AttributeError):
            raise DraftConflict("The checked package's guest prerequisites could not be verified. No cloud resources were requested.") from None

    def _destination(self, value):
        if (not isinstance(value, dict) or set(value) != {"project", "instance", "zone", "machine_type", "disk_gb", "console_port"} or
                not PROJECT_VALID(value.get("project")) or
                not isinstance(value.get("instance"), str) or not NAME.fullmatch(value["instance"]) or
                not isinstance(value.get("zone"), str) or not ZONE.fullmatch(value["zone"]) or
                not isinstance(value.get("machine_type"), str) or not MACHINE.fullmatch(value["machine_type"]) or
                type(value.get("disk_gb")) is not int or not 10 <= value["disk_gb"] <= 65536 or
                type(value.get("console_port")) is not int or not 1 <= value["console_port"] <= 65535):
            raise DraftConflict("The accepted fixed cloud destination could not be verified.")

    def _interrupted(self, directory, value):
        if value["status"] in {"provisioning", "removing"}:
            value["status"] = "removal-required" if "removal" in value else "resume-required"
            self._save(directory, value)
        return value

    def _start(self, directory, value):
        task = asyncio.create_task(self._run_removal(directory, value) if "removal" in value else self._run(directory, value))
        self.tasks[value["operation_id"]] = task
        task.add_done_callback(lambda done: done.exception() if not done.cancelled() else None)
        return task

    async def status(self, guide_id, operation_id=None):
        directory = self._directory(guide_id)
        value = self._read(directory, operation_id)
        if value and value["operation_id"] in self.tasks:
            return self._public(value)
        with _lock(directory):
            value = self._read(directory, operation_id)
            if value and value["operation_id"] not in self.tasks:
                value = self._interrupted(directory, value)
            return self._public(value)

    async def resume(self, guide_id, operation_id):
        operation_id = _uuid(operation_id)
        async with self.admission:
            if self.closed:
                raise DraftConflict("Cloud preparation is shutting down.")
            directory = self._directory(guide_id)
            value = self._read(directory, operation_id)
            if value and "removal" in value:
                raise DraftConflict("This cloud trial has a removal request. Recheck that exact removal instead of resuming creation.")
            task = self.tasks.get(operation_id)
            if task is not None:
                return self._public(self._read(directory, operation_id))
            with _lock(directory):
                value = self._read(directory, operation_id)
                if not value:
                    raise DraftConflict("The original cloud request receipt is required to resume.")
                if "removal" in value:
                    raise DraftConflict("This cloud trial has a removal request. Recheck that exact removal instead of resuming creation.")
                task = self.tasks.get(operation_id)
                if task is None:
                    value["status"] = "provisioning"
                    self._save(directory, value)
                    task = self._start(directory, value)
        return self._public(self._read(directory, operation_id))

    async def _cli(self, value, parts, *, allow_empty=False):
        if getattr(self.command, "cleanup_pending", False):
            raise _Unknown()
        arguments = [*parts, "--account=" + value["account"], "--project=" + value["destination"]["project"],
            "--quiet", "--verbosity=error", "--no-log-http"]
        try:
            result = await self.command.command(arguments)
            if not isinstance(result, CommandResult):
                result = CommandResult(*result)
            if result.code != 0 or getattr(self.command, "cleanup_pending", False):
                raise _Unknown()
            return None if allow_empty and not result.output.strip() else _json(result.output)
        except asyncio.CancelledError:
            raise
        except Exception:
            raise _Unknown() from None

    async def _fresh(self, value, *, owned):
        request = value["request"]
        snapshot = await self.preflight._snapshot(value["guide_id"], request["build_id"], request["version"])
        if snapshot != (value["binding"], value["destination"], True):
            raise DraftConflict("The accepted cloud source changed.")
        checked = await self.preflight.check_guided(value["guide_id"], request["version"], request["build_id"], value["account"], request["project"])
        permitted = {"instance-collision" if owned.get("vm") else "", "firewall-collision" if owned.get("firewall") else ""}
        if owned.get("vm") and owned.get("firewall"):
            # These headroom checks ask for an additional VM/rule. No resource
            # mutation remains once this generation's complete pair is owned.
            permitted.update({"regional-quota", "project-quota"})
        if (checked.get("guided_supported") is not True or checked.get("binding") != value["binding"] or
                checked.get("destination") != value["destination"] or checked.get("account") != value["account"] or
                checked.get("project") != request["project"] or not checked.get("checks") or
                any(row.get("status") != "checked" for row in checked["checks"] if row.get("required", True) and row.get("id") not in permitted)):
            raise DraftConflict("The current cloud prerequisites changed.")
        latest = await self.preflight._snapshot(value["guide_id"], request["build_id"], request["version"])
        if latest != snapshot:
            raise DraftConflict("The accepted cloud source changed during preparation.")

    async def _rows(self, value, kind):
        d = value["destination"]
        name = d["instance"] + "-iap-ssh" if kind == "firewall" else d["instance"]
        group = "firewall-rules" if kind == "firewall" else "instances" if kind == "vm" else "disks"
        fields = {"firewall": "id,name,selfLink,description,network,direction,disabled,priority,targetTags,sourceRanges,allowed,denied,sourceTags,sourceServiceAccounts,targetServiceAccounts,destinationRanges",
            "vm": "id,name,selfLink,zone,machineType,status,labels,tags,serviceAccounts,metadata,networkInterfaces,disks,shieldedInstanceConfig",
            "disk": "id,name,selfLink,zone,type,sizeGb,sourceImage,users,status"}[kind]
        parts = ["compute", group, "list", "--filter=name=" + name, "--limit=3", "--format=json(" + fields + ")"]
        if kind != "firewall":
            parts.append("--zones=" + d["zone"])
        rows = await self._cli(value, parts)
        if not isinstance(rows, list) or len(rows) > MAX_RESOURCES or any(not isinstance(row, dict) or row.get("name") != name for row in rows) or len(rows) > 1:
            raise _Unknown()
        return rows[0] if rows else None

    def _resource(self, value, row, kind):
        d = value["destination"]
        if not _resource_id(row.get("id")):
            raise DraftConflict("The observed cloud resource identity could not be verified.")
        base = "/projects/" + d["project"]
        suffix = base + ("/global/firewalls/" + d["instance"] + "-iap-ssh" if kind == "firewall" else "/zones/" + d["zone"] + "/" + ("instances/" if kind == "vm" else "disks/") + d["instance"])
        if not _link(row.get("selfLink"), suffix):
            raise DraftConflict("The cloud resource belongs to a different destination.")
        recorded = value.get("resources", {}).get(kind)
        if recorded and recorded["id"] != row["id"]:
            raise DraftConflict("The original cloud resource identity changed.")
        if kind == "firewall":
            if (row.get("description") != "MobKit Launch deployment " + value["ownership_token"] or
                    not _link(row.get("network"), base + "/global/networks/default") or
                    row.get("direction") != "INGRESS" or row.get("disabled", False) is not False or
                    row.get("priority") != 1000 or row.get("targetTags") != [d["instance"] + "-iap"] or
                    row.get("sourceRanges") != ["35.235.240.0/20"] or
                    row.get("allowed") != [{"IPProtocol": "tcp", "ports": ["22"]}] or
                    any(row.get(field) for field in ["denied", "sourceTags", "sourceServiceAccounts", "targetServiceAccounts", "destinationRanges"])):
                raise DraftConflict("The cloud access rule has foreign ownership or a different scope.")
        elif kind == "vm":
            metadata = row.get("metadata", {}).get("items", [])
            items = {item.get("key"): item.get("value") for item in metadata if isinstance(item, dict)}
            network = row.get("networkInterfaces", [])
            access = value["access"]
            if (row.get("labels", {}).get("mobkit_launch") != value["ownership_token"] or
                    not _link(row.get("zone"), base + "/zones/" + d["zone"]) or
                    not _link(row.get("machineType"), base + "/zones/" + d["zone"] + "/machineTypes/" + d["machine_type"]) or
                    row.get("tags", {}).get("items") != [d["instance"] + "-iap"] or row.get("serviceAccounts", []) != [] or
                    len(metadata) != 3 or set(items) != {"enable-guest-attributes", "ssh-keys", "startup-script"} or
                    items.get("enable-guest-attributes") != "TRUE" or items.get("ssh-keys") != access["user"] + ":" + access["public_key"] or
                    items.get("startup-script") != value["bootstrap_script"] or
                    row.get("shieldedInstanceConfig", {}).get("enableSecureBoot") is not True or
                    len(network) != 1 or not _link(network[0].get("network"), base + "/global/networks/default") or
                    network[0].get("ipv6AccessConfigs") or network[0].get("externalIpv6") or
                    len(network[0].get("accessConfigs", [])) != 1 or network[0]["accessConfigs"][0].get("type") != "ONE_TO_ONE_NAT"):
                raise DraftConflict("The VM has foreign ownership or differs from the accepted fixed plan.")
            try:
                ipaddress.IPv4Address(network[0]["accessConfigs"][0]["natIP"])
            except (ValueError, KeyError, TypeError):
                raise DraftConflict("The VM's accepted external IPv4 access could not be verified.") from None
        else:
            if (not _link(row.get("zone"), base + "/zones/" + d["zone"]) or
                    not _link(row.get("type"), base + "/zones/" + d["zone"] + "/diskTypes/pd-balanced") or
                    str(row.get("sizeGb")) != str(d["disk_gb"]) or
                    not re.search(r"/projects/debian-cloud/global/images/debian-12-[a-zA-Z0-9-]+$", str(row.get("sourceImage", "")))):
                raise DraftConflict("The owned boot disk differs from the accepted fixed plan.")
        return {"id": row["id"], "name": row["name"], "self_link": row["selfLink"]}

    def _endpoint(self, value, owned):
        d = value["destination"]
        return {"transport": "gcp-iap", "operation_id": value["operation_id"], "host": "compute." + owned["vm"]["id"],
            "ssh_port": 22, "account": value["account"], "project": d["project"], "zone": d["zone"],
            "instance": d["instance"], "instance_id": owned["vm"]["id"], "ownership_token": value["ownership_token"], "user": value["access"]["user"]}

    async def _observe(self, value):
        firewall, vm, disk = await self._rows(value, "firewall"), await self._rows(value, "vm"), await self._rows(value, "disk")
        owned = {}
        for kind, row in (("firewall", firewall), ("vm", vm), ("disk", disk)):
            if row is not None:
                owned[kind] = self._resource(value, row, kind)
        if disk and not vm or vm and not disk:
            raise DraftConflict("The original VM and boot disk could not be joined.")
        if vm and disk:
            disks = vm.get("disks", [])
            if (len(disks) != 1 or disks[0].get("boot") is not True or disks[0].get("autoDelete") is not True or
                    disks[0].get("source") != disk["selfLink"] or disk.get("users") != [vm["selfLink"]]):
                raise DraftConflict("The VM and original boot disk could not be joined.")
            owned["running"] = vm.get("status") == "RUNNING" and disk.get("status") == "READY"
        for kind in value.get("resources", {}):
            if kind not in owned:
                raise DraftConflict("An original cloud resource is missing. Replacement resources are not created.")
        return owned

    async def remove(self, guide_id, operation_id, request_id, acknowledge_data_loss):
        operation_id, request_id = _uuid(operation_id), _uuid(request_id)
        if acknowledge_data_loss is not True:
            raise ValueError("Explicitly acknowledge deletion of this cloud trial's application data.")
        async with self.admission:
            if self.closed:
                raise DraftConflict("Cloud preparation is shutting down.")
            directory = self._directory(guide_id)
            active = self.tasks.get(operation_id)
            if active:
                value = self._read(directory, operation_id)
                if not value or "removal" not in value:
                    raise DraftConflict("The original cloud creation command must finish before removal can be admitted.")
                if value["removal"]["request_id"] != request_id:
                    raise DraftConflict("Resume the original cloud removal request. Its identity cannot be replaced.")
                return self._public(value)
            with _lock(directory):
                value = self._read(directory, operation_id)
                if not value:
                    raise DraftConflict("The original cloud receipt is required for removal.")
                removal = value.get("removal")
                if removal and removal["request_id"] != request_id:
                    raise DraftConflict("Resume the original cloud removal request. Its identity cannot be replaced.")
                active = self.tasks.get(operation_id)
                if active and not removal:
                    raise DraftConflict("The original cloud creation command must finish before removal can be admitted.")
                if not removal:
                    value["removal"] = {"request_id": request_id, "acknowledge_data_loss": True,
                        "resources": {kind: {"status": "waiting"} for kind in REMOVAL_KINDS}}
                if not active and value["status"] != "removed":
                    value["status"] = "removing"
                    self._save(directory, value)
                    self._start(directory, value)
            return self._public(self._read(directory, operation_id))

    async def _original_account(self, value):
        accounts, checks, _ = await self.preflight._accounts(value["account"])
        if (not checks or any(row.get("status") != "checked" for row in checks) or
                value["account"] not in {row.get("id", row.get("account")) for row in accounts}):
            raise DraftConflict("The original selected cloud account could not be verified.")

    def _removal_attempt_path(self, directory, value, kind):
        return directory / (value["operation_id"] + ".removing-" + kind + ".attempt")

    def _removal_attempt_content(self, value, kind):
        return (json.dumps({"protocol": 1, "operation_id": value["operation_id"], "guide_id": value["guide_id"],
            "request_id": value["removal"]["request_id"], "account_fingerprint": value["account_fingerprint"],
            "ownership_token": value["ownership_token"], "kind": kind, "resource": value["resources"][kind]},
            sort_keys=True, separators=(",", ":")) + "\n").encode()

    def _removal_attempts(self, directory, value):
        spent = set()
        for kind in REMOVAL_KINDS:
            path = self._removal_attempt_path(directory, value, kind)
            if path.exists() or path.is_symlink():
                if "removal" not in value or kind not in value["resources"]:
                    raise DraftConflict("The original cloud removal intent could not be verified.")
                if private_file(path, 8192) != self._removal_attempt_content(value, kind):
                    raise DraftConflict("The original cloud removal intent changed. No delete was repeated.")
                spent.add(kind)
            if "removal" in value and value["removal"]["resources"][kind]["status"] == "removing" and kind not in spent:
                raise DraftConflict("The original cloud removal intent is missing.")
        return spent

    def _reserve_removal(self, directory, value, kind):
        path = self._removal_attempt_path(directory, value, kind)
        try:
            descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
        except FileExistsError:
            self._removal_attempts(directory, value)
            raise _Unknown() from None
        with os.fdopen(descriptor, "wb") as stream:
            stream.write(self._removal_attempt_content(value, kind))
            stream.flush()
            os.fsync(stream.fileno())
        descriptor = os.open(directory, os.O_RDONLY)
        try:
            os.fsync(descriptor)
        finally:
            os.close(descriptor)

    async def _observe_removal(self, directory, value):
        spent = self._attempts(directory, value)
        rows = {kind: await self._rows(value, kind) for kind in REMOVAL_KINDS}
        owned = {}
        for kind in ("firewall", "vm"):
            if rows[kind] is not None:
                phase = "creating-firewall" if kind == "firewall" else "creating-vm"
                if phase not in spent:
                    raise DraftConflict("The observed cloud resource has no original creation intent.")
                owned[kind] = self._resource(value, rows[kind], kind)
        vm, disk = rows["vm"], rows["disk"]
        if disk is not None:
            if "disk" not in value["resources"] and vm is None:
                raise DraftConflict("An unrecorded orphan disk has no verified original ownership.")
            owned["disk"] = self._resource(value, disk, "disk")
        if vm is not None:
            disks = vm.get("disks", [])
            if (disk is None or len(disks) != 1 or disks[0].get("boot") is not True or
                    type(disks[0].get("autoDelete")) is not bool or disks[0].get("source") != disk["selfLink"] or
                    disk.get("users") != [vm["selfLink"]]):
                raise DraftConflict("The original VM and its sole original boot disk could not be joined for removal.")
        elif disk is not None and disk.get("users") != []:
            raise DraftConflict("The original disk is still attached. It cannot be removed.")
        unresolved = set()
        for kind in REMOVAL_KINDS:
            item = value["removal"]["resources"][kind]
            if kind in owned:
                if item["status"] == "absent":
                    raise DraftConflict("An original resource reappeared after confirmed absence.")
                value["resources"].setdefault(kind, owned[kind])
            else:
                phase = "creating-firewall" if kind == "firewall" else "creating-vm"
                if phase in spent and kind not in value["resources"]:
                    unresolved.add(kind)
                else:
                    item["status"] = "absent"
        self._save(directory, value)
        return owned, unresolved

    async def _run_removal(self, directory, value):
        identity = value["operation_id"]
        try:
            # Remote coordination joins local custody and stops owned tunnels.
            # The durable removal intent is visible, but no cloud flock is held.
            if self.prepare_removal is None or await self.prepare_removal(json.loads(json.dumps(value))) is not True:
                raise DraftConflict("Owned remote operations and browser connections must be coordinated before removal.")
            with _lock(directory):
                value = self._read(directory, identity)
                for kind in REMOVAL_KINDS:
                    spent = self._removal_attempts(directory, value)
                    if value["removal"]["resources"][kind]["status"] != "absent":
                        value["removal"]["resources"][kind]["status"] = "removing" if kind in spent else "checking"
                        self._save(directory, value)
                    await self._original_account(value)
                    owned, unresolved = await self._observe_removal(directory, value)
                    if kind not in owned:
                        continue
                    if kind in spent:
                        raise _Unknown()
                    if kind == "disk" and "vm" in owned:
                        raise DraftConflict("The original disk is still attached. It cannot be removed.")
                    d = value["destination"]
                    group = {"vm": "instances", "disk": "disks", "firewall": "firewall-rules"}[kind]
                    parts = ["compute", group, "delete", owned[kind]["name"], "--format=json"]
                    if kind != "firewall":
                        parts.append("--zone=" + d["zone"])
                    if kind == "vm":
                        parts.append("--keep-disks=all")
                    self._reserve_removal(directory, value, kind)
                    value["removal"]["resources"][kind]["status"] = "removing"
                    self._save(directory, value)
                    await self._cli(value, parts, allow_empty=True)
                    owned, unresolved = await self._observe_removal(directory, value)
                    if kind in owned:
                        raise _Unknown()
                if unresolved or any(item["status"] != "absent" for item in value["removal"]["resources"].values()):
                    raise _Unknown()
                value["status"] = "removed"
                self._save(directory, value)
                return self._public(value)
        except asyncio.CancelledError:
            with _lock(directory):
                value = self._read(directory, identity)
                value["status"] = "removal-required"
                self._save(directory, value)
            raise
        except Exception:
            with _lock(directory):
                value = self._read(directory, identity)
                value["status"] = "removal-required"
                self._save(directory, value)
            return self._public(value)
        finally:
            self.tasks.pop(identity, None)

    def _attempt(self, value, phase):
        kind = "firewall" if phase == "creating-firewall" else "vm"
        return {"protocol": 1, "operation_id": value["operation_id"], "guide_id": value["guide_id"],
            "request_fingerprint": value["request_fingerprint"], "binding": value["binding"],
            "account_fingerprint": value["account_fingerprint"], "destination": value["destination"],
            "ownership_token": value["ownership_token"], "public_metadata_sha256": value["public_metadata_sha256"],
            "phase": phase, "resource": {"kind": kind, "name": value["destination"]["instance"] + ("-iap-ssh" if kind == "firewall" else "")}}

    def _attempt_path(self, directory, value, phase):
        return directory / (value["operation_id"] + "." + phase + ".attempt")

    def _attempt_content(self, value, phase):
        return (json.dumps(self._attempt(value, phase), sort_keys=True, separators=(",", ":")) + "\n").encode()

    def _attempts(self, directory, value):
        spent = set()
        for phase in ("creating-firewall", "creating-vm"):
            path = self._attempt_path(directory, value, phase)
            if path.exists() or path.is_symlink():
                try:
                    record = private_file(path, 8192)
                except (ValueError, TypeError, OSError, DraftConflict):
                    raise DraftConflict("The original cloud mutation intent could not be verified. No replacement resource was requested.") from None
                if record != self._attempt_content(value, phase):
                    raise DraftConflict("The original cloud mutation intent changed. No replacement resource was requested.")
                spent.add(phase)
        required = set()
        if value["phase"] != "checking":
            required.add("creating-firewall")
        if value["phase"] in {"creating-vm", "vm-ready"}:
            required.add("creating-vm")
        if not required <= spent:
            raise DraftConflict("The original cloud mutation intent is missing. Existing resources are not replaced.")
        return spent

    def _reserve_mutation(self, directory, value, phase):
        path = self._attempt_path(directory, value, phase)
        content = self._attempt_content(value, phase)
        try:
            descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
        except FileExistsError:
            self._attempts(directory, value)
            raise _Unknown() from None
        with os.fdopen(descriptor, "wb") as stream:
            stream.write(content)
            stream.flush()
            os.fsync(stream.fileno())
        descriptor = os.open(directory, os.O_RDONLY)
        try:
            os.fsync(descriptor)
        finally:
            os.close(descriptor)

    async def _mutation(self, directory, value, phase, parts):
        self._reserve_mutation(directory, value, phase)
        value.update(phase=phase, pending={"phase": phase, "started_at": _now()})
        self._save(directory, value)
        await self._cli(value, parts)
        # Do not clear pending until an independent collection observation joins
        # the exact generation. CLI return code alone is not resource evidence.

    def _metadata_files(self, directory, value):
        folder = _folder(directory / (value["operation_id"] + "-metadata"))
        if any(c in str(folder) for c in [",", "\n", "\r", "\x00"]):
            raise DraftConflict("The host-owned metadata file path cannot be represented by this cloud adapter.")
        content = {"ssh-keys": value["access"]["user"] + ":" + value["access"]["public_key"],
            "startup-script": value["bootstrap_script"]}
        for name, text in content.items():
            path = folder / name
            if path.exists():
                if private_file(path, 8192) != text.encode():
                    raise DraftConflict("The saved cloud metadata files changed. Existing resources are not replaced.")
            else:
                write_bytes(path, text.encode())
        descriptor = os.open(folder, os.O_RDONLY)
        try:
            os.fsync(descriptor)
        finally:
            os.close(descriptor)
        return "--metadata-from-file=" + ",".join(name + "=" + str(folder / name) for name in content)

    async def _run(self, directory, value):
        identity = value["operation_id"]
        try:
            with _lock(directory):
                # Rejoin durable pending/resource facts after admission. All
                # final receipt writes retain this exact nonblocking lock too.
                value = self._read(directory, identity)
                if "removal" in value:
                    return self._public(value)
                try:
                    owned = await self._observe(value)
                    await self._fresh(value, owned=owned)
                    spent = self._attempts(directory, value)
                    if "creating-firewall" in spent and not owned.get("firewall") or "creating-vm" in spent and not owned.get("vm"):
                        # Even positive absence after an uncertain mutation does not
                        # authorize an automatic second create for this generation.
                        raise _Unknown()
                    if owned.get("firewall") and "creating-firewall" not in spent or owned.get("vm") and "creating-vm" not in spent:
                        raise DraftConflict("The observed cloud resources have no original mutation intent. No resource was adopted.")
                    for kind in ("firewall", "vm", "disk"):
                        if owned.get(kind):
                            value["resources"][kind] = owned[kind]
                    if owned.get("vm") and value["phase"] != "vm-ready":
                        value.update(phase="creating-vm", pending=None)
                    elif owned.get("firewall") and value["phase"] in {"checking", "creating-firewall"}:
                        value.update(phase="creating-firewall", pending=None)
                    self._save(directory, value)
                    d = value["destination"]
                    if not owned.get("firewall"):
                        await self._mutation(directory, value, "creating-firewall", ["compute", "firewall-rules", "create", d["instance"] + "-iap-ssh",
                            "--network=default", "--direction=INGRESS", "--priority=1000", "--allow=tcp:22",
                            "--source-ranges=35.235.240.0/20", "--target-tags=" + d["instance"] + "-iap",
                            "--description=MobKit Launch deployment " + value["ownership_token"], "--format=json"])
                        owned = await self._observe(value)
                        if not owned.get("firewall"):
                            raise _Unknown()
                        value["resources"]["firewall"] = owned["firewall"]
                        value["pending"] = None
                        self._save(directory, value)
                    await self._fresh(value, owned=owned)
                    if not owned.get("vm"):
                        metadata_files = self._metadata_files(directory, value)
                        await self._mutation(directory, value, "creating-vm", ["compute", "instances", "create", d["instance"], "--zone=" + d["zone"],
                            "--machine-type=" + d["machine_type"], "--image-family=debian-12", "--image-project=debian-cloud",
                            "--network=default",
                            "--boot-disk-size=" + str(d["disk_gb"]) + "GB", "--boot-disk-type=pd-balanced", "--tags=" + d["instance"] + "-iap",
                            "--labels=mobkit_launch=" + value["ownership_token"], "--no-service-account", "--no-scopes",
                            "--shielded-secure-boot", "--metadata=enable-guest-attributes=TRUE", metadata_files, "--format=json"])
                        owned = await self._observe(value)
                        if not owned.get("vm"):
                            raise _Unknown()
                    for kind in ("firewall", "vm", "disk"):
                        value["resources"][kind] = owned[kind]
                    await self._fresh(value, owned=owned)
                    if not owned.get("running"):
                        raise _Unknown()
                    value.update(status="vm-ready", phase="vm-ready", pending=None)
                    value["endpoint"] = self._endpoint(value, owned)
                    self._save(directory, value)
                except asyncio.CancelledError:
                    value["status"] = "resume-required"
                    self._save(directory, value)
                    raise
                except DraftConflict:
                    value["status"] = "blocked"
                    self._save(directory, value)
                except Exception:
                    value["status"] = "resume-required"
                    self._save(directory, value)
                return self._public(value)
        finally:
            self.tasks.pop(identity, None)

    def _endpoint_receipt(self, endpoint):
        if not isinstance(endpoint, dict):
            raise DraftConflict("The saved cloud endpoint could not be verified.")
        try:
            identity = _uuid(endpoint.get("operation_id"))
        except ValueError:
            raise DraftConflict("The saved cloud endpoint could not be verified.") from None
        # Only an endpoint from a saved ready receipt selects a resource. A
        # later guide revision does not revoke observation/stop of that receipt.
        parent = self.root / "gcp-provisioning"
        if not parent.exists():
            raise DraftConflict("The original cloud endpoint receipt is required.")
        parent = _folder(parent)
        matches = []
        for index, directory in enumerate(parent.iterdir()):
            if index >= 1024:
                raise DraftConflict("The bounded cloud receipt inventory could not select this endpoint.")
            if directory.is_dir() and not directory.is_symlink():
                value = self._read(directory, identity)
                if value:
                    matches.append((directory, value))
        if len(matches) != 1:
            raise DraftConflict("The original cloud endpoint receipt is required.")
        directory, value = matches[0]
        if value.get("endpoint") != endpoint:
            raise DraftConflict("The saved cloud endpoint does not match its ready receipt.")
        return directory, value

    async def lifecycle(self, endpoint):
        _, value = self._endpoint_receipt(endpoint)
        return value["status"] if "removal" in value else None

    async def verify(self, endpoint):
        directory, value = self._endpoint_receipt(endpoint)
        with _lock(directory):
            try:
                value = self._read(directory, value["operation_id"])
                if value.get("endpoint") != endpoint or "removal" in value:
                    raise DraftConflict("This cloud trial has a removal request. New remote access is not admitted.")
                await self._original_account(value)
                owned = await self._observe(value)
                if not owned.get("vm") or not owned.get("firewall"):
                    raise DraftConflict("The original cloud endpoint ownership could not be verified.")
            except DraftConflict:
                raise
            except asyncio.CancelledError:
                raise
            except Exception:
                raise DraftConflict("The original cloud endpoint could not be independently observed.") from None
        return dict(endpoint)

    async def shutdown(self):
        self.closed = True
        async with self.admission:
            tasks = list(self.tasks.values())
            for task in tasks:
                task.cancel()
            await asyncio.gather(*tasks, return_exceptions=True)
        if getattr(self.command, "cleanup_pending", False):
            raise DraftConflict("An owned cloud command has not positively stopped. Resume remains unavailable.")


def PROJECT_VALID(value):
    try:
        return _project(value) == value
    except ValueError:
        return False
