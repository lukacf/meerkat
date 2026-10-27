"""Initialize a new Forge project's storage using MobKit's native authority."""
from __future__ import annotations

import asyncio
from contextlib import contextmanager
import errno
import fcntl
import inspect
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import tempfile
import uuid


def _open_project_directory(root: Path) -> int:
    """Traverse every component without following a symlink."""
    descriptor = os.open(root.anchor, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        for component in root.parts[1:]:
            child = os.open(component, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW,
                            dir_fd=descriptor)
            os.close(descriptor)
            descriptor = child
        return descriptor
    except BaseException:
        os.close(descriptor)
        raise


def _project_directory_safe(info) -> bool:
    return (stat.S_ISDIR(info.st_mode) and info.st_uid == os.getuid()
            and not info.st_mode & 0o022)


def require_project_directory(root: Path) -> None:
    """Check the project folder without creating state or acquiring its lock."""
    error = "Unsafe project lock location. Preserve existing state and private project permissions."
    descriptor = None
    try:
        descriptor = _open_project_directory(Path(root).absolute())
        info = os.fstat(descriptor)
        if not _project_directory_safe(info):
            if stat.S_ISDIR(info.st_mode) and info.st_uid == os.getuid() and info.st_mode & 0o022:
                raise RuntimeError(
                    "This owned project folder allows group or other write access.\n"
                    "Installation stopped before environment or application setup.\n"
                    "Run this non-recursive repair in this same project folder:\n"
                    "  chmod go-w .\n"
                    "Then retry the same install command from this folder.\n"
                    "Keep .venv, .env, workspace, data and .mobkit in place.")
            raise RuntimeError(error)
    except OSError:
        raise RuntimeError(error) from None
    finally:
        if descriptor is not None:
            os.close(descriptor)


def _project_lock_identity(info) -> tuple[int, int]:
    return info.st_dev, info.st_ino


def _close_project_descriptor(descriptor: int, identity: tuple[int, int]) -> None:
    try:
        if _project_lock_identity(os.fstat(descriptor)) == identity:
            os.close(descriptor)
    except OSError as failure:
        if failure.errno != errno.EBADF:
            raise


class ProjectLockLease:
    """Currentness of the original project flock, valid only inside its context.

    This token never opens or reacquires another lock. Native authority and
    source selection remain the caller's separate responsibilities.
    """

    def __init__(self, root: Path, root_descriptor: int, directory_descriptor: int,
                 descriptor: int, guard_descriptor: int):
        self._root = root
        self._root_descriptor = root_descriptor
        self._directory_descriptor = directory_descriptor
        self._descriptor = descriptor
        self._guard_descriptor = guard_descriptor
        self._root_identity = _project_lock_identity(os.fstat(root_descriptor))
        self._directory_identity = _project_lock_identity(os.fstat(directory_descriptor))
        self._lock_identity = _project_lock_identity(os.fstat(descriptor))
        self._pid = os.getpid()
        self._closed = False
        self._invalidated = False

    @property
    def root(self) -> Path:
        return self._root

    def require_current(self) -> None:
        """Refuse lost descriptor custody, replacement, or changed private access."""
        error = "The project lock is no longer current. Preserve existing state and restart the app."
        if self._closed or self._invalidated or os.getpid() != self._pid:
            raise RuntimeError(error)
        current_root = None
        try:
            root_info = os.fstat(self._root_descriptor)
            directory_info = os.fstat(self._directory_descriptor)
            lock_info = os.fstat(self._descriptor)
            guard_info = os.fstat(self._guard_descriptor)
            if (not _project_directory_safe(root_info)
                    or _project_lock_identity(root_info) != self._root_identity
                    or not stat.S_ISDIR(directory_info.st_mode)
                    or directory_info.st_uid != os.getuid() or directory_info.st_mode & 0o077
                    or _project_lock_identity(directory_info) != self._directory_identity
                    or not stat.S_ISREG(lock_info.st_mode) or lock_info.st_uid != os.getuid()
                    or lock_info.st_mode & 0o077 or lock_info.st_nlink != 1
                    or _project_lock_identity(lock_info) != self._lock_identity
                    or _project_lock_identity(guard_info) != self._lock_identity):
                raise ValueError
            current_root = _open_project_directory(self._root)
            if (_project_lock_identity(os.fstat(current_root)) != self._root_identity
                    or _project_lock_identity(os.stat(".mobkit", dir_fd=current_root,
                                                     follow_symlinks=False)) != self._directory_identity):
                raise ValueError
            current_lock = os.stat("launch.lock", dir_fd=self._directory_descriptor,
                                   follow_symlinks=False)
            if (not stat.S_ISREG(current_lock.st_mode)
                    or _project_lock_identity(current_lock) != self._lock_identity):
                raise ValueError
        except (OSError, ValueError):
            self._invalidated = True
            raise RuntimeError(error) from None
        finally:
            if current_root is not None:
                os.close(current_root)


@contextmanager
def project_lock(root: Path):
    """Hold one flock and optionally yield its synchronous currentness token."""
    root = Path(root).absolute()
    root_descriptor = directory_descriptor = descriptor = None
    held_descriptors = []
    lease = None
    error = "Unsafe project lock location. Preserve existing state and private project permissions."
    try:
        try:
            root_descriptor = _open_project_directory(root)
            held_descriptors.append((root_descriptor, _project_lock_identity(os.fstat(root_descriptor))))
            root_info = os.fstat(root_descriptor)
            if not _project_directory_safe(root_info):
                raise ValueError
            try:
                os.mkdir(".mobkit", mode=0o700, dir_fd=root_descriptor)
            except FileExistsError:
                pass
            directory_descriptor = os.open(".mobkit", os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW,
                                           dir_fd=root_descriptor)
            held_descriptors.append((directory_descriptor, _project_lock_identity(os.fstat(directory_descriptor))))
            info = os.fstat(directory_descriptor)
            if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077:
                raise ValueError
            descriptor = os.open("launch.lock", os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW | os.O_NONBLOCK,
                                 0o600, dir_fd=directory_descriptor)
            held_descriptors.append((descriptor, _project_lock_identity(os.fstat(descriptor))))
            info = os.fstat(descriptor)
            if (not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid()
                    or info.st_mode & 0o077 or info.st_nlink != 1):
                raise ValueError
        except (OSError, ValueError):
            raise RuntimeError(error) from None
        try:
            fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            raise RuntimeError("This project is already running. Stop its other process first.") from None
        # A dup shares this original flock, rather than acquiring another one.
        # Keep ownership if a private numeric descriptor is accidentally recycled.
        guard_descriptor = os.dup(descriptor)
        held_descriptors.append((guard_descriptor, _project_lock_identity(os.fstat(guard_descriptor))))
        lease = ProjectLockLease(root, root_descriptor, directory_descriptor, descriptor, guard_descriptor)
        lease.require_current()
        yield lease
    finally:
        if lease is not None:
            lease._closed = True
        for held, identity in reversed(held_descriptors):
            _close_project_descriptor(held, identity)


def write_private(path: Path, value: dict):
    temporary = path.with_suffix(".pending")
    descriptor = os.open(temporary, os.O_CREAT | os.O_TRUNC | os.O_WRONLY, 0o600)
    with os.fdopen(descriptor, "w") as stream:
        json.dump(value, stream, indent=2)
        stream.write("\n")
        stream.flush()
        os.fsync(stream.fileno())
    temporary.replace(path)


def native_storage(root: Path) -> dict:
    """The maintenance tool, product name and continuity ledger of this package.

    A 0.9 package runs the public Meta release; a 0.8 package keeps MobKit.
    """
    version = json.loads((root / "deployment.json").read_text())["mobkitVersion"]
    try:
        line = tuple(int(part) for part in version.split(".")[:2])
    except (AttributeError, ValueError):
        raise RuntimeError("Unrecognized package runtime version. Preserve the state and inspect deployment.json.") from None
    if line >= (0, 9):
        return {"tool": "meta_gateway", "name": "Meta", "version": version, "continuity": "meta-continuity"}
    return {"tool": "mobkit_gateway", "name": "MobKit", "version": version, "continuity": "mobkit-continuity"}


PREDECESSOR_STATE = (
    "This project's .mobkit/state holds MobKit 0.8 state, which this Meta {version} package does not "
    "open. No data was changed. Keep the state with its original MobKit package, or carry it over "
    "once with Meta's own storage tools while the app is stopped and backed up, starting with "
    ".bin/meta_gateway storage-adopt-predecessor --state-dir .mobkit/state as Meta's upgrade guide "
    "describes.")


def migration(root: Path, *, apply: bool, report_path: Path | None = None) -> dict:
    native = native_storage(root)
    binary = root / ".bin" / native["tool"]
    if not binary.is_file():
        raise RuntimeError("The storage setup tool is missing. Run bash install.sh first.")
    command = [str(binary), "storage-migrate", "--state-dir", str(root / ".mobkit/state"), "--json"]
    if apply:
        command.append("--apply")
    try:
        result = subprocess.run(command, capture_output=True, text=True, timeout=120, check=False)
        report = json.loads(result.stdout)
    except (OSError, subprocess.TimeoutExpired, json.JSONDecodeError):
        raise RuntimeError(f"{native['name']} could not prepare storage. No existing data was deleted. Retry startup.") from None
    if not isinstance(report, dict):
        raise RuntimeError(f"{native['name']} returned an invalid storage report.")
    destination = report_path or root / ".mobkit/storage-initialization.json"
    write_private(destination, report)
    errors = report.get("errors")
    # Meta refuses predecessor state before it reads any store; its error
    # names the adoption verb that carries the state over.
    if (native["tool"] == "meta_gateway" and isinstance(errors, list)
            and any(isinstance(error, str) and "storage-adopt-predecessor" in error for error in errors)):
        raise RuntimeError(PREDECESSOR_STATE.format(version=native["version"]))
    backfill = report.get("head_canonical_backfill") or {}
    if (not isinstance(backfill, dict) or result.returncode or report.get("errors")
            or backfill.get("failures") or backfill.get("skipped_unparseable") or backfill.get("vanished")):
        raise RuntimeError(f"{native['name']} storage preparation needs attention. Inspect {destination.relative_to(root)}.")
    return report


def current_storage(report: dict, continuity: str = "mobkit-continuity") -> bool:
    return any(entry.get("domain") == continuity and
               (entry.get("after") or entry.get("before") or 0) >= 2
               for entry in report.get("ledger", []))


def _upgrade_report(report: dict, state: Path, *, apply: bool) -> bool:
    """Validate the bounded supported native report; return whether dry-run needs apply."""
    error = "MobKit did not confirm complete upgrade storage. Preserve its retained native report."
    # Exact stampable ceilings shared by MobKit .46/.47/.48 with Core .51. Recorded
    # means a ledger exists, not that its native migrations are complete.
    ceilings = {"mobkit-continuity": 2, "mobkit-metadata": 1,
                "mobkit-console": 3, "mobkit-memory": 3, "jobs": 4}
    if (not isinstance(report, dict) or report.get("mode") != ("apply" if apply else "dry_run")
            or not isinstance(report.get("state_dir"), str)
            or Path(report["state_dir"]).resolve() != state.resolve()
            or report.get("errors") != [] or not isinstance(report.get("ledger"), list)
            or not report["ledger"]):
        raise RuntimeError(error)
    continuity, seen, needed = [], set(), False
    for entry in report["ledger"]:
        if (not isinstance(entry, dict) or not isinstance(entry.get("database"), str)
                or not isinstance(entry.get("domain"), str) or not entry["domain"]
                or not Path(entry["database"]).resolve().is_relative_to(state.resolve())):
            raise RuntimeError(error)
        key = (entry["database"], entry["domain"])
        if key in seen:
            raise RuntimeError(error)
        seen.add(key)
        for field in ("before", "after"):
            value = entry.get(field)
            if value is not None and (type(value) is not int or value < 0):
                raise RuntimeError(error)
        action = entry.get("action")
        allowed = {"stamped", "already-current", "report-only", "exempt"} if apply else {
            "recorded", "would-stamp", "report-only", "exempt"}
        if action not in allowed:
            raise RuntimeError(error)
        version = entry.get("after") if apply else entry.get("before")
        if action not in {"report-only", "exempt"}:
            if (apply or action == "recorded") and (type(version) is not int or version < 1):
                raise RuntimeError(error)
            needed |= action == "would-stamp"
        ceiling = ceilings.get(entry["domain"])
        if ceiling is not None:
            if (action in {"report-only", "exempt"} or version is not None and version > ceiling
                    or apply and version != ceiling):
                raise RuntimeError(error)
            needed |= version != ceiling
        elif action not in {"report-only", "exempt"}:
            raise RuntimeError(error)
        if entry["domain"] == "mobkit-continuity":
            continuity.append(version)
    if not continuity or apply and any(version != 2 for version in continuity):
        raise RuntimeError(error)
    backfill = report.get("head_canonical_backfill")
    lists = ("converted", "reconverted", "failures", "vanished", "skipped_unparseable")
    if (not isinstance(backfill, dict) or type(backfill.get("examined")) is not int
            or backfill["examined"] < 0 or any(not isinstance(backfill.get(key), list) for key in lists)
            or any(backfill[key] for key in ("failures", "vanished", "skipped_unparseable"))
            or backfill.get("applied") is not apply or type(backfill.get("ledger_stamped")) is not bool
            or apply and backfill["ledger_stamped"] is not True
            or not apply and (backfill["ledger_stamped"] or backfill["converted"] or backfill["reconverted"])):
        raise RuntimeError(error)
    # Native .46 examines every legacy blob, including already current heads.
    # Its ledger stamp, not examined/converted counts, confirms the crossing.
    for field, expected in (("renames", "renamed" if apply else "would-rename"), ("twins", None)):
        rows = report.get(field)
        if not isinstance(rows, list):
            raise RuntimeError(error)
        for row in rows:
            if not isinstance(row, dict):
                raise RuntimeError(error)
            if field == "renames":
                if row.get("action") != expected:
                    raise RuntimeError(error)
            elif (row.get("errors") != [] or row.get("byte_identical") is not True
                    or not isinstance(row.get("resolution"), dict)
                    or row["resolution"].get("kind") != "deduped"):
                raise RuntimeError(error)
        needed |= bool(rows)
    return needed


def upgrade_storage(root: Path, from_version: str, to_version: str, *,
                    require_owner_current=None) -> dict:
    """Explicit dependency upgrade under the caller's stopped project lease.

    The worker must already have marked native activity forward-only. This
    helper neither starts agents nor acquires a second lease or rewinds state.
    The optional synchronous caller fence gates each new native operation.
    Once apply is entered, retain its result and publish the confirmed marker.
    """
    def current():
        if require_owner_current is None:
            return
        error = "Storage upgrade needs its original synchronous owner fence."
        if not callable(require_owner_current):
            raise RuntimeError(error)
        result = require_owner_current()
        if inspect.isawaitable(result):
            if inspect.iscoroutine(result):
                result.close()
            raise RuntimeError(error)
        if result is False:
            raise RuntimeError("The storage upgrade owner is no longer current. Preserve existing state.")

    if (from_version, to_version) not in {("0.8.42", "0.8.46"), ("0.8.42", "0.8.47"), ("0.8.46", "0.8.47"),
            ("0.8.42", "0.8.48"), ("0.8.46", "0.8.48"), ("0.8.47", "0.8.48")}:
        raise RuntimeError("This storage dependency upgrade is not supported.")
    root = Path(root)
    marker, state = root / ".mobkit/launch-storage.json", root / ".mobkit/state"
    error = "Unrecognized upgrade storage marker or project. Preserve existing state."
    def unique(pairs):
        result = {}
        for key, value in pairs:
            if key in result:
                raise ValueError
            result[key] = value
        return result
    try:
        for path in (root / "deployment.json", marker):
            info = path.lstat()
            if (not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_nlink != 1
                    or not 0 < info.st_size <= (64_000 if path == marker else 4 * 1024 * 1024)
                    or any(parent.is_symlink() for parent in path.parents)):
                raise ValueError
        if (not state.is_dir() or state.is_symlink() or state.stat().st_uid != os.getuid()
                or not any(state.iterdir())):
            raise ValueError
        deployment = json.loads((root / "deployment.json").read_text(), object_pairs_hook=unique)
        original = marker.read_bytes()
        provenance = json.loads(original, object_pairs_hook=unique)
        slug = deployment.get("slug") if isinstance(deployment, dict) else None
        if (not isinstance(slug, str) or not re.fullmatch(r"[a-z][a-z0-9-]{1,39}", slug)
                or deployment.get("mobkitVersion") != to_version or not isinstance(provenance, dict)
                or set(provenance) != {"format", "project", "version", "phase"}
                or type(provenance["format"]) is not int or provenance["format"] != 1
                or provenance["project"] != slug or provenance["version"] != from_version
                or provenance["phase"] != "ready"):
            raise ValueError
    except (OSError, ValueError, TypeError):
        raise RuntimeError(error) from None
    reports = root / ".mobkit/storage-upgrades"
    reports.mkdir(mode=0o700, exist_ok=True)
    if (reports.is_symlink() or reports.stat().st_uid != os.getuid()
            or reports.stat().st_mode & 0o077):
        raise RuntimeError(error)
    attempt = reports / uuid.uuid4().hex
    attempt.mkdir(mode=0o700)
    read_path, apply_path = attempt / "read.json", attempt / "apply.json"
    current()
    report = migration(root, apply=False, report_path=read_path)
    needed = _upgrade_report(report, state, apply=False)
    current()
    if needed:
        report = migration(root, apply=True, report_path=apply_path)
        _upgrade_report(report, state, apply=True)
    info = marker.lstat()
    if (not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_nlink != 1
            or marker.read_bytes() != original):
        raise RuntimeError("The storage marker changed during native upgrade. Preserve state and native reports.")
    binding = {"format": 1, "project": slug, "version": to_version, "phase": "ready"}
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(mode="w", dir=marker.parent,
                                         prefix=".launch-storage-upgrade-", delete=False) as stream:
            temporary = Path(stream.name)
            os.fchmod(stream.fileno(), 0o600)
            json.dump(binding, stream, indent=2)
            stream.write("\n")
            stream.flush()
            os.fsync(stream.fileno())
        temporary.replace(marker)
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)
    return {"from_version": from_version, "to_version": to_version,
            "read_report": str(read_path), "apply_report": str(apply_path) if needed else None,
            "applied": needed, "marker": binding}


async def prepare_storage(root: Path, build_runtime):
    """Run once, before user interaction. Never write native database tables."""
    native = native_storage(root)
    state = root / ".mobkit/state"
    state.mkdir(parents=True, exist_ok=True, mode=0o700)
    marker = root / ".mobkit/launch-storage.json"
    deployment = json.loads((root / "deployment.json").read_text())
    binding = {"format": 1, "project": deployment["slug"], "version": deployment["mobkitVersion"]}
    provenance = None
    if marker.exists():
        try:
            provenance = json.loads(marker.read_text())
        except (OSError, json.JSONDecodeError):
            raise RuntimeError("Unreadable Forge storage marker. Inspect .mobkit/launch-storage.json.") from None
        if (not isinstance(provenance, dict) or
            any(provenance.get(key) != value for key, value in binding.items()) or
            provenance.get("phase") not in {"preparing", "ready"}):
            raise RuntimeError("Unrecognized Forge storage marker. Preserve the state and inspect .mobkit/launch-storage.json.")
    nonempty = any(state.iterdir())
    if not nonempty:
        provenance = None  # A marker cannot vouch for absent/replaced storage.
    if nonempty and (not provenance or provenance["phase"] == "ready"):
        # Always inspect through MobKit before trusting ready state. This also
        # covers restored backups without treating the marker as schema truth.
        report = await asyncio.to_thread(migration, root, apply=False)
        if not current_storage(report, native["continuity"]):
            raise RuntimeError(
                f"Existing storage needs an explicit {native['name']} migration. Stop the app, back up .mobkit/state, "
                f"then run .bin/{native['tool']} storage-migrate --state-dir .mobkit/state --apply. "
                "Start again after that command succeeds."
            )
        write_private(marker, {**binding, "phase": "ready"})
        return
    print("Preparing your team's durable storage (first start only)...", flush=True)
    if not provenance:
        write_private(marker, {**binding, "phase": "preparing"})
    if nonempty:
        # Resume an interrupted initialization through the same native tool.
        report = await asyncio.to_thread(migration, root, apply=True)
        if current_storage(report, native["continuity"]):
            write_private(marker, {**binding, "phase": "ready"})
            return
    runtime = await build_runtime()
    try:
        status = await runtime.identity_bootstrap_status()
        if not status.complete or not status.ready or status.counts.broken:
            raise RuntimeError("Initial agent setup did not finish. Inspect the gateway log and retry startup.")
    finally:
        await runtime.shutdown()
    report = await asyncio.to_thread(migration, root, apply=True)
    if not current_storage(report, native["continuity"]):
        raise RuntimeError(f"{native['name']} did not confirm current storage. Inspect .mobkit/storage-initialization.json.")
    write_private(marker, {**binding, "phase": "ready"})
