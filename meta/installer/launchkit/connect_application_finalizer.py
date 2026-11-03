"""Finalize one live Toolkit application preparation under its Forge owner.

The supplied caller retains the exclusive project lease and source selection.
Toolkit compiles and validates the candidate. Forge publishes that exact
immutable activation; the same Toolkit runtime must adopt it and report Ready.
There is no subprocess owner, native authority issuance or retry here.
"""
from __future__ import annotations

import asyncio
from dataclasses import dataclass
import hashlib
import inspect
import json
import math
import os
from pathlib import Path
import stat

from .connect_application_policy import _completed_application_policy_inputs_files
from .connect_installer_caller import (
    CustodyError, ExactFile, NativeInstallerCaller, _Preparation, _json,
    _path, _private_directory, _private_parent, _selection, _write_new,
)


def _sync_directory(path):
    descriptor = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def _write_bytes(path, content):
    """Stage original bytes with the existing caller's exclusive file rules."""
    path = _path(path, fresh=True)
    _private_parent(path)
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(descriptor, "wb") as stream:
        os.fchmod(stream.fileno(), 0o600)
        stream.write(content)
        stream.flush()
        os.fsync(stream.fileno())
    _sync_directory(path.parent)
    result = ExactFile(path, hashlib.sha256(content).hexdigest())
    result.read()
    return result


def application_command_executor(command, *, root: Path, timeout: float):
    """Adapt the existing app-owned command lane without another process owner.

    ``command`` is the maintained generated remote_operations.command helper.
    It alone owns its local process handle and deadline. An entered command is
    joined across repeated waiter cancellation; no result is cleanup or Ready.
    """
    if (not callable(command) or isinstance(timeout, bool)
            or not isinstance(timeout, (int, float)) or not math.isfinite(timeout) or timeout <= 0):
        raise CustodyError("The application command lane needs its existing owner and finite deadline.")

    async def execute(argv):
        from .connect_installer_caller import ProcessOutcome
        task = asyncio.create_task(asyncio.to_thread(command, argv, root=root,
            timeout=timeout, raw_outcome=True))
        cancelled = False
        while True:
            try:
                outcome = await asyncio.shield(task)
                break
            except asyncio.CancelledError:
                if task.cancelled():
                    raise
                cancelled = True
            except BaseException:
                if cancelled:
                    raise asyncio.CancelledError
                raise
        if cancelled:
            raise asyncio.CancelledError
        if (not isinstance(outcome, dict) or set(outcome) != {"returncode", "stdout", "stderr"}
                or type(outcome["returncode"]) is not int
                or not isinstance(outcome["stdout"], bytes) or not isinstance(outcome["stderr"], bytes)):
            raise CustodyError("The existing command owner did not retain exact exited streams.")
        return ProcessOutcome(**outcome)

    return execute


@dataclass(frozen=True)
class _Current:
    """Observed native pointer, not a replacement activation revision system."""

    target: str | None
    identity: tuple | None
    activation: ExactFile | None

    @classmethod
    def observe(cls, root):
        pointer = root / "current"
        try:
            before = pointer.lstat()
        except FileNotFoundError:
            return cls(None, None, None)
        if not stat.S_ISLNK(before.st_mode) or before.st_uid != os.getuid():
            raise CustodyError("The existing native current pointer is not an owned symlink.")
        target = os.readlink(pointer)
        selected = Path(target)
        selected = selected if selected.is_absolute() else root / selected
        selected = _path(selected)
        if selected.parent != root:
            raise CustodyError("The existing current activation is outside its selected deployment root.")
        # A dangling link is present and invalid, never an absent pointer.
        from .connect_installer_caller import _read, MAX_INPUT_BYTES
        content = _read(selected, MAX_INPUT_BYTES)
        activation = ExactFile(selected, hashlib.sha256(content).hexdigest())
        identity = (before.st_dev, before.st_ino, before.st_mtime_ns, before.st_ctime_ns)
        after = pointer.lstat()
        if identity != (after.st_dev, after.st_ino, after.st_mtime_ns, after.st_ctime_ns) or target != os.readlink(pointer):
            raise CustodyError("The native current pointer changed during observation.")
        return cls(target, identity, activation)

    def pin(self):
        return {"target": self.target, "identity": self.identity,
            "activation": self.activation.pin() if self.activation else None}

    def require_pointer(self, root):
        """Recheck the observed pointer without reading its activation bytes."""
        pointer = root / "current"
        try:
            info = pointer.lstat()
        except FileNotFoundError:
            if self.target is None:
                return
            raise CustodyError("The original native current pointer disappeared.")
        if (self.target is None or not stat.S_ISLNK(info.st_mode) or info.st_uid != os.getuid()
                or self.identity != (info.st_dev, info.st_ino, info.st_mtime_ns, info.st_ctime_ns)
                or os.readlink(pointer) != self.target):
            raise CustodyError("The native current pointer no longer matches the selected activation.")


def _identity(info):
    return (info.st_dev, info.st_ino, info.st_mode, info.st_uid,
        info.st_nlink, info.st_size, info.st_mtime_ns, info.st_ctime_ns)


@dataclass(frozen=True)
class _Publication:
    selected: dict
    files: tuple
    directories: tuple
    staged_pointer: tuple

    def require_files(self):
        # Full byte validation ran in the worker between equal snapshots. These
        # identities fence its handoff without hashing on the SDK's owner loop.
        try:
            for path, expected in self.directories:
                _private_directory(path)
                if _identity(path.lstat()) != expected:
                    raise CustodyError("A publication directory changed after authentication.")
            for path, expected in self.files:
                _private_parent(_path(path))
                if _identity(path.lstat()) != expected:
                    raise CustodyError("A publication file changed after authentication.")
        except OSError as error:
            raise CustodyError("The authenticated publication closure is no longer present.") from error

    def require_pointer(self, path, target):
        try:
            info = path.lstat()
            if (not stat.S_ISLNK(info.st_mode) or info.st_uid != os.getuid()
                    or _identity(info) != self.staged_pointer or os.readlink(path) != str(target)):
                raise CustodyError("The staged publication pointer changed before replacement.")
        except OSError as error:
            raise CustodyError("The staged publication pointer changed before replacement.") from error


class ApplicationPolicyFinalizer:
    """One-shot callback for ToolkitApplicationRuntime's live held preparation.

    Construct with the original plan/source pins and existing exclusive owner.
    Construction reads files and must run in the host's joined prepare worker,
    bracketed by that host's loop-side owner checks. The optional publication
    callback is a trusted synchronous source metadata fence used on the loop.
    The executor must be a separately owned command lane, not the Supervisor
    child slot occupied by the waiting application. Retained/mixed updates are
    outside this fresh finite-construction callback's native finish contract.
    """

    def __init__(self, caller: NativeInstallerCaller, *, preparation_plan: ExactFile,
            source_policy: ExactFile, activation_template: ExactFile,
            working_directory: Path, bundle_path: Path, activation_path: Path,
            source_files: tuple[ExactFile, ...] = (), require_publication_current=None):
        if (not isinstance(caller, NativeInstallerCaller)
                or any(not isinstance(pin, ExactFile) for pin in (preparation_plan, source_policy, activation_template))
                or not isinstance(source_files, tuple)
                or any(not isinstance(pin, ExactFile) for pin in source_files)
                or (require_publication_current is not None and not callable(require_publication_current))):
            raise CustodyError("Finalization requires the existing caller and exact original sources.")
        # Construction is a file reader inside the owner's joined prepare
        # worker. The host brackets construction with its real loop owner.
        caller._require_filesystem_current()
        plan = _json(preparation_plan.read())
        if not isinstance(plan, dict) or not isinstance(plan.get("deployment_root"), str):
            raise CustodyError("Finalization requires the original deployment root.")
        self.root = _private_directory(Path(plan["deployment_root"]))
        self.workspace = _private_directory(working_directory)
        self.bundle_path, self.activation_path = _path(bundle_path, fresh=True), _path(activation_path, fresh=True)
        self.next_pointer = _path(self.root[0] / (self.activation_path.name + ".current.next"), fresh=True)
        if (self.bundle_path.parent != self.root[0] or self.activation_path.parent != self.root[0]
                or self.bundle_path == self.activation_path
                or self.bundle_path in {self.root[0] / "current", self.next_pointer}
                or self.workspace[0] in {self.root[0], self.bundle_path, self.activation_path}
                or self.activation_path.name == "current"):
            raise CustodyError("Finalization needs separate fresh immutable outputs and a private workspace.")
        self.caller = caller
        self.require_publication_current = require_publication_current
        self.plan, self.policy, self.template = preparation_plan, source_policy, activation_template
        self.sources = tuple(dict.fromkeys((source_policy, activation_template, *source_files)))
        self.original = _Current.observe(self.root[0])
        self.selection = None
        self.publication_entered = False
        self._entered = False
        self._finalize_task = None
        self._record_owned = False
        self._check_sources()

    def _check_sources(self):
        self.caller._require_filesystem_current()
        if (_private_directory(self.root[0]) != self.root
                or _private_directory(self.workspace[0]) != self.workspace):
            raise CustodyError("The selected finalization directories changed.")
        for pin in (self.plan, *self.sources):
            _private_parent(_path(pin.path))
            pin.read()
        self.caller._require_filesystem_current()

    def _stage(self, manifest):
        self._check_sources()
        if _Current.observe(self.root[0]) != self.original:
            raise CustodyError("The native current activation changed before finalization.")
        bundle = self.caller._retain_completed_application_preparation_files(manifest,
            preparation_plan=self.plan, source_files=self.sources)
        inputs = _completed_application_policy_inputs_files(bundle, self.policy)
        _write_new(self.workspace[0] / "finalization.json", {"status": "entered",
            "preparation_plan": self.plan.pin(), "completed_manifest": manifest.pin(),
            "source_files": [pin.pin() for pin in self.sources],
            "original_current": self.original.pin(), "bundle": str(self.bundle_path),
            "activation": str(self.activation_path)})
        self._record_owned = True
        directory = self.workspace[0] / "artifacts"
        _path(directory, fresh=True)
        directory.mkdir(mode=0o700)
        _sync_directory(directory.parent)
        policy = _write_bytes(self.workspace[0] / "final-local-policy.json", inputs["local_policy_input"])
        artifacts = tuple(_write_bytes(directory / name, pin.read())
            for pin, name in zip(inputs["source_artifacts"], inputs["source_artifact_names"], strict=True))
        acquisition = inputs["acquisition"]
        artifacts += (_write_bytes(directory / acquisition.path, acquisition.content.encode()),)
        self._check_sources()
        bundle._authenticate_files()
        return bundle, policy, artifacts, _private_directory(directory)

    def _publication_snapshot(self, bundle, preparation, selection_file, selected, staged_pointer):
        _, outputs = _selection(json.dumps(selected, allow_nan=False).encode())
        pins = tuple(dict.fromkeys((self.plan, *self.sources, bundle.manifest,
            bundle.setup, bundle.application_tool_policy,
            *((bundle.resource_handoff,) if bundle.resource_handoff is not None else ()),
            *bundle.source_files, *(pin for pair in bundle.member_surfaces for pin in pair),
            selection_file, *preparation.artifacts, preparation.activation_template, *outputs,
            *(ExactFile(preparation.bundle_path / pin.path.name, pin.sha256)
                for pin in preparation.artifacts),
            *((self.original.activation,) if self.original.activation is not None else ()))))
        directories = tuple(dict.fromkeys((self.root[0], self.workspace[0],
            bundle.directory[0], bundle.artifact_directory[0], preparation.artifacts_directory[0],
            preparation.bundle_path, *(pin.path.parent for pin in pins))))
        return _Publication(selected,
            tuple((pin.path, _identity(_path(pin.path).lstat())) for pin in pins),
            tuple((path, _identity(_private_directory(path)[0].lstat())) for path in directories),
            staged_pointer)

    def _prepare_publication(self, bundle, preparation, selection_file):
        # The existing exclusive project owner makes this observation and
        # replacement serial. rename() alone is not an interprocess CAS.
        bundle._authenticate_files()
        self._check_sources()
        selected, _ = _selection(selection_file.read())
        preparation.require_inputs()
        selected = preparation.checked_selection(selected)
        # Both pointer paths share the native root/filesystem, even when the
        # owner's retained workspace is on a different volume.
        temporary = self.next_pointer
        _path(temporary, fresh=True)
        os.symlink(str(self.activation_path), temporary)
        staged_pointer = temporary.lstat()
        staged_identity = _identity(staged_pointer)
        _sync_directory(temporary.parent)
        _write_new(self.workspace[0] / "publication.json", {"status": "entered",
            "selection": selected, "original_current": self.original.pin()})
        self.publication_entered = True
        # Bind worker byte authentication to metadata checked again at the
        # loop-side commit, including every original and copied surface.
        publication = self._publication_snapshot(bundle, preparation, selection_file,
            selected, staged_identity)
        bundle._authenticate_files()
        self._check_sources()
        preparation.checked_selection(selected)
        if _Current.observe(self.root[0]) != self.original:
            raise CustodyError("The native current pointer no longer matches the selected activation.")
        publication.require_files()
        publication.require_pointer(temporary, self.activation_path)
        return publication

    def _publish(self, publication):
        # Owner callbacks must execute on their actual SDK loop. The separate
        # trusted publication fence can cover accepted source files outside the
        # preparation inputs. It is a synchronous metadata fence, not hashing.
        self.caller.require_current()
        if self.require_publication_current is not None:
            self.require_publication_current()
        publication.require_files()
        self.original.require_pointer(self.root[0])
        publication.require_pointer(self.next_pointer, self.activation_path)
        # No await separates these current source/owner checks from rename.
        # Retain the candidate before an ambiguous rename or fsync outcome.
        # Never infer rollback or re-prepare from an adoption/transport failure.
        self.selection = publication.selected
        os.replace(self.next_pointer, self.root[0] / "current")
        return publication.selected

    def _finish_publication(self, selected):
        # Always join synchronization after replacement, including owner loss.
        # The loop-side post-await fence decides whether to return the result.
        _sync_directory(self.root[0])
        if _Current.observe(self.root[0]).activation != ExactFile(
                self.activation_path, selected["activation"]["sha256"]):
            raise CustodyError("The published native pointer differs from the validated selection.")
        self.caller._require_filesystem_current()
        return selected

    async def finalize_selection(self, manifest: ExactFile):
        """Publish a validated candidate and retain wire refs for the owner.

        Toolkit's runtime owns joining this entered callback across cancellation.
        Calling again, including after uncertainty, is deliberately rejected.
        """
        if self._entered:
            raise CustodyError("This application finalization has already entered; retain its original outcome.")
        if not isinstance(manifest, ExactFile):
            raise CustodyError("Finalization requires the same live preparation's exact manifest.")
        # Claim before yielding so simultaneous invocations cannot both stage.
        self._entered = True
        requests = asyncio.Queue()
        loop = asyncio.get_running_loop()

        async def owner_call(function, *args):
            result = loop.create_future()
            requests.put_nowait((result, function, args))
            return await result

        # The operation keeps one shielded Task. Its original awaiting owner
        # services synchronous fences and publication on this exact Task.
        task = asyncio.create_task(self._finalize(manifest, _owner_call=owner_call))
        self._finalize_task = task
        request = asyncio.create_task(requests.get())
        cancelled = None
        try:
            while not task.done():
                try:
                    await asyncio.wait((task, request), return_when=asyncio.FIRST_COMPLETED)
                except asyncio.CancelledError as error:
                    cancelled = cancelled or error
                    # Keep the same operation and queued request across repeated
                    # waiter cancellation until their entered work settles.
                    continue
                if request.done():
                    result, function, args = request.result()
                    if not result.done():
                        try:
                            value = function(*args)
                            if inspect.isawaitable(value) or inspect.isasyncgen(value):
                                if inspect.iscoroutine(value):
                                    value.close()
                                raise CustodyError("The finalizer owner fence must be synchronous.")
                        except BaseException as error:
                            result.set_exception(error)
                        else:
                            result.set_result(value)
                    request = asyncio.create_task(requests.get())
        finally:
            # No queue waiter or unconsumed response may outlive this operation.
            # Cancelling the getter never cancels entered filesystem/native work.
            request.cancel()
            joined = asyncio.gather(request, return_exceptions=True)
            while not joined.done():
                try:
                    await asyncio.shield(joined)
                except asyncio.CancelledError as error:
                    cancelled = cancelled or error
            if not request.cancelled():
                request.result()[0].cancel()
            while not requests.empty():
                requests.get_nowait()[0].cancel()
        try:
            result = task.result()
        except BaseException as error:
            if cancelled is not None and error is not cancelled:
                raise BaseExceptionGroup("Finalization and its cancelled waiter retained their original outcomes.",
                    [cancelled, error]) from None
            raise
        if cancelled is not None:
            raise cancelled
        return result

    async def _finalize(self, manifest, *, _owner_call):
        status, error_type = "outcome_unknown", None
        try:
            bundle, policy, artifacts, directory = await self.caller._current_filesystem(self._stage, manifest, _owner_call=_owner_call)
            preparation = _Preparation(self.root, directory, self.bundle_path,
                self.activation_path, artifacts, self.template)
            prepared = await self.caller.prepare_application_policy(policy,
                artifacts_directory=directory[0], artifacts=artifacts, deployment_root=self.root[0],
                bundle_path=self.bundle_path, activation_template=self.template,
                activation_path=self.activation_path, attempt_path=self.workspace[0] / "prepare.json",
                source_files=(self.plan, bundle.manifest, *self.sources), _owner_call=_owner_call)
            if prepared.status != "receipt_verified" or prepared.selection is None:
                raise CustodyError("Native policy preparation did not return a verified candidate; retain its attempt.")
            selection_file = await self.caller._current_filesystem(_write_bytes, self.workspace[0] / "selection.json",
                (json.dumps(prepared.selection, ensure_ascii=True, allow_nan=False) + "\n").encode(),
                _owner_call=_owner_call)
            validated = await self.caller.validate_application_policy(selection_file,
                attempt_path=self.workspace[0] / "validate.json",
                source_files=(self.plan, bundle.manifest, *self.sources, *artifacts), _owner_call=_owner_call)
            if validated.status != "receipt_verified":
                raise CustodyError("Native policy validation did not return a verified outcome; retain its attempt.")
            publication = await self.caller._current_filesystem(self._prepare_publication,
                bundle, preparation, selection_file, _owner_call=_owner_call)
            selected = await _owner_call(self._publish, publication)
            await asyncio.to_thread(self._finish_publication, selected)
            await _owner_call(self.caller.require_current)
            status = "published"
            return selected
        except BaseException as error:
            error_type = type(error).__name__
            raise
        finally:
            # An existing entry belongs to its original attempt. Never overwrite
            # it if exclusive staging refused this caller before its own entry.
            if self._record_owned:
                await asyncio.to_thread(_write_new, self.workspace[0] / "finalization.outcome.json",
                    {"status": status, "error_type": error_type,
                        "publication_entered": self.publication_entered, "selection": self.selection})

    async def __call__(self, completed_manifest):
        # Import the genuine SDK before entry/publication. Missing dependencies
        # are not repaired with a MobKit compatibility alias.
        from meerkat_toolkit.host_control import HostBootstrapSelection, SelectedHostArtifact
        if not isinstance(completed_manifest, SelectedHostArtifact):
            raise CustodyError("The finalizer requires Toolkit's selected completed manifest.")
        selected = await self.finalize_selection(ExactFile(Path(completed_manifest.path), completed_manifest.sha256))
        return HostBootstrapSelection(deployment_root=selected["deployment_root"],
            **{name: SelectedHostArtifact(**selected[name]) for name in (
                "activation", "policy_manifest", "local_policy")})
