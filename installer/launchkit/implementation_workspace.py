"""Private implementation projects and host-issued, content-bound verification.

The compiler owns the deployment framework. An implementation agent can read it
and add ordinary project files, but cannot replace it or write verification
receipts. Executable checks use a fresh copy with a clean environment inside
macOS sandbox-exec or verified Linux bubblewrap. There is deliberately no
unsandboxed fallback on hosts where isolation is unavailable. Destination prerequisites are never
installed here, and these checks do not prove live credentials or external I/O.
"""
from __future__ import annotations

from contextlib import contextmanager
from dataclasses import dataclass
from datetime import datetime, timezone
import fcntl
import hashlib
import hmac
import json
import os
from pathlib import Path, PurePosixPath
import selectors
import shutil
import signal
import site
import stat
import subprocess
import sys
import tempfile
import time
import uuid

from .blueprint import Blueprint
from .compiler import DECLARATIONS_PATH, continuation_contract, generate_package, generate_runtime_refresh
from .projects import DraftConflict
from .generator import PackageFile
from .dom_check import is_dom_test, run_dom_tests
from .checker_runtime import CheckerRuntimeError, resolve_node

MAX_FILE_BYTES = 16 * 1024 * 1024
MAX_PROJECT_BYTES = 64 * 1024 * 1024
MAX_PROJECT_FILES = 2048
MAX_BATCH_FILES = 32
MAX_BATCH_EDITS = 128
MAX_BATCH_ARGUMENT_BYTES = 4 * 1024 * 1024
MAX_BATCH_JOURNAL_BYTES = 16 * 1024 * 1024
CHECK_TIMEOUT_SECONDS = 30
CHECK_TOTAL_SECONDS = 60
CHECK_OUTPUT_BYTES = 64 * 1024
TERMINATION_WAIT_SECONDS = 1
_STATE_VERSION = 1
_RESULT_MARKER = "MOBKIT_HOST_TEST_RESULT="
_COMPLETION_MARKER = "MOBKIT_HOST_COMPLETED="
PROTECTED_APPLICATION_PATHS = frozenset({DECLARATIONS_PATH, "application/tool-policy.json",
    "application/configuration.json", "application/console.json", "config/access.toml",
    "application/runtime-selection.json"})


class WorkspaceError(ValueError):
    """A project path, mutation, or verification violated the workspace boundary."""


class WorkspaceBusy(DraftConflict):
    """Another owner held the workspace lock for longer than a read may wait."""


@dataclass(frozen=True)
class FrameworkSnapshot:
    """A host-only snapshot, never an implementation tool argument or receipt."""

    blueprint_digest: str
    files: tuple[PackageFile, ...]
    baseline_digest: str
    previous_build_id: str
    previous_source_digest: str
    origin: dict
    runtime_candidate: dict | None = None
    compiler_baseline_digest: str | None = None
    protected_application_paths: tuple[str, ...] = ()


def _json(value):
    return json.dumps(value, ensure_ascii=True, sort_keys=True, separators=(",", ":"))


def _sha(content):
    return hashlib.sha256(content if isinstance(content, bytes) else content.encode("utf-8")).hexdigest()


def _wheel_path(name):
    parts = name.split("/")
    return len(parts) == 3 and parts[:2] == ["application", "vendor"] and parts[-1].endswith(".whl")


def _read_package_file(path, name):
    """Read a frozen source file using the existing text normalization contract."""
    info = _regular(path)
    if info.st_size > MAX_FILE_BYTES:
        raise WorkspaceError("A source file exceeds the file size limit.")
    if _wheel_path(name):
        content = path.read_bytes()
    else:
        try:
            content = path.read_text(encoding="utf-8")
        except UnicodeError:
            raise WorkspaceError("Implementation files must be UTF-8 text.") from None
        if "\x00" in content:
            raise WorkspaceError("Implementation files must be text without NUL bytes.")
    return PackageFile(name, content, stat.S_IMODE(info.st_mode))


def _read_protected_application_file(path, name):
    """Retain selected source bytes exactly, including line endings."""
    info = _regular(path)
    if info.st_size > MAX_FILE_BYTES:
        raise WorkspaceError("A source file exceeds the file size limit.")
    content = path.read_bytes()
    if len(content) > MAX_FILE_BYTES:
        raise WorkspaceError("A source file exceeds the file size limit.")
    try:
        text = content.decode("utf-8")
    except UnicodeError:
        raise WorkspaceError("Implementation files must be UTF-8 text.") from None
    if "\x00" in text:
        raise WorkspaceError("Implementation files must be text without NUL bytes.")
    return PackageFile(name, text, stat.S_IMODE(info.st_mode))


def _text_size(content):
    if not isinstance(content, str) or "\x00" in content:
        raise WorkspaceError("Write UTF-8 text without NUL bytes within the file size limit.")
    try:
        size = len(content.encode("utf-8"))
    except UnicodeError:
        raise WorkspaceError("Write valid UTF-8 text without NUL bytes.") from None
    if size > MAX_FILE_BYTES:
        raise WorkspaceError("Write UTF-8 text without NUL bytes within the file size limit.")
    return size


def _now():
    return datetime.now(timezone.utc).isoformat()


def _safe_name(value):
    if not isinstance(value, str) or not value or len(value) > 300:
        raise WorkspaceError("Use a nonempty project-relative file path of at most 300 characters.")
    if "\\" in value or any(ord(char) < 32 for char in value):
        raise WorkspaceError("Project paths cannot contain backslashes or control characters.")
    parts = value.split("/")
    if any(part in ("", ".", "..") for part in parts) or PurePosixPath(value).is_absolute():
        raise WorkspaceError("Absolute paths and parent traversal are not allowed.")
    if any(part.startswith(".") and not (index == len(parts) - 1 and part == ".gitignore")
           for index, part in enumerate(parts)) and value != ".env.example":
        raise WorkspaceError("Private environment, runtime, and hidden files are not accessible.")
    if any(part in ("__pycache__", "node_modules") for part in parts):
        raise WorkspaceError("Generated caches and installed dependencies are not project source files.")
    return value


def _regular(path):
    info = path.lstat()
    if not stat.S_ISREG(info.st_mode) or info.st_nlink != 1:
        raise WorkspaceError("Project files must be ordinary files, never symlinks or hard links.")
    if info.st_mode & (stat.S_ISUID | stat.S_ISGID | stat.S_ISVTX):
        raise WorkspaceError("Special file permission bits are not allowed.")
    return info


def _authorable(name, baseline):
    if name.casefold() == DECLARATIONS_PATH.casefold():
        raise WorkspaceError("Announcement declarations are protected native configuration. Use the host preparation owner for a new build; a source-only repair cannot change them.")
    if name.casefold() in {path.casefold() for path in baseline}:
        raise WorkspaceError("Compiled deployment files are read-only. Add application code or assets instead.")
    if name.casefold() in ("implementation-report.json", "acceptance-report.json") or name.split("/", 1)[0].casefold() == "acceptance-tests":
        raise WorkspaceError("Implementation reports are issued by the host, not authored project code.")
    if name.lower().endswith(".whl"):
        raise WorkspaceError("Python wheels are host-adopted assets, not authored text files.")
    if name.lower().endswith((".py", ".pyw")) and name.split("/", 1)[0] not in ("application", "tests"):
        raise WorkspaceError("New Python code belongs inside application/ or tests/ so it cannot shadow the deployment framework.")
    if name.lower().endswith((".so", ".dylib", ".pyd", ".pyc", ".pyo")):
        raise WorkspaceError("Native libraries and Python bytecode are not authored text source.")


def _atomic_write(path, content, mode=0o600):
    descriptor, temporary = tempfile.mkstemp(prefix=".host-write-", dir=path.parent)
    try:
        os.fchmod(descriptor, mode)
        options = {} if isinstance(content, bytes) else {"encoding": "utf-8", "newline": ""}
        with os.fdopen(descriptor, "wb" if isinstance(content, bytes) else "w", **options) as stream:
            stream.write(content)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


class ImplementationWorkspace:
    """One frozen Blueprint and an additive implementation project.

    ``root`` is a host-owned, private directory, not the user's deployment
    folder. ``project`` contains public compiler output, authored text and
    host-adopted wheel assets.
    Private host metadata and signing material are outside that directory and
    outside every executable check's sandbox. No API accepts a shell command.
    """

    def __init__(self, root: str | Path, blueprint: Blueprint | dict, *, read_lock_timeout=None):
        candidate = Path(root).absolute()
        if candidate.is_symlink():
            raise WorkspaceError("The workspace root cannot be a symlink.")
        candidate.mkdir(parents=True, mode=0o700, exist_ok=True)
        self.root = candidate.resolve()
        # Opening a reader leaves a private root's metadata unchanged:
        # recovery observes it, and a same-mode chmod still moves ctime.
        if stat.S_IMODE(self.root.stat().st_mode) != 0o700:
            os.chmod(self.root, 0o700)
        self.project = self.root / "project"
        self._state_path = self.root / "workspace-state.json"
        self._key_path = self.root / "verification-key"
        self._lock_path = self.root / "workspace.lock"
        self._batch_path = self.root / "batch-transaction.json"
        self.blueprint = Blueprint.model_validate(
            blueprint.model_dump() if isinstance(blueprint, Blueprint) else blueprint
        )
        self._blueprint_digest = _sha(_json(self.blueprint.model_dump(mode="json")))
        self._preparation_checks = []
        # A server view waits at most this long to read; the builder's writes wait.
        self._read_lock_timeout = read_lock_timeout
        for path in (self.project, self._state_path, self._key_path, self._lock_path, self._batch_path):
            if path.is_symlink():
                raise WorkspaceError("Workspace storage cannot contain symlinks.")

    @contextmanager
    def _locked(self, *, timeout=None):
        """Hold the workspace lock. A read with a timeout refuses instead of waiting longer."""
        descriptor = os.open(self._lock_path, os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
        try:
            if os.fstat(descriptor).st_nlink != 1:
                raise WorkspaceError("Workspace lock cannot be a hard link.")
            if timeout is None:
                fcntl.flock(descriptor, fcntl.LOCK_EX)
            else:
                deadline = time.monotonic() + timeout
                while True:
                    try:
                        fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
                        break
                    except BlockingIOError:
                        if time.monotonic() >= deadline:
                            raise WorkspaceBusy("The application workspace is busy with another operation. "
                                "Try again in a moment.") from None
                        time.sleep(0.05)
            self._recover_batch_locked()
            yield
        finally:
            os.close(descriptor)

    def _state(self):
        if not self._state_path.exists():
            raise WorkspaceError("Seed the implementation workspace first.")
        _regular(self._state_path)
        state = json.loads(self._state_path.read_text(encoding="utf-8"))
        if state.get("version") != _STATE_VERSION or state.get("blueprint_digest") != self._blueprint_digest:
            raise WorkspaceError("This workspace belongs to a different frozen Blueprint.")
        framework = state.get("framework")
        if framework is not None and (not isinstance(framework, dict)
                or framework.get("kind") != "host-framework-provenance"
                or framework.get("blueprint_digest") != self._blueprint_digest
                or framework.get("baseline_digest") != _sha(_json(state["baseline"]))
                or not hmac.compare_digest(str(framework.get("signature", "")), self._signed(framework))):
            raise WorkspaceError("The host framework provenance no longer matches this workspace.")
        self._adopted_wheels(state)
        self._protected_application_paths(state)
        return state

    @staticmethod
    def _protected_application_paths(state):
        paths = (state.get("framework") or {}).get("protected_application_paths", [])
        if (not isinstance(paths, list) or any(not isinstance(name, str) for name in paths)
                or paths != sorted(set(paths)) or not set(paths) <= PROTECTED_APPLICATION_PATHS
                or not set(paths) <= state["baseline"].keys()):
            raise WorkspaceError("The protected application source paths are invalid.")
        return tuple(paths)

    def require_preparation_current(self):
        """Recheck the original host selections before publishing this build."""
        for require_current in self._preparation_checks:
            require_current()
        if self._preparation_checks:
            with self._locked():
                self._files(self._state())

    def adopt_application_inputs(self, sources, *, require_current):
        """Host-only adoption into the existing signed framework baseline.

        The preparation owner supplies actual selected source paths and its
        currentness fence. Neither builder tools nor HTTP requests expose this
        method. It transfers source bytes only, never native preparation facts.
        """
        if (not isinstance(sources, dict) or not sources or not callable(require_current)
                or any(not isinstance(name, str) or name not in PROTECTED_APPLICATION_PATHS for name in sources)):
            raise WorkspaceError("Choose actual protected application inputs through the host preparation owner.")
        require_current()
        observed = []
        files = []
        for name, source in sorted(sources.items()):
            path = Path(source).absolute()
            if any(parent.is_symlink() for parent in (path, *path.parents)):
                raise WorkspaceError("Selected application source cannot traverse symlinks.")
            info = _regular(path)
            if info.st_uid != os.getuid() or stat.S_IMODE(info.st_mode) not in (0o600, 0o644):
                raise WorkspaceError("Selected application source must be owned ordinary text with its original mode.")
            item = _read_protected_application_file(path, name)
            observed.append((path, (info.st_dev, info.st_ino, info.st_size, info.st_mtime_ns,
                info.st_ctime_ns, info.st_uid, info.st_mode), item))
            # The selected artifact retains its original source mode in the
            # currentness pin. Public package text uses the existing archive
            # projection, so this single destination baseline owns mode 0644.
            files.append(PackageFile(name, item.content, 0o644))

        def original_sources_current():
            require_current()
            for path, identity, item in observed:
                if any(parent.is_symlink() for parent in (path, *path.parents)):
                    raise WorkspaceError("Selected application source cannot traverse symlinks.")
                info = _regular(path)
                current = (info.st_dev, info.st_ino, info.st_size, info.st_mtime_ns,
                    info.st_ctime_ns, info.st_uid, info.st_mode)
                if current != identity or _read_protected_application_file(path, item.path) != item:
                    raise WorkspaceError("The originally selected application source changed before builder admission.")
            require_current()

        with self._locked():
            original_sources_current()
            state = self._state()
            before = self._files(state)
            prior_paths = self._protected_application_paths(state)
            baseline = dict(state["baseline"])
            if (DECLARATIONS_PATH in sources and DECLARATIONS_PATH not in baseline
                    and state["framework"].get("basis") in {"accepted-baseline", "runtime-refresh"}):
                raise WorkspaceError("A retained-framework repair cannot add announcement declarations. Prepare a new build with its native configuration.")
            for item in files:
                if any(entry.path.casefold() == item.path.casefold() and entry.path != item.path for entry in before):
                    raise WorkspaceError("Selected application source aliases an existing project path.")
                if item.path in baseline:
                    if (item.path not in prior_paths or baseline[item.path] != {
                            "sha256": _sha(item.content), "mode": item.mode}):
                        raise WorkspaceError("Selected application source collides with the protected framework.")
                elif any(name.casefold() == item.path.casefold() for name in baseline):
                    raise WorkspaceError("Selected application source aliases the protected framework.")
                baseline[item.path] = {"sha256": _sha(item.content), "mode": item.mode}
            manifest = "".join(f"{entry['sha256']}  {name}\n" for name, entry in sorted(baseline.items())
                if name != "SHA256SUMS")
            baseline["SHA256SUMS"] = {"sha256": _sha(manifest), "mode": state["baseline"]["SHA256SUMS"]["mode"]}
            next_state = json.loads(_json(state))
            next_state["baseline"] = baseline
            framework = next_state["framework"]
            framework["protected_application_paths"] = sorted(set(prior_paths) | sources.keys())
            framework["baseline_digest"] = _sha(_json(baseline))
            framework["signature"] = self._signed(framework)
            next_state["last_check"] = None
            self._commit_changes_locked(state, before,
                [*files, PackageFile("SHA256SUMS", manifest, baseline["SHA256SUMS"]["mode"])], next_state)
            original_sources_current()
            self._preparation_checks.append(original_sources_current)

    def _save_state(self, state):
        _atomic_write(self._state_path, _json(state) + "\n")

    def _adopted_wheels(self, state):
        assets = state.get("adopted_wheels", {})
        if not isinstance(assets, dict):
            raise WorkspaceError("Host-adopted wheel ownership is invalid.")
        for name, entry in assets.items():
            if (not _wheel_path(_safe_name(name)) or not isinstance(entry, dict)
                    or set(entry) != {"sha256", "size", "mode"}
                    or not isinstance(entry["sha256"], str) or len(entry["sha256"]) != 64
                    or any(char not in "0123456789abcdef" for char in entry["sha256"])
                    or type(entry["size"]) is not int or not 0 < entry["size"] <= MAX_FILE_BYTES
                    or type(entry["mode"]) is not int or entry["mode"] != 0o644):
                raise WorkspaceError("Host-adopted wheel ownership is invalid.")
        if "adopted_wheels" in state or "adopted_wheels_signature" in state:
            expected = self._signed({"blueprint_digest": self._blueprint_digest, "files": assets})
            if not hmac.compare_digest(str(state.get("adopted_wheels_signature", "")), expected):
                raise WorkspaceError("Host-adopted wheel ownership no longer matches this workspace.")
        return assets

    def adopt_wheel(self, path: str, source: str | Path, expected_sha256: str):
        """Host-only adoption before checking/reviewing; never registered as a tool.

        The caller selects an ordinary local artifact and its expected hash.
        Only public destination-relative metadata enters source projections.
        """
        path = _safe_name(path)
        if not _wheel_path(path):
            raise WorkspaceError("Adopt Python wheels only at application/vendor/<filename>.whl.")
        if (not isinstance(expected_sha256, str) or len(expected_sha256) != 64
                or any(char not in "0123456789abcdef" for char in expected_sha256)):
            raise WorkspaceError("Adopt a wheel with its exact lowercase SHA256.")
        source = Path(source).absolute()
        if any(part.is_symlink() for part in (source, *source.parents)):
            raise WorkspaceError("A wheel artifact must be an ordinary file without links.")
        descriptor = os.open(source, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        with os.fdopen(descriptor, "rb") as stream:
            info = os.fstat(stream.fileno())
            if (not stat.S_ISREG(info.st_mode) or info.st_nlink != 1 or info.st_uid != os.getuid()
                    or not 0 < info.st_size <= MAX_FILE_BYTES
                    or info.st_mode & (stat.S_ISUID | stat.S_ISGID | stat.S_ISVTX)):
                raise WorkspaceError("A wheel artifact must be an owned ordinary file within the size limit.")
            content = stream.read(MAX_FILE_BYTES + 1)
        if len(content) != info.st_size or not hmac.compare_digest(_sha(content), expected_sha256):
            raise WorkspaceError("The wheel artifact does not match its expected SHA256 and size.")
        with self._locked():
            state = self._state()
            before = self._files(state)
            by_path = {item.path: item for item in before}
            if path not in by_path and path.casefold() in {name.casefold() for name in by_path}:
                raise WorkspaceError("Use the wheel asset's exact existing path spelling.")
            previous = by_path.get(path)
            assets = self._adopted_wheels(state)
            entry = {"sha256": expected_sha256, "size": len(content), "mode": 0o644}
            if previous is not None and assets.get(path) == entry:
                return {"path": path, **entry, "binary": True, "host_owned": True,
                        "digest": self._digest(before)}
            new_size = sum(_regular(self._target(item.path)).st_size for item in before if item.path != path) + len(content)
            if new_size > MAX_PROJECT_BYTES or previous is None and len(before) >= MAX_PROJECT_FILES:
                raise WorkspaceError("The project exceeds its file or total size limit.")
            self._target(path)
            next_state = json.loads(_json(state))
            next_state["adopted_wheels"] = {**assets, path: entry}
            next_state["adopted_wheels_signature"] = self._signed(
                {"blueprint_digest": self._blueprint_digest, "files": next_state["adopted_wheels"]})
            next_state["changes"].append({"path": path, "before": _sha(previous.content) if previous else None,
                                          "after": expected_sha256, "at": _now()})
            next_state["last_check"] = None
            committed = self._commit_changes_locked(state, before, [PackageFile(path, content)], next_state)
            return {"path": path, **entry, "binary": True, "host_owned": True,
                    "digest": committed["digest"]}

    def seed(self, framework: FrameworkSnapshot | None = None, *, fresh_reason=None,
             runtime_refresh=False, runtime_target=None, legacy_ordinary=False):
        """Seed once, retaining a validated accepted framework for same-scope repairs.

        Preservation is source provenance only. This workspace gets a new key,
        no check receipt and no acceptance evidence. Current host checks still
        execute the actual retained framework bytes.
        """
        if type(runtime_refresh) is not bool or runtime_refresh and framework is None:
            raise WorkspaceError("A runtime refresh needs an exact accepted framework snapshot.")
        if runtime_target is not None:
            from .runtime_selection import MetaToolkitRuntimeTarget
            initial_product = type(runtime_target) is MetaToolkitRuntimeTarget and framework is None and not runtime_refresh
            if not initial_product and (not runtime_refresh or not isinstance(framework, FrameworkSnapshot)):
                raise WorkspaceError("A candidate requires an explicit runtime refresh of an accepted framework.")
        selection = None
        if runtime_target is not None:
            from .runtime_selection import implementation_candidate_selection
            selection = implementation_candidate_selection(runtime_target, deployment_target=self.blueprint.target)
        with self._locked():
            if self._state_path.exists():
                state = self._state()
                files = self._files(state)
                retained = self._runtime_candidate(state, files)
                if selection is not None and selection != retained:
                    raise WorkspaceError("The selected candidate differs from this protected workspace.")
                return self._list(files, state)
            if self.project.exists() and (self.project.is_symlink() or any(self.project.iterdir())):
                raise WorkspaceError("Refusing to seed over an existing, unowned project.")
            self.project.mkdir(mode=0o700, exist_ok=True)
            retained_candidate = bool(isinstance(framework, FrameworkSnapshot) and framework.runtime_candidate is not None
                                      and not runtime_refresh)
            deployment = (next((item for item in framework.files if item.path == "deployment.json"), None)
                          if runtime_refresh and runtime_target is not None and not legacy_ordinary else None)
            if deployment is not None:
                # The accepted deployment keeps its route: today's compiler
                # baseline for a refresh of an ordinary application is ordinary.
                from .compiler import accepted_ordinary_route
                legacy_ordinary = accepted_ordinary_route(self.blueprint, json.loads(deployment.content),
                    runtime_target)
            # A saved candidate is already protected source. Reopening or
            # repairing it never recompiles against today's default or bundle.
            files = (list(framework.files) if retained_candidate else
                     generate_package(self.blueprint, runtime_target=runtime_target,
                         legacy_ordinary=legacy_ordinary) if runtime_target is not None
                     else generate_package(self.blueprint))
            compiled_baseline = self._baseline(files)
            basis, origin, previous, changed_contract = "current-compiler", None, None, []
            retained_template_paths = []
            protected_application_paths = ()
            if framework is not None:
                if (not isinstance(framework, FrameworkSnapshot)
                        or framework.blueprint_digest != self._blueprint_digest):
                    raise WorkspaceError("A retained framework must belong to the identical frozen Blueprint.")
                retained_baseline = self._baseline(framework.files)
                if _sha(_json(retained_baseline)) != framework.baseline_digest:
                    raise WorkspaceError("The retained framework snapshot changed before seeding.")
                protected_application_paths = framework.protected_application_paths
                self._protected_application_paths({"baseline": retained_baseline,
                    "framework": {"protected_application_paths": list(protected_application_paths)}})
                retained_selection = self._runtime_candidate({"baseline": retained_baseline,
                    "framework": {"runtime_candidate": framework.runtime_candidate}
                        if framework.runtime_candidate is not None else {}}, framework.files)
                if retained_candidate:
                    selection = retained_selection
                old_contract, new_contract = continuation_contract(framework.files), continuation_contract(files)
                changed_contract = sorted(name for name in set(old_contract) | set(new_contract)
                                          if old_contract.get(name) != new_contract.get(name))
                # Compiler drift is provenance, not an implicit host upgrade.
                # Explicit refresh and changed specifications arrive without
                # a retained framework through ordinary service admission.
                files = (generate_runtime_refresh(self.blueprint, framework.files, runtime_target=runtime_target,
                            protected_application_paths=protected_application_paths)
                         if runtime_refresh and runtime_target is not None else
                         generate_runtime_refresh(self.blueprint, framework.files)
                         if runtime_refresh else list(framework.files))
                selected_baseline = self._baseline(files)
                retained_template_paths = sorted(name for name in set(selected_baseline) | set(compiled_baseline)
                    if name != "SHA256SUMS" and name not in protected_application_paths
                    and selected_baseline.get(name) != compiled_baseline.get(name))
                basis, fresh_reason = (("runtime-refresh", "explicit-runtime-refresh") if runtime_refresh
                                       else ("accepted-baseline", "unchanged-specification"))
                origin = json.loads(_json(framework.origin))
                previous = {"build_id": framework.previous_build_id,
                            "source_digest": framework.previous_source_digest}
            baseline = {}
            for item in files:
                name = _safe_name(item.path)
                destination = self.project / name
                destination.parent.mkdir(parents=True, mode=0o700, exist_ok=True)
                _atomic_write(destination, item.content, item.mode)
                baseline[name] = {"sha256": _sha(item.content), "mode": item.mode}
            _atomic_write(self._key_path, os.urandom(32).hex() + "\n")
            state = {"version": _STATE_VERSION, "blueprint_digest": self._blueprint_digest,
                     "seeded_at": _now(), "baseline": baseline, "changes": [], "last_check": None}
            provenance = {"kind": "host-framework-provenance", "version": 1,
                "blueprint_digest": self._blueprint_digest, "baseline_digest": _sha(_json(baseline)),
                "basis": basis, "reason": fresh_reason or "new-implementation",
                "origin": origin, "previous": previous,
                "changed_contract_paths": changed_contract,
                "current_compiler_baseline_digest": (framework.compiler_baseline_digest or framework.baseline_digest
                    if retained_candidate else _sha(_json(compiled_baseline))),
                "retained_template_paths": retained_template_paths,
                "scope": "Framework source provenance only; no current-template upgrade or new verification is established."}
            if protected_application_paths:
                provenance["protected_application_paths"] = list(protected_application_paths)
            if selection is not None:
                provenance["runtime_candidate"] = selection
                self._runtime_candidate({"baseline": baseline, "framework": provenance}, files)
            provenance["signature"] = self._signed(provenance)
            state["framework"] = provenance
            self._save_state(state)
            return self._list(self._files(state), state)

    @staticmethod
    def _baseline(files):
        result = {}
        size = 0
        for item in files:
            name = _safe_name(item.path)
            if name in result or name.casefold() in {path.casefold() for path in result}:
                raise WorkspaceError("The framework snapshot contains duplicate file paths.")
            if not isinstance(item.content, str) or "\x00" in item.content or item.mode not in (0o600, 0o644, 0o755):
                raise WorkspaceError("The framework snapshot contains an invalid text file or mode.")
            item_size = len(item.content.encode("utf-8"))
            size += item_size
            if item_size > MAX_FILE_BYTES or size > MAX_PROJECT_BYTES or len(result) >= MAX_PROJECT_FILES:
                raise WorkspaceError("The framework snapshot exceeds its file or total size limit.")
            result[name] = {"sha256": _sha(item.content), "mode": item.mode}
        return result

    def _runtime_candidate(self, state, files):
        """Recover identity only after the caller checks the host HMAC and files.

        This pure source comparison never admits or reads external artifacts.
        A package document or authored file without signed provenance cannot
        establish a selected runtime.
        """
        from .runtime_selection import (parse_candidate_selection, MetaToolkitRuntimeTarget,
            selected_runtime_marker, selected_runtime_version, selected_runtime_platform)
        selected = {item.path: item for item in files}
        value = (state.get("framework") or {}).get("runtime_candidate")
        deployment = json.loads(selected["deployment.json"].content)
        paths = {"runtime-candidate.json", "runtime_candidates.py", "install_candidate.py"}
        if value is None:
            if "runtimeCandidate" in deployment or paths & selected.keys():
                raise WorkspaceError("Candidate source needs its host-selected framework provenance.")
            return None
        target = parse_candidate_selection(value)
        expected = selected_runtime_marker(target)
        version = selected_runtime_version(target)
        product = type(target) is MetaToolkitRuntimeTarget
        platforms = {"mac": "apple-darwin", "linux": "unknown-linux-gnu"}
        if product and target.target == "linux-x86_64":
            platforms["gcp"] = "unknown-linux-gnu"
        if (not paths <= state["baseline"].keys() or not paths <= selected.keys()
                or _json(json.loads(selected["runtime-candidate.json"].content)) != _json(value)
                or _json(deployment.get("runtimeCandidate")) != _json(expected)
                or deployment.get("mobkitVersion") != version
                or deployment.get("target") != self.blueprint.target
                or self.blueprint.target not in platforms
                or not selected_runtime_platform(target).endswith("-" + platforms[self.blueprint.target])
                or (not product and selected["requirements.txt"].content.splitlines().count(
                    "meerkat-mobkit==" + version) != 1)
                or (product and any(selected["requirements.txt"].content.splitlines().count(line) != 1
                    for line in (f"meerkat-meta=={target.meta.version} --hash=sha256:{target.meta.sha256}",
                        f"meerkat-toolkit=={target.toolkit.version} --hash=sha256:{target.toolkit.sha256}")))):
            raise WorkspaceError("The protected candidate selection does not match its framework source.")
        return json.loads(_json(value))

    def _framework_snapshot(self, state, files, *, build_id, accepted_source_digest=None):
        """Called under the source lock after host acceptance/currentness selection."""
        digest = self._digest(files)
        provenance = state.get("framework") or {}
        if accepted_source_digest is not None:
            if accepted_source_digest != digest:
                raise WorkspaceError("The accepted framework source changed before continuation.")
            origin = {"build_id": build_id, "source_digest": digest}
        elif (provenance.get("basis") == "accepted-baseline"
                or provenance.get("basis") == "runtime-refresh" and provenance.get("runtime_candidate")) and provenance.get("origin"):
            # _state checked the host signature and exact baseline binding.
            # An interrupted repair retains that origin but transfers no pass.
            origin = provenance["origin"]
        else:
            return None
        selected = {item.path: item for item in files}
        # _files derives the final export manifest, while the protected seed
        # manifest belongs to baseline-only files. Preserve its original bytes.
        selected["SHA256SUMS"] = PackageFile("SHA256SUMS", self._target("SHA256SUMS").read_text(),
                                             state["baseline"]["SHA256SUMS"]["mode"])
        baseline_files = tuple(selected[name] for name in sorted(state["baseline"]))
        if self._baseline(baseline_files) != state["baseline"]:
            raise WorkspaceError("The retained framework no longer matches the protected baseline.")
        return FrameworkSnapshot(self._blueprint_digest, baseline_files, _sha(_json(state["baseline"])),
                                 build_id, digest, json.loads(_json(origin)),
                                 self._runtime_candidate(state, files),
                                 provenance.get("current_compiler_baseline_digest"),
                                 self._protected_application_paths(state))

    def framework_provenance(self):
        """Public source provenance, excluding the private host attestation."""
        with self._locked(timeout=self._read_lock_timeout):
            state = self._state()
            self._files(state)
            return {key: value for key, value in (state.get("framework") or {}).items() if key != "signature"}

    def _target(self, name):
        name = _safe_name(name)
        if self.project.is_symlink() or not self.project.is_dir():
            raise WorkspaceError("The project directory is invalid.")
        current = self.project
        for part in PurePosixPath(name).parts[:-1]:
            current = current / part
            if current.is_symlink() or (current.exists() and not current.is_dir()):
                raise WorkspaceError("Project parent directories cannot be links or files.")
        target = self.project / name
        if target.is_symlink():
            raise WorkspaceError("Project files cannot be symlinks.")
        if target.exists():
            _regular(target)
        return target

    def _files(self, state, project=None):
        """Read an entire bounded tree; reject unexpected links and frozen edits."""
        project = self.project if project is None else project
        if project.is_symlink() or not project.is_dir():
            raise WorkspaceError("The project directory is invalid.")
        result = []
        size = 0
        assets = self._adopted_wheels(state)
        protected = self._protected_application_paths(state)
        for directory, children, names in os.walk(project, followlinks=False):
            for child in children:
                folder = Path(directory) / child
                _safe_name(folder.relative_to(project).as_posix() + "/placeholder")
                if folder.is_symlink():
                    raise WorkspaceError("Project directories cannot be symlinks.")
            for name in names:
                path = Path(directory) / name
                relative = _safe_name(path.relative_to(project).as_posix())
                info = _regular(path)
                size += info.st_size
                if info.st_size > MAX_FILE_BYTES or size > MAX_PROJECT_BYTES or len(result) >= MAX_PROJECT_FILES:
                    raise WorkspaceError("The project exceeds its file or total size limit.")
                if relative.lower().endswith(".whl") and relative not in assets:
                    raise WorkspaceError("The project contains a wheel not adopted by its host.")
                item = (_read_protected_application_file(path, relative) if relative in protected
                    else _read_package_file(path, relative))
                content, mode = item.content, item.mode
                if relative in assets:
                    if assets[relative] != {"sha256": _sha(content), "size": len(content), "mode": mode}:
                        raise WorkspaceError("A host-adopted wheel changed outside its ownership binding.")
                    result.append(item)
                    continue
                original = state["baseline"].get(relative)
                if original and (original["sha256"] != _sha(content) or original["mode"] != mode):
                    raise WorkspaceError(f"The compiled deployment file {relative} was changed outside the host.")
                if not original:
                    _authorable(relative, state["baseline"])
                result.append(PackageFile(relative, content, mode))
        if set(state["baseline"]) - {item.path for item in result}:
            raise WorkspaceError("A compiled deployment file is missing.")
        if set(assets) - {item.path for item in result}:
            raise WorkspaceError("A host-adopted wheel is missing.")
        # The agent cannot edit the manifest. The host derives it from all
        # current output, so the verification digest matches the final export.
        result = [item for item in result if item.path != "SHA256SUMS"]
        manifest = "".join(f"{_sha(item.content)}  {item.path}\n" for item in sorted(result, key=lambda item: item.path))
        return sorted([*result, PackageFile("SHA256SUMS", manifest)], key=lambda item: item.path)

    @staticmethod
    def _digest(files):
        return _sha(_json([{ "path": item.path, "sha256": _sha(item.content), "mode": item.mode }
                           for item in files]))

    @staticmethod
    def _list(files, state):
        return [{"path": item.path, "size": len(item.as_bytes()), "sha256": _sha(item.content),
                 "modified": item.path != "SHA256SUMS" and state["baseline"].get(item.path, {}).get("sha256") != _sha(item.content),
                 "derived": item.path == "SHA256SUMS",
                 **({"binary": True, "host_owned": True} if isinstance(item.content, bytes) else {})}
                for item in files]

    def list_files(self):
        with self._locked(timeout=self._read_lock_timeout):
            state = self._state()
            return self._list(self._files(state), state)

    def read_file(self, path: str):
        _safe_name(path)
        with self._locked(timeout=self._read_lock_timeout):
            state = self._state()
            self._target(path)
            for item in self._files(state):
                if item.path == path:
                    if isinstance(item.content, bytes):
                        raise WorkspaceError("Host-adopted wheels expose metadata only, not text content.")
                    return item.content
            raise WorkspaceError("That project file does not exist.")

    def write_file(self, path: str, content: str):
        path = _safe_name(path)
        _text_size(content)
        with self._locked():
            state = self._state()
            before = self._files(state)
            return self._write_file_locked(path, content, state, before)

    def edit_file(self, path: str, expected_sha256: str, old_text: str, new_text: str):
        """Replace one exact match in current read/list text under the source lock.

        The existing UTF-8 reader normalizes newlines. Its advertised content
        hash, rather than a distinct raw-byte hash, owns this precondition.
        """
        path = _safe_name(path)
        if (not isinstance(expected_sha256, str) or len(expected_sha256) != 64
                or any(char not in "0123456789abcdef" for char in expected_sha256)):
            raise WorkspaceError("Use the current lowercase SHA256 from project read/list metadata.")
        _text_size(old_text)
        _text_size(new_text)
        if not old_text:
            raise WorkspaceError("The text to replace must be nonempty.")
        if old_text == new_text:
            raise WorkspaceError("The replacement must change the project text.")
        with self._locked():
            state = self._state()
            before = self._files(state)
            _authorable(path, state["baseline"])
            self._target(path)
            previous = next((item for item in before if item.path == path), None)
            if previous is None:
                raise WorkspaceError("That project file does not exist. Use write_project_file to create it.")
            if not hmac.compare_digest(expected_sha256, _sha(previous.content)):
                raise WorkspaceError("The project file changed. Read it and its current SHA256 before editing.")
            # Advance one character so overlapping occurrences remain ambiguous.
            first = previous.content.find(old_text)
            if first < 0:
                raise WorkspaceError("The exact text to replace was not found.")
            if previous.content.find(old_text, first + 1) >= 0:
                raise WorkspaceError("The exact text to replace must match only once.")
            content = previous.content[:first] + new_text + previous.content[first + len(old_text):]
            return self._write_file_locked(path, content, state, before)

    def apply_batch(self, operations):
        """Apply bounded writes and ordered exact edits as one source transaction.

        Every existing file requires the SHA advertised by read/list. A new
        write requires expected_sha256=None. Validation completes under the
        source lock before staging or changing files. A durable private journal
        resolves an interrupted directory/state commit on the next locked API.
        """
        if not isinstance(operations, list) or not 1 <= len(operations) <= MAX_BATCH_FILES:
            raise WorkspaceError(f"Provide 1-{MAX_BATCH_FILES} unique project file operations.")
        argument_bytes, edit_count, names = 0, 0, set()
        for operation in operations:
            if not isinstance(operation, dict) or operation.get("kind") not in ("write", "edit"):
                raise WorkspaceError("Each batch operation must be a write or edit object.")
            fields = {"kind", "path", "expected_sha256", "content"} if operation["kind"] == "write" else {"kind", "path", "expected_sha256", "edits"}
            if set(operation) != fields:
                raise WorkspaceError("Use exactly the documented fields for each batch operation.")
            path = _safe_name(operation["path"])
            if path.casefold() in names:
                raise WorkspaceError("Each project path may occur only once in a batch.")
            names.add(path.casefold())
            expected = operation["expected_sha256"]
            if expected is not None and (not isinstance(expected, str) or len(expected) != 64
                    or any(char not in "0123456789abcdef" for char in expected)):
                raise WorkspaceError("Use the current lowercase SHA256, or null only for a new write.")
            if operation["kind"] == "write":
                argument_bytes += _text_size(operation["content"])
            else:
                if expected is None:
                    raise WorkspaceError("An edit requires an existing file's current SHA256.")
                edits = operation["edits"]
                if not isinstance(edits, list) or not 1 <= len(edits) <= MAX_BATCH_EDITS:
                    raise WorkspaceError("Provide a bounded nonempty list of exact replacements.")
                edit_count += len(edits)
                if edit_count > MAX_BATCH_EDITS:
                    raise WorkspaceError("The batch arguments exceed their text or replacement limit.")
                for edit in edits:
                    if not isinstance(edit, dict) or set(edit) != {"old_text", "new_text"}:
                        raise WorkspaceError("Each replacement contains only old_text and new_text.")
                    argument_bytes += _text_size(edit["old_text"]) + _text_size(edit["new_text"])
                    if argument_bytes > MAX_BATCH_ARGUMENT_BYTES:
                        raise WorkspaceError("The batch arguments exceed their text or replacement limit.")
                    if not edit["old_text"] or edit["old_text"] == edit["new_text"]:
                        raise WorkspaceError("Each exact replacement must change nonempty old text.")
            argument_bytes += len(path.encode("utf-8")) + 64
            if argument_bytes > MAX_BATCH_ARGUMENT_BYTES or edit_count > MAX_BATCH_EDITS:
                raise WorkspaceError("The batch arguments exceed their text or replacement limit.")
        with self._locked():
            state = self._state()
            before = self._files(state)
            by_path = {item.path: item for item in before}
            # Refuse aliases and file/parent collisions before any staging.
            prefixes = {}
            for name in [*by_path, *(operation["path"] for operation in operations)]:
                parts = name.split("/")
                for length in range(1, len(parts) + 1):
                    prefix = "/".join(parts[:length])
                    key = prefix.casefold()
                    if key in prefixes and prefixes[key] != prefix:
                        raise WorkspaceError("Batch paths must use the project's exact existing spelling.")
                    prefixes[key] = prefix
            all_paths = {name.casefold() for name in by_path} | names
            for operation in operations:
                parts = operation["path"].casefold().split("/")
                if any("/".join(parts[:length]) in all_paths for length in range(1, len(parts))):
                    raise WorkspaceError("A batch file path cannot also be another file's parent.")
            changes = []
            raw_sizes = {item.path: _regular(self._target(item.path)).st_size for item in before}
            for operation in operations:
                path, expected = operation["path"], operation["expected_sha256"]
                _authorable(path, state["baseline"])
                self._target(path)
                previous = by_path.get(path)
                if (previous is None and expected is not None or previous is not None
                        and (expected is None or not hmac.compare_digest(expected, _sha(previous.content)))):
                    raise WorkspaceError("A batch file changed or already exists. Read its current SHA256 before retrying.")
                if operation["kind"] == "write":
                    content = operation["content"]
                else:
                    if previous is None:
                        raise WorkspaceError("That project file does not exist. Use a batch write to create it.")
                    content = previous.content
                    for edit in operation["edits"]:
                        old, new = edit["old_text"], edit["new_text"]
                        first = content.find(old)
                        if first < 0 or content.find(old, first + 1) >= 0:
                            raise WorkspaceError("Each exact replacement must match once in its ordered input text.")
                        content = content[:first] + new + content[first + len(old):]
                size = _text_size(content)
                if previous is not None and content == previous.content:
                    raise WorkspaceError("Each batch operation must change the project text.")
                raw_sizes[path] = size
                mode = previous.mode if previous else (0o755 if path.endswith(".sh") else 0o644)
                changes.append(PackageFile(path, content, mode))
            if sum(raw_sizes.values()) > MAX_PROJECT_BYTES or len(raw_sizes) > MAX_PROJECT_FILES:
                raise WorkspaceError("The project exceeds its file or total size limit.")
            next_state = json.loads(_json(state))
            at = _now()
            next_state["changes"].extend({"path": item.path,
                "before": _sha(by_path[item.path].content) if item.path in by_path else None,
                "after": _sha(item.content), "at": at} for item in changes)
            next_state["last_check"] = None
            return self._commit_changes_locked(state, before, changes, next_state)

    def _commit_changes_locked(self, state, before, changes, next_state):
        """Use the existing source/state transaction for text or host assets."""
        by_path = {item.path: item for item in before}
        before_digest = self._digest(before)
        before_text = self._state_path.read_text(encoding="utf-8")
        if json.loads(before_text) != state:
            raise WorkspaceError("The workspace state changed. Read source before retrying.")
        after_text = _json(next_state) + "\n"
        token = uuid.uuid4().hex
        staged, backup = self.root / ("batch-stage-" + token), self.root / ("batch-backup-" + token)
        after_files = {**by_path, **{item.path: item for item in changes}}
        del after_files["SHA256SUMS"]
        manifest = "".join(f"{_sha(item.content)}  {item.path}\n"
            for item in sorted(after_files.values(), key=lambda item: item.path))
        after_files["SHA256SUMS"] = PackageFile("SHA256SUMS", manifest)
        after_digest = self._digest(sorted(after_files.values(), key=lambda item: item.path))
        journal = {"version": 1, "token": token, "blueprint_digest": self._blueprint_digest,
            "before_state": before_text, "after_state": after_text,
            "before_digest": before_digest, "after_digest": after_digest}
        journal["signature"] = self._signed(journal)
        payload = _json(journal) + "\n"
        if len(payload.encode("utf-8")) > MAX_BATCH_JOURNAL_BYTES:
            raise WorkspaceError("The batch transaction metadata exceeds its limit.")
        try:
            # Publish recovery intent before staging, so even a partial
            # copy is removed before the next locked source API.
            _atomic_write(self._batch_path, payload)
            self._sync_directory(self.root)
            shutil.copytree(self.project, staged, copy_function=shutil.copyfile)
            os.chmod(staged, 0o700)
            for directory, folders, files in os.walk(staged):
                os.chmod(directory, 0o700)
                for filename in files:
                    name = (Path(directory) / filename).relative_to(staged).as_posix()
                    os.chmod(Path(directory) / filename, by_path[name].mode)
            if self._digest(self._files(state, staged)) != before_digest:
                raise WorkspaceError("The project changed while staging the batch. Read it before retrying.")
            for item in changes:
                target = staged / item.path
                target.parent.mkdir(parents=True, mode=0o700, exist_ok=True)
                _atomic_write(target, item.content, item.mode)
            if self._digest(self._files(next_state, staged)) != after_digest:
                raise WorkspaceError("The staged batch source does not match its validated changes.")
            self._sync_batch_tree(staged)
            if self._state_path.read_text(encoding="utf-8") != before_text or self._digest(self._files(state)) != before_digest:
                raise WorkspaceError("The project changed while staging the batch. Read it before retrying.")
            self._sync_batch_tree(self.project)
            os.replace(self.project, backup)
            self._sync_directory(self.root)
            os.replace(staged, self.project)
            self._sync_directory(self.root)
            self._save_state(next_state)
            self._sync_directory(self.root)
            self._recover_batch_locked()
        except Exception:
            if self._batch_path.exists():
                self._recover_batch_locked()
            elif staged.exists():
                self._remove_batch_tree(staged)
            raise
        return {"files": [{"path": item.path, "size": len(item.as_bytes()),
            "sha256": _sha(item.content), "modified": True} for item in changes],
            "digest": after_digest}

    @staticmethod
    def _sync_directory(path):
        descriptor = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        try:
            os.fsync(descriptor)
        finally:
            os.close(descriptor)

    @classmethod
    def _sync_batch_tree(cls, path):
        directories = []
        for directory, folders, files in os.walk(path, followlinks=False):
            directory = Path(directory)
            if directory.is_symlink() or not directory.is_dir():
                raise WorkspaceError("Batch storage must contain ordinary directories.")
            directories.append(directory)
            for folder in folders:
                if (directory / folder).is_symlink():
                    raise WorkspaceError("Batch storage cannot contain links.")
            for filename in files:
                target = directory / filename
                _regular(target)
                descriptor = os.open(target, os.O_RDONLY | os.O_NOFOLLOW)
                try:
                    os.fsync(descriptor)
                finally:
                    os.close(descriptor)
        for directory in reversed(directories):
            cls._sync_directory(directory)

    @staticmethod
    def _remove_batch_tree(path):
        if path.is_symlink() or not path.is_dir():
            raise WorkspaceError("Batch storage must be an ordinary directory.")
        for directory, folders, files in os.walk(path, followlinks=False):
            for folder in folders:
                if (Path(directory) / folder).is_symlink():
                    raise WorkspaceError("Batch storage cannot contain links.")
            for filename in files:
                _regular(Path(directory) / filename)
        shutil.rmtree(path)

    def _recover_batch_locked(self):
        """Finish a committed batch or restore its old project before any API."""
        if not self._batch_path.exists() and not self._batch_path.is_symlink():
            return
        info = _regular(self._batch_path)
        if info.st_size > MAX_BATCH_JOURNAL_BYTES:
            raise WorkspaceError("The batch transaction metadata exceeds its limit.")
        try:
            journal = json.loads(self._batch_path.read_text(encoding="utf-8"))
            fields = {"version", "token", "blueprint_digest", "before_state", "after_state", "before_digest", "after_digest", "signature"}
            if (not isinstance(journal, dict) or set(journal) != fields or journal["version"] != 1
                    or journal["blueprint_digest"] != self._blueprint_digest
                    or not isinstance(journal["token"], str) or len(journal["token"]) != 32
                    or any(char not in "0123456789abcdef" for char in journal["token"])
                    or not hmac.compare_digest(str(journal["signature"]), self._signed(journal))):
                raise ValueError
            before, after = json.loads(journal["before_state"]), json.loads(journal["after_state"])
            for state in (before, after):
                if state["version"] != _STATE_VERSION or state["blueprint_digest"] != self._blueprint_digest:
                    raise ValueError
            _regular(self._state_path)
            current = self._state_path.read_text(encoding="utf-8")
            if current not in (journal["before_state"], journal["after_state"]):
                raise ValueError
        except (TypeError, KeyError, ValueError):
            raise WorkspaceError("The host batch transaction cannot be safely recovered.") from None
        staged = self.root / ("batch-stage-" + journal["token"])
        backup = self.root / ("batch-backup-" + journal["token"])
        if current == journal["after_state"]:
            if self._digest(self._files(after)) != journal["after_digest"]:
                raise WorkspaceError("The committed batch source no longer matches its transaction.")
            # Keep the checked backup until current source/state directory
            # entries are durable, including recovery after a failed fsync.
            self._sync_directory(self.root)
        elif backup.exists():
            if self._digest(self._files(before, backup)) != journal["before_digest"]:
                raise WorkspaceError("The original batch source no longer matches its transaction.")
            if self.project.exists() or self.project.is_symlink():
                # A prior recovery may have stopped midway through removal.
                # The checked original tree remains complete in backup.
                self._remove_batch_tree(self.project)
            os.replace(backup, self.project)
            self._sync_directory(self.root)
        elif self._digest(self._files(before)) != journal["before_digest"]:
            raise WorkspaceError("The original batch source no longer matches its transaction.")
        for path in (staged, backup):
            if path.exists() or path.is_symlink():
                self._remove_batch_tree(path)
        self._batch_path.unlink()
        self._sync_directory(self.root)

    def _write_file_locked(self, path, content, state, before):
        """Commit authored source and invalidate checks; caller owns the lock."""
        size = _text_size(content)
        _authorable(path, state["baseline"])
        target = self._target(path)
        previous = next((item for item in before if item.path == path), None)
        # _files bounds the actual tree, including the protected seed manifest.
        # Its returned manifest is derived and its text has normalized newlines,
        # so neither returned text size is the on-disk total used by that check.
        new_size = sum(_regular(self._target(item.path)).st_size for item in before if item.path != path) + size
        if new_size > MAX_PROJECT_BYTES or (previous is None and len(before) >= MAX_PROJECT_FILES):
            raise WorkspaceError("The project exceeds its file or total size limit.")
        target.parent.mkdir(parents=True, mode=0o700, exist_ok=True)
        mode = previous.mode if previous else (0o755 if path.endswith(".sh") else 0o644)
        _atomic_write(target, content, mode)
        state["changes"].append({"path": path, "before": _sha(previous.content) if previous else None,
                                 "after": _sha(content), "at": _now()})
        state["last_check"] = None
        self._save_state(state)
        return {"path": path, "size": size, "sha256": _sha(content),
                "modified": True, "digest": self._digest(self._files(state))}

    def digest(self):
        with self._locked(timeout=self._read_lock_timeout):
            state = self._state()
            return self._digest(self._files(state))

    current_digest = digest

    def _signed(self, receipt):
        _regular(self._key_path)
        key = bytes.fromhex(self._key_path.read_text(encoding="ascii").strip())
        payload = {key: value for key, value in receipt.items() if key != "signature"}
        return hmac.new(key, _json(payload).encode(), hashlib.sha256).hexdigest()

    def _verification(self, state, files):
        receipt = state.get("last_check")
        if not isinstance(receipt, dict) or receipt.get("digest") != self._digest(files):
            return None
        if not hmac.compare_digest(str(receipt.get("signature", "")), self._signed(receipt)):
            return None
        return json.loads(_json(receipt))

    def verification(self):
        """Return only the current, authentic receipt; edits make it disappear."""
        with self._locked(timeout=self._read_lock_timeout):
            state = self._state()
            return self._verification(state, self._files(state))

    def export_files(self, require_verified=True):
        with self._locked(timeout=self._read_lock_timeout):
            state = self._state()
            files = self._files(state)
            receipt = self._verification(state, files)
            if require_verified and (not receipt or not receipt.get("ready_to_package")):
                raise WorkspaceError("Run passing executable checks with at least one authored test before packaging.")
            return files

    def check(self, tests=None):
        """Run host-owned source checks and authored Python or offline DOM tests.

        ``tests`` selects existing test_*.py or test_*.dom.cjs files, never commands.
        Receipts report offline evidence only. A passing fake-I/O test is not
        evidence that real authentication, a connector, or hardware works.
        """
        with self._locked():
            state = self._state()
            files = self._files(state)
            digest = self._digest(files)
            known = {item.path for item in files}
            all_tests = sorted(path for path in known if path.startswith("tests/")
                               and Path(path).name.startswith("test_")
                               and (path.endswith(".py") or is_dom_test(path)))
            if tests is None:
                selected = all_tests
            else:
                if isinstance(tests, str):
                    tests = [tests]
                if not isinstance(tests, list) or not tests or len(tests) > 100:
                    raise WorkspaceError("Select existing tests/test_*.py or tests/test_*.dom.cjs files.")
                selected = sorted(set(_safe_name(path) for path in tests))
                if any(path not in all_tests for path in selected):
                    raise WorkspaceError("Only existing tests/test_*.py or tests/test_*.dom.cjs files can be selected.")
            python_tests = [path for path in selected if path.endswith(".py")]
            dom_tests = [path for path in selected if is_dom_test(path)]
            # A new verification attempt supersedes an earlier receipt even
            # if infrastructure cleanup later fails before issuing a result.
            state["last_check"] = None
            self._save_state(state)
            checks = []
            from .application_inventory_source import application_inventory_source, selects_application_inventory
            source_files = {item.path: item.as_bytes() for item in files}
            try:
                if selects_application_inventory(source_files):
                    application_inventory_source(source_files)
                    checks.append({"name": "native application inventory source", "status": "pass",
                        "detail": "Exact compiled roster and application-authored class declarations joined; native admission remains pending."})
            except (ValueError, TypeError, KeyError, RecursionError) as error:
                checks.append({"name": "native application inventory source", "status": "fail", "detail": str(error)[:2000]})
            for item in files:
                if item.path.endswith(".py"):
                    try:
                        compile(item.content, item.path, "exec", dont_inherit=True)
                        checks.append({"name": f"python syntax: {item.path}", "status": "pass", "detail": "Compiled without executing project code."})
                    except (SyntaxError, ValueError) as error:
                        checks.append({"name": f"python syntax: {item.path}", "status": "fail", "detail": str(error)[:2000]})
            with tempfile.TemporaryDirectory(prefix="mobkit-implementation-") as temporary:
                staging = Path(temporary).resolve()
                snapshot = staging / "project"
                snapshot.mkdir(mode=0o700)
                for item in files:
                    path = snapshot / item.path
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_bytes(item.as_bytes())
                    path.chmod(item.mode)
                scratch = staging / "scratch"
                scratch.mkdir(mode=0o700)
                sandbox = _sandbox_executable()
                profile = _sandbox_profile(snapshot, scratch, staging)
                environment = _check_environment(scratch)
                executable = str(Path(sys.executable).resolve())
                execution_deadline = time.monotonic() + CHECK_TOTAL_SECONDS
                def execute(command, read_paths=(), input_bytes=None):
                    remaining = execution_deadline - time.monotonic()
                    if remaining <= 0:
                        return {"status": "fail", "detail": "The complete check exceeded its total executable time limit."}
                    selected_profile = (_sandbox_profile(snapshot, scratch, staging, protected_reads=read_paths)
                                        if read_paths else profile)
                    options = {"timeout": min(CHECK_TIMEOUT_SECONDS, remaining)}
                    if input_bytes is not None:
                        options["input_bytes"] = input_bytes
                    return _run_sandboxed(command, sandbox, selected_profile, snapshot, environment, **options)
                preflight = execute([executable, "-I", "-S", "-c", "print('sandbox ready')"])
                sandbox_ready = preflight["status"] == "pass"
                if not sandbox_ready:
                    preflight = {"status": "unavailable", "detail": "The trusted sandbox startup probe failed; generated code was not executed.\n" + preflight["detail"]}
                checks.append({"name": "execution sandbox", **preflight})
                # Parser-only commands do not run source code, but still use
                # the same boundary. A missing sandbox never permits execution.
                try:
                    node = resolve_node(shutil.which("node"))
                    node_error = "Node.js or the execution sandbox is unavailable. Prepare app checks in Forge, or run .venv/bin/python -m launchkit.checker_runtime on a supported Mac or Linux host. Execution still requires available isolation."
                except CheckerRuntimeError as error:
                    node, node_error = None, str(error)
                for item in files:
                    if item.path.endswith((".js", ".mjs", ".cjs")):
                        result = execute([str(Path(node).resolve()), "--check", str(snapshot / item.path)], [Path(node).resolve()]) if node and sandbox_ready else {"status": "unavailable", "detail": node_error}
                        checks.append({"name": f"node syntax: {item.path}", **result})
                    elif item.path.endswith(".sh"):
                        bash = shutil.which("bash")
                        result = execute([bash, "--noprofile", "--norc", "-n", str(snapshot / item.path)]) if bash and sandbox_ready else {"status": "unavailable", "detail": "Bash or the execution sandbox is unavailable."}
                        checks.append({"name": f"shell syntax: {item.path}", **result})
                runner = staging / "host-check.py"
                runner.write_text(_IMPORT_HARNESS, encoding="utf-8")
                python_paths = _json(_trusted_python_paths())
                import_token = uuid.uuid4().hex
                result = execute([executable, "-I", "-S", str(runner), str(snapshot), python_paths, import_token]) if sandbox_ready else {"status": "unavailable", "detail": "Imports were not executed because the sandbox is unavailable."}
                result = _require_completion(result, import_token)
                checks.append({"name": "python import harness", **result})
                app_token = uuid.uuid4().hex
                app_command = [executable, "-I", "-S", "-c", _APP_HARNESS, str(snapshot), python_paths, app_token]
                result = execute([*app_command, "--check"]) if sandbox_ready else {"status": "unavailable", "detail": "Deployment checks were not executed because the sandbox is unavailable."}
                result = _require_completion(result, app_token)
                checks.append({"name": "compiled deployment configuration", **result})
                if "application/__init__.py" in known:
                    app_token = uuid.uuid4().hex
                    app_command[-1] = app_token
                    result = execute([*app_command, "--application-check"]) if sandbox_ready else {"status": "unavailable", "detail": "Application lifecycle validation requires the sandbox."}
                    result = _require_completion(result, app_token)
                    checks.append({"name": "application lifecycle and tool contracts", **result})
                tests_run = 0
                if not selected:
                    checks.append({"name": "authored tests", "status": "fail", "detail": "Add meaningful stdlib unittest tests/test_*.py or offline tests/test_*.dom.cjs tests."})
                elif sandbox_ready and python_tests:
                    runner.write_text(_TEST_HARNESS, encoding="utf-8")
                    test_token = uuid.uuid4().hex
                    result = execute([executable, "-I", "-S", str(runner), str(snapshot), python_paths, test_token, *python_tests])
                    try:
                        summary = json.loads(result.get("detail", "").rsplit(_RESULT_MARKER + test_token + "=", 1)[1].splitlines()[0])
                        tests_run = summary["tests_run"] if type(summary.get("tests_run")) is int else 0
                        if tests_run <= 0 or summary.get("skipped", 0) >= tests_run:
                            result = {**result, "status": "fail", "detail": "No non-skipped authored tests were executed.\n" + result["detail"]}
                    except (IndexError, ValueError, TypeError, KeyError):
                        result = {**result, "status": "fail", "detail": "The host test harness did not return a valid completion count.\n" + result["detail"]}
                    checks.append({"name": "authored tests", **result})
                elif not sandbox_ready:
                    checks.append({"name": "authored tests", "status": "unavailable", "detail": "Tests were not executed because the sandbox is unavailable."})
                if dom_tests:
                    dom = (run_dom_tests(dom_tests, snapshot, snapshot, scratch, staging, execute)
                           if sandbox_ready else {"status": "unavailable", "detail": "DOM tests require the execution sandbox.",
                                                  "tests_run": 0, "test_results": []})
                    checks.append({"name": "authored DOM tests", **dom})
                    tests_run += dom["tests_run"]
            statuses = {item["status"] for item in checks}
            status = "fail" if "fail" in statuses else "unavailable" if "unavailable" in statuses else "pass"
            receipt = {"id": uuid.uuid4().hex, "issued_at": _now(), "issuer": "mobkit-launch-host",
                       "status": status, "digest": digest, "checks": checks, "tests_run": tests_run,
                       "selected_tests": selected, "complete_test_suite": selected == all_tests,
                       "ready_to_package": status == "pass" and tests_run > 0 and selected == all_tests,
                       "scope": "Offline syntax, imports, application contracts, and authored tests. No live connector, auth, provider, deployment, or hardware proof.",
                       "limits": {"process_seconds": CHECK_TIMEOUT_SECONDS, "execution_total_seconds": CHECK_TOTAL_SECONDS,
                                  "process_output_bytes": CHECK_OUTPUT_BYTES},
                       "sandbox": _sandbox_description()}
            receipt["signature"] = self._signed(receipt)
            state["last_check"] = receipt
            self._save_state(state)
            return json.loads(_json(receipt))


Workspace = ImplementationWorkspace


def _sandbox_executable():
    if sys.platform == "darwin":
        return shutil.which("sandbox-exec")
    if sys.platform == "linux":
        from .linux_sandbox import executable
        return executable()
    return None


def _sandbox_description():
    if sys.platform == "linux":
        from .linux_sandbox import DESCRIPTION
        return DESCRIPTION
    return "macOS sandbox-exec: no network, clean environment, project read-only, isolated writable scratch; no unsandboxed fallback."


@contextmanager
def _sandbox_launch(command, sandbox, profile, cwd, environment):
    if sys.platform == "linux":
        from .linux_sandbox import launch
        with launch(sandbox, profile, command, cwd, environment) as value:
            yield value
    else:
        yield [sandbox, "-p", profile, *command], [], None


def _check_environment(scratch):
    # Never inherit HOME, API keys, auth bindings, proxy variables, Python
    # startup hooks, NODE_OPTIONS, BASH_ENV, or any parent process secrets.
    return {"HOME": str(scratch), "TMPDIR": str(scratch), "TMP": str(scratch), "TEMP": str(scratch),
            "PATH": "/usr/bin:/bin", "LANG": "en_US.UTF-8", "LC_ALL": "en_US.UTF-8",
            "OPENSSL_CONF": "/dev/null",
            "PYTHONDONTWRITEBYTECODE": "1", "PYTHONNOUSERSITE": "1"}


def _trusted_python_paths():
    # Expose installed host dependencies explicitly, without executing .pth
    # files or importing the installer project into an untrusted process.
    return [str(Path(path).resolve()) for path in site.getsitepackages() if Path(path).is_dir()]


def _sandbox_profile(snapshot, scratch, staging, *, protected_reads=()):
    if sys.platform == "linux":
        from .linux_sandbox import profile
        reads = [*_trusted_python_paths(), *protected_reads]
        return profile(snapshot, scratch, staging, reads)
    # System and package-manager runtimes are trusted prerequisites. User
    # home, this workspace, keychains, launch agents and parent env are absent.
    # -I -S excludes all site initialization and .pth execution.
    readable = ["/System/Library", "/System/Volumes/Preboot", "/usr/lib", "/usr/libexec", "/usr/share", "/usr/bin",
                "/bin", "/sbin", "/private/var/db/dyld", "/dev/null", "/dev/urandom",
                "/opt/homebrew/Cellar", "/opt/homebrew/opt", "/Library/Frameworks", str(snapshot), str(staging / "host-check.py")]
    base = str(Path(sys.base_prefix).resolve())
    if base not in readable:
        readable.append(base)
    readable.extend(_trusted_python_paths())
    readable.extend(str(Path(path).resolve()) for path in protected_reads)
    literals = " ".join(f"(subpath {_json(path)})" for path in readable)
    return f"""(version 1)
(deny default)
(deny network*)
(deny process-fork)
(allow process-exec)
(allow sysctl-read)
(allow file-read-metadata)
(allow file-read* (literal "/") {literals} (subpath {_json(str(scratch))}))
(allow file-write* (subpath {_json(str(scratch))}) (literal "/dev/null"))
"""


def _terminate_process(process):
    """Stop and reap our exact child, accounting for exit and group races.

    Sandbox checks cannot fork. If signaling the fresh session fails, a direct
    PID signal is therefore sufficient, but only after Popen confirms that its
    child is still alive. A failed signal is never treated as confirmed cleanup.
    """
    if process.poll() is not None:
        return
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except (ProcessLookupError, PermissionError):
        if process.poll() is not None:
            return
        try:
            process.kill()
        except (ProcessLookupError, PermissionError) as error:
            # A short parser can finish during sandbox-exec startup before
            # either signal is admitted. Accept only an observed child exit,
            # with the same bounded reap window used after a successful kill.
            try:
                process.wait(timeout=TERMINATION_WAIT_SECONDS)
                return
            except subprocess.TimeoutExpired:
                raise WorkspaceError(f"Sandbox child {process.pid} could not be terminated; no verification receipt can be issued.") from error
    try:
        process.wait(timeout=TERMINATION_WAIT_SECONDS)
    except subprocess.TimeoutExpired as error:
        raise WorkspaceError(f"Sandbox child {process.pid} termination was not confirmed within the cleanup limit; no verification receipt can be issued.") from error


def _run_sandboxed(command, sandbox, profile, cwd, environment, *, timeout=None, input_bytes=None):
    # Only the trusted DOM launcher uses this bounded, host-owned channel.
    # Existing Python commands retain DEVNULL and receive no input or key.
    if input_bytes is not None and (type(input_bytes) is not bytes or not 0 < len(input_bytes) <= 64):
        raise WorkspaceError("Sandbox host input must be nonempty bounded bytes.")
    if not sandbox:
        return {"status": "unavailable", "detail": "The trusted OS sandbox is unavailable; generated code was not executed."}
    try:
        with _sandbox_launch(command, sandbox, profile, cwd, environment) as (argv, descriptors, completed):
            return _run_isolated_command(argv, descriptors, completed, cwd, environment,
                                         timeout=timeout, input_bytes=input_bytes)
    except ValueError as error:
        if sys.platform != "linux":
            raise
        from .linux_sandbox import LinuxSandboxError
        if isinstance(error, LinuxSandboxError):
            raise WorkspaceError(str(error)) from None
        raise


def _run_isolated_command(argv, descriptors, completed, cwd, environment, *, timeout, input_bytes):
    try:
        options = {"pass_fds": tuple(descriptors)} if descriptors else {}
        process = subprocess.Popen(argv, cwd=cwd, env=environment,
                                   stdin=subprocess.PIPE if input_bytes is not None else subprocess.DEVNULL,
                                   stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                   start_new_session=True, **options)
        if completed is not None:
            os.close(descriptors.pop())
    except OSError as error:
        return {"status": "unavailable", "detail": f"Could not start the sandbox: {error.strerror or type(error).__name__}."}
    output = bytearray()
    limit = None
    selector = selectors.DefaultSelector()
    selector.register(process.stdout, selectors.EVENT_READ)
    deadline = time.monotonic() + (CHECK_TIMEOUT_SECONDS if timeout is None else timeout)
    termination_attempted = False
    try:
        if input_bytes is not None:
            try:
                # At most 64 bytes fit in a fresh pipe without awaiting a
                # generated reader. Close immediately so its read sees EOF.
                process.stdin.write(input_bytes)
                process.stdin.close()
            except (OSError, ValueError):
                limit = "The trusted sandbox input channel closed before delivery."
        if limit:
            termination_attempted = True
            _terminate_process(process)
        while selector.get_map():
            if limit:
                break
            if time.monotonic() >= deadline:
                limit = "The executable check exceeded its time limit."
                break
            for key, _ in selector.select(min(0.1, max(0, deadline - time.monotonic()))):
                chunk = os.read(key.fileobj.fileno(), min(8192, CHECK_OUTPUT_BYTES + 1 - len(output)))
                if not chunk:
                    selector.unregister(key.fileobj)
                    continue
                output.extend(chunk)
                if len(output) > CHECK_OUTPUT_BYTES:
                    limit = "The executable check exceeded its output limit."
                    break
            if limit:
                break
        if limit:
            termination_attempted = True
            _terminate_process(process)
        try:
            process.wait(timeout=max(0.1, deadline - time.monotonic()))
        except subprocess.TimeoutExpired:
            limit = "The executable check exceeded its time limit."
            termination_attempted = True
            _terminate_process(process)
    finally:
        selector.close()
        if process.stdin is not None and not process.stdin.closed:
            try:
                process.stdin.close()
            except OSError:
                pass
        process.stdout.close()
        if not termination_attempted and process.poll() is None:
            _terminate_process(process)
        if completed is not None:
            from .linux_sandbox import confirm_cleanup
            confirm_cleanup(completed)
    detail = output[:CHECK_OUTPUT_BYTES].decode("utf-8", errors="replace").strip()
    if limit:
        return {"status": "fail", "detail": limit + ("\n" + detail if detail else "")}
    if process.returncode:
        return {"status": "fail", "detail": f"Exit {process.returncode}." + ("\n" + detail if detail else "")}
    return {"status": "pass", "detail": detail or "Completed successfully."}


def _require_completion(result, token):
    marker = _COMPLETION_MARKER + token
    if result["status"] == "pass" and marker not in result.get("detail", "").splitlines():
        return {"status": "fail", "detail": "The host harness did not finish; premature project termination is not a passing check.\n" + result.get("detail", "")}
    return {**result, "detail": result["detail"].replace(marker, "Host harness completed.")}


_APP_HARNESS = r'''import json, pathlib, runpy, sys, traceback
root = pathlib.Path(sys.argv[1]).resolve()
sys.path[:0] = [str(root), *json.loads(sys.argv[2])]
token = sys.argv[3]
sys.argv = [str(root / "app.py"), *sys.argv[4:]]
try:
    runpy.run_path(str(root / "app.py"), run_name="__main__")
except BaseException:
    traceback.print_exc()
    raise SystemExit(1)
print("MOBKIT_HOST_COMPLETED=" + token, flush=True)
'''

_IMPORT_HARNESS = r'''import importlib, json, pathlib, runpy, sys, traceback
root = pathlib.Path(sys.argv[1]).resolve()
sys.path[:0] = [str(root), *json.loads(sys.argv[2])]
token = sys.argv[3]
sys.argv = [str(root)]
try:
    runpy.run_path(str(root / "app.py"), run_name="__implementation_import__")
    if (root / "application" / "__init__.py").is_file():
        importlib.import_module("application")
except BaseException:
    traceback.print_exc()
    raise SystemExit(1)
print("Deployment and application entrypoint imports passed; no services started.")
print("MOBKIT_HOST_COMPLETED=" + token, flush=True)
'''

_TEST_HARNESS = r'''import importlib.util, json, pathlib, sys, traceback, unittest
root = pathlib.Path(sys.argv[1]).resolve()
sys.path[:0] = [str(root), *json.loads(sys.argv[2])]
token = sys.argv[3]
paths = sys.argv[4:]
sys.argv = [str(root)]
suite = unittest.TestSuite()
try:
    for index, relative in enumerate(paths):
        path = root / relative
        spec = importlib.util.spec_from_file_location("host_selected_test_" + str(index), path)
        module = importlib.util.module_from_spec(spec)
        sys.modules[spec.name] = module
        spec.loader.exec_module(module)
        suite.addTests(unittest.defaultTestLoader.loadTestsFromModule(module))
    result = unittest.TextTestRunner(verbosity=2).run(suite)
except BaseException:
    traceback.print_exc()
    raise SystemExit(1)
print("MOBKIT_HOST_TEST_RESULT=" + token + "=" + json.dumps({"tests_run": result.testsRun, "skipped": len(result.skipped)}), flush=True)
raise SystemExit(0 if result.wasSuccessful() and result.testsRun > len(result.skipped) else 1)
'''
