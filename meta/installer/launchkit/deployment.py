"""User-admitted local deployment of an immutable, checked application package."""
from __future__ import annotations

import asyncio
from copy import deepcopy
from contextlib import contextmanager, nullcontext
import ctypes
from datetime import datetime, timezone
import fcntl
import hashlib
import io
import json
import logging
import os
from pathlib import Path, PurePosixPath
import platform
import re
import stat
import sys
import tempfile
import uuid
import zipfile
from urllib.parse import quote, urlencode

from .projects import DraftConflict, fingerprint
from .templates.storage_setup import project_lock, write_private
from .templates_v2.application_configuration import validate_application_fields, validate_configuration_values
from .templates_v2.console_configuration import (console_configuration,
    console_authentication as declared_console_authentication, validate_console_configuration_values)
from .artifact_binding import package_matches, report_brief_hash, matches as artifact_matches, current_version, proposal_hash
from .deployment_deadlines import (WORKER_STOP_TIMEOUT, PROCESS_FORCE_REAP_TIMEOUT,
                                   reported_gateway_horizon, shutdown_budget)
from .deployment_ports import observe_local_console_port, require_available_console_port, installed_console_port
from .deployment_sources import compile_native_source, read_package_source, native_fields, NATIVE_SOURCE, NATIVE_KEYS

ACTIVE = {"queued", "installing", "checking", "starting", "running", "stopping"}
PROVIDER_KEYS = {"OPENAI_API_KEY", "ANTHROPIC_API_KEY", "GEMINI_API_KEY"}
MAX_PACKAGE_BYTES = 64 * 1024 * 1024
OWNER_FILE = ".mobkit/launch-deployment-owner.json"
PRIVATE_RUNTIME_UPDATE_ONLY = "This private runtime candidate is admitted only through the checked update of an existing stopped installation. Use that exact update or its forward recovery."

PRODUCT_CONFIGURE_REQUIRED = "This selected Meta/Toolkit installation needs its original Configure startup inputs. Ordinary Start and background startup are unavailable."
LOG = logging.getLogger(__name__)


def forge_interpreter():
    """Forge's own CPython: the worker creates every application venv from it."""
    tag = f"cp{sys.version_info.major}{sys.version_info.minor}"
    try:
        libc = os.confstr("CS_GNU_LIBC_VERSION") or ""
    except (AttributeError, ValueError, OSError):
        libc = ""
    glibc = re.fullmatch(r"glibc ([0-9]+)\.([0-9]+).*", libc)
    macos = re.match(r"([0-9]+)\.([0-9]+)", platform.mac_ver()[0] + ".0")
    return {"python_tag": tag, "abi_tag": tag, "implementation": "cpython",
        "system": platform.system(), "machine": platform.machine(),
        "glibc_minimum": [int(glibc[1]), int(glibc[2])] if glibc else [0, 0],
        "macos": (int(macos[1]), int(macos[2])) if macos else (0, 0)}


def selected_interpreter_refusal(target, interpreter=None):
    """Why Forge's interpreter cannot install the selected dependency wheels, or None.

    The application venv inherits Forge's interpreter, so an incompatible
    compiled wheel would otherwise fail midway, after the selected SDK pair
    is already installed in that venv.
    """
    from .remote_product_control import _wheel_compatible
    host = forge_interpreter() if interpreter is None else interpreter

    def macos_platform(tag):
        found = re.fullmatch(r"macosx_([0-9]+)_([0-9]+)_([a-z0-9_]+)", tag)
        return tag == "any" or (found is not None and found[3] in {host["machine"], "universal2"}
            and (int(found[1]), int(found[2])) <= host["macos"])

    def tags(name):
        return [part.split(".") for part in name.removesuffix(".whl").split("-")[-3:]]

    def abi_compatible(name):
        return _wheel_compatible(name.removesuffix(".whl").rpartition("-")[0] + "-any.whl", host)

    def compatible(name):
        if host["system"] == "Linux":
            return _wheel_compatible(name, host)
        # _wheel_compatible knows only Linux platform tags: it checks the python
        # and ABI tags, and the macOS platform tags are checked here.
        return abi_compatible(name) and any(map(macos_platform, tags(name)[2]))

    refused = [name for name in (Path(wheel.path).name for wheel in target.dependency_wheels) if not compatible(name)]
    if not refused:
        return None
    running = int(host["python_tag"][3:])
    # Only a wheel refused for its own CPython ABI names the CPython it needs;
    # an abi3 or platform-only refusal does not.
    needed = sorted({int(tag[3:]) for name in refused if not abi_compatible(name) and "abi3" not in tags(name)[1]
        for tag in tags(name)[0] if re.fullmatch(r"cp3[0-9]+", tag)} - {running})
    if needed:
        return (f"This product's application packages are built for CPython {' or '.join(f'3.{minor}' for minor in needed)}, "
            f"but this Forge runs CPython 3.{running}, so installing them would fail midway. Stop Forge, move only its "
            f".venv folder aside and start it again with PYTHON_BIN set to a CPython 3.{needed[0]} interpreter, as the "
            "README describes. Your saved setups are kept.")
    return f"This product's application packages do not support this computer: {', '.join(refused[:3])}."


def selected_host_refusal(glibc_floor, interpreter=None):
    """Why this computer cannot run the selected product's native programs, or None.

    A Linux product declares the oldest glibc they run on. Below it every
    package still installs, and the application fails only when it starts.
    """
    if glibc_floor is None:
        return None
    floor = tuple(glibc_floor)
    host = forge_interpreter() if interpreter is None else interpreter
    found = tuple(host["glibc_minimum"])
    if host["system"] == "Linux" and found >= floor:
        return None
    needed = f"glibc {floor[0]}.{floor[1]}"
    present = f"glibc {found[0]}.{found[1]}" if host["system"] == "Linux" and found != (0, 0) else "no glibc"
    return (f"This product's native programs need {needed} or newer, but this computer has {present}, so the "
        f"application would install and then fail when it starts. Install it on a Linux computer with {needed} "
        "or newer, or upgrade this computer's operating system.")


def selected_update_refusal(target, interpreter=None):
    """Why this computer cannot install an update to the selected product, or None.

    Stage asks before it writes anything. The glibc floor comes from the
    pinned assembly and resource build record alone; the install worker
    refuses again on the fully read product.
    """
    from .runtime_candidates import declared_glibc_floor
    return (selected_host_refusal(declared_glibc_floor(target), interpreter)
        or selected_interpreter_refusal(target, interpreter))


def staged_runtime_refusal(update):
    """Why this computer cannot install a staged update's selected product, or None.

    The product's declared glibc floor and this Forge's interpreter are both
    checked, as Stage checks them, before the worker moves the runtime aside.
    """
    selection = (update.get("runtime_dependency_update") or {}).get("runtime_candidate")
    if selection is None:
        return None
    from .runtime_selection import MetaToolkitRuntimeTarget, parse_candidate_selection
    target = parse_candidate_selection(selection)
    return selected_update_refusal(target) if type(target) is MetaToolkitRuntimeTarget else None


def product_runtime_source(files, selected, *, verified_artifacts=None, source_custody=None):
    """Join an owner's retained product selection to its exact accepted source.

    The caller supplies the implementation or original installation authority.
    This byte reader cannot admit a target from an authored package marker.
    """
    from .runtime_selection import (MetaToolkitRuntimeTarget, parse_candidate_selection,
        candidate_selection, selected_runtime_artifacts, selected_runtime_version, selected_runtime_marker)
    from .deployment_updates import _candidate_source_selection, _same_toml
    target = parse_candidate_selection(selected)
    if type(target) is not MetaToolkitRuntimeTarget:
        raise DraftConflict(PRIVATE_RUNTIME_UPDATE_ONLY)
    definition = json.loads(files["deployment.json"][0])
    report = json.loads(files["implementation-report.json"][0])
    if (not _same_toml(_candidate_source_selection(files,
                **({"source_custody": source_custody} if source_custody is not None else {})), selected)
            or not _same_toml(report.get("framework", {}).get("runtime_candidate"), selected)
            or definition.get("mobkitVersion") != selected_runtime_version(target)
            or definition.get("runtimeCandidate") != selected_runtime_marker(target)):
        raise DraftConflict("The retained product selection differs from its exact accepted package.")
    from .runtime_selection import selected_runtime_platform
    from .runtime_dependencies import platform_target
    if selected_runtime_platform(target) != platform_target():
        raise DraftConflict("The selected native product differs from this exact local target.")
    artifacts = selected_runtime_artifacts(target, verified_artifacts=verified_artifacts,
        source_custody=source_custody, files=files)
    candidate_selection(target, deployment_target=definition["target"], verified_artifacts=artifacts,
        source_custody=source_custody, files=files)
    return artifacts


async def await_validation(function, *args, **kwargs):
    """Keep bounded file work off the loop and drain its owned thread."""
    pending = asyncio.create_task(asyncio.to_thread(function, *args, **kwargs))
    try:
        return await asyncio.shield(pending)
    except asyncio.CancelledError:
        # Cancellation cannot stop a thread. Retain this one task until its
        # read ends, including repeated cancellation, then preserve cancellation.
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


def _require_status_package_current(directory, observation):
    from .connect_application_update_startup import _file_identity
    from .connect_installer_caller import CustodyError, _private_directory
    try:
        if (_private_directory(directory) != observation[0]
                or _file_identity(Path(directory) / "package.zip") != observation[1]):
            raise DraftConflict("The installed package changed during status inspection.")
    except (OSError, CustodyError) as error:
        raise DraftConflict("The installed package changed during status inspection.") from error


def _observe_status_package(function, directory, *args):
    from .connect_application_update_startup import _file_identity
    from .connect_installer_caller import CustodyError, _private_directory
    try:
        observation = (_private_directory(directory), _file_identity(Path(directory) / "package.zip"))
    except (OSError, CustodyError) as error:
        raise DraftConflict("The installed package could not be inspected.") from error
    result = function(directory, *args)
    _require_status_package_current(directory, observation)
    return result, observation


def now():
    return datetime.now(timezone.utc).isoformat()


def new_installation_receipt(identity, guide_id, source, version, build_id, digest, target,
        execution_mode, files, source_kind, *, runtime_selection=None):
    """Publish the existing installation shape from its already admitted source."""
    receipt = {"deployment_id": identity, "guide_id": guide_id, "guide_version": source["guide_version"],
        "brief_hash": source["brief_hash"], "admitted_guide_version": version,
               "build_id": build_id, "blueprint_hash": source["blueprint_hash"], "package_sha256": digest, **native_fields(source),
               "destination": str(target), "target": "local", "execution_mode": execution_mode,
               "status": "queued", "phase": "install",
               "detail": "Preparing your owned deployment folder.", "created_at": now(), "updated_at": now(),
               "checks": [], "progress": [], "logs": [], "console_url": None, "bootstrap": None,
               "runtime_pid": None, "unresolved": json.loads(files["implementation-report.json"][0]).get("unresolved", []) if source_kind == "application" else []}
    if "organization_hash" in source:
        receipt["organization_hash"] = source["organization_hash"]
    if runtime_selection is not None:
        receipt["runtime_candidate"] = deepcopy(runtime_selection)
    return receipt


def read_receipt(root):
    return json.loads((Path(root) / "receipt.json").read_text())


def worker_stop_timeout(root):
    """Wait for an owned worker's stop around the horizon its application reported.

    The worker records that horizon at readiness and clears it for each new
    child. An unreadable or invalid record is waited as unreported: the stop
    itself must not fail on it.
    """
    try:
        horizon_ms = reported_gateway_horizon(read_receipt(root))
    except (OSError, ValueError, AttributeError):
        horizon_ms = None
    return WORKER_STOP_TIMEOUT if horizon_ms is None else shutdown_budget(horizon_ms)["worker"]


@contextmanager
def receipt_lock(root):
    descriptor = os.open(Path(root) / "journal.lock", os.O_CREAT | os.O_RDWR, 0o600)
    try:
        fcntl.flock(descriptor, fcntl.LOCK_EX)
        yield
    finally:
        os.close(descriptor)


def update_receipt(root, **changes):
    with receipt_lock(root):
        value = read_receipt(root)
        value.update(changes, updated_at=now())
        write_private(Path(root) / "receipt.json", value)
        return value


def journal(root, message, **changes):
    with receipt_lock(root):
        value = read_receipt(root)
        entries = [*value.get("progress", []), {"at": now(), "message": message}][-100:]
        value.update(changes, updated_at=now(), progress=entries, detail=message)
        write_private(Path(root) / "receipt.json", value)
        return value


def refusal_detail(error):
    """The installer's own typed refusal as one bounded sentence, else its type name.

    CustodyError and DraftConflict carry authored, user-facing text. Any other
    error may carry private values, so only its type name is shown. The result
    follows a clause: ": <text>" or " (<TypeName>)."
    """
    from .connect_installer_caller import CustodyError
    text = " ".join(str(error).split())[:2000] if isinstance(error, (CustodyError, DraftConflict)) else ""
    if not text:
        return f" ({type(error).__name__})."
    return ": " + text + ("" if text.endswith((".", "!", "?")) else ".")


def _log_install_completion(directory_name):
    def completed(task):
        # Retrieve the task's exception without changing its result or custody.
        if task.cancelled() or task.exception() is None:
            return
        LOG.warning("Meta Forge: the Install completion of deployment %s failed%s",
            directory_name, refusal_detail(task.exception()))
    return completed


def interactive_cleanup_matches(receipt, *, require_worker_exit=True):
    """A failed receipt or an available project lease is not teardown evidence."""
    cleanup = receipt.get("interactive_cleanup")
    identity = receipt.get("interactive_lifecycle_id")
    try:
        valid_id = isinstance(identity, str) and str(uuid.UUID(identity)) == identity
    except (ValueError, AttributeError):
        valid_id = False
    return bool(valid_id and isinstance(cleanup, dict) and cleanup.get("confirmed") is True
        and (not require_worker_exit or cleanup.get("worker_exit_confirmed") is True
             or cleanup.get("worker_exit_confirmed") is False
             and cleanup.get("supervisor_absence_confirmed") is True
             and cleanup.get("supervisor_absence_scope") == "cold-host-owned-native-teardown"
             and cleanup.get("boundary") == "owned-process-groups-absent")
        and cleanup.get("lifecycle_id") == identity
        and all(cleanup.get(key) == receipt.get(key) for key in
                ("deployment_id", "guide_id", "build_id", "guide_version", "blueprint_hash",
                 "destination", "package_sha256"))
        and all(cleanup.get(key) == value for key, value in native_fields(receipt).items())
        and "interactive_cleanup_attempt_id" in receipt
        and cleanup.get("startup_attempt_id") == receipt["interactive_cleanup_attempt_id"]
        and (receipt.get("startup_attempt_id") is None
             or receipt["startup_attempt_id"] == receipt["interactive_cleanup_attempt_id"])
        and all(isinstance(cleanup.get(key), str) and re.fullmatch(r"[a-f0-9]{64}", cleanup[key])
                for key in ("source_digest", "owner_sha256"))
        and type(cleanup.get("supervisor_pid")) is int and cleanup["supervisor_pid"] > 0
        and cleanup.get("boundary") in {"owned-process-groups-absent", "worker-not-admitted"}
        and (cleanup.get("boundary") != "worker-not-admitted" or receipt.get("startup_attempt_id") is None)
        and type(cleanup.get("return_code")) is int and cleanup["return_code"] in (0, 1))


def interactive_source_binding(directory, receipt, *, allow_missing=False):
    from .deployment_updates import checked_package
    _, files = checked_package(Path(directory), "package.zip", receipt["package_sha256"])
    source = read_package_source(files, receipt)
    digest = source["source_digest"]
    destination = verify_destination(receipt, files, allow_missing=allow_missing)
    owner = destination / OWNER_FILE
    info = owner.lstat()
    if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_nlink != 1:
        raise ValueError("The cleaned installation's owner could not be verified.")
    return {"source_digest": digest, "owner_sha256": hashlib.sha256(owner.read_bytes()).hexdigest()}


async def require_owned_group_absent(process, *, timeout=1):
    """Observe the group created for this actual child handle, without signaling it."""
    deadline = asyncio.get_running_loop().time() + timeout
    while True:
        if process.returncode is None:
            raise ValueError("The owned application parent has not confirmed exit.")
        try:
            os.killpg(process.pid, 0)
        except ProcessLookupError:
            return
        except PermissionError:
            raise ValueError("The owned application group cleanup could not be observed.") from None
        if asyncio.get_running_loop().time() >= deadline:
            raise ValueError("The owned application group has not confirmed exit.")
        await asyncio.sleep(0.05)


async def bootstrap_interactive_worker(directory, lifecycle_id, selection_path, selection_sha256, *,
        owner_source_sha256=None):
    """Keep bootstrap pins distinct from the positional update protocol."""
    return await interactive_worker(directory, "start", lifecycle_id,
        bootstrap_selection={"path": selection_path, "sha256": selection_sha256},
        **({"bootstrap_owner_source_sha256": owner_source_sha256} if owner_source_sha256 is not None else {}))


async def interactive_worker(directory, mode, lifecycle_id, selection_path=None, selection_sha256=None,
                             *, bootstrap_selection=None, bootstrap_owner_source_sha256=None):
    """Confirm only a normal return through the fixed worker's teardown boundary."""
    from . import deployment_worker as worker
    from .deployment_updates import read_update_admission, transaction, require_recovery_source
    update_selection = ({"path": selection_path, "sha256": selection_sha256}
                        if selection_path is not None or selection_sha256 is not None else None)
    bootstrap_selection = deepcopy(bootstrap_selection)
    if bootstrap_selection is not None:
        if mode != "start" or update_selection is not None:
            raise ValueError("Bootstrap requires its distinct original interactive Start admission.")
        from .deployment_application_bootstrap import read_bootstrap_admission
        read_bootstrap_admission(directory, bootstrap_selection)
    expected = read_update_admission(directory, mode, update_selection) if update_selection is not None else None
    if mode in {"update", "rollback", "recover-current"} and expected is None:
        raise ValueError("The owned interactive update has no original admission.")
    receipt = read_receipt(directory)
    if (mode not in {"install", "start", "update", "rollback", "recover-current"}
            or receipt.get("execution_mode", "interactive") != "interactive"
            or receipt.get("service") or receipt.get("interactive_lifecycle_id") != lifecycle_id):
        raise ValueError("The owned interactive worker admission changed.")
    binding_keys = ("deployment_id", "guide_id", "build_id", "guide_version",
        "blueprint_hash", "destination", "package_sha256")
    transaction_admission = None
    transaction_update = None
    transaction_sources = None
    if expected is not None:
        transaction_admission = deepcopy(receipt.get("update_action"))
        if (mode in {"update", "rollback", "recover-current"} and (
                not isinstance(transaction_admission, dict)
                or transaction_admission.get("action") != ("apply-update" if mode == "update" else mode)
                or transaction_admission.get("update_id") != expected["binding"]["update_id"])):
            raise ValueError("The owned interactive update admission changed.")
        receipt, _, update = transaction(directory, expected["binding"]["update_id"],
                                         expected_binding=expected["binding"], expected_installation=expected["installation"])
        require_recovery_source(directory, expected)
        transaction_update = deepcopy(update)
        transaction_sources = {update["previous_package_sha256"]: update["from_build_id"],
                               update["package_sha256"]: update["to_build_id"]}
        if transaction_sources.get(receipt.get("package_sha256")) != receipt.get("build_id"):
            raise ValueError("The owned interactive update source changed.")
        # The same transaction may leave either checked source installed. Its
        # deployment and destination remain fixed while actual child handles
        # supply teardown evidence for every attempted source.
        binding_keys = ("deployment_id", "guide_id", "destination")
    binding = {key: receipt.get(key) for key in binding_keys}
    binding.update(native_fields(receipt))
    # This runs in a dedicated fixed host process. Capture actual child handles
    # at the existing teardown seam; do not reconstruct ownership from receipts.
    original = worker.Supervisor
    supervisors = []

    class InteractiveSupervisor(original):
        def __init__(self, root):
            super().__init__(root)
            self.cleanup_observed = False
            self.cleanup_groups = {}
            supervisors.append(self)

        async def terminate(self, *args, **kwargs):
            process = self.child
            self.cleanup_observed = False
            if process is not None:
                self.cleanup_groups[id(process)] = (process, False)
            await super().terminate(*args, **kwargs)
            if process is not None:
                await require_owned_group_absent(process)
                self.cleanup_groups[id(process)] = (process, True)
            self.cleanup_observed = True

    worker.Supervisor = InteractiveSupervisor
    try:
        if bootstrap_selection is not None:
            code = await worker.run(Path(directory), mode, bootstrap_selection=bootstrap_selection,
                bootstrap_lifecycle_id=lifecycle_id,
                **({"bootstrap_owner_source_sha256": bootstrap_owner_source_sha256}
                    if bootstrap_owner_source_sha256 is not None else {}))
        else:
            code = (await worker.run(Path(directory), mode, update_selection=update_selection)
                    if update_selection is not None else await worker.run(Path(directory), mode))
    finally:
        worker.Supervisor = original
    if (asyncio.current_task().cancelling() or not supervisors
            or any(not item.cleanup_observed or any(not confirmed for _, confirmed in item.cleanup_groups.values())
                   for item in supervisors)):
        raise ValueError("The interactive worker did not confirm uncancelled cleanup.")
    if code is not None and (type(code) is not int or code not in (0, 1)):
        raise ValueError("The interactive worker returned an unknown result.")
    if transaction_sources is not None:
        latest, _, update = transaction(directory, expected["binding"]["update_id"],
            cleanup_update=transaction_update, expected_binding=expected["binding"], expected_installation=expected["installation"])
        if (latest.get("update_action") != transaction_admission
                or {update["previous_package_sha256"]: update["from_build_id"],
                    update["package_sha256"]: update["to_build_id"]} != transaction_sources
                or transaction_sources.get(latest.get("package_sha256")) != latest.get("build_id")
                or any(latest.get(key) != value for key, value in binding.items())):
            raise ValueError("The owned interactive update changed during cleanup.")
        binding = {key: latest.get(key) for key in ("deployment_id", "guide_id", "build_id", "guide_version",
            "blueprint_hash", "destination", "package_sha256")}
        binding.update(native_fields(latest))
    with receipt_lock(directory):
        latest = read_receipt(directory)
        if (latest.get("interactive_lifecycle_id") != lifecycle_id
                or transaction_admission is not None and latest.get("update_action") != transaction_admission
                or any(latest.get(key) != value for key, value in binding.items())):
            raise ValueError("The interactive worker ownership changed during cleanup.")
        source_binding = interactive_source_binding(directory, latest)
        latest.update(interactive_cleanup={**binding, "lifecycle_id": lifecycle_id, "confirmed": True,
            **source_binding, "boundary": "owned-process-groups-absent",
            "startup_attempt_id": latest.get("startup_attempt_id"), "supervisor_pid": os.getpid(),
            "return_code": code or 0, "completed_at": now(), "worker_exit_confirmed": False},
            interactive_cleanup_attempt_id=latest.get("startup_attempt_id"), updated_at=now())
        write_private(Path(directory) / "receipt.json", latest)
    return code


def _local_host_target():
    return {"darwin": "mac", "linux": "linux"}.get(sys.platform)


def local_target(blueprint):
    expected = _local_host_target()
    if not expected or blueprint.get("target") != expected:
        raise ValueError("This proposal targets another host. Ask the guide to choose this computer's operating system before local installation; GCP remains a separate deployment adapter.")


def require_accepted(status):
    if (status.get("acceptance_current") is not True or status.get("acceptance_trusted") is not True
            or (status.get("acceptance") or {}).get("status") != "pass"):
        raise ValueError("Run independent acceptance for the current checked source before installing. An older source-only package is not accepted deployment evidence.")


def configuration_fields(definition, application_fields=None):
    fields = [dict(field) for field in definition["environment"]]
    knowledge = definition["knowledge"]
    if knowledge["mode"] == "local":
        generated = {knowledge["token_env"], knowledge["space_env"]}
        fields = [{**field, "required": False, "automatic": True} if field["key"] in generated else field
                  for field in fields]
        if not any(field["key"] == "OPENAI_API_KEY" for field in fields):
            fields.append({"key": "OPENAI_API_KEY", "label": "OpenAI API key for knowledge embeddings",
                           "secret": True, "required": True})
    if application_fields is not None:
        value = application_fields if isinstance(application_fields, dict) else {"fields": application_fields}
        fields.extend(validate_application_fields(value, {field["key"] for field in fields} | PROVIDER_KEYS))
    return fields


def package_configuration_fields(definition, files):
    """Read declarative private fields only from the immutable checked package."""
    entry = files.get("application/configuration.json")
    if entry is None:
        return configuration_fields(definition)
    if len(entry[0]) > 256_000:
        raise ValueError("Application configuration must be a bounded ordinary JSON file.")
    try:
        value = json.loads(entry[0].decode("utf-8"))
    except (ValueError, UnicodeError):
        raise ValueError("Application configuration must contain valid UTF-8 JSON.") from None
    return configuration_fields(definition, value)


def deployment_handoff(definition, files, proposal, guide_id, build_id):
    """Describe only commands supported by this immutable package, without running them."""
    target = definition.get("target")
    if target not in {"mac", "linux", "gcp"} or target != proposal.get("target"):
        raise DraftConflict("The package destination differs from this proposal. Rebuild before installing.")
    cloud = proposal.get("gcp") or {}
    existing_host = target == "gcp" and cloud.get("provisioning") == "existing_host"
    host_target = _local_host_target()
    local = target == host_target
    handoff = {"mode": "local" if local else "remote-package", "status": "not-connected",
               "package_url": f"/api/guide/{quote(guide_id, safe='')}/implementation/package?{urlencode({'build_id': build_id})}",
               "steps": [], "operations": [], "limitations": []}
    if local:
        return target, host_target, handoff
    handoff["limitations"] = [
        "Forge has not connected to or verified a remote deployment. Downloading this accepted source is not a running-system check.",
        "Enter private credentials on the destination using setup.py. This page does not send credentials to another computer.",
        "Custom UI and external chat channel health require their own live checks; a native Console link does not verify them.",
    ]
    required = {"setup.py", "start.sh", "install.sh"}
    if target == "gcp":
        required |= {"deploy/deploy-gcp.sh", "deploy/install-service.sh", "deploy/tunnel-gcp.sh", "remote_operations.py"}
    missing = sorted(required - files.keys())
    if missing:
        handoff["limitations"].append("This accepted package lacks the required handoff scripts: " + ", ".join(missing) + ". Continue implementation before deployment.")
        return target, host_target, handoff
    steps = handoff["steps"]
    steps.append({"title": "Download and extract the accepted package" if target == "gcp" else "Download the accepted package",
                  "where": "this computer",
                  "description": "Use the download button above. " + ("Extract into a new empty folder that you own on this computer. Open the project folder containing install.sh and start.sh; this may be an inner folder created by the ZIP. Open a terminal there for the provisioning and tunnel commands." if target == "gcp" and not existing_host else "Keep this exact ZIP for transfer to the destination; Forge does not upload it for you."), "commands": []})
    if target != "gcp" or existing_host:
        steps.append({"title": "Transfer the ZIP and extract it on the destination", "where": "destination computer",
                      "description": "Transfer the downloaded ZIP to your destination using your usual file transfer, then extract it into a new empty folder you own there. If this is the destination computer, extract here. Keep the source unchanged. Open the project folder containing install.sh and start.sh; this may be an inner folder created by the ZIP. Open a terminal there for the destination commands below.", "commands": []})
    if existing_host:
        from .blueprint import CloudOptions
        selected = CloudOptions.model_validate(cloud)
        if (not selected.existing_instance or not selected.existing_zone_confirmed
                or not re.fullmatch(r"[a-z][a-z0-9-]{4,28}[a-z0-9]", selected.project)):
            raise DraftConflict("The existing-host package needs its actual project, instance name and confirmed zone.")
        steps.append({"title": "Use the existing VM", "where": "existing destination",
            "description": f"Destination: {selected.project}, {selected.zone}, {selected.existing_instance}. Use its authorized SSH access and prepare Python 3.11+ and selected connector prerequisites there. The application folder is {definition.get('slug', '')}; it is not the VM name. No new VM, firewall rule or host bootstrap is offered. Preserve any existing installation and use its saved update controls.",
            "commands": []})
    if target == "gcp" and not existing_host:
        cloud = proposal.get("gcp") or {}
        project, zone, slug = cloud.get("project", ""), cloud.get("zone", ""), definition.get("slug", "")
        if (not re.fullmatch(r"[a-z][a-z0-9-]{4,28}[a-z0-9]", project)
                or not re.fullmatch(r"[a-z]+-[a-z]+[0-9]+-[a-z]", zone)
                or not re.fullmatch(r"[a-z][a-z0-9-]{1,39}", slug)):
            handoff["limitations"].append("The checked proposal needs a valid Google Cloud project, zone and project folder before exact cloud instructions are available.")
            handoff["steps"] = []
            return target, host_target, handoff
        steps += [
            {"title": "Prepare Google Cloud access", "where": "your computer",
             "description": f"Install the Google Cloud CLI and sign in to an account authorized for project {project}. The project needs billing and Compute Engine/IAP access. This page does not create cloud resources or sign you in.", "commands": ["gcloud auth login"]},
            {"title": "Review, then create the VM", "where": "your computer",
             "description": f"The script targets {project}, {zone}, VM {slug}. The second command creates billed resources after its project confirmation. It keeps an ownership receipt for recovery; private credentials are excluded from upload.",
             "commands": ["bash deploy/deploy-gcp.sh --dry-run", "bash deploy/deploy-gcp.sh"]},
            {"title": "Configure private credentials on the VM", "where": "SSH session on the VM",
             "description": "Start this SSH command on your computer, then run the folder and setup commands inside that session. Install the selected connector prerequisites on the VM. The private setup interview asks for provider, connector and application fields without editing files.",
             "commands": [f"gcloud compute ssh {slug} --project {project} --zone {zone} --tunnel-through-iap", f"cd ~/src/{slug}", ".venv/bin/python setup.py"]},
        ]
    else:
        steps.append({"title": "Install and enter private configuration", "where": "destination computer",
                      "description": "Install Python 3.11 or newer and the selected connector prerequisites there, then run this command. The setup interview collects private fields without editing files.", "commands": ["bash install.sh"]})
    knowledge = definition.get("knowledge") or {}
    if knowledge.get("mode") != "none" and "knowledge/manage.py" in files:
        commands = ([".venv/bin/python knowledge/manage.py up"] if knowledge.get("mode") == "local" else [])
        steps.append({"title": "Check the selected knowledge service", "where": "VM SSH session" if target == "gcp" else "destination computer",
                      "description": "A local knowledge service requires Docker with Compose and Git on the destination. The verification command checks actual authenticated service access.",
                      "commands": [*commands, ".venv/bin/python knowledge/manage.py verify"]})
    checks = ["bash start.sh --doctor", "bash start.sh --application-check"]
    if definition.get("mcpConnections"):
        checks.append("bash start.sh --connections-check")
    service = target == "gcp" or (target == "linux" and {"deploy/install-service.sh", "remote_operations.py"}.issubset(files))
    start_command = "bash deploy/install-service.sh" if target == "gcp" else "bash deploy/install-service.sh start" if service else "bash start.sh"
    start_description = "Resolve failed checks before starting. Starting runs the accepted application's configured startup work. These commands do not prove custom interfaces or live external workflows are working. "
    if service:
        start_description += "The owned background service requires your Linux user systemd manager and waits for native bootstrap and Console health. Running in the background does not guarantee operation after logout or reboot."
    else:
        start_description += "Keep this terminal open while the application runs. Stop it with Control-C."
    steps.append({"title": "Verify, then start the background service" if service else "Verify, then run in this terminal",
                  "where": "VM SSH session" if target == "gcp" else "destination computer",
                  "description": start_description, "commands": [*checks, start_command]})
    port = definition.get("port")
    if type(port) is not int or not 1024 <= port <= 65535:
        raise ValueError("The accepted package has an invalid native Console port.")
    if target != "gcp":
        steps.append({"title": "If the destination is another computer, open a private SSH tunnel",
                      "where": "your computer with the browser, not the destination SSH session",
                      "description": "Skip this step when using a browser on the destination itself. Otherwise replace USER@HOST with your SSH user and destination address. Keep this separate tunnel terminal open; stop the tunnel with Control-C. Do not expose the Console publicly.",
                      "commands": [f"ssh -N -L {port}:127.0.0.1:{port} USER@HOST"]})
    steps.append({"title": "Open the native MobKit Console", "where": "your computer with the browser",
                  "description": f"{'Keep the IAP tunnel open, then visit' if target == 'gcp' else 'Use a browser on the destination, or keep the SSH tunnel above open on your computer, then visit'} http://127.0.0.1:{port}/console. This is the native agent chat and admin Console. Its remote health is not tracked by Forge.",
                  "commands": ["bash deploy/tunnel-gcp.sh"] if target == "gcp" else []})
    if existing_host:
        handoff["limitations"].append("The optional IAP tunnel uses the selected existing instance and requires already authorized SSH access. Forge has not verified that access or this host's capacity. Its new-VM provisioner is unavailable for this proposal.")
    if service:
        handoff["operations"] = [
            {"title": "Check service status", "commands": ["bash deploy/install-service.sh status"]},
            {"title": "Stop the application", "commands": ["bash deploy/install-service.sh stop"]},
            {"title": "Restart the same installed application", "commands": ["bash deploy/install-service.sh restart"]},
            {"title": "Remove only the owned service, preserving source, private settings and data", "commands": ["bash deploy/install-service.sh remove-service"]},
            {"title": "Choose logout behavior or change private setup", "commands": ["bash deploy/install-service.sh stop", "bash deploy/install-service.sh setup"]},
            {"title": "Use a terminal instead of the background service", "description": "Choose this alternative when a user systemd manager is unavailable. Stop the background service first if it is running. Keep this terminal open and stop the application with Control-C; do not run both copies.", "commands": ["bash start.sh"]},
        ]
        handoff["limitations"].append("The background service belongs to the destination's current Linux user. The guided service setup asks before enabling operation after logout; that system setting can require sudo. A user manager without that setting normally stops its services on logout. No unattended boot or remote health is established by these instructions.")
    if target == "gcp" and not existing_host:
        handoff["limitations"].append("If VM installation stops, keep this exact package folder and use bash deploy/deploy-gcp.sh --resume. Cleanup commands and ownership evidence are in the package README and script output.")
    return target, host_target, handoff


def package_console_scope(files, fields, state):
    """Describe declared Console authentication and the exact scope of host health evidence."""
    authentication = "loopback-only"
    entry = files.get("application/console.json")
    if entry is not None:
        try:
            if len(entry[0]) > 4096 or json.loads(entry[0].decode("utf-8")) != {"authentication": "local-password"}:
                raise ValueError
        except (ValueError, UnicodeError):
            raise ValueError("The accepted package has invalid Console authentication metadata.") from None
        declared = {field["key"]: field for field in fields}
        if any(key not in declared or declared[key].get("required") is not True
               or declared[key].get("secret") is not secret or "default" in declared[key]
               for key, secret in [("CONSOLE_USER_ID", False), ("CONSOLE_LOGIN_PASSWORD", True)]):
            raise ValueError("Authenticated Console metadata needs its required private setup fields.")
        authentication = "local-password"
    interfaces = (state.get("brief") or {}).get("facts", {}).get("interfaces") or {}
    requested = interfaces.get("summary", "") if interfaces.get("source_message_ids") else ""
    return {"authentication": authentication, "health_scope": "native-bootstrap-and-console",
            "requested_interfaces": requested, "custom_interfaces_verified": False}


def parse_setting(value):
    if value.startswith('"'):
        return json.loads(value)
    if len(value) >= 2 and value[0] == value[-1] == "'":
        return value[1:-1]
    return value


def package_files(payload):
    """Reject traversal, links, duplicates, private state, and oversized ZIP data."""
    if len(payload) > MAX_PACKAGE_BYTES:
        raise ValueError("The checked application package is too large.")
    files, original_modes, original_types = {}, {}, {}
    size = 0
    with zipfile.ZipFile(io.BytesIO(payload)) as archive:
        for item in archive.infolist():
            name = item.filename
            parts = name.split("/")
            mode = item.external_attr >> 16
            if (not name or PurePosixPath(name).is_absolute() or "\\" in name
                    or any(part in {"", ".", ".."} for part in parts)
                    or any(ord(char) < 32 for char in name) or name in files
                    or stat.S_ISLNK(mode) or item.is_dir()
                    or parts[0] in {".venv", ".bin", ".mobkit", "data"}
                    or parts[-1] == ".env"):
                raise ValueError("The checked package contains an unsafe deployment path.")
            size += item.file_size
            if size > MAX_PACKAGE_BYTES or len(files) >= 2048:
                raise ValueError("The checked application package exceeded its file limit.")
            files[name] = (archive.read(item), 0o755 if mode & 0o111 else 0o644)
            original_modes[name] = stat.S_IMODE(mode)
            original_types[name] = stat.S_IFMT(mode)
    if NATIVE_SOURCE in files:
        if (any(mode not in {0o600, 0o644, 0o755} for mode in original_modes.values())
                or any(kind != stat.S_IFREG for kind in original_types.values())):
            raise ValueError("The compiled agent-team package has unsupported source modes.")
        files = {name: (content, original_modes[name]) for name, (content, _) in files.items()}
        read_package_source(files)
    if (not {"deployment.json", "app.py", "install.sh"}.issubset(files)
            or NATIVE_SOURCE not in files and "implementation-report.json" not in files):
        raise ValueError("Use a checked application package containing the native host.")
    return files


def owned_destination(value):
    if not isinstance(value, str) or not value.strip() or len(value) > 2000 or any(ord(c) < 32 for c in value):
        raise ValueError("Choose a destination folder on this computer.")
    path = Path(value.strip()).expanduser()
    if not path.is_absolute():
        raise ValueError("Use an absolute destination, such as ~/src/my-team.")
    path = Path(os.path.abspath(path))
    if path in {Path("/"), Path.home()}:
        raise ValueError("Choose a project folder inside a directory you own.")
    for part in [*reversed(path.parents), path]:
        if part.is_symlink():
            raise ValueError("The destination and its parent folders cannot be symbolic links.")
        if part.exists() and not part.is_dir():
            raise ValueError("The destination must be a folder.")
    existing = next(part for part in [path, *path.parents] if part.exists())
    if existing.stat().st_uid != os.getuid():
        raise ValueError("Choose a destination inside a folder owned by your user.")
    return path


def ownership(receipt):
    return {**{key: receipt[key] for key in ("deployment_id", "guide_id", "build_id", "package_sha256", "destination")},
            **native_fields(receipt)}


def verify_destination(receipt, files, *, allow_missing=False):
    destination = owned_destination(receipt["destination"])
    marker = destination / OWNER_FILE
    if marker.is_symlink() or marker.parent.is_symlink():
        raise DraftConflict("The deployment ownership marker was replaced. No files were changed.")
    try:
        actual = json.loads(marker.read_text())
    except (OSError, ValueError):
        raise DraftConflict("This folder has no matching Forge ownership receipt. No files were changed.") from None
    if actual != ownership(receipt):
        raise DraftConflict("This folder belongs to another deployment. Choose a new folder.")
    for name, (content, mode) in files.items():
        target = destination / name
        if any(part.is_symlink() for part in [target, *target.parents] if part != destination.parent):
            raise DraftConflict("A deployed source path was replaced by a link. No files were changed.")
        if not target.exists() and allow_missing:
            continue
        if not target.is_file() or hashlib.sha256(target.read_bytes()).digest() != hashlib.sha256(content).digest():
            raise DraftConflict("Installed source differs from the checked package. Preserve this folder and choose a new destination.")
        if receipt.get("source_kind") == "native-team" and stat.S_IMODE(target.stat().st_mode) != mode:
            raise DraftConflict("Installed agent-team source permissions differ from its compiled package. Preserve this folder.")
    return destination


class _DestinationSourceObservation:
    """Process-local file observation for one already materialized source.

    Full source bytes are checked before these metadata fences are captured.
    Any change refuses the original operation; this observation never selects
    replacement files, survives a process restart or admits an installation.
    The caller retains its existing lease and performs a full boundary check.
    """

    def __init__(self, receipt, files):
        from .connect_application_update_startup import _file_identity
        if (type(files) is not dict or any(type(name) is not str
                or type(item) is not tuple or len(item) != 2
                or type(item[0]) is not bytes or type(item[1]) is not int
                or not 0 <= item[1] <= 0o7777 for name, item in files.items())):
            raise DraftConflict("Installation source observation needs immutable checked bytes and modes.")
        self._destination = owned_destination(receipt["destination"])
        self._owner = ownership(receipt)
        self._files = dict(files)
        paths = [self._destination / OWNER_FILE,
                 *(self._destination / name for name in self._files)]
        self._identities = {path: _file_identity(path) for path in paths}
        verify_destination(receipt, files)
        self.require_current(receipt, files)

    def require_current(self, receipt, files):
        from .connect_application_update_startup import _file_identity
        if (ownership(receipt) != self._owner
                or owned_destination(receipt["destination"]) != self._destination
                or type(files) is not dict or set(files) != set(self._files)
                or any(type(files[name]) is not tuple or len(files[name]) != 2
                    or files[name][0] is not content or type(files[name][1]) is not int
                    or files[name][1] != mode
                    for name, (content, mode) in self._files.items())):
            raise DraftConflict("The original installation source selection changed.")
        try:
            if any(_file_identity(path) != identity
                    for path, identity in self._identities.items()):
                raise DraftConflict("Installed source changed during its original installation.")
        except (OSError, ValueError) as error:
            raise DraftConflict("Installed source changed during its original installation.") from error


def _rename_new_source(stage, parent, name):
    """Move a completed stage atomically, with no replacing rename fallback."""
    library = ctypes.CDLL(None, use_errno=True)
    if sys.platform == "linux":
        function, flags = getattr(library, "renameat2", None), 1  # RENAME_NOREPLACE
    elif sys.platform == "darwin":
        function, flags = getattr(library, "renameatx_np", None), 0x4  # RENAME_EXCL
    else:
        function, flags = None, 0
    if function is None:
        raise OSError("This computer cannot publish source atomically without replacing existing files.")
    function.argtypes = (ctypes.c_int, ctypes.c_char_p, ctypes.c_int, ctypes.c_char_p, ctypes.c_uint)
    function.restype = ctypes.c_int
    if function(stage, b"source", parent, os.fsencode(name), flags) != 0:
        code = ctypes.get_errno()
        raise OSError(code, os.strerror(code), name)


def _publish_new_source(target, content, mode, *, exact_mode, require_current):
    """Publish complete bytes exclusively; abandoned private stages are not reused."""
    parent = os.open(target.parent, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    stage_name = ".forge-source-" + uuid.uuid4().hex
    stage = None
    stage_identity = source_identity = None
    try:
        os.mkdir(stage_name, 0o700, dir_fd=parent)
        stage = os.open(stage_name, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=parent)
        os.fchmod(stage, 0o700)
        stage_info = os.fstat(stage)
        stage_identity = (stage_info.st_dev, stage_info.st_ino)
        descriptor = os.open("source", os.O_RDWR | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, mode, dir_fd=stage)
        source_info = os.fstat(descriptor)
        source_identity = (source_info.st_dev, source_info.st_ino)
        with os.fdopen(descriptor, "w+b") as stream:
            if exact_mode:
                # A private directory keeps these exact compiler modes private
                # until the complete file is published outside it.
                os.fchmod(stream.fileno(), mode)
            if stream.write(content) != len(content):
                raise OSError("The generated source could not be written completely.")
            stream.flush()
            os.fsync(stream.fileno())
            stream.seek(0)
            if stream.read(len(content) + 1) != content:
                raise OSError("The staged source differs from the checked package.")
            require_current()
            current_parent = target.parent.lstat()
            original_parent = os.fstat(parent)
            staged = os.stat("source", dir_fd=stage, follow_symlinks=False)
            if ((current_parent.st_dev, current_parent.st_ino) != (original_parent.st_dev, original_parent.st_ino)
                    or not stat.S_ISREG(staged.st_mode) or staged.st_nlink != 1
                    or (staged.st_dev, staged.st_ino) != source_identity):
                raise DraftConflict("The source publication path changed. No existing source was replaced.")
            _rename_new_source(stage, parent, target.name)
    finally:
        # Only this attempt's original entry can be removed. Never scan or
        # reclaim another attempt's residue, even after an interrupted publish.
        if stage is not None:
            try:
                info = os.stat("source", dir_fd=stage, follow_symlinks=False)
                if source_identity == (info.st_dev, info.st_ino):
                    os.unlink("source", dir_fd=stage)
            except OSError:
                pass
            finally:
                os.close(stage)
        try:
            info = os.stat(stage_name, dir_fd=parent, follow_symlinks=False)
            if stage_identity == (info.st_dev, info.st_ino):
                os.rmdir(stage_name, dir_fd=parent)
        except OSError:
            pass
        finally:
            os.close(parent)


def materialize(root, *, require_current=None):
    receipt = read_receipt(root)
    payload = (Path(root) / "package.zip").read_bytes()
    if hashlib.sha256(payload).hexdigest() != receipt["package_sha256"]:
        raise DraftConflict("The saved checked package changed. No deployment files were changed.")
    files = package_files(payload)
    destination = verify_destination(receipt, files, allow_missing=True)
    for name, (content, mode) in files.items():
        if require_current is not None:
            require_current()
        target = destination / name
        if target.exists():
            continue
        target.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        _publish_new_source(target, content, mode, exact_mode=receipt.get("source_kind") == "native-team",
            require_current=lambda: (require_current() if require_current is not None else None,
                verify_destination(receipt, {name: (content, mode)}, allow_missing=True)))
    verify_destination(receipt, files)
    return files


def prepare_private_settings(destination, fields, submitted, use_host, *, port=None, remove_fields=(), existing_values=None,
                             console_authentication=None):
    """Validate private input without claiming a destination or writing credentials."""
    if not isinstance(submitted, dict) or len(submitted) > 100:
        raise ValueError("Provide configuration values in the private setup form.")
    allowed = {field["key"]: field for field in fields}
    allowed["MOBKIT_PORT"] = {"secret": False, "required": False}
    if set(submitted) - set(allowed):
        raise ValueError("The setup form contains a setting not requested by this package.")
    if (not isinstance(remove_fields, (list, tuple)) or len(remove_fields) > 100
            or any(not isinstance(key, str) for key in remove_fields)
            or len(set(remove_fields)) != len(remove_fields)):
        raise ValueError("Choose each optional setting to remove once.")
    removed = set(remove_fields)
    if any(key not in allowed or key == "MOBKIT_PORT" or allowed[key].get("required")
           or allowed[key].get("automatic") for key in removed):
        raise ValueError("Only optional settings requested by this package can be removed.")
    if any(submitted.get(key) for key in removed):
        raise ValueError("Choose either a replacement or removal for an optional setting.")
    path = destination / ".env"
    if path.is_symlink():
        raise DraftConflict("The private configuration path is a link. No credentials were saved.")
    new_configuration = existing_values is None and not path.exists()
    values = dict(existing_values) if existing_values is not None else {}
    if existing_values is None and path.exists():
        for line in path.read_text().splitlines():
            if not line or line.startswith("#"):
                continue
            key, separator, value = line.partition("=")
            if not separator or not re.fullmatch(r"[A-Z][A-Z0-9_]*", key):
                raise ValueError("Existing private configuration is invalid. Preserve it and inspect the destination.")
            values[key] = parse_setting(value)
    for field in fields:
        key = field["key"]
        if not re.fullmatch(r"[A-Z][A-Z0-9_]*", key):
            raise ValueError("The package requests an invalid configuration name.")
        if key in removed:
            continue
        # Source defaults fill the first setup, never restore an explicit absence.
        if new_configuration and field.get("default") and not field.get("secret"):
            values.setdefault(key, field["default"])
        if use_host and key in PROVIDER_KEYS and os.environ.get(key):
            values[key] = os.environ[key]
    for key, value in submitted.items():
        if not isinstance(value, str) or len(value) > 32768 or "\x00" in value:
            raise ValueError("A configuration value is invalid or too long.")
        if value:
            values[key] = value
    for key in removed:
        values.pop(key, None)
    if port is not None:
        values.setdefault("MOBKIT_PORT", str(port))
    try:
        selected_port = int(values.get("MOBKIT_PORT", "0"))
    except ValueError:
        selected_port = 0
    if not 1024 <= selected_port <= 65535:
        raise ValueError("Choose a Console port between 1024 and 65535.")
    validate_configuration_values(fields, values)
    if console_authentication is None:
        authenticated_console = console_configuration(destination, fields)
    elif console_authentication in {"loopback-only", "local-password"}:
        authenticated_console = console_authentication == "local-password"
        if authenticated_console:
            declared_console_authentication({"authentication": console_authentication}, fields)
    else:
        raise ValueError("The package has unsupported Console authentication metadata.")
    if authenticated_console:
        validate_console_configuration_values(fields, values)
    _serialized_private_configuration(values)
    return values


def private_configuration_snapshot(destination):
    """Read bounded private values and file identity only inside the host boundary."""
    path = destination / ".env"
    try:
        descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    except FileNotFoundError:
        return {}, None
    except OSError:
        raise DraftConflict("The private configuration is not an owned ordinary file. No values were changed.") from None
    try:
        info = os.fstat(descriptor)
        if (not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_nlink != 1
                or info.st_mode & 0o077 or info.st_size > 256 * 1024):
            raise DraftConflict("The private configuration is not an owned private ordinary file. No values were changed.")
        with os.fdopen(descriptor, "rb", closefd=False) as stream:
            original = stream.read(256 * 1024 + 1)
        if len(original) > 256 * 1024:
            raise DraftConflict("The private configuration exceeded its safe size. No values were changed.")
    finally:
        os.close(descriptor)
    values = {}
    try:
        for line in original.decode("utf-8").splitlines():
            row = line.strip()
            if not row or row.startswith("#"):
                continue
            key, separator, value = row.partition("=")
            if not separator or not re.fullmatch(r"[A-Z][A-Z0-9_]*", key):
                raise ValueError
            value = parse_setting(value.strip())
            if not isinstance(value, str) or len(value) > 32768 or "\x00" in value:
                raise ValueError
            values[key] = value
    except (ValueError, UnicodeError):
        raise DraftConflict("Existing private configuration could not be read safely. No values were changed.") from None
    identity = (info.st_dev, info.st_ino, info.st_mode, info.st_uid, info.st_nlink, info.st_mtime_ns)
    return values, (original, identity)


def _serialized_private_configuration(values):
    """Use the same bounded bytes for first setup and later replacement."""
    content = "# Private deployment configuration. Never commit this file.\n" + "".join(
        f"{key}={json.dumps(value, ensure_ascii=True)}\n" for key, value in sorted(values.items()))
    if len(content.encode("utf-8")) > 256 * 1024:
        raise ValueError("The private configuration exceeded its safe size. No values were changed.")
    return content


def replace_private_configuration(destination, values, expected):
    """Publish atomically only while the checked private file is still unchanged."""
    content = _serialized_private_configuration(values)
    if private_configuration_snapshot(destination)[1] != expected:
        raise DraftConflict("Private settings changed during setup. Reload; the newer values were preserved.")
    descriptor, name = tempfile.mkstemp(prefix=".env.", suffix=".pending", dir=destination)
    temporary = Path(name)
    try:
        with os.fdopen(descriptor, "w") as stream:
            stream.write(content)
            stream.flush()
            os.fsync(stream.fileno())
        if private_configuration_snapshot(destination)[1] != expected:
            raise DraftConflict("Private settings changed during setup. Reload; the newer values were preserved.")
        os.replace(temporary, destination / ".env")
    finally:
        temporary.unlink(missing_ok=True)


def write_private_settings(destination, values):
    content = _serialized_private_configuration(values)
    temporary = destination / ".env.launch-pending"
    fd = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_TRUNC | os.O_NOFOLLOW, 0o600)
    try:
        with os.fdopen(fd, "w") as stream:
            os.fchmod(stream.fileno(), 0o600)
            stream.write(content)
            stream.flush()
            os.fsync(stream.fileno())
        temporary.replace(destination / ".env")
    finally:
        temporary.unlink(missing_ok=True)


def private_settings(destination, fields, submitted, use_host, *, port=None):
    values = prepare_private_settings(destination, fields, submitted, use_host, port=port)
    write_private_settings(destination, values)


def validate_installed_console_settings(files, destination):
    """Check retained values against the verified package before restart admission."""
    fields = package_configuration_fields(json.loads(files["deployment.json"][0]), files)
    if package_console_scope(files, fields, {})["authentication"] == "local-password":
        values, _ = private_configuration_snapshot(destination)
        validate_console_configuration_values(fields, values)


def read_configuration_presence(path):
    """Return names only; no private values leave the destination setup boundary."""
    if not path.exists() or path.is_symlink():
        return set()
    result = set()
    for line in path.read_text().splitlines():
        key, separator, value = line.partition("=")
        if separator and re.fullmatch(r"[A-Z][A-Z0-9_]*", key):
            try:
                present = parse_setting(value)
            except ValueError:
                continue
            if present:
                result.add(key)
    return result


class DeploymentService:
    def __init__(self, root, guide, implementation, operations=None, service_manager=None):
        self.root = Path(root) / "deployments"
        self.guide, self.implementation, self.operations = guide, implementation, operations
        self.lock = asyncio.Lock()
        self.tasks, self.processes = {}, {}
        self.worker_admissions = {}
        self.application_setup_selection = None
        self.application_setup_preparation = None
        self.application_setup_retention = None
        self.application_setup_continuation = None
        self.application_setup_start = None
        self.application_replacement_selection = None
        self.application_host_selection = None
        self._application_preparations = {}
        # First reads of selected products in flight; deployment_updates keeps the reads.
        self._product_reading = []
        from .deployment_service import UserServiceManager
        self.service_manager = service_manager or UserServiceManager()

    async def _guide(self, identity):
        return await (self.operations.read(self.guide, identity) if self.operations else self.guide.get(identity))

    def agent_team_availability(self):
        """Whether this Forge exports and installs agent team packages.

        Every entry does. A compiled agent team installs the public Meta
        release it pins, checked against that release's checksums, so it does
        not depend on the product a launcher selected for this Forge's own
        applications.
        """
        return {"available": True}

    def require_agent_team_available(self):
        availability = self.agent_team_availability()
        if not availability["available"]:
            raise DraftConflict(availability["reason"])

    def _background_folder_refusal(self, capability, destination=None, *, setup_card=False):
        """Name the macOS folders a launchd login item cannot read, before anything is installed.

        The unit runs Forge's own Python and reads Forge's saved deployment
        files and the installation folder at every login.
        """
        if capability.get("manager") != "launchd":
            return None
        from .deployment_service import launchd_folder_refusal, launchd_interpreter
        return launchd_folder_refusal([launchd_interpreter(), self.root,
                                       *([destination] if destination is not None else [])], setup_card=setup_card)

    @staticmethod
    def _requested_destination(value):
        """The destination as owned_destination reads it; None for one it refuses."""
        if (not isinstance(value, str) or not value.strip() or len(value) > 2000
                or any(ord(character) < 32 for character in value)):
            return None
        try:
            path = Path(value.strip()).expanduser()
        except RuntimeError:
            return None
        return path if path.is_absolute() else None

    def _directory(self, guide_id, deployment_id=None):
        for value in [guide_id, *([deployment_id] if deployment_id else [])]:
            try:
                if str(uuid.UUID(value)) != value:
                    raise ValueError
            except (ValueError, TypeError, AttributeError):
                raise LookupError("This deployment could not be found.") from None
        parent = self.root / guide_id
        if deployment_id is None:
            try:
                deployment_id = json.loads((parent / "latest.json").read_text())["deployment_id"]
            except (OSError, ValueError, KeyError):
                raise LookupError("No local deployment has been started for this setup.") from None
            return self._directory(guide_id, deployment_id)
        return parent / deployment_id

    def _installed_configuration(self, guide_id, deployment_id, *, required_status="stopped", with_files=False,
                                 source_custody=None):
        """Authenticate installed bytes independently of the selected implementation."""
        from .deployment_updates import checked_package
        directory = self._directory(guide_id)
        if directory != self._directory(guide_id, deployment_id):
            raise DraftConflict("A different installation is selected. Reload its private settings.")
        receipt = read_receipt(directory)
        if receipt.get("deployment_id") != deployment_id or receipt.get("guide_id") != guide_id:
            raise DraftConflict("The installed configuration belongs to a different setup.")
        process = self.processes.get(deployment_id)
        if (receipt.get("status") != required_status or deployment_id in self.tasks
                or process is not None and process.returncode is None
                or receipt.get("process_live") is True or receipt.get("runtime_pid") is not None):
            raise DraftConflict("Stop this installed application before changing its private settings.")
        if (required_status == "stopped" and receipt.get("execution_mode", "interactive") == "interactive"
                and receipt.get("interactive_lifecycle_id") is not None and not interactive_cleanup_matches(receipt)):
            raise DraftConflict("The owned runtime's cleanup is unconfirmed. No private-settings access was granted.")
        if (receipt.get("update") or {}).get("status") in {
                "snapshotting", "applying", "verifying", "restoring", "recovering-candidate",
                "recovering-current", "recovery-needed"}:
            raise DraftConflict("Finish or recover this installation's source update before changing private settings.")
        _, files = checked_package(directory, "package.zip", receipt["package_sha256"])
        try:
            definition = json.loads(files["deployment.json"][0])
            digest = read_package_source(files, receipt)["source_digest"]
            if definition.get("target") == "gcp" and source_custody is not None:
                from .product_runtime_custody import (gcp_product_deployment_target,
                    require_product_installation)
                from .runtime_selection import parse_candidate_selection
                require_product_installation(source_custody, receipt)
                physical_target = gcp_product_deployment_target(source_custody,
                    parse_candidate_selection(receipt.get("runtime_candidate")),
                    destination=receipt.get("destination"), files=files)
                local_target({"target": physical_target})
            else:
                local_target(definition)
            fields = package_configuration_fields(definition, files)
        except (ValueError, KeyError, TypeError):
            raise DraftConflict("The checked installed configuration could not be verified on this computer.") from None
        destination = verify_destination(receipt, files)
        if (required_status == "stopped" and receipt.get("execution_mode", "interactive") == "interactive"
                and receipt.get("interactive_lifecycle_id") is not None):
            cleanup = receipt["interactive_cleanup"]
            if (cleanup.get("source_digest") != digest
                    or cleanup.get("owner_sha256") != hashlib.sha256((destination / OWNER_FILE).read_bytes()).hexdigest()):
                raise DraftConflict("The cleanup confirmation belongs to different installed source or ownership.")
            self._require_cleanup_supervisor_absent(receipt)
        for name in [*files, OWNER_FILE]:
            info = (destination / name).stat()
            if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_nlink != 1:
                raise DraftConflict("Installed source ownership could not be verified. No private values were changed.")
        lock = destination / ".mobkit/launch.lock"
        if lock.exists() or lock.is_symlink():
            info = lock.lstat()
            if (not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid()
                    or info.st_nlink != 1 or info.st_mode & 0o077):
                raise DraftConflict("The installed process lock could not be verified. No private values were changed.")
        snapshot = directory, receipt, destination, definition, fields, digest
        return (snapshot, files) if with_files else snapshot

    @staticmethod
    def _configuration_binding(receipt):
        result = {key: deepcopy(receipt.get(key)) for key in (
            "deployment_id", "guide_id", "guide_version", "build_id", "package_sha256", "blueprint_hash",
            "source_kind", "source_id", "source_digest",
            "brief_hash", "destination", "execution_mode", "service", "service_supervisor_pid",
            "service_instance_id", "service_cleanup_confirmed", "update",
            "interactive_lifecycle_id", "interactive_cleanup_attempt_id", "interactive_cleanup")}
        if (type(receipt.get("runtime_candidate")) is dict
                and receipt["runtime_candidate"].get("format") == "forge-meta-toolkit-runtime-selection-v1"):
            result["runtime_candidate"] = deepcopy(receipt["runtime_candidate"])
        return result

    @staticmethod
    def _installed_cleanup_matches(snapshot):
        _, receipt, destination, _, _, digest = snapshot
        cleanup = receipt.get("interactive_cleanup") or {}
        return bool(interactive_cleanup_matches(receipt) and cleanup.get("source_digest") == digest
                and cleanup.get("owner_sha256") == hashlib.sha256((destination / OWNER_FILE).read_bytes()).hexdigest())

    @staticmethod
    def _require_cleanup_supervisor_absent(receipt):
        cleanup = receipt.get("interactive_cleanup") or {}
        if cleanup.get("boundary") == "worker-not-admitted":
            return
        try:
            os.kill(cleanup["supervisor_pid"], 0)
        except ProcessLookupError:
            return
        except (OSError, OverflowError, KeyError):
            raise DraftConflict("The original owned supervisor's exit could not be observed. Preserve this folder; cleanup remains unconfirmed.") from None
        raise DraftConflict("The original owned supervisor has not confirmed absence. Wait for cleanup before settings or a fresh start.")

    async def _configuration_validation(self, source_custody, function, /, *args, **kwargs):
        """Keep a guest's original owner on-task while file work is joined."""
        if source_custody is None:
            return function(*args, **kwargs)
        from .remote_product_destination_owner import StoppedProductSource
        from .product_runtime_custody import await_product_validation
        if type(source_custody) is not StoppedProductSource:
            raise DraftConflict("Private settings require their actual entered installed source.")
        source_custody.require_owner_current()
        result = await await_product_validation(source_custody, function, *args, **kwargs)
        source_custody.require_owner_current()
        return result


    async def _stopped_configuration(self, guide_id, deployment_id, version=None, *, source_custody=None):
        if source_custody is not None and self.guide is None and self.implementation is None:
            from .product_runtime_custody import require_product_installation
            from .deployment_application_bootstrap import (require_bootstrap_history_settled,
                require_restore_history_current)
            from .connect_installer_caller import _same_json_value
            from .deployment_updates import require_replacement_history_settled
            from .deployment_supervisor import pending_custody
            if not self.lock.locked():
                raise DraftConflict("Destination settings require their original deployment lock.")
            from .product_runtime_custody import product_source_custody
            source_custody = product_source_custody(source_custody)
            source_custody.require_owner_current()
            context = source_custody._context
            if (self.root != Path(context["host"]["home"]) / ".local/state/meta-forge/deployments"
                    or guide_id != context["guide_id"] or deployment_id != context["deployment_id"]):
                raise DraftConflict("Destination settings differ from their original canonical deployment owner.")
            snapshot = await self._configuration_validation(source_custody,
                self._installed_configuration, guide_id, deployment_id, source_custody=source_custody)
            directory, receipt, destination, *_ = snapshot
            _, _, stopped = self._stopped_application_owner(guide_id, deployment_id)
            if (type(receipt.get("guide_version")) is not int
                    or version is not None and (type(version) is not int or version != receipt["guide_version"])
                    or source_custody._lease.root != destination):
                raise DraftConflict("Destination settings changed their original installed version or lease.")

            view = source_custody._installed_view
            startup = deepcopy(view.inputs) if view is not None else None

            def current():
                require_product_installation(source_custody, receipt)
                stopped()
                if (source_custody._installed_view is not view
                        or view is not None and not _same_json_value(view.inputs, startup)):
                    raise DraftConflict("Settings changed their original admitted startup selection.")
                if startup is None:
                    require_bootstrap_history_settled(directory)
                else:
                    require_restore_history_current(directory, startup)
                require_replacement_history_settled(directory)
                if pending_custody(directory, receipt):
                    raise DraftConflict("The original installation support owner remains unsettled.")
                fresh = self._installed_configuration(guide_id, deployment_id, source_custody=source_custody)
                if (self._configuration_binding(fresh[1]) != self._configuration_binding(receipt)
                        or fresh[-1] != snapshot[-1]):
                    raise DraftConflict("The original installed source changed while reading private settings.")
                return fresh

            fresh = await self._configuration_validation(source_custody, current)
            return {"id": guide_id, "version": receipt["guide_version"]}, fresh
        state = await self._guide(guide_id)
        if (state.get("id") != guide_id or type(state.get("version")) is not int
                or version is not None and (type(version) is not int or state["version"] != version)
                or (state.get("operation") or {}).get("status") == "running"):
            raise DraftConflict("The guide changed or is busy. Reload private settings before saving.")
        snapshot = self._installed_configuration(guide_id, deployment_id, source_custody=source_custody)
        directory, receipt, *_ = snapshot
        if receipt.get("execution_mode") == "background":
            from .deployment_service import require_stopped_service
            await require_stopped_service(directory, self.service_manager)
        native_team = receipt.get("source_kind") == "native-team"
        try:
            candidate = None if native_team else await self.implementation.status(guide_id)
        except LookupError:
            candidate = None
        except (OSError, ValueError, KeyError, TypeError):
            raise DraftConflict("The application builder's current state could not be verified. Reload before changing private settings.") from None
        if candidate is not None:
            identity = candidate.get("build_id")
            process = getattr(self.implementation, "processes", {}).get(identity)
            if (candidate.get("status") not in {"completed", "failed", "interrupted"}
                    or identity in getattr(self.implementation, "tasks", {})
                    or process is not None and process.returncode is None
                    or any(key in candidate and candidate[key] is not True
                           for key in ("worker_stop_confirmed", "cleanup_confirmed"))):
                raise DraftConflict("Wait for the application builder to stop before changing private settings.")
        current = await self._guide(guide_id)
        fresh = self._installed_configuration(guide_id, deployment_id, source_custody=source_custody)
        try:
            selected = None if native_team else self.implementation.selected_build_id(guide_id)
        except LookupError:
            selected = None
        if (current.get("id") != guide_id or current.get("version") != state["version"]
                or type(current.get("version")) is not int
                or (current.get("operation") or {}).get("status") == "running"
                or selected != (candidate.get("build_id") if candidate is not None else None)
                or self._configuration_binding(fresh[1]) != self._configuration_binding(receipt)
                or fresh[-1] != snapshot[-1]):
            raise DraftConflict("The setup or installation changed while preparing private settings. Reload before saving.")
        return current, fresh

    async def settings_options(self, guide_id, deployment_id, *, console_port=None):
        async with self.lock:
            return await self._settings_options_locked(guide_id, deployment_id, console_port=console_port)

    async def _settings_options_locked(self, guide_id, deployment_id, *, console_port=None,
                                       project_lease=None, source_custody=None):
        """Read through the original lock, borrowing only an actual entered lease."""
        if not self.lock.locked():
            raise DraftConflict("Installed settings require their original deployment lock.")
        if project_lease is not None:
            from .templates.storage_setup import ProjectLockLease
            if type(project_lease) is not ProjectLockLease:
                raise DraftConflict("Installed settings require their actual original project lease.")
            project_lease.require_current()
        if source_custody is not None:
            from .product_runtime_custody import product_source_custody
            source_custody = product_source_custody(source_custody)
            source_custody.require_owner_current()
            if project_lease is not source_custody._lease:
                raise DraftConflict("Destination settings require their identical entered source lease.")
        state, snapshot = await self._stopped_configuration(guide_id, deployment_id,
            source_custody=source_custody)
        directory, receipt, destination, definition, fields, digest = snapshot
        if project_lease is not None:
            if project_lease.root != destination:
                raise DraftConflict("Installed settings belong to another project's lease.")
            project_lease.require_current()
        scope = project_lock(destination) if project_lease is None else nullcontext(project_lease)
        with scope as lease:
            def read():
                lease.require_current()
                fresh = self._installed_configuration(guide_id, deployment_id, source_custody=source_custody)
                if self._configuration_binding(fresh[1]) != self._configuration_binding(receipt):
                    raise DraftConflict("The installation changed while reading private settings. Reload before saving.")
                values, _ = private_configuration_snapshot(destination)
                public_fields = [{**{key: value for key, value in field.items()
                                    if key != "default" or not field.get("secret")},
                                  "configured": bool(values.get(field["key"])),
                                  "removable": not field.get("required", False) and not field.get("automatic", False)}
                                 for field in fields]
                lease.require_current()
                return public_fields
            public_fields = await self._configuration_validation(source_custody, read)
        keys = [field["key"] for field in fields if field["key"] in PROVIDER_KEYS
                and not field.get("automatic") and os.environ.get(field["key"])]
        observation = observe_local_console_port(console_port, suggest_alternate=True) if console_port is not None else None
        if source_custody is not None:
            source_custody.require_owner_current()
            project_lease.require_current()
        return {"version": state["version"], "deployment_id": deployment_id, "build_id": receipt["build_id"],
                "source_digest": digest, "package_sha256": receipt["package_sha256"], **native_fields(receipt),
                **({"current_guide_version": state["version"]} if receipt.get("source_kind") == "native-team" else {}),
                "guide_version": receipt["guide_version"], "destination": receipt["destination"],
                "execution_mode": receipt.get("execution_mode", "interactive"), "fields": public_fields,
                "console_port": definition["port"], "can_use_host_provider_credentials": bool(keys),
                "host_provider_keys": keys, "source_verified": True, "supported": True,
                "scope": "verified-installed-private-configuration", "requires_stopped": True,
                **({"console_port_observation": observation} if observation is not None else {})}

    async def configure(self, guide_id, version, deployment_id, build_id, source_digest, package_sha256,
                        settings=None, remove_fields=None, use_host_provider_credentials=False,
                        *, source_kind=None, source_id=None, application_settings_validator=None):
        async with self.lock:
            return await self._configure_locked(guide_id, version, deployment_id, build_id,
                source_digest, package_sha256, settings, remove_fields, use_host_provider_credentials,
                source_kind=source_kind, source_id=source_id,
                application_settings_validator=application_settings_validator)

    async def _configure_locked(self, guide_id, version, deployment_id, build_id, source_digest, package_sha256,
                        settings=None, remove_fields=None, use_host_provider_credentials=False,
                        *, source_kind=None, source_id=None, application_settings_validator=None,
                        project_lease=None, source_custody=None):
        if not self.lock.locked():
            raise DraftConflict("Configure requires its original deployment lock.")
        if project_lease is not None:
            from .templates.storage_setup import ProjectLockLease
            if type(project_lease) is not ProjectLockLease:
                raise DraftConflict("Configure requires its actual entered project lease.")
            project_lease.require_current()
        if type(use_host_provider_credentials) is not bool:
            raise ValueError("Choose whether to use this computer's provider credentials explicitly.")
        _, snapshot = await self._stopped_configuration(guide_id, deployment_id, version, source_custody=source_custody)
        directory, receipt, destination, definition, fields, digest = snapshot
        if (receipt.get("source_kind") == "native-team"
                and (source_kind != "native-team" or source_id != receipt.get("source_id"))
                or receipt.get("source_kind", "application") == "application"
                and (source_kind not in {None, "application"} or source_id is not None)):
            raise DraftConflict("The selected installed source changed. Reload its private settings.")
        if (build_id != receipt["build_id"] or source_digest != digest
                or package_sha256 != receipt["package_sha256"]):
            raise DraftConflict("The installed source changed. Reload its private settings before saving.")
        submitted = settings if settings is not None else {}
        if (not isinstance(submitted, dict) or any(field.get("automatic") and field["key"] in submitted
                                                  for field in fields)):
            raise ValueError("Automatic private settings cannot be replaced through this form.")
        if source_custody is not None and project_lease is not source_custody._lease:
            raise DraftConflict("Destination Configure requires its identical original source lease.")
        scope = project_lock(destination) if project_lease is None else nullcontext(project_lease)
        with scope as lease:
            if lease.root != destination:
                raise DraftConflict("Configure belongs to another project's lease.")
            lease.require_current()
            def save():
                fresh, files = self._installed_configuration(guide_id, deployment_id, with_files=True, source_custody=source_custody)
                if self._configuration_binding(fresh[1]) != self._configuration_binding(receipt) or fresh[-1] != digest:
                    raise DraftConflict("The installation changed while saving private settings. Reload before saving.")
                if application_settings_validator is not None:
                    validated = application_settings_validator(guide_id, fresh[1], fresh[2], files, submitted)
                    if validated is not None:
                        if asyncio.iscoroutine(validated):
                            validated.close()
                        raise ValueError("Installed settings validation must complete synchronously.")
                existing, private_snapshot = private_configuration_snapshot(destination)
                values = prepare_private_settings(destination, fields, submitted, use_host_provider_credentials,
                    port=definition["port"], remove_fields=remove_fields if remove_fields is not None else (),
                    existing_values=existing)
                if private_configuration_snapshot(destination)[1] != private_snapshot:
                    raise DraftConflict("Private settings changed during setup. Reload; the newer values were preserved.")
                if values == existing:
                    return read_receipt(directory)
                if source_custody is None:
                    replace_private_configuration(destination, values, private_snapshot)
                else:
                    from .product_installed_source import replace_installed_private_configuration
                    replace_installed_private_configuration(source_custody, values, private_snapshot)
                journal(directory, "Private settings saved. This application remains stopped; start it explicitly when ready.",
                        health="unknown", health_checked_at=None, console_url=None,
                        startup_attempt_id=None, startup_console_health=None)
                lease.require_current()
                return read_receipt(directory)
            return await self._configuration_validation(source_custody, save)


    def _require_installed_package_selected(self, directory, guide_id):
        from .connect_installer_caller import _read
        selected = json.loads(_read(directory.parent / "latest.json", MAX_PACKAGE_BYTES))
        if (not isinstance(selected, dict) or not selected.get("deployment_id")
                or directory != self._directory(guide_id, selected["deployment_id"])):
            raise DraftConflict("A different installation is selected. Reload before downloading its package.")

    def _installed_package_read(self, directory, guide_id, deployment_id, build_id, source_digest, package_sha256):
        """Observe only retained source and ownership, never installed settings."""
        from .connect_application_update_startup import _file_identity
        from .connect_accepted_source import _read as read_owned_source
        from .connect_installer_caller import _read
        selected_path = directory.parent / "latest.json"
        selected_identity = _file_identity(selected_path)
        self._require_installed_package_selected(directory, guide_id)
        receipt = json.loads(_read(directory / "receipt.json", MAX_PACKAGE_BYTES))
        if (receipt.get("deployment_id") != deployment_id or receipt.get("guide_id") != guide_id
                or receipt.get("target") != "local" or receipt.get("source_kind", "application") != "application"
                or receipt.get("build_id") != build_id or receipt.get("package_sha256") != package_sha256):
            raise DraftConflict("The selected installed application does not match this package request.")
        if (receipt.get("update") or {}).get("status") in {
                "snapshotting", "applying", "verifying", "restoring", "recovering-candidate",
                "recovering-current", "recovery-needed"}:
            raise DraftConflict("Finish or recover this installation's source update before downloading its package.")
        # The retained caller reader binds a bounded, nonblocking ordinary fd
        # to its final path. A replaced FIFO cannot hang this read thread.
        payload = _read(directory / "package.zip", MAX_PACKAGE_BYTES)
        if hashlib.sha256(payload).hexdigest() != package_sha256:
            raise DraftConflict("The retained installed package changed. No package was rebuilt.")
        files = package_files(payload)
        source = read_package_source(files, receipt)
        if (source["source_digest"] != source_digest
                or receipt.get("source_digest") not in {None, source_digest}
                or receipt.get("brief_hash") not in {None, source["brief_hash"]}
                or receipt.get("source_id") not in {None, build_id}):
            raise DraftConflict("The installed source does not match this package request.")
        destination = owned_destination(receipt["destination"])
        observations = [(selected_path, selected_identity), (destination, _file_identity(destination))]
        source_pins = []
        for name, (expected, _) in files.items():
            path = destination / name
            content, pin = read_owned_source(path, len(expected))
            if content != expected:
                raise DraftConflict("Installed source differs from the checked package. Preserve this folder.")
            source_pins.append(pin)
        content, owner_pin = read_owned_source(destination / OWNER_FILE, MAX_PACKAGE_BYTES, private=True)
        if json.loads(content) != ownership(receipt):
            raise DraftConflict("This folder belongs to another deployment. No package was downloaded.")
        source_pins.append(owner_pin)
        for pin in source_pins:
            pin.require_current()
        if any(_file_identity(path) != identity for path, identity in observations):
            raise DraftConflict("Installed source changed during the package download.")
        return payload, receipt, observations, source_pins

    async def installed_package(self, guide_id, deployment_id, build_id, source_digest, package_sha256):
        """Download the exact selected installation archive without rebuilding it."""
        from .connect_application_update_startup import _file_identity
        from .connect_installer_caller import CustodyError, _read
        try:
            if (not isinstance(build_id, str) or str(uuid.UUID(build_id)) != build_id
                    or any(not isinstance(value, str) or not re.fullmatch(r"[a-f0-9]{64}", value)
                           for value in (source_digest, package_sha256))):
                raise ValueError("Choose the exact installed application source and package.")
            directory = self._directory(guide_id, deployment_id)
            observed, archive_observation = await await_validation(_observe_status_package,
                self._installed_package_read, directory, guide_id, deployment_id, build_id, source_digest, package_sha256)
            payload, receipt, observations, source_pins = observed
            self._require_installed_package_selected(directory, guide_id)
            _require_status_package_current(directory, archive_observation)
            current = json.loads(_read(directory / "receipt.json", MAX_PACKAGE_BYTES))
            if (self._configuration_binding(current) != self._configuration_binding(receipt)
                    or current.get("target") != receipt.get("target")):
                raise DraftConflict("The installed source or ownership changed during the package download.")
            if any(_file_identity(path) != identity for path, identity in observations):
                raise DraftConflict("Installed source changed during the package download.")
            for pin in source_pins:
                pin.require_current()
            return payload
        except (OSError, CustodyError, KeyError, TypeError, AttributeError, json.JSONDecodeError) as error:
            raise DraftConflict("The retained installed package or its source ownership could not be verified.") from error

    async def native_package(self, guide_id, version):
        self.require_agent_team_available()
        state = await self._guide(guide_id)
        if state.get("id") != guide_id:
            raise DraftConflict("The accepted plan belongs to a different guide.")
        payload, source, slug = await asyncio.to_thread(compile_native_source, state, version)
        latest = await self._guide(guide_id)
        if latest.get("id") != guide_id:
            raise DraftConflict("The accepted plan belongs to a different guide.")
        fresh, checked, _ = await asyncio.to_thread(compile_native_source, latest, version)
        if checked != source or fresh != payload:
            raise DraftConflict("The accepted team source changed. Reload before continuing.")
        return payload, source, slug

    async def _initial_product(self, guide_id, build_id, source, files, *, expected=None):
        definition = json.loads(files["deployment.json"][0])
        if "runtimeCandidate" not in definition:
            return None
        reader = getattr(self.implementation, "selected_runtime_target", None)
        if not callable(reader):
            return None
        from .runtime_selection import MetaToolkitRuntimeTarget, candidate_selection, selected_runtime_artifacts
        target = await await_validation(reader, guide_id, build_id=build_id, source_digest=source["source_digest"])
        if type(target) is not MetaToolkitRuntimeTarget:
            if expected is not None:
                raise DraftConflict("The original implementation-selected product changed before installation.")
            return None
        artifacts = (await self._selected_product_artifacts(target) if expected is None else
            await await_validation(selected_runtime_artifacts, target, verified_artifacts=expected.artifacts))
        selected = candidate_selection(target, verified_artifacts=artifacts)
        if expected is not None and selected != expected.selection:
            raise DraftConflict("The original implementation-selected product changed before installation.")
        artifacts = await await_validation(product_runtime_source, files, selected,
            verified_artifacts=artifacts)
        from .templates.install_candidate import verify_selected_application_dependencies
        await await_validation(verify_selected_application_dependencies,
            files.get("requirements.application.txt", (b"", 0o644))[0], artifacts)
        from types import SimpleNamespace
        # The product's glibc floor goes first: the CPython remedy (PYTHON_BIN)
        # cannot lift it.
        return SimpleNamespace(selection=selected, artifacts=artifacts,
            interpreter_refusal=selected_host_refusal(artifacts.glibc_floor)
                or selected_interpreter_refusal(artifacts.selection))

    async def _selected_product_artifacts(self, target):
        """A selected product's verified artifacts, read in full once while it is unchanged.

        The process's kept read (deployment_updates.KEPT_RUNTIME_READS) is
        returned after every verified input's file fingerprint is rechecked; a
        changed input is read again in full. Concurrent first reads of one
        product share a single read.
        """
        from .deployment_updates import KEPT_RUNTIME_READS
        reading = self.__dict__.setdefault("_product_reading", [])
        task = next((pending for selected, pending in reading if selected == target), None)
        if task is None:
            task = asyncio.create_task(await_validation(KEPT_RUNTIME_READS, target))
            reading.append((target, task))
            task.add_done_callback(lambda done: reading.__setitem__(slice(None),
                [row for row in reading if row[1] is not done]))
        return await asyncio.shield(task)

    async def options(self, guide_id, build_id=None, *, console_port=None, source_kind="application", version=None):
        if source_kind not in {"application", "native-team"}:
            raise ValueError("Choose an application or native-team source.")
        if source_kind == "native-team":
            self.require_agent_team_available()
            if build_id is not None:
                raise ValueError("An agent-team package has no application build ID.")
            payload, source, slug = await self.native_package(guide_id, version)
            state = await self._guide(guide_id)
            status = {**source, "current": True}
        else:
            status = await self.implementation.status(guide_id)
            if build_id and status["build_id"] != build_id:
                raise DraftConflict("A newer application build is selected.")
            require_accepted(status)
            state = await self._guide(guide_id)
            payload, slug = await self.implementation.package(guide_id, status["build_id"])
        proposal = state.get("proposal") or state.get("blueprint") or {}
        # Unpacking the whole package must not hold the event loop.
        files = await asyncio.to_thread(package_files, payload)
        deployment = json.loads(files["deployment.json"][0])
        source = read_package_source(files)
        if source["build_id"] != status["build_id"] or not package_matches(source, state):
            raise DraftConflict("The package belongs to a different proposal or application brief. Reload before installing.")
        product = await self._initial_product(guide_id, status["build_id"], source, files) if source_kind == "application" else None
        fields = [{key: value for key, value in field.items() if key != "default" or not field.get("secret")}
                  for field in package_configuration_fields(deployment, files)]
        target, host_target, handoff = deployment_handoff(deployment, files, proposal, guide_id, status["build_id"])
        if source_kind == "native-team":
            handoff["package_url"] = f"/api/guide/{quote(guide_id, safe='')}/deployment/package?{urlencode({'source_kind': 'native-team', 'version': version})}"
        console_scope = package_console_scope(files, fields, state)
        configured, configured_deployment_id, configured_destination = set(), None, None
        try:
            owned = await self.status(guide_id)
            if (owned["build_id"] == status["build_id"] and all(owned.get(key) == value for key, value in native_fields(source).items()) and type(owned.get("guide_version")) is int
                    and owned["guide_version"] == status["guide_version"]):
                configured = read_configuration_presence(Path(owned["destination"]) / ".env")
                configured_deployment_id, configured_destination = owned["deployment_id"], owned["destination"]
        except LookupError:
            pass
        fields = [{**field, "configured": field["key"] in configured} for field in fields]
        keys = [field["key"] for field in fields if field["key"] in PROVIDER_KEYS and os.environ.get(field["key"])]
        supported = target == host_target and host_target is not None
        background = ({"available": False, "detail": PRODUCT_CONFIGURE_REQUIRED} if product is not None else
            await self.service_manager.capability() if supported else {"available": False})
        refusal = (await await_validation(self._background_folder_refusal, background)
                   if background["available"] else None)
        if refusal is not None:
            background = {**background, "available": False, "detail": refusal}
        if source_kind == "native-team":
            fresh_payload, latest, _ = await self.native_package(guide_id, version)
            if latest != source or fresh_payload != payload:
                raise DraftConflict("The agent team changed while preparing private setup. Reload before installing.")
        else:
            latest = await self.implementation.status(guide_id)
            require_accepted(latest)
            if (latest["build_id"] != status["build_id"] or latest["guide_version"] != status["guide_version"]
                    or not latest["current"] or latest.get("source_digest") != source["source_digest"]):
                raise DraftConflict("The application changed while preparing private setup. Reload before installing.")
        state = await self._guide(guide_id)
        if not package_matches(source, state) or source_kind == "native-team" and state["version"] != version:
            raise DraftConflict("The setup changed while preparing private setup. Reload before installing.")
        if product is not None:
            product = await self._initial_product(guide_id, status["build_id"], source, files, expected=product)
            latest = await self.implementation.status(guide_id)
            require_accepted(latest)
            state = await self._guide(guide_id)
            if (latest["build_id"] != status["build_id"] or latest.get("source_digest") != source["source_digest"]
                    or latest["guide_version"] != status["guide_version"] or not latest["current"]
                    or not package_matches(source, state)):
                raise DraftConflict("The selected product changed while preparing setup. Reload before installing.")
        observation = observe_local_console_port(deployment["port"] if console_port is None else console_port,
                                                suggest_alternate=True) if supported or console_port is not None else None
        refusal = getattr(product, "interpreter_refusal", None)
        return {"build_id": status["build_id"], "guide_version": status["guide_version"],
                "source_digest": source["source_digest"], **native_fields(source),
                "current_guide_version": state["version"], "current": True,
                "default_destination": str(Path.home() / "src" / slug), "home_directory": str(Path.home()), "fields": fields,
                "configured_deployment_id": configured_deployment_id, "configured_destination": configured_destination,
                "console_port": deployment["port"], "can_use_host_provider_credentials": bool(keys) and supported,
                "host_provider_keys": keys if supported else [], "target": target, "host_target": host_target,
                "handoff": handoff, "console_scope": console_scope, "knowledge_mode": deployment["knowledge"]["mode"],
                "supported": supported, "default_execution_mode": "interactive",
                "fresh_install": {"available": supported and ("runtimeCandidate" not in deployment or product is not None)
                        and refusal is None,
                    "reason": refusal or (PRIVATE_RUNTIME_UPDATE_ONLY if "runtimeCandidate" in deployment and product is None else None),
                    **({"next_step": "configure"} if product is not None and refusal is None else {})},
                **({"console_port_observation": observation} if observation is not None else {}),
                "execution_modes": [
                    {"id": "interactive", "label": "While Forge is open", "available": supported,
                     "description": "Forge owns this process. Closing Forge stops the application and preserves its native state."},
                    {"id": "background", "label": "Run in the background", "available": background["available"],
                     "description": background.get("persistence", "Uses this logged-in user's operating-system service manager."),
                     "reason": background.get("detail"), "host_runtime_dependency": background.get("host_runtime_dependency")} ]}

    async def submit(self, guide_id, version, build_id, destination, settings=None, use_host_provider_credentials=False,
                     execution_mode="interactive", *, source_kind="application", source_id=None, source_digest=None):
        async with self.lock:
            if source_kind == "native-team":
                self.require_agent_team_available()
            if execution_mode not in {"interactive", "background"}:
                raise ValueError("Choose interactive or background deployment.")
            if execution_mode == "background":
                capability = await self.service_manager.capability()
                if not capability["available"]:
                    raise ValueError("This user's operating-system service manager is unavailable. Choose interactive deployment.")
                refusal = await await_validation(self._background_folder_refusal, capability,
                    self._requested_destination(destination))
                if refusal is not None:
                    raise ValueError(refusal)
            state = await self._guide(guide_id)
            if state["version"] != version or state.get("operation", {}).get("status") == "running":
                raise DraftConflict("The guide is changing. Install only the current checked proposal.")
            local_target(state.get("proposal") or state.get("blueprint") or {})
            if source_kind == "native-team":
                if build_id is not None:
                    raise ValueError("An agent-team package has no application build ID.")
                payload, source, _ = await self.native_package(guide_id, version)
                if source_id != source["source_id"] or source_digest != source["source_digest"]:
                    raise DraftConflict("The selected agent-team source changed. Reload its setup before installing.")
            elif source_kind == "application":
                if source_id is not None or source_digest is not None:
                    raise ValueError("Application installation uses its independently accepted build ID.")
                checked = await self.implementation.status(guide_id)
                if checked["build_id"] != build_id:
                    raise DraftConflict("A newer application build is selected.")
                require_accepted(checked)
                payload, _ = await self.implementation.package(guide_id, build_id)
            else:
                raise ValueError("Choose an application or native-team source.")
            files = package_files(payload)
            definition = json.loads(files["deployment.json"][0])
            source = read_package_source(files)
            product = await self._initial_product(guide_id, build_id, source, files) if source_kind == "application" else None
            if "runtimeCandidate" in definition and product is None:
                raise DraftConflict(PRIVATE_RUNTIME_UPDATE_ONLY)
            if product is not None and product.interpreter_refusal:
                raise DraftConflict(product.interpreter_refusal)
            if product is not None and execution_mode != "interactive":
                raise DraftConflict(PRODUCT_CONFIGURE_REQUIRED)
            if source["build_id"] != build_id or not package_matches(source, state):
                raise DraftConflict("The package belongs to another proposal.")
            if source["blueprint_hash"] != proposal_hash(state.get("proposal") or state.get("blueprint")):
                raise DraftConflict("The package belongs to another design.")
            fields = package_configuration_fields(definition, files)
            console_authentication = package_console_scope(files, fields, state)["authentication"]
            target = owned_destination(destination)
            previous = None
            try:
                previous = self._directory(guide_id)
                receipt = await self.status(guide_id)
            except LookupError:
                receipt = None
            digest = hashlib.sha256(payload).hexdigest()
            same = receipt and receipt["build_id"] == build_id and receipt.get("source_kind", "application") == source_kind and receipt["package_sha256"] == digest and receipt["destination"] == str(target)
            if same and receipt.get("execution_mode", "interactive") != execution_mode:
                raise DraftConflict("This owned deployment uses another lifecycle mode. Keep its mode or choose a new empty destination.")
            if receipt and receipt["status"] in ACTIVE:
                if same:
                    return receipt
                raise DraftConflict("Stop this setup's current deployment before installing another one.")
            if (same and execution_mode == "interactive" and receipt["status"] != "stopped"):
                raise DraftConflict("Stop and verify cleanup of this installed application before changing settings or retrying setup.")
            if target.exists() and any(target.iterdir()) and not same:
                raise DraftConflict("This folder is not empty. Choose a new empty folder; Forge will not overwrite it.")
            if same:
                if product is not None:
                    from .deployment_application_bootstrap import require_bootstrap_history_settled
                    from .deployment_updates import require_replacement_history_settled
                    if receipt.get("update") is not None:
                        raise DraftConflict("Use the original selected product update or recovery owner; initial installation cannot replace it.")
                    require_bootstrap_history_settled(previous)
                    require_replacement_history_settled(previous)
                verify_destination(receipt, files, allow_missing=True)
            background_receipt = private_snapshot = existing = None
            if same and execution_mode == "background":
                from .deployment_service import require_stopped_service
                from .deployment_updates import checked_package

                def background_snapshot():
                    if self._directory(guide_id) != previous:
                        raise DraftConflict("A different installation is selected. Reload before retrying setup.")
                    current = read_receipt(previous)
                    if self._configuration_binding(current) != self._configuration_binding(receipt):
                        raise DraftConflict("The owned background installation changed. Reload before retrying setup.")
                    if current.get("service"):
                        return self._installed_configuration(guide_id, current["deployment_id"])[1]
                    # A partial install can retry before a service is created,
                    # but missing service metadata is never cleanup evidence.
                    process = self.processes.get(current["deployment_id"])
                    if (current.get("status") not in {"stopped", "failed", "interrupted"}
                            or current["deployment_id"] in self.tasks
                            or process is not None and process.returncode is None
                            or current.get("process_live") is True or current.get("runtime_pid") is not None
                            or current.get("service") is not None or current.get("service_supervisor_pid") is not None
                            or current.get("service_instance_id") is not None
                            or (current.get("update") or {}).get("status") in {
                                "snapshotting", "applying", "verifying", "restoring", "recovering-candidate",
                                "recovering-current", "recovery-needed"}):
                        raise DraftConflict("Stop and verify this owned background installation before retrying setup.")
                    checked_package(previous, "package.zip", current["package_sha256"])
                    verify_destination(current, files, allow_missing=True)
                    return current

                background_receipt = background_snapshot()
                if background_receipt.get("service"):
                    await require_stopped_service(previous, self.service_manager)
                    background_receipt = background_snapshot()
                existing, private_snapshot = private_configuration_snapshot(target)
            # Validate in memory before publishing ownership or a queued deployment.
            # Private values remain within setup and never enter model input or journals.
            values = await asyncio.to_thread(prepare_private_settings, target, fields,
                settings if settings is not None else {}, bool(use_host_provider_credentials), port=definition["port"],
                console_authentication=console_authentication,
                **({"existing_values": existing} if private_snapshot is not None else {}))
            async def check_current_package():
                if source_kind == "native-team":
                    fresh_payload, fresh_source, _ = await self.native_package(guide_id, version)
                    if fresh_source != source or fresh_payload != payload:
                        raise DraftConflict("The accepted team changed before installation. No configuration was saved and no process started.")
                    return
                checked = await self.implementation.status(guide_id)
                latest = await self._guide(guide_id)
                require_accepted(checked)
                if (latest["version"] != version or latest.get("operation", {}).get("status") == "running"
                        or self.implementation.selected_build_id(guide_id) != build_id
                        or checked["build_id"] != build_id or not checked["ready_to_package"] or not checked.get("current")
                        or not package_matches(source, latest)
                        or proposal_hash(latest.get("proposal") or latest.get("blueprint")) != source["blueprint_hash"]
                        or checked.get("source_digest") != source["source_digest"]):
                    raise DraftConflict("The proposal changed before deployment admission. No configuration was saved and no process started.")
            if product is not None:
                product = await self._initial_product(guide_id, build_id, source, files, expected=product)
            await check_current_package()
            # Preparation awaited private reads and authority checks. Verify the
            # destination again before publishing anything into it.
            target = owned_destination(destination)
            stopped_snapshot = None
            if same:
                if product is not None and receipt.get("runtime_candidate") != product.selection:
                    raise DraftConflict("The original installed product selection changed. No setup was saved.")
                verify_destination(receipt, files, allow_missing=True)
                if execution_mode == "interactive":
                    stopped_snapshot = self._installed_configuration(guide_id, receipt["deployment_id"])
            elif target.exists() and any(target.iterdir()):
                raise DraftConflict("This folder is not empty. Choose a new empty folder; Forge will not overwrite it.")
            created_paths, created_directories = [], []
            if same:
                directory = previous
            else:
                identity = str(uuid.uuid4())
                directory = self._directory(guide_id, identity)
                receipt = new_installation_receipt(identity, guide_id, source, version, build_id, digest,
                    target, execution_mode, files, source_kind,
                    runtime_selection=product.selection if product is not None else None)
            with project_lock(target) if stopped_snapshot or background_receipt else nullcontext():
                if stopped_snapshot:
                    fresh = self._installed_configuration(guide_id, receipt["deployment_id"])
                    if product is not None:
                        from .deployment_updates import _same_toml
                        if not _same_toml(self._configuration_binding(fresh[1]), self._configuration_binding(receipt)):
                            raise DraftConflict("The original installed product selection changed. No setup was saved.")
                    if (self._configuration_binding(fresh[1]) != self._configuration_binding(stopped_snapshot[1])
                            or fresh[-1] != stopped_snapshot[-1]):
                        raise DraftConflict("The installed source changed before private setup. Reload before saving.")
                if background_receipt:
                    background_snapshot()
                    if background_receipt.get("service"):
                        await require_stopped_service(previous, self.service_manager)
                    await check_current_package()
                    background_snapshot()
                    if private_configuration_snapshot(target)[1] != private_snapshot:
                        raise DraftConflict("Private settings changed during setup. Reload; the newer values were preserved.")
                # There is no asynchronous gap between the final source fence and admission.
                if product is not None:
                    if same:
                        from .deployment_application_bootstrap import require_bootstrap_history_settled
                        from .deployment_updates import require_replacement_history_settled
                        if read_receipt(directory).get("update") is not None:
                            raise DraftConflict("Use the original selected product update or recovery owner; initial installation cannot replace it.")
                        require_bootstrap_history_settled(directory)
                        require_replacement_history_settled(directory)
                    from .runtime_selection import selected_runtime_artifacts
                    selected_runtime_artifacts(product.artifacts.selection, verified_artifacts=product.artifacts)
                require_available_console_port(int(values["MOBKIT_PORT"]))
                try:
                    if not same:
                        directory.mkdir(parents=True, mode=0o700)
                        created_directories.append(directory)
                        target.mkdir(parents=True, exist_ok=True, mode=0o700)
                        (target / ".mobkit").mkdir(mode=0o700)
                        created_directories.append(target / ".mobkit")
                        for path in (directory / "receipt.json", target / OWNER_FILE):
                            created_paths.extend([path, path.with_suffix(".pending")])
                            write_private(path, receipt if path.parent == directory else ownership(receipt))
                        created_paths.append(directory / "package.zip")
                        descriptor = os.open(directory / "package.zip", os.O_CREAT | os.O_WRONLY | os.O_EXCL, 0o600)
                        with os.fdopen(descriptor, "wb") as stream:
                            stream.write(payload)
                        created_paths.extend([target / ".env", target / ".env.launch-pending", directory / "journal.lock"])
                    if background_receipt:
                        if values != existing:
                            replace_private_configuration(target, values, private_snapshot)
                    else:
                        write_private_settings(target, values)
                    journal(directory, "Installation admitted for this checked package.", status="queued", phase="install",
                            console_url=None, runtime_pid=None, bootstrap=None, checks=[],
                            startup_attempt_id=None, startup_console_health=None, startup_port_observation=None,
                            health="unknown", health_checked_at=None, process_live=False)
                    if not same:
                        created_paths.append((directory.parent / "latest.json").with_suffix(".pending"))
                        write_private(directory.parent / "latest.json", {"deployment_id": identity})
                except OSError:
                    if same:
                        journal(directory, "Private configuration could not be saved. No process started; retry setup explicitly.",
                                status="stopped" if stopped_snapshot or background_receipt and background_receipt.get("service") else "failed", phase="configure", console_url=None, runtime_pid=None, bootstrap=None,
                                health="failed", health_checked_at=None, process_live=False)
                    else:
                        # Only this submission's newly created files are discarded.
                        # An earlier selected deployment and an existing empty folder survive.
                        for path in reversed(created_paths):
                            try:
                                path.unlink(missing_ok=True)
                            except OSError:
                                pass
                        for path in reversed(created_directories):
                            try:
                                path.rmdir()
                            except OSError:
                                pass
                    raise ValueError("Private setup could not be saved. No process started; retry setup or choose another empty folder.") from None
                self._launch(directory, "install")
            return await self.status(guide_id)

    def _launch(self, directory, mode, *, update_selection=None, bootstrap_selection=None,
            bootstrap_owner_source=None):
        receipt = read_receipt(directory)
        if "runtime_candidate" in receipt:
            from .runtime_selection import MetaToolkitRuntimeTarget, parse_candidate_selection
            if (type(parse_candidate_selection(receipt["runtime_candidate"])) is MetaToolkitRuntimeTarget
                    and bootstrap_selection is None and update_selection is None and mode != "install"):
                raise DraftConflict(PRODUCT_CONFIGURE_REQUIRED)
            if (type(parse_candidate_selection(receipt["runtime_candidate"])) is MetaToolkitRuntimeTarget
                    and mode == "install"):
                from .deployment_application_bootstrap import require_bootstrap_history_settled
                from .deployment_updates import require_replacement_history_settled
                if update_selection is not None or receipt.get("update") is not None:
                    raise DraftConflict("Use the original selected product update or recovery owner; initial installation cannot replace it.")
                require_bootstrap_history_settled(directory)
                require_replacement_history_settled(directory)
        if bootstrap_selection is not None and (update_selection is not None or mode != "start"
                or receipt.get("execution_mode") != "interactive" or receipt.get("service")):
            raise DraftConflict("Bootstrap requires its distinct original interactive Start admission.")
        if receipt.get("execution_mode", "interactive") == "interactive":
            # Every new worker owns fresh teardown evidence. Transaction modes
            # cannot inherit an install/start completion marker from old bytes.
            lifecycle = str(uuid.uuid4())
            update_receipt(directory, interactive_lifecycle_id=lifecycle,
                           interactive_cleanup_attempt_id=None, interactive_cleanup=None)
            self.worker_admissions.pop(directory.name, None)
            self.worker_admissions[directory.name] = {"lifecycle_id": lifecycle, "spawn_started": False, "mode": mode}
        # Capture the action owner's pin synchronously, before the task can
        # reread mutable journal/receipt projections on another turn.
        if bootstrap_selection is not None:
            run = self._run(directory, mode, bootstrap_selection=deepcopy(bootstrap_selection),
                **({"bootstrap_owner_source": bootstrap_owner_source} if bootstrap_owner_source is not None else {}))
        elif update_selection is not None:
            run = self._run(directory, mode, update_selection=deepcopy(update_selection))
        elif mode == "install" and self.application_setup_retention is not None:
            original = self._configuration_binding(read_receipt(directory))
            for key in ("interactive_cleanup", "interactive_cleanup_attempt_id"):
                original.pop(key)
            run = self._run(directory, mode, install_retention=self.application_setup_retention,
                install_binding=original)
        else:
            run = self._run(directory, mode)
        task = asyncio.create_task(run)
        self.tasks[directory.name] = task
        if mode == "install" and self.application_setup_retention is not None:
            # Retention may refuse after the exited handle is retired. Retrieve
            # that same task's exception without changing its result or custody,
            # and keep its typed text, or only its type name, in the Forge log.
            task.add_done_callback(_log_install_completion(directory.name))

    async def prepare_supervisor_refresh(self, guide_id, deployment_id, *, expected_service_digest):
        """Prepare host support separately from application update or startup."""
        async with self.lock:
            return await self._prepare_supervisor_refresh_locked(guide_id, deployment_id,
                expected_service_digest=expected_service_digest)

    def _support_current(self, guide_id, deployment_id, directory):
        if self._directory(guide_id) != directory:
            raise DraftConflict("A newer deployment is now selected.")
        receipt = read_receipt(directory)
        if receipt.get("deployment_id") != deployment_id or receipt.get("guide_id") != guide_id:
            raise DraftConflict("The selected installation owner changed.")
        if deployment_id in self.tasks:
            raise DraftConflict("Wait for this deployment's active operation before updating its supervisor.")

    async def _require_installed_service_readable(self, directory, deployment_id=None):
        """Refuse a start or support refresh of an earlier launchd unit in a protected macOS folder.

        It runs before any manager call. Stop is never refused.
        """
        from .deployment_service import installed_service_folder_refusal
        receipt = read_receipt(directory)
        if ((deployment_id is not None and receipt.get("deployment_id") != deployment_id)
                or receipt.get("execution_mode") != "background" or not isinstance(receipt.get("service"), dict)):
            return
        refusal = await await_validation(installed_service_folder_refusal, receipt["service"], receipt)
        if refusal is not None:
            raise DraftConflict(refusal)

    async def _prepare_supervisor_refresh_locked(self, guide_id, deployment_id, *, expected_service_digest, revision=None):
        directory = self._directory(guide_id)
        self._support_current(guide_id, deployment_id, directory)
        await self._require_installed_service_readable(directory, deployment_id)
        current = lambda: self._support_current(guide_id, deployment_id, directory)
        from .deployment_supervisor import prepare, require_selected_stopped
        selected = read_receipt(directory).get("supervisor_refresh")
        if revision and isinstance(selected, dict) and selected.get("revision") == revision:
            await require_selected_stopped(directory, revision, self.service_manager,
                expected_digest=selected["descriptor_sha256"], require_current=current)
        else:
            from .deployment_service import require_stopped_service
            await require_stopped_service(directory, self.service_manager)
        current()
        return prepare(directory, expected_service_digest, revision=revision)

    async def apply_supervisor_refresh(self, guide_id, deployment_id, revision, *, expected_digest):
        """Publish only the exact prepared stopped-service source revision."""
        async with self.lock:
            return await self._apply_supervisor_refresh_locked(guide_id, deployment_id,
                revision, expected_digest=expected_digest)

    async def _apply_supervisor_refresh_locked(self, guide_id, deployment_id, revision, *, expected_digest):
        directory = self._directory(guide_id)
        self._support_current(guide_id, deployment_id, directory)
        await self._require_installed_service_readable(directory, deployment_id)
        from .deployment_supervisor import apply
        return await apply(directory, revision, self.service_manager, expected_digest=expected_digest,
            require_current=lambda: self._support_current(guide_id, deployment_id, directory))

    async def _support_result_locked(self, guide_id, deployment_id):
        directory = self._directory(guide_id)
        self._support_current(guide_id, deployment_id, directory)
        from .deployment_supervisor import support_projection
        support = await support_projection(directory, self.service_manager,
            require_current=lambda: self._support_current(guide_id, deployment_id, directory))
        self._support_current(guide_id, deployment_id, directory)
        return {**read_receipt(directory), "support_request": support,
            "health": "unknown", "console_url": None}

    async def stage_update(self, guide_id, deployment_id, version, build_id):
        async with self.lock:
            receipt = await self.status(guide_id)
            if receipt.get("source_kind") == "native-team":
                raise DraftConflict("Agent-team source updates are not supported here. Keep this installation or choose a new empty destination.")
            if receipt["deployment_id"] != deployment_id:
                raise DraftConflict("A newer deployment is selected.")
            if ((receipt["status"] in ACTIVE and receipt.get("phase") in {"update", "recovery"})
                    or (receipt.get("update") or {}).get("status") in {"snapshotting", "applying", "verifying", "restoring", "recovering-candidate", "recovering-current", "recovery-needed"}):
                raise DraftConflict("Finish or recover the current update before staging another package.")
            state = await self._guide(guide_id)
            if state["version"] != version or state.get("operation", {}).get("status") == "running":
                raise DraftConflict("Wait for the current guide revision before staging an update.")
            local_target(state.get("proposal") or state.get("blueprint") or {})
            checked = await self.implementation.status(guide_id)
            require_accepted(checked)
            if checked["build_id"] != build_id or not checked.get("ready_to_package") or not checked.get("current"):
                raise DraftConflict("Choose the exact current independently accepted application.")
            if receipt["build_id"] == build_id:
                raise DraftConflict("This accepted application is already installed.")
            payload, _ = await self.implementation.package(guide_id, build_id)
            # Unpacking the whole package must not hold the event loop.
            files = await asyncio.to_thread(package_files, payload)
            report = json.loads(files["implementation-report.json"][0])
            runtime_target = None
            if "runtimeCandidate" in json.loads(files["deployment.json"][0]):
                # Only the implementation owner's pinned, accepted framework
                # can supply a previously selected candidate. No HTTP field or
                # package marker selects a new runtime for this deployment.
                runtime_target = await asyncio.to_thread(self.implementation.selected_runtime_target,
                    guide_id, build_id=build_id, source_digest=report.get("verification", {}).get("digest"))
                if runtime_target is None:
                    raise DraftConflict("The candidate package has no exact accepted host runtime selection.")
            setup_blockers = await self._setup_owner_blockers(guide_id, deployment_id, files, report)
            # Reading the installed source and every selected runtime, which
            # can be a full native product, must not hold the event loop.
            prepared = await await_validation(self._prepare_update_stage, guide_id, deployment_id,
                payload, report, runtime_target, setup_blockers)
            current = await self.implementation.status(guide_id)
            require_accepted(current)
            latest = await self._guide(guide_id)
            if (latest["version"] != version or latest.get("operation", {}).get("status") == "running"
                    or current["build_id"] != build_id or not current.get("ready_to_package") or not current.get("current")
                    or current.get("source_digest") != report.get("verification", {}).get("digest")
                    or not package_matches(report, latest) or report.get("build_id") != build_id
                    or report.get("blueprint_hash") != proposal_hash(latest.get("proposal") or latest.get("blueprint"))):
                raise DraftConflict("The accepted application changed while preparing its update.")
            # No await separates the final source/design fences and publication,
            # which rechecks what the preparation read.
            from .deployment_updates import publish_stage
            publish_stage(self._directory(guide_id), prepared)
            return await self.status(guide_id)

    def _prepare_update_stage(self, guide_id, deployment_id, payload, report, runtime_target, setup_blockers):
        """Read and check a Stage in a worker thread; nothing is written here."""
        from .deployment_updates import prepare_stage
        restore_selection = self._setup_update_restore(guide_id, deployment_id)
        setup = ({"restore_selection": restore_selection, "setup_blockers": setup_blockers}
            if restore_selection is not None else
            {"ordinary_owner": True, "setup_blockers": setup_blockers}
            if self._ordinary_restart_selected(guide_id, deployment_id) else {})
        if runtime_target is not None:
            setup["runtime_target"] = runtime_target
        return prepare_stage(self._directory(guide_id), payload, report, **setup)

    def _setup_owner(self, guide_id, deployment_id):
        """The selected host that owns a set-up installation's completed setup, or None.

        Only an installation that entered Fresh has one, and only through the
        trusted host selection, never a request. A background Restore service
        keeps the existing Fresh refusal.
        """
        from .deployment_application_bootstrap import FRESH_ENTRY_NAME, restored_service
        from .forge_application_host import ForgeApplicationHost
        directory = self._directory(guide_id, deployment_id)
        entry = directory / FRESH_ENTRY_NAME
        if not entry.exists() and not entry.is_symlink() or restored_service(read_receipt(directory)):
            return None
        select = getattr(self, "application_host_selection", None)
        registry = getattr(select, "__self__", None)
        from .local_application_registry import LocalApplicationRegistry
        if (type(registry) is LocalApplicationRegistry and select == registry.selected_host
                and registry._reconnect_status(self, guide_id, deployment_id) is not None):
            # Only the claimed setup action recovers the owner after a restart.
            from .deployment_updates import SETUP_OWNER_RECONNECT
            raise DraftConflict(SETUP_OWNER_RECONNECT)
        host = (select(guide_id, deployment_id) if callable(select)
            else getattr(getattr(self, "application_setup_selection", None), "__self__", None))
        if type(host) is not ForgeApplicationHost or host._startup_selection is None:
            return None
        return host

    def _setup_update_restore(self, guide_id, deployment_id):
        """The trusted owner's published completed Restore pin, after setup.

        Without a completed Restore, the update path keeps its existing Fresh
        refusal.
        """
        from .deployment_application_bootstrap import _shape
        host = self._setup_owner(guide_id, deployment_id)
        if host is None:
            return None
        try:
            pin, value = host._selected_startup_observation(guide_id, deployment_id)
        except FileNotFoundError:
            return None
        return pin.pin() if _shape(value) == "restore" else None

    async def _setup_owner_blockers(self, guide_id, deployment_id, files, report):
        """Stage refuses, after setup, what the installation's owner would refuse later.

        The candidate must keep the setup owner's protected choices. Under the
        per-installation registry, its signed source chain must also continue
        the installed Build. A single selected host owns every Build's source.
        """
        from .deployment_updates import ORDINARY_OWNER_BLOCKER, SETUP_OWNER_BLOCKER
        from .forge_application_host import owner_choices_match
        from .local_application_registry import LocalApplicationRegistry
        host, blocker = self._setup_owner(guide_id, deployment_id), SETUP_OWNER_BLOCKER
        if host is None:
            host, blocker = (await asyncio.to_thread(self._ordinary_owner, guide_id, deployment_id),
                ORDINARY_OWNER_BLOCKER)
        if host is None:
            return ()
        if not owner_choices_match(host, files):
            return (blocker,)
        select = getattr(self, "application_host_selection", None)
        registry = getattr(select, "__self__", None)
        if type(registry) is LocalApplicationRegistry and select == registry.selected_host:
            continues = await asyncio.to_thread(registry.continues_installed_source, guide_id, deployment_id,
                report.get("build_id"), report.get("verification", {}).get("digest"))
            if not continues:
                return (blocker,)
        return ()

    def _ordinary_owner(self, guide_id, deployment_id):
        """The retained owner of an installed application without a setup helper, or None.

        Only the launcher's registry selection supplies it, never a request.
        Before that owner is reconnected after a Forge restart, Stage keeps its
        existing checks; Apply checks the owner's choices once it reconnects.
        """
        from .local_application_registry import LocalApplicationRegistry
        from .ordinary_application_source import OrdinaryApplicationHost
        if not self._ordinary_restart_selected(guide_id, deployment_id):
            return None
        select = getattr(self, "application_host_selection", None)
        registry = getattr(select, "__self__", None)
        if (type(registry) is not LocalApplicationRegistry or select != registry.selected_host
                or registry._reconnect_status(self, guide_id, deployment_id) is not None):
            return None
        host = select(guide_id, deployment_id)
        return host if type(host) is OrdinaryApplicationHost else None

    def _product_before_setup(self, guide_id, deployment_id):
        """A Meta Toolkit product installation that has not entered its setup.

        No update worker can start it without its setup's startup inputs. An
        application without a setup helper starts through its owner instead.
        """
        from .deployment_application_bootstrap import FRESH_ENTRY_NAME
        from .runtime_selection import MetaToolkitRuntimeTarget, parse_candidate_selection
        directory = self._directory(guide_id, deployment_id)
        selection = read_receipt(directory).get("runtime_candidate")
        entry = directory / FRESH_ENTRY_NAME
        return (selection is not None and type(parse_candidate_selection(selection)) is MetaToolkitRuntimeTarget
            and not entry.exists() and not entry.is_symlink()
            and not self._ordinary_restart_selected(guide_id, deployment_id))

    async def _run(self, directory, mode, *, update_selection=None, bootstrap_selection=None,
            install_retention=None, install_binding=None, bootstrap_owner_source=None):
        process = None
        code = None
        background_update = False
        completed_install = False
        retention = install_retention
        original_install = None
        bootstrap_selection = deepcopy(bootstrap_selection)
        try:
            receipt = read_receipt(directory)
            if (mode == "install" and retention is not None
                    and receipt.get("execution_mode", "interactive") == "interactive"
                    and not receipt.get("service")
                    and (receipt.get("runtime_candidate") or {}).get("format") == "forge-meta-toolkit-runtime-selection-v1"):
                original_install = self._configuration_binding(receipt)
                for key in ("interactive_cleanup", "interactive_cleanup_attempt_id"):
                    original_install.pop(key)
                if original_install != install_binding or self.application_setup_retention != retention:
                    raise DraftConflict("The original queued Install owner changed before its worker entered.")
            bootstrap = None
            if bootstrap_selection is not None:
                if (update_selection is not None or mode != "start"
                        or receipt.get("execution_mode") != "interactive" or receipt.get("service")):
                    raise DraftConflict("Bootstrap requires its distinct original interactive Start admission.")
                from .deployment_application_bootstrap import (
                    bootstrap_lifecycle_binding, observe_installed_bootstrap,
                    require_bootstrap_admission_current)
                original_lifecycle = bootstrap_lifecycle_binding(receipt)
                admission = self.worker_admissions.get(directory.name)
                if admission is not None and admission.get("lifecycle_id") != receipt.get("interactive_lifecycle_id"):
                    raise DraftConflict("The original queued bootstrap worker lifecycle changed.")
                def current_bootstrap_owner():
                    current = read_receipt(directory)
                    active = self.worker_admissions.get(directory.name)
                    if (bootstrap_lifecycle_binding(current) != original_lifecycle
                            or active is not None and active.get("lifecycle_id") != current.get("interactive_lifecycle_id")):
                        raise DraftConflict("The original queued bootstrap owner changed before spawning.")
                selected, observed, bootstrap_owner_source = await observe_installed_bootstrap(
                    directory, bootstrap_selection, require_current=current_bootstrap_owner,
                    owner_source=bootstrap_owner_source)
                bootstrap = selected, observed
                require_bootstrap_admission_current(directory, bootstrap_selection, *bootstrap)
                if bootstrap_lifecycle_binding(read_receipt(directory)) != original_lifecycle:
                    raise DraftConflict("The original bootstrap worker lifecycle changed before spawning.")
                admission = self.worker_admissions.get(directory.name)
                if admission is not None and admission.get("lifecycle_id") != receipt.get("interactive_lifecycle_id"):
                    raise DraftConflict("The original queued bootstrap worker lifecycle changed before spawning.")
            expected = None
            if update_selection is not None:
                from .deployment_updates import read_update_admission, transaction, require_recovery_source
                expected = read_update_admission(directory, mode, update_selection)
                receipt, _, _ = transaction(directory, expected["binding"]["update_id"],
                                            expected_binding=expected["binding"], expected_installation=expected["installation"])
                require_recovery_source(directory, expected)
            if mode in {"update", "rollback", "recover-current"} and expected is None:
                raise DraftConflict("The worker has no original admitted update selection.")
            if receipt.get("execution_mode") == "background" and mode in {"update", "rollback", "recover-current"}:
                background_update = True
                from .deployment_service import update_background
                admission = receipt.get("update_action") or {}
                if admission.get("action") != ("apply-update" if mode == "update" else mode) or not admission.get("update_id"):
                    raise DraftConflict("An exact admitted background update action is required.")
                if admission["update_id"] != expected["binding"]["update_id"]:
                    raise DraftConflict("The original admitted background update changed.")
                await update_background(directory, self.service_manager, mode,
                    update_id=expected["binding"]["update_id"], update_selection=update_selection)
                return
            from .deployment_worker import clean_environment
            source = str(Path(__file__).resolve().parents[1])
            environment = clean_environment()
            environment["PYTHONPATH"] = source
            arguments = [sys.executable, "-m", "launchkit.deployment_worker", str(directory), mode]
            if receipt.get("execution_mode", "interactive") == "interactive":
                # A fixed host wrapper records successful teardown, independently
                # of the application's startup/health result. No authored code
                # can supply the worker's completion signal.
                arguments = [sys.executable, "-c",
                    "import asyncio,sys; from launchkit.owned_worker import serve_as_owned_worker; "
                    "serve_as_owned_worker(); from launchkit.deployment import interactive_worker; "
                    "sys.exit(asyncio.run(interactive_worker(*sys.argv[1:])) or 0)",
                    str(directory), mode, receipt.get("interactive_lifecycle_id", "")]
            if bootstrap_selection is not None:
                owner_argument = (", owner_source_sha256=" + repr(hashlib.sha256(bootstrap_owner_source).hexdigest())
                    if bootstrap_owner_source is not None else "")
                arguments = [sys.executable, "-c",
                    "import asyncio,sys; from launchkit.owned_worker import serve_as_owned_worker; "
                    "serve_as_owned_worker(); from launchkit.deployment import bootstrap_interactive_worker; "
                    "sys.exit(asyncio.run(bootstrap_interactive_worker(*sys.argv[1:]" + owner_argument + ")) or 0)",
                    str(directory), receipt.get("interactive_lifecycle_id", ""),
                    bootstrap_selection["path"], bootstrap_selection["sha256"]]
                require_bootstrap_admission_current(directory, bootstrap_selection, *bootstrap)
                if bootstrap_lifecycle_binding(read_receipt(directory)) != original_lifecycle:
                    raise DraftConflict("The original bootstrap worker lifecycle changed before spawning.")
                admission = self.worker_admissions.get(directory.name)
                if admission is not None and admission.get("lifecycle_id") != receipt.get("interactive_lifecycle_id"):
                    raise DraftConflict("The original queued bootstrap worker lifecycle changed before spawning.")
            elif update_selection is not None:
                arguments.extend((update_selection["path"], update_selection["sha256"]))
            admission = self.worker_admissions.get(directory.name)
            if admission:
                admission["spawn_started"] = True
            spawning = asyncio.create_task(asyncio.create_subprocess_exec(*arguments,
                cwd=source, env=environment, stdin=asyncio.subprocess.PIPE,
                stdout=asyncio.subprocess.DEVNULL, stderr=asyncio.subprocess.DEVNULL, start_new_session=True))
            cancelled = False
            while True:
                try:
                    process = await asyncio.shield(spawning)
                    break
                except asyncio.CancelledError:
                    if spawning.cancelled():
                        raise
                    # Keep the original admission alive through repeated Stop
                    # and Forge shutdown cancellation until its handle returns.
                    cancelled = True
            self.processes[directory.name] = process
            if cancelled:
                raise asyncio.CancelledError
            code = await process.wait()
            receipt = read_receipt(directory)
            completed_install = original_install is not None and code == 0
            if receipt["status"] in ACTIVE and not (receipt.get("execution_mode") == "background" and receipt.get("service")):
                journal(directory, f"The deployment supervisor stopped (exit {code}). Retry from the saved folder.",
                        status="interrupted", console_url=None, runtime_pid=None)
        except asyncio.CancelledError:
            if process and process.returncode is None:
                async def stop_worker():
                    if process.stdin:
                        process.stdin.close()
                    try:
                        await asyncio.wait_for(process.wait(), worker_stop_timeout(directory))
                    except asyncio.TimeoutError:
                        try:
                            os.killpg(process.pid, 9)
                        except ProcessLookupError:
                            pass  # Still reap the original handle after the exit race.
                        try:
                            await asyncio.wait_for(process.wait(), PROCESS_FORCE_REAP_TIMEOUT)
                        except asyncio.TimeoutError:
                            raise RuntimeError("The owned deployment worker did not confirm exit after bounded force cleanup.") from None

                stopping = asyncio.create_task(stop_worker())
                while True:
                    try:
                        await asyncio.shield(stopping)
                        break
                    except asyncio.CancelledError:
                        if stopping.cancelled():
                            raise
                        # One teardown retains its original deadlines. A later
                        # cancellation cannot discard its live worker handle.
            receipt = read_receipt(directory)
            if not (receipt.get("execution_mode") == "background" and receipt.get("service")):
                journal(directory, "Forge stopped. Your installed files and native state are preserved. Start again to resume.",
                        status="interrupted", console_url=None, runtime_pid=None)
            raise
        except Exception as error:
            if not background_update or read_receipt(directory)["status"] != "failed":
                journal(directory, f"The deployment supervisor could not start ({type(error).__name__}). Retry from the saved folder.",
                        status="failed", console_url=None, runtime_pid=None)
        finally:
            try:
                if process is not None:
                    latest = read_receipt(directory)
                    cleanup = latest.get("interactive_cleanup")
                    if process.returncode is None:
                        journal(directory, "The owned supervisor did not confirm exit. Preserve this folder; stop and verify cleanup before settings or a fresh start.",
                                status="interrupted", console_url=None, runtime_pid=None, process_live=True,
                                health="unknown", interactive_cleanup=None, interactive_cleanup_attempt_id=None)
                    elif (latest.get("execution_mode", "interactive") == "interactive"
                            and (latest.get("status") == "stopped" or cleanup is not None)):
                        if (interactive_cleanup_matches(latest, require_worker_exit=False)
                                and cleanup.get("supervisor_pid") == process.pid
                                and type(process.returncode) is int and process.returncode == cleanup.get("return_code")
                                and (code is None or type(code) is int and code == process.returncode)):
                            # Child teardown is provisional until this actual
                            # wrapper handle confirms its intact final exit.
                            update_receipt(directory, interactive_cleanup={**cleanup,
                                "worker_exit_confirmed": True, "worker_completed_at": now()})
                        else:
                            # Persist negative process evidence before losing its
                            # handle, so later reads cannot reuse a stale marker.
                            journal(directory, "The owned supervisor did not confirm an intact cleanup completion. Preserve this folder; settings and a fresh start remain blocked.",
                                    status="interrupted", console_url=None, runtime_pid=None, process_live=False,
                                    interactive_cleanup=None, interactive_cleanup_attempt_id=None)
                if completed_install:
                    # Keep the original exited worker tracked while waiting:
                    # Stop and shutdown must still cancel and join this task.
                    try:
                        async with self.lock:
                            latest = read_receipt(directory)
                            cleanup = latest.get("interactive_cleanup") or {}
                            current = self._configuration_binding(latest)
                            for key in ("interactive_cleanup", "interactive_cleanup_attempt_id"):
                                current.pop(key)
                            if (asyncio.current_task().cancelling() or self.application_setup_retention != retention
                                    or current != original_install or self._directory(latest["guide_id"]) != directory
                                    or latest.get("status") != "stopped" or latest.get("phase") != "configure"
                                    or not interactive_cleanup_matches(latest)
                                    or cleanup.get("boundary") != "owned-process-groups-absent"
                                    or cleanup.get("supervisor_pid") != process.pid or process.returncode != 0
                                    or self.tasks.get(directory.name) is not asyncio.current_task()
                                    or self.processes.get(directory.name) is not process):
                                raise DraftConflict("The original successful Install completion changed before input retention.")
                            # Retirement and trusted source retention are one
                            # synchronous lock-held operation, with no new owner.
                            self.tasks.pop(directory.name)
                            self.processes.pop(directory.name)
                            retention(self, latest["guide_id"], directory.name)
                    except Exception as error:
                        journal(directory, "Trusted setup inputs were not retained" + refusal_detail(error)
                            + " Your files are preserved. Check setup status before continuing.")
                        raise
            finally:
                if self.tasks.get(directory.name) is asyncio.current_task():
                    self.tasks.pop(directory.name, None)
                if process is not None and process.returncode is not None and self.processes.get(directory.name) is process:
                    self.processes.pop(directory.name, None)

    def _destination_control_owner(self, owner, guide_id, deployment_id):
        """Authenticate the selected in-process guest owner, never a saved flag."""
        from .destination_application_factory import DestinationApplicationFactory
        from .forge_application_host import ForgeApplicationHost
        from .runtime_selection import parse_candidate_selection
        if type(owner) is not DestinationApplicationFactory:
            raise DraftConflict("Destination control needs its actual retained application factory.")
        owner._identity(self, guide_id, deployment_id)
        directory = self._directory(guide_id, deployment_id)
        receipt = read_receipt(directory)
        if (directory != owner.directory or type(owner.host) is not ForgeApplicationHost
                or owner.host.guide_id != guide_id or receipt.get("scope") is not None
                or receipt.get("source_kind", "application") != "application"
                or any(receipt.get(key) != owner.context[key] for key in
                    ("guide_id", "deployment_id", "guide_version", "build_id", "package_sha256"))
                or receipt.get("destination") != owner.context["host"]["destination"]
                or parse_candidate_selection(receipt.get("runtime_candidate")) != owner.host._target):
            raise DraftConflict("Destination control differs from its original installed source owner.")
        return owner

    @staticmethod
    def _destination_control_package(directory, receipt, context):
        """File-only installed facts; no source lease or physical target admission."""
        from .deployment_updates import checked_package
        _, files = checked_package(directory, "package.zip", receipt["package_sha256"])
        source = read_package_source(files, receipt)
        if source["source_digest"] != context["source_digest"]:
            raise DraftConflict("Destination control changed its original accepted source digest.")
        verify_destination(receipt, files)
        return source

    async def status(self, guide_id, *, state=None, _joined_file_checks=False, destination_owner=None):
        if state is not None and (not isinstance(state, dict) or state.get("id") != guide_id):
            raise ValueError("Deployment evidence requires the matching detached guide state.")
        directory = self._directory(guide_id)
        receipt = read_receipt(directory)
        if destination_owner is not None:
            if state is not None:
                raise DraftConflict("Destination observation cannot supply the origin's current guide state.")
            self._destination_control_owner(destination_owner, guide_id, directory.name)
        from .deployment_supervisor import pending_custody, support_projection
        if pending_custody(directory, receipt):
            # Pending custody is not an ordinary health observation. Its GET
            # must not invoke background reconciliation or write the receipt.
            support = await support_projection(directory, self.service_manager,
                require_current=lambda: self._support_current(guide_id, directory.name, directory))
            self._support_current(guide_id, directory.name, directory)
            current = read_receipt(directory)
            return {**current, "support_request": support, "current": False,
                "current_guide_version": None, "health": "unknown",
                "console_url": None, "cleanup_recovery": None, "update_capabilities": {
                    "background_activation_supported": False, "requires_stopped": True,
                    "deployment_id": current["deployment_id"], "build_id": current.get("build_id"),
                    "package_sha256": current["package_sha256"],
                    "reason": "Complete the exact installation support update before any service admission."}}
        if (receipt.get("execution_mode") == "background" and receipt.get("service")
                and not (directory.name in self.tasks and (not receipt.get("service_supervisor_pid")
                    or receipt.get("phase") in {"update", "recovery"}))):
            from .deployment_service import background_status
            receipt = await background_status(directory, self.service_manager)
        elif receipt["status"] in ACTIVE and directory.name not in self.tasks:
            receipt = journal(directory, "Forge was interrupted. The owned folder and native state are preserved; resume explicitly.",
                              status="interrupted", console_url=None, runtime_pid=None, process_live=False, health="unknown")
        from .deployment_updates import (inspect_transaction, require_inspection_current,
            require_inspection_binding_current)
        from .deployment_service import background_update_capabilities, _service_health_current
        inspected_receipt = read_receipt(directory)
        destination_source, destination_package = None, None
        async def file_check(function, *args, update=None):
            nonlocal destination_package
            result, observation = await await_validation(_observe_status_package, function, directory, *args)
            _require_status_package_current(directory, observation)
            require_inspection_binding_current(directory, inspected_receipt, update)
            if destination_owner is not None and destination_package is None:
                destination_package = observation
            return result
        # OS observations can project status and runtime PID. Source, service
        # binding, instance and native readiness still belong to this receipt.
        presented = {**receipt, "status": inspected_receipt.get("status"),
                     "runtime_pid": inspected_receipt.get("runtime_pid")}
        if receipt.get("health") != "ready":
            # An unavailable OS observation can clear projected bootstrap.
            # A ready observation must retain its exact checked bootstrap.
            presented["bootstrap"] = inspected_receipt.get("bootstrap")
        same_source = _service_health_current(presented, inspected_receipt)
        if destination_owner is None:
            state = state if state is not None else await self._guide(guide_id)
        else:
            # Active children retain their own lease. These immutable package
            # facts neither reopen a parent source nor select runtime authority.
            self._destination_control_owner(destination_owner, guide_id, directory.name)
            destination_source = await file_check(self._destination_control_package, deepcopy(inspected_receipt),
                deepcopy(destination_owner.context))
            self._destination_control_owner(destination_owner, guide_id, directory.name)
        brief_hash = receipt.get("brief_hash")
        if not brief_hash:
            # Legacy installed packages may already contain the independently
            # accepted full brief digest. Preserve their original provenance.
            try:
                from .deployment_updates import checked_package
                _, files = (await file_check(checked_package, "package.zip", receipt["package_sha256"])
                    if _joined_file_checks else checked_package(directory, "package.zip", receipt["package_sha256"]))
                report = json.loads(files["implementation-report.json"][0])
                brief_hash = report_brief_hash(report)
            except (OSError, ValueError, KeyError, DraftConflict):
                pass
        receipt["current"] = False if destination_owner is not None else (
            artifact_matches(receipt, state, brief_hash=brief_hash) if brief_hash else bool(
                receipt.get("guide_id") == state.get("id") and type(receipt.get("guide_version")) is int and
                state["version"] == receipt["guide_version"] and
                proposal_hash(state.get("proposal") or state.get("blueprint")) == receipt["blueprint_hash"]))
        receipt["brief_hash"] = brief_hash
        receipt["current_guide_version"] = state["version"] if receipt["current"] else None
        try:
            if not same_source:
                raise DraftConflict("The installed source or runtime owner changed during observation.")
            if inspected_receipt.get("update"):
                _, _, update = await await_validation(inspect_transaction, directory, inspected_receipt["update"]["update_id"])
                if _joined_file_checks:
                    await file_check(require_inspection_current, inspected_receipt, update, update=update)
                else:
                    require_inspection_current(directory, inspected_receipt, update)
                # background_status rereads the receipt. Only add the checked
                # journal projection; never restore an older service snapshot.
                receipt["update"] = update
            else:
                update = None
            capability_receipt = {**inspected_receipt, "update": update} if update else inspected_receipt
            if destination_owner is None:
                receipt["update_capabilities"] = await await_validation(background_update_capabilities, directory, deepcopy(capability_receipt))
            else:
                # A destination can observe its original running owner without
                # admitting stopped-only source updates or origin design truth.
                receipt["update_capabilities"] = {"background_activation_supported": False,
                    "requires_stopped": True, "deployment_id": receipt["deployment_id"],
                    "build_id": receipt["build_id"], "package_sha256": receipt["package_sha256"],
                    "reason": "Application source updates remain owned by the original Forge installation at the origin."}
            if _joined_file_checks:
                await file_check(require_inspection_current, inspected_receipt, update, update=update)
            else:
                require_inspection_current(directory, inspected_receipt, update)
            if self._directory(guide_id) != directory:
                raise DraftConflict("A newer deployment is now selected.")
        except DraftConflict as error:
            current = read_receipt(self._directory(guide_id))
            support = await support_projection(self._directory(guide_id), self.service_manager)
            return {**current, "current": False, "current_guide_version": None,
                "support_request": support,
                "process_live": False, "health": "unknown", "console_url": None,
                "cleanup_recovery": None, "update_capabilities": {
                    "background_activation_supported": False, "requires_stopped": True,
                    "deployment_id": current["deployment_id"], "build_id": current["build_id"],
                    "package_sha256": current["package_sha256"], "reason": str(error)}}
        if receipt.get("source_kind") == "native-team":
            receipt["update_capabilities"].update(background_activation_supported=False,
                reason="Agent-team source updates are not supported here. Its installed source and state are preserved.")
        receipt["cleanup_recovery"] = self._cleanup_recovery(guide_id, receipt)
        receipt["support_request"] = await support_projection(directory, self.service_manager,
            require_current=lambda: self._support_current(guide_id, directory.name, directory))
        current = read_receipt(self._directory(guide_id))
        # Compare canonical owners and readiness, preserving the OS projection.
        if not _service_health_current(inspected_receipt, current):
            support = {**receipt["support_request"], "deployment_id": current["deployment_id"],
                "build_id": current.get("build_id"), "package_sha256": current["package_sha256"],
                "available": False, "prepare_available": False, "apply_available": False,
                "stopped_verified": False, "service_digest": None, "selected": None, "phase": "unavailable",
                "reason": "The installation changed while checking its support. Check its current state again."}
            return {**current, "support_request": support, "current": False, "current_guide_version": None,
                "process_live": False, "health": "unknown", "console_url": None, "cleanup_recovery": None,
                "update_capabilities": {"background_activation_supported": False, "requires_stopped": True,
                    "deployment_id": current["deployment_id"], "build_id": current.get("build_id"),
                    "package_sha256": current["package_sha256"], "reason": support["reason"]}}
        if destination_owner is not None:
            self._destination_control_owner(destination_owner, guide_id, directory.name)
            _require_status_package_current(directory, destination_package)
            receipt.update(source_digest=destination_source["source_digest"], source_verified=True)
        return receipt

    def _cleanup_recovery(self, guide_id, receipt):
        if (receipt.get("execution_mode", "interactive") != "interactive" or receipt.get("service")
                or receipt.get("status") not in {"failed", "interrupted"}):
            return None
        result = {"available": False, "deployment_id": receipt.get("deployment_id"),
            "build_id": receipt.get("build_id"), "package_sha256": receipt.get("package_sha256"),
            "guide_version": receipt.get("guide_version"),
            "scope": "owned-interactive-cleanup"}
        if not interactive_cleanup_matches(receipt):
            result["reason"] = ("The failed runtime's cleanup has not been confirmed. A failed status or a free "
                "project lock does not prove its native children stopped. Preserve this folder; private settings remain locked.")
            return result
        try:
            snapshot = self._installed_configuration(guide_id, receipt["deployment_id"], required_status=receipt["status"])
        except (OSError, ValueError, KeyError, TypeError, DraftConflict):
            result["reason"] = "The installed source or remaining worker could not be verified. Keep this folder and inspect its deployment activity before recovery."
            return result
        if not self._installed_cleanup_matches(snapshot):
            result["reason"] = "The cleanup confirmation belongs to different installed source or ownership. Private settings remain locked."
            return result
        try:
            self._require_cleanup_supervisor_absent(receipt)
        except DraftConflict:
            result["reason"] = "The original owned supervisor's absence could not be confirmed. Wait for cleanup; private settings and a fresh start remain blocked."
            return result
        result["available"] = True
        result["source_digest"] = snapshot[-1]
        return result

    def _confirm_worker_not_admitted(self, guide_id, directory, admission):
        """Only this host's original admission can prove a subprocess never began."""
        receipt = read_receipt(directory)
        if (not admission or admission is not self.worker_admissions.get(directory.name)
                or admission.get("spawn_started") is not False
                or admission.get("lifecycle_id") != receipt.get("interactive_lifecycle_id")
                or receipt.get("execution_mode", "interactive") != "interactive" or receipt.get("service")
                or receipt.get("startup_attempt_id") is not None or receipt.get("runtime_pid") is not None
                or directory.name in self.tasks or directory.name in self.processes
                or self._directory(guide_id) != directory):
            return False
        destination = owned_destination(receipt["destination"])
        with project_lock(destination):
            source_binding = interactive_source_binding(directory, receipt, allow_missing=True)
            latest = read_receipt(directory)
            if self._configuration_binding(latest) != self._configuration_binding(receipt):
                raise DraftConflict("This installation changed before worker cancellation could be verified.")
            binding = {key: latest.get(key) for key in ("deployment_id", "guide_id", "build_id", "guide_version",
                "blueprint_hash", "destination", "package_sha256", "startup_attempt_id")}
            binding.update(native_fields(latest))
            journal(directory, "The owned worker was cancelled before subprocess admission. No runtime was started; the checked package and folder are preserved.",
                status="stopped", phase="stop", runtime_pid=None, console_url=None, process_live=False,
                interactive_cleanup={**binding, **source_binding, "lifecycle_id": admission["lifecycle_id"],
                    "boundary": "worker-not-admitted", "confirmed": True, "supervisor_pid": os.getpid(),
                    "return_code": 0, "completed_at": now(), "worker_exit_confirmed": True}, interactive_cleanup_attempt_id=None)
        return True

    def _reconcile_absent_cleanup_supervisor(self, guide_id, deployment_id, directory, before):
        """Observe cold-host absence without claiming a captured wrapper exit code."""
        latest = read_receipt(directory)
        cleanup = latest.get("interactive_cleanup") or {}
        if (deployment_id in self.tasks or deployment_id in self.processes
                or deployment_id in self.worker_admissions
                or self._configuration_binding(latest) != self._configuration_binding(before)
                or latest.get("status") != before.get("status")
                or latest.get("status") not in {"failed", "interrupted"}
                or latest.get("execution_mode", "interactive") != "interactive" or latest.get("service")
                or latest.get("process_live") is not False or latest.get("runtime_pid") is not None
                or not interactive_cleanup_matches(latest, require_worker_exit=False)
                or cleanup.get("worker_exit_confirmed") is not False
                or cleanup.get("boundary") != "owned-process-groups-absent"):
            raise DraftConflict("The failed runtime's cleanup is unconfirmed. No cold-host reconciliation was admitted.")
        snapshot = self._installed_configuration(guide_id, deployment_id, required_status=latest["status"])
        try:
            with project_lock(snapshot[2]):
                # Hold the receipt journal across the final source, ownership and
                # absence checks. A free project lease supplies no teardown proof.
                with receipt_lock(directory):
                    fresh = self._installed_configuration(guide_id, deployment_id, required_status=latest["status"])
                    if (self._configuration_binding(fresh[1]) != self._configuration_binding(latest)
                            or fresh[-1] != snapshot[-1]
                            or deployment_id in self.tasks or deployment_id in self.processes
                            or deployment_id in self.worker_admissions
                            or cleanup.get("source_digest") != fresh[-1]
                            or cleanup.get("owner_sha256") != hashlib.sha256((fresh[2] / OWNER_FILE).read_bytes()).hexdigest()):
                        raise DraftConflict("The installed source or cleanup ownership changed before cold-host reconciliation.")
                    self._require_cleanup_supervisor_absent(fresh[1])
                    # Native teardown was positively recorded by this exact
                    # worker. Its actual OS return code is unavailable on a cold
                    # host; preserve both the saved result and the false flag.
                    observed = now()
                    value = fresh[1]
                    value.update(interactive_cleanup={**cleanup,
                        "supervisor_absence_confirmed": True,
                        "supervisor_absence_scope": "cold-host-owned-native-teardown",
                        "supervisor_absence_observed_at": observed}, updated_at=observed)
                    write_private(directory / "receipt.json", value)
                    return value
        except RuntimeError:
            raise DraftConflict("The installed project is still in use. No cold-host cleanup reconciliation was saved.") from None

    async def _stop_failed_interactive(self, guide_id, deployment_id):
        directory = self._directory(guide_id, deployment_id)
        before = read_receipt(directory)
        if (before.get("execution_mode", "interactive") != "interactive" or before.get("service")
                or before.get("status") not in {"failed", "interrupted"}):
            raise DraftConflict("This installation needs its normal lifecycle controls.")
        process, task = self.processes.get(deployment_id), self.tasks.get(deployment_id)
        if process is not None and process.returncode is None:
            if process.stdin is None:
                raise DraftConflict("The owned worker cannot confirm cleanup. No process was replaced.")
            process.stdin.write(b'{"action":"stop"}\n')
            try:
                await process.stdin.drain()
            except (BrokenPipeError, ConnectionResetError):
                pass  # Its actual return and the fixed cleanup record still gate recovery.
        try:
            if task is not None:
                await asyncio.wait_for(asyncio.shield(task), worker_stop_timeout(directory))
            elif process is not None:
                await asyncio.wait_for(process.wait(), worker_stop_timeout(directory))
        except asyncio.TimeoutError:
            raise DraftConflict("The owned worker has not finished cleanup. No private settings were changed.") from None
        latest = read_receipt(directory)
        if process is None and task is None and not interactive_cleanup_matches(latest):
            latest = self._reconcile_absent_cleanup_supervisor(guide_id, deployment_id, directory, before)
        if (latest.get("interactive_lifecycle_id") != before.get("interactive_lifecycle_id")
                or process is not None and ((latest.get("interactive_cleanup") or {}).get("supervisor_pid") != process.pid
                    or type(process.returncode) is not int
                    or process.returncode != (latest.get("interactive_cleanup") or {}).get("return_code"))
                or not interactive_cleanup_matches(latest)):
            raise DraftConflict("The failed runtime's cleanup is unconfirmed. No stopped state or private-settings access was granted.")
        # Full installed source and receipt authority, plus positive owned
        # teardown, precede the project lease. A lock alone cannot prove cleanup.
        snapshot = self._installed_configuration(guide_id, deployment_id, required_status=latest["status"])
        try:
            with project_lock(snapshot[2]):
                fresh = self._installed_configuration(guide_id, deployment_id, required_status=latest["status"])
                if (self._configuration_binding(fresh[1]) != self._configuration_binding(snapshot[1])
                        or fresh[-1] != snapshot[-1] or not self._installed_cleanup_matches(fresh)):
                    raise DraftConflict("This installation changed during cleanup verification. Reload before recovery.")
                self._require_cleanup_supervisor_absent(fresh[1])
                journal(directory, "The owned runtime finished cleanup and its installed source was verified. Change private settings if needed, then start this installed application explicitly.",
                    status="stopped", phase="stop", runtime_pid=None, console_url=None, process_live=False)
        except RuntimeError:
            raise DraftConflict("The installed project is still in use. Stop its other owned process before cleanup recovery.") from None
        return await self.status(guide_id)

    async def guide_status(self, guide_id, *, state):
        """Project exact installed source evidence without importing application code."""
        from .deployment_updates import checked_package
        receipt = await self.status(guide_id, state=state)
        result = {**receipt, "source_verified": False, "source_digest": None}
        def verified_source():
            _, files = checked_package(self._directory(guide_id), "package.zip", receipt["package_sha256"])
            source = read_package_source(files, receipt)
            digest = source["source_digest"]
            destination = verify_destination(receipt, files)
            for name in files:
                info = (destination / name).stat()
                if info.st_uid != os.getuid() or info.st_nlink != 1:
                    return {}
            return {"source_verified": True, "source_digest": digest}
        try:
            # Reading and hashing the saved package and its installed copy
            # must not hold the event loop.
            result.update(await await_validation(verified_source))
        except (OSError, ValueError, KeyError, TypeError, DraftConflict):
            # A failed source check is evidence of uncertainty, never a model
            # instruction to repair source or permission to use old acceptance.
            pass
        return result

    async def experiences(self, guide_id):
        from .deployment_experiences import installed_application_experiences
        return await installed_application_experiences(self, guide_id)

    async def use(self, guide_id):
        from .installed_feedback import installed_application_use
        return await installed_application_use(self, guide_id)

    async def record_success(self, guide_id, version, deployment_id, build_id, source_digest,
                             package_sha256, *, request_id, task):
        from .installed_feedback import record_installed_success
        return await record_installed_success(self, guide_id, version, deployment_id, build_id,
            source_digest, package_sha256, request_id=request_id, task=task)

    async def feedback(self, guide_id, version, deployment_id, build_id, source_digest, package_sha256,
                       continuation_build_id, continuation_source_digest, *, request, outcome, task="",
                       continue_current_candidate=False):
        from .installed_feedback import improve_installed_application
        return await improve_installed_application(self, guide_id, version, deployment_id, build_id, source_digest,
            package_sha256, continuation_build_id, continuation_source_digest, request=request, outcome=outcome,
            task=task, continue_current_candidate=continue_current_candidate)

    def _action_request_path(self, action_request_id):
        try:
            if not isinstance(action_request_id, str) or str(uuid.UUID(action_request_id)) != action_request_id:
                raise ValueError
        except (ValueError, TypeError, AttributeError):
            raise ValueError("Choose a canonical action request UUID.") from None
        return self.root / "action-requests" / (action_request_id + ".json")

    def _read_action_request(self, path):
        try:
            value = json.loads(path.read_text())
        except FileNotFoundError:
            raise LookupError("This deployment action request has not been recorded.") from None
        except (OSError, ValueError, RecursionError):
            raise DraftConflict("The saved action request could not be verified. Its effects remain uncertain.") from None
        if (not isinstance(value, dict) or value.get("version") != 1
                or value.get("action_request_id") != path.stem
                or value.get("phase") not in {"pending", "settled", "unknown"}
                or (value.get("phase") == "settled" and value.get("outcome") not in {"returned", "error"})
                or (value.get("phase") != "settled" and value.get("outcome") is not None)):
            raise DraftConflict("The saved action request could not be verified. Its effects remain uncertain.")
        return value

    @staticmethod
    def _require_action_request_binding(value, binding):
        if any(value.get(key) != expected for key, expected in binding.items()):
            raise DraftConflict("This action request UUID belongs to a different deployment action. No action was repeated.")

    def _claim_action_request(self, binding):
        path = self._action_request_path(binding["action_request_id"])
        path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        with receipt_lock(path.parent):
            try:
                value = self._read_action_request(path)
            except LookupError:
                value = {"version": 1, **binding, "phase": "pending", "outcome": None}
                write_private(path, value)
                return value, True
            self._require_action_request_binding(value, binding)
            return value, False

    def _settle_action_request(self, binding, outcome, *, resumed=False):
        path = self._action_request_path(binding["action_request_id"])
        with receipt_lock(path.parent):
            value = self._read_action_request(path)
            self._require_action_request_binding(value, binding)
            if resumed and value["phase"] == "settled":
                return value
            if value["phase"] != "pending":
                raise DraftConflict("The action request changed before its host action finished. Its effects remain uncertain.")
            value = {**value, "phase": "settled", "outcome": outcome}
            write_private(path, value)
            return value

    async def action_request(self, guide_id, deployment_id, action_request_id):
        # Exact request metadata only. Ordinary status can observe or reconcile
        # the runtime journal and cannot establish this handler's terminality.
        self._directory(guide_id, deployment_id)
        value = self._read_action_request(self._action_request_path(action_request_id))
        self._require_action_request_binding(value, {"guide_id": guide_id, "deployment_id": deployment_id})
        return {"action_request": value}

    @staticmethod
    def _support_action_fields(action, action_request_id, expected_service_digest, revision, descriptor_sha256, resume):
        if type(action) is not str:
            raise ValueError("Choose a supported deployment action.")
        if type(resume) is not bool:
            raise ValueError("Choose explicit support continuation as a boolean.")
        support = action in {"prepare-support", "apply-support"}
        if not support:
            if resume or any(value is not None for value in (expected_service_digest, revision, descriptor_sha256)):
                raise ValueError("Support fields are only used for installation support actions.")
            return {}
        if action_request_id is None:
            raise ValueError("Installation support requires an exact action request UUID.")
        for value in (expected_service_digest, descriptor_sha256):
            if value is not None and (type(value) is not str or not re.fullmatch(r"[a-f0-9]{64}", value)):
                raise ValueError("Choose the exact installation support digest.")
        if action == "prepare-support":
            if expected_service_digest is None or revision is not None or descriptor_sha256 is not None:
                raise ValueError("Preparing support requires only the exact current service digest.")
        else:
            try:
                if type(revision) is not str or str(uuid.UUID(revision)) != revision:
                    raise ValueError
            except (ValueError, TypeError, AttributeError):
                raise ValueError("Choose the exact prepared support revision.") from None
            if expected_service_digest is not None or descriptor_sha256 is None:
                raise ValueError("Applying support requires only the exact prepared revision and descriptor digest.")
        return {"expected_service_digest": expected_service_digest,
            "revision": revision, "descriptor_sha256": descriptor_sha256}

    async def action(self, guide_id, deployment_id, action, update_id=None, *, action_request_id=None,
                     expected_service_digest=None, revision=None, descriptor_sha256=None, resume=False,
                     destination_owner=None):
        if destination_owner is not None and (action_request_id is None or update_id is not None
                or action not in {"prepare-setup", "finish-setup", "start-background", "stop", "restart"}
                or expected_service_digest is not None or revision is not None
                or descriptor_sha256 is not None or resume):
            raise ValueError("Destination control requires an original setup/lifecycle action UUID without update or support selection.")
        support_fields = self._support_action_fields(action, action_request_id,
            expected_service_digest, revision, descriptor_sha256, resume)
        if support_fields and update_id is not None:
            raise ValueError("An application update identity cannot select installation support.")
        if action in {"prepare-setup", "finish-setup", "start-background"} and (action_request_id is None or update_id is not None):
            raise ValueError("Starting in background requires its exact action request UUID without an update identity." if action == "start-background"
                else "Finishing setup requires its exact action request UUID without an update identity." if action == "finish-setup"
                else "Preparing setup requires its exact action request UUID without an update identity.")
        if (action == "apply-update" and self.application_replacement_selection is not None
                and action_request_id is None):
            raise ValueError("Selected replacement admission requires its exact action request UUID.")
        async with self.lock:
            if action_request_id is None:
                if action == "restart" and self._ordinary_restart_selected(guide_id, deployment_id):
                    raise ValueError("Restarting the ordinary application requires its exact action request UUID.")
                result = await self._action_locked(guide_id, deployment_id, action, update_id)
                if action == "stop":
                    await self._continue_application_setup(guide_id, deployment_id)
                return result
            self._directory(guide_id, deployment_id)
            binding = {"action_request_id": action_request_id, "guide_id": guide_id,
                       "deployment_id": deployment_id, "action": action, "update_id": update_id, **support_fields}
            if resume:
                # Explicit continuation cannot create a previously unrecorded
                # command, nor turn an ordinary cold request into replay.
                value = self._read_action_request(self._action_request_path(action_request_id))
                self._require_action_request_binding(value, binding)
                claimed = False
            else:
                value, claimed = self._claim_action_request(binding)
            continuing = bool(resume and support_fields and value["phase"] == "pending")
            if not claimed and not continuing:
                # Never replay either a settled request or an unresolved cold
                # admission. Native/service state still belongs to its journal.
                directory = self._directory(guide_id, deployment_id)
                result = {**read_receipt(directory), "action_request": value}
                if support_fields:
                    from .deployment_supervisor import support_projection
                    result["support_request"] = await support_projection(directory, self.service_manager,
                        require_current=lambda: self._support_current(guide_id, deployment_id, directory))
                return result
            try:
                if destination_owner is not None:
                    self._destination_control_owner(destination_owner, guide_id, deployment_id)
                if support_fields:
                    if action == "prepare-support":
                        await self._prepare_supervisor_refresh_locked(guide_id, deployment_id,
                            expected_service_digest=expected_service_digest, revision=action_request_id)
                    else:
                        await self._apply_supervisor_refresh_locked(guide_id, deployment_id,
                            revision, expected_digest=descriptor_sha256)
                    result = await self._support_result_locked(guide_id, deployment_id)
                elif action == "prepare-setup":
                    if not callable(self.application_setup_preparation):
                        raise DraftConflict("The original application preparation inputs have not been selected by this host.")
                    await self.application_setup_preparation(self, guide_id, deployment_id)
                    result = read_receipt(self._directory(guide_id, deployment_id))
                elif action == "finish-setup":
                    starter = self.application_setup_start
                    if starter is not None:
                        import inspect
                        if (not callable(starter) or not any(inspect.iscoroutinefunction(value)
                                for value in (starter, getattr(starter, "__call__", None)))):
                            raise DraftConflict("Preserve the original application startup operation owner.")
                        await self._continue_application_setup(guide_id, deployment_id)
                        result = await starter(self, guide_id, deployment_id)
                        if self.application_setup_start != starter:
                            raise DraftConflict("The original application startup operation owner changed during admission.")
                    elif not callable(self.application_setup_selection):
                        raise DraftConflict("The original application setup inputs have not been selected by this host.")
                    else:
                        await self._continue_application_setup(guide_id, deployment_id)
                        selected = self.application_setup_selection(guide_id, deployment_id)
                        result = await self._start_application_locked(guide_id, deployment_id, application_startup=selected)
                elif action == "start-background":
                    await self._continue_application_setup(guide_id, deployment_id)
                    selection_owner = self.application_setup_selection
                    selected = self._selected_service_restore(selection_owner, guide_id, deployment_id)

                    def require_owner_current():
                        from .connect_installer_caller import _same_json_value
                        current = self._selected_service_restore(selection_owner, guide_id, deployment_id)
                        if not _same_json_value(current, selected):
                            raise DraftConflict("The original service Restore selection changed during admission.")

                    result = await self._start_restored_service_locked(guide_id, deployment_id,
                        application_startup=selected, require_owner_current=require_owner_current,
                        **({"destination_owner": destination_owner} if destination_owner is not None else {}))
                elif (action == "restart" and destination_owner is None
                        and self._ordinary_restart_selected(guide_id, deployment_id)):
                    if update_id is not None:
                        raise ValueError("Ordinary restart cannot select an application update.")
                    result = await self._restart_ordinary_application_locked(guide_id, deployment_id)
                elif action == "apply-update" and self.application_replacement_selection is not None:
                    selection_owner = self.application_replacement_selection
                    if not callable(selection_owner):
                        raise DraftConflict("Preserve the original installed replacement selection owner.")
                    selected = await await_validation(selection_owner,
                        guide_id, deployment_id, update_id)
                    if self.application_replacement_selection != selection_owner:
                        raise DraftConflict("The installed replacement selection owner changed during admission.")
                    result = await self._action_locked(guide_id, deployment_id, action, update_id,
                        application_startup=selected)
                else:
                    result = await self._action_locked(guide_id, deployment_id, action, update_id,
                        **({"destination_owner": destination_owner} if destination_owner is not None else {}))
                    if action == "stop" and (destination_owner is None
                            or result.get("execution_mode") != "background" or not result.get("service")):
                        await self._continue_application_setup(guide_id, deployment_id)
            except asyncio.CancelledError:
                # Cancellation, a crash or storage uncertainty leaves pending.
                # It is not evidence that the action was refused before effects.
                raise
            except Exception:
                # The host coroutine has finished raising. Existing scheduled
                # workers may continue; their own journal remains authoritative.
                self._settle_action_request(binding, "error", resumed=continuing)
                raise
            if destination_owner is not None:
                self._destination_control_owner(destination_owner, guide_id, deployment_id)
            value = self._settle_action_request(binding, "returned", resumed=continuing)
            return {**result, "action_request": value}

    async def _continue_application_setup(self, guide_id, deployment_id):
        """Action-owned source retention, never invoked by GET or replay."""
        owner = self.application_setup_continuation
        if owner is None:
            return
        if not self.lock.locked() or not callable(owner):
            raise DraftConflict("Preserve the original locked application continuation owner.")
        await owner(self, guide_id, deployment_id)
        if self.application_setup_continuation != owner:
            raise DraftConflict("The original application continuation owner changed during its action.")

    def _ordinary_restart_selected(self, guide_id, deployment_id):
        """Route the checked installed source; files do not select its runtime."""
        from .deployment_updates import checked_package
        from .ordinary_application_source import ordinary_application
        from .runtime_selection import MetaToolkitRuntimeTarget, parse_candidate_selection
        directory = self._directory(guide_id, deployment_id)
        receipt = read_receipt(directory)
        if (receipt.get("source_kind", "application") != "application"
                or receipt.get("execution_mode") != "interactive" or receipt.get("service")
                or receipt.get("runtime_candidate") is None
                or type(parse_candidate_selection(receipt.get("runtime_candidate"))) is not MetaToolkitRuntimeTarget):
            return False
        _, package = checked_package(directory, "package.zip", receipt["package_sha256"])
        if "application/runtime-selection.json" not in package:
            return False
        return ordinary_application(json.loads(package["blueprint.json"][0]))

    async def _restart_ordinary_application_locked(self, guide_id, deployment_id):
        """One claimed Restart joins Stop before selecting the original source."""
        import inspect
        from .connect_installer_caller import _same_json_value
        from .deployment_application_bootstrap import _shape
        selection_owner = self.application_setup_selection
        continuation_owner = self.application_setup_continuation
        if (not self.lock.locked() or not callable(selection_owner) or not callable(continuation_owner)
                or inspect.iscoroutinefunction(selection_owner) or inspect.isasyncgenfunction(selection_owner)
                or inspect.iscoroutinefunction(getattr(selection_owner, "__call__", None))
                or inspect.isasyncgenfunction(getattr(selection_owner, "__call__", None))):
            raise DraftConflict("Ordinary restart requires the original synchronous source selection and continuation owners.")

        def select():
            if (self.application_setup_selection != selection_owner
                    or self.application_setup_continuation != continuation_owner):
                raise DraftConflict("The original ordinary restart source owner changed.")
            value = selection_owner(guide_id, deployment_id)
            if inspect.isawaitable(value) or inspect.isasyncgen(value):
                if inspect.iscoroutine(value):
                    value.close()
                raise DraftConflict("The original ordinary startup supplier must complete synchronously.")
            if _shape(value) != "ordinary":
                raise DraftConflict("Ordinary restart cannot substitute Fresh or Restore inputs.")
            if (self.application_setup_selection != selection_owner
                    or self.application_setup_continuation != continuation_owner):
                raise DraftConflict("The original ordinary restart source owner changed during selection.")
            return deepcopy(value)

        stopped = await self._action_locked(guide_id, deployment_id, "stop")
        if stopped.get("status") != "stopped":
            raise DraftConflict("The original ordinary runtime did not confirm cleanup before restart.")
        if (self.application_setup_selection != selection_owner
                or self.application_setup_continuation != continuation_owner):
            raise DraftConflict("The original ordinary restart source owner changed during Stop.")
        await self._continue_application_setup(guide_id, deployment_id)
        selected = select()
        result = await self._start_application_locked(guide_id, deployment_id, application_startup=selected)
        if not _same_json_value(select(), selected):
            raise DraftConflict("The original ordinary startup selection changed during restart admission.")
        return result

    async def start_application(self, guide_id, deployment_id, *, application_startup, owner_source=None):
        """Admit supplied native Fresh/Restore inputs through the stopped owner.

        This private entry is not a browser action and does not prepare native
        inputs, infer policy acceptance or grant replay of an uncertain attempt.
        """
        if not isinstance(application_startup, dict):
            raise DraftConflict("The application owner must supply its actual Fresh or Restore selection.")
        selected = deepcopy(application_startup)
        async with self.lock:
            return await self._start_application_locked(guide_id, deployment_id, application_startup=selected,
                **({"owner_source": owner_source} if owner_source is not None else {}))

    async def prepare_application(self, guide_id, deployment_id, **selection):
        """Private owner bridge to draft Fresh inputs, without preparing or starting."""
        from .deployment_application_preparation import prepare_application
        async with self.lock:
            return await prepare_application(self, guide_id, deployment_id, **selection)

    async def cleanup_application_preparation(self, guide_id, deployment_id, *, supervisor):
        """Join only this original preparation's retained child and lease."""
        from .deployment_application_preparation import cleanup_application_preparation
        async with self.lock:
            await cleanup_application_preparation(self, guide_id, deployment_id, supervisor=supervisor)

    def _selected_service_restore(self, selection_owner, guide_id, deployment_id):
        """Read only the retained synchronous startup supplier for this action."""
        import inspect
        from .deployment_application_bootstrap import _shape
        if (not callable(selection_owner) or self.application_setup_selection != selection_owner
                or inspect.iscoroutinefunction(selection_owner) or inspect.isasyncgenfunction(selection_owner)
                or inspect.iscoroutinefunction(getattr(selection_owner, "__call__", None))
                or inspect.isasyncgenfunction(getattr(selection_owner, "__call__", None))):
            raise DraftConflict("The original synchronous application Restore supplier is unavailable.")
        selected = selection_owner(guide_id, deployment_id)
        if inspect.isawaitable(selected) or inspect.isasyncgen(selected):
            if inspect.iscoroutine(selected):
                selected.close()
            raise DraftConflict("The original application Restore supplier must complete synchronously.")
        if self.application_setup_selection != selection_owner:
            raise DraftConflict("The original application startup owner changed during selection.")
        if _shape(selected) != "restore":
            raise DraftConflict("Background startup requires completed Restore inputs; Fresh is never replayed.")
        return deepcopy(selected)

    async def restored_service_capability(self, guide_id, deployment_id, *, application_startup,
                                          require_owner_current):
        """Read stopped source/Restore/manager eligibility without publishing or starting."""
        async with self.lock:
            return await self._restored_service_capability_locked(guide_id, deployment_id,
                application_startup=application_startup, require_owner_current=require_owner_current)

    async def _restored_service_capability_locked(self, guide_id, deployment_id, *, application_startup,
                                                  require_owner_current, source_custody=None):
        """Read stopped source/Restore/manager eligibility without publishing or starting."""
        unavailable = {"available": False,
            "detail": "Background service startup is unavailable for these original stopped inputs."}
        try:
            if not self.lock.locked():
                raise DraftConflict("Background availability requires its original deployment lock.")
            if source_custody is not None:
                from .remote_product_destination_owner import StoppedProductSource
                if type(source_custody) is not StoppedProductSource:
                    raise DraftConflict("Background availability requires its actual installed source owner.")
                source_custody.require_owner_current()
            from .deployment_application_bootstrap import (
                _shape, _observe_inputs, require_restore_history_current, FRESH_ENTRY_NAME)
            from .connect_application_update_startup import _file_identity
            from .deployment_supervisor import pending_custody
            from .deployment_updates import require_replacement_history_settled
            if not callable(require_owner_current) or _shape(application_startup) != "restore":
                return unavailable

            def owner_current():
                if source_custody is not None:
                    source_custody.require_owner_current()
                import inspect
                result = require_owner_current()
                if result is False or inspect.isawaitable(result) or inspect.isasyncgen(result):
                    if inspect.iscoroutine(result):
                        result.close()
                    raise DraftConflict("Background availability requires synchronous original-owner currentness.")

            async def validate(function, *args, **kwargs):
                # Application callbacks belong to this original task, while
                # bounded filesystem validation belongs to the joined worker.
                owner_current()
                if source_custody is None:
                    result = await await_validation(function, *args, **kwargs)
                else:
                    from .product_runtime_custody import await_product_validation
                    result = await await_product_validation(source_custody, function, *args, **kwargs)
                owner_current()
                return result

            if deployment_id in self._application_preparations:
                return unavailable
            directory, receipt, stopped_current = self._stopped_application_owner(guide_id, deployment_id)
            selected = deepcopy(application_startup)
            manager = self.service_manager
            if await validate(pending_custody, directory, deepcopy(receipt)):
                return unavailable
            snapshot = await validate(self._installed_configuration, guide_id, deployment_id,
                **({"source_custody": source_custody} if source_custody is not None else {}))
            destination = snapshot[2]
            if source_custody is not None:
                from .product_runtime_custody import require_product_installation
                await validate(require_product_installation, source_custody, receipt)
                source_custody._lease.require_current()
                if source_custody._lease.root != destination:
                    raise DraftConflict("Background availability changed its original project lease.")

            def observe():
                stopped_current()
                require_replacement_history_settled(directory)
                observed = _observe_inputs(directory, read_receipt(directory), selected,
                    **({"source_custody": source_custody} if source_custody is not None else {}))
                entry = directory / FRESH_ENTRY_NAME
                history = ((entry, _file_identity(entry)),) if entry.exists() or entry.is_symlink() else ()
                require_restore_history_current(directory, selected)
                if history:
                    from dataclasses import replace
                    observed = replace(observed, files=(*observed.files, *history))
                configuration = private_configuration_snapshot(destination)[1]
                stopped_current()
                observed.require_current()
                return observed, configuration, bool(history)

            observed, configuration, had_entry = await validate(observe)
            capability = await manager.capability()

            def current():
                stopped_current()
                observed.require_current()
                require_restore_history_current(directory, selected)
                entry = directory / FRESH_ENTRY_NAME
                if bool(entry.exists() or entry.is_symlink()) != had_entry:
                    raise DraftConflict("The original setup history changed while reading background availability.")
                if private_configuration_snapshot(destination)[1] != configuration:
                    raise DraftConflict("Private inputs changed while reading background availability.")
                if self.service_manager is not manager:
                    raise DraftConflict("The original service manager owner changed.")

            await validate(current)
            if not isinstance(capability, dict) or capability.get("available") is not True:
                return unavailable
            refusal = await validate(self._background_folder_refusal, capability, destination, setup_card=True)
            if refusal is not None:
                return {"available": False, "detail": refusal}
            result = {"available": True}
            persistence = capability.get("persistence")
            if isinstance(persistence, str) and persistence.strip() and len(persistence) <= 500:
                result["persistence"] = persistence
            return result
        except (OSError, ValueError, RuntimeError, DraftConflict, LookupError):
            return unavailable

    async def start_restored_service(self, guide_id, deployment_id, *, application_startup,
                                     require_owner_current, owner_source=None):
        """Trusted owner entry; normal journal dispatch can reuse the locked join."""
        async with self.lock:
            return await self._start_restored_service_locked(guide_id, deployment_id,
                application_startup=application_startup, require_owner_current=require_owner_current,
                **({"owner_source": owner_source} if owner_source is not None else {}))

    async def _start_restored_service_locked(self, guide_id, deployment_id, *, application_startup,
                                     require_owner_current, owner_source=None, destination_owner=None):
        """Convert one stopped owner to a frozen Restore-only user service."""
        import inspect
        from .deployment_application_bootstrap import (
            bootstrap_installation_binding, observe_bootstrap_admission,
            retain_bootstrap_admission, require_bootstrap_history_settled, _shape)
        from .deployment_service import (prepare_service, stop_background, service_binding, verify_service,
            retained_bootstrap_files_current)
        from .deployment_supervisor import pending_custody
        from .deployment_updates import checked_package, require_replacement_history_settled
        from .product_installed_source import open_installed_product_source
        from .product_runtime_custody import await_product_validation
        if not callable(require_owner_current) or _shape(application_startup) != "restore":
            raise DraftConflict("Service startup requires the original owner's completed Restore selection.")
        selected = deepcopy(application_startup)
        selected_host = selected_source = source_selection = None
        selector = self.application_host_selection
        if owner_source is None and selector is not None:
            from .forge_application_host import ForgeApplicationHost
            from .product_runtime_custody import product_source_custody
            if not callable(selector):
                raise DraftConflict("Preserve the original application host source selector.")
            selected_host = selector(guide_id, deployment_id)
            if selected_host is not None and type(selected_host) is not ForgeApplicationHost:
                raise DraftConflict("The service source selector must retain its actual original application host.")
            if selected_host is not None and selected_host._product_source_custody is not None:
                # A closed carrier retains independently selected bytes only.
                # Its old lease and callback cannot authorize this operation.
                selected_source = product_source_custody(selected_host._product_source_custody)
                owner_source = selected_source._owner_source
                source_selection = (owner_source, deepcopy(selected_source._context),
                    deepcopy(selected_source.origin), selected_source._root)

        def owner_current():
            result = require_owner_current()
            if result is False or inspect.isawaitable(result) or inspect.isasyncgen(result):
                if inspect.iscoroutine(result):
                    result.close()
                raise DraftConflict("The original service owner requires a successful synchronous currentness check.")
            if selected_host is not None:
                if (self.application_host_selection != selector
                        or selector(guide_id, deployment_id) is not selected_host
                        or selected_host._product_source_custody is not selected_source):
                    raise DraftConflict("The original service host source owner changed during admission.")
                if selected_source is not None and (selected_source._owner_source,
                        selected_source._context, selected_source.origin, selected_source._root) != source_selection:
                    raise DraftConflict("The original service host source selection changed during admission.")

        if deployment_id in self._application_preparations:
            raise DraftConflict("Settle the original application preparation custody before service startup.")
        directory, receipt, require_stopped = self._stopped_application_owner(guide_id, deployment_id)
        binding = bootstrap_installation_binding(receipt)

        def current():
            require_stopped()
            owner_current()

        current()
        if await await_validation(pending_custody, directory, deepcopy(receipt)):
            raise DraftConflict("Complete the original installation support custody before service startup.")
        current()
        capability = await self.service_manager.capability()
        if not capability["available"]:
            raise DraftConflict("The logged-in user's service manager is unavailable. No service was installed.")
        current()
        if destination_owner is None:
            snapshot = await await_validation(self._installed_configuration, guide_id, deployment_id)
            _, _, destination, definition, _, _ = snapshot
        else:
            self._destination_control_owner(destination_owner, guide_id, deployment_id)
            destination = owned_destination(receipt["destination"])
        current()
        refusal = await await_validation(self._background_folder_refusal, capability, destination, setup_card=True)
        if refusal is not None:
            raise DraftConflict(refusal)
        current()
        service = None
        service_identity = None

        def file_service_current():
            now_receipt = read_receipt(directory)
            if (self._directory(guide_id) != directory
                    or bootstrap_installation_binding(now_receipt) != binding
                    or service_binding(verify_service(directory, now_receipt)) != service_identity
                    or private_configuration_snapshot(destination)[1] != configuration):
                raise DraftConflict("The original service source, private inputs or frozen selection changed.")

        def source_current():
            # The caller's original callback never runs in a file worker. The
            # exact returned service owns the one permitted publication change.
            owner_current()
            if service is None:
                require_stopped()
            else:
                file_service_current()

        from .deployment_application_bootstrap import kept_native_inventory
        retained = await await_validation(kept_native_inventory, directory)
        with project_lock(destination) as lease:
            async with open_installed_product_source(directory, project_lease=lease,
                    require_current=source_current, owner_source=owner_source, retained=retained,
                    application_startup=selected) as source_custody:
                if destination_owner is not None:
                    self._destination_control_owner(destination_owner, guide_id, deployment_id)
                    if source_custody is None:
                        raise DraftConflict("Destination service admission lost its original installed source.")
                    snapshot = await await_product_validation(source_custody, self._installed_configuration,
                        guide_id, deployment_id, source_custody=source_custody)
                    _, _, checked_destination, definition, _, _ = snapshot
                    if checked_destination != destination:
                        raise DraftConflict("Destination service admission changed its original project root.")
                    current()
                if source_selection is not None and (source_custody is None
                        or (source_custody._owner_source, source_custody._context,
                            source_custody.origin, source_custody._root) != source_selection):
                    raise DraftConflict("The original service host source differs from this installed owner.")
                def retain():
                    lease.require_current()
                    require_stopped()
                    require_replacement_history_settled(directory)
                    _, package = checked_package(directory, "package.zip", receipt["package_sha256"])
                    validate_installed_console_settings(package, destination)
                    configuration = private_configuration_snapshot(destination)[1]
                    pin, observed = retain_bootstrap_admission(directory, receipt, selected,
                        **({"source_custody": source_custody} if source_custody is not None else {}))
                    require_bootstrap_history_settled(directory, pin)
                    require_stopped()
                    lease.require_current()
                    return pin, observed, configuration

                current()
                pin, observed, configuration = await await_product_validation(source_custody, retain)
                current()
                lease.require_current()
                observed.require_current()
                if private_configuration_snapshot(destination)[1] != configuration:
                    raise DraftConflict("Private settings changed before service admission.")
                require_available_console_port(installed_console_port(destination, definition["port"]))
                current()
                if source_custody is not None:
                    source_custody.require_owner_current()
                # The original source is still held while the trusted launcher
                # retains its independent digest. No native child runs here.
                service = prepare_service(directory, read_receipt(directory), bootstrap_selection=pin,
                    **({"source_custody": source_custody} if source_custody is not None else {}))
                service_identity = deepcopy(service_binding(service))
                source_current()
                def observe_published():
                    _, observed = observe_bootstrap_admission(directory, pin,
                        **({"source_custody": source_custody} if source_custody is not None else {}))
                    return retained_bootstrap_files_current(observed,
                        **({"source_custody": source_custody} if source_custody is not None else {}))
                files_current = await await_product_validation(source_custody, observe_published)
                source_current()
        # Both original temporary scopes are closed before the manager may
        # launch the service worker and its child's own real project lease.
        # Post-manager reads retain only original file observations, not a
        # closed parent carrier or another competing project lease.
        async def service_current():
            owner_current()
            await await_validation(file_service_current)
            owner_current()
            await await_validation(files_current)
            owner_current()
            await await_validation(file_service_current)
            owner_current()

        await service_current()
        journal(directory, "Starting the owned service with its original completed Restore selection.",
            status="starting", phase="service", runtime_pid=None, console_url=None,
            bootstrap=None, startup_attempt_id=None, startup_console_health=None,
            startup_port_observation=None, health="unknown", health_checked_at=None, process_live=False)
        await self._admit_restored_service_manager(directory, service, service_current)
        # Existing status requires actual manager/native readiness; manager
        # admission alone is not a running application claim.
        return await self.status(guide_id, _joined_file_checks=True,
            **({"destination_owner": destination_owner} if destination_owner is not None else {}))

    async def _admit_restored_service_manager(self, directory, service, require_current):
        """Join this Restore unit's manager admission and original Stop on failure."""
        from .deployment_service import stop_background
        pending = asyncio.create_task(self.service_manager.start(service))
        cancellation = failure = None
        while True:
            try:
                await asyncio.shield(pending)
                break
            except asyncio.CancelledError as error:
                if pending.cancelled():
                    failure = error
                    break
                if cancellation is None:
                    cancellation = error
                    pending.cancel()
            except BaseException as error:
                failure = error
                break
        if cancellation is None and failure is None:
            try:
                import inspect
                if inspect.iscoroutinefunction(require_current):
                    await require_current()
                else:
                    await await_validation(require_current)
            except asyncio.CancelledError as error:
                cancellation = error
            except BaseException as error:
                failure = error
        if cancellation is not None or failure is not None:
            # Join the same admission before stopping its exact unit. A
            # cancelled waiter cannot abandon a newly admitted service.
            cleanup = asyncio.create_task(stop_background(directory, self.service_manager))
            while True:
                try:
                    result = await asyncio.shield(cleanup)
                    if result.get("status") != "stopped":
                        raise DraftConflict("The original service cleanup remains unconfirmed.")
                    break
                except asyncio.CancelledError as error:
                    if cleanup.cancelled():
                        raise
                    cancellation = cancellation or error
                except BaseException as error:
                    raise BaseExceptionGroup("Service admission and its cleanup failed.",
                        [item for item in (cancellation, failure, error) if item is not None]) from None
            if cancellation is not None:
                raise cancellation from failure
            raise failure

    def _stopped_application_owner(self, guide_id, deployment_id):
        """Share the existing stopped source fence across private owner entries."""
        from .deployment_application_bootstrap import bootstrap_installation_binding
        directory = self._directory(guide_id)
        receipt = read_receipt(directory)

        def require_stopped_owner():
            current = read_receipt(directory)
            process = self.processes.get(deployment_id)
            if (self._directory(guide_id) != directory or directory.name != deployment_id
                    or current.get("guide_id") != guide_id
                    or current.get("deployment_id") != deployment_id
                    or bootstrap_installation_binding(current) != bootstrap_installation_binding(receipt)
                    or current.get("source_kind", "application") != "application"
                    or current.get("execution_mode") != "interactive" or current.get("service")
                    or current.get("status") != "stopped" or deployment_id in self.tasks
                    or process is not None and process.returncode is None
                    or current.get("process_live") is True or current.get("runtime_pid") is not None
                    or self._configuration_binding(current) != self._configuration_binding(receipt)):
                raise DraftConflict("Bootstrap requires this exact stopped interactive application owner.")
            if ((current.get("update") or {}).get("status") in {
                    "snapshotting", "applying", "verifying", "restoring", "recovering-candidate",
                    "recovering-current", "recovery-needed", "restored-stopped", "candidate-restored-stopped"}):
                raise DraftConflict("Complete the original update recovery before admitting Fresh or Restore startup.")
            if current.get("interactive_lifecycle_id") is not None and not interactive_cleanup_matches(current):
                raise DraftConflict("The original runtime cleanup remains unconfirmed. No bootstrap was admitted.")

        require_stopped_owner()
        return directory, receipt, require_stopped_owner

    async def _start_application_locked(self, guide_id, deployment_id, *, application_startup, owner_source=None,
                                        destination_owner=None):
        """Existing startup admission, called with the original action lock held."""
        if deployment_id in self._application_preparations:
            raise DraftConflict("Settle the original application preparation custody before startup.")
        if not isinstance(application_startup, dict):
            raise DraftConflict("The application owner must supply its actual Fresh or Restore selection.")
        selected = deepcopy(application_startup)
        from .deployment_application_bootstrap import (
            bootstrap_installation_binding, retain_bootstrap_admission,
            require_bootstrap_history_settled)
        from .deployment_supervisor import pending_custody
        from .deployment_updates import checked_package, require_replacement_history_settled
        directory, receipt, receipt_current = self._stopped_application_owner(guide_id, deployment_id)

        def require_stopped_owner():
            # The original factory/worker and the canonical stopped receipt
            # stay one calling-task fence across every joined file await.
            if destination_owner is not None:
                self._destination_control_owner(destination_owner, guide_id, deployment_id)
            receipt_current()

        require_stopped_owner()
        if await await_validation(pending_custody, directory, deepcopy(receipt)):
            raise DraftConflict("Complete the original installation support custody before bootstrap startup.")
        require_stopped_owner()
        if destination_owner is None:
            snapshot = await await_validation(self._installed_configuration, guide_id, deployment_id)
            _, _, destination, definition, _, _ = snapshot
        else:
            self._destination_control_owner(destination_owner, guide_id, deployment_id)
            destination = owned_destination(receipt["destination"])
        require_stopped_owner()
        from .deployment_application_bootstrap import kept_native_inventory
        from .product_installed_source import open_installed_product_source
        from .product_runtime_custody import await_product_validation
        retained = await await_validation(kept_native_inventory, directory)
        require_stopped_owner()
        with project_lock(destination) as lease:
            # Close this parent source scope before the child takes its own
            # project lease. Only the original private caller selects inputs.
            async with open_installed_product_source(directory, project_lease=lease,
                    require_current=require_stopped_owner, application_startup=selected,
                    owner_source=owner_source, retained=retained) as source_custody:
                if destination_owner is not None:
                    self._destination_control_owner(destination_owner, guide_id, deployment_id)
                    if source_custody is None:
                        raise DraftConflict("Destination startup lost its original installed source.")
                    snapshot = await await_product_validation(source_custody, self._installed_configuration,
                        guide_id, deployment_id, source_custody=source_custody)
                    _, _, checked_destination, definition, _, _ = snapshot
                    if checked_destination != destination:
                        raise DraftConflict("Destination startup changed its original project root.")
                    require_stopped_owner()
                def retain():
                    lease.require_current()
                    require_replacement_history_settled(directory)
                    _, package = checked_package(directory, "package.zip", receipt["package_sha256"])
                    validate_installed_console_settings(package, destination)
                    configuration = private_configuration_snapshot(destination)[1]
                    pin, observation = retain_bootstrap_admission(directory, receipt, selected,
                        **({"source_custody": source_custody} if source_custody is not None else {}))
                    require_bootstrap_history_settled(directory, pin)
                    lease.require_current()
                    return pin, observation, configuration

                pin, observation, configuration = await await_product_validation(source_custody, retain)
                require_stopped_owner()
                lease.require_current()
                observation.require_current()
                if private_configuration_snapshot(destination)[1] != configuration:
                    raise DraftConflict("Private application settings changed during bootstrap admission.")
                require_available_console_port(installed_console_port(destination, definition["port"]))
                require_stopped_owner()
                lease.require_current()
                observation.require_current()
                require_bootstrap_history_settled(directory, pin)
                if private_configuration_snapshot(destination)[1] != configuration:
                    raise DraftConflict("Private application settings changed before bootstrap queueing.")
                bootstrap_owner_source = source_custody._owner_source if source_custody is not None else None
            require_stopped_owner()
            lease.require_current()
            observation.require_current()
            if private_configuration_snapshot(destination)[1] != configuration:
                raise DraftConflict("Private application settings changed while closing parent source custody.")
            journal(directory, "Application bootstrap admitted for its original checked source.",
                status="queued", phase="start", console_url=None, runtime_pid=None, bootstrap=None,
                startup_attempt_id=None, startup_console_health=None, startup_port_observation=None,
                health="unknown", health_checked_at=None, process_live=False)
        if destination_owner is not None:
            self._destination_control_owner(destination_owner, guide_id, deployment_id)
        self._launch(directory, "start", bootstrap_selection=pin,
            **({"bootstrap_owner_source": bootstrap_owner_source} if bootstrap_owner_source is not None else {}))
        return await self.status(guide_id, _joined_file_checks=True,
            **({"destination_owner": destination_owner} if destination_owner is not None else {}))

    async def apply_replacement(self, guide_id, deployment_id, update_id, *, application_startup):
        """Admit actual native inputs from the stopped application's owner.

        This private Python entry is not exposed by the browser action schema.
        It does not prepare inputs, accept policy or infer a replay decision.
        """
        if not isinstance(application_startup, dict):
            raise DraftConflict("The application owner must supply its actual replacement selection.")
        selected = deepcopy(application_startup)
        async with self.lock:
            return await self._action_locked(guide_id, deployment_id, "apply-update", update_id,
                application_startup=selected)

    async def _action_locked(self, guide_id, deployment_id, action, update_id=None, *, application_startup=None,
                             destination_owner=None):
        directory = self._directory(guide_id)
        if destination_owner is not None:
            self._destination_control_owner(destination_owner, guide_id, deployment_id)
            if action not in {"stop", "restart"} or update_id is not None or application_startup is not None:
                raise DraftConflict("Destination lifecycle cannot select origin source updates or installation retry.")
        if action in {"restart", "retry"}:
            await self._require_installed_service_readable(directory, deployment_id)
        receipt = read_receipt(directory) if application_startup is not None else await self.status(guide_id,
            **({"destination_owner": destination_owner} if destination_owner is not None else {}))
        if receipt["deployment_id"] != deployment_id:
            raise DraftConflict("A newer deployment is now selected.")
        if action not in {"stop", "restart", "retry", "apply-update", "rollback", "recover-current"}:
            raise ValueError("Choose a supported deployment action.")
        if application_startup is not None and (action != "apply-update"
                or receipt.get("execution_mode", "interactive") != "interactive" or receipt.get("service")
                or receipt.get("status") != "stopped"):
            raise DraftConflict("Replacement admission requires the stopped application's interactive update owner.")
        if application_startup is not None:
            from .deployment_supervisor import pending_custody
            from .deployment_updates import require_inspection_binding_current
            if await await_validation(pending_custody, directory, deepcopy(receipt)):
                raise DraftConflict("Complete the exact installation support custody update before replacement admission.")
            require_inspection_binding_current(directory, receipt)
        if (destination_owner is not None and action == "restart"
                and receipt.get("execution_mode", "interactive") == "interactive" and not receipt.get("service")):
            # The same UUID joins the original Stop, then selects only completed
            # Restore. It never replaces a live child or repeats entered Fresh.
            stopped = await self._action_locked(guide_id, deployment_id, "stop",
                destination_owner=destination_owner)
            if stopped.get("status") != "stopped":
                raise DraftConflict("The original destination runtime did not confirm cleanup before restart.")
            await self._continue_application_setup(guide_id, deployment_id)
            selected = self._selected_service_restore(self.application_setup_selection, guide_id, deployment_id)
            if destination_owner._selected_startup(guide_id, deployment_id) != selected:
                raise DraftConflict("The original destination Restore selection changed before restart.")
            return await destination_owner.start_application(self, guide_id, deployment_id)
        if action != "stop":
            from .deployment_application_bootstrap import (require_bootstrap_history_settled,
                require_update_bootstrap_history, restored_service)
            bootstrap_selection = None
            if action == "restart" and restored_service(receipt):
                from .deployment_service import verify_service
                bootstrap_selection = verify_service(directory, receipt)["bootstrap_selection"]
            if action in {"apply-update", "rollback", "recover-current"}:
                # After setup, these admit only the completed Restore recorded at
                # Stage, read from the private journal projection. Apply also
                # requires the owner's current publication. Rollback and recovery
                # may read their own unfinished transaction as lineage.
                from .deployment_application_bootstrap import SETUP_RESTORE_CHANGED, UPDATE_STAGED_BEFORE_SETUP
                from .deployment_updates import setup_restore_pin, update_binding
                selected = read_receipt(directory).get("update") or {}
                recorded = setup_restore_pin(directory, selected)
                if (action == "apply-update" and recorded is not None
                        and self._setup_update_restore(receipt["guide_id"], deployment_id) != recorded):
                    raise DraftConflict(SETUP_RESTORE_CHANGED)
                if (action == "apply-update" and recorded is None
                        and self._setup_update_restore(receipt["guide_id"], deployment_id) is not None):
                    # Staged before the first setup, so Stage recorded no
                    # completed Restore. That setup is complete now: the
                    # update needs a new Stage, not the Fresh refusal below.
                    raise DraftConflict(UPDATE_STAGED_BEFORE_SETUP)
                require_update_bootstrap_history(directory, recorded,
                    in_flight=update_binding(selected) if action != "apply-update"
                        and selected.get("update_id") == update_id else None)
                if (action == "apply-update" and application_startup is None
                        and await await_validation(self._product_before_setup, guide_id, deployment_id)):
                    from .deployment_updates import FIRST_SETUP_UPDATE
                    raise DraftConflict(FIRST_SETUP_UPDATE)
            else:
                require_bootstrap_history_settled(directory, bootstrap_selection)
        if receipt.get("source_kind") == "native-team" and action in {"apply-update", "rollback", "recover-current"}:
            raise DraftConflict("Agent-team source updates are not supported here. Its installed source and state are preserved.")
        if (action == "stop" and receipt.get("execution_mode", "interactive") == "interactive"
                and not receipt.get("service") and receipt["status"] in {"failed", "interrupted"}):
            return await self._stop_failed_interactive(guide_id, deployment_id)
        if action in {"apply-update", "rollback", "recover-current"}:
            from .deployment_updates import (CURRENT_RECOVERY_STATES, inspect_transaction,
                                              require_inspection_current, require_inspection_binding_current,
                                              retain_update_admission)
            require_admission_current = (require_inspection_binding_current if application_startup is not None
                else require_inspection_current)
            admitted_receipt, _, update = await await_validation(inspect_transaction, directory, update_id)
            require_admission_current(directory, admitted_receipt, update)
            if self._directory(guide_id) != directory:
                raise DraftConflict("A newer deployment is now selected.")
            background = receipt.get("execution_mode") == "background"
            if background:
                from .deployment_service import require_stopped_service, require_candidate_update_protocol
                if not receipt.get("update_capabilities", {}).get("background_activation_supported"):
                    raise DraftConflict(receipt.get("update_capabilities", {}).get("reason") or "The owned background update adapter could not be verified.")
                if receipt["status"] != "stopped":
                    raise DraftConflict("Stop and verify this exact owned background service before changing its installed source.")
                stopped_service = await require_stopped_service(directory, self.service_manager)
                if action != "rollback":
                    require_candidate_update_protocol(stopped_service, update)
            if receipt["status"] in ACTIVE or deployment_id in self.tasks:
                raise DraftConflict("Stop and verify this owned runtime before changing installed source.")
            if action in {"apply-update", "recover-current"}:
                eligible = {"staged", "snapshotting"} if action == "apply-update" else CURRENT_RECOVERY_STATES
                if update["status"] not in eligible or update["blockers"]:
                    raise DraftConflict("This staged package has update requirements that must be resolved first.")
                if action == "apply-update":
                    # The worker moves the current runtime aside before it
                    # installs the selected product with this interpreter, so
                    # refuse here a computer that product cannot run on.
                    refusal = staged_runtime_refusal(update)
                    if refusal is not None:
                        raise DraftConflict(refusal)
                checked = await self.implementation.status(guide_id)
                require_accepted(checked)
                latest = await self._guide(guide_id)
                if (not current_version(checked, latest) or latest.get("operation", {}).get("status") == "running"
                        or checked["build_id"] != update["to_build_id"] or not checked.get("ready_to_package")
                        or not checked.get("current") or checked.get("source_digest") != update["source_digest"]
                        or proposal_hash(latest.get("proposal") or latest.get("blueprint")) != update["blueprint_hash"]
                        or update.get("brief_hash") and update["brief_hash"] != fingerprint(latest.get("brief") or {})
                        or not update.get("brief_hash") and latest["version"] != update["guide_version"]):
                    raise DraftConflict("The staged application is no longer the current accepted build.")
                # Check the exact journal again after awaited authority
                # checks; read threads never reconcile the public receipt.
                _, _, current_update = await await_validation(inspect_transaction, directory, update_id)
                if current_update != update:
                    raise DraftConflict("The update changed while admitting this action. Review its current receipt.")
            elif update["status"] not in {"completed", "applying", "verifying", "restoring", "recovering-candidate", "recovering-current", "candidate-restored-stopped", "recovery-needed"} or not update["rollback_available"]:
                raise DraftConflict("There is no previously activated package to restore for this update.")
            if background:
                await require_stopped_service(directory, self.service_manager)
                _, _, final_update = await await_validation(inspect_transaction, directory, update_id)
                if final_update != update:
                    raise DraftConflict("The exact background update changed before admission.")
            if background:
                final_service = await require_stopped_service(directory, self.service_manager)
                if action != "rollback":
                    require_candidate_update_protocol(final_service, final_update)
            # Finish service observations before the final accepted-build and
            # guide checks. Their results cannot admit a superseded build.
            if action in {"apply-update", "recover-current"}:
                checked = await self.implementation.status(guide_id)
                require_accepted(checked)
                latest = await self._guide(guide_id)
                if (latest.get("operation", {}).get("status") == "running"
                        or self.implementation.selected_build_id(guide_id) != update["to_build_id"]
                        or checked.get("build_id") != update["to_build_id"]
                        or checked.get("source_digest") != update["source_digest"]
                        or not checked.get("current") or not checked.get("ready_to_package")
                        or not current_version(checked, latest)
                        or proposal_hash(latest.get("proposal") or latest.get("blueprint")) != update["blueprint_hash"]
                        or update.get("brief_hash") and update["brief_hash"] != fingerprint(latest.get("brief") or {})
                        or not update.get("brief_hash") and latest["version"] != update["guide_version"]):
                    raise DraftConflict("The accepted application changed before update admission.")
            require_admission_current(directory, admitted_receipt, update)
            if (self._directory(guide_id) != directory or deployment_id in self.tasks
                    or self.processes.get(deployment_id) is not None
                        and self.processes[deployment_id].returncode is None
                    or read_receipt(directory).get("status") in ACTIVE):
                raise DraftConflict("The selected installation or runtime owner changed before update admission.")
            mode = {"apply-update": "update", "rollback": "rollback", "recover-current": "recover-current"}[action]
            if application_startup is None:
                selection = retain_update_admission(directory, update, mode)
            else:
                from .deployment_updates import retain_replacement_update_admission
                selection, observation = await await_validation(retain_replacement_update_admission,
                    directory, deepcopy(update), mode, application_startup=application_startup)
                # Preserve the original observation while accepted design/build
                # checks await. A later observation cannot replace its inputs.
                observation.require_current()
                checked = await self.implementation.status(guide_id)
                require_accepted(checked)
                latest = await self._guide(guide_id)
                if (latest.get("operation", {}).get("status") == "running"
                        or self.implementation.selected_build_id(guide_id) != update["to_build_id"]
                        or checked.get("build_id") != update["to_build_id"]
                        or checked.get("source_digest") != update["source_digest"]
                        or not checked.get("current") or not checked.get("ready_to_package")
                        or not current_version(checked, latest)
                        or proposal_hash(latest.get("proposal") or latest.get("blueprint")) != update["blueprint_hash"]
                        or update.get("brief_hash") and update["brief_hash"] != fingerprint(latest.get("brief") or {})
                        or not update.get("brief_hash") and latest["version"] != update["guide_version"]):
                    raise DraftConflict("The accepted application changed during replacement admission.")
                require_admission_current(directory, admitted_receipt, update)
                if (self._directory(guide_id) != directory or deployment_id in self.tasks
                        or self.processes.get(deployment_id) is not None
                            and self.processes[deployment_id].returncode is None
                        or read_receipt(directory).get("status") != "stopped"):
                    raise DraftConflict("The selected installation or runtime changed during replacement admission.")
                observation.require_current()
            journal(directory, "Update action admitted. Waiting for the installed project's exclusive lease.",
                    status="queued", phase="update", runtime_pid=None, console_url=None, bootstrap=None,
                    startup_attempt_id=None, startup_console_health=None,
                    health="unknown", health_checked_at=None, process_live=False,
                    update_service_verification=None,
                    update_action={"action": action, "update_id": update_id})
            self._launch(directory, mode, update_selection=selection)
            return await self.status(guide_id, _joined_file_checks=True) if application_startup is not None else await self.status(guide_id)
        if action in {"restart", "retry"} and "runtime_candidate" in receipt:
            from .runtime_selection import MetaToolkitRuntimeTarget, parse_candidate_selection
            if type(parse_candidate_selection(receipt["runtime_candidate"])) is MetaToolkitRuntimeTarget:
                from .deployment_application_bootstrap import restored_service
                service_restart = action == "restart" and restored_service(receipt)
                if not service_restart and (action != "retry" or receipt.get("execution_mode") != "interactive" or receipt.get("service")):
                    raise DraftConflict(PRODUCT_CONFIGURE_REQUIRED)
                if not service_restart and receipt.get("update") is not None:
                    raise DraftConflict("Use the original selected product update or recovery owner; an initial installation retry cannot replace it.")
        if receipt.get("execution_mode") == "background" and receipt.get("service"):
            from .deployment_service import verify_service, stop_background
            if deployment_id in self.tasks:
                # Finish/cancel the installer-to-manager handoff first. It
                # must not start a unit after a concurrent user stop.
                task = self.tasks[deployment_id]
                task.cancel()
                await asyncio.gather(task, return_exceptions=True)
                self.tasks.pop(deployment_id, None)
                receipt = read_receipt(directory)
            service = verify_service(directory, receipt)
            if action == "stop":
                return await stop_background(directory, self.service_manager)
            if action == "restart":
                def destination_current():
                    if destination_owner is not None:
                        self._destination_control_owner(destination_owner, guide_id, deployment_id)

                destination_current()
                payload = (directory / "package.zip").read_bytes()
                if hashlib.sha256(payload).hexdigest() != receipt["package_sha256"]:
                    raise DraftConflict("The saved checked package changed. No process was started.")
                files = package_files(payload)
                destination = verify_destination(receipt, files)
                validate_installed_console_settings(files, destination)
                stopped = await stop_background(directory, self.service_manager)
                destination_current()
                if stopped["status"] != "stopped":
                    raise DraftConflict("The previous native runtime did not confirm shutdown. No replacement was started.")
                service_current = None
                if "bootstrap_selection" in service:
                    from .deployment_application_bootstrap import (bootstrap_installation_binding,
                        installed_bootstrap_source, observe_bootstrap_admission)
                    from .deployment_service import (service_binding, retained_bootstrap_owner_source_sha256,
                        retained_bootstrap_files_current)
                    from .product_runtime_custody import await_product_validation
                    binding = bootstrap_installation_binding(stopped)
                    service_identity = deepcopy(service_binding(service))
                    configuration = private_configuration_snapshot(destination)[1]
                    owner_sha256 = await await_validation(retained_bootstrap_owner_source_sha256,
                        directory, service)
                    destination_current()

                    def file_service_current():
                        current = read_receipt(directory)
                        if (self._directory(guide_id) != directory
                                or bootstrap_installation_binding(current) != binding
                                or service_binding(verify_service(directory, current)) != service_identity
                                or private_configuration_snapshot(destination)[1] != configuration):
                            raise DraftConflict("The original Restore service source, private inputs or frozen selection changed.")
                    def source_current():
                        # This callback remains on the original action task;
                        # the file-only reader below never calls the factory.
                        destination_current()
                        file_service_current()

                    # The frozen manifest independently selected this digest.
                    # Observe under one real pre-manager lease, then retain
                    # only the authenticated file facts for a live child's hold.
                    from .deployment_application_bootstrap import kept_native_inventory
                    retained = await await_validation(kept_native_inventory, directory)
                    destination_current()
                    async with installed_bootstrap_source(directory, service["bootstrap_selection"],
                            require_current=source_current, owner_source_sha256=owner_sha256,
                            retained=retained) as source_custody:
                        def observe_restart():
                            _, observed = observe_bootstrap_admission(directory, service["bootstrap_selection"],
                                **({"source_custody": source_custody} if source_custody is not None else {}))
                            return retained_bootstrap_files_current(observed,
                                **({"source_custody": source_custody} if source_custody is not None else {}))
                        files_current = await await_product_validation(source_custody, observe_restart)
                        destination_current()

                    async def service_current():
                        destination_current()
                        await await_validation(file_service_current)
                        destination_current()
                        await await_validation(files_current)
                        destination_current()
                        await await_validation(file_service_current)
                        destination_current()

                    await service_current()
                destination_current()
                require_available_console_port(installed_console_port(destination, json.loads(files["deployment.json"][0])["port"]))
                journal(directory, "Restart admitted for this owned background service.", status="starting",
                        runtime_pid=None, console_url=None, bootstrap=None,
                        # Preserve the old lifetime's confirmed cleanup until
                        # the actual manager worker publishes its successor;
                        # a successor that never registers then stops at once.
                        # Only reaching ready again re-records folder evidence.
                        service_folder_evidence=None,
                        startup_attempt_id=None, startup_console_health=None, startup_port_observation=None,
                        health="unknown", health_checked_at=None, process_live=False)
                destination_current()
                if service_current is not None:
                    await self._admit_restored_service_manager(directory, service, service_current)
                else:
                    await self.service_manager.start(service)
                    destination_current()
                return await self.status(guide_id,
                    **({"destination_owner": destination_owner} if destination_owner is not None else {}))
            stopped = await stop_background(directory, self.service_manager)
            if stopped["status"] != "stopped":
                raise DraftConflict("Stop and verify the existing service before retrying installation.")
            receipt = stopped
        if action == "stop":
            process = self.processes.get(deployment_id)
            task = self.tasks.get(deployment_id)
            if (receipt.get("status") == "stopped" and receipt.get("phase") == "configure"
                    and (receipt.get("runtime_candidate") or {}).get("format") == "forge-meta-toolkit-runtime-selection-v1"
                    and task is not None and process is not None and process.returncode is not None
                    and (self.worker_admissions.get(deployment_id) or {}).get("mode") == "install"):
                # An exited product worker may still be waiting for this
                # action lock before retaining inputs. Stop joins that exact
                # completion task without permitting its callback afterward.
                task.cancel()
                await asyncio.gather(task, return_exceptions=True)
                return await self.status(guide_id,
                    **({"destination_owner": destination_owner} if destination_owner is not None else {}))
            if receipt["status"] not in ACTIVE:
                return receipt
            journal(directory, "Stopping your application and native runtime.", status="stopping")
            if process and process.stdin:
                process.stdin.write(b'{"action":"stop"}\n')
                await process.stdin.drain()
                if task is not None:
                    try:
                        await asyncio.wait_for(asyncio.shield(task), worker_stop_timeout(directory))
                    except asyncio.TimeoutError:
                        raise DraftConflict("The owned worker has not finished cleanup. No stopped state was confirmed.") from None
            elif task is not None:
                admission = self.worker_admissions.get(deployment_id)
                task.cancel()
                await asyncio.gather(task, return_exceptions=True)
                self.tasks.pop(deployment_id, None)
                if not self._confirm_worker_not_admitted(guide_id, directory, admission):
                    journal(directory, "The worker ended; runtime cleanup still needs verification before settings or a fresh start.",
                            status="interrupted", console_url=None, runtime_pid=None, process_live=False)
                    await self._stop_failed_interactive(guide_id, deployment_id)
            return await self.status(guide_id,
                    **({"destination_owner": destination_owner} if destination_owner is not None else {}))
        if receipt["status"] in ACTIVE:
            raise DraftConflict("Stop the application before restarting it.")
        if (receipt.get("update") or {}).get("status") in {"applying", "verifying", "restoring", "recovering-candidate", "recovering-current", "recovery-needed"}:
            raise DraftConflict("Recover the interrupted update before starting this installed source.")
        if (receipt.get("execution_mode", "interactive") == "interactive" and not receipt.get("service")
                and receipt["status"] in {"failed", "interrupted"}
                and (receipt.get("update") or {}).get("status") not in {"restored-stopped", "candidate-restored-stopped"}):
            # A failed parent may have left a native child behind. An
            # available lease or a retry click cannot replace teardown.
            await self._stop_failed_interactive(guide_id, deployment_id)
            if self._directory(guide_id) != directory:
                raise DraftConflict("A different installation was selected during cleanup. Reload before starting.")
            receipt = read_receipt(directory)
        process = self.processes.get(deployment_id)
        if deployment_id in self.tasks or process is not None and process.returncode is None:
            raise DraftConflict("Wait for this installation's owned worker to finish cleanup before starting again.")
        interactive = receipt.get("execution_mode", "interactive") == "interactive" and not receipt.get("service")
        if interactive and receipt.get("interactive_lifecycle_id") is not None and not interactive_cleanup_matches(receipt):
            raise DraftConflict("The owned runtime's cleanup is unconfirmed. Stop and verify cleanup before a fresh start.")
        def checked_package():
            payload = (directory / "package.zip").read_bytes()
            if hashlib.sha256(payload).hexdigest() != receipt["package_sha256"]:
                raise DraftConflict("The saved checked package changed. No process was started.")
            files = package_files(payload)
            return files, verify_destination(receipt, files, allow_missing=action == "retry")
        # Reading and hashing the saved package, its installed copy and the
        # selected product must not hold the event loop.
        files, destination = await asyncio.to_thread(checked_package)
        # Every in-process taker of this project lock runs under self.lock, and
        # the lock refuses instead of waiting, so its holder may await threads.
        with project_lock(destination) if interactive else nullcontext():
            validate_installed_console_settings(files, destination)
            if interactive and receipt.get("interactive_lifecycle_id") is not None:
                checked_cleanup = await asyncio.to_thread(interactive_source_binding, directory, receipt,
                    allow_missing=(receipt.get("interactive_cleanup") or {}).get("boundary") == "worker-not-admitted")
                if any(receipt["interactive_cleanup"].get(key) != value for key, value in checked_cleanup.items()):
                    raise DraftConflict("The cleanup confirmation belongs to different installed source or ownership.")
                self._require_cleanup_supervisor_absent(receipt)
            require_available_console_port(installed_console_port(destination, json.loads(files["deployment.json"][0])["port"]))
            selection = None
            mode = "install" if action == "retry" else "start"
            if "runtime_candidate" in receipt:
                from .runtime_selection import MetaToolkitRuntimeTarget, parse_candidate_selection
                selected_target = parse_candidate_selection(receipt["runtime_candidate"])
                if type(selected_target) is MetaToolkitRuntimeTarget:
                    await asyncio.to_thread(product_runtime_source, files, receipt["runtime_candidate"])
                    if mode != "install" or not interactive:
                        raise DraftConflict(PRODUCT_CONFIGURE_REQUIRED)
                    if receipt.get("update") is not None:
                        raise DraftConflict("Use the original selected product update or recovery owner; an initial installation retry cannot replace it.")
                    from .deployment_application_bootstrap import require_bootstrap_history_settled
                    from .deployment_updates import require_replacement_history_settled
                    require_bootstrap_history_settled(directory)
                    require_replacement_history_settled(directory)
            if interactive and (receipt.get("update") or {}).get("status") in {"restored-stopped", "candidate-restored-stopped"}:
                from .deployment_updates import retain_update_admission
                selection = retain_update_admission(directory, receipt["update"], mode)
            journal(directory, "Restart admitted for the installed checked source.", status="queued",
                    phase="install" if action == "retry" else "start", console_url=None, runtime_pid=None, bootstrap=None,
                    startup_attempt_id=None, startup_console_health=None, startup_port_observation=None,
                    health="unknown", health_checked_at=None, process_live=False,
                    **({"service_folder_evidence": None} if receipt.get("service") else {}))
            if selection is not None:
                self._launch(directory, mode, update_selection=selection)
            else:
                self._launch(directory, mode)
        return await self.status(guide_id,
                    **({"destination_owner": destination_owner} if destination_owner is not None else {}))

    async def shutdown(self):
        from .deployment_application_preparation import shutdown_preparations
        for custody in tuple(self._application_preparations.values()):
            custody.supervisor.stop.set()
            if custody.active and custody.owner_task is not asyncio.current_task():
                custody.owner_task.cancel()
        cancellation = None
        identities, early = [], {}
        if not self.lock.locked():
            # release() can leave a successful waiter queued while locked()
            # is false. Cancel these workers before any acquisition can yield,
            # retaining their identities and joins across the fair handoff.
            identities = list(dict.fromkeys([*self.tasks, *self.processes]))
            early = dict(self.tasks)
            for task in early.values():
                task.cancel()
        else:
            # An already-exited Install completion can be queued ahead of
            # shutdown's fair lock waiter. Cancel only that exact task now,
            # retaining its original join; live worker ordering is unchanged.
            for identity, task in tuple(self.tasks.items()):
                process = self.processes.get(identity)
                admission = self.worker_admissions.get(identity) or {}
                if (process is None or process.returncode != 0
                        or not isinstance(admission, dict) or admission.get("mode") != "install"):
                    continue
                try:
                    directories = list(self.root.glob(f"*/{identity}"))
                    if len(directories) != 1:
                        continue
                    receipt = read_receipt(directories[0])
                except (OSError, ValueError):
                    # This early observation cannot replace the original
                    # locked joins and final receipt refusal below.
                    continue
                if not isinstance(receipt, dict):
                    continue
                cleanup = receipt.get("interactive_cleanup")
                candidate = receipt.get("runtime_candidate")
                if not isinstance(cleanup, dict) or not isinstance(candidate, dict):
                    continue
                try:
                    completed = (receipt.get("status") == "stopped" and receipt.get("phase") == "configure"
                        and candidate.get("format") == "forge-meta-toolkit-runtime-selection-v1"
                        and admission.get("lifecycle_id") == receipt.get("interactive_lifecycle_id")
                        and interactive_cleanup_matches(receipt)
                        and cleanup.get("boundary") == "owned-process-groups-absent"
                        and cleanup.get("supervisor_pid") == process.pid)
                except (KeyError, TypeError, ValueError):
                    continue
                if completed:
                    identities.append(identity)
                    early[identity] = task
                    task.cancel()
        # Preserve one admission through cancellation, including an unlocked
        # handoff. Any existing or earlier queued action must settle first.
        acquiring = asyncio.create_task(self.lock.acquire())
        while True:
            try:
                await asyncio.shield(acquiring)
                break
            except asyncio.CancelledError as error:
                if acquiring.cancelled():
                    raise
                cancellation = error
        try:
            identities = list(dict.fromkeys([*identities, *self.tasks, *self.processes]))
            captured = dict(self.tasks)
            for task in captured.values():
                if task not in early.values():
                    task.cancel()
            # An earlier admitted action can replace a deployment's task.
            # Keep both joins; the current capture alone owns map removal.
            joining = list(dict.fromkeys([*early.values(), *captured.values()]))
            async def finish_owned_cleanup():
                failures = []
                try:
                    await self._shutdown_locked(identities, captured, joining)
                except BaseException as failure:
                    failures.append(failure)
                try:
                    await shutdown_preparations(self)
                except BaseException as failure:
                    failures.append(failure)
                if len(failures) == 1:
                    raise failures[0]
                if failures:
                    raise BaseExceptionGroup("Forge owned cleanup did not fully settle.", failures)

            pending = asyncio.create_task(finish_owned_cleanup())
            while True:
                try:
                    await asyncio.shield(pending)
                    break
                except asyncio.CancelledError as error:
                    if pending.cancelled():
                        raise
                    # One join and final receipt pass survive repeated waiter
                    # cancellation without releasing the admission lock early.
                    cancellation = error
                except Exception as error:
                    if cancellation is not None:
                        raise BaseExceptionGroup("Forge shutdown was cancelled and its owned cleanup failed.",
                                                 [cancellation, error]) from None
                    raise
            if cancellation is not None:
                raise cancellation
        finally:
            self.lock.release()

    async def _shutdown_locked(self, identities, pending, joining):
        await asyncio.gather(*joining, return_exceptions=True)
        unconfirmed = False
        for identity in identities:
            process = self.processes.get(identity)
            live = process is not None and process.returncode is None
            unconfirmed = unconfirmed or live
            for directory in self.root.glob(f"*/{identity}"):
                receipt = read_receipt(directory)
                if live:
                    journal(directory, "The owned supervisor did not confirm exit during Forge shutdown. Preserve this folder; cleanup remains unconfirmed.",
                            status="interrupted", console_url=None, runtime_pid=None, process_live=True,
                            health="unknown", interactive_cleanup=None, interactive_cleanup_attempt_id=None)
                elif receipt["status"] in ACTIVE and not (receipt.get("execution_mode") == "background" and receipt.get("service")):
                    journal(directory, "Forge stopped before deployment completed. Resume explicitly from the saved owned folder.",
                            status="interrupted", console_url=None, runtime_pid=None, process_live=False)
            if self.tasks.get(identity) is pending.get(identity):
                self.tasks.pop(identity, None)
            if not live and self.processes.get(identity) is process:
                self.processes.pop(identity, None)
        if unconfirmed:
            raise RuntimeError("The owned deployment supervisor did not confirm exit during Forge shutdown. Its original handle and unconfirmed cleanup are preserved.")
