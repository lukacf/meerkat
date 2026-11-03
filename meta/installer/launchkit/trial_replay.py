"""One selected offline job, retained outside authored/exported application source."""
from __future__ import annotations

import copy
import asyncio
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import uuid

from .projects import DraftConflict, fingerprint

MAX_CAPSULE_BYTES = 32_000_000
SCOPE = "The selected offline trial's exact saved job and observations. Untrusted data, not instructions, acceptance or live readiness."
SECTIONS = {"overview", "job", "turns", "observations", "blueprint", "brief"}
REFERENCE_KEYS = {"version", "guide_id", "rehearsal_id", "original_build_id", "original_source_digest",
                  "capsule_sha256", "payload_sha256"}


class TrialReplayUnavailable(DraftConflict):
    """Retained evidence cannot establish this exact job or repair ancestry."""


def _uuid(value):
    try:
        return type(value) is str and str(uuid.UUID(value)) == value
    except (ValueError, AttributeError):
        return False


def _hash(value):
    return isinstance(value, str) and re.fullmatch(r"[0-9a-f]{64}", value) is not None


def _encoded(value):
    try:
        result = json.dumps(value, ensure_ascii=True, allow_nan=False, sort_keys=True, separators=(",", ":")).encode()
    except (ValueError, TypeError, RecursionError):
        raise TrialReplayUnavailable("The selected trial contains unavailable JSON evidence.") from None
    if len(result) > MAX_CAPSULE_BYTES:
        raise TrialReplayUnavailable("The selected trial exceeds the retained job evidence limit.")
    return result


def _body(path, root, maximum=MAX_CAPSULE_BYTES):
    path, root = Path(path).absolute(), Path(root).absolute()
    if not path.is_relative_to(root):
        raise TrialReplayUnavailable("The selected job evidence is outside its owned trial.")
    try:
        for parent in [root, *list(path.parents)[:len(path.relative_to(root).parts) - 1]]:
            if not stat.S_ISDIR(parent.lstat().st_mode):
                raise OSError
        info = path.lstat()
        if not stat.S_ISREG(info.st_mode) or info.st_nlink != 1 or info.st_size > maximum:
            raise OSError
        descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
        with os.fdopen(descriptor, "rb") as stream:
            opened = os.fstat(stream.fileno())
            if (not stat.S_ISREG(opened.st_mode) or opened.st_nlink != 1
                    or (opened.st_dev, opened.st_ino) != (info.st_dev, info.st_ino)
                    or opened.st_size > maximum):
                raise OSError
            body = stream.read(maximum + 1)
        if len(body) > maximum:
            raise OSError
        return body
    except OSError:
        raise TrialReplayUnavailable("The selected job's saved evidence is missing or unavailable.") from None


def _json(path, root, maximum=MAX_CAPSULE_BYTES):
    body = _body(path, root, maximum)
    try:
        value = json.loads(body)
        _encoded(value)
    except (ValueError, UnicodeError, RecursionError):
        raise TrialReplayUnavailable("The selected job's saved evidence is unreadable.") from None
    return value, hashlib.sha256(body).hexdigest()


def _payload(capsule):
    if (not isinstance(capsule, dict) or set(capsule) != {"version", "payload", "payload_sha256", "captured_evidence"}
            or capsule.get("version") != 1 or type(capsule.get("version")) is not int
            or not isinstance(capsule.get("payload"), dict)
            or fingerprint(capsule["payload"]) != capsule.get("payload_sha256")):
        raise TrialReplayUnavailable("The saved selected job failed its content binding.")
    payload = capsule["payload"]
    expected = {"guide_id", "guide_version", "rehearsal_id", "original_build_id", "original_source_digest",
                "blueprint", "brief", "initial", "turns", "observations", "continuations"}
    evidence = capsule["captured_evidence"]
    if (set(payload) != expected
            or not all(_uuid(payload.get(k)) for k in ("guide_id", "rehearsal_id", "original_build_id"))
            or not _hash(payload.get("original_source_digest"))
            or type(payload.get("guide_version")) is not int or payload["guide_version"] < 1
            or any(not isinstance(payload[k], dict) for k in ("blueprint", "brief", "initial"))
            or any(not isinstance(payload[k], list) for k in ("turns", "observations", "continuations"))
            or not isinstance(evidence, dict)
            or set(evidence) != {"input_sha256", "turns_sha256", "observations_sha256", "worker_stop_confirmed", "application_stop_confirmed"}
            or not _hash(evidence.get("input_sha256"))
            or any(evidence.get(k) is not None and not _hash(evidence[k]) for k in ("turns_sha256", "observations_sha256"))
            or evidence.get("worker_stop_confirmed") is not True or evidence.get("application_stop_confirmed") is not True):
        raise TrialReplayUnavailable("The saved selected job's required evidence is incomplete.")
    initial, turns = payload["initial"], payload["turns"]
    if (set(initial) != {"text", "flow_id", "params", "fixtures", "application_fixtures", "keep_open"}
            or type(initial["keep_open"]) is not bool or len(turns) > 10
            or any(not isinstance(row, dict)
                or set(row) != {"turn", "text", "status", "native_admission", "terminal_interaction"}
                or type(row["turn"]) is not int or row["turn"] != number
                or not isinstance(row["text"], str) or not 1 <= len(row["text"]) <= 6000
                or not isinstance(row["native_admission"], dict) or not isinstance(row["terminal_interaction"], dict)
                or not isinstance(row["status"], str)
                or row["status"] not in {"settled", "failed", "interrupted", "admitted", "admitting"}
                or number < len(turns) and row["status"] != "settled"
                for number, row in enumerate(turns, 1))):
        raise TrialReplayUnavailable("The saved selected job's input or user-turn prefix is incomplete.")
    try:
        from .blueprint import Blueprint
        from .rehearsal import validate_trial
        from .application_trial import validate_application_fixtures
        text, params, fixtures = validate_trial(initial["text"], initial["flow_id"], initial["params"],
            initial["fixtures"], Blueprint.model_validate(payload["blueprint"]))
        application_fixtures = validate_application_fixtures(initial["application_fixtures"])
        if (text != initial["text"] or params != initial["params"] or fixtures != initial["fixtures"]
                or application_fixtures != initial["application_fixtures"]
                or turns and turns[0]["text"] != text
                or initial["flow_id"] is not None and (turns or initial["keep_open"])
                or len(turns) > 1 and not initial["keep_open"]
                or payload["continuations"] != [{"text": row["text"], "original_turn": row["turn"]} for row in turns[1:]]):
            raise ValueError
    except (ValueError, TypeError, KeyError, RecursionError):
        raise TrialReplayUnavailable("The saved selected job's exact input or follow-up requests are unavailable.") from None
    return payload


def _reference(capsule, body):
    payload = _payload(capsule)
    return {"version": 1, **{key: payload[key] for key in
        ("guide_id", "rehearsal_id", "original_build_id", "original_source_digest")},
        "capsule_sha256": hashlib.sha256(body).hexdigest(), "payload_sha256": capsule["payload_sha256"]}


def capture_trial_replay(rehearsal, guide_id, state, receipt):
    """Read the exact selected stopped trial, never its shortened repair prompt."""
    from .blueprint import Blueprint
    from .rehearsal import MAX_REHEARSAL_TURNS, validate_trial
    from .application_trial import MAX_OBSERVATION_BYTES, source_files, validate_application_fixtures
    from .implementation_workspace import ImplementationWorkspace

    trial_id = receipt.get("rehearsal_id")
    source = receipt.get("application_source") or {}
    if (not all(_uuid(value) for value in (guide_id, trial_id, source.get("build_id")))
            or not _hash(source.get("source_digest")) or receipt.get("guide_id") != guide_id
            or receipt.get("mode") != "application" or receipt.get("current") is not True
            or receipt.get("status") not in {"completed", "failed", "interrupted", "unavailable"}
            or receipt.get("worker_stop_confirmed") is not True
            or receipt.get("application_stop_confirmed") is not True
            or trial_id in rehearsal.tasks or trial_id in rehearsal.processes):
        raise TrialReplayUnavailable("End the selected application trial and confirm its cleanup before saving a repair job.")
    directory = rehearsal._directory(guide_id, trial_id)
    frozen, input_hash = _json(directory / "input.json", directory)
    if (not isinstance(frozen, dict) or frozen.get("guide_id") != guide_id
            or frozen.get("mode") != "application" or frozen.get("guide_version") != receipt.get("guide_version")
            or type(frozen.get("guide_version")) is not int or frozen["guide_version"] <= 0
            or (frozen.get("application_source") or {}).get("build_id") != source["build_id"]
            or (frozen.get("application_source") or {}).get("source_digest") != source["source_digest"]
            or fingerprint(frozen.get("blueprint")) != fingerprint(state.get("proposal") or state.get("blueprint"))
            or fingerprint(frozen.get("brief", {})) != fingerprint(state.get("brief", {}))):
        raise TrialReplayUnavailable("The selected job no longer matches this application design and source.")
    try:
        actual_source = source_files(directory / "application-source")
        if ImplementationWorkspace._digest(actual_source) != source["source_digest"]:
            raise ValueError
    except (ValueError, OSError):
        raise TrialReplayUnavailable("The selected trial's frozen application source changed or is unavailable.") from None
    try:
        spec = Blueprint.model_validate(frozen["blueprint"])
        text, params, fixtures = validate_trial(frozen["text"], frozen.get("flow_id"), frozen.get("params"), frozen.get("fixtures"), spec)
        application_fixtures = validate_application_fixtures(frozen.get("application_fixtures"))
    except (ValueError, TypeError, KeyError):
        raise TrialReplayUnavailable("The selected trial's saved inputs are unavailable for exact replay.") from None
    if (text != frozen["text"] or params != frozen.get("params") or fixtures != frozen.get("fixtures")
            or application_fixtures != frozen.get("application_fixtures")
            or type(frozen.get("keep_open", False)) is not bool):
        raise TrialReplayUnavailable("The selected trial's saved input normalization changed.")
    initial = {"text": text, "flow_id": frozen.get("flow_id"), "params": params, "fixtures": fixtures,
               "application_fixtures": application_fixtures, "keep_open": frozen.get("keep_open", False)}
    turns, turns_hash = [], None
    path = directory / "turns.json"
    if path.exists() or path.is_symlink():
        ledger, turns_hash = _json(path, directory, 1_000_000)
        if not isinstance(ledger, dict) or not isinstance(ledger.get("turns"), list):
            raise TrialReplayUnavailable("The selected trial's user-turn history is unavailable.")
        raw_turns = ledger["turns"]
        if len(raw_turns) > MAX_REHEARSAL_TURNS or raw_turns != receipt.get("turns", []):
            raise TrialReplayUnavailable("The selected trial's complete turn history changed.")
        conversation = None
        for index, row in enumerate(raw_turns, 1):
            if (not isinstance(row, dict) or type(row.get("turn")) is not int or row["turn"] != index
                    or not isinstance(row.get("text"), str) or not 1 <= len(row["text"]) <= 6000
                    or index == 1 and row["text"] != text
                    or not isinstance(row.get("status"), str)
                    or row.get("status") not in {"settled", "failed", "interrupted", "admitted", "admitting"}
                    or index < len(raw_turns) and row.get("status") != "settled"):
                raise TrialReplayUnavailable("The selected trial's admitted user-turn prefix is incomplete.")
            admission = row.get("native_admission") or {}
            if not isinstance(admission, dict):
                raise TrialReplayUnavailable("The selected trial's native admission evidence is invalid.")
            if row.get("status") == "settled" and any(not isinstance(admission.get(k), str) or not admission[k]
                    or len(admission[k]) > 500 for k in ("interaction_id", "conversation_id")):
                raise TrialReplayUnavailable("The selected trial's settled turn has no native admission binding.")
            if admission:
                if any(not isinstance(admission.get(k), str) or not admission[k] or len(admission[k]) > 500
                       for k in ("interaction_id", "conversation_id")):
                    raise TrialReplayUnavailable("The selected trial's native admission binding is unavailable.")
                if conversation is not None and admission["conversation_id"] != conversation:
                    raise TrialReplayUnavailable("The selected trial's saved turns changed native conversation.")
                conversation = admission["conversation_id"]
            terminal = row.get("terminal_interaction") or {}
            if not isinstance(terminal, dict):
                raise TrialReplayUnavailable("The selected trial's terminal evidence is unavailable.")
            turns.append({"turn": index, "text": row["text"], "status": row["status"],
                "native_admission": {k: copy.deepcopy(admission[k]) for k in ("interaction_id", "conversation_id") if k in admission},
                "terminal_interaction": {k: copy.deepcopy(terminal[k]) for k in
                    ("id", "cursor", "kind", "status", "interaction_id") if k in terminal}})
    elif receipt.get("turns") or receipt.get("turn_count", 0) or initial["keep_open"] and receipt.get("native_admission"):
        raise TrialReplayUnavailable("The selected trial's saved user-turn history is missing.")
    if frozen.get("flow_id") is not None and turns:
        raise TrialReplayUnavailable("The selected flow has an unexpected text-turn history.")
    observations, observation_hash = [], None
    path = directory / "application-observations.jsonl"
    if path.exists() or path.is_symlink():
        body = _body(path, directory, MAX_OBSERVATION_BYTES)
        if body and not body.endswith(b"\n"):
            raise TrialReplayUnavailable("The selected trial's callback observation history is incomplete.")
        try:
            observations = [json.loads(line) for line in body.splitlines()]
        except (ValueError, UnicodeError, RecursionError):
            raise TrialReplayUnavailable("The selected trial's callback observation history is unreadable.") from None
        if observations != receipt.get("application_observations", []):
            raise TrialReplayUnavailable("The selected trial's complete callback observations changed.")
        if any(not isinstance(row, dict) or row.get("source_digest") != source["source_digest"]
                or row.get("scope") != "offline-application" for row in observations):
            raise TrialReplayUnavailable("The selected callback ledger belongs to another source.")
        revision, pending, stopped = 0, None, False
        for row in observations:
            kind = row.get("kind")
            if not isinstance(kind, str) or kind not in {"application_started", "callback_requested", "callback_returned", "callback_failed", "application_stopped"}:
                raise TrialReplayUnavailable("The selected callback observation kind is unavailable.")
            revision += kind != "application_stopped"
            if type(row.get("observation_revision")) is not int or row["observation_revision"] != revision or stopped:
                raise TrialReplayUnavailable("The selected callback observation sequence is incomplete.")
            if kind == "callback_requested":
                if pending is not None or not isinstance(row.get("identity"), str) or not isinstance(row.get("name"), str):
                    raise TrialReplayUnavailable("The selected callback request sequence is incomplete.")
                pending = (row["identity"], row["name"])
            elif kind in {"callback_returned", "callback_failed"}:
                if pending != (row.get("identity"), row.get("name")):
                    raise TrialReplayUnavailable("The selected callback result has no matching request.")
                pending = None
            elif kind == "application_stopped":
                stopped = True
                if row.get("process_stop_confirmed") is not True:
                    raise TrialReplayUnavailable("The selected callback sandbox stop is unconfirmed.")
        if pending is not None or observations and not stopped:
            raise TrialReplayUnavailable("The selected callback observation history ended before confirmed cleanup.")
        observation_hash = hashlib.sha256(body).hexdigest()
    elif (receipt.get("application_observations") or (receipt.get("application_trial") or {}).get("available") is True):
        raise TrialReplayUnavailable("The selected trial's saved callback observation history is missing.")
    payload = {"guide_id": guide_id, "guide_version": frozen["guide_version"], "rehearsal_id": trial_id,
        "original_build_id": source["build_id"], "original_source_digest": source["source_digest"],
        "blueprint": frozen["blueprint"], "brief": copy.deepcopy(frozen.get("brief", {})),
        "initial": initial, "turns": turns, "observations": observations,
        "continuations": [{"text": row["text"], "original_turn": row["turn"]} for row in turns[1:]]}
    capsule = {"version": 1, "payload": payload, "payload_sha256": fingerprint(payload),
        "captured_evidence": {"input_sha256": input_hash, "turns_sha256": turns_hash,
            "observations_sha256": observation_hash, "worker_stop_confirmed": True, "application_stop_confirmed": True}}
    _encoded(capsule)
    return copy.deepcopy(capsule)


def save_trial_replay(build_directory, capsule):
    body = _encoded(capsule)
    reference = _reference(capsule, body)
    directory = Path(build_directory) / "trial-replay"
    directory.mkdir(mode=0o700)
    fd = os.open(directory / "capsule.json", os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd, "wb") as stream:
        stream.write(body); stream.flush(); os.fsync(stream.fileno())
    return reference


def _load(build_directory, reference):
    if (not isinstance(reference, dict) or set(reference) != REFERENCE_KEYS or reference.get("version") != 1
            or type(reference.get("version")) is not int
            or not all(_uuid(reference.get(k)) for k in ("guide_id", "rehearsal_id", "original_build_id"))
            or not all(_hash(reference.get(k)) for k in ("original_source_digest", "capsule_sha256", "payload_sha256"))):
        raise TrialReplayUnavailable("No exact saved job is attached to this repair build.")
    body = _body(Path(build_directory) / "trial-replay/capsule.json", build_directory)
    try:
        capsule = json.loads(body)
        _encoded(capsule)
    except (ValueError, UnicodeError, RecursionError):
        raise TrialReplayUnavailable("The selected job's saved evidence is unreadable.") from None
    if _reference(capsule, body) != reference:
        raise TrialReplayUnavailable("The saved job changed after its repair binding.")
    return capsule


def retained_trial_replay(build_directory, frozen):
    reference = frozen.get("trial_replay")
    if not reference:
        return None
    capsule = _load(build_directory, reference)
    _ancestry(frozen, _payload(capsule), Path(build_directory).name)
    return copy.deepcopy(capsule)


def _ancestry(frozen, payload, target_build_id):
    chain = frozen.get("trial_replay_ancestry")
    parent = frozen.get("previous_implementation") or {}
    if (not isinstance(chain, list) or not 1 <= len(chain) <= 1000
            or any(not isinstance(row, dict) or set(row) != {"build_id", "source_digest"}
                or not _uuid(row["build_id"]) or not _hash(row["source_digest"]) for row in chain)
            or len({row["build_id"] for row in chain}) != len(chain)
            or target_build_id in {row["build_id"] for row in chain}
            or chain[0] != {"build_id": payload["original_build_id"], "source_digest": payload["original_source_digest"]}
            or not isinstance(parent, dict) or chain[-1] != {"build_id": parent.get("build_id"),
                "source_digest": parent.get("copied_source_digest")}):
        raise TrialReplayUnavailable("The selected job's source continuation ancestry is unavailable.")
    return copy.deepcopy(chain)


def trial_replay_ancestry(capsule, previous_directory, previous_input, copied_source_digest, *, retained=False):
    payload = _payload(capsule)
    parent = {"build_id": Path(previous_directory).name, "source_digest": copied_source_digest}
    if not _hash(copied_source_digest):
        raise TrialReplayUnavailable("The copied repair source has no exact content binding.")
    if retained:
        chain = _ancestry(previous_input, payload, parent["build_id"])
        if len(chain) >= 1000:
            raise TrialReplayUnavailable("The saved job's source continuation history exceeds its retention bound.")
        return [*chain, parent]
    if parent != {"build_id": payload["original_build_id"], "source_digest": payload["original_source_digest"]}:
        raise TrialReplayUnavailable("The selected job does not belong to the copied repair source.")
    return [parent]


def trial_replay_projection(build_directory, frozen, status, state):
    reference = frozen.get("trial_replay")
    if not reference:
        return None
    projection = {"available": False, "guide_id": state["id"], "guide_version": state["version"],
        "build_id": status.get("build_id"), "source_digest": status.get("source_digest"),
        "detail": "Finish current source checks and build cleanup before retrying the saved job."}
    try:
        payload = _payload(_load(build_directory, reference))
        _ancestry(frozen, payload, status.get("build_id"))
        if (payload["guide_id"] != state["id"] or payload["original_build_id"] == status.get("build_id")
                or fingerprint(payload["blueprint"]) != fingerprint(frozen.get("blueprint"))
                or fingerprint(payload["brief"]) != fingerprint(frozen.get("brief", {}))):
            raise TrialReplayUnavailable("The selected job no longer belongs to this repair's application scope.")
        projection.update({key: reference[key] for key in
            ("rehearsal_id", "original_build_id", "original_source_digest", "payload_sha256")})
        projection["available"] = bool(status.get("current") is True and status.get("source_verified") is True
            and _hash(status.get("source_digest")) and status.get("status") in {"completed", "failed", "interrupted"}
            and status.get("cleanup_pending") is False and status.get("cleanup_in_progress") is False
            and status.get("cleanup_custody_lost") is not True and status.get("worker_stop_confirmed") is True)
        if projection["available"]:
            projection["detail"] = "Retry the selected job with its original inputs and saved follow-up requests against this checked repair."
    except (TrialReplayUnavailable, KeyError, TypeError, ValueError):
        projection["detail"] = "This repair's saved job evidence is unavailable. Its application source is preserved."
    return projection


async def load_trial_replay(implementation, guide_id, rehearsal_id, original_build_id, original_source_digest,
                           build_id, source_digest, *, state=None):
    if not all(_uuid(v) for v in (guide_id, rehearsal_id, original_build_id, build_id)) or not all(
            _hash(v) for v in (original_source_digest, source_digest)):
        raise TrialReplayUnavailable("Use the saved job and current repaired source references.")
    state = state if state is not None else await implementation._guide(guide_id)
    status = await implementation.status(guide_id, state=state)
    directory = implementation._directory(guide_id, build_id)
    expected = {"guide_id": guide_id, "rehearsal_id": rehearsal_id, "original_build_id": original_build_id,
                "original_source_digest": original_source_digest, "build_id": build_id, "source_digest": source_digest}
    def inspect():
        frozen, _ = _json(directory / "input.json", directory)
        projection = trial_replay_projection(directory, frozen, status, state)
        if (not projection or projection.get("available") is not True
                or any(projection.get(k) != v for k, v in expected.items())):
            raise TrialReplayUnavailable("The saved job or checked repair changed. Refresh before retrying this job.")
        capsule = _load(directory, frozen["trial_replay"])
        payload = _payload(capsule)
        return {"reference": copy.deepcopy(frozen["trial_replay"]), "payload_sha256": capsule["payload_sha256"],
                **copy.deepcopy({key: payload[key] for key in
                    ("initial", "continuations", "blueprint", "brief", "observations")}),
                "original_turns": copy.deepcopy(payload["turns"])}
    result = await asyncio.to_thread(inspect)
    if implementation._directory(guide_id).name != build_id:
        raise TrialReplayUnavailable("The saved job or checked repair changed. Refresh before retrying this job.")
    return result


def read_trial_replay(build_directory, reference, *, section="overview", offset=0, limit=8000):
    if type(section) is not str or section not in SECTIONS or type(offset) is not int or offset < 0 or type(limit) is not int or not 1 <= limit <= 16000:
        raise ValueError("Choose a saved-job section and a character page of 1-16000.")
    capsule = _load(build_directory, reference); payload = _payload(capsule)
    value = ({"reference": reference, "sections": sorted(SECTIONS - {"overview"}),
        "saved_turns": len(payload["turns"]), "callback_observations": len(payload["observations"]),
        "scope": SCOPE} if section == "overview" else payload["initial" if section == "job" else section])
    content = json.dumps(value, ensure_ascii=True, allow_nan=False, sort_keys=True, indent=2)
    if offset > len(content):
        raise ValueError("The page offset is beyond this saved-job section.")
    end = min(len(content), offset + limit)
    return {"section": section, "offset": offset, "total_characters": len(content), "content": content[offset:end],
            "next_offset": end if end < len(content) else None, "payload_sha256": capsule["payload_sha256"], "scope": SCOPE}
