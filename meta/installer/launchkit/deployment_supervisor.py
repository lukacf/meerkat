"""Explicit stopped-service supervisor refresh with forward-only custody.

This changes trusted supervisor source only. It never controls the OS service,
runs application/SDK/native code, installs a runtime, or changes private data.
"""
from __future__ import annotations

from copy import deepcopy
import hashlib
import json
import os
from pathlib import Path
import stat
import uuid

from . import deployment_service as services
from . import deployment_updates as updates
from .deployment import read_receipt, receipt_lock, now, verify_destination, await_validation
from .deployment_updates import checked_package, _candidate_source_selection, _same_toml, inspect_transaction
from .projects import DraftConflict, fingerprint
from .runtime_dependencies import RuntimeDependencyError, verify_runtime
from .templates.storage_setup import project_lock

MAX_FILE = 4 * 1024 * 1024
MAX_TOTAL = 32 * 1024 * 1024
PHASES = {"prepared", "retaining", "retained", "installing", "installed", "publishing", "completed"}
SOURCE_KEYS = ("deployment_id", "guide_id", "build_id", "guide_version", "blueprint_hash", "brief_hash",
    "package_sha256", "destination", "execution_mode", "source_kind", "service_supervisor_pid",
    "service_instance_id", "service_cleanup_confirmed", "startup_attempt_id", "update")
PROTOCOL_KEYS = {"host_files", "update_startup_protocol", "update_worker_sha256",
    "candidate_startup_protocol", "candidate_worker_sha256"}
MAX_ADMISSION_CUSTODY = 64
OWNER_KEYS = ("deployment_id", "guide_id", "destination", "execution_mode", "source_kind", "target")


def service_digest(service):
    return fingerprint(services.service_binding(service))


def _sha(value):
    return hashlib.sha256(value).hexdigest()


def _encoded(value):
    return (json.dumps(value, sort_keys=True, indent=2) + "\n").encode()


def _directory(path):
    path = Path(path)
    if any(parent.is_symlink() for parent in [path, *path.parents]):
        raise DraftConflict("Supervisor custody paths cannot contain links.")
    info = path.stat()
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid() or stat.S_IMODE(info.st_mode) != 0o700:
        raise DraftConflict("Supervisor custody directories must be private and owned.")
    return path


def _relative(name):
    if (not isinstance(name, str) or not name or len(name) > 2000 or "\\" in name
            or Path(name).is_absolute() or any(ord(char) < 32 for char in name)
            or any(part in {"", ".", ".."} for part in name.split("/"))):
        raise DraftConflict("The supervisor inventory contains an unsafe path.")


def _read(path, mode, *, limit=MAX_FILE):
    content = services.private_file(path, maximum=limit)
    if stat.S_IMODE(Path(path).stat().st_mode) != mode:
        raise DraftConflict("The supervisor custody file mode changed.")
    return content


def _files(root, expected=None, *, archived=False):
    root = _directory(root)
    paths = list(root.rglob("*"))
    if len(paths) > 512:
        raise DraftConflict("The supervisor inventory exceeded its bounds.")
    files, total = {}, 0
    for path in paths:
        if path.is_symlink():
            raise DraftConflict("The supervisor inventory cannot contain links.")
        if path.is_dir():
            _directory(path)
            continue
        name = str(path.relative_to(root))
        _relative(name)
        mode = 0o400 if archived else 0o600
        content = _read(path, mode)
        total += len(content)
        if len(files) >= 256 or total > MAX_TOTAL:
            raise DraftConflict("The supervisor inventory exceeded its bounds.")
        files[name] = content
    manifest = {name: {"sha256": _sha(content), "bytes": len(content), "mode": 0o600}
        for name, content in files.items()}
    if not files or expected is not None and manifest != expected:
        raise DraftConflict("The exact supervisor bundle changed.")
    parents = {str(Path(name).parent) for name in files}
    parents |= {str(parent) for name in files for parent in Path(name).parents if str(parent) != "."}
    parents.discard(".")
    if {str(p.relative_to(root)) for p in paths if p.is_dir()} != parents:
        raise DraftConflict("The supervisor directory inventory changed.")
    return files, manifest


def _write(path, content, mode):
    missing, parent = [], path.parent
    while not parent.exists() and not parent.is_symlink():
        missing.append(parent)
        parent = parent.parent
    for directory in reversed(missing):
        directory.mkdir(mode=0o700)
    _directory(path.parent)
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, mode)
    with os.fdopen(descriptor, "wb") as stream:
        stream.write(content)
        stream.flush()
        os.fsync(stream.fileno())


def _tree(root, files, *, archived=False):
    root.mkdir(mode=0o700)
    for name, content in files.items():
        _relative(name)
        _write(root / name, content, 0o400 if archived else 0o600)
    manifest = _files(root, archived=archived)[1]
    for path in sorted((p for p in root.rglob("*") if p.is_dir()), key=lambda p: len(p.parts), reverse=True):
        _sync(path)
    _sync(root)
    return manifest


def _sync(path):
    descriptor = os.open(path, os.O_RDONLY)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def write_private(path, value):
    """Publish owned custody metadata without following preexisting pending links."""
    path = Path(path)
    _directory(path.parent)
    temporary = path.with_name(f".{path.name}.{uuid.uuid4()}.pending")
    _write(temporary, _encoded(value), 0o600)
    temporary.replace(path)
    _sync(path.parent)


def _fence(receipt):
    result = {key: deepcopy(receipt.get(key)) for key in SOURCE_KEYS}
    # Legacy ordinary-source descriptors predate candidate admission. Preserve
    # their shape while pinning an already admitted candidate in new custody.
    if "runtime_candidate" in receipt:
        result["runtime_candidate"] = deepcopy(receipt["runtime_candidate"])
    return result


def _stopped(receipt):
    if (receipt.get("execution_mode") != "background" or receipt.get("status") != "stopped"
            or receipt.get("service_cleanup_confirmed") is not True or receipt.get("runtime_pid") is not None):
        raise DraftConflict("Refresh requires this exact cleaned and stopped background service.")


def _closed_candidate_update(directory, receipt, selection):
    """Check the named historical journal, never select it as a live update."""
    update = receipt["update"]
    root = _directory(updates.update_directory(directory, update["update_id"]))
    if (not _same_toml(json.loads(_read(root / "update.json", 0o600)), update)
            or update.get("status") != "completed"
            or update["package_sha256"] != receipt["package_sha256"]):
        raise ValueError("The retained candidate journal changed.")
    _, candidate = checked_package(root, "candidate.zip", update["package_sha256"])
    _, previous = checked_package(root, "previous.zip", update["previous_package_sha256"])
    _read(root / "previous-receipt.json", 0o600)
    if (not _same_toml(_candidate_source_selection(candidate), selection)
            or not _same_toml(receipt.get("runtime_candidate"), selection)):
        raise ValueError("The retained candidate selection changed.")
    runtime, maintained = update.get("runtime_dependency_update"), update.get("maintained_host_update")
    if runtime is not None and maintained is None:
        transition = updates._completed_runtime_dependency_transition(previous, candidate, receipt, update, root)
        descriptor = runtime
    elif maintained is not None and runtime is None:
        transition = updates._completed_maintained_host_transition(previous, candidate, receipt, update, root)
        descriptor = maintained
    else:
        raise ValueError("The retained source has no single qualified crossing.")
    if (transition is None or not _same_toml(transition, descriptor)
            or not _same_toml(descriptor.get("runtime_candidate"), selection)):
        raise ValueError("The retained candidate crossing changed.")
    return root, update


def _retained_candidate_admission(directory, receipt, selection):
    """Follow only hash-bound custody receipts to the original admission.

    A new refresh may be pending, but every older custody hop is closed.
    Equal-source hops retain the same update; source crossings must bind the
    exact archived predecessor of a completed maintained host transition.
    """
    cursor, seen = receipt, set()
    root, update = _closed_candidate_update(directory, cursor, selection)
    while update.get("runtime_dependency_update") is None:
        selected = cursor.get("supervisor_refresh")
        if (type(selected) is not dict or set(selected) != {"revision", "descriptor_sha256"}
                or selected["revision"] in seen or len(seen) >= MAX_ADMISSION_CUSTODY):
            raise ValueError("The retained admission has no bounded custody anchor.")
        custody, document, journal, actual = _load(directory, selected["revision"], selected["descriptor_sha256"])
        if not _same_toml(actual, selected):
            raise ValueError("The retained custody selector changed.")
        if journal["phase"] != "completed":
            if seen or _fence(cursor) != document["source_fence"]:
                raise ValueError("Historical admission custody is not completed.")
            _live_state(directory, custody, document, cursor, journal["phase"])
        elif not _same_toml(services.service_binding(cursor["service"]),
                services.service_binding(document["after_service"])):
            raise ValueError("The retained receipt differs from its completed custody.")
        seen.add(selected["revision"])
        content = _read(custody / "before/receipt.json", 0o400)
        prior = json.loads(content)
        _stopped(prior)
        stable = lambda value: {key: item for key, item in services.service_binding(value["service"]).items()
                                if key not in PROTOCOL_KEYS}
        if (_sha(content) != document["before_receipt_sha256"]
                or not _same_toml({key: prior.get(key) for key in OWNER_KEYS},
                    {key: receipt.get(key) for key in OWNER_KEYS})
                or not _same_toml(stable(prior), stable(receipt))
                or not _same_toml(prior.get("runtime_candidate"), selection)):
            raise ValueError("The retained admission belongs to another source or service owner.")
        bound = lambda value: {key: value.get(key) for key in updates.BOUND_KEYS}
        if _same_toml(bound(cursor), bound(prior)):
            if not _same_toml(cursor["update"], prior.get("update")):
                raise ValueError("An unchanged source hop changed its selected history.")
        elif (update.get("maintained_host_update") is None
                or prior.get("package_sha256") != update["previous_package_sha256"]
                or not _same_toml(bound(prior), json.loads(_read(root / "previous-receipt.json", 0o600)))):
            raise ValueError("The maintained host does not retain this exact predecessor.")
        cursor = prior
        root, update = _closed_candidate_update(directory, cursor, selection)
    return update["runtime_artifacts"]["native"]


def _ordinary_source(directory, receipt):
    _stopped(receipt)
    _, files = checked_package(directory, "package.zip", receipt["package_sha256"])
    verify_destination(receipt, files)
    definition = json.loads(files["deployment.json"][0])
    if receipt.get("source_kind") == "native-team":
        raise DraftConflict("Supervisor refresh preserves the exact ordinary 0.8.47 source before candidate publication.")
    if "runtimeCandidate" not in definition:
        if "runtime_candidate" in receipt or definition.get("mobkitVersion") != "0.8.47":
            raise DraftConflict("Supervisor refresh preserves the exact ordinary 0.8.47 source before candidate publication.")
        return
    # Refresh only the supervisor of an exact, previously admitted candidate.
    # Closed source history preserves original admission separately from the
    # present installed byte check. Neither admits or installs a new runtime.
    try:
        selection = _candidate_source_selection(files)
        framework = json.loads(files["implementation-report.json"][0])["framework"]
        update = receipt.get("update") or {}
        if (selection is None or not _same_toml(selection, receipt.get("runtime_candidate"))
                or not _same_toml(selection, framework.get("runtime_candidate"))
                or update.get("status") != "completed"
                or update.get("package_sha256") != receipt["package_sha256"]):
            raise ValueError("The candidate source has no matching completed admission.")
        admitted, _, checked = inspect_transaction(directory, update["update_id"])
        if (not _same_toml(checked, update)
                or not _same_toml(updates.inspection_binding(admitted), updates.inspection_binding(receipt))
                or not _same_toml(admitted.get("runtime_candidate"), selection)):
            raise ValueError("The candidate source differs from its retained admission.")
        frozen = _retained_candidate_admission(directory, admitted, selection)
        from .runtime_selection import parse_candidate_selection
        physical = verify_runtime(receipt["destination"], "0.8.48", runtime_target=parse_candidate_selection(selection))
        if (not _same_toml(physical, frozen)
                or "startup_candidate_runtime_proof" in receipt
                and not _same_toml(receipt["startup_candidate_runtime_proof"], frozen)):
            raise ValueError("The installed runtime differs from its original admission.")
        late = updates.require_inspection_current(directory, admitted, checked)
        carrier = lambda value: {"present": "startup_candidate_runtime_proof" in value,
            "value": value.get("startup_candidate_runtime_proof")}
        if not _same_toml(carrier(late), carrier(receipt)):
            raise ValueError("The retained startup runtime proof changed during verification.")
        _, late_files = checked_package(directory, "package.zip", admitted["package_sha256"])
        verify_destination(admitted, late_files)
    except (OSError, KeyError, ValueError, TypeError, UnicodeError, RecursionError, RuntimeDependencyError):
        raise DraftConflict("Supervisor refresh requires the exact retained candidate source and completed installed runtime admission.") from None


def _root(directory, revision):
    try:
        if not isinstance(revision, str) or str(uuid.UUID(revision)) != revision:
            raise ValueError
    except ValueError:
        raise DraftConflict("Choose the exact supervisor refresh revision.") from None
    return _directory(_directory(Path(directory) / "supervisor-refreshes") / revision)


def _load(directory, revision, expected_digest=None):
    root = _root(directory, revision)
    content = _read(root / "descriptor.json", 0o400)
    digest = _sha(content)
    if expected_digest is not None and digest != expected_digest:
        raise DraftConflict("The selected supervisor refresh descriptor changed.")
    document = json.loads(content)
    if (document.get("format") != "forge-supervisor-refresh-v1" or document.get("revision") != revision
            or document.get("directory") != str(Path(directory).absolute())):
        raise DraftConflict("The supervisor refresh belongs to different ownership.")
    old, new = document["before_service"], document["after_service"]
    if (service_digest(old) != document["before_service_digest"]
            or service_digest(new) != document["after_service_digest"]
            or {k: v for k, v in services.service_binding(old).items() if k not in PROTOCOL_KEYS}
            != {k: v for k, v in services.service_binding(new).items() if k not in PROTOCOL_KEYS}):
        raise DraftConflict("The supervisor refresh changes its preserved service identity.")
    for prefix, service in (("before", old), ("after", new)):
        _, manifest = _files(root / prefix / "host", document[prefix + "_manifest"], archived=True)
        if service["host_files"] != {name: row["sha256"] for name, row in manifest.items()}:
            raise DraftConflict("The retained supervisor manifest changed.")
        if json.loads(_read(root / prefix / "service.json", 0o400)) != service:
            raise DraftConflict("The retained service metadata changed.")
    prior = _read(root / "before/receipt.json", 0o400)
    unit = _read(root / "before/unit", 0o400)
    if (_sha(prior) != document["before_receipt_sha256"] or _fence(json.loads(prior)) != document["source_fence"]
            or services.service_binding(json.loads(prior)["service"]) != services.service_binding(old)
            or _sha(unit) != old["unit_sha256"]):
        raise DraftConflict("The retained source and service custody changed.")
    services.require_candidate_startup_protocol(new,
        message="The staged supervisor lacks its exact candidate startup contract.")
    services.require_update_protocol(new)
    journal = json.loads(_read(root / "journal.json", 0o600))
    if (journal.get("format") != "forge-supervisor-custody-v1" or journal.get("revision") != revision
            or journal.get("descriptor_sha256") != digest or journal.get("phase") not in PHASES):
        raise DraftConflict("The supervisor custody journal changed.")
    return root, document, journal, {"revision": revision, "descriptor_sha256": digest}


def _selector(receipt, selected):
    if receipt.get("supervisor_refresh") != selected:
        raise DraftConflict("A different supervisor refresh is selected.")


def _projection(receipt):
    projection = deepcopy(receipt)
    projection.pop("supervisor_refresh", None)
    return projection


def _live_state(directory, root, document, receipt, phase):
    live, staged, retained = Path(directory) / "service-host", root / "staged-host", root / "retained-host"
    old, new = document["before_manifest"], document["after_manifest"]
    live_kind = None
    if live.exists() or live.is_symlink():
        _, actual = _files(live)
        live_kind = "before" if actual == old else "after" if actual == new else "foreign"
    staged_kind = "after" if (staged.exists() or staged.is_symlink()) and _files(staged, new)[1] == new else None
    retained_kind = "before" if (retained.exists() or retained.is_symlink()) and _files(retained, old)[1] == old else None
    binding = services.service_binding(receipt.get("service") or {})
    receipt_kind = ("before" if binding == services.service_binding(document["before_service"])
        else "after" if binding == services.service_binding(document["after_service"]) else "foreign")
    state = (live_kind, staged_kind, retained_kind, receipt_kind)
    states = {
        "prepared": {("before", "after", None, "before")},
        "retaining": {("before", "after", None, "before"), (None, "after", "before", "before")},
        "retained": {(None, "after", "before", "before")},
        "installing": {(None, "after", "before", "before"), ("after", None, "before", "before")},
        "installed": {("after", None, "before", "before")},
        "publishing": {("after", None, "before", "before"), ("after", None, "before", "after")},
        "completed": {("after", None, "before", "after")},
    }
    if state not in states[phase]:
        raise DraftConflict("The supervisor custody state is not an exact before/after transition.")
    return state


def _phase(root, journal, phase):
    changed = {**journal, "phase": phase, "updated_at": now()}
    write_private(root / "journal.json", changed)
    _sync(root)
    return changed


def require_complete(directory, receipt):
    selected = receipt.get("supervisor_refresh")
    if not isinstance(selected, dict) or set(selected) != {"revision", "descriptor_sha256"}:
        raise DraftConflict("The supervisor refresh selection is invalid.")
    root, document, journal, actual = _load(directory, selected["revision"], selected["descriptor_sha256"])
    if (actual != selected or journal["phase"] != "completed"
            or services.service_binding(receipt.get("service") or {}) != services.service_binding(document["after_service"])):
        raise DraftConflict("Complete the exact supervisor custody refresh before any service admission.")


def inspect(directory, revision, *, expected_digest=None):
    root, document, journal, selected = _load(directory, revision, expected_digest)
    if journal["phase"] != "completed":
        receipt = read_receipt(directory)
        _selector(receipt, selected)
        if _fence(receipt) != document["source_fence"]:
            raise DraftConflict("The installed source or stopped service instance changed during refresh.")
        _ordinary_source(directory, receipt)
        _live_state(directory, root, document, receipt, journal["phase"])
    return _custody_projection(document, journal, selected)


def _custody_projection(document, journal, selected):
    return {**selected, "phase": journal["phase"], "before_service_digest": document["before_service_digest"],
        "after_service_digest": document["after_service_digest"], "retained_files": len(document["before_manifest"]),
        "staged_files": len(document["after_manifest"])}


def pending_custody(directory, receipt):
    """An unfinished or unverifiable selector must bypass normal health reads."""
    if "supervisor_refresh" not in receipt:
        return False
    try:
        selected = receipt["supervisor_refresh"]
        _, _, journal, checked = _load(directory, selected["revision"], selected["descriptor_sha256"])
        _selector(receipt, checked)
        return journal["phase"] != "completed"
    except (OSError, ValueError, KeyError, TypeError, DraftConflict):
        return True


def _trusted_files(directory=None, service=None):
    selected = (services.retained_bootstrap_owner_source_sha256(directory, service)
        if directory is not None else None)
    files = services.supervisor_source_files(**({"bootstrap_owner_source_sha256": selected}
        if selected is not None else {}))
    if not isinstance(files, dict) or not files or len(files) > 256 or sum(map(len, files.values())) > MAX_TOTAL:
        raise DraftConflict("The trusted supervisor snapshot exceeded its bounds.")
    for name, content in files.items():
        _relative(name)
        if not isinstance(content, bytes) or len(content) > MAX_FILE:
            raise DraftConflict("The trusted supervisor snapshot contains unsupported bytes.")
    return files


def _support_binding(receipt):
    return {"source": _fence(receipt), "inspection": updates.inspection_binding(receipt),
        "status": receipt.get("status"), "runtime_pid": receipt.get("runtime_pid"),
        "startup_proof": {"present": "startup_candidate_runtime_proof" in receipt,
            "value": receipt.get("startup_candidate_runtime_proof")}}


def _support_metadata(directory, receipt):
    """Check unit and custody bytes without repeating the physical runtime proof."""
    selected, projection = receipt.get("supervisor_refresh"), None
    if selected is not None:
        root, document, journal, checked = _load(directory, selected["revision"], selected["descriptor_sha256"])
        _selector(receipt, checked)
        if journal["phase"] == "completed":
            require_complete(directory, receipt)
        else:
            if _fence(receipt) != document["source_fence"]:
                raise DraftConflict("The installed source or stopped service instance changed during refresh.")
            _live_state(directory, root, document, receipt, journal["phase"])
        projection = _custody_projection(document, journal, checked)
    service = (document["before_service"] if selected and journal["phase"] != "completed"
               else services.verify_service(directory, receipt))
    unit = services.private_file(service["unit_path"])
    if selected and journal["phase"] != "completed" and unit != _read(root / "before/unit", 0o400):
        raise DraftConflict("The preserved service unit changed.")
    return service, unit, projection


async def support_projection(directory, manager, *, require_current=None):
    """Checked public custody projection; never reconcile or publish a receipt."""
    receipt = read_receipt(directory)
    result = {"scope": "owned-stopped-supervisor-refresh", "requires_stopped": True,
        "deployment_id": receipt["deployment_id"], "build_id": receipt.get("build_id"),
        "package_sha256": receipt["package_sha256"], "available": False,
        "prepare_available": False, "apply_available": False, "service_digest": None,
        "stopped_verified": False, "phase": "unavailable", "selected": None}
    try:
        if require_current:
            require_current()
        if receipt.get("execution_mode") != "background" or receipt.get("source_kind") == "native-team":
            raise DraftConflict("Installation support updates require an owned background application.")
        binding = _support_binding(receipt)
        service, unit, projection = await await_validation(_support_metadata, directory, receipt)
        if projection is not None:
            result.update(selected=projection, phase=projection["phase"])
        _stopped(receipt)
        result["service_digest"] = service_digest(service)
        if require_current:
            require_current()
        if not _same_toml(_support_binding(read_receipt(directory)), binding):
            raise DraftConflict("The installation changed while checking its support update.")
        observation = await manager.observe(service)
        if require_current:
            require_current()
        current = read_receipt(directory)
        if not _same_toml(_support_binding(current), binding):
            raise DraftConflict("The installation changed while checking its support update.")
        # One complete proof follows the manager await. It retains its own late
        # installed SDK/native and source fences and drains on cancellation.
        await await_validation(_ordinary_source, directory, current)
        if require_current:
            require_current()
        current = read_receipt(directory)
        _stopped(current)
        if not _same_toml(_support_binding(current), binding):
            raise DraftConflict("The installation changed while checking its support update.")
        # The worker handback is an added scheduling boundary. Recheck exact
        # package, installed source, unit and custody bytes after that boundary.
        _, files = checked_package(directory, "package.zip", current["package_sha256"])
        verify_destination(current, files)
        fresh_service, fresh_unit, fresh_projection = _support_metadata(directory, current)
        if (fresh_unit != unit or not _same_toml(fresh_projection, projection)
                or service_digest(fresh_service) != result["service_digest"]):
            raise DraftConflict("The selected support update changed while checking it.")
        if not services.manager_stopped(observation):
            raise DraftConflict("Stop this application and confirm its cleanup before updating installation support.")
        if projection is not None and projection["phase"] != "completed":
            result.update(available=True, apply_available=True)
        else:
            proposed = services.supervisor_metadata(service, _trusted_files(directory, service))
            if services.service_binding(proposed) != services.service_binding(service):
                result.update(available=True, prepare_available=True)
                if projection is None:
                    result["phase"] = "available"
            elif projection is None:
                result["phase"] = "current"
        result["stopped_verified"] = True
        return result
    except (OSError, ValueError, KeyError, TypeError, DraftConflict):
        result.update(available=False, prepare_available=False, apply_available=False,
            stopped_verified=False,
            reason="The owned stopped installation support could not be verified. Preserve this installation and check its deployment activity.")
        return result


async def require_selected_stopped(directory, revision, manager, *, expected_digest, require_current=None):
    """Admit explicit continuation of one selected frozen custody, without writes."""
    root, document, journal, selected = _load(directory, revision, expected_digest)
    receipt = read_receipt(directory)
    _selector(receipt, selected)
    _ordinary_source(directory, receipt)
    if _fence(receipt) != document["source_fence"]:
        raise DraftConflict("The installed source or stopped service instance changed during refresh.")
    checked = inspect(directory, revision, expected_digest=expected_digest)
    service = document["before_service"] if journal["phase"] != "completed" else services.verify_service(directory, receipt)
    if services.private_file(service["unit_path"]) != _read(root / "before/unit", 0o400):
        raise DraftConflict("The preserved service unit changed.")
    if require_current:
        require_current()
    observation = await manager.observe(service)
    if require_current:
        require_current()
    current = read_receipt(directory)
    _ordinary_source(directory, current)
    _selector(current, selected)
    if (_fence(current) != document["source_fence"]
            or inspect(directory, revision, expected_digest=expected_digest) != checked
            or services.private_file(service["unit_path"]) != _read(root / "before/unit", 0o400)
            or not services.manager_stopped(observation)):
        raise DraftConflict("The exact stopped support custody changed while admitting continuation.")
    return checked


def prepare(directory, expected_service_digest, *, revision=None):
    directory = _directory(Path(directory).absolute())
    receipt = read_receipt(directory)
    _ordinary_source(directory, receipt)
    if revision is not None:
        try:
            if type(revision) is not str or str(uuid.UUID(revision)) != revision:
                raise ValueError
        except (ValueError, TypeError, AttributeError):
            raise DraftConflict("Choose the exact supervisor refresh revision.") from None
        selected = receipt.get("supervisor_refresh")
        if isinstance(selected, dict) and selected.get("revision") == revision:
            _, document, journal, checked = _load(directory, revision, selected["descriptor_sha256"])
            _selector(receipt, checked)
            if (document["before_service_digest"] != expected_service_digest
                    or _fence(receipt) != document["source_fence"]):
                raise DraftConflict("The exact prepared support request changed.")
            if journal["phase"] == "completed":
                require_complete(directory, receipt)
                services.verify_service(directory, receipt)
            return inspect(directory, revision, expected_digest=checked["descriptor_sha256"])
        # A crash before selection can leave incomplete bytes. Never search,
        # replace them, or mint a different custody for this exact request.
        store = directory / "supervisor-refreshes"
        if store.exists() and ((store / revision).exists() or (store / revision).is_symlink()):
            raise DraftConflict("This support preparation was interrupted before selection. Preserve its exact custody; no replacement was prepared.")
    service = services.verify_service(directory, receipt)
    if service_digest(service) != expected_service_digest:
        raise DraftConflict("The selected supervisor service binding changed.")
    files = _trusted_files(directory, service)
    proposed = services.supervisor_metadata(service, files)
    if services.service_binding(proposed) == services.service_binding(service):
        raise DraftConflict("This exact supervisor source is already installed.")
    with project_lock(Path(receipt["destination"])), receipt_lock(directory):
        current = read_receipt(directory)
        _ordinary_source(directory, current)
        if _fence(current) != _fence(receipt) or services.service_binding(services.verify_service(directory, current)) != services.service_binding(service):
            raise DraftConflict("The source or service changed while preparing its supervisor refresh.")
        old_files, old_manifest = _files(directory / "service-host")
        store = directory / "supervisor-refreshes"
        store.mkdir(mode=0o700, exist_ok=True)
        _directory(store)
        revision = revision or str(uuid.uuid4())
        root = store / revision
        root.mkdir(mode=0o700)
        before, after = root / "before", root / "after"
        before.mkdir(mode=0o700); after.mkdir(mode=0o700)
        _tree(before / "host", old_files, archived=True)
        new_manifest = _tree(after / "host", files, archived=True)
        _tree(root / "staged-host", files)
        prior = _encoded(current)
        _write(before / "receipt.json", prior, 0o400)
        _write(before / "unit", services.private_file(service["unit_path"]), 0o400)
        _write(before / "service.json", _encoded(service), 0o400)
        _write(after / "service.json", _encoded(proposed), 0o400)
        document = {"format": "forge-supervisor-refresh-v1", "revision": revision, "directory": str(directory),
            "source_fence": _fence(current), "before_service": service, "after_service": proposed,
            "before_service_digest": service_digest(service), "after_service_digest": service_digest(proposed),
            "before_manifest": old_manifest, "after_manifest": new_manifest, "before_receipt_sha256": _sha(prior)}
        content = _encoded(document)
        _write(root / "descriptor.json", content, 0o400)
        selected = {"revision": revision, "descriptor_sha256": _sha(content)}
        write_private(root / "journal.json", {"format": "forge-supervisor-custody-v1", **selected, "phase": "prepared", "updated_at": now()})
        _load(directory, revision, selected["descriptor_sha256"])
        for path in (before, after, root, store, directory):
            _sync(path)
        write_private(directory / "receipt.json", {**current, "supervisor_refresh": selected})
        _sync(directory)
    return inspect(directory, revision, expected_digest=selected["descriptor_sha256"])


async def apply(directory, revision, manager, *, expected_digest, require_current=None):
    directory = _directory(Path(directory).absolute())
    root, document, journal, selected = _load(directory, revision, expected_digest)
    receipt = read_receipt(directory)
    if require_current:
        require_current()
    _selector(receipt, selected)
    if journal["phase"] == "completed":
        if require_current:
            await require_selected_stopped(directory, revision, manager,
                expected_digest=expected_digest, require_current=require_current)
        require_complete(directory, receipt)
        services.verify_service(directory, receipt)
        return inspect(directory, revision, expected_digest=expected_digest)
    _ordinary_source(directory, receipt)
    if _fence(receipt) != document["source_fence"]:
        raise DraftConflict("The installed source or stopped service instance changed during refresh.")
    _live_state(directory, root, document, receipt, journal["phase"])
    observation = await manager.observe(document["before_service"])
    if require_current:
        require_current()
    if not services.manager_stopped(observation):
        raise DraftConflict("The exact OS service is not stopped. No supervisor source was changed.")
    with project_lock(Path(receipt["destination"])):
        observation = await manager.observe(document["before_service"])
        if require_current:
            require_current()
        if not services.manager_stopped(observation):
            raise DraftConflict("The exact OS service changed before supervisor publication.")
        with receipt_lock(directory):
            if require_current:
                require_current()
            current = read_receipt(directory)
            _selector(current, selected)
            _ordinary_source(directory, current)
            if _fence(current) != document["source_fence"]:
                raise DraftConflict("The installed source or stopped service instance changed during refresh.")
            root, document, journal, _ = _load(directory, revision, expected_digest)
            if journal["phase"] == "completed":
                require_complete(directory, current)
                services.verify_service(directory, current)
                return inspect(directory, revision, expected_digest=expected_digest)
            state = _live_state(directory, root, document, current, journal["phase"])
            if services.private_file(document["before_service"]["unit_path"]) != _read(root / "before/unit", 0o400):
                raise DraftConflict("The preserved service unit changed. No supervisor source was published.")
            if state[0] == "before":
                services.verify_service(directory, _projection(current))
                journal = _phase(root, journal, "retaining")
                (directory / "service-host").rename(root / "retained-host")
                _sync(directory); _sync(root)
                journal = _phase(root, journal, "retained")
            if not (directory / "service-host").exists():
                journal = _phase(root, journal, "installing")
                (root / "staged-host").rename(directory / "service-host")
                _sync(directory); _sync(root)
                journal = _phase(root, journal, "installed")
            _files(directory / "service-host", document["after_manifest"])
            proposed = deepcopy(document["after_service"])
            if "public_summary" in current["service"]:
                proposed["public_summary"] = deepcopy(current["service"]["public_summary"])
            else:
                proposed.pop("public_summary", None)
            services.verify_service(directory, _projection({**current, "service": proposed}))
            journal = _phase(root, journal, "publishing")
            write_private(directory / "receipt.json", {**current, "service": proposed})
            _sync(directory)
            journal = _phase(root, journal, "completed")
            require_complete(directory, read_receipt(directory))
    return inspect(directory, revision, expected_digest=expected_digest)
