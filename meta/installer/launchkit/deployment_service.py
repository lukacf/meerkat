"""Owned user-service definitions and bounded operating-system lifecycle control.

This helper never installs a service merely by reading options or status. A unit
is created only after an explicitly selected background deployment passes its
installed checks. It contains paths, never application credentials.
"""
from __future__ import annotations

import asyncio
import ast
import hashlib
import json
import os
from pathlib import Path
import plistlib
import re
import signal
import stat
import sys
import unicodedata
import uuid

from .projects import DraftConflict
from .deployment_deadlines import (UNREPORTED_GATEWAY_SHUTDOWN_HORIZON_MS, USER_SERVICE_STOP_TIMEOUT, SERVICE_STOP_COMMAND_MARGIN,
                                   LEGACY_USER_SERVICE_STOP_TIMEOUT, reported_gateway_horizon,
                                   service_stop_budgets, shutdown_budget)

SERVICE_TIMEOUT = 60
SERVICE_UPDATE_TIMEOUT = 1200
SERVICE_PENDING_MAX_WAIT = 1200
SERVICE_START_TIMEOUT = 270
UPDATE_STARTUP_PROTOCOL = 1
CANDIDATE_STARTUP_PROTOCOL = 1
MAX_CONTROL_OUTPUT = 128 * 1024
# Fixed public inputs of the current compiler used by a frozen update worker.
# Existing frozen hosts are not modified or inferred to have these resources.
STATIC_RESOURCES = (
    'blueprint-limits.json',
    'templates/start.sh',
    'templates/install.sh',
    'templates/deploy-gcp.sh',
    'templates/install-service.sh',
    'templates_elephant/README.md',
    'templates_elephant/Dockerfile',
    'templates_v2/console_setup.mjs',
    'templates_v2/console_setup.css',
    'templates_v2/console_shell.mjs',
    'templates_v2/console_shell.css',
    'templates_v2/controller_review.mjs',
    'templates_v2/controller_review.css',
    'templates_v2/toolkit-ui/LICENSE-APACHE',
    'templates_v2/toolkit-ui/LICENSE-MIT',
    'templates_v2/toolkit-ui/THIRD_PARTY_NOTICES.md',
    'templates_v2/toolkit-ui/package.json',
    'templates_v2/toolkit-ui/index.js',
    'templates_v2/toolkit-ui/index.d.ts',
    'templates_v2/toolkit-ui/model.js',
    'templates_v2/toolkit-ui/model.d.ts',
    'templates_v2/toolkit-ui/projection.js',
    'templates_v2/toolkit-ui/projection.d.ts',
    'templates_v2/toolkit-ui/setup-conversation.js',
    'templates_v2/toolkit-ui/setup-conversation.d.ts',
    'templates_v2/toolkit-ui/setup-markdown.js',
    'templates_v2/toolkit-ui/setup-markdown.d.ts',
    'templates_v2/toolkit-ui/setup-model.js',
    'templates_v2/toolkit-ui/setup-model.d.ts',
    'templates_v2/toolkit-ui/view.js',
    'templates_v2/toolkit-ui/view.d.ts',
    'templates_v2/toolkit-ui/style.css',
    'templates_v2/toolkit-ui/vendor/entities/LICENSE',
    'templates_v2/toolkit-ui/vendor/entities/package.json',
    'templates_v2/toolkit-ui/vendor/entities/decode-codepoint.js',
    'templates_v2/toolkit-ui/vendor/entities/decode.js',
    'templates_v2/toolkit-ui/vendor/entities/generated/decode-data-html.js',
    'templates_v2/toolkit-ui/vendor/entities/generated/decode-data-xml.js',
    'templates_v2/toolkit-ui/vendor/entities/internal/bin-trie-flags.js',
    'templates_v2/toolkit-ui/vendor/entities/internal/decode-shared.js',
    'templates_v2/toolkit-ui/vendor/marked/LICENSE',
    'templates_v2/toolkit-ui/vendor/marked/package.json',
    'templates_v2/toolkit-ui/vendor/marked/marked.esm.js',
)
SAFE_PATH = "/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin:/opt/homebrew/bin"


class ServiceQueryUnavailable(RuntimeError):
    """Retain bounded query facts without command output or caller context."""

    def __init__(self, queries):
        super().__init__("The user service manager could not be queried.")
        classifications = (
            ("reentrancy", ("reentrancy avoided",)),
            ("permission", ("permission denied", "operation not permitted", "access denied")),
            ("absent", ("could not find service", "could not find domain", "not found")),
        )
        self.query_failures = []
        for query, code, output in queries[:2]:
            text = output[:MAX_CONTROL_OUTPUT].casefold()
            classification = next((kind for kind, phrases in classifications
                                   if any(phrase in text for phrase in phrases)), "unknown")
            self.query_failures.append({"query": query, "return_code": code,
                                        "classification": classification})


def private_file(path, *, maximum=4 * 1024 * 1024):
    path = Path(path)
    for parent in [path, *path.parents]:
        if parent.is_symlink():
            raise DraftConflict("The service control path cannot contain symbolic links.")
    metadata = path.stat()
    if (not stat.S_ISREG(metadata.st_mode) or metadata.st_uid != os.getuid()
            or metadata.st_nlink != 1 or metadata.st_mode & 0o077
            or metadata.st_size > maximum):
        raise DraftConflict("The service control file is not a private owned ordinary file.")
    return path.read_bytes()


def digest(content):
    return hashlib.sha256(content).hexdigest()


# macOS privacy protection stops a login item from reading these home folders
# and other volumes without a grant it cannot ask for. A launchd service that
# runs or reads files there stalls in Python start-up and reports nothing.
# Third-party cloud folders (~/Library/CloudStorage) are refused too, failing closed.
MACOS_PROTECTED_FOLDERS = ("Desktop", "Documents", "Downloads", "Library/Mobile Documents", "Library/CloudStorage")
MACOS_VOLUMES = "/Volumes"
_PROTECTED_REASONS = {
    "folder": ("macOS does not let a background login item read Desktop, Documents, Downloads, iCloud Drive or "
               "cloud storage folders."),
    "volume": ("macOS does not let a background login item read external or network volumes, which may also be "
               "unavailable at login."),
}
_NEW_INSTALLATION = ("Move the Forge folder and the installation folder to a folder such as ~/MetaForge, "
    "or choose 'While Forge is open'.")
# The setup card's own button is Start installed application.
_SETUP_CARD = ("To run this application in the background, install it again with the Forge folder and the "
    "installation folder in a folder such as ~/MetaForge. Start installed application runs it while Forge is open.")
# An installed unit is not moved: moving its folders would break its installation.
_EARLIER_SERVICE = ("Stop this service and install the application again in a folder such as ~/MetaForge, "
    "or install it to run while Forge is open.")
_RUNS_WITH_PERMISSION = ("This service runs only while this computer lets background login items read them, "
    "for example with Full Disk Access.")
_UNVERIFIED_LEGACY = ("Forge cannot confirm that this older service can read them. If it does not start, stop it "
    "and install the application again in a folder such as ~/MetaForge, or install it to run while Forge is open.")
# A unit created by this Forge records that its supervisor writes the ready
# record below. A support refresh copies the record unchanged, so a unit
# created earlier stays a legacy unit.
FOLDER_EVIDENCE_PROTOCOL = 1
PROTECTED_FOLDER_REFUSAL = _PROTECTED_REASONS["folder"] + " " + _NEW_INSTALLATION
OTHER_VOLUME_REFUSAL = _PROTECTED_REASONS["volume"] + " " + _NEW_INSTALLATION


def launchd_interpreter():
    """The Python a new launchd unit runs, as its definition names it."""
    return Path(sys.executable).absolute()


def _folded(path):
    # macOS folder names are case- and normalization-insensitive by default.
    return tuple(unicodedata.normalize("NFD", part).casefold() for part in Path(path).parts)


def _volume_link(entry):
    """The target of a link in the volumes folder, read without entering any volume."""
    try:
        with os.scandir(entry.parent) as entries:
            for item in entries:
                if _folded(item.name) == _folded(entry.name):
                    return Path(os.readlink(item.path)) if item.is_symlink() else None
    except OSError:
        pass
    return None


def _protected_location(path, protected, home, volumes):
    """Follow a path as a lookup would, stopping before a protected folder.

    A link is read where it is, outside protected folders. Nothing inside a
    protected folder or another volume is looked up, so the check itself never
    shows a privacy prompt. A missing part is checked as written.
    """
    def location(parts):
        if any(parts[:len(root)] == root for root in protected):
            return "folder"
        if (len(parts) > len(volumes) and parts[:len(volumes)] == volumes
                and not any(parts[:len(root)] == root for root in home)):
            return "volume"
        return None

    path = Path(path)
    if not path.is_absolute():
        path = Path.cwd() / path
    current, pending, links, present = Path(path.anchor), list(path.parts[1:]), 0, True
    while pending:
        name = pending.pop(0)
        if name == "..":
            current = current.parent
            continue
        candidate = current / name
        found = location(_folded(candidate))
        if found == "volume" and len(_folded(candidate)) == len(volumes) + 1 and present:
            target = _volume_link(candidate)
            if target is not None:
                # The startup disk's entry links back to an ordinary folder.
                links += 1
                if links > 40:
                    return None
                if target.is_absolute():
                    current = Path(target.anchor)
                pending = [*target.parts[1 if target.is_absolute() else 0:], *pending]
                continue
        if found is not None:
            return found
        if present:
            try:
                mode = os.lstat(candidate).st_mode
                target = Path(os.readlink(candidate)) if stat.S_ISLNK(mode) else None
            except (OSError, ValueError):
                present, target = False, None
            if target is not None:
                links += 1
                if links > 40:
                    return None
                if target.is_absolute():
                    current = Path(target.anchor)
                pending = [*target.parts[1 if target.is_absolute() else 0:], *pending]
                continue
        current = candidate
    return location(_folded(current))


def launchd_folder_refusal(paths, *, home=None, volumes=None, setup_card=False, installed=False, running=False):
    """Why a launchd login item could not read one of these paths, or None.

    ``setup_card`` words the advice for the completed-setup card,
    ``installed`` for a service that already exists there and ``running``
    for one that has shown it can read them.
    """
    home = Path(home) if home is not None else Path.home()
    homes = {_folded(home)}
    try:
        homes.add(_folded(home.resolve()))
    except (OSError, RuntimeError):
        pass
    protected = [(*root, *_folded(name)) for root in homes for name in MACOS_PROTECTED_FOLDERS]
    volumes = _folded(volumes if volumes is not None else MACOS_VOLUMES)
    found = {_protected_location(path, protected, homes, volumes) for path in paths}
    kind = "folder" if "folder" in found else "volume" if "volume" in found else None
    if kind is None:
        return None
    return _PROTECTED_REASONS[kind] + " " + (_SETUP_CARD if setup_card else _RUNS_WITH_PERMISSION if running
                                            else _EARLIER_SERVICE if installed else _NEW_INSTALLATION)


SERVICE_FOLDER_EVIDENCE_KEYS = ("manager", "domain", "label", "unit_path", "interpreter", "receipt_directory")


def service_folder_evidence(service, receipt):
    """The record this unit's registered instance writes when it reaches ready.

    It names the instance, the unit (manager, domain, label and unit path) and
    the paths the unit itself reads. A supervisor refresh changes the
    service's file digests but none of these, so it keeps the record.
    """
    return {"instance_id": receipt.get("service_instance_id"),
            **{key: service.get(key) for key in SERVICE_FOLDER_EVIDENCE_KEYS},
            "destination": receipt.get("destination")}


def _service_folder_paths(service, receipt):
    paths = [service.get("interpreter"), service.get("receipt_directory"), receipt.get("destination")]
    return [path for path in paths if isinstance(path, str) and path]


def _ready_evidence_current(service, receipt):
    evidence = receipt.get("service_folder_evidence")
    return (isinstance(evidence, dict) and bool(receipt.get("service_instance_id"))
            and evidence == service_folder_evidence(service, receipt))


def _legacy_supervisor(service):
    """Its frozen supervisor predates the ready record, so it can never write one."""
    return service.get("folder_evidence_protocol") != FOLDER_EVIDENCE_PROTOCOL


def installed_service_folder_refusal(service, receipt):
    """The typed refusal for an existing launchd unit whose own files are protected, or None.

    No migration: the unit runs the interpreter its definition names, from the
    deployment directory that holds service-host/launcher.py, for the
    installation folder. It is not refused when this exact unit's current
    registered instance recorded that it reached ready there, which shows
    macOS lets it read them; any other start clears that record. A legacy
    unit, whose supervisor cannot write the record, is not refused either: a
    wrong allow costs one stalled start, which Stop settles and the status
    explains. Stop is never refused.
    """
    if not isinstance(service, dict) or service.get("manager") != "launchd":
        return None
    refusal = launchd_folder_refusal(_service_folder_paths(service, receipt), installed=True)
    if refusal is not None and (_ready_evidence_current(service, receipt) or _legacy_supervisor(service)):
        return None
    return refusal


def installed_service_folder_note(service, receipt):
    """The status note for a launchd unit whose own files are in a protected folder, or None."""
    if not isinstance(service, dict) or service.get("manager") != "launchd":
        return None
    note = launchd_folder_refusal(_service_folder_paths(service, receipt), running=True)
    if note is not None and _legacy_supervisor(service) and not _ready_evidence_current(service, receipt):
        note += " " + _UNVERIFIED_LEGACY
    return note


def service_plan(identity, directory, *, platform=None, home=None):
    if str(uuid.UUID(identity)) != identity:
        raise ValueError("Use the exact owned deployment identity.")
    platform = platform or sys.platform
    home = Path(home) if home is not None else Path.home()
    label = "ai.mobkit.launch." + identity
    if platform == "darwin":
        manager, executable = "launchd", "/bin/launchctl"
        unit = home / "Library/LaunchAgents" / (label + ".plist")
        persistence = "Runs after you log in and survives closing Forge. It stops when you log out."
    elif platform == "linux":
        manager, executable = "systemd-user", "/usr/bin/systemctl"
        unit = home / ".config/systemd/user" / (label + ".service")
        persistence = "Runs after you log in and survives closing Forge. Running while logged out requires existing user lingering; Forge never enables it or asks for sudo."
    else:
        raise ValueError("Background deployment requires macOS or Linux.")
    return {"manager": manager, "manager_executable": executable, "label": label,
            "unit_path": str(unit), "domain": "gui/" + str(os.getuid()),
            "receipt_directory": str(Path(directory).absolute()), "persistence": persistence,
            "interpreter": str(Path(sys.executable).absolute()),
            "stop_timeout_seconds": USER_SERVICE_STOP_TIMEOUT,
            "host_runtime_dependency": "Keep Forge's explicit Python environment installed. The supervisor source is privately snapshotted and does not depend on the repository working directory."}


def unit_bytes(plan, *, legacy=False):
    root = Path(plan["receipt_directory"])
    stop_timeout = plan.get("stop_timeout_seconds", LEGACY_USER_SERVICE_STOP_TIMEOUT)
    arguments = [plan["interpreter"], "-I", str(root / "service-host/launcher.py"), str(root)]
    if "bootstrap_selection" in plan:
        pin = plan["bootstrap_selection"]
        arguments.extend((pin["path"], pin["sha256"]))
    if plan["manager"] == "launchd":
        return plistlib.dumps({"Label": plan["label"], "ProgramArguments": arguments,
            "WorkingDirectory": str(root), "RunAtLoad": True,
            "KeepAlive": {"SuccessfulExit": False}, "ThrottleInterval": 30,
            "ExitTimeOut": stop_timeout, "Umask": 0o077,
            "EnvironmentVariables": {"PATH": SAFE_PATH, "PYTHONNOUSERSITE": "1"}}, sort_keys=True)
    def quote(value, *, argument=False):
        # systemd expands percent specifiers even inside quoted arguments.
        value = str(value).replace("\\", "\\\\").replace('"', '\\"').replace("%", "%%")
        if argument:
            value = value.replace("$", "$$")
        return '"' + value + '"'
    command = " ".join(quote(value, argument=index > 0 or legacy) for index, value in enumerate(arguments))
    cwd = quote(root) if legacy else str(root).replace("%", "%%")
    # The description is part of the frozen ownership bytes for existing units.
    return ("[Unit]\nDescription=MobKit Launch owned deployment\n\n[Service]\n"
            "Type=simple\nWorkingDirectory=" + cwd + "\nExecStart=" + command + "\n"
            "Restart=on-failure\nRestartSec=30\nTimeoutStopSec=" + str(stop_timeout) + "\nKillMode=mixed\n"
            "UMask=0077\nNoNewPrivileges=true\nEnvironment=PATH=" + SAFE_PATH + "\n"
            "Environment=PYTHONNOUSERSITE=1\n\n[Install]\nWantedBy=default.target\n").encode()


def unit_gateway_horizon(record):
    """The gateway horizon a unit or plan was sized for; None for the unreported one."""
    try:
        return reported_gateway_horizon(record)
    except ValueError:
        raise DraftConflict("The recorded service shutdown budget is unsupported. Its definition is preserved.") from None


STOP_BUDGET_DROP_IN = "forge-stop-budget.conf"


def stop_budget_drop_in(service):
    """The owned systemd drop-in that widens a recorded unit's stop budget."""
    return Path(service["unit_path"] + ".d") / STOP_BUDGET_DROP_IN


def drop_in_bytes(stop_timeout):
    return f"[Service]\nTimeoutStopSec={stop_timeout}\n".encode()


def owned_drop_in(content, unit_budget, recorded):
    """The recorded drop-in, or the earlier one a longer widening has not replaced yet.

    A widening records its budget before it replaces the drop-in, so an
    interrupted one leaves the earlier drop-in: a budget above the unit's own
    and below the recorded one. The next readiness replaces it.
    """
    if content == drop_in_bytes(recorded):
        return True
    earlier = re.fullmatch(rb"\[Service\]\nTimeoutStopSec=([1-9][0-9]{0,7})\n", content)
    return earlier is not None and unit_budget < int(earlier[1]) < recorded


def recorded_stop_budget(receipt):
    """The widened stop budget recorded outside the service identity, or None."""
    record = receipt.get("service_stop_budget")
    if record is None:
        return None
    if (not isinstance(record, dict) or set(record) != {"gateway_shutdown_horizon_ms", "stop_timeout_seconds"}
            or unit_gateway_horizon(record) is None or type(record["stop_timeout_seconds"]) is not int
            or record["stop_timeout_seconds"] != shutdown_budget(record["gateway_shutdown_horizon_ms"])["service"]):
        raise DraftConflict("The recorded service shutdown budget is unsupported. Its definition is preserved.")
    return record


def _drop_in_content(path):
    for parent in (path, path.parent):
        if parent.is_symlink():
            raise DraftConflict("The service control path cannot contain symbolic links.")
    return private_file(path) if path.exists() else None


def stop_command_timeout(service):
    """Outlast the manager's own stop of this unit, never below the current budget's."""
    budget = service.get("stop_timeout_seconds", LEGACY_USER_SERVICE_STOP_TIMEOUT)
    if service["manager"] == "systemd-user":
        try:
            content = _drop_in_content(stop_budget_drop_in(service))
        except (OSError, DraftConflict):
            content = None
        widened = re.fullmatch(rb"\[Service\]\nTimeoutStopSec=([0-9]{1,8})\n", content or b"")
        if widened:
            budget = max(budget, int(widened[1]))
    return max(budget, USER_SERVICE_STOP_TIMEOUT) + SERVICE_STOP_COMMAND_MARGIN


def widen_service_stop_budget(directory, horizon_ms):
    """Widen a recorded unit's stop budget to a reported horizon; return the reload to run.

    An application that reports nothing (None) is sized for the unreported
    horizon, which the record names explicitly.

    The unit bytes and the service identity never change: an owned systemd
    drop-in carries the larger TimeoutStopSec. Its record is written first, so
    an interrupted write leaves a controllable unit with the earlier drop-in or
    none, and every readiness while a record exists rewrites a missing or
    earlier drop-in and reloads the manager again, completing an interrupted
    step. It never narrows a budget. launchd reads ExitTimeOut only when it
    loads a job, so a recorded plist keeps its own.
    """
    from .deployment import now, read_receipt, receipt_lock
    from .templates.storage_setup import write_private
    directory = Path(directory)
    service = verify_service(directory, read_receipt(directory))
    if service["manager"] != "systemd-user":
        return None
    sized_for = UNREPORTED_GATEWAY_SHUTDOWN_HORIZON_MS if horizon_ms is None else horizon_ms
    budget = shutdown_budget(sized_for)["service"]
    drop_in = stop_budget_drop_in(service)
    with receipt_lock(directory):
        receipt = read_receipt(directory)
        if service_binding(receipt.get("service") or {}) != service_binding(service):
            raise DraftConflict("The owned service definition changed. No service was controlled.")
        record = recorded_stop_budget(receipt)
        recorded = 0 if record is None else record["stop_timeout_seconds"]
        if budget > max(service.get("stop_timeout_seconds", LEGACY_USER_SERVICE_STOP_TIMEOUT), recorded):
            record = {"gateway_shutdown_horizon_ms": sized_for, "stop_timeout_seconds": budget}
            write_private(directory / "receipt.json", {**receipt, "service_stop_budget": record, "updated_at": now()})
        elif record is None:
            return None
        content = drop_in_bytes(record["stop_timeout_seconds"])
        if _drop_in_content(drop_in) != content:
            drop_in.parent.mkdir(mode=0o700, exist_ok=True)
            temporary = drop_in.with_name("." + drop_in.name + "." + uuid.uuid4().hex)
            descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
            try:
                with os.fdopen(descriptor, "wb") as stream:
                    stream.write(content)
                    stream.flush()
                    os.fsync(stream.fileno())
                os.replace(temporary, drop_in)
            except BaseException:
                temporary.unlink(missing_ok=True)
                raise
    return [service["manager_executable"], "--user", "daemon-reload"]


def verify_service(directory, receipt):
    if "supervisor_refresh" in receipt:
        from .deployment_supervisor import require_complete
        require_complete(directory, receipt)
    service = receipt.get("service")
    if not isinstance(service, dict):
        raise DraftConflict("This deployment has no owned operating-system service.")
    plan = service_plan(receipt["deployment_id"], directory)
    interpreter = service.get("interpreter")
    if (not isinstance(interpreter, str) or not Path(interpreter).is_absolute()
            or len(interpreter) > 2000 or any(ord(char) < 32 for char in interpreter)):
        raise DraftConflict("The admitted service interpreter path is invalid.")
    plan["interpreter"] = interpreter
    # Missing metadata identifies the exact older renderer, not permission to
    # rewrite its frozen unit or claim it has the current graceful-stop budget.
    # A budget is the legacy one, the earlier pin's, or derived from the
    # unreported horizon or the one this unit was sized for.
    stop_timeout = service.get("stop_timeout_seconds", LEGACY_USER_SERVICE_STOP_TIMEOUT)
    if type(stop_timeout) is not int or stop_timeout not in service_stop_budgets(unit_gateway_horizon(service)):
        raise DraftConflict("The recorded service shutdown budget is unsupported. Its definition is preserved.")
    plan["stop_timeout_seconds"] = stop_timeout
    if "bootstrap_selection" in service:
        from .connect_installer_caller import _artifact_pin, _path, _private_parent
        pin = _artifact_pin(service["bootstrap_selection"])
        if pin.path.parent != Path(directory) or not pin.path.name.startswith("bootstrap-admission-"):
            raise DraftConflict("The service Restore admission belongs to another installation.")
        _private_parent(_path(pin.path))
        pin.verify()
        plan["bootstrap_selection"] = pin.pin()
    for key in ("manager", "manager_executable", "label", "unit_path", "domain", "receipt_directory"):
        if service.get(key) != plan[key]:
            raise DraftConflict("The service ownership definition no longer matches this deployment.")
    content = private_file(plan["unit_path"])
    if (digest(content) == service.get("unit_sha256") and content != unit_bytes(plan)
            and plan["manager"] == "systemd-user" and content == unit_bytes(plan, legacy=True)):
        raise DraftConflict("This recorded owned systemd unit has an invalid quoted WorkingDirectory. Its original definition is preserved; activation requires a verified service-definition migration.")
    if digest(content) != service.get("unit_sha256") or content != unit_bytes(plan):
        raise DraftConflict("The owned service definition changed. No service was controlled.")
    # A widened stop budget lives outside the service identity: the recorded
    # drop-in, byte-exact, the earlier one an interrupted widening left, or
    # none yet after an interrupted first write.
    record = recorded_stop_budget(receipt)
    if plan["manager"] != "systemd-user":
        if record is not None:
            raise DraftConflict("The recorded service shutdown budget is unsupported. Its definition is preserved.")
    else:
        widened = _drop_in_content(stop_budget_drop_in(plan))
        if widened is not None and (record is None or not owned_drop_in(
                widened, stop_timeout, record["stop_timeout_seconds"])):
            raise DraftConflict("The owned service definition changed. No service was controlled.")
    host = Path(directory) / "service-host"
    manifest = service.get("host_files")
    if not isinstance(manifest, dict) or not manifest or len(manifest) > 256:
        raise DraftConflict("The service supervisor ownership evidence is missing.")
    paths = list(host.rglob("*"))
    if any(path.is_symlink() for path in paths):
        raise DraftConflict("The privately snapshotted supervisor cannot contain links.")
    actual = {str(path.relative_to(host)) for path in paths if path.is_file()}
    if actual != set(manifest):
        raise DraftConflict("The privately snapshotted supervisor files changed.")
    for name, expected in manifest.items():
        parts = name.split("/")
        if (not name or any(part in {"", ".", ".."} for part in parts)
                or "\\" in name or Path(name).is_absolute()
                or digest(private_file(host / name)) != expected):
            raise DraftConflict("The privately snapshotted supervisor source changed.")
    return service


def _service_launcher(bootstrap_owner_source_sha256=None):
    """Select historical intake bytes with a trusted bounded source literal."""
    if bootstrap_owner_source_sha256 is not None and (type(bootstrap_owner_source_sha256) is not str
            or re.fullmatch(r"[a-f0-9]{64}", bootstrap_owner_source_sha256) is None):
        raise DraftConflict("The service needs its independently selected original owner digest.")
    selected = ("BOOTSTRAP_OWNER_SOURCE_SHA256 = " + repr(bootstrap_owner_source_sha256) + "\n"
        if bootstrap_owner_source_sha256 is not None else "")
    keyword = (", bootstrap_owner_source_sha256=BOOTSTRAP_OWNER_SOURCE_SHA256"
        if bootstrap_owner_source_sha256 is not None else "")
    return ("import asyncio, os, pathlib, sys\n"
        "sys.dont_write_bytecode = True\n"
        "allowed = {'PATH', 'HOME', 'USER', 'LOGNAME', 'LANG', 'LC_ALL', 'TMPDIR', 'SHELL', 'SSL_CERT_FILE', 'SSL_CERT_DIR'}\n"
        "environment = {key: value for key, value in os.environ.items() if key in allowed}\n"
        "os.environ.clear(); os.environ.update(environment)\n"
        "sys.path.insert(0, str(pathlib.Path(__file__).parent))\n"
        "from launchkit.deployment_worker import run\n"
        "if len(sys.argv) not in {2, 4}: raise ValueError('The fixed service arguments changed.')\n"
        "selection = {'path': sys.argv[2], 'sha256': sys.argv[3]} if len(sys.argv) == 4 else None\n"
        + selected + "sys.exit(asyncio.run(run(pathlib.Path(sys.argv[1]), 'service', bootstrap_selection=selection"
        + keyword + ")) or 0)\n").encode()


def retained_bootstrap_owner_source_sha256(directory, service):
    """Read the original fixed literal through its verified host manifest.

    Retained Python is data here. Old launchers without this selected slot keep
    their existing default behavior; neither intake nor owner.json selects it.
    """
    from .deployment import read_receipt
    checked = verify_service(directory, read_receipt(directory))
    if service_binding(checked) != service_binding(service):
        raise DraftConflict("The original service source selection changed.")
    content = private_file(Path(directory) / "service-host/launcher.py")
    if digest(content) != checked["host_files"].get("launcher.py"):
        raise DraftConflict("The original service launcher changed during source selection.")
    try:
        tree = ast.parse(content.decode("utf-8"))
        slot = "BOOTSTRAP_OWNER_SOURCE_SHA256"
        names = [node for node in ast.walk(tree) if isinstance(node, ast.Name) and node.id == slot]
        keywords = [node for node in ast.walk(tree) if isinstance(node, ast.keyword)
            and node.arg == "bootstrap_owner_source_sha256"]
        if not names and not keywords:
            return None
        assignments = [node for node in tree.body if isinstance(node, ast.Assign)
            and len(node.targets) == 1 and isinstance(node.targets[0], ast.Name)
            and node.targets[0].id == slot]
        if (len(assignments) != 1 or len(names) != 2 or len(keywords) != 1
                or not isinstance(assignments[0].value, ast.Constant)
                or not isinstance(keywords[0].value, ast.Name) or keywords[0].value.id != slot):
            raise ValueError
        selected = assignments[0].value.value
        if type(selected) is not str or re.fullmatch(r"[a-f0-9]{64}", selected) is None:
            raise ValueError
        # Only our emitted slot selects authority. A malformed or substituted
        # program cannot be promoted by extracting a convenient string from it.
        if content != _service_launcher(selected) or "bootstrap_selection" not in checked:
            raise ValueError
        return selected
    except (SyntaxError, UnicodeError, ValueError, TypeError):
        raise DraftConflict("The frozen service has no exact original owner-source literal.") from None


def retained_bootstrap_files_current(observation, *, source_custody=None):
    """Retain authenticated file facts, never the parent's lease or authority.

    The running child owns the actual nonreentrant project lease. Its parent
    checks these original source/input observations after manager admission.
    """
    if source_custody is None:
        return observation.require_current
    from .connect_installer_caller import _private_directory
    from .product_installed_source import require_installed_inventory
    from .product_runtime_custody import require_product_source
    from .templates.storage_setup import require_project_directory
    source = require_product_source(source_custody)
    inputs = tuple(source._observed)
    root, names, view = source._root, source.files, source._installed_view

    def directory(path):
        if path != root:
            return _private_directory(path)
        # The installed project admits owned 0755 without group/other write;
        # artifact/intake stores keep their original private predicates.
        require_project_directory(path)
        info = path.stat()
        return path, (info.st_dev, info.st_ino, info.st_mode, info.st_uid)

    directories = tuple(directory(item.root) for item in inputs)

    def current():
        observation.require_current()
        if any(directory(path) != (path, identity) for path, identity in directories):
            raise DraftConflict("The original service source observation directories changed.")
        for item in inputs:
            item.finish()
        require_installed_inventory(root, names, view)
        observation.require_current()
        for item in inputs:
            item.finish()
    current()
    return current


def supervisor_source_files(*, bootstrap_owner_source_sha256=None):
    """Return bounded trusted host bytes without reading any installed state."""
    source = Path(__file__).parent
    files = {}
    for path in sorted(source.rglob("*.py")):
        if path.is_symlink() or "__pycache__" in path.parts:
            continue
        name = "launchkit/" + str(path.relative_to(source))
        content = path.read_bytes()
        if len(content) > 4 * 1024 * 1024 or len(files) >= 255:
            raise ValueError("The trusted supervisor snapshot exceeded its bounds.")
        files[name] = content
    # Explicit public host resources required by the supervisor import graph.
    # Never sweep arbitrary configuration, credentials or application data.
    for name in STATIC_RESOURCES:
        path = source / name
        if path.is_symlink() or not path.is_file():
            raise DraftConflict("A required trusted supervisor resource is missing or linked.")
        content = path.read_bytes()
        if len(content) > 64 * 1024 or len(files) >= 255:
            raise ValueError("A trusted supervisor resource exceeded its bound.")
        files["launchkit/" + name] = content
    files["launcher.py"] = _service_launcher(bootstrap_owner_source_sha256)
    return files


def supervisor_metadata(service, files):
    """Bind a new frozen host inventory while preserving the exact unit plan."""
    return {**service, "update_startup_protocol": UPDATE_STARTUP_PROTOCOL,
            "update_worker_sha256": digest(files["launchkit/deployment_worker.py"]),
            "candidate_startup_protocol": CANDIDATE_STARTUP_PROTOCOL,
            "candidate_worker_sha256": digest(files["launchkit/deployment_worker.py"]),
            "host_files": {name: digest(content) for name, content in files.items()}}


def prepare_service(directory, receipt, *, bootstrap_selection=None, source_custody=None):
    """Create a private frozen supervisor and exact UUID unit; no manager action."""
    from .deployment import update_receipt
    directory = Path(directory)
    owner_sha256 = None
    if source_custody is not None:
        from .product_runtime_custody import require_product_installation
        require_product_installation(source_custody, receipt)
        if bootstrap_selection is None:
            raise DraftConflict("The original destination source needs its selected Restore admission.")
        owner_sha256 = digest(source_custody._owner_source)
    if receipt.get("service"):
        service = verify_service(directory, receipt)
        if (service.get("bootstrap_selection") != bootstrap_selection
                or source_custody is not None
                and retained_bootstrap_owner_source_sha256(directory, service) != owner_sha256):
            raise DraftConflict("The frozen service cannot replace its original Restore selection.")
        return service
    plan = service_plan(receipt["deployment_id"], directory)
    # A longer horizon the application last reported widens the new unit;
    # it never gets less than the unreported horizon's budget.
    horizon_ms = unit_gateway_horizon(receipt)
    if horizon_ms is not None and shutdown_budget(horizon_ms)["service"] > plan["stop_timeout_seconds"]:
        plan["stop_timeout_seconds"] = shutdown_budget(horizon_ms)["service"]
        plan["gateway_shutdown_horizon_ms"] = horizon_ms
    if bootstrap_selection is not None:
        from .deployment_application_bootstrap import observe_bootstrap_admission
        selected, observed = observe_bootstrap_admission(directory, bootstrap_selection,
            **({"source_custody": source_custody} if source_custody is not None else {}))
        if (receipt.get("execution_mode") != "interactive" or receipt.get("status") != "stopped"
                or selected["application_startup"]["kind"] != "restore"):
            raise DraftConflict("Service startup requires the original stopped owner's completed Restore inputs.")
        observed.require_current()
        plan["bootstrap_selection"] = dict(bootstrap_selection)
    unit = Path(plan["unit_path"])
    for parent in [unit, *unit.parents]:
        if parent.is_symlink():
            raise DraftConflict("The service definition path contains a link. No unit was installed.")
    if unit.exists():
        raise DraftConflict("This service label already has a definition. No existing service was overwritten.")
    files = supervisor_source_files(**({"bootstrap_owner_source_sha256": owner_sha256}
        if owner_sha256 is not None else {}))
    host = directory / "service-host"
    host.mkdir(mode=0o700)
    for name, content in files.items():
        target = host / name
        target.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        descriptor = os.open(target, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
        with os.fdopen(descriptor, "wb") as stream:
            stream.write(content)
    content = unit_bytes(plan)
    unit.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    descriptor = os.open(unit, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(descriptor, "wb") as stream:
        stream.write(content)
    service = {**supervisor_metadata({**plan, "unit_sha256": digest(content)}, files),
               **({"folder_evidence_protocol": FOLDER_EVIDENCE_PROTOCOL} if plan["manager"] == "launchd" else {})}
    fields = {"service": service}
    if bootstrap_selection is not None:
        fields["execution_mode"] = "background"
    update_receipt(directory, **fields)
    return verify_service(directory, {**receipt, **fields})


class UserServiceManager:
    async def command(self, arguments, *, allow_failure=False, capture_output=True, timeout=None):
        environment = {"PATH": SAFE_PATH, "HOME": str(Path.home()), "USER": os.environ.get("USER", ""),
                       "LANG": "C", "LC_ALL": "C"}
        # systemctl --user requires this user-owned session bus. No credentials,
        # proxies, provider keys or Python module paths are inherited.
        if sys.platform == "linux":
            environment["XDG_RUNTIME_DIR"] = "/run/user/" + str(os.getuid())
            environment["DBUS_SESSION_BUS_ADDRESS"] = "unix:path=" + environment["XDG_RUNTIME_DIR"] + "/bus"
        process = None
        spawning = asyncio.create_task(asyncio.create_subprocess_exec(*arguments, env=environment,
            stdin=asyncio.subprocess.DEVNULL, stdout=asyncio.subprocess.PIPE if capture_output else asyncio.subprocess.DEVNULL,
            stderr=asyncio.subprocess.STDOUT, start_new_session=True))
        try:
            try:
                process = await asyncio.shield(spawning)
            except asyncio.CancelledError:
                process = await spawning
                raise
            async def capture():
                output = bytearray()
                exceeded = False
                if process.stdout:
                    while chunk := await process.stdout.read(4096):
                        if len(output) + len(chunk) > MAX_CONTROL_OUTPUT:
                            exceeded = True
                        elif not exceeded:
                            output.extend(chunk)
                await process.wait()
                if exceeded:
                    raise RuntimeError("The operating-system service query exceeded its output bound.")
                return output.decode(errors="replace")
            output = await asyncio.wait_for(capture(), SERVICE_TIMEOUT if timeout is None else timeout)
            if process.returncode and not allow_failure:
                raise RuntimeError("The user service manager rejected this operation. Inspect service state before retrying.")
            return process.returncode, output
        finally:
            if process and process.returncode is None:
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                except PermissionError:
                    # A short-lived privileged host command may exit before
                    # asyncio publishes its result. Recover only its owned
                    # process handle; never reconstruct a process from a PID.
                    await asyncio.sleep(0)
                    if process.returncode is None:
                        try:
                            process.kill()
                        except ProcessLookupError:
                            pass
                        except PermissionError:
                            await asyncio.sleep(0.05)
                            if process.returncode is None:
                                raise RuntimeError("The owned service-manager command could not be stopped.") from None
                async def drain():
                    if process.stdout:
                        while await process.stdout.read(4096):
                            pass
                    await process.wait()
                try:
                    await asyncio.wait_for(drain(), 10)
                except asyncio.TimeoutError:
                    raise RuntimeError("Termination of the owned service-manager command could not be confirmed.") from None

    async def capability(self):
        try:
            plan = service_plan(str(uuid.UUID(int=0)), Path("/unused"))
            arguments = ([plan["manager_executable"], "print", plan["domain"]] if plan["manager"] == "launchd"
                         else [plan["manager_executable"], "--user", "show", "--property=Version"])
            code, _ = await self.command(arguments, allow_failure=True, capture_output=False)
            return {"available": code == 0, "manager": plan["manager"], "persistence": plan["persistence"],
                    "host_runtime_dependency": plan["host_runtime_dependency"]}
        except (OSError, ValueError, RuntimeError, TimeoutError):
            return {"available": False, "detail": "The logged-in user's operating-system service manager is unavailable."}

    async def observe(self, service):
        executable, label = service["manager_executable"], service["label"]
        if service["manager"] == "launchd":
            code, output = await self.command([executable, "print", service["domain"] + "/" + label], allow_failure=True)
            if code:
                # A domain query distinguishes an absent unit from lost manager access.
                domain_code, domain_output = await self.command([executable, "print", service["domain"]], allow_failure=True,
                                                                capture_output=False)
                if domain_code:
                    raise ServiceQueryUnavailable([("service", code, output), ("domain", domain_code, domain_output)])
                return {"loaded": False, "pid": None, "state": "stopped"}
            paths = re.findall(r"^\s*path = (.+)$", output, re.MULTILINE)
            if paths != [service["unit_path"]]:
                raise DraftConflict("The operating-system service points to another definition.")
            match = re.search(r"^\s*pid = ([0-9]+)$", output, re.MULTILINE)
            return {"loaded": True, "pid": int(match[1]) if match else None,
                    "state": "running" if match else "waiting"}
        code, output = await self.command([executable, "--user", "show", label + ".service",
            "--property=LoadState,ActiveState,SubState,MainPID,ControlPID,ControlGroup,FragmentPath"], allow_failure=True)
        if code:
            raise ServiceQueryUnavailable([("service", code, output)])
        values = dict(line.split("=", 1) for line in output.splitlines() if "=" in line)
        if values.get("LoadState") == "not-found":
            return {"loaded": False, "pid": None, "state": "stopped"}
        if values.get("FragmentPath") != service["unit_path"]:
            raise DraftConflict("The operating-system service points to another definition.")
        return {"loaded": True, "pid": int(values.get("MainPID") or "0") or None,
                "control_pid": int(values.get("ControlPID") or "0") or None,
                "control_group": values.get("ControlGroup", ""),
                "state": values.get("ActiveState", "unknown"), "substate": values.get("SubState", "unknown")}

    async def start(self, service):
        if not Path(service["interpreter"]).is_file():
            raise DraftConflict("The admitted Python environment is unavailable. Restore it before starting this service.")
        observation = await self.observe(service)
        executable, label = service["manager_executable"], service["label"]
        if service["manager"] == "launchd":
            await self.command([executable, "enable", service["domain"] + "/" + label])
            if not observation["loaded"]:
                await self.command([executable, "bootstrap", service["domain"], service["unit_path"]])
            else:
                await self.command([executable, "kickstart", service["domain"] + "/" + label])
        else:
            await self.command([executable, "--user", "daemon-reload"])
            await self.command([executable, "--user", "enable", "--now", label + ".service"])
        return await self.observe(service)

    async def stop(self, service):
        observation = await self.observe(service)
        executable, label = service["manager_executable"], service["label"]
        if service["manager"] == "launchd":
            # Persist an explicit user stop across the next login as well.
            await self.command([executable, "disable", service["domain"] + "/" + label])
            if observation["loaded"]:
                await self.command([executable, "bootout", service["domain"] + "/" + label],
                                   timeout=stop_command_timeout(service))
        else:
            await self.command([executable, "--user", "disable", "--now", label + ".service"],
                               timeout=stop_command_timeout(service))
        return await self.observe(service)


def _service_health_current(current, expected):
    from .deployment_updates import inspection_binding, _same_toml
    return (_same_toml(inspection_binding(current), inspection_binding(expected))
            and _same_toml(current.get("bootstrap"), expected.get("bootstrap"))
            and _same_toml(current.get("admission_health"), expected.get("admission_health")))


def _record_service_observation(directory, service, public, observation, *, expected_receipt, detail=None):
    """An awaited observation cannot replace a newer host or native state."""
    from .deployment import now, read_receipt, receipt_lock
    from .templates.storage_setup import write_private
    with receipt_lock(directory):
        current = read_receipt(directory)
        current_service = current.get("service")
        if (not isinstance(current_service, dict) or service_binding(current_service) != service_binding(service)
                or not _service_health_current(current, expected_receipt)):
            return current, False
        current.update(service={**current_service, "public_summary": public},
                       service_observation=observation, updated_at=now())
        if detail is not None:
            current["detail"] = detail
        write_private(Path(directory) / "receipt.json", current)
        return current, True


async def background_status(directory, manager):
    """Read actual manager ownership; never revive a saved PID or readiness flag."""
    from .deployment import now, package_files, read_receipt, verify_destination
    from .admission_health import fresh_pending_admission
    directory = Path(directory)
    receipt = read_receipt(directory)
    inspected_receipt = receipt
    service = receipt.get("service") if isinstance(receipt.get("service"), dict) else {}
    try:
        payload = (directory / "package.zip").read_bytes()
        if digest(payload) != receipt["package_sha256"]:
            raise DraftConflict("The checked service package changed.")
        files = package_files(payload)
        verify_destination(receipt, files)
        service = verify_service(directory, receipt)
        observation = await manager.observe(service)
        receipt = read_receipt(directory)
        if not _service_health_current(receipt, inspected_receipt):
            return {**receipt, "process_live": False, "health": "unknown", "console_url": None}
        observed_pid = observation.get("pid")
        matching = bool(observed_pid and observed_pid == receipt.get("service_supervisor_pid"))
        pending = bool(matching and receipt["status"] == "starting" and receipt.get("runtime_pid")
            and receipt.get("process_live") is True and receipt.get("health") == "pending"
            and fresh_pending_admission(receipt) is not None)
        confirmed = (matching and receipt["status"] == "running" and receipt.get("runtime_pid")
            and receipt.get("bootstrap") and receipt.get("admission_health") is None)
        healthy = False
        if confirmed:
            definition = json.loads(files["deployment.json"][0])
            from .deployment_worker import console_health_headers, read_settings, readiness
            port = int(read_settings(Path(receipt["destination"]))["MOBKIT_PORT"])
            url = "http://127.0.0.1:" + str(port) + "/console"
            readiness({"pid": receipt["runtime_pid"], "bootstrap": receipt["bootstrap"], "console_url": url},
                      definition, receipt["runtime_pid"], port)
            import httpx
            try:
                async with httpx.AsyncClient(timeout=3, trust_env=False) as client:
                    response = await client.get(url.rsplit("/console", 1)[0] + "/console/identities",
                                                headers=console_health_headers(Path(receipt["destination"])))
                    response.raise_for_status()
                    actual = {item["identity"] for item in response.json()["identities"]}
                    healthy = {item["identity"] for item in definition["agents"]}.issubset(actual)
            except Exception:
                healthy = False
        changes = {"service_observation": {**observation, "checked_at": now(), "owned": True},
                   "process_live": bool(confirmed or pending), "health": "ready" if healthy else "pending" if pending else "unavailable",
                   "console_url": url if healthy else None,
                   "health_checked_at": receipt.get("health_checked_at") if pending else now()}
        if observed_pid and not matching:
            changes.update(status="starting", runtime_pid=None, bootstrap=None)
        elif not observed_pid:
            safely_stopped = receipt.get("service_cleanup_confirmed") is True or not receipt.get("service_supervisor_pid")
            changes.update(status=("stopped" if safely_stopped else "interrupted") if manager_stopped(observation)
                           else "failed" if observation["state"] == "failed" or receipt["status"] == "failed" else "starting",
                           runtime_pid=None, bootstrap=None)
        public = {"manager": service["manager"], "state": observation["state"], "pid": observed_pid,
                  "definition_verified": True, "persistence": service["persistence"]}
        changes["service"] = {**service, "public_summary": public}
        # The service worker alone owns startup/bootstrap/terminality. A slow
        # read-only health query must not overwrite a newer native startup.
        observed, recorded = _record_service_observation(directory, service, public, changes["service_observation"],
            expected_receipt=inspected_receipt)
        if not recorded:
            return {**observed, "process_live": False, "health": "unknown", "console_url": None}
        if ("status" in changes and changes["status"] != observed.get("status")
                and observed.get("status") != "failed"):
            # The manager observation may project a different current status.
            # Pair it with current detail, without rewriting the worker's saved
            # lifecycle report or hiding an actual failed native attempt.
            details = {
                "starting": ("The background service has started, but this application's startup and Console health are not confirmed."
                    if observed_pid else "The background service is waiting. Agent and Console startup has not been confirmed."),
                "stopped": "The background service is stopped. Installed files and saved records are preserved.",
                "interrupted": "The background service is stopped, but cleanup of the previous application is unconfirmed. Check its saved activity before restarting.",
                "failed": "The background service reports a failure. Check its saved activity before restarting.",
            }
            if changes["status"] in details:
                changes["detail"] = details[changes["status"]]
        from .deployment import await_validation
        note = await await_validation(installed_service_folder_note, service, observed)
        if note is not None:
            # Projected only: a unit whose files are in a protected macOS
            # folder runs only while macOS lets it read them.
            changes["protected_folder"] = note
            if changes.get("status", observed.get("status")) == "starting" and not matching:
                # Without this unit's own ready record it waits in Python
                # start-up and never registers. Say why.
                reason = await await_validation(installed_service_folder_refusal, service, observed)
                if reason is not None:
                    changes["detail"] = reason
        return {**observed, **changes}
    except (OSError, ValueError, DraftConflict, RuntimeError, TimeoutError) as error:
        observation = {"owned": False, "state": "unknown", "checked_at": now()}
        if isinstance(error, ServiceQueryUnavailable):
            observation["query_failures"] = error.query_failures
        observed, _ = _record_service_observation(directory, service,
            {"manager": service.get("manager"), "state": "unknown", "pid": None, "definition_verified": False},
            observation,
            expected_receipt=inspected_receipt,
            detail=str(error) if isinstance(error, (DraftConflict, RuntimeError)) else "The owned service could not be verified. Preserve its files and inspect before retrying.")
        return {**observed, "process_live": False, "health": "unknown", "console_url": None}


NEVER_STARTED_STOP = ("The background service is stopped. It ended before it started this application, so there was "
    "nothing to clean up. Installed files and saved records are preserved.")


def _unregistered_lifetime(service, receipt):
    """No worker of this exact unit registered a lifetime.

    A Forge supervisor registers its PID and instance (deployment_worker run)
    before it creates a Supervisor or starts any native runtime. Without that
    record, a unit that has ended never started one. A frozen supervisor
    without the lifetime protocol record keeps today's custody.
    """
    current = receipt.get("service")
    return (receipt.get("service_supervisor_pid") is None and receipt.get("service_instance_id") is None
            and isinstance(current, dict) and service_binding(current) == service_binding(service)
            and type(service.get("update_startup_protocol")) is int
            and service.get("update_worker_sha256") == service.get("host_files", {}).get("launchkit/deployment_worker.py"))


def service_process_absent(pid):
    """True only when no process has this PID; a live, foreign or reused PID keeps custody."""
    if type(pid) is not int or pid <= 0:
        return False
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return True
    except OSError:
        return False
    return False


async def stop_background(directory, manager):
    """Stop only the verified UUID unit and require its own native cleanup receipt.

    A process the manager reported that never registered its lifetime cannot
    send that receipt. Such a stop settles at once when the manager reports
    the unit gone, the re-read receipt still has no registration and every PID
    the manager reported during this stop is gone: it started no native runtime.
    """
    from .deployment import journal, read_receipt
    directory = Path(directory)
    receipt = read_receipt(directory)
    service = verify_service(directory, receipt)
    observation = await manager.observe(service)
    instance = receipt.get("service_instance_id")
    unregistered = _unregistered_lifetime(service, receipt)
    reported = {observation["pid"]} if observation.get("pid") else set()
    had_supervisor = bool(observation.get("pid") or receipt.get("service_supervisor_pid"))
    journal(directory, "Stopping the owned operating-system service and its native runtime.", status="stopping",
            console_url=None)
    await manager.stop(service)
    deadline = asyncio.get_running_loop().time() + SERVICE_TIMEOUT
    stopped_summary = {"manager": service["manager"], "state": "stopped", "pid": None, "definition_verified": True}
    while True:
        current = read_receipt(directory)
        observation = await manager.observe(service)
        if observation.get("pid"):
            reported.add(observation["pid"])
        cleaned = (not had_supervisor or current.get("service_cleanup_confirmed") is True
                   and current.get("service_instance_id") == instance)
        if manager_stopped(observation) and cleaned:
            return journal(directory, "The operating-system service is stopped. Installed files and native state are preserved.",
                           status="stopped", runtime_pid=None, console_url=None, process_live=False,
                           health="unknown",
                           service_observation={**observation, "owned": True}, service={**service, "public_summary": stopped_summary})
        # A reported process that never registered cannot report cleanup.
        if (unregistered and manager_stopped(observation)
                and _unregistered_lifetime(service, read_receipt(directory))
                and all(service_process_absent(pid) for pid in reported)):
            return journal(directory, NEVER_STARTED_STOP, status="stopped", runtime_pid=None, console_url=None,
                           process_live=False, health="unknown", service_cleanup_confirmed=True,
                           service_observation={**observation, "owned": True},
                           service={**service, "public_summary": stopped_summary})
        if asyncio.get_running_loop().time() >= deadline:
            return journal(directory, "The service manager was stopped, but the previous native runtime did not confirm cleanup. No replacement will be started.",
                           status="interrupted", console_url=None, process_live=False, health="unknown")
        await asyncio.sleep(0.2)


def manager_stopped(observation):
    """A waiting/restarting unit is not proof of an explicit service stop."""
    return (not observation.get("pid") and not observation.get("control_pid")
            and not observation.get("control_group")
            and (observation.get("loaded") is False or observation.get("state") in {"stopped", "inactive"}))


def service_binding(service):
    return {key: value for key, value in service.items() if key != "public_summary"}


def require_update_protocol(service):
    if (type(service.get("update_startup_protocol")) is not int
            or service["update_startup_protocol"] != UPDATE_STARTUP_PROTOCOL
            or service.get("update_worker_sha256") != service.get("host_files", {}).get("launchkit/deployment_worker.py")):
        raise DraftConflict("This preserved background supervisor does not support source-bound update startup verification. Its frozen definition is unchanged; update preview remains available, but activation requires a new supported installation or an explicit service migration.")


def require_candidate_startup_protocol(service, *, message=None):
    """Check the startup closure required by the frozen candidate reader."""
    files = service.get("host_files") or {}
    current_reader = digest(Path(__file__).with_name("runtime_candidates.py").read_bytes())
    required = ("launchkit/runtime_selection.py", "launchkit/runtime_dependencies.py",
                "launchkit/runtime_candidates.py", "launchkit/templates/install_candidate.py",
                "launchkit/deployment_supervisor.py")
    # A closed historical reader retains its own admitted helper closure.
    if files.get("launchkit/runtime_candidates.py") == current_reader:
        required += ("launchkit/sdk_wheel_integrity.py",)
    if (type(service.get("candidate_startup_protocol")) is not int
            or service["candidate_startup_protocol"] != CANDIDATE_STARTUP_PROTOCOL
            or service.get("candidate_worker_sha256") != files.get("launchkit/deployment_worker.py")
            or not all(name in files for name in required)):
        raise DraftConflict(message or "This preserved background supervisor cannot verify the selected candidate runtime on startup and restart. The update is staged and the installation is unchanged. Activation requires an explicit supported supervisor migration.")


def require_candidate_update_protocol(service, update):
    """A generic update protocol does not admit candidate startup or restart."""
    helper = "launchkit/sdk_wheel_integrity.py"
    current_reader = digest(Path(__file__).with_name("runtime_candidates.py").read_bytes())
    maintained = update.get("maintained_host_update") or {}
    dependency = update.get("runtime_dependency_update")
    if dependency is not None:
        from .deployment_updates import _runtime_supervisor_paths, _sha256
        version = dependency.get("version") if type(dependency) is dict else None
        if type(version) is not int or version not in {1, 2, 3, 4}:
            raise DraftConflict("The staged runtime update has no supported frozen history contract.")
        if version in {2, 3, 4}:
            sources = dependency.get("required_supervisor_sources")
            files = service.get("host_files") or {}
            if (type(sources) is not dict or set(sources) != set(_runtime_supervisor_paths(version, sources=sources))
                    or helper not in sources and sources.get("launchkit/runtime_candidates.py") == current_reader
                    or not all(_sha256(expected) and files.get(name) == expected for name, expected in sources.items())):
                raise DraftConflict("This preserved background supervisor cannot verify the exact current runtime history and compiler closure. Its frozen definition is unchanged. Activation requires an explicit supported supervisor migration.")
    selection = ((update.get("runtime_dependency_update") or {}).get("runtime_candidate")
                 or maintained.get("runtime_candidate"))
    if selection is None:
        return
    require_candidate_startup_protocol(service)
    files = service.get("host_files") or {}
    if maintained:
        from .deployment_updates import _maintained_supervisor_paths, _sha256
        sources = maintained.get("required_supervisor_sources")
        if (type(sources) is not dict or set(sources) != set(_maintained_supervisor_paths(sources=sources))
                or helper not in sources and sources.get("launchkit/runtime_candidates.py") == current_reader
                or not all(_sha256(expected) and files.get(name) == expected for name, expected in sources.items())):
            raise DraftConflict("This preserved background supervisor cannot verify the exact maintained Console host refresh. The update is staged and the installation is unchanged. Activation requires an explicit supported supervisor migration.")


def background_update_capabilities(directory, receipt):
    """Project only verified source and frozen service-definition support."""
    result = {"background_activation_supported": False, "requires_stopped": True,
              "deployment_id": receipt["deployment_id"], "build_id": receipt["build_id"],
              "package_sha256": receipt["package_sha256"]}
    if receipt.get("execution_mode") != "background":
        return result
    try:
        from .deployment_updates import (checked_package, inspect_transaction, verify_transition,
            inspection_binding, require_inspection_current, _same_toml)
        from .deployment import verify_destination, read_receipt
        snapshot = read_receipt(directory)
        if not _same_toml(inspection_binding(snapshot), inspection_binding(receipt)):
            raise DraftConflict("The supplied deployment changed before capability inspection. Check its current state again.")
        selected_update = None
        _, files = checked_package(directory, "package.zip", receipt["package_sha256"])
        try:
            verify_destination(receipt, files)
        except DraftConflict:
            update = receipt.get("update") or {}
            if update.get("status") not in {"applying", "verifying", "restoring", "recovering-candidate", "recovering-current", "recovery-needed"}:
                raise
            bound, root, update = inspect_transaction(directory, update["update_id"])
            _, before = checked_package(root, "previous.zip", update["previous_package_sha256"])
            _, after = checked_package(root, "candidate.zip", update["package_sha256"])
            verify_transition(bound, before, after)
        service = verify_service(directory, receipt)
        require_update_protocol(service)
        if (receipt.get("update") or {}).get("update_id"):
            _, _, selected_update = inspect_transaction(directory, receipt["update"]["update_id"])
            require_candidate_update_protocol(service, selected_update)
        require_inspection_current(directory, snapshot, selected_update)
        result.update(background_activation_supported=True, manager=service["manager"],
                      unit_sha256=service["unit_sha256"],
                      host_manifest_sha256=digest(json.dumps(service["host_files"], sort_keys=True).encode()))
    except (OSError, ValueError, KeyError, DraftConflict) as error:
        result["reason"] = str(error) if isinstance(error, DraftConflict) else "The owned source or frozen service definition could not be verified."
    return result


async def require_stopped_service(directory, manager):
    """Admission/copy fence; never stop or replace a runtime as a read action."""
    from .deployment import read_receipt
    receipt = read_receipt(directory)
    service = verify_service(directory, receipt)
    observation = await manager.observe(service)
    current = read_receipt(directory)
    if service_binding(verify_service(directory, current)) != service_binding(service):
        raise DraftConflict("The owned service definition changed while checking its stopped state.")
    if (current.get("service_supervisor_pid"), current.get("service_instance_id")) != (
            receipt.get("service_supervisor_pid"), receipt.get("service_instance_id")):
        raise DraftConflict("The background service instance changed while checking its stopped state. Inspect and stop its current instance.")
    if (not manager_stopped(observation)
            or current.get("service_supervisor_pid") and current.get("service_cleanup_confirmed") is not True):
        raise DraftConflict("Stop this exact owned background service and confirm its native cleanup before changing source.")
    return service


async def update_background(directory, manager, mode, *, update_id, update_selection=None):
    """Use the ordinary transaction with a fresh, owned OS-service startup."""
    from .deployment import journal, read_receipt, update_receipt
    from .deployment_worker import Supervisor, update_runtime
    from .deployment_updates import read_update_admission, transaction, update_binding
    selected = read_update_admission(directory, mode, update_selection) if update_selection is not None else None
    if selected is not None and selected["binding"]["update_id"] != update_id:
        raise DraftConflict("The original background update admission changed.")
    _, _, original = transaction(directory, update_id,
        expected_binding=selected["binding"] if selected is not None else None,
        expected_installation=selected["installation"] if selected is not None else None)
    expected_binding = update_binding(original)
    expected_installation = selected["installation"] if selected is not None else None

    def require_original():
        if update_selection is not None:
            read_update_admission(directory, mode, update_selection)
        return transaction(directory, update_id, expected_binding=expected_binding, expected_installation=expected_installation)

    class BackgroundSupervisor(Supervisor):
        def __init__(self, directory):
            super().__init__(directory)
            # Authored checks clear startup fields before handing off. Retain
            # the preceding service attempt so stale proof cannot be reused.
            self.previous_instance = self.receipt.get("service_instance_id")
            self.previous_attempt = self.receipt.get("startup_attempt_id")
            self.failed_start = None
            self.pending_update_remaining = None
            self.pending_started_at = None
            self.pending_suspended = 0.0
            self.pending_limit = min(SERVICE_PENDING_MAX_WAIT, SERVICE_UPDATE_TIMEOUT)

        def resume_update_budget(self):
            if self.pending_update_remaining is not None:
                observed_at = asyncio.get_running_loop().time()
                elapsed = observed_at - self.pending_started_at
                paused = min(elapsed, max(0.0, self.pending_limit - self.pending_suspended))
                self.pending_suspended += paused
                remaining = self.pending_update_remaining - (elapsed - paused)
                if not update_deadline.expired():
                    update_deadline.reschedule(observed_at + max(0.0, remaining))
                self.pending_update_remaining = None
                self.pending_started_at = None
                return paused
            return 0.0

        async def before_source_change(self):
            from .deployment import await_validation
            from .deployment_updates import inspect_transaction, require_inspection_current
            bound, _, selected_update = await await_validation(inspect_transaction,
                self.directory, update_id, expected_binding=expected_binding, expected_installation=expected_installation)
            # The stopped-service observation must follow expensive inspection.
            service = await require_stopped_service(self.directory, manager)
            require_inspection_current(self.directory, bound, selected_update)
            require_original()
            require_update_protocol(service)
            if mode != "rollback":
                require_candidate_update_protocol(service, selected_update)

        async def before_source_restore(self):
            receipt = read_receipt(self.directory)
            self.previous_instance = receipt.get("service_instance_id")
            self.previous_attempt = receipt.get("startup_attempt_id")
            try:
                stopped = await stop_background(self.directory, manager)
            finally:
                # Stop drains the frozen supervisor's output. Retain it before
                # a fresh fallback supervisor replaces the ordinary log list.
                self.retain_failed_start()
            if stopped["status"] != "stopped":
                raise RuntimeError("The attempted background runtime did not confirm cleanup.")

        async def start(self, definition, *, on_ready=None):
            receipt = read_receipt(self.directory)
            self.failed_start = {"attempt_id": str(uuid.uuid4()), "build_id": receipt["build_id"],
                "package_sha256": receipt["package_sha256"], "prior_logs": receipt.get("logs", []),
                "prior_instance": self.previous_instance, "prior_attempt": self.previous_attempt}
            try:
                return await self.start_service(definition, on_ready=on_ready)
            except BaseException as error:
                self.failed_start["error"] = error
                failed = read_receipt(self.directory)
                self.failed_start["failed_instance"] = failed.get("service_instance_id")
                self.failed_start["failed_attempt"] = failed.get("startup_attempt_id")
                self.failed_start["failed_logs"] = failed.get("logs", [])
                raise
            finally:
                # A pending attempt must not disable bounds on cleanup or a
                # later fallback attempt, even on cancellation or failure.
                self.resume_update_budget()

        def retain_failed_start(self):
            context = self.failed_start
            if not context or "error" not in context:
                return
            from .deployment_updates import retain_failed_service_attempt
            from .deployment_worker import Redactor, read_settings
            receipt = read_receipt(self.directory)
            if (receipt.get("service_instance_id"), receipt.get("startup_attempt_id")) != (
                    context["failed_instance"], context["failed_attempt"]):
                raise DraftConflict("The failed service attempt changed during cleanup. No later instance's diagnostics were attributed to it.")
            instance = receipt.get("service_instance_id")
            if instance == context["prior_instance"]:
                instance = None
            attempt = receipt.get("startup_attempt_id") if instance else None
            if attempt == context["prior_attempt"]:
                attempt = None
            logs = [item for item in context["failed_logs"] if item not in context["prior_logs"]]
            logs.extend(item for item in receipt.get("logs", [])
                        if item not in context["prior_logs"] and item not in logs)
            # Re-read all current private values; diagnostic archives also omit
            # installed/private control paths, without loading authored code.
            redact = Redactor([*read_settings(self.destination).values(),
                               str(self.destination), str(self.directory)])
            retain_failed_service_attempt(self.directory, update_id, context,
                error=context["error"], logs=logs, redact=redact,
                service_instance_id=instance, startup_attempt_id=attempt)
            self.failed_start = None

        async def start_service(self, definition, *, on_ready=None):
            from .deployment_updates import checked_package
            from .deployment import verify_destination
            from .admission_health import fresh_pending_admission
            receipt = read_receipt(self.directory)
            service = await require_stopped_service(self.directory, manager)
            require_original()
            require_update_protocol(service)
            previous_instance, previous_attempt = self.previous_instance, self.previous_attempt
            expected = (receipt["build_id"], receipt["package_sha256"])
            journal(self.directory, "Starting the exact owned background service; awaiting fresh native startup and authenticated Console health.",
                    status="starting", phase="update", runtime_pid=None, bootstrap=None,
                    startup_attempt_id=None, startup_console_health=None, admission_health=None, console_url=None,
                    health="unknown", health_checked_at=None, process_live=False,
                    # Only reaching ready again re-records folder evidence.
                    service_folder_evidence=None)
            await manager.start(service)
            require_original()
            deadline = asyncio.get_running_loop().time() + SERVICE_START_TIMEOUT
            pending_since = None
            while True:
                if self.stop.is_set():
                    raise asyncio.CancelledError
                # Suspending the admission wait never permits an unbounded
                # manager/health query. Each active observation is still
                # limited by the transaction budget left before pending.
                async with asyncio.timeout(self.pending_update_remaining):
                    observed = await background_status(self.directory, manager)
                require_original()
                current = read_receipt(self.directory)
                if ((current["build_id"], current["package_sha256"]) != expected
                        or service_binding(verify_service(self.directory, current)) != service_binding(service)):
                    raise DraftConflict("The exact background source or service definition changed during startup.")
                _, files = checked_package(self.directory, "package.zip", expected[1])
                verify_destination(current, files)
                instance, attempt = current.get("service_instance_id"), current.get("startup_attempt_id")
                health = current.get("startup_console_health") or {}
                manager_observation = observed.get("service_observation") or {}
                pending = (observed.get("status") == "starting" and observed.get("health") == "pending"
                    and observed.get("process_live") is True
                    and manager_observation.get("owned") is True
                    and manager_observation.get("state") in {"running", "active"}
                    and manager_observation.get("pid") == current.get("service_supervisor_pid")
                    and instance and instance != previous_instance and attempt and attempt != previous_attempt
                    and current.get("service_cleanup_confirmed") is False
                    and _service_health_current(current, observed)
                    and fresh_pending_admission(current) is not None)
                checked_at = asyncio.get_running_loop().time()
                suspended = self.pending_suspended + (checked_at - pending_since if pending_since is not None else 0.0)
                pause = pending and suspended < self.pending_limit
                if pause and pending_since is None:
                    remaining = update_deadline.when() - checked_at
                    if remaining <= 0 or checked_at >= deadline:
                        raise TimeoutError("The update startup deadline expired before admission recovery was observed.")
                    # Fresh pending frames may suspend the normal budgets,
                    # within one cumulative cap across this transaction.
                    # Even a parked manager query remains bounded overall.
                    self.pending_update_remaining = remaining
                    self.pending_started_at = checked_at
                    update_deadline.reschedule(checked_at + remaining + self.pending_limit - self.pending_suspended)
                    pending_since = checked_at
                elif not pause and pending_since is not None:
                    deadline += self.resume_update_budget()
                    pending_since = None
                if (observed.get("status") == "running" and observed.get("health") == "ready"
                        and observed.get("process_live") is True
                        and instance and instance != previous_instance and attempt and attempt != previous_attempt
                        and current.get("service_cleanup_confirmed") is False
                        and health.get("attempt_id") == attempt and health.get("status") == "pass"):
                    if self.stop.is_set():
                        raise asyncio.CancelledError
                    if on_ready:
                        on_ready()
                    update_receipt(self.directory, update_service_verification={
                        "update_id": update_id, "build_id": expected[0], "package_sha256": expected[1],
                        "service_instance_id": instance, "startup_attempt_id": attempt,
                        "service_supervisor_pid": current["service_supervisor_pid"],
                        "runtime_pid": current["runtime_pid"], "verified_at": observed["health_checked_at"],
                        "unit_sha256": service["unit_sha256"],
                        "scope": "Fresh owned service instance, native bootstrap and authenticated Console identities. Application jobs and external services remain unverified."})
                    restored = (read_receipt(self.directory).get("update") or {}).get("status") == "rolled-back"
                    message = ("The previous verified source restarted as the owned background service and passed fresh native bootstrap and authenticated Console health."
                               if restored else "The accepted application update passed fresh native bootstrap and authenticated Console health as the owned background service.")
                    journal(self.directory, message,
                            status="running", phase="run", health="ready", console_url=observed["console_url"],
                            process_live=True, health_checked_at=observed["health_checked_at"])
                    return
                if current.get("status") == "failed":
                    raise RuntimeError("The background application failed its native startup. Inspect the retained redacted diagnostics.")
                if not pause and asyncio.get_running_loop().time() >= deadline:
                    raise TimeoutError("The owned background service did not supply fresh native startup and authenticated Console health in time.")
                await asyncio.sleep(0.2)

    supervisor = BackgroundSupervisor(directory)
    try:
        async with asyncio.timeout(SERVICE_UPDATE_TIMEOUT) as update_deadline:
            require_original()
            await update_runtime(supervisor, mode, update_id=update_id,
                expected_binding=expected_binding, update_selection=update_selection)
    except asyncio.CancelledError:
        receipt = read_receipt(directory)
        if receipt.get("service_cleanup_confirmed") is True:
            journal(directory, "The background update was interrupted after its owned runtime cleanup. Verified source and current records are retained.",
                    status="stopped", runtime_pid=None, console_url=None, process_live=False, health="unknown")
        else:
            journal(directory, "The background update was interrupted; native cleanup remains unconfirmed. Preserve this exact transaction before recovery.",
                    status="interrupted", console_url=None, process_live=False, health="unknown")
        raise
    except Exception as error:
        journal(directory, supervisor.redact(str(error)) if isinstance(error, (RuntimeError, ValueError, TimeoutError)) else "The background update needs inspection before recovery.",
                status="failed", runtime_pid=None, console_url=None, process_live=False, health="unknown")
        raise
    finally:
        supervisor.stop.set()
        await supervisor.terminate()
