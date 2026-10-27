"""Frozen, isolated native design rehearsals with replayable Console evidence."""
from __future__ import annotations

import asyncio
import copy
from contextlib import AsyncExitStack, contextmanager
from dataclasses import asdict, dataclass
from datetime import datetime, timezone
import hashlib
from itertools import islice
import json
import os
from pathlib import Path, PurePosixPath
import re
import sys
import uuid

from .blueprint import Blueprint
from .deployment_deadlines import WORKER_STOP_TIMEOUT, PROCESS_FORCE_REAP_TIMEOUT
from .owned_worker import spawn_owned_worker, stop_owned_worker
from .architect import ArchitectUnavailable
from .compiler import resolved_blueprint
from .implementation import now, read_receipt, update_receipt
from .planner import ENV_KEYS, planner_capability
from .projects import DraftConflict, fingerprint
from .templates.storage_setup import write_private

REHEARSAL_SECONDS = 600
REHEARSAL_IDLE_SECONDS = 300
MAX_REHEARSAL_TURNS = 10
MAX_COMMAND_BYTES = 32_000
MAX_FIXTURE_FILES = 32
MAX_FIXTURE_BYTES = 500_000
MAX_FILE_BYTES = 200_000
MAX_TRACE_BYTES = 20_000_000
MAX_TRACE_FRAMES = 20_000
MAX_REJECTED_STARTS = 1000
SCOPE = "Native design rehearsal with isolated state and explicit fixture data. Not full application, live integration or deployment proof."
_ACTIVE = {"queued", "starting", "running", "waiting_input", "ending"}


@dataclass(frozen=True)
class StartNotAdmitted:
    """Durable host rejection, never worker cleanup or source verification."""
    guide_id: str
    guide_version: int
    mode: str
    start_request_id: str
    start_request_fingerprint: str
    http_status: int
    witness_version: int = 1
    status: str = "rejected"
    start_request_complete: bool = False

    def projection(self):
        return {key: value for key, value in asdict(self).items() if key != "http_status"}


@dataclass(frozen=True)
class ReplayNotAdmitted(StartNotAdmitted):
    """A durable rejection bound to the host's selected replay payload."""
    replay_version: int = 1
    original_rehearsal_id: str | None = None
    original_build_id: str | None = None
    original_source_digest: str | None = None
    build_id: str | None = None
    source_digest: str | None = None
    payload_sha256: str | None = None


def replay_request_fingerprint(guide_id, version, rehearsal_id, original_build_id,
                               original_source_digest, build_id, source_digest, payload_sha256):
    """Bind a fresh host-loaded journey, including both source generations."""
    return fingerprint({"replay_version": 1, "guide_id": guide_id, "guide_version": version,
        "original_rehearsal_id": rehearsal_id, "original_build_id": original_build_id,
        "original_source_digest": original_source_digest, "build_id": build_id,
        "source_digest": source_digest, "payload_sha256": payload_sha256})


def replay_rejection_matches(witness, guide_id, version, rehearsal_id, original_build_id,
                             original_source_digest, build_id, source_digest, start_request_id):
    """Validate public bindings without borrowing a generic error as proof."""
    if (not isinstance(witness, ReplayNotAdmitted) or type(version) is not int or version < 1
            or type(witness.guide_version) is not int or type(witness.witness_version) is not int
            or witness.replay_version != 1 or type(witness.replay_version) is not int
            or witness.mode != "application" or witness.status != "rejected"
            or witness.start_request_complete is not False or witness.witness_version != 1
            or type(witness.http_status) is not int or witness.http_status not in {404, 409, 422, 503}):
        return False
    expected = {"guide_id": guide_id, "guide_version": version,
        "original_rehearsal_id": rehearsal_id, "original_build_id": original_build_id,
        "original_source_digest": original_source_digest, "build_id": build_id,
        "source_digest": source_digest, "start_request_id": start_request_id}
    try:
        for identity in (guide_id, rehearsal_id, original_build_id, build_id, start_request_id):
            if type(identity) is not str or str(uuid.UUID(identity)) != identity:
                return False
    except (ValueError, TypeError, AttributeError):
        return False
    if (any(getattr(witness, name) != value for name, value in expected.items())
            or any(not isinstance(value, str) or not re.fullmatch(r"[0-9a-f]{64}", value)
                   for value in (original_source_digest, source_digest, witness.payload_sha256))):
        return False
    return witness.start_request_fingerprint == replay_request_fingerprint(guide_id, version,
        rehearsal_id, original_build_id, original_source_digest, build_id, source_digest, witness.payload_sha256)


def validate_start_request_id(value):
    if value is None:
        return
    try:
        if type(value) is not str or str(uuid.UUID(value)) != value:
            raise ValueError
    except (ValueError, TypeError, AttributeError):
        raise ValueError("Use a canonical UUID for the trial start action.") from None


def start_request_fingerprint(guide_id, version, text, *, flow_id=None, params=None, fixtures=None,
                              mode="design", build_id=None, application_fixtures=None, keep_open=False):
    """Bind the exact typed request, before trial input normalization."""
    value = {"guide_id": guide_id, "version": version, "text": text, "flow_id": flow_id,
             "params": params, "fixtures": fixtures, "mode": mode, "build_id": build_id,
             "application_fixtures": application_fixtures, "keep_open": keep_open}
    try:
        encoded = json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=True, allow_nan=False)
    except (ValueError, TypeError, RecursionError):
        raise ValueError("Use ordinary finite JSON trial input.") from None
    return hashlib.sha256(encoded.encode()).hexdigest()


def deadline_after(seconds):
    return datetime.fromtimestamp(datetime.now(timezone.utc).timestamp() + seconds, timezone.utc).isoformat()


def deadline_passed(value):
    try:
        return datetime.fromisoformat(value).timestamp() <= datetime.now(timezone.utc).timestamp()
    except (ValueError, TypeError):
        return True


def trial_turn_state(directory):
    """Only the owned worker writes turn acknowledgements and native bindings."""
    try:
        value = json.loads((directory / "turns.json").read_text())
        return value if isinstance(value, dict) else {}
    except (OSError, ValueError):
        return {}


def trial_control_state(directory):
    """Only the serial host admission path writes the command ledger."""
    try:
        value = json.loads((directory / "control.json").read_text())
        return value if isinstance(value, dict) else {}
    except (OSError, ValueError):
        return {}


def validate_trial(text, flow_id, params, fixtures, spec):
    """Validate bounded input before admitting any paid native work."""
    if not isinstance(text, str) or len(text) > 6000:
        raise ValueError("Use a sample message of at most 6000 characters.")
    text = text.strip()
    if not text and not flow_id:
        raise ValueError("Write a sample message or choose one of this proposal's flows.")
    if flow_id is not None and flow_id not in {flow.id for flow in spec.flows}:
        raise ValueError("Choose an actual flow from the current proposal.")
    if flow_id is None and params is not None:
        raise ValueError("Flow parameters require a selected flow.")
    if flow_id is not None and params is None:
        params = {"sample": text}
    try:
        encoded = json.dumps(params, ensure_ascii=True, allow_nan=False)
    except (ValueError, TypeError):
        raise ValueError("Use ordinary JSON flow parameters.") from None
    if len(encoded) > 30_000:
        raise ValueError("Flow parameters exceed the 30 KB rehearsal limit.")
    fixtures = fixtures if fixtures is not None else {}
    if not isinstance(fixtures, dict) or set(fixtures) - {"files", "notes"}:
        raise ValueError("Fixture data contains only files and notes.")
    files, notes = fixtures.get("files", {}), fixtures.get("notes", [])
    if not isinstance(files, dict) or len(files) > MAX_FIXTURE_FILES:
        raise ValueError("Supply at most 32 fixture text files.")
    total = 0
    for name, content in files.items():
        if (not isinstance(name, str) or not name or len(name) > 240 or "\\" in name
                or any(ord(char) < 32 for char in name)):
            raise ValueError("Use visible, project-relative fixture paths.")
        path = PurePosixPath(name)
        if path.is_absolute() or str(path) != name or any(part in {"", ".", ".."} or part.startswith(".") for part in path.parts):
            raise ValueError("Use visible, project-relative fixture paths.")
        if not isinstance(content, str) or len(content.encode("utf-8")) > MAX_FILE_BYTES:
            raise ValueError("Each fixture is UTF-8 text of at most 200 KB.")
        total += len(content.encode("utf-8"))
    if any(any(other.startswith(name + "/") for other in files) for name in files):
        raise ValueError("A fixture path cannot be both a file and a parent folder.")
    if (not isinstance(notes, list) or len(notes) > 32
            or any(not isinstance(note, str) or not note.strip() or len(note) > 10000 for note in notes)):
        raise ValueError("Supply at most 32 fixture notes of 1-10000 characters.")
    total += sum(len(note.encode("utf-8")) for note in notes)
    if total > MAX_FIXTURE_BYTES:
        raise ValueError("Fixture data exceeds the 500 KB total limit.")
    return text, copy.deepcopy(params), {"files": copy.deepcopy(files), "notes": copy.deepcopy(notes)}


def masked_capabilities(spec):
    """Declare capabilities absent from a trial, without invented tool results."""
    result = []
    for item in spec.agent_classes:
        masked = [name for name in ("builtins", "shell", "workgraph", "schedule", "knowledge")
                  if getattr(item.tools, name)]
        masked.extend("mcp:" + grant.connection_id for grant in item.tools.mcp)
        masked.extend("rust_bundle:" + name for name in item.tools.rust_bundles)
        for identity in item.identities():
            result.append({"identity": identity, "class_id": item.id,
                           "masked": masked + ["application adapters", "live inbound channels", "mob management"],
                           "available": ["native peer comms"]
                                + (["isolated identity memory"] if item.tools.memory else [])
                                + (["fixture workspace (read only)"] if item.tools.workspace else [])
                                + (["isolated notes " + item.tools.notes] if item.tools.notes != "none" else [])})
    return result


def worker_environment(directory, spec):
    """Only selected model-provider keys cross into the trusted trial worker."""
    root = Path(__file__).resolve().parents[1]
    environment = {"PATH": os.defpath, "PYTHONPATH": str(root), "PYTHONNOUSERSITE": "1",
                   "HOME": str(directory / "home"), "TMPDIR": str(directory / "tmp"),
                   "LANG": "en_US.UTF-8", "LC_ALL": "en_US.UTF-8"}
    for provider in {item.provider for item in spec.agent_classes}:
        key = ENV_KEYS[provider]
        if os.getenv(key):
            environment[key] = os.environ[key]
    return environment


def missing_trial_providers(spec):
    """Keep proposed models; host model custody covers only its original pair."""
    from .guide_runtime import selected_runtime
    configuration = selected_runtime().model_configuration
    return sorted({item.provider for item in spec.agent_classes
        if not os.getenv(ENV_KEYS[item.provider]) and not (configuration is not None
            and (item.provider, item.model) == (configuration["provider"], configuration["model"]))})


def trace_cursor(value):
    if value is None or value == "console:0":
        return 0
    if isinstance(value, str) and value.startswith("rehearsal:"):
        suffix = value.removeprefix("rehearsal:")
        if suffix.isdecimal() and int(suffix) >= 0:
            return int(suffix)
    raise ValueError("Use a rehearsal replay cursor returned by this trial.")


def rehearsal_receipt(directory):
    try:
        return read_receipt(directory)
    except (OSError, ValueError):
        raise LookupError("This design rehearsal could not be found.") from None


def application_preview_projection(receipt, frozen, state):
    """Expose only an inert snapshot of this current frozen application trial."""
    from .application_preview import ApplicationPreviewError, validate_application_preview
    from .artifact_binding import frozen_matches
    if (receipt.get("mode") != "application" or receipt.get("current") is not True
            or not frozen_matches(receipt, frozen, state)):
        return None
    source = frozen.get("application_source")
    preview = receipt.get("application_preview")
    revision = receipt.get("application_observation_revision")
    if (not isinstance(source, dict) or source != receipt.get("application_source")
            or not isinstance(preview, dict) or type(revision) is not int or not 1 <= revision <= 2001
            or type(preview.get("observation_revision")) is not int
            or preview["observation_revision"] != revision
            or preview.get("trial_id") != receipt.get("rehearsal_id")
            or preview.get("build_id") != source.get("build_id")
            or not source.get("build_id") or preview.get("source_digest") != source.get("source_digest")
            or not source.get("source_digest") or type(preview.get("guide_version")) is not int
            or preview["guide_version"] != frozen.get("guide_version")):
        return None
    keys = {"status", "scope", "sanitization", "title", "html", "detail", "live_ready", "document_sha256",
            "trial_id", "build_id", "source_digest", "guide_version", "observation_revision"}
    projection = {key: value for key, value in preview.items() if key in keys}
    if projection.get("status") in ("unsupported", "failed"):
        # Unavailable snapshots may retain no document. Only explanatory copy
        # is replaced; the exact bindings and static safety markers stay checked.
        if projection.get("html") is not None or projection.get("document_sha256") is not None:
            return None
        projection.update(title=None, html=None, document_sha256=None,
            detail=("This application trial has no render_preview() hook." if projection["status"] == "unsupported"
                    else "The application preview could not be rendered safely. Its callback observations remain available."))
    try:
        projection = validate_application_preview(projection)
    except (ApplicationPreviewError, ValueError, TypeError, RecursionError):
        return None
    if projection["status"] == "available":
        projection["detail"] = None
    return projection


class RehearsalService:
    def __init__(self, root, guide, operations=None, *, implementation=None):
        self.root = Path(root) / "rehearsals"
        self.guide, self.operations = guide, operations
        self.implementation = implementation
        self.lock = asyncio.Lock()
        self.tasks, self.processes = {}, {}

    async def _guide(self, identity):
        if self.operations is not None:
            return await self.operations.read(self.guide, identity)
        return await self.guide.get(identity)

    def _directory(self, guide_id, rehearsal_id=None):
        try:
            if str(uuid.UUID(guide_id)) != guide_id:
                raise ValueError
            if rehearsal_id is not None and str(uuid.UUID(rehearsal_id)) != rehearsal_id:
                raise ValueError
        except (ValueError, TypeError, AttributeError):
            raise LookupError("This design rehearsal could not be found.") from None
        parent = self.root / guide_id
        if rehearsal_id is None:
            try:
                rehearsal_id = json.loads((parent / "latest.json").read_text())["rehearsal_id"]
            except (OSError, ValueError, KeyError):
                raise LookupError("No design rehearsal has been started for this setup.") from None
            return self._directory(guide_id, rehearsal_id)
        return parent / rehearsal_id

    def _rejected_start_path(self, guide_id, request_id):
        # UUID validation is shared with the ordinary trial reservation path.
        self._directory(guide_id, request_id)
        return self.root / "rejected-starts" / guide_id / (request_id + ".json")

    def _rejection_ledger_full(self, parent):
        return parent.exists() and sum(1 for _ in islice(parent.iterdir(), MAX_REJECTED_STARTS)) >= MAX_REJECTED_STARTS

    def _durable_rejected_start(self, path, binding):
        """Both creation and replay must prove the bound fence is durable."""
        try:
            if path.is_symlink() or not path.is_file() or path.stat().st_size > 4096:
                raise ValueError
            value = json.loads(path.read_text())
            witness_type = ReplayNotAdmitted if "replay_version" in value else StartNotAdmitted
            witness = witness_type(**value)
            if (type(witness.witness_version) is not int or witness.witness_version != 1
                    or witness.status != "rejected" or witness.start_request_complete is not False
                    or type(witness.http_status) is not int or witness.http_status not in {404, 409, 422, 503}
                    or any(getattr(witness, key) != item for key, item in binding.items())
                    or type(witness.guide_version) is not int or witness.mode not in {"design", "application"}):
                raise ValueError
            if isinstance(witness, ReplayNotAdmitted) and not replay_rejection_matches(witness,
                    witness.guide_id, witness.guide_version, witness.original_rehearsal_id,
                    witness.original_build_id, witness.original_source_digest,
                    witness.build_id, witness.source_digest, witness.start_request_id):
                raise ValueError
            # A previous write may have replaced the file before a directory
            # flush failed. Existence alone cannot authorize a negative response.
            for folder in (path.parent, path.parent.parent, self.root, self.root.parent):
                descriptor = os.open(folder, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0))
                try:
                    os.fsync(descriptor)
                finally:
                    os.close(descriptor)
            if path.is_symlink() or not path.is_file() or path.stat().st_size > 4096 or json.loads(path.read_text()) != asdict(witness):
                raise ValueError
        except (ValueError, TypeError, OSError):
            raise DraftConflict("This rejected trial action has unverified durability or different saved input. Its admission remains blocked; recover it when host storage is available.") from None
        return witness

    def _replay_rejected_start(self, path, binding):
        witness = self._durable_rejected_start(path, binding)
        error_type = {404: LookupError, 409: DraftConflict, 422: ValueError, 503: ArchitectUnavailable}[witness.http_status]
        error = error_type("This request was not admitted. Keep its saved history, correct the input or host requirements, and prepare a new trial request.")
        error.trial_start_rejection = witness
        raise error

    @contextmanager
    def _start_rejection_boundary(self, reserved, binding):
        try:
            yield
        except (LookupError, DraftConflict, ArchitectUnavailable, ValueError) as error:
            request_id = binding.get("start_request_id") if binding else None
            if (reserved is not None and binding and binding["mode"] in {"design", "application"}
                    and type(binding["guide_version"]) is int and not reserved.exists() and not reserved.is_symlink()
                    and request_id not in self.tasks and request_id not in self.processes):
                status = (409 if isinstance(error, DraftConflict) else 503 if isinstance(error, ArchitectUnavailable)
                          else 404 if isinstance(error, LookupError) else 422)
                witness_type = ReplayNotAdmitted if "replay_version" in binding else StartNotAdmitted
                witness = witness_type(**binding, http_status=status)
                path = self._rejected_start_path(binding["guide_id"], request_id)
                try:
                    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
                    if self._rejection_ledger_full(path.parent) or path.exists() or path.is_symlink():
                        raise OSError("Rejected-action ledger cannot accept this witness.")
                    write_private(path, asdict(witness))
                    if self._durable_rejected_start(path, binding) != witness:
                        raise DraftConflict("Rejected-action witness was not retained.")
                except (OSError, ValueError, DraftConflict):
                    # An ordinary error response does not establish non-admission.
                    pass
                else:
                    error.trial_start_rejection = witness
            raise

    async def submit(self, guide_id, version, text, *, flow_id=None, params=None, fixtures=None,
                     mode="design", build_id=None, application_fixtures=None, keep_open=False,
                     start_request_id=None):
        async with self.lock:
            return await self._submit_locked(guide_id, version, text, flow_id=flow_id, params=params,
                fixtures=fixtures, mode=mode, build_id=build_id, application_fixtures=application_fixtures,
                keep_open=keep_open, start_request_id=start_request_id)

    async def replay(self, guide_id, version, rehearsal_id, original_build_id,
                     original_source_digest, build_id, source_digest, start_request_id):
        """Re-admit one verified historical journey against its repaired source.

        Only the host capsule supplies content. An existing action recovers its
        original admission before checking today's selection or provider state.
        The historical trial is never relabelled current or modified here.
        """
        async with self.lock:
            validate_start_request_id(start_request_id)
            if start_request_id is None or type(version) is not int or version < 1:
                raise ValueError("Use a version and a fresh keyed action for this replay.")
            self._directory(guide_id, rehearsal_id)
            for identity in (original_build_id, build_id):
                validate_start_request_id(identity)
                if identity is None:
                    raise ValueError("Choose the original and repaired application builds.")
            if any(not isinstance(value, str) or not re.fullmatch(r"[0-9a-f]{64}", value)
                   for value in (original_source_digest, source_digest)):
                raise ValueError("Use the checked original and repaired source digests.")
            public = {"replay_version": 1, "original_rehearsal_id": rehearsal_id,
                "original_build_id": original_build_id, "original_source_digest": original_source_digest,
                "build_id": build_id, "source_digest": source_digest}
            reserved = self._directory(guide_id, start_request_id)
            if reserved.exists() or reserved.is_symlink():
                return await self._recover_replay(reserved, guide_id, version, start_request_id, public)
            rejected = self._rejected_start_path(guide_id, start_request_id)
            if rejected.exists() or rejected.is_symlink():
                try:
                    witness = ReplayNotAdmitted(**json.loads(rejected.read_text()))
                    if not replay_rejection_matches(witness, guide_id, version, rehearsal_id,
                            original_build_id, original_source_digest, build_id, source_digest, start_request_id):
                        raise ValueError
                except (OSError, ValueError, TypeError, RecursionError):
                    raise DraftConflict("This replay action has different or unverified rejected input. Preserve it and recover its saved history.") from None
                binding = {"guide_id": guide_id, "guide_version": version, "mode": "application",
                    "start_request_id": start_request_id, "start_request_fingerprint": witness.start_request_fingerprint,
                    **public, "payload_sha256": witness.payload_sha256}
                self._replay_rejected_start(rejected, binding)
            if self.implementation is None:
                raise DraftConflict("The repaired application's checked source is unavailable.")
            async with self.implementation.lock:
                binding = self._replay_negative_binding(guide_id, version, start_request_id, public)
                with self._start_rejection_boundary(reserved, binding):
                    state = await self._guide(guide_id)
                    if state.get("version") != version or state.get("operation", {}).get("status") == "running":
                        raise DraftConflict("This setup changed. Reload before replaying the selected job.")
                    from .trial_replay import load_trial_replay
                    try:
                        capsule = await load_trial_replay(self.implementation, guide_id, rehearsal_id,
                            original_build_id, original_source_digest, build_id, source_digest, state=state)
                    except (OSError, UnicodeError, KeyError, TypeError, RecursionError):
                        raise DraftConflict("The saved job's host evidence is unavailable. Recover its saved state before retrying this action.") from None
                    initial = capsule["initial"]
                    continuations = copy.deepcopy(capsule["continuations"])
                    metadata = {**public, "payload_sha256": capsule["payload_sha256"],
                                "reference": copy.deepcopy(capsule["reference"])}
                    if initial.get("flow_id") is not None and continuations:
                        raise DraftConflict("A flow replay cannot attach a text continuation prefix.")
                    return await self._submit_locked(guide_id, version, initial["text"],
                        flow_id=initial.get("flow_id"), params=initial.get("params"), fixtures=initial.get("fixtures"),
                        mode="application", build_id=build_id, application_fixtures=initial.get("application_fixtures"),
                        keep_open=bool(continuations), start_request_id=start_request_id,
                        _replay={"metadata": metadata, "continuations": continuations})

    def _replay_negative_binding(self, guide_id, version, request_id, public):
        """Bind a rejection to a host-retained reference, never admit content.

        A missing capsule or changed selection must not trap a pending action
        whose job hash is still known. Reading its saved reference establishes
        only which request was rejected. Actual admission still requires the
        complete capsule, ancestry, current source and cleanup checks above.
        """
        from .trial_replay import _json, REFERENCE_KEYS, TrialReplayUnavailable
        try:
            directory = self.implementation._directory(guide_id, public["build_id"])
            frozen, _ = _json(directory / "input.json", directory)
            reference = frozen.get("trial_replay")
            expected = {"guide_id": guide_id, "rehearsal_id": public["original_rehearsal_id"],
                "original_build_id": public["original_build_id"],
                "original_source_digest": public["original_source_digest"]}
            if (not isinstance(reference, dict) or set(reference) != REFERENCE_KEYS
                    or type(reference.get("version")) is not int or reference["version"] != 1
                    or frozen.get("guide_id") != guide_id or directory.name != public["build_id"]
                    or any(reference.get(key) != value for key, value in expected.items())
                    or any(not isinstance(reference.get(key), str)
                        or not re.fullmatch(r"[0-9a-f]{64}", reference[key])
                        for key in ("capsule_sha256", "payload_sha256"))):
                return None
            digest = replay_request_fingerprint(guide_id, version, public["original_rehearsal_id"],
                public["original_build_id"], public["original_source_digest"],
                public["build_id"], public["source_digest"], reference["payload_sha256"])
            return {"guide_id": guide_id, "guide_version": version, "mode": "application",
                "start_request_id": request_id, "start_request_fingerprint": digest,
                **public, "payload_sha256": reference["payload_sha256"]}
        except (TrialReplayUnavailable, LookupError, DraftConflict, OSError, ValueError, TypeError,
                KeyError, AttributeError, RecursionError):
            return None

    async def _recover_replay(self, directory, guide_id, version, request_id, public):
        try:
            if directory.is_symlink() or (directory / "input.json").is_symlink():
                raise ValueError
            if (directory / "receipt.json").is_symlink():
                raise ValueError
            frozen = json.loads((directory / "input.json").read_text())
            receipt = rehearsal_receipt(directory)
            saved = frozen.get("trial_replay")
            if (not isinstance(saved, dict) or not isinstance(receipt.get("trial_replay"), dict)
                    or any(saved.get(key) != value or receipt["trial_replay"].get(key) != value
                           for key, value in public.items())
                    or not isinstance(saved.get("payload_sha256"), str)
                    or not re.fullmatch(r"[0-9a-f]{64}", saved["payload_sha256"])
                    or receipt["trial_replay"].get("payload_sha256") != saved["payload_sha256"]):
                raise ValueError
            digest = replay_request_fingerprint(guide_id, version, public["original_rehearsal_id"],
                public["original_build_id"], public["original_source_digest"],
                public["build_id"], public["source_digest"], saved["payload_sha256"])
        except (OSError, ValueError, TypeError, KeyError, LookupError, RecursionError):
            raise DraftConflict("This replay action has different or incomplete saved input. Recover its saved trial before starting a distinct action.") from None
        return await self._replay_start(directory, guide_id, version, request_id, digest)

    async def _submit_locked(self, guide_id, version, text, *, flow_id=None, params=None, fixtures=None,
                     mode="design", build_id=None, application_fixtures=None, keep_open=False,
                     start_request_id=None, _replay=None):
        validate_start_request_id(start_request_id)
        request_fingerprint = None
        reserved, binding = None, None
        if start_request_id is not None:
            if _replay is None:
                request_fingerprint = start_request_fingerprint(guide_id, version, text,
                    flow_id=flow_id, params=params, fixtures=fixtures, mode=mode, build_id=build_id,
                    application_fixtures=application_fixtures, keep_open=keep_open)
            else:
                metadata = _replay["metadata"]
                request_fingerprint = replay_request_fingerprint(guide_id, version,
                    metadata["original_rehearsal_id"], metadata["original_build_id"],
                    metadata["original_source_digest"], metadata["build_id"],
                    metadata["source_digest"], metadata["payload_sha256"])
            # Keep the hashed input fixed across awaited guide/source reads.
            params, fixtures, application_fixtures = copy.deepcopy((params, fixtures, application_fixtures))
            reserved = self._directory(guide_id, start_request_id)
            if reserved.exists() or reserved.is_symlink():
                return await self._replay_start(reserved, guide_id, version, start_request_id, request_fingerprint)
            binding = {"guide_id": guide_id, "guide_version": version, "mode": mode,
                       "start_request_id": start_request_id, "start_request_fingerprint": request_fingerprint}
            if _replay is not None:
                binding.update({name: _replay["metadata"][name] for name in (
                    "replay_version", "original_rehearsal_id", "original_build_id", "original_source_digest",
                    "build_id", "source_digest", "payload_sha256")})
            rejected = self._rejected_start_path(guide_id, start_request_id)
            if rejected.exists() or rejected.is_symlink():
                self._replay_rejected_start(rejected, binding)
            if self._rejection_ledger_full(rejected.parent):
                raise DraftConflict("This setup's rejected-request history is full. Existing requests remain preserved; no new keyed trial can be admitted.")
        with self._start_rejection_boundary(reserved, binding):
            if type(keep_open) is not bool or keep_open and flow_id is not None:
                raise ValueError("Keep an open conversation only for a text trial, without a flow.")
            if mode not in {"design", "application"}:
                raise ValueError("Choose a design or application trial.")
            if mode == "design" and (build_id is not None or application_fixtures is not None):
                raise ValueError("Application source and fixtures require an application trial.")
            state = await self._guide(guide_id)
            if type(version) is not int or state["version"] != version:
                raise DraftConflict("This setup changed. Reload before trying its design.")
            if state.get("operation", {}).get("status") == "running":
                raise DraftConflict("Wait for the guide's current revision before trying the design.")
            proposal = state.get("proposal") or state.get("blueprint")
            if not proposal:
                raise ValueError("Get a concrete proposed agent system before trying the design.")
            spec = Blueprint.model_validate(proposal)
            text, params, fixtures = validate_trial(text, flow_id, params, fixtures, spec)
            application_source, application_files = None, None
            scope = SCOPE
            if mode == "application":
                from .application_snapshot import checked_trial_source
                from .application_trial import SCOPE as APPLICATION_SCOPE, validate_application_fixtures
                application_fixtures = validate_application_fixtures(application_fixtures)
                application_source, application_files = await checked_trial_source(
                    self.implementation, guide_id, state, build_id)
                scope = APPLICATION_SCOPE
            capability = planner_capability()
            if not capability["available"]:
                raise ArchitectUnavailable(capability["reason"])
            missing = missing_trial_providers(spec)
            if missing:
                raise ArchitectUnavailable("Configure the selected model providers on the host before rehearsing: " + ", ".join(sorted(missing)))
            try:
                previous = self._directory(guide_id)
                receipt = rehearsal_receipt(previous)
            except LookupError:
                receipt = None
            if receipt and previous.name in self.processes:
                raise DraftConflict("The earlier rehearsal worker has not stopped. Cancel it before starting another trial.")
            if receipt and (receipt.get("application_stop_confirmed") is False
                    or receipt.get("mode") == "application"
                    and receipt.get("application_stop_confirmed") is not True
                    and previous.name not in self.tasks):
                raise DraftConflict("The previous application's sandbox cleanup is unconfirmed. A stopped native worker does not prove its application child stopped; another trial cannot be admitted.")
            if receipt and (receipt.get("worker_stop_confirmed") is False
                    or receipt.get("worker_stop_confirmed") is not True
                    and previous.name not in self.tasks):
                raise DraftConflict("The previous trial's worker cleanup is unconfirmed after losing its host ownership. Another trial cannot be admitted.")
            if receipt and receipt["status"] in _ACTIVE:
                if previous.name in self.tasks:
                    raise DraftConflict("A native design rehearsal is already running for this setup.")
                update_receipt(previous, status="interrupted", native_console_url=None,
                               detail="The earlier host stopped. Its saved native trace remains available.")
            identity = start_request_id or str(uuid.uuid4())
            directory = self._directory(guide_id, identity)
            directory.mkdir(parents=True, mode=0o700)
            for name in ("home", "tmp"):
                (directory / name).mkdir(mode=0o700)
            expires_at = deadline_after(REHEARSAL_SECONDS)
            frozen = {"guide_id": guide_id, "guide_version": version,
                      "blueprint": spec.model_dump(mode="json"), "brief": copy.deepcopy(state.get("brief", {})),
                      "text": text, "flow_id": flow_id, "params": params, "fixtures": fixtures,
                      "duration_seconds": REHEARSAL_SECONDS, "scope": scope, "mode": mode,
                      "application_source": application_source, "application_fixtures": application_fixtures}
            frozen["keep_open"] = keep_open
            frozen["expires_at"] = expires_at
            resolved = resolved_blueprint(spec)
            initial = {"rehearsal_id": identity, "guide_id": guide_id, "guide_version": version,
                       "blueprint_hash": fingerprint(frozen["blueprint"]), "brief_hash": fingerprint(frozen["brief"]),
                       "blueprint": frozen["blueprint"], "identities": resolved["identities"], "edges": resolved["edges"],
                       "text": text, "flow_id": flow_id, "params": params, "status": "queued",
                       "scope": scope, "mode": mode, "application_source": application_source,
                       "live_ready": False, "connected": False,
                       "masked_capabilities": masked_capabilities(spec), "native_console_url": None,
                       "native_admission": None, "flow_run_id": None, "frame_count": 0,
                       "worker_stop_confirmed": None,
                       "trace_complete": False, "detail": "Preparing the actual proposed mob in isolated state.",
                       "created_at": now(), "updated_at": now(), "duration_seconds": REHEARSAL_SECONDS}
            initial.update(keep_open=keep_open, turn_count=0, current_turn=0, turns=[],
                           idle_seconds=REHEARSAL_IDLE_SECONDS, max_turns=MAX_REHEARSAL_TURNS,
                           expires_at=expires_at, idle_expires_at=None)
            if _replay is not None:
                metadata = copy.deepcopy(_replay["metadata"])
                metadata.update(status="queued", expected_turns=0 if flow_id is not None
                                else 1 + len(_replay["continuations"]), completed_turns=0,
                                scope="Fresh native user journey from saved input. Model decisions may differ; domain acceptance remains separate.")
                frozen["trial_replay"] = copy.deepcopy(metadata)
                frozen["replay_continuations"] = copy.deepcopy(_replay["continuations"])
                initial["trial_replay"] = metadata
            if keep_open and mode == "application":
                initial["application_stop_confirmed"] = None
            if start_request_id is not None:
                binding = {"start_request_id": start_request_id, "start_request_fingerprint": request_fingerprint}
                frozen.update(binding)
                initial.update(binding, start_request_complete=True)
                # A reserved action is never retried as fresh native work, even
                # if freezing or final publication fails after this write.
                write_private(directory / "input.json", frozen)
                write_private(directory / "receipt.json", {**initial, "status": "unavailable",
                    "start_request_complete": False,
                    "detail": "This trial's preparation did not finish. Its saved input is preserved; recovering this action does not start another worker."})
            if application_files is not None:
                from .application_snapshot import write_trial_source, checked_trial_source
                await asyncio.to_thread(write_trial_source, directory, application_files)
                latest_state = await self._guide(guide_id)
                latest_source, _ = await checked_trial_source(self.implementation, guide_id, latest_state, build_id)
                if latest_state["version"] != version or latest_source != application_source:
                    raise DraftConflict("The application or setup changed while freezing this trial. Reload before trying it.")
            write_private(directory / "input.json", frozen)
            latest_state = await self._guide(guide_id)
            from .artifact_binding import matches
            if (latest_state["version"] != version or latest_state.get("operation", {}).get("status") == "running"
                    or not matches(initial, latest_state)):
                raise DraftConflict("The setup changed before rehearsal admission. Reload before trying it.")
            write_private(directory / "receipt.json", initial)
            write_private(directory.parent / "latest.json", {"rehearsal_id": identity})
            task = asyncio.create_task(self._run(directory))
            self.tasks[identity] = task
            task.add_done_callback(lambda done: done.exception() if not done.cancelled() else None)
            return await self.status(guide_id, identity, state=state)

    async def _replay_start(self, directory, guide_id, version, start_request_id, request_fingerprint):
        """Recover only the original host action, without changing latest or launching."""
        try:
            if directory.is_symlink() or not directory.is_dir():
                raise ValueError
            for name in ("input.json", "receipt.json"):
                if (directory / name).is_symlink():
                    raise ValueError
            frozen = json.loads((directory / "input.json").read_text())
            receipt = rehearsal_receipt(directory)
            expected = {"guide_id": guide_id, "guide_version": version,
                        "start_request_id": start_request_id, "start_request_fingerprint": request_fingerprint}
            if (not isinstance(frozen, dict) or not isinstance(receipt, dict)
                    or any(frozen.get(key) != value or receipt.get(key) != value for key, value in expected.items())
                    or receipt.get("rehearsal_id") != start_request_id
                    or type(receipt.get("start_request_complete")) is not bool):
                raise ValueError
        except (OSError, ValueError, TypeError, KeyError, LookupError, RecursionError):
            raise DraftConflict("This trial start action has different or incomplete saved input. Check its saved trial before starting a distinct action.") from None
        return await self.status(guide_id, start_request_id)

    def _owned_process(self, directory):
        task, process = self.tasks.get(directory.name), self.processes.get(directory.name)
        return process if task and not task.done() and process and process.returncode is None else None

    async def _command(self, directory, command):
        process = self._owned_process(directory)
        if process is None or process.stdin is None or process.stdin.is_closing():
            raise DraftConflict("This trial's owned worker is no longer available. Start another trial.")
        encoded = (json.dumps(command, ensure_ascii=False, allow_nan=False, separators=(",", ":")) + "\n").encode()
        if len(encoded) > MAX_COMMAND_BYTES:
            raise ValueError("The trial command exceeds its bounded input limit.")
        # Persist before sending. An ambiguous write remains reserved and must
        # never authorize another paid turn under a different key.
        write_private(directory / "control.json", {"command": command, "status": "sent"})
        try:
            process.stdin.write(encoded)
            await asyncio.wait_for(process.stdin.drain(), 5)
        except (OSError, ConnectionError, asyncio.TimeoutError) as error:
            write_private(directory / "control.json", {"command": command, "status": "uncertain"})
            raise DraftConflict("The trial command's delivery is uncertain. Check its activity or End it; do not resend.") from error

    async def continue_trial(self, guide_id, version, rehearsal_id, expected_turn, text):
        async with self.lock, AsyncExitStack() as source_lease:
            directory = self._directory(guide_id, rehearsal_id)
            if rehearsal_receipt(directory).get("trial_replay") is not None:
                raise DraftConflict("This replay follows its saved user-turn prefix automatically. End or cancel it before starting a different job.")
            if rehearsal_receipt(directory).get("mode") == "application":
                # Hold the implementation's admission lease across the current
                # source check and host command. No new build can replace it.
                if self.implementation is None:
                    raise DraftConflict("This application trial's source is unavailable.")
                await source_lease.enter_async_context(self.implementation.lock)
            state = await self._guide(guide_id)
            receipt = await self.status(guide_id, rehearsal_id, state=state)
            if (type(version) is not int or state["version"] != version
                    or receipt.get("guide_version") != version
                    or state.get("operation", {}).get("status") == "running"
                    or not receipt.get("can_continue")):
                raise DraftConflict("This open trial changed, expired or stopped. Start a current trial before continuing.")
            if type(expected_turn) is not int or expected_turn != receipt.get("turn_count"):
                raise DraftConflict("This trial already received a turn. Reload its actual activity before continuing.")
            text, _, _ = validate_trial(text, None, None, {}, Blueprint.model_validate(receipt["blueprint"]))
            # status() may await a source check. Re-read the guide after it,
            # then publish synchronously against the same settled worker state.
            fresh = await self._guide(guide_id)
            from .artifact_binding import matches
            turns = trial_turn_state(directory)
            control = trial_control_state(directory)
            if (fresh["version"] != version or fresh.get("operation", {}).get("status") == "running"
                    or not matches(receipt, fresh) or turns.get("status") != "waiting_input"
                    or turns.get("turn_count") != expected_turn or self._pending(control, turns)
                    or deadline_passed(turns.get("idle_expires_at"))
                    or deadline_passed(receipt.get("expires_at"))):
                raise DraftConflict("This trial changed before follow-up admission. Reload its actual activity.")
            command = {"kind": "continue", "id": str(uuid.uuid4()), "turn": expected_turn + 1,
                       "expected_turn": expected_turn, "text": text}
            await self._command(directory, command)
            return await self.status(guide_id, rehearsal_id)

    @staticmethod
    def _pending(control, turns):
        command = control.get("command") or {}
        acknowledged = {row.get("command_id") for row in turns.get("turns", []) if isinstance(row, dict)}
        return bool(command and (command.get("id") != turns.get("end_command_id") if command.get("kind") == "end"
                                 else command.get("id") not in acknowledged))

    async def end(self, guide_id, rehearsal_id):
        async with self.lock:
            directory = self._directory(guide_id, rehearsal_id)
            receipt = rehearsal_receipt(directory)
            if not receipt.get("keep_open"):
                raise ValueError("Only an open text trial has an End action.")
            # End can arrive while subprocess creation is still being admitted.
            # The host task retains ownership and will expose that exact child.
            task = self.tasks.get(directory.name)
            deadline = asyncio.get_running_loop().time() + 5
            while (receipt["status"] in {"queued", "starting"} and task and not task.done()
                    and self._owned_process(directory) is None and asyncio.get_running_loop().time() < deadline):
                await asyncio.sleep(.02)
                receipt = rehearsal_receipt(directory)
            if receipt["status"] not in _ACTIVE or self._owned_process(directory) is None:
                return await self.status(guide_id, rehearsal_id)
            control = trial_control_state(directory)
            if (control.get("command") or {}).get("kind") != "end":
                await self._command(directory, {"kind": "end", "id": str(uuid.uuid4())})
            return await self.status(guide_id, rehearsal_id)

    async def _stop_process(self, process):
        await stop_owned_worker(process)

    def _stop_failed(self, directory, error):
        update_receipt(directory, status="failed", trace_complete=False, worker_stop_confirmed=False,
                       detail=f"The native worker's termination could not be confirmed ({type(error).__name__}). Retry cancellation before starting another trial.")

    async def _drive_replay(self, directory, process, continuations):
        """Deliver saved user turns only after the owned native turn settles.

        This uses the ordinary host command ledger. It does not replay guessed
        tool calls or promise that the model will make the same decisions.
        """
        expected = 1
        while process.returncode is None:
            turns = trial_turn_state(directory)
            receipt = rehearsal_receipt(directory)
            if turns.get("status") in {"failed", "interrupted", "completed", "ending"}:
                return
            if receipt.get("status") not in _ACTIVE:
                return
            if turns.get("status") != "waiting_input":
                await asyncio.sleep(.05)
                continue
            if turns.get("turn_count") != expected:
                if turns.get("turn_count") == expected - 1 and self._pending(trial_control_state(directory), turns):
                    await asyncio.sleep(.05)
                    continue
                raise DraftConflict("The replay's native turn sequence changed. Its next saved request was not admitted.")
            async with self.lock, AsyncExitStack() as source_lease:
                if self.implementation is None:
                    raise DraftConflict("The replay's checked application source is unavailable.")
                await source_lease.enter_async_context(self.implementation.lock)
                frozen = json.loads((directory / "input.json").read_text())
                state = await self._guide(frozen["guide_id"])
                from .artifact_binding import matches
                from .application_snapshot import checked_trial_source
                source, _ = await checked_trial_source(self.implementation, frozen["guide_id"], state,
                    frozen["application_source"]["build_id"])
                turns = trial_turn_state(directory)
                control = trial_control_state(directory)
                receipt = rehearsal_receipt(directory)
                if (self._owned_process(directory) is not process
                        or state.get("version") != frozen["guide_version"]
                        or state.get("operation", {}).get("status") == "running"
                        or not matches(receipt, state) or source != frozen["application_source"]
                        or turns.get("status") != "waiting_input" or turns.get("turn_count") != expected
                        or not turns.get("conversation_id")
                        or deadline_passed(turns.get("idle_expires_at"))
                        or deadline_passed(frozen.get("expires_at"))):
                    raise DraftConflict("The replay changed or stopped before the next saved user turn. Its actual native history remains available.")
                if self._pending(control, turns):
                    # An admitted End is allowed, while an uncertain continuation
                    # must never be sent a second time under another command ID.
                    if (control.get("command") or {}).get("kind") == "end":
                        return
                    raise DraftConflict("The replay's prior command delivery is unresolved. Do not resend its saved turn.")
                rows = turns.get("turns")
                texts = [frozen["text"], *[row["text"] for row in continuations]]
                if (not isinstance(rows, list) or len(rows) != expected
                        or any(not isinstance(row, dict) or row.get("turn") != number
                               or row.get("status") != "settled" or row.get("text") != texts[number - 1]
                               for number, row in enumerate(rows, 1))):
                    raise DraftConflict("The replay's preceding native turn is incomplete. No later saved request was admitted.")
                metadata = frozen["trial_replay"]
                update_receipt(directory, trial_replay={**metadata, "status": "replaying", "completed_turns": expected})
                if expected > len(continuations):
                    command = {"kind": "end", "id": str(uuid.uuid5(uuid.UUID(directory.name), "replay:end"))}
                    await self._command(directory, command)
                    return
                text = continuations[expected - 1]["text"]
                text, _, _ = validate_trial(text, None, None, {}, Blueprint.model_validate(frozen["blueprint"]))
                command = {"kind": "continue", "id": str(uuid.uuid5(uuid.UUID(directory.name), f"replay:turn:{expected + 1}")),
                    "turn": expected + 1, "expected_turn": expected, "text": text}
                await self._command(directory, command)
                expected += 1
            await asyncio.sleep(.05)

    def _finish_replay(self, directory):
        receipt = rehearsal_receipt(directory)
        metadata = receipt.get("trial_replay")
        if not isinstance(metadata, dict):
            return
        frozen = json.loads((directory / "input.json").read_text())
        turns = trial_turn_state(directory)
        expected = metadata["expected_turns"]
        rows = turns.get("turns") or []
        texts = [frozen["text"], *[row["text"] for row in frozen["replay_continuations"]]]
        prefix_complete = bool(expected == 0 or (turns.get("turn_count") == expected and len(rows) == expected
            and all(isinstance(row, dict) and row.get("turn") == number and row.get("status") == "settled"
                    and row.get("text") == texts[number - 1] for number, row in enumerate(rows, 1))))
        completed = receipt.get("status") == "completed" and prefix_complete
        changes = {"trial_replay": {**metadata, "status": "completed" if completed else "incomplete",
                                   "completed_turns": turns.get("turn_count", 0)}}
        if receipt.get("status") == "completed" and not prefix_complete:
            changes.update(status="failed", detail="The worker stopped before the complete saved user-turn prefix settled. Its observed history is preserved; the selected job was not replayed completely.")
        update_receipt(directory, **changes)

    async def _run(self, directory):
        identity, process = directory.name, None
        feeder = None
        stop_confirmed = stop_attempted = False
        async def confirm_stop():
            nonlocal stop_confirmed, stop_attempted
            stop_attempted = True
            await self._stop_process(process)
            stop_confirmed = True
        try:
            frozen = json.loads((directory / "input.json").read_text())
            spec = Blueprint.model_validate(frozen["blueprint"])
            source = str(Path(__file__).resolve().parents[1])
            from .guide_runtime import capture_worker_runtime
            runtime_digest = capture_worker_runtime(directory)
            with (directory / "worker.log").open("w") as log:
                os.chmod(directory / "worker.log", 0o600)
                process, cancelled = await spawn_owned_worker(sys.executable, "-m",
                    "launchkit.rehearsal_worker", str(directory), "--owned-stdin",
                    "--agent-runtime-sha256", runtime_digest, cwd=source,
                    env=worker_environment(directory, spec), stdin=asyncio.subprocess.PIPE,
                    stdout=log, stderr=log)
                self.processes[identity] = process
                if cancelled:
                    raise asyncio.CancelledError
                if frozen.get("trial_replay") and frozen.get("keep_open"):
                    feeder = asyncio.create_task(self._drive_replay(directory, process, frozen["replay_continuations"]))
                async def wait_worker():
                    if feeder is None:
                        return await process.wait()
                    waiting = asyncio.create_task(process.wait())
                    try:
                        done, _ = await asyncio.wait({waiting, feeder}, return_when=asyncio.FIRST_COMPLETED)
                        if feeder in done:
                            await feeder
                        return await waiting
                    finally:
                        if not waiting.done():
                            waiting.cancel()
                            await asyncio.gather(waiting, return_exceptions=True)
                try:
                    await asyncio.wait_for(wait_worker(), REHEARSAL_SECONDS + WORKER_STOP_TIMEOUT + PROCESS_FORCE_REAP_TIMEOUT)
                except asyncio.TimeoutError:
                    await confirm_stop()
                    update_receipt(directory, status="failed", trace_complete=False, native_console_url=None,
                                   detail="The design rehearsal exceeded its bounded worker lifetime.")
                if not stop_confirmed:
                    await confirm_stop()
            receipt = read_receipt(directory)
            if receipt["status"] in _ACTIVE:
                update_receipt(directory, status="failed", native_console_url=None, trace_complete=False,
                               detail="The native rehearsal process stopped before a terminal result.")
            update_receipt(directory, worker_stop_confirmed=True)
        except asyncio.CancelledError:
            if process:
                try:
                    await confirm_stop()
                except Exception as error:
                    self._stop_failed(directory, error)
                    raise
            no_application = {"application_stop_confirmed": True} if process is None else {}
            update_receipt(directory, status="interrupted", native_console_url=None, trace_complete=False, worker_stop_confirmed=True,
                           **no_application,
                           detail="The rehearsal stopped. Its native trace remains available for replay.")
            raise
        except Exception as error:
            if process and not stop_confirmed:
                if stop_attempted:
                    self._stop_failed(directory, error)
                    raise
                try:
                    await confirm_stop()
                except Exception as stop_error:
                    self._stop_failed(directory, stop_error)
                    raise
            no_application = {"application_stop_confirmed": True} if process is None else {}
            update_receipt(directory, status="failed", native_console_url=None, trace_complete=False, worker_stop_confirmed=True,
                           **no_application,
                           detail=f"The native rehearsal could not finish ({type(error).__name__}).")
        finally:
            if feeder is not None:
                if not feeder.done():
                    feeder.cancel()
                await asyncio.gather(feeder, return_exceptions=True)
            self.tasks.pop(identity, None)
            if process is None or stop_confirmed:
                self.processes.pop(identity, None)
            self._finish_replay(directory)

    async def status(self, guide_id, rehearsal_id=None, *, state=None):
        directory = self._directory(guide_id, rehearsal_id)
        receipt = rehearsal_receipt(directory)
        if receipt["status"] in _ACTIVE and directory.name not in self.tasks:
            process = self.processes.get(directory.name)
            if process is not None:
                self._stop_failed(directory, RuntimeError("A worker remains without its host task"))
                receipt = rehearsal_receipt(directory)
            else:
                confirmed = receipt.get("worker_stop_confirmed") is True
                receipt = update_receipt(directory, status="interrupted", native_console_url=None,
                                         worker_stop_confirmed=confirmed, trace_complete=False,
                                         detail=("The previous host stopped; replay the saved native trace or start another trial."
                                             if confirmed else "The previous host's worker ownership was lost. Cleanup is unconfirmed; the saved native trace remains available."))
        state = state if state is not None else await self._guide(guide_id)
        proposal = state.get("proposal") or state.get("blueprint")
        from .artifact_binding import matches
        receipt["current"] = matches(receipt, state)
        receipt["current_guide_version"] = state["version"] if receipt["current"] else None
        if receipt.get("mode") == "application" and receipt["current"]:
            from .application_snapshot import trial_build_current
            source = receipt.get("application_source") or {}
            try:
                build = await self.implementation.status(guide_id, state=state)
                receipt["current"] = bool(trial_build_current(build, state, source.get("build_id"))
                    and build.get("source_digest") == source.get("source_digest"))
            except (LookupError, ValueError, OSError):
                receipt["current"] = False
        if not receipt["current"]:
            receipt["current_guide_version"] = None
        turns = trial_turn_state(directory)
        receipt["session_status"] = turns.get("status")
        # Older one-shot workers left turns.json running after normal cleanup.
        # Reconcile only the read model; retain the original stored history.
        if (receipt.get("keep_open") is False and receipt["status"] == "completed"
                and receipt.get("trace_complete") is True and receipt.get("worker_stop_confirmed") is True
                and (receipt.get("mode") != "application" or receipt.get("application_stop_confirmed") is True)
                and turns.get("status") == "running"):
            receipt["session_status"] = "completed"
        for key in ("turns", "turn_count", "current_turn", "conversation_id", "idle_expires_at", "ended_reason"):
            if key in turns:
                receipt[key] = copy.deepcopy(turns[key])
        control = trial_control_state(directory)
        pending = self._pending(control, turns)
        if control.get("command"):
            command = control["command"]
            receipt["turn_command"] = {"id": command["id"], "turn": command.get("turn"),
                                       "status": control.get("status") if pending else "acknowledged"}
        if receipt.get("keep_open") and receipt["status"] in _ACTIVE and pending:
            receipt["status"] = "ending" if control["command"]["kind"] == "end" else "running"
            receipt["detail"] = ("Ending this trial after its admitted native work settles."
                if receipt["status"] == "ending" else "Delivering the follow-up to this trial's existing native conversation.")
        receipt["can_continue"] = bool(receipt.get("keep_open") and receipt["current"]
            and receipt.get("trial_replay") is None
            and state["version"] == receipt.get("guide_version")
            and state.get("operation", {}).get("status") != "running"
            and receipt["status"] == "waiting_input" and not pending
            and turns.get("status") == "waiting_input"
            and self._owned_process(directory) is not None
            and turns.get("conversation_id")
            and 0 < turns.get("turn_count", 0) < MAX_REHEARSAL_TURNS
            and not deadline_passed(turns.get("idle_expires_at"))
            and not deadline_passed(receipt.get("expires_at")))
        receipt["live_ready"] = receipt["connected"] = False
        preview = None
        if receipt.get("mode") == "application":
            try:
                frozen = json.loads((directory / "input.json").read_text())
                preview = application_preview_projection(receipt, frozen, state)
            except (OSError, ValueError, TypeError, KeyError, RecursionError):
                pass  # Optional preview never creates source or readiness authority.
        receipt["application_preview"] = preview
        return receipt

    async def timeline(self, guide_id, after=None, limit=200, rehearsal_id=None):
        if type(limit) is not int or not 1 <= limit <= 1000:
            raise ValueError("Use a replay page size from 1-1000.")
        baseline = trace_cursor(after)
        receipt = await self.status(guide_id, rehearsal_id)
        directory = self._directory(guide_id, receipt["rehearsal_id"])
        rows, latest = [], 0
        path = directory / "timeline.jsonl"
        if path.exists():
            with path.open() as stream:
                for line in stream:
                    try:
                        row = json.loads(line)
                    except ValueError:
                        if not line.endswith("\n"):
                            break  # A worker may still be appending the last row.
                        raise ValueError("The saved native trace contains an unreadable row.") from None
                    seq = row["rehearsal_seq"]
                    latest = max(latest, seq)
                    if seq > baseline and len(rows) < limit:
                        rows.append(row)
        if baseline > latest:
            raise ValueError("The replay cursor is beyond this trial's saved native trace.")
        frontier = rows[-1]["rehearsal_seq"] if rows else baseline
        return {"frames": rows, "next_cursor": f"rehearsal:{frontier}", "latest_cursor": f"rehearsal:{latest}",
                "rehearsal_id": receipt["rehearsal_id"], "status": receipt["status"],
                "current": receipt["current"], "scope": receipt["scope"], "trace_complete": receipt["trace_complete"]}

    async def cancel(self, guide_id, rehearsal_id=None):
        async with self.lock:
            directory = self._directory(guide_id, rehearsal_id)
            task = self.tasks.get(directory.name)
            owned_task = bool(task and not task.done())
            if task and not task.done():
                task.cancel()
                await asyncio.gather(task, return_exceptions=True)
            process = self.processes.get(directory.name)
            owned_process = process is not None
            if process is not None:
                try:
                    await self._stop_process(process)
                except Exception as error:
                    self._stop_failed(directory, error)
                    from .architect import ArchitectUnavailable
                    raise ArchitectUnavailable("The native worker's termination could not be confirmed. Retry cancellation before starting another trial.") from error
                self.processes.pop(directory.name, None)
            receipt = rehearsal_receipt(directory)
            if receipt.get("worker_stop_confirmed") is not True and not (owned_task or owned_process):
                from .architect import ArchitectUnavailable
                raise ArchitectUnavailable("This open trial's worker ownership was lost. Cancellation cannot confirm cleanup without its owned process; the saved trace is retained.")
            if receipt["status"] in _ACTIVE or receipt.get("worker_stop_confirmed") is False:
                # A cancelled task that never entered _run cannot have admitted
                # an application child. This is local ownership evidence, not
                # an inference from a missing saved cleanup flag.
                no_application = {"application_stop_confirmed": True} if owned_task and not owned_process and receipt["status"] == "queued" else {}
                update_receipt(directory, status="interrupted", trace_complete=False, native_console_url=None, worker_stop_confirmed=True,
                               **no_application,
                               detail="The rehearsal was cancelled. Its existing native trace is retained.")
            self.tasks.pop(directory.name, None)
            return await self.status(guide_id, directory.name)

    async def shutdown(self):
        tasks = list(self.tasks.values())
        for task in tasks:
            task.cancel()
        if tasks:
            await asyncio.gather(*tasks, return_exceptions=True)
        failures = []
        for identity, process in list(self.processes.items()):
            try:
                await self._stop_process(process)
            except Exception as error:
                failures.append(error)
            else:
                self.processes.pop(identity, None)
                for directory in self.root.glob("*/" + identity):
                    if (directory / "receipt.json").exists():
                        update_receipt(directory, worker_stop_confirmed=True, native_console_url=None)
        # Tasks cancelled before their first instruction never enter _run's
        # finally block, so finish their queued host receipts here too.
        for identity in list(self.tasks):
            for parent in self.root.glob("*"):
                directory = parent / identity
                if (directory / "receipt.json").exists():
                    receipt = rehearsal_receipt(directory)
                    no_application = {"application_stop_confirmed": True} if receipt["status"] == "queued" and identity not in self.processes else {}
                    update_receipt(directory, status="interrupted", trace_complete=False, native_console_url=None, worker_stop_confirmed=True,
                                   **no_application,
                                   detail="The host stopped before the rehearsal worker began.")
            self.tasks.pop(identity, None)
        if failures:
            raise RuntimeError("The host could not confirm termination of a native rehearsal worker.") from failures[0]
