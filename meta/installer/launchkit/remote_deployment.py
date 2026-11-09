"""Source-bound wizard admission and observation for a chosen SSH destination."""
from __future__ import annotations

import asyncio
import base64
from contextlib import contextmanager
from copy import deepcopy
import errno
import fcntl
import gzip
import hashlib
import io
import json
import logging
import os
from pathlib import Path, PurePosixPath
import re
import stat
import tarfile
import time
import uuid
import zlib

from .artifact_binding import current_version, package_matches, proposal_hash
from .deployment import (MAX_PACKAGE_BYTES, await_validation, package_files, package_configuration_fields,
                         package_console_scope, prepare_private_settings, require_accepted, now)
from .deployment_ports import checked_console_port
from .projects import DraftConflict
from .ssh_transport import SshTransport, SshTransportError, validate_endpoint
from .remote_browser_access import RemoteBrowserAccessService
from .application_access import application_observation
from .remote_destination import (ControllerError, PRODUCT_RUNTIME_UNSUPPORTED,
                                 connection_prerequisites, private_settings_input)

LOG = logging.getLogger(__name__)
PROTOCOL = 1
HASH = re.compile(r"[a-f0-9]{64}")
FINGERPRINT = re.compile(r"SHA256:[A-Za-z0-9+/]{20,90}={0,2}")
ACTIVE = {"connecting", "installing", "checking", "starting", "running", "stopping", "restarting", "recovery-needed"}
PRIVATE_COMPONENTS = {".env", ".venv", ".bin", ".mobkit", ".git", "__pycache__", ".source"}
PRIVATE_ROOTS = {"data", "workspace", "source"}
NEEDED = {"remote_operations.py", "setup.py", "install.sh", "application_configuration.py", "mcp_connections.py"}
MAX_CONTROLLER = 512 * 1024
# A new installation is built here, outside every guide's inventory, and
# renamed into place only once its record is complete.
STAGING = ".connecting"


class RemoteProductActionNotSent(DraftConflict):
    """A product mutation was rejected before beginning its delivery intent."""
    code = "remote_product_action_not_sent"

    def __init__(self, action, action_request_id):
        super().__init__("This request was not sent. Reload the installed application before trying again.")
        self.action = action
        self.action_request_id = action_request_id


def build_guide_version(receipt):
    """Retain exact build provenance; old records used their connect version."""
    version = receipt.get("build_guide_version", receipt.get("guide_version"))
    if type(version) is not int or not 1 <= version <= 2 ** 53 - 1:
        raise DraftConflict("The saved remote build provenance is invalid. Preserve this installation before another action.")
    return version


def source_archive(files):
    """Transfer only declared public package source; preserve the independent ZIP binding."""
    selected = {name: entry for name, entry in files.items()
                if name != "SHA256SUMS" and not PRIVATE_COMPONENTS.intersection(name.split("/"))
                and name.split("/")[0] not in PRIVATE_ROOTS and not name.endswith(".receipt.json")}
    digest = hashlib.sha256()
    rows = []
    for name, (content, _) in sorted(selected.items()):
        hashed = hashlib.sha256(content).hexdigest()
        digest.update(name.encode() + b"\0" + bytes.fromhex(hashed))
        rows.append(hashed + "  " + name + "\n")
    output = io.BytesIO()
    # Destination custody pins the actual archive bytes as well as its source
    # digest. A later transfer of the same accepted package must not gain a new
    # gzip timestamp and look like changed source.
    with gzip.GzipFile(fileobj=output, mode="wb", mtime=0) as compressed:
        with tarfile.open(fileobj=compressed, mode="w:") as archive:
            for name, (content, mode) in sorted({**selected, "SHA256SUMS": ("".join(rows).encode(), 0o644)}.items()):
                info = tarfile.TarInfo(name)
                info.size, info.mode, info.mtime = len(content), mode, 0
                archive.addfile(info, io.BytesIO(content))
    return output.getvalue(), digest.hexdigest()


def package_digest(payload):
    return hashlib.sha256(payload).hexdigest()


def private_file(path, maximum=MAX_PACKAGE_BYTES):
    path = Path(path)
    if any(item.is_symlink() for item in (path, *path.parents)):
        raise DraftConflict("The owned remote installation record contains a link. No remote action was taken.")
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(descriptor, "rb") as stream:
        info = os.fstat(stream.fileno())
        if (not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_nlink != 1
                or info.st_mode & 0o077 or info.st_size > maximum):
            raise DraftConflict("The owned remote installation record is not private or ordinary.")
        return stream.read(maximum + 1)


def write_bytes(path, content):
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(descriptor, "wb") as stream:
        stream.write(content)
        stream.flush()
        os.fsync(stream.fileno())


def read_receipt(directory):
    return json.loads(private_file(Path(directory) / "receipt.json", 256 * 1024))


def write_private(path, value):
    path = Path(path)
    if any(item.is_symlink() for item in (path, *path.parents)):
        raise DraftConflict("The owned remote installation record contains a link.")
    if path.exists():
        private_file(path, 256 * 1024)
    temporary = path.parent / ("." + path.name + "." + uuid.uuid4().hex + ".pending")
    try:
        write_bytes(temporary, (json.dumps(value, indent=2, allow_nan=False) + "\n").encode())
        temporary.replace(path)
    finally:
        temporary.unlink(missing_ok=True)


@contextmanager
def receipt_lock(directory):
    path = Path(directory) / "journal.lock"
    if any(item.is_symlink() for item in (path, *path.parents)):
        raise DraftConflict("The owned remote installation lock contains a link.")
    descriptor = os.open(path, os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    try:
        info = os.fstat(descriptor)
        if (not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid()
                or info.st_nlink != 1 or info.st_mode & 0o077):
            raise DraftConflict("The remote installation lock is not private and owned.")
        fcntl.flock(descriptor, fcntl.LOCK_EX)
        yield
    finally:
        os.close(descriptor)


def update_receipt(directory, **changes):
    with receipt_lock(directory):
        value = read_receipt(directory)
        value.update(changes, updated_at=now())
        write_private(Path(directory) / "receipt.json", value)
        return value


def _sync_directory(path):
    descriptor = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def _staging_lock(path):
    """Claim one staging entry for this connect's whole life; None if it is held or gone."""
    descriptor = os.open(path, os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    try:
        info = os.fstat(descriptor)
        if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_nlink != 1:
            raise DraftConflict("The remote installation store is not an owned directory.")
        fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
        # A sweep may have claimed and unlinked this name between the open and
        # the flock; a claim on a nameless inode protects nothing.
        current = os.stat(path, follow_symlinks=False)
        if (current.st_dev, current.st_ino) != (info.st_dev, info.st_ino):
            os.close(descriptor)
            return None
        return descriptor
    except (BlockingIOError, FileNotFoundError):
        os.close(descriptor)
        return None
    except BaseException:
        os.close(descriptor)
        raise


def _end_staging(entry, lock):
    """Remove a held staging entry, if still there, and release its claim.

    A copy that cannot be removed is left for the next sweep. This never raises,
    so it cannot replace the error that ended the connect.
    """
    try:
        if entry.exists() and not entry.is_symlink():
            for item in entry.iterdir():
                if item.is_symlink() or not item.is_file():
                    raise OSError("unexpected entry " + item.name)
                item.unlink()
            entry.rmdir()
        entry.with_name(entry.name + ".lock").unlink(missing_ok=True)
    except OSError as error:
        LOG.warning("Left the connect staging entry %s for a later sweep: %s", entry.name, error)
    finally:
        os.close(lock)


def journal(directory, message, **changes):
    with receipt_lock(directory):
        value = read_receipt(directory)
        progress = [*value.get("progress", []), {"at": now(), "message": message}][-100:]
        value.update(changes, updated_at=now(), progress=progress, detail=message)
        write_private(Path(directory) / "receipt.json", value)
        return value


def destination_input(value):
    if (not isinstance(value, str) or not value.strip() or len(value) > 2000
            or any(ord(char) < 32 or ord(char) == 127 for char in value)):
        raise ValueError("Choose a project folder on the Linux computer.")
    value = value.strip()
    if not value.startswith(("/", "~/")) or str(PurePosixPath(value)) in {"/", "~"}:
        raise ValueError("Use a project folder such as ~/src/my-team on the Linux computer.")
    if ".." in PurePosixPath(value).parts:
        raise ValueError("Choose a project folder without parent-directory traversal.")
    return value


def binding(receipt):
    return {key: receipt[key] for key in ("guide_id", "guide_version", "build_id", "accepted_source_digest",
            "package_sha256", "transferred_source_digest", "controller_sha256")}


def product_control_observation(receipt, value):
    """Join a verified controller reply to its original finite product selection.

    This reader grants no process authority. The exact SSH/controller and retained
    guest factory own the observations; an immutable discovery binding is data.
    """
    product = value.get("product_control")
    if product is None:
        return None
    try:
        held = product["binding"]
        actual = product["factory_status"]
        previous = receipt.get("product_control")
        if (value.get("scope") != "retained-original-product-owner" or product.get("owner_live") is not True
                or product.get("initial_install_complete") is not True or held.get("protocol") != 1
                or held.get("binding") != binding(receipt)
                or held.get("install_operation_id") != receipt.get("install_operation_id")
                or not operation_identity(held.get("install_operation_id"))
                or held.get("control_manifest_sha256") != receipt.get("product_control_manifest_sha256")
                or not isinstance(held.get("initial_install_sha256"), str)
                or not re.fullmatch(r"[a-f0-9]{64}", held["initial_install_sha256"])
                or any(actual.get(key) != receipt[key] for key in ("guide_id", "deployment_id", "build_id", "package_sha256"))
                or actual.get("source_digest") != receipt["accepted_source_digest"]
                or actual.get("destination") != receipt["destination"]
                or previous is not None and previous.get("binding") != held):
            raise ValueError
        for name in ("worker", "command"):
            birth = held[name]
            if (type(birth) is not dict or type(birth.get("pid")) is not int or birth["pid"] <= 0
                    or birth.get("uid") != receipt["host"]["uid"] or not isinstance(birth.get("start_ticks"), str)
                    or not isinstance(birth.get("boot_id"), str)):
                raise ValueError
        return deepcopy(product)
    except (ValueError, KeyError, TypeError, AttributeError):
        raise DraftConflict("The retained product control observation changed its original source or process binding.") from None


def product_setup_observation(receipt, product):
    """Presentation of actual stopped setup inputs, joined to the original package."""
    setup = product.get("setup_status") if isinstance(product, dict) else None
    if setup is None:
        return None
    if (not isinstance(setup, dict) or setup.get("scope") != "selected-stopped-application-inputs"
            or type(setup.get("available")) is not bool
            or any(setup.get(key) != receipt[key] for key in ("guide_id", "deployment_id", "build_id", "package_sha256", "guide_version"))):
        raise DraftConflict("The product setup inputs changed their original installed package binding.")
    return deepcopy(setup)



def product_action_request(receipt, product, action, action_request_id):
    request = product.get("action_request") if isinstance(product, dict) else None
    if request is None:
        return None
    expected = {"action_request_id": action_request_id, "guide_id": receipt["guide_id"],
        "deployment_id": receipt["deployment_id"], "action": action, "update_id": None}
    if action == "configure":
        expected.update(guide_version=receipt["guide_version"], build_id=receipt["build_id"],
            source_digest=receipt["accepted_source_digest"], package_sha256=receipt["package_sha256"])
    if (not isinstance(request, dict) or any(request.get(key) != selected for key, selected in expected.items())
            or request.get("phase") not in {"pending", "settled", "unknown"}
            or request.get("phase") == "settled" and request.get("outcome") not in {"returned", "error"}):
        raise DraftConflict("The canonical action UUID belongs to another product operation. No action was repeated.")
    return deepcopy(request)


def product_stopped_configuration(receipt, product):
    actual = product["factory_status"]
    return (receipt.get("status") == "stopped" and actual.get("status") == "stopped"
        and receipt.get("source_current") is True and actual.get("source_verified") is True
        and actual.get("execution_mode", "interactive") == "interactive"
        and not actual.get("process_live") and not actual.get("service")
        and receipt.get("worker_live") is True and receipt.get("cleanup_confirmed") is False)


def operation_identity(value):
    try:
        return isinstance(value, str) and str(uuid.UUID(value)) == value
    except (ValueError, AttributeError):
        return False


def product_transfer_pending(receipt):
    """Only the original not-yet-posted product Install may continue transfers."""
    operation = receipt.get("install_operation_id")
    return (receipt.get("product_runtime") is True
        and receipt.get("remote_admission_requested") is True
        and receipt.get("product_phase") in {"source-intake", "control-transfer"}
        and operation_identity(operation)
        and receipt.get("pending_operation") == {"id": operation, "action": "install"}
        and receipt.get("operation_id") is None
        and not any(receipt.get(key) for key in ("admitted", "worker_live", "service_owned", "service_observed")))



def retry_observed(receipt):
    """A failed attempt can be replaced only from joined destination evidence."""
    pending = receipt.get("pending_operation")
    service = receipt.get("service")
    if not isinstance(service, dict) or pending is not None and not isinstance(pending, dict):
        return False
    stopped_service = service.get("status") == "not-installed" or (
        receipt.get("service_owned") is True and service.get("status") in {"inactive", "failed"}
        and service.get("quiescent") is True)
    return (receipt.get("status") == "failed" and receipt.get("remote_admission_requested") is True
        and receipt.get("admitted") is True and receipt.get("retryable") is True
        and operation_identity(receipt.get("operation_id"))
        and receipt.get("retry_of_operation_id") == receipt["operation_id"]
        and receipt.get("worker_live") is False and receipt.get("cleanup_confirmed") is True
        and receipt.get("source_current") is True and receipt.get("service_observed") is True
        and stopped_service and (not pending or pending.get("action") == "retry-install"
            and pending.get("previous_operation_id") == receipt["operation_id"]))


def controller_program(content):
    # Bundled canonical helpers grow the immutable controller. Compress its
    # exact bytes so the isolated Python argument stays below Linux limits.
    encoded = base64.b64encode(zlib.compress(content, level=9)).decode("ascii")
    expected = hashlib.sha256(content).hexdigest()
    return ("import base64,hashlib,zlib; s=zlib.decompress(base64.b64decode(" + repr(encoded) + ")); "
            "assert hashlib.sha256(s).hexdigest()==" + repr(expected) + "; "
            "exec(compile(s,'<mobkit-remote-controller>','exec'),"
            "{'__name__':'__main__','CONTROLLER_SOURCE':s})")


class RemoteDeploymentService:
    def __init__(self, root, guide, implementation, local_deployment, *, transport=None, cloud_transport=None, product_control_selection=None):
        self.root = Path(root) / "remote-installations"
        self.guide, self.implementation, self.local = guide, implementation, local_deployment
        self.transport = transport or SshTransport()
        self.cloud_transport = cloud_transport
        if product_control_selection is not None:
            from .remote_product_control import ProductControlSelection
            if type(product_control_selection) is not ProductControlSelection:
                raise ValueError("Select the original private product control owner.")
        self.product_control_selection = self._product_control_selection = product_control_selection
        self.cloud_removal_status = None
        self.lock = asyncio.Lock()
        self.observation_locks = {}
        self.tasks = {}
        self.last_observed = {}
        self.browser_access = RemoteBrowserAccessService(self)

    async def stage_product_source(self, guide_id, *, build_id, source_digest, guide_version,
            deployment_id, custody, transaction_directory, destination_host, transport,
            endpoint, expected_fingerprint, require_current, timeout=600, cloud=None):
        """Private trusted intake, without normal Install or native setup admission.

        The original supplying host owns the observed endpoint, artifact custody
        and retained currentness callback. No HTTP route or browser input selects
        this entry. Its returned state is source staging only, never deployment
        readiness, settings authorization or a native Fresh/Restore selection.
        """
        from .remote_product_destination_owner import stage_product_destination_source
        source_options = {}
        if isinstance(endpoint, dict) and endpoint.get("transport") == "gcp-iap":
            from .gcp_transport import validate_gcp_endpoint
            from .remote_product_artifacts import _current
            from .ssh_transport import _operation_deadline, _remaining
            _current(require_current)
            checked = validate_gcp_endpoint(endpoint)
            if (transport is not self.cloud_transport or not isinstance(cloud, dict)
                    or cloud.get("status") != "vm-ready" or cloud.get("vm_verified") is not True
                    or cloud.get("guide_id") != guide_id or cloud.get("operation_id") != checked["operation_id"]
                    or cloud.get("endpoint") != checked or not isinstance(cloud.get("destination"), dict)
                    or any(cloud["destination"].get(key) != checked[key] for key in ("project", "zone", "instance"))
                    or "account" in cloud and cloud["account"] != checked["account"]):
                raise DraftConflict("Product source staging needs its original GCP IAP transport owner.")
            original_cloud, original_owner = deepcopy(cloud), require_current
            def cloud_source_current():
                _current(original_owner)
                if self.cloud_transport is not transport or cloud != original_cloud:
                    raise DraftConflict("The original cloud operation or IAP transport changed during source staging.")
            require_current = cloud_source_current
            deadline = _operation_deadline(timeout, None)
            state = await self.guide.get(guide_id)
            _current(require_current)
            binding = original_cloud.get("binding")
            cloud_version = binding.get("guide_version") if isinstance(binding, dict) else None
            proposal = state.get("proposal") or state.get("blueprint") or {}
            expected = {"guide_id": guide_id, "guide_version": cloud_version, "build_id": build_id,
                "blueprint_hash": proposal_hash(proposal), "source_digest": source_digest,
                "package_sha256": binding.get("package_sha256") if isinstance(binding, dict) else None}
            if (type(cloud_version) is not int or not 1 <= cloud_version <= state["version"]
                    or binding != expected or proposal.get("target") != "gcp"
                    or any(proposal.get("gcp", {}).get(key) != checked[key] for key in ("project", "zone"))
                    or checked["instance"] != proposal.get("project", {}).get("slug")):
                raise DraftConflict("The cloud computer belongs to another accepted package or destination.")
            await self.require_cloud_available({"target": "gcp", "endpoint": checked})
            if await transport.verify(checked) != checked:
                raise DraftConflict("The original cloud computer changed before product source staging.")
            _current(require_current)
            timeout = _remaining(deadline)
            source_options["source_disposition"] = {"target": "gcp", "binding": deepcopy(binding), **{
                key: checked[key] for key in ("project", "zone", "instance", "instance_id", "operation_id")}}
        return await stage_product_destination_source(self.implementation, guide_id,
            build_id=build_id, source_digest=source_digest, guide_version=guide_version,
            deployment_id=deployment_id, custody=custody, transaction_directory=transaction_directory,
            destination_host=destination_host, transport=transport, endpoint=endpoint,
            expected_fingerprint=expected_fingerprint, require_current=require_current, timeout=timeout,
            **source_options)

    async def cloud_status(self, receipt):
        if receipt.get("target") != "gcp" or self.cloud_removal_status is None:
            return None
        try:
            value = await self.cloud_removal_status(receipt["endpoint"])
            if value is not None and value not in {"removing", "removal-required", "removed"}:
                raise ValueError()
            return value
        except (ValueError, LookupError, OSError, RuntimeError, TypeError, KeyError):
            raise DraftConflict("The original cloud removal state could not be verified. No remote connection was requested.") from None

    async def require_cloud_available(self, receipt):
        if await self.cloud_status(receipt) is not None:
            raise DraftConflict("Cloud resource removal has been requested. This installation cannot admit another remote operation.")

    async def prepare_cloud_removal(self, value):
        """Join local custody only; deleting the VM can end destination-owned work."""
        from .gcp_transport import validate_gcp_endpoint
        try:
            guide_id, operation_id = value["guide_id"], value["operation_id"]
            if not operation_identity(guide_id) or not operation_identity(operation_id):
                raise ValueError()
            endpoint = value.get("endpoint")
            if endpoint is not None:
                endpoint = validate_gcp_endpoint(endpoint)
                destination = value["destination"]
                if (endpoint["operation_id"] != operation_id or endpoint["account"] != value["account"]
                        or endpoint["ownership_token"] != value["ownership_token"]
                        or endpoint["instance_id"] != value["resources"]["vm"]["id"]
                        or any(endpoint[key] != destination[key] for key in ("project", "zone", "instance"))):
                    raise ValueError()
        except (SshTransportError, ValueError, KeyError, TypeError, AttributeError):
            raise DraftConflict("The original cloud removal destination could not be joined to its saved installation.") from None
        async with self.lock:
            parent = self.root / guide_id
            if any(path.is_symlink() for path in (parent, *parent.parents)):
                raise DraftConflict("The saved remote installation inventory contains a link.")
            if not parent.exists():
                return True
            for index, directory in enumerate(parent.iterdir()):
                if index >= 1024:
                    raise DraftConflict("The saved remote installation inventory exceeds its bound.")
                if directory.name == "latest.json":
                    continue
                if directory.is_symlink() or not directory.is_dir() or not operation_identity(directory.name):
                    raise DraftConflict("The saved remote installation inventory could not be verified.")
                receipt = read_receipt(directory)
                if not isinstance(receipt, dict):
                    raise DraftConflict("The saved remote installation receipt could not be verified.")
                selected = receipt.get("endpoint")
                if receipt.get("target") == "gcp" and not isinstance(selected, dict):
                    raise DraftConflict("The saved cloud installation has no verified original endpoint.")
                if (not isinstance(selected, dict) or selected.get("operation_id") != operation_id) and receipt.get("cloud_operation_id") != operation_id:
                    continue
                if receipt.get("target") != "gcp":
                    raise DraftConflict("The saved cloud installation has a different destination type.")
                _, receipt = await await_validation(self.stored, guide_id, directory.name)
                if endpoint is None or receipt.get("endpoint") != endpoint or receipt.get("cloud_operation_id") != operation_id:
                    raise DraftConflict("The saved remote installation belongs to a different cloud removal destination.")
                task = self.tasks.get(directory.name)
                if task is not None:
                    # Local SSH observation owns its cleanup until it returns.
                    # Its destination worker/operation receipt remains historical.
                    await asyncio.shield(task)
                async with self.operation_lock(directory):
                    _, receipt = await await_validation(self.stored, guide_id, directory.name)
                    if receipt.get("endpoint") != endpoint or receipt.get("cloud_operation_id") != operation_id:
                        raise DraftConflict("The saved cloud installation changed during removal coordination.")
                    closed = await self.browser_access.revoke(guide_id, directory.name,
                        "Browser access is closed because this cloud computer is being removed.")
                    key = self.browser_access.key(guide_id, directory.name)
                    if closed.get("cleanup_confirmed") is not True or any(
                            self.browser_access.public(candidate).get("cleanup_confirmed") is not True
                            for candidate in self.browser_access.deployment_keys(key)):
                        raise DraftConflict("The local browser connection has not confirmed cleanup. Close it again before cloud removal.")
            return True

    def directory(self, guide_id, deployment_id=None):
        try:
            if str(uuid.UUID(guide_id)) != guide_id:
                raise ValueError
            parent = self.root / guide_id
            if deployment_id is None:
                deployment_id = json.loads(private_file(parent / "latest.json", 4096))["deployment_id"]
            if str(uuid.UUID(deployment_id)) != deployment_id:
                raise ValueError
        except FileNotFoundError:
            raise LookupError("No remote installation is selected for this setup.") from None
        except (ValueError, KeyError, TypeError):
            raise ValueError("Choose an exact owned remote installation.") from None
        return parent / deployment_id

    def stored(self, guide_id, deployment_id=None):
        directory = self.directory(guide_id, deployment_id)
        receipt = json.loads(private_file(directory / "receipt.json", 256 * 1024))
        if receipt.get("guide_id") != guide_id or receipt.get("deployment_id") != directory.name:
            raise DraftConflict("The remote installation ownership changed. No remote action was taken.")
        self.frozen(directory, receipt)
        return directory, receipt

    def frozen(self, directory, receipt):
        payload = private_file(directory / "package.zip")
        content = private_file(directory / "controller.py", MAX_CONTROLLER)
        if (hashlib.sha256(payload).hexdigest() != receipt["package_sha256"]
                or hashlib.sha256(content).hexdigest() != receipt["controller_sha256"]):
            raise DraftConflict("The owned remote package or controller changed. Preserve this installation and inspect its source.")
        files = package_files(payload)
        archive, digest = source_archive(files)
        if digest != receipt["transferred_source_digest"]:
            raise DraftConflict("The owned remote source manifest changed. No remote action was taken.")
        report = json.loads(files["implementation-report.json"][0])
        if (not isinstance(report, dict) or type(report.get("guide_version")) is not int
                or report["guide_version"] != build_guide_version(receipt)):
            raise DraftConflict("The owned remote package's original build provenance changed. No remote action was taken.")
        if receipt.get("product_runtime") is True:
            from .remote_product_control import MAX_BUNDLE
            controls = private_file(directory / "product-control.json", 1024 * 1024)
            bundle = private_file(directory / "control-bundle.zip", MAX_BUNDLE)
            if (hashlib.sha256(controls).hexdigest() != receipt.get("product_control_manifest_sha256")
                    or len(bundle) != receipt.get("product_control_bundle_size")
                    or hashlib.sha256(bundle).hexdigest() != receipt.get("product_control_bundle_sha256")):
                raise DraftConflict("The original product control custody changed.")
        return files, archive, content

    async def require_current_request(self, guide_id, version, *, files=None, admission=True):
        state = await self.local._guide(guide_id)
        if (state.get("id") != guide_id or type(version) is not int or state.get("version") != version
                or admission and (state.get("operation") or {}).get("status") == "running"):
            raise DraftConflict("The guide changed. Reload its current accepted application before another action.")
        if files is not None and not package_matches(json.loads(files["implementation-report.json"][0]), state):
            raise DraftConflict("The accepted application changed while preparing this request. No new remote action was taken.")
        return state

    async def accepted(self, guide_id, version, build_id, *, target="linux", state=None, admission=True, product_owner=None):
        supplied_state = state is not None
        state = state if supplied_state else await self.local._guide(guide_id)
        if not isinstance(state, dict) or state.get("id") != guide_id:
            raise ValueError("Remote evidence requires the matching detached guide state.")
        if (state["version"] != version
                or admission and (state.get("operation") or {}).get("status") == "running"):
            raise DraftConflict("The guide changed. Connect or install only its current accepted application.")
        if target not in {"linux", "gcp"} or (state.get("proposal") or state.get("blueprint") or {}).get("target") != target:
            raise DraftConflict("Connect only the accepted application's chosen destination.")
        checked = await self.implementation.status(guide_id, state=state) if supplied_state else await self.implementation.status(guide_id)
        require_accepted(checked)
        if (checked["build_id"] != build_id or not current_version(checked, state) or not checked.get("ready_to_package")
                or type(checked.get("guide_version")) is not int or checked["guide_version"] < 1
                or not HASH.fullmatch(checked.get("source_digest", ""))):
            raise DraftConflict("Build and accept the current application before connecting its destination.")
        payload, slug = (await self.implementation.package(guide_id, build_id, state=state) if supplied_state
                         else await self.implementation.package(guide_id, build_id))
        # Unpacking the whole package must not hold the event loop.
        files = await await_validation(package_files, payload)
        spec = json.loads(files["deployment.json"][0])
        report = json.loads(files["implementation-report.json"][0])
        if (report.get("build_id") != build_id or report.get("guide_version") != checked["guide_version"]
                or not package_matches(report, state)
                or report.get("verification", {}).get("digest") != checked["source_digest"]
                or report.get("blueprint_hash") != proposal_hash(state.get("proposal") or state.get("blueprint"))
                or spec.get("target") != target):
            raise DraftConflict("The accepted package differs from this proposal or application source.")
        runtime = spec.get("runtimeCandidate")
        product = isinstance(runtime, dict) and runtime.get("source") == "meta-toolkit-product"
        if product:
            from .remote_product_control import ProductControlSelection
            from .remote_product_artifacts import _joined
            from .runtime_selection import parse_candidate_selection
            selected = self.product_control_selection
            if (type(selected) is not ProductControlSelection
                    or selected is not self._product_control_selection
                    or admission and product_owner is not selected):
                raise DraftConflict(PRODUCT_RUNTIME_UNSUPPORTED)
            selected.require_current()
            await _joined(selected.read_dependencies, _owner_current=selected.require_current)
            original = await _joined(self.implementation.selected_runtime_target, guide_id,
                build_id=build_id, source_digest=checked["source_digest"],
                _owner_current=selected.require_current)
            if (original != selected.runtime_target
                    or parse_candidate_selection(json.loads(files["runtime-candidate.json"][0])) != original):
                raise DraftConflict("The accepted source differs from its original product/control selection.")
        if not NEEDED.issubset(files):
            raise DraftConflict("This accepted package lacks guided Linux setup support. Continue implementation before deploying it.")
        fields = package_configuration_fields(spec, files)
        if not supplied_state:
            await self.require_current_request(guide_id, version, files=files, admission=admission)
        return {"state": state, "checked": checked, "payload": payload, "files": files,
                "spec": spec, "fields": fields, "slug": slug,
                "console_scope": package_console_scope(files, fields, state),
                **({"product_control": selected} if product else {})}

    async def selection_matches(self, receipt, selected):
        return (selected["state"]["id"] == receipt["guide_id"]
            and selected["checked"]["guide_version"] == build_guide_version(receipt)
            and selected["checked"]["build_id"] == receipt["build_id"]
            and selected["checked"]["source_digest"] == receipt["accepted_source_digest"]
            # Hashing the whole package must not hold the event loop.
            and await await_validation(package_digest, selected["payload"]) == receipt["package_sha256"])

    async def current_state(self, receipt, *, state=None):
        try:
            snapshot = state if state is not None else await self.local._guide(receipt["guide_id"])
            options = {"target": receipt.get("target", "linux"), "state": snapshot, "admission": False}
            selected = await self.accepted(receipt["guide_id"], snapshot["version"], receipt["build_id"], **options)
            if not await self.selection_matches(receipt, selected):
                return None
            if state is None:
                latest = await self.local._guide(receipt["guide_id"])
                if (latest.get("id") != receipt["guide_id"]
                        or not package_matches(json.loads(selected["files"]["implementation-report.json"][0]), latest)):
                    return None
                return latest
            return selected["state"]
        except (ValueError, LookupError, OSError):
            return None

    async def current(self, receipt, *, state=None):
        return await self.current_state(receipt, state=state) is not None

    async def current_public(self, receipt):
        state = await self.current_state(receipt)
        value = self.public(receipt, current=state is not None,
            current_guide_version=state["version"] if state is not None else None)
        if state is None or (state.get("operation") or {}).get("status") == "running":
            value["can_configure"] = False
            if receipt.get("product_runtime") is True:
                value.update(can_prepare_setup=False, can_finish_setup=False, can_start_background=False, can_restart=False)
        return value

    async def connect(self, guide_id, version, build_id, host, user, ssh_port=22, destination="", *, product_owner=None):
        try:
            endpoint = validate_endpoint({"host": host, "user": user, "ssh_port": ssh_port})
        except SshTransportError:
            raise ValueError("Enter a computer address, SSH user and valid SSH port.") from None
        return await self._connect(guide_id, version, build_id, endpoint, destination, product_owner=product_owner)

    async def connect_cloud(self, guide_id, version, build_id, cloud, *, product_owner=None):
        if (self.cloud_transport is None or not isinstance(cloud, dict) or cloud.get("status") != "vm-ready"
                or cloud.get("vm_verified") is not True or cloud.get("guide_id") != guide_id):
            raise DraftConflict("Verify the owned cloud computer before connecting its application.")
        from .gcp_transport import validate_gcp_endpoint
        try:
            endpoint = validate_gcp_endpoint(cloud.get("endpoint"))
        except SshTransportError:
            raise DraftConflict("The owned cloud computer has invalid connection evidence.") from None
        destination = cloud.get("destination")
        if (not isinstance(destination, dict) or cloud.get("operation_id") != endpoint["operation_id"]
                or any(destination.get(key) != endpoint[key] for key in ("project", "zone", "instance"))
                or "account" in cloud and cloud["account"] != endpoint["account"]):
            raise DraftConflict("The cloud operation and computer do not match.")
        await self.require_cloud_available({"target": "gcp", "endpoint": endpoint})
        if await self.cloud_transport.verify(endpoint) != endpoint:
            raise DraftConflict("The cloud computer identity changed. No destination was reserved.")
        return await self._connect(guide_id, version, build_id, endpoint,
            "~/src/" + cloud["destination"]["instance"], cloud=cloud, product_owner=product_owner)

    def browser_transport(self, receipt):
        if receipt.get("target") == "gcp":
            if self.cloud_transport is None:
                raise DraftConflict("The owned cloud transport is unavailable.")
            return self.cloud_transport
        select = getattr(self.transport, "browser_transport", None)
        if callable(select):
            return select(receipt["endpoint"])
        return None

    async def _connect(self, guide_id, version, build_id, endpoint, destination, *, cloud=None, product_owner=None):
        destination = destination_input(destination)
        target = "gcp" if cloud is not None else "linux"
        async with self.lock:
            await self.require_cloud_available({"target": target, "endpoint": endpoint})
            selected = await self.accepted(guide_id, version, build_id, target=target, product_owner=product_owner)
            if cloud is not None:
                cloud_version = (cloud.get("binding") or {}).get("guide_version")
                expected = {"guide_id": guide_id, "guide_version": cloud_version, "build_id": build_id,
                    "blueprint_hash": proposal_hash(selected["state"].get("proposal") or selected["state"].get("blueprint")),
                    "source_digest": selected["checked"]["source_digest"],
                    "package_sha256": await await_validation(package_digest, selected["payload"])}
                proposal = selected["state"].get("proposal") or selected["state"].get("blueprint")
                if (type(cloud_version) is not int or not 1 <= cloud_version <= version
                        or cloud.get("binding") != expected or endpoint != cloud.get("endpoint")
                        or endpoint.get("project") != proposal.get("gcp", {}).get("project")
                        or endpoint.get("zone") != proposal.get("gcp", {}).get("zone")
                        or endpoint.get("instance") != selected["spec"]["slug"]):
                    raise DraftConflict("The cloud computer belongs to another accepted package or destination.")
                if await self.cloud_transport.verify(endpoint) != endpoint:
                    raise DraftConflict("The cloud computer identity changed. No destination was reserved.")
            await self.require_current_request(guide_id, version, files=selected["files"])
            previous = None
            try:
                directory, previous = await await_validation(self.stored, guide_id)
            except LookupError:
                pass
            same = previous and previous["endpoint"] == endpoint and previous["destination_input"] == destination and (
                previous["build_id"] == build_id and previous["accepted_source_digest"] == selected["checked"]["source_digest"])
            safe_previous = previous and ((previous["status"] == "stopped" and previous.get("worker_live") is False)
                or (previous["status"] == "not-installed" and previous.get("admitted") is False))
            if previous and (previous["status"] in ACTIVE or previous["status"] == "observation-unavailable"
                    or previous.get("remote_admission_requested") and not safe_previous):
                if same:
                    return await self.current_public(previous)
                raise DraftConflict("Stop and verify the current remote installation before choosing another destination.")
            if same:
                self.start_task(directory, "preflight")
                return await self.current_public(read_receipt(directory))
            identity = str(uuid.uuid4())
            if any(item.is_symlink() for item in (self.root, *self.root.parents)):
                raise DraftConflict("The owned remote installation store contains a link.")
            self.root.mkdir(parents=True, exist_ok=True, mode=0o700)
            for folder in (self.root, self.root / guide_id, self.root / STAGING):
                folder.mkdir(exist_ok=True, mode=0o700)
                info = folder.lstat()
                if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid():
                    raise DraftConflict("The remote installation store is not an owned directory.")
                folder.chmod(0o700)
            self._sweep_staging()
            directory = self.root / guide_id / identity
            staging = self.root / STAGING / identity
            lock = _staging_lock(staging.with_name(identity + ".lock"))
            if lock is None:
                raise FileExistsError(errno.EEXIST, os.strerror(errno.EEXIST), str(staging))
            try:
                staging.mkdir(mode=0o700)
                from .remote_interface_bundle import controller_bytes
                product_bundle = None
                if selected.get("product_control") is not None:
                    from .remote_product_control import freeze_product_control
                    from .remote_product_artifacts import _joined
                    product_bundle = await _joined(freeze_product_control, selected["product_control"],
                        _owner_current=selected["product_control"].require_current)
                controller = controller_bytes(**({"product_control": product_bundle} if product_bundle is not None else {}))
                if len(controller) > MAX_CONTROLLER:
                    raise DraftConflict("The trusted remote controller exceeded its original bound.")
                _, transferred = await await_validation(source_archive, selected["files"])
                receipt = {"protocol": PROTOCOL, "deployment_id": identity, "guide_id": guide_id,
                    "guide_version": version, "build_id": build_id,
                    "build_guide_version": selected["checked"]["guide_version"],
                    "accepted_source_digest": selected["checked"]["source_digest"],
                    "package_sha256": await await_validation(package_digest, selected["payload"]),
                    "transferred_source_digest": transferred, "controller_sha256": hashlib.sha256(controller).hexdigest(),
                    "endpoint": endpoint, "destination_input": destination, "destination": destination,
                    "status": "connecting", "last_known_status": None, "created_at": now(), "updated_at": now(),
                    "fields": selected["fields"], "console_scope": selected["console_scope"],
                    "preflight": None, "progress": [], "checks": [],
                    "scope": "native-bootstrap-and-console", "custom_interfaces_verified": False,
                    "external_workflows_verified": False, "remote_admission_requested": False,
                    "observation": {"available": False}, "detail": "Checking the chosen Linux computer through its existing trusted SSH connection."}
                # Writing and syncing the whole package must not hold the event loop.
                await await_validation(write_bytes, staging / "package.zip", selected["payload"])
                write_bytes(staging / "controller.py", controller)
                if product_bundle is not None:
                    from .remote_product_control import _json
                    product_bundle.require_current()
                    control_bytes = _json(product_bundle.manifest)
                    # Writing and syncing the whole bundle must not hold the event
                    # loop. Its original owner must still be current afterwards;
                    # a refusal discards the staged copy like any other failure.
                    await await_validation(write_bytes, staging / "control-bundle.zip", product_bundle.content)
                    product_bundle.require_current()
                    write_bytes(staging / "product-control.json", control_bytes)
                    receipt.update(product_runtime=True,
                        product_control_manifest_sha256=hashlib.sha256(control_bytes).hexdigest(),
                        product_control_bundle_sha256=product_bundle.manifest["bundle"]["sha256"],
                        product_control_bundle_size=product_bundle.manifest["bundle"]["size"])
                    if cloud is not None:
                        cloud_bytes = _json(cloud)
                        write_bytes(staging / "product-cloud.json", cloud_bytes)
                        receipt["product_cloud_sha256"] = hashlib.sha256(cloud_bytes).hexdigest()
                if cloud is not None:
                    receipt.update(target="gcp", cloud_operation_id=cloud["operation_id"],
                        detail="Checking the owned Google Cloud computer through its verified IAP SSH connection.")
                write_private(staging / "receipt.json", receipt)
                _sync_directory(staging)
                if directory.exists() or directory.is_symlink():
                    raise FileExistsError(errno.EEXIST, os.strerror(errno.EEXIST), str(directory))
                os.rename(staging, directory)
            except BaseException:
                # A failed or cancelled connect leaves nothing in the inventory.
                # Every awaited write has returned before this runs.
                _end_staging(staging, lock)
                raise
            _end_staging(staging, lock)
            # Past the rename the installation is complete under its own name,
            # even if naming it as the selection below fails.
            _sync_directory(directory.parent)
            write_private(directory.parent / "latest.json", {"deployment_id": identity})
            self.start_task(directory, "preflight")
            return self.public(receipt, current=True, current_guide_version=version)

    def _sweep_staging(self):
        """Remove staging copies that no live connect, in any process, still claims."""
        root, names, unexpected = self.root / STAGING, set(), set()
        for entry in list(root.iterdir()):
            name = entry.name.removesuffix(".lock")
            try:
                info = entry.lstat()
            except FileNotFoundError:
                continue
            kind = stat.S_ISREG if entry.name.endswith(".lock") else stat.S_ISDIR
            if not kind(info.st_mode) or info.st_uid != os.getuid() or not operation_identity(name):
                LOG.warning("Left an unexpected connect staging entry in place: %s", entry.name)
                unexpected.add(name)
            else:
                names.add(name)
        for name in sorted(names - unexpected):
            try:
                lock = _staging_lock(root / (name + ".lock"))
            except (OSError, DraftConflict) as error:
                LOG.warning("Left the connect staging entry %s in place: %s", name, error)
                continue
            if lock is not None:
                _end_staging(root / name, lock)

    def start_task(self, directory, action):
        identity = directory.name
        if identity in self.tasks:
            return
        self.tasks[identity] = asyncio.create_task(self.probe(directory, action))
        def finished(task):
            self.tasks.pop(identity, None)
            if not task.cancelled():
                task.exception()  # Consume a failed observer without changing remote custody.
        self.tasks[identity].add_done_callback(finished)

    async def rpc(self, directory, receipt, action, *, request_version=None, _publish_product_intent=None, **extra):
        await self.require_cloud_available(receipt)
        files, _, content = await await_validation(self.frozen, directory, receipt)
        if request_version is not None:
            await self.require_current_request(receipt["guide_id"], request_version, files=files)
        body = {"action": action, "deployment_id": receipt["deployment_id"], "binding": binding(receipt),
                "destination": receipt["destination"], **extra}
        if receipt.get("host"):
            body["expected_host"] = {key: receipt["host"][key] for key in ("uid", "home", "destination")}
        transport = self.cloud_transport if receipt.get("target") == "gcp" else self.transport
        if transport is None:
            raise DraftConflict("The owned cloud transport is unavailable.")
        endpoint, program = receipt["endpoint"], controller_program(content)
        expected_fingerprint = receipt.get("host_key_fingerprint")
        if _publish_product_intent is not None:
            # All local preparation is complete. Keep publication synchronous
            # with entering transport; delivery uncertainty begins here.
            _publish_product_intent()
        value = await transport.request(endpoint, program, body, timeout=30,
            expected_fingerprint=expected_fingerprint)
        observed = value.pop("_transport", {})
        fingerprint = observed.get("host_key_fingerprint")
        if not isinstance(fingerprint, str) or not FINGERPRINT.fullmatch(fingerprint):
            raise SshTransportError("invalid_response")
        if receipt.get("host_key_fingerprint") and receipt["host_key_fingerprint"] != fingerprint:
            raise DraftConflict("The SSH server identity changed. The existing installation is preserved; verify the destination before any action.")
        if (value.get("protocol") != PROTOCOL or value.get("deployment_id") != receipt["deployment_id"]
                or value.get("binding") != binding(receipt)):
            raise DraftConflict("The remote response belongs to another installation. Its result was not adopted.")
        return value, fingerprint

    def operation_lock(self, directory):
        return self.observation_locks.setdefault(directory.name, asyncio.Lock())

    async def probe(self, directory, action):
        async with self.operation_lock(directory):
            await self._probe(directory, action)

    async def _probe(self, directory, action):
        try:
            receipt = read_receipt(directory)
            if await self.cloud_status(receipt) is not None:
                return
            files, _, _ = await await_validation(self.frozen, directory, receipt)
            spec = json.loads(files["deployment.json"][0])
            pending = receipt.get("pending_operation")
            status_request = ({"operation_id": pending["id"]} if action == "status"
                and receipt.get("product_control_manifest_sha256") is not None and isinstance(pending, dict)
                and pending.get("action") != "install" else {})
            value, fingerprint = await self.rpc(directory, receipt, action,
                **({"knowledge_mode": spec["knowledge"]["mode"], **connection_prerequisites(spec)}
                   if action == "preflight" else status_request))
            if action == "preflight":
                if not await self.current(receipt):
                    journal(directory, "The application changed during the destination check. Rebuild or reconnect its current accepted source.", status="failed")
                    return
                host = value.get("host")
                if (not isinstance(host, dict) or type(host.get("uid")) is not int or host["uid"] < 0
                        or not isinstance(host.get("home"), str) or not host["home"].startswith("/")
                        or not isinstance(host.get("destination"), str) or not host["destination"].startswith("/")):
                    raise DraftConflict("The destination did not return its actual user and owned project folder.")
                if receipt.get("host") and any(host.get(key) != receipt["host"].get(key) for key in ("uid", "home", "destination")):
                    raise DraftConflict("The destination user or owned project folder changed. No private setup was sent.")
                if value.get("ready") is True:
                    try:
                        python = tuple(int(part) for part in host["python"].split(".")[:2])
                    except (KeyError, AttributeError, ValueError):
                        python = ()
                    if host.get("platform") != "linux" or python < (3, 11):
                        raise DraftConflict("Guided remote installation requires a verified Linux destination with Python 3.11 or newer.")
                status = "configuration-required" if value.get("ready") is True else "failed"
                journal(directory, "The destination checks passed. Enter private setup to install." if status == "configuration-required"
                    else "The destination needs attention. Resolve the failed checks, then check it again.",
                    status=status, preflight=value, host=host, destination=host["destination"],
                    host_key_fingerprint=fingerprint, checks=value.get("checks", []), observation={"available": True},
                    private_settings_supported=value.get("private_settings_supported") is True)
            else:
                await self.adopt(directory, receipt, value)
            self.last_observed[directory.name] = time.monotonic()
        except (SshTransportError, TimeoutError, OSError) as error:
            self.unavailable(directory, error=error)
        except (ValueError, LookupError):
            journal(directory, "The remote installation response could not be joined to its owned source and destination. Preserve it and check the connection again.",
                    status="observation-unavailable", observation={"available": False})
        except asyncio.CancelledError:
            raise
        finally:
            self.last_observed[directory.name] = time.monotonic()

    def unavailable(self, directory, *, error=None):
        previous = read_receipt(directory)
        last = previous.get("last_known_status") if previous["status"] == "observation-unavailable" else previous["status"]
        configuration_error = isinstance(error, SshTransportError) and error.code == "configuration_unavailable"
        detail = (str(error) if configuration_error else
            "The trusted SSH connection is unavailable. The destination may still be working; recheck this installation before retrying or stopping.")
        if (not configuration_error and previous.get("target") == "gcp"
                and previous.get("remote_admission_requested") is False and previous.get("preflight") is None
                and not any(previous.get(key) for key in ("admitted", "worker_live", "service_owned", "service_observed"))
                and not any(previous.get(key) is not None for key in ("install_operation_id", "operation_id", "pending_operation", "service"))):
            from .gcp_transport import validate_gcp_endpoint
            try:
                endpoint = validate_gcp_endpoint(previous.get("endpoint"))
            except SshTransportError:
                pass
            else:
                if previous.get("cloud_operation_id") == endpoint["operation_id"]:
                    detail += (" On a new cloud VM, guest preparation may still be running. Wait, then choose Recheck remote status for this same VM. "
                        "If it remains unavailable, ask the VM administrator to verify that its original startup script completed and that Python 3.11 or newer, "
                        "venv, ensurepip and the user service manager are available. This observation does not identify the cause.")
        update_receipt(directory, status="observation-unavailable", last_known_status=last,
            observation={"available": False}, detail=detail)

    async def adopt(self, directory, receipt, value, *, expected_operation=None):
        typed_observation = value.get("status") in {"failed", "recovery-needed"}
        if expected_operation and (not value.get("error") or typed_observation) and value.get("operation_id") != expected_operation:
            raise DraftConflict("The destination did not confirm this exact remote operation. Recheck its saved custody before retrying.")
        if value.get("error") and value.get("status") not in {"failed", "recovery-needed"}:
            error = value["error"]
            if not isinstance(error, dict) or not isinstance(error.get("code"), str) or len(error["code"]) > 100:
                raise DraftConflict("The destination did not return a recognized operation result.")
            update_receipt(directory, status="recovery-needed", last_known_status=receipt.get("last_known_status") or receipt["status"],
                observation={"available": True}, remote_error_code=error["code"],
                **({"product_control": None} if receipt.get("product_runtime") is True else {}),
                detail="The destination refused this operation. Preserve the installation and recheck its ownership and operation before retrying.")
            return
        allowed = {"configuration-required", "installing", "checking", "starting", "running", "stopping", "restarting", "stopped", "failed", "not-installed", "recovery-needed"}
        status = value.get("status")
        if status not in allowed:
            raise DraftConflict("The remote operation has no recognized authoritative state.")
        if status == "failed" and (value.get("cleanup_confirmed") is not True or value.get("worker_live") is not False):
            status = "recovery-needed"
        details = {key: value[key] for key in ("checks", "progress", "service", "worker_pid", "worker_live", "source_current", "ready", "detail", "phase", "operation_id", "admitted", "service_owned", "service_observed", "cleanup_confirmed") if key in value}
        # Capabilities and attempt ancestry never inherit from an older observation.
        details.update(retryable=value.get("retryable") is True,
            retry_of_operation_id=value.get("retry_of_operation_id"),
            previous_operation_id=value.get("previous_operation_id"),
            failure_phase=value.get("failure_phase"))
        details["ready"] = status == "running" and value.get("ready") is True and value.get("source_current") is True
        # Native access authority comes from this observation. An incomplete
        # response must not inherit a previous service or worker observation.
        details.update(service=value.get("service") if isinstance(value.get("service"), dict) else {},
            service_owned=value.get("service_owned") is True, service_observed=value.get("service_observed") is True,
            worker_live=value.get("worker_live") if type(value.get("worker_live")) is bool else None,
            source_current=value.get("source_current") is True, admitted=value.get("admitted") is True)
        details.update(private_settings_supported=value.get("private_settings_supported") is True,
            private_settings_configurable=value.get("private_settings_configurable") is True)
        product = product_control_observation(receipt, value)
        details["product_control"] = product
        from .remote_browser_access import console_port
        files, _, _ = await await_validation(self.frozen, directory, receipt)
        application_console_url = (product["factory_status"].get("console_url") if product is not None
            else details["service"].get("console_url"))
        details["application_experiences"] = application_observation(files, receipt,
            value.get("application_experiences"), console_port(application_console_url))
        details.setdefault("detail", {
            "installing": "The destination admitted the accepted source and is preparing its dependencies and private runtime.",
            "checking": "The destination is running the accepted application's installation checks.",
            "starting": "The destination is checking startup of its owned native runtime and console.",
            "running": "The owned service is running. Native readiness, custom interfaces and external workflows have separate verification scopes.",
            "stopping": "The destination admitted an exact owned service stop. Recheck until its exit is observed.",
            "restarting": "The destination admitted a restart of the current accepted owned source.",
            "stopped": "The destination observed its owned service stopped. Its private configuration and data are preserved.",
            "failed": "The remote operation needs attention. Its observed service and worker custody remain visible below.",
            "not-installed": "The destination did not admit this installation. You can deliberately re-enter private setup and retry the same owned request.",
            "recovery-needed": "Remote ownership or worker cleanup needs inspection. Preserve this installation and recheck before starting replacement work.",
        }.get(status, "The destination returned its owned installation status."))
        if value.get("error"):
            error = value["error"]
            if not isinstance(error, dict) or not isinstance(error.get("code"), str) or len(error["code"]) > 100:
                raise DraftConflict("The destination did not return a recognized operation result.")
            details["remote_error_code"] = error["code"]
        pending = receipt.get("pending_operation")
        if pending is not None and not isinstance(pending, dict):
            raise DraftConflict("The saved remote operation custody is invalid. Preserve it before another action.")
        if pending and product is not None:
            request = product.get("action_request")
            installed = (pending["action"] == "install" and product["initial_install_complete"] is True
                and product["binding"]["install_operation_id"] == pending["id"])
            settled = (isinstance(request, dict) and request.get("action_request_id") == pending["id"]
                and request.get("guide_id") == receipt["guide_id"] and request.get("deployment_id") == receipt["deployment_id"]
                and request.get("action") == pending["action"] and request.get("phase") == "settled")
            if installed or settled:
                # A completed canonical operation does not claim the retained
                # control worker exited or that its whole process was cleaned up.
                details["pending_operation"] = None
        elif pending and value.get("operation_id") == pending["id"] and value.get("worker_live") is False:
            details["pending_operation"] = None
        update_receipt(directory, **details, status=status, last_known_status=status,
            observation={"available": True})

    def public(self, receipt, *, current, cloud_status=None, current_guide_version=None):
        value = deepcopy(receipt)
        value["source_binding_current"] = current
        value["current_guide_version"] = current_guide_version if current else None
        observed = (value.get("observation") or {}).get("available") is True
        can_admit = (value["status"] == "configuration-required" and not value.get("remote_admission_requested")) or (
            value["status"] == "not-installed" and value.get("admitted") is False)
        value["can_install"] = current and observed and can_admit and (value.get("preflight") or {}).get("ready") is True
        value["can_retry_install"] = current and observed and retry_observed(value)
        controlled = observed and value.get("worker_live") is False and value.get("service_owned") is True and value.get("service_observed") is True
        value["can_stop"] = bool(value.get("remote_admission_requested")) and controlled and value["status"] not in {"stopped", "recovery-needed"}
        value["can_restart"] = current and controlled and value["status"] == "stopped" and value.get("source_current") is True
        value["private_settings_supported"] = value.get("private_settings_supported") is True
        value["can_configure"] = (current and controlled and value["status"] == "stopped"
            and value.get("source_current") is True and value["private_settings_supported"]
            and value.get("private_settings_configurable") is True and not value.get("pending_operation"))
        if receipt.get("product_runtime") is True:
            value.update(can_retry_install=False, can_restart=False, can_stop=False, can_configure=False,
                can_prepare_setup=False, can_finish_setup=False, can_start_background=False,
                private_settings_supported=False, product_scope="byte-install-stopped")
            product = (product_control_observation(receipt, {"scope": "retained-original-product-owner",
                "product_control": receipt.get("product_control")}) if observed else None)
            if product is not None:
                actual = product["factory_status"]
                setup = product_setup_observation(receipt, product)
                pending = receipt.get("pending_operation") or (product.get("action_request") or {}).get("phase") in {"pending", "unknown"}
                stopped = product_stopped_configuration(receipt, product) and not pending
                available = current and stopped and isinstance(setup, dict) and setup.get("available") is True
                restore = available and setup.get("kind") == "restore"
                background = (actual.get("execution_mode") == "background" and actual.get("status") == "stopped"
                    and actual.get("source_verified") is True and actual.get("service_cleanup_confirmed") is True)
                value.update(product_scope="retained-original-product-owner", private_settings_supported=True,
                    can_configure=current and stopped,
                    can_prepare_setup=current and stopped and isinstance(setup, dict) and setup.get("action") == "prepare-setup"
                        and (available and setup.get("kind") == "fresh" or setup.get("available") is False and setup.get("reason") == "owner_reconnect"),
                    can_finish_setup=available and setup.get("action") is None and setup.get("kind") in {"fresh", "restore"},
                    can_start_background=restore and (setup.get("background") or {}).get("available") is True,
                    can_restart=current and not pending and (restore or background),
                    can_stop=not pending and actual.get("status") in {"queued", "installing", "checking", "starting", "running", "restarting"})
        if not value["can_install"] and not value["can_retry_install"]:
            value["fields"] = []
        if cloud_status is not None:
            value.update(cloud_status=cloud_status, ready=False, fields=[], can_install=False,
                can_retry_install=False, can_stop=False, can_restart=False, can_configure=False,
                **({"can_prepare_setup": False, "can_finish_setup": False, "can_start_background": False}
                    if receipt.get("product_runtime") is True else {}),
                observation={"available": False},
                detail="Cloud resource removal is in progress. Application status is the last observed destination state."
                    if cloud_status == "removing" else
                    "Cloud resource removal needs attention. Application status is the last observed destination state."
                    if cloud_status == "removal-required" else
                    "The original cloud resources were removed. Application status is historical; no graceful service stop is claimed.")
        return value

    async def _settings_selection(self, guide_id, deployment_id, version=None, *, product_owner=None):
        directory, receipt = await await_validation(self.stored, guide_id, deployment_id)
        if self.directory(guide_id) != directory:
            raise DraftConflict("Choose the selected remote installation before changing private settings.")
        await self.require_cloud_available(receipt)
        state = await self.local._guide(guide_id)
        if version is not None and (type(version) is not int or state.get("version") != version):
            raise DraftConflict("The guide changed. Reload its installed private settings before saving.")
        selected = await self.accepted(guide_id, state["version"], receipt["build_id"],
            target=receipt.get("target", "linux"), state=state,
            **({"product_owner": product_owner} if receipt.get("product_runtime") is True else {}))
        if not await self.selection_matches(receipt, selected):
            raise DraftConflict("The accepted application changed. Preserve this installation before changing private settings.")
        await self._probe(directory, "status")
        receipt = read_receipt(directory)
        if (receipt.get("observation") or {}).get("available") is not True:
            raise DraftConflict("The destination could not freshly verify private-settings access. Recheck this saved installation before saving.")
        if not receipt.get("private_settings_supported"):
            raise DraftConflict("This installation's original controller does not support guided private-settings changes. Its source and private state were preserved.")
        if not self.public(receipt, current=True)["can_configure"]:
            raise DraftConflict("Verify this installation and fully stop its owned service before changing private settings.")
        await self.require_current_request(guide_id, state["version"], files=selected["files"])
        return directory, receipt, selected

    @staticmethod
    def _settings_response(value, receipt=None):
        error = value.get("error")
        if error:
            code = error.get("code") if isinstance(error, dict) else None
            if code == "configuration-invalid":
                raise ValueError("The private settings do not match this installed package's declared fields or Console rules.")
            if code == "configuration-changed":
                raise DraftConflict("Private settings changed while saving. Reload; the newer values were preserved.")
            raise DraftConflict("The destination could not verify stopped private-settings access. Preserve this installation and check its owned status again.")
        if receipt is not None and receipt.get("product_runtime") is True:
            product = product_control_observation(receipt, value)
            if (product is None or value.get("private_settings_supported") is not True
                    or value.get("private_settings_configurable") is not True
                    or not product_stopped_configuration(value, product)):
                raise DraftConflict("The original product owner did not confirm stopped settings access.")
            return
        if (value.get("private_settings_supported") is not True
                or value.get("private_settings_configurable") is not True or value.get("status") != "stopped"
                or value.get("source_current") is not True or value.get("worker_live") is not False
                or value.get("cleanup_confirmed") is not True or value.get("service_owned") is not True
                or value.get("service_observed") is not True or (value.get("service") or {}).get("quiescent") is not True):
            raise DraftConflict("The destination did not confirm this exact stopped installation. Recheck before changing private settings.")

    async def settings_options(self, guide_id, deployment_id, *, product_owner=None):
        async with self.lock:
            directory = self.directory(guide_id, deployment_id)
            async with self.operation_lock(directory):
                directory, receipt, selected = await self._settings_selection(guide_id, deployment_id, product_owner=product_owner)
                try:
                    value, _ = await self.rpc(directory, receipt, "settings-options",
                        request_version=selected["state"]["version"])
                except (SshTransportError, TimeoutError, OSError) as error:
                    self.unavailable(directory, error=error)
                    raise DraftConflict("The destination's private settings could not be observed. Recheck this saved installation before saving.") from None
                try:
                    self._settings_response(value, receipt)
                except (ValueError, DraftConflict):
                    error = value.get("error")
                    if (error.get("code") if isinstance(error, dict) else None) not in {"configuration-invalid", "configuration-changed"}:
                        self.unavailable(directory)
                    raise
                fields = selected["fields"]
                configured = value.get("configured_keys")
                if (not isinstance(configured, list) or any(not isinstance(key, str) for key in configured)
                        or len(set(configured)) != len(configured)
                        or set(configured) - {field["key"] for field in fields}):
                    raise DraftConflict("The destination returned invalid private-settings metadata. No private values were read back.")
                port = checked_console_port(value.get("console_port"))
                await self.require_current_request(guide_id, selected["state"]["version"], files=selected["files"])
                if receipt.get("product_runtime") is True:
                    self._product_owner(product_owner)
                await self.adopt(directory, receipt, value)
                return {"version": selected["state"]["version"], "deployment_id": deployment_id,
                    "build_id": receipt["build_id"], "source_digest": receipt["accepted_source_digest"],
                    "package_sha256": receipt["package_sha256"], "guide_version": receipt["guide_version"],
                    "fields": [{**{key: item for key, item in field.items() if key != "default" or not field.get("secret")},
                        "configured": field["key"] in configured,
                        "removable": not field.get("required") and not field.get("automatic")} for field in fields],
                    "console_port": port, "console_scope": deepcopy(selected["console_scope"]),
                    "can_use_host_provider_credentials": False,
                    "source_verified": True, "supported": True, "requires_stopped": True,
                    "scope": "verified-remote-installed-private-configuration",
                    **({"remote_control_binding": deepcopy(value["product_control"]["binding"])}
                        if receipt.get("product_runtime") is True else {})}

    async def configure(self, guide_id, version, deployment_id, build_id, source_digest, package_sha256,
                        settings=None, remove_fields=None, *, action_request_id=None, product_owner=None):
        async with self.lock:
            directory = self.directory(guide_id, deployment_id)
            async with self.operation_lock(directory):
                receipt = read_receipt(directory)
                if receipt.get("product_runtime") is True:
                    return await self._configure_product(directory, receipt, version, build_id,
                        source_digest, package_sha256, settings, remove_fields,
                        action_request_id=action_request_id, product_owner=product_owner)
                directory, receipt, selected = await self._settings_selection(guide_id, deployment_id, version)
                if (build_id != receipt["build_id"] or source_digest != receipt["accepted_source_digest"]
                        or package_sha256 != receipt["package_sha256"]):
                    raise DraftConflict("The installed source changed. Reload its private settings before saving.")
                submitted = settings if settings is not None else {}
                removed = remove_fields if remove_fields is not None else ()
                try:
                    private_settings_input(selected["fields"], submitted, removed)
                except ControllerError:
                    raise ValueError("Choose declared private replacements and optional removals; automatic settings cannot be changed here.") from None
                values = dict(submitted)
                try:
                    value, _ = await self.rpc(directory, receipt, "configure", request_version=version,
                        settings=values, remove_fields=list(removed))
                except (SshTransportError, TimeoutError, OSError) as error:
                    self.unavailable(directory, error=error)
                    return await self.current_public(read_receipt(directory))
                finally:
                    values.clear()
                try:
                    self._settings_response(value)
                except (ValueError, DraftConflict):
                    error = value.get("error")
                    if (error.get("code") if isinstance(error, dict) else None) not in {"configuration-invalid", "configuration-changed"}:
                        self.unavailable(directory)
                    raise
                if value.get("configuration_saved") is not True:
                    self.unavailable(directory)
                    return await self.current_public(read_receipt(directory))
                await self.adopt(directory, receipt, value)
                journal(directory, "Private settings saved. This remote application remains stopped; restart it explicitly when ready.",
                    ready=False, detail="Private settings saved. This remote application remains stopped; restart it explicitly when ready.")
                return {**await self.current_public(read_receipt(directory)), "configuration_saved": True}

    def _product_owner(self, product_owner):
        from .remote_product_control import ProductControlSelection
        selected = self.product_control_selection
        if (type(selected) is not ProductControlSelection or selected is not self._product_control_selection
                or product_owner is not selected):
            raise DraftConflict(PRODUCT_RUNTIME_UNSUPPORTED)
        selected.require_current()
        return selected

    async def _product_request_readback(self, directory, receipt, action, action_request_id, *, product_owner):
        """A lost reply observes the canonical destination journal, never reposts."""
        if not operation_identity(action_request_id):
            raise ValueError("Product actions require their exact canonical request UUID.")
        self._product_owner(product_owner)
        pending = receipt.get("pending_operation")
        if pending and pending != {"id": action_request_id, "action": action}:
            await self._probe(directory, "status")
            receipt = read_receipt(directory)
            if receipt.get("pending_operation"):
                raise DraftConflict("Recheck the original pending product action before requesting another action.")
        try:
            value, _ = await self.rpc(directory, receipt, "status", operation_id=action_request_id)
        except (SshTransportError, TimeoutError, OSError) as error:
            self.unavailable(directory, error=error)
            return read_receipt(directory), False
        self._product_owner(product_owner)
        product = product_control_observation(receipt, value)
        if product is None:
            raise DraftConflict("This installation has no authenticated retained product owner.")
        request = product_action_request(receipt, product, action, action_request_id)
        await self.adopt(directory, receipt, value)
        receipt = read_receipt(directory)
        # Even an absent journal cannot authorize redelivery of an already
        # posted UUID. The original request may still be in transport.
        return receipt, request is None and not receipt.get("pending_operation")

    async def _product_result(self, receipt, action, action_request_id):
        result = await self.current_public(receipt)
        request = (receipt.get("product_control") or {}).get("action_request") or {}
        if (action == "configure" and result.get("observation", {}).get("available") is True
                and request.get("action") == action and request.get("action_request_id") == action_request_id
                and request.get("phase") == "settled" and request.get("outcome") == "returned"):
            result["configuration_saved"] = True
        return result

    async def _product_send(self, directory, receipt, action, action_request_id, *, product_owner, version=None, **extra):
        intent_started = False

        def publish_intent():
            nonlocal receipt, intent_started
            self._product_owner(product_owner)
            # A failure during publication is conservative: do not certify
            # non-delivery from an incomplete local journal write.
            intent_started = True
            receipt = journal(directory, "Requesting the original installed product action.",
                pending_operation={"id": action_request_id, "action": action})

        try:
            self._product_owner(product_owner)
            await self.require_cloud_available(receipt)
            self._product_owner(product_owner)
            value, _ = await self.rpc(directory, receipt, action, operation_id=action_request_id,
                request_version=version, _publish_product_intent=publish_intent, **extra)
            self._product_owner(product_owner)
            product = product_control_observation(receipt, value)
            request = product_action_request(receipt, product, action, action_request_id)
            if product is None or request is None:
                raise DraftConflict("The product owner did not confirm this exact canonical action request.")
            await self.adopt(directory, receipt, value, expected_operation=action_request_id)
        except (ValueError, LookupError):
            if not intent_started:
                raise RemoteProductActionNotSent(action, action_request_id) from None
            raise
        except (SshTransportError, TimeoutError, OSError) as error:
            if not intent_started:
                raise RemoteProductActionNotSent(action, action_request_id) from None
            self.unavailable(directory, error=error)
        return await self._product_result(read_receipt(directory), action, action_request_id)

    async def _configure_product(self, directory, receipt, version, build_id, source_digest,
            package_sha256, settings, remove_fields, *, action_request_id, product_owner):
        self._product_owner(product_owner)
        await self.require_cloud_available(receipt)
        if self.directory(receipt["guide_id"]) != directory:
            raise DraftConflict("Choose the selected remote installation before changing private settings.")
        if (build_id != receipt["build_id"] or source_digest != receipt["accepted_source_digest"]
                or package_sha256 != receipt["package_sha256"]):
            raise DraftConflict("The installed source changed. Reload its private settings before saving.")
        receipt, new_request = await self._product_request_readback(directory, receipt, "configure", action_request_id, product_owner=product_owner)
        if not new_request:
            return await self._product_result(receipt, "configure", action_request_id)
        values = {}
        try:
            try:
                # Canonical readback has proved this is a new request. Only its
                # remaining local preparation can certify non-delivery here.
                directory, receipt, selected = await self._settings_selection(receipt["guide_id"], receipt["deployment_id"],
                    version, product_owner=product_owner)
                submitted = settings if settings is not None else {}
                removed = remove_fields if remove_fields is not None else ()
                try:
                    private_settings_input(selected["fields"], submitted, removed)
                except ControllerError:
                    raise ValueError("Choose declared private replacements and optional removals; automatic settings cannot be changed here.") from None
                await self.require_current_request(receipt["guide_id"], version, files=selected["files"])
                values = dict(submitted)
                removed = list(removed)
            except (ValueError, LookupError, SshTransportError, TimeoutError, OSError):
                raise RemoteProductActionNotSent("configure", action_request_id) from None
            # Delivery owns its own publication marker and uncertain outcomes;
            # the local preparation classifier must never wrap this await.
            return await self._product_send(directory, receipt, "configure", action_request_id,
                product_owner=product_owner, version=version, settings=values, remove_fields=removed)
        finally:
            values.clear()

    async def application_setup(self, guide_id, deployment_id, *, product_owner=None):
        async with self.lock:
            directory = self.directory(guide_id, deployment_id)
            async with self.operation_lock(directory):
                directory, receipt, selected = await self._settings_selection(guide_id, deployment_id, product_owner=product_owner)
                if receipt.get("product_runtime") is not True:
                    raise DraftConflict("Guided application setup requires the original retained product owner.")
                try:
                    value, _ = await self.rpc(directory, receipt, "application-setup", request_version=selected["state"]["version"])
                except (SshTransportError, TimeoutError, OSError) as error:
                    self.unavailable(directory, error=error)
                    raise DraftConflict("Application setup could not be freshly checked. Preserve this installation and recheck its status.") from None
                product = product_control_observation(receipt, value)
                setup = product_setup_observation(receipt, product)
                if product is None or setup is None or not product_stopped_configuration(value, product):
                    raise DraftConflict("The product owner did not confirm these exact stopped setup inputs.")
                await self.require_current_request(guide_id, selected["state"]["version"], files=selected["files"])
                self._product_owner(product_owner)
                await self.adopt(directory, receipt, value)
                return {**setup, "source_digest": receipt["accepted_source_digest"],
                    "remote_control_binding": deepcopy(product["binding"])}

    async def _product_action(self, directory, receipt, action, *, action_request_id, product_owner):
        if action not in {"prepare-setup", "finish-setup", "start-background", "stop", "restart"}:
            raise ValueError("Choose an original installed product setup or runtime action.")
        self._product_owner(product_owner)
        await self.require_cloud_available(receipt)
        if self.directory(receipt["guide_id"]) != directory or not receipt.get("remote_admission_requested"):
            raise DraftConflict("Choose this selected installed product before changing its runtime.")
        receipt, new_request = await self._product_request_readback(directory, receipt, action, action_request_id, product_owner=product_owner)
        if not new_request:
            return await self._product_result(receipt, action, action_request_id)
        snapshot = await self.current_state(receipt)
        current = snapshot is not None
        if action != "stop":
            if not current:
                raise DraftConflict("Start only the current accepted installed source. The older installation can still be stopped.")
            selected = await self.accepted(receipt["guide_id"], snapshot["version"], receipt["build_id"],
                target=receipt.get("target", "linux"), state=snapshot, product_owner=product_owner)
            if not await self.selection_matches(receipt, selected):
                raise DraftConflict("The accepted product changed before its runtime action.")
            await self.require_current_request(receipt["guide_id"], snapshot["version"], files=selected["files"])
        capability = {"prepare-setup": "can_prepare_setup", "finish-setup": "can_finish_setup",
            "start-background": "can_start_background", "stop": "can_stop", "restart": "can_restart"}[action]
        if not self.public(receipt, current=current).get(capability):
            raise DraftConflict("Recheck this original product's setup and runtime before requesting this action.")
        await self.browser_access.revoke(receipt["guide_id"], receipt["deployment_id"],
            "Browser access is closed while the original installed application changes.")
        return await self._product_send(directory, receipt, action, action_request_id,
            product_owner=product_owner, version=snapshot["version"] if action != "stop" else None)

    async def status(self, guide_id, *, force=False):
        return await self._status(guide_id, force=force)

    async def _observation_cloud_status(self, receipt, *, interview):
        try:
            return await self.cloud_status(receipt), False
        except DraftConflict:
            if not interview:
                raise
            # Only the original cloud observer's failure is handled here.
            # Package/source custody failures still propagate independently.
            return None, True

    async def _observation_public(self, receipt, current, *, interview, current_guide_version=None):
        cloud, unavailable = await self._observation_cloud_status(receipt, interview=interview)
        value = self.public(receipt, current=current, cloud_status=cloud, current_guide_version=current_guide_version)
        if unavailable:
            value.update(ready=False, fields=[], can_install=False, can_retry_install=False,
                can_stop=False, can_restart=False, can_configure=False, observation={"available": False},
                **({"can_prepare_setup": False, "can_finish_setup": False, "can_start_background": False}
                    if receipt.get("product_runtime") is True else {}))
        return value

    async def _status(self, guide_id, *, force=False, state=None, interview=False):
        # Reading and hashing the saved package and controller must not hold
        # the event loop.
        directory, receipt = await await_validation(self.stored, guide_id)
        # An interview observes admitted work only. It neither starts another
        # destination preflight nor reconciles any browser connection.
        observe = not interview or receipt.get("remote_admission_requested") is True
        cloud, cloud_unavailable = await self._observation_cloud_status(receipt, interview=interview)
        if observe and not cloud_unavailable and cloud is None and receipt["deployment_id"] not in self.tasks and (force or time.monotonic() - self.last_observed.get(directory.name, -100) >= 3):
            async with self.operation_lock(directory):
                receipt = read_receipt(directory)
                if force or time.monotonic() - self.last_observed.get(directory.name, -100) >= 3:
                    await self._probe(directory, "status" if receipt.get("remote_admission_requested") else "preflight")
        receipt = read_receipt(directory)
        snapshot = await self.current_state(receipt, state=state)
        current = snapshot is not None
        value = await self._observation_public(receipt, current, interview=interview,
            current_guide_version=snapshot["version"] if current else None)
        if snapshot is None or (snapshot.get("operation") or {}).get("status") == "running":
            value["can_configure"] = False
            if receipt.get("product_runtime") is True:
                value.update(can_prepare_setup=False, can_finish_setup=False, can_start_background=False, can_restart=False)
        if not interview:
            await self.browser_access.reconcile(value, current)
        return value

    async def guide_status(self, guide_id, *, state):
        """Observe saved remote custody without guide reentry or browser changes."""
        if not isinstance(state, dict) or state.get("id") != guide_id:
            raise ValueError("Remote evidence requires the matching detached guide state.")
        value = await self._status(guide_id, state=state, interview=True)
        directory, receipt = await await_validation(self.stored, guide_id)
        if receipt["deployment_id"] != value["deployment_id"] or binding(receipt) != binding(value):
            raise DraftConflict("The selected remote installation changed during its observation.")
        value = await self._observation_public(receipt, value["source_binding_current"], interview=True)
        files, _, _ = await await_validation(self.frozen, directory, receipt)
        report = json.loads(files["implementation-report.json"][0])
        source = report.get("verification", {}).get("digest")
        if (report.get("build_id") != receipt["build_id"]
                or report.get("guide_version") != build_guide_version(receipt)
                or not isinstance(source, str) or not HASH.fullmatch(source)
                or source != receipt["accepted_source_digest"]):
            raise DraftConflict("The saved remote installation's accepted package binding could not be verified.")
        # Saved ready flags do not establish fresh availability after a restart
        # or while another observer owns this installation.
        age = time.monotonic() - self.last_observed.get(directory.name, -100)
        value.update(owned_package_verified=True,
            current_guide_version=state["version"] if value["source_binding_current"] else None,
            observation_fresh=0 <= age < 3 and directory.name not in self.tasks)
        return value

    async def install(self, guide_id, version, build_id, deployment_id, settings, *, product_owner=None):
        async with self.lock:
            directory, receipt = await await_validation(self.stored, guide_id, deployment_id)
            async with self.operation_lock(directory):
                receipt = read_receipt(directory)
                await self.require_cloud_available(receipt)
                if self.directory(guide_id) != directory:
                    raise DraftConflict("Choose the selected remote installation before entering private setup.")
                selected = await self.accepted(guide_id, version, build_id, target=receipt.get("target", "linux"), product_owner=product_owner)
                if not await self.selection_matches(receipt, selected):
                    raise DraftConflict("The application changed. Connect its current accepted source before installing.")
                continuing_product = selected.get("product_control") is not None and product_transfer_pending(receipt)
                retry = receipt.get("remote_admission_requested") and receipt["status"] == "not-installed" and receipt.get("admitted") is False
                if receipt.get("remote_admission_requested") and not retry and not continuing_product:
                    await self._probe(directory, "status")
                    receipt = read_receipt(directory)
                    return await self.current_public(receipt)
                if not continuing_product and not self.public(receipt, current=True)["can_install"]:
                    raise DraftConflict("Complete the exact destination check before private setup.")
                port = selected["spec"]["port"]
                if (retry or continuing_product) and "requested_console_port" in receipt:
                    port = checked_console_port(receipt["requested_console_port"])
                validator = directory / ("settings-validation-" + uuid.uuid4().hex)
                values = prepare_private_settings(validator, selected["fields"], settings, False,
                    port=port, console_authentication=selected["console_scope"]["authentication"])
                try:
                    if continuing_product:
                        # The original source intake may already own a nonempty
                        # project. Its exact repeated requests recheck that source,
                        # observed account and SSH pin; a generic empty-folder
                        # preflight cannot authorize or replace this custody.
                        await self.require_current_request(guide_id, version, files=selected["files"])
                        receipt = read_receipt(directory)
                        if not product_transfer_pending(receipt):
                            raise DraftConflict("The original product transfer request changed. Preserve its custody.")
                    else:
                        await self._probe(directory, "preflight")
                        receipt = read_receipt(directory)
                        if receipt["status"] != "configuration-required" or not await self.current(receipt):
                            raise DraftConflict("The destination or application changed before installation. Check it again.")
                    if selected.get("product_control") is not None:
                        return await self._install_product(directory, receipt, selected, version, values)
                    _, archive, _ = await await_validation(self.frozen, directory, receipt)
                    operation_id = receipt.get("install_operation_id") or str(uuid.uuid4())
                    await self.require_cloud_available(receipt)
                    await self.require_current_request(guide_id, version, files=selected["files"])
                    # Custody and nonsecret requested port intent are durable
                    # before SSH admission. Private values are not stored here.
                    journal(directory, "Sending the exact accepted package and private setup through the verified SSH connection.",
                        status="installing", remote_admission_requested=True, install_operation_id=operation_id,
                        requested_console_port=checked_console_port(int(values["MOBKIT_PORT"])),
                        pending_operation={"id": operation_id, "action": "install"})
                    receipt = read_receipt(directory)
                    try:
                        value, _ = await self.rpc(directory, receipt, "install", operation_id=operation_id,
                            request_version=version,
                            archive=base64.b64encode(archive).decode("ascii"), settings=values)
                        await self.adopt(directory, receipt, value, expected_operation=operation_id)
                    except (SshTransportError, TimeoutError, OSError, ValueError, LookupError) as error:
                        self.unavailable(directory, error=error)
                finally:
                    values.clear()
                receipt = read_receipt(directory)
                return await self.current_public(receipt)

    async def _install_product(self, directory, receipt, selected, version, values):
        """Existing origin admission/journal, complete custody, then actual stopped Install."""
        from .remote_product_artifacts import transfer_product_artifacts
        from .runtime_candidates import ArtifactFile
        manifest_bytes = private_file(directory / "product-control.json", 1024 * 1024)
        manifest = json.loads(manifest_bytes)
        selection = selected["product_control"]
        operation_id = receipt.get("install_operation_id") or str(uuid.uuid4())
        original_binding, endpoint = binding(receipt), deepcopy(receipt["endpoint"])
        original_fingerprint = receipt["host_key_fingerprint"]
        expected_host = {key: receipt["host"][key] for key in ("uid", "home", "destination")}
        transport = self.cloud_transport if receipt.get("target") == "gcp" else self.transport
        if receipt.get("remote_admission_requested"):
            if not product_transfer_pending(receipt):
                raise DraftConflict("The original product Install already reached admission. Observe its saved request.")
            # Reuse the same immutable transaction/source-intake records below.
            # Missing records mean the original call had not yet retained them;
            # existing mismatched/partial intake records retain their own refusal.
        else:
            journal(directory, "Retaining the original product transfer and stopped installation request.",
                status="installing", remote_admission_requested=True, install_operation_id=operation_id,
                requested_console_port=checked_console_port(int(values["MOBKIT_PORT"])),
                pending_operation={"id": operation_id, "action": "install"}, product_phase="source-intake")
        def current():
            latest = read_receipt(directory)
            if (binding(latest) != original_binding or latest.get("endpoint") != endpoint
                    or latest.get("host_key_fingerprint") != original_fingerprint
                    or latest.get("pending_operation") != {"id": operation_id, "action": "install"}
                    or self.product_control_selection is not selection or self._product_control_selection is not selection):
                raise DraftConflict("The original product remote admission owner changed.")
            selection.require_current()
        current()
        for name in ("artifact-transfer", "source-transfer"):
            (directory / name).mkdir(mode=0o700, exist_ok=True)
        try:
            custody = await transfer_product_artifacts(self.implementation, receipt["guide_id"],
                build_id=receipt["build_id"], source_digest=receipt["accepted_source_digest"],
                transaction_id=receipt["deployment_id"], transaction_directory=directory / "artifact-transfer",
                destination_host=expected_host, transport=transport, endpoint=endpoint,
                expected_fingerprint=original_fingerprint, require_current=current)
            current()
            cloud = None
            if receipt.get("target") == "gcp":
                cloud_bytes = private_file(directory / "product-cloud.json", 1024 * 1024)
                if hashlib.sha256(cloud_bytes).hexdigest() != receipt["product_cloud_sha256"]:
                    raise DraftConflict("The original product cloud operation changed.")
                cloud = json.loads(cloud_bytes)
            await self.stage_product_source(receipt["guide_id"], build_id=receipt["build_id"],
                source_digest=receipt["accepted_source_digest"], guide_version=selected["checked"]["guide_version"],
                deployment_id=receipt["deployment_id"], custody=custody,
                transaction_directory=directory / "source-transfer", destination_host=expected_host,
                transport=transport, endpoint=endpoint, expected_fingerprint=original_fingerprint,
                require_current=current, cloud=cloud)
            current()
            await self.require_current_request(receipt["guide_id"], version, files=selected["files"])
            _, _, content = await await_validation(self.frozen, directory, read_receipt(directory))
            body = {"action": "product-control", "deployment_id": receipt["deployment_id"],
                "binding": original_binding, "destination": receipt["destination"], "expected_host": expected_host}
            artifact = ArtifactFile(directory / "control-bundle.zip", manifest["bundle"]["size"],
                manifest["bundle"]["sha256"])
            journal(directory, "Continuing the original selected control-source transfer.",
                product_phase="control-transfer")
            current()
            value = await transport.transfer_file(endpoint, controller_program(content), body, artifact,
                timeout=600, expected_fingerprint=original_fingerprint, require_current=current)
            current()
            if (value.get("control_source_retained") is not True or value.get("deployment_id") != receipt["deployment_id"]
                    or value.get("binding") != original_binding
                    or (value.get("_transport") or {}).get("host_key_fingerprint") != original_fingerprint):
                raise DraftConflict("The destination did not retain its exact product control source.")
            await self.require_current_request(receipt["guide_id"], version, files=selected["files"])
            current()
            journal(directory, "The complete original product/control bytes are retained; admitting stopped Install.",
                product_phase="byte-install")
            fresh = read_receipt(directory)
            value, _ = await self.rpc(directory, fresh, "install", operation_id=operation_id,
                request_version=version, settings=values)
            await self.adopt(directory, fresh, value, expected_operation=operation_id)
        except (SshTransportError, TimeoutError, OSError, ValueError, LookupError) as error:
            # Before byte-install the same explicit private request may continue
            # its original transfer intents. After that boundary, a reply loss
            # only observes the original UUID and never replays Fresh/Install.
            self.unavailable(directory, error=error)
        return await self.current_public(read_receipt(directory))

    async def retry_install(self, guide_id, version, build_id, deployment_id, previous_operation_id, settings):
        if not operation_identity(previous_operation_id):
            raise ValueError("Choose the exact failed installation attempt before retrying.")
        async with self.lock:
            directory, receipt = await await_validation(self.stored, guide_id, deployment_id)
            async with self.operation_lock(directory):
                await self.require_cloud_available(receipt)
                if self.directory(guide_id) != directory:
                    raise DraftConflict("Choose the selected remote installation before retrying private setup.")
                selected = await self.accepted(guide_id, version, build_id, target=receipt.get("target", "linux"))
                if not await self.selection_matches(receipt, selected):
                    raise DraftConflict("The application changed. Retry only its exact accepted installed source.")
                # A POST replay is an observation until the predecessor is proven
                # eligible now. It never turns an expired response into fresh work.
                await self._probe(directory, "status")
                receipt = read_receipt(directory)
                if receipt.get("operation_id") != previous_operation_id:
                    if (receipt.get("previous_operation_id") == previous_operation_id
                            and receipt.get("operation_id") == receipt.get("retry_install_operation_id")):
                        return await self.current_public(receipt)
                    raise DraftConflict("The failed attempt changed. Recheck its current owned status before retrying.")
                if not self.public(receipt, current=True)["can_retry_install"]:
                    raise DraftConflict("Retry needs a verified failed installer, completed cleanup and its unchanged accepted source.")
                pending = receipt.get("pending_operation")
                if pending and (pending.get("action") != "retry-install"
                        or pending.get("previous_operation_id") != previous_operation_id
                        or not operation_identity(pending.get("id"))):
                    raise DraftConflict("Recheck the pending remote operation before requesting another attempt.")
                # Refresh host identity and prerequisites without replacing the
                # failed attempt with an initial-setup projection.
                try:
                    checked, _ = await self.rpc(directory, receipt, "preflight",
                        knowledge_mode=selected["spec"]["knowledge"]["mode"],
                        **connection_prerequisites(selected["spec"]))
                except (SshTransportError, TimeoutError, OSError) as error:
                    self.unavailable(directory, error=error)
                    receipt = read_receipt(directory)
                    return await self.current_public(receipt)
                host = checked.get("host")
                if (not isinstance(host, dict) or any(host.get(key) != receipt["host"].get(key)
                        for key in ("uid", "home", "destination"))):
                    raise DraftConflict("The destination identity changed. No private retry settings were sent.")
                try:
                    python = tuple(int(part) for part in host["python"].split(".")[:2])
                except (KeyError, AttributeError, ValueError):
                    python = ()
                if host.get("platform") != "linux" or python < (3, 11):
                    raise DraftConflict("Retry requires the same verified Linux destination with Python 3.11 or newer.")
                update_receipt(directory, retry_checks=checked.get("checks", []),
                    retry_preflight_ready=checked.get("ready") is True)
                if checked.get("ready") is not True or not await self.current(receipt):
                    raise DraftConflict("The destination or accepted source needs attention before another installation attempt.")
                validator = directory / ("settings-validation-" + uuid.uuid4().hex)
                values = prepare_private_settings(validator, selected["fields"], settings, False,
                    port=selected["spec"]["port"], console_authentication=selected["console_scope"]["authentication"])
                requested_port = {}
                if settings.get("MOBKIT_PORT"):
                    requested_port["requested_console_port"] = checked_console_port(int(values["MOBKIT_PORT"]))
                else:
                    # An admitted installation's private .env remains the
                    # authority unless this retry explicitly replaces its port.
                    values.pop("MOBKIT_PORT", None)
                try:
                    operation_id = pending["id"] if pending else str(uuid.uuid4())
                    await self.require_cloud_available(receipt)
                    await self.require_current_request(guide_id, version, files=selected["files"])
                    journal(directory, "Admitting a deliberate retry of the exact failed installation; existing data and prior attempt evidence are preserved.",
                        status="installing", retry_install_operation_id=operation_id,
                        retry_install_previous_operation_id=previous_operation_id,
                        **requested_port,
                        pending_operation={"id": operation_id, "action": "retry-install",
                            "previous_operation_id": previous_operation_id})
                    receipt = read_receipt(directory)
                    try:
                        result, _ = await self.rpc(directory, receipt, "retry-install", operation_id=operation_id,
                            request_version=version,
                            previous_operation_id=previous_operation_id, settings=values)
                        await self.adopt(directory, receipt, result, expected_operation=operation_id)
                    except (SshTransportError, TimeoutError, OSError, ValueError, LookupError) as error:
                        self.unavailable(directory, error=error)
                finally:
                    values.clear()
                receipt = read_receipt(directory)
                return await self.current_public(receipt)

    async def action(self, guide_id, deployment_id, action, *, action_request_id=None, product_owner=None):
        async with self.lock:
            directory, receipt = await await_validation(self.stored, guide_id, deployment_id)
            async with self.operation_lock(directory):
                receipt = read_receipt(directory)
                await self.require_cloud_available(receipt)
                if receipt.get("product_runtime") is True:
                    return await self._product_action(directory, receipt, action,
                        action_request_id=action_request_id, product_owner=product_owner)
                if action not in {"stop", "restart"}:
                    raise ValueError("Choose stop or restart for this owned remote installation.")
                if not receipt.get("remote_admission_requested"):
                    raise DraftConflict("This remote installation has not admitted an application yet.")
                snapshot = await self.current_state(receipt)
                current = snapshot is not None
                if action == "restart" and not current:
                    raise DraftConflict("Restart only the current accepted installed source. The older installation can still be stopped.")
                pending = receipt.get("pending_operation")
                if pending:
                    if pending["action"] != action:
                        raise DraftConflict("Recheck the pending remote operation before requesting a different control action.")
                    operation_id = pending["id"]
                else:
                    if not self.public(receipt, current=current)["can_" + action]:
                        raise DraftConflict("Recheck this owned service before requesting its control operation.")
                    operation_id = str(uuid.uuid4())
                await self.browser_access.revoke(guide_id, deployment_id,
                    "Browser access is closed while the owned remote service changes. Reconnect after its readiness is verified.")
                await self.require_cloud_available(receipt)
                if action == "restart":
                    files, _, _ = await await_validation(self.frozen, directory, receipt)
                    await self.require_current_request(guide_id, snapshot["version"], files=files)
                journal(directory, "Requesting an owned remote service operation.", status="stopping" if action == "stop" else "restarting",
                        pending_operation={"id": operation_id, "action": action})
                receipt = read_receipt(directory)
                try:
                    value, _ = await self.rpc(directory, receipt, action, operation_id=operation_id,
                        request_version=snapshot["version"] if action == "restart" else None)
                    await self.adopt(directory, receipt, value, expected_operation=operation_id)
                except (SshTransportError, TimeoutError, OSError, ValueError, LookupError) as error:
                    self.unavailable(directory, error=error)
                receipt = read_receipt(directory)
                return await self.current_public(receipt)

    async def use(self, guide_id, deployment_id=None):
        from .remote_installed_feedback import remote_application_use
        return await remote_application_use(self, guide_id, deployment_id)

    async def record_success(self, guide_id, version, deployment_id, build_id, source_digest,
                             package_sha256, *, request_id, task):
        from .installed_feedback import record_remote_success
        return await record_remote_success(self, guide_id, version, deployment_id, build_id,
            source_digest, package_sha256, request_id=request_id, task=task)

    async def feedback(self, guide_id, version, remote_binding, continuation_build_id,
                       continuation_source_digest, *, request, outcome, task="",
                       continue_current_candidate=False):
        from .remote_installed_feedback import improve_remote_application
        return await improve_remote_application(self, guide_id, version, remote_binding,
            continuation_build_id, continuation_source_digest, request=request, outcome=outcome,
            task=task, continue_current_candidate=continue_current_candidate)

    async def shutdown(self):
        await self.browser_access.shutdown()
        tasks = list(self.tasks.values())
        for task in tasks:
            task.cancel()
        await asyncio.gather(*tasks, return_exceptions=True)
        # Remote workers/services keep their destination-owned custody. Shutdown
        # closes local observation only and never claims a remote stop.
