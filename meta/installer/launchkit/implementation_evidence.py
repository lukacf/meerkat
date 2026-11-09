"""Read-only guide projections of host-verified implementation evidence.

The provider is a host boundary: it verifies receipt signatures and the current
source before setting trust flags. This projection checks the guide, proposal,
brief and outcome bindings again. Model prose and authored manifests never
establish execution, connection or deployment evidence.
"""
from __future__ import annotations

import copy
import re

from .projects import fingerprint
from .artifact_binding import current_version, proposal_hash

ACCEPTANCE_ISSUER = "mobkit-launch-acceptance-host"
_DIGEST = re.compile(r"^[0-9a-f]{64}$")
_IN_PROGRESS = {"queued", "running", "checking", "reviewing", "repairing"}
_INACTIVE = {"deferred", "cancelled"}
_BUILD_ACTIVE = {"queued", "running", "checking"}
_BUILD_TERMINAL = {"completed", "failed", "interrupted"}
_BUILD_PHASES = {"implementation", "independent_review", "automatic_repair", "complete"}
_BUILD_FLAGS = ("stop_requested", "cleanup_pending", "cleanup_in_progress", "cleanup_custody_lost")
_SCOPE_ID = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._:-]{0,179}$")


def _text(value, limit=1200):
    return value[:limit] if isinstance(value, str) else ""


def _strings(value, limit=12, text_limit=600):
    if not isinstance(value, list):
        return []
    return list(dict.fromkeys(_text(item, text_limit) for item in value
                              if isinstance(item, str) and item.strip()))[:limit]


def _digest(value):
    return isinstance(value, str) and _DIGEST.fullmatch(value) is not None


def _trial_replay_evidence(state, receipt, *, current, source_verified):
    result = {"available": False, "action": "Try the same job again"}
    replay = receipt.get("trial_replay") if isinstance(receipt, dict) else None
    if (not isinstance(replay, dict) or replay.get("available") is not True or not current or not source_verified
            or replay.get("guide_id") != state.get("id") or type(replay.get("guide_version")) is not int
            or replay["guide_version"] != state.get("version") or replay.get("build_id") != receipt.get("build_id")
            or replay.get("source_digest") != receipt.get("source_digest")
            or receipt.get("status") not in _BUILD_TERMINAL or receipt.get("cleanup_pending") is not False
            or receipt.get("cleanup_in_progress") is not False or receipt.get("cleanup_custody_lost") is not False
            or receipt.get("worker_stop_confirmed") is not True
            or any(not isinstance(replay.get(k), str) or not _SCOPE_ID.fullmatch(replay[k])
                for k in ("guide_id", "build_id", "rehearsal_id", "original_build_id"))
            or any(not _digest(replay.get(k)) for k in ("source_digest", "original_source_digest", "payload_sha256"))):
        return result
    result.update({key: replay[key] for key in ("guide_id", "guide_version", "build_id", "source_digest",
        "rehearsal_id", "original_build_id", "original_source_digest", "payload_sha256")})
    result.update(available=True, scope="The saved offline job's original inputs and user turns against checked repaired source. Model decisions may differ; this does not establish acceptance or a live outcome.")
    return result


def _implementation_lifecycle(state, receipt, *, content_current, source_verified, unavailable):
    """Project selected host lifecycle independently of application freshness.

    The provider is ImplementationService.status, including its current custody
    observation. These local gates describe that snapshot, not an admission or
    provider capability. Callers must also check guide/provider/host eligibility;
    the service rechecks selection, current version and ownership on each action.
    No worker prose, diagnostics, paths or process metadata enter this projection.
    """
    result = {"scope": "host-build", "availability": "invalid", "status": None,
              "phase": None, "build_id": None, "guide_version": None,
              "control_guide_version": None, "content_current": False,
              **{name: None for name in _BUILD_FLAGS}, "worker_stop_confirmed": None,
              "build_gate_clear": False, "stop_available": False,
              "retry_cleanup_available": False, "resume_available": False,
              "rebuild_available": False, "application_trial_gate_clear": False,
              "reason_code": "invalid_evidence",
              "detail": "Current build lifecycle evidence is incomplete. Refresh its status before choosing an action."}
    if unavailable:
        result.update(availability="unavailable", reason_code="evidence_unavailable",
                      detail="Current build lifecycle evidence is unavailable. Refresh its status before choosing an action.")
        return result
    if receipt is None:
        result.update(availability="missing", reason_code="no_build",
                      build_gate_clear=True,
                      detail="No saved application build was found for this setup. A first build can be prepared when its proposal and host capabilities are ready.")
        return result
    if not isinstance(receipt, dict) or not receipt:
        return result
    status, phase = receipt.get("status"), receipt.get("phase")
    build_id, guide_id = receipt.get("build_id"), receipt.get("guide_id")
    version, control_version = receipt.get("guide_version"), state.get("version")
    if (not isinstance(status, str) or status not in _BUILD_ACTIVE | _BUILD_TERMINAL
            or phase is not None and (not isinstance(phase, str) or phase not in _BUILD_PHASES)
            or not isinstance(build_id, str) or not _SCOPE_ID.fullmatch(build_id)
            or not isinstance(guide_id, str) or not _SCOPE_ID.fullmatch(guide_id)
            or guide_id != state.get("id")
            or type(version) is not int or version < 1
            or type(control_version) is not int or control_version < 1
            or any(type(receipt.get(name)) is not bool for name in _BUILD_FLAGS)
            or "worker_stop_confirmed" in receipt and type(receipt["worker_stop_confirmed"]) is not bool):
        return result
    flags = {name: receipt[name] for name in _BUILD_FLAGS}
    # These combinations cannot be produced by the host custody projection.
    if (flags["cleanup_custody_lost"] and not flags["cleanup_pending"]
            or flags["cleanup_in_progress"] and not flags["cleanup_pending"]):
        return result
    terminal = status in _BUILD_TERMINAL
    gate_clear = terminal and not flags["cleanup_pending"] and not flags["cleanup_in_progress"]
    result.update(availability="available", status=status, phase=phase, build_id=build_id,
                  guide_version=version, control_guide_version=control_version,
                  content_current=content_current, **flags,
                  worker_stop_confirmed=receipt.get("worker_stop_confirmed"),
                  build_gate_clear=gate_clear,
                  stop_available=(not terminal and not flags["stop_requested"]
                                  and not flags["cleanup_in_progress"] and not flags["cleanup_custody_lost"]),
                  retry_cleanup_available=(terminal and flags["cleanup_pending"]
                                           and not flags["cleanup_in_progress"] and not flags["cleanup_custody_lost"]),
                  resume_available=(gate_clear and content_current and status in {"failed", "interrupted"}),
                  rebuild_available=gate_clear,
                  application_trial_gate_clear=(gate_clear and content_current and source_verified))
    if flags["cleanup_custody_lost"]:
        reason, detail = "cleanup_custody_lost", "This host no longer owns the worker handle needed to confirm cleanup. Saved source remains available; cleanup recovery is required before another build or trial."
    elif flags["cleanup_in_progress"]:
        reason, detail = "cleanup_in_progress", "The build runtime is closing. Wait for confirmed cleanup before another build or application trial."
    elif terminal and flags["cleanup_pending"]:
        reason, detail = "cleanup_retry_needed", "The build finished but its worker cleanup is unconfirmed. Retry cleanup in the builder before another build or application trial."
    elif flags["stop_requested"] and not terminal:
        reason, detail = "stop_in_progress", "The builder is stopping. Follow its saved activity and wait for confirmed cleanup."
    elif not terminal:
        if status == "checking":
            reason, detail = "source_check", "The host is running isolated source and application checks."
        elif phase == "independent_review":
            reason, detail = "independent_review", "The independent reviewer is checking the application against the saved brief."
        elif phase == "automatic_repair":
            reason, detail = "automatic_repair", "The builder is repairing findings from the independent review."
        elif status == "queued":
            reason, detail = "queued", "The saved build request is waiting to start."
        else:
            reason, detail = "building", "The builder is implementing and testing the saved application design."
    elif status == "failed":
        reason, detail = "build_failed", "This build did not finish. Saved source and findings remain available to the builder; current checks must be established on continuation."
    elif status == "interrupted":
        reason, detail = "build_interrupted", "This build was interrupted. Saved source and recorded activity remain available for continuation."
    else:
        reason, detail = "build_completed", "The host reports this build completed without pending cleanup. Package and application readiness require their separate current evidence."
    result.update(reason_code=reason, detail=detail)
    return result


def _requirements(state, acceptance):
    rows = (state.get("brief") or {}).get("requirements", [])
    if rows:
        return rows
    # Older briefs have an outcome fact but no requested-feature ledger. The
    # acceptance host supplies one tracking-only outcome without rewriting it.
    summary = (state.get("brief") or {}).get("facts", {}).get("outcome", {}).get("summary")
    accepted = acceptance.get("requirements", []) if isinstance(acceptance, dict) else []
    if not summary and len(accepted) == 1 and isinstance(accepted[0], dict):
        summary = accepted[0].get("text")
    return [{"id": "requested-outcome", "outcome": summary, "disposition": "open",
             "source_message_ids": [], "tracking_only": True}] if summary else []


def project_implementation_evidence(state, status=None, *, unavailable=False):
    """Return bounded display evidence without changing user-linked decisions.

    ``source_verified``, ``acceptance_trusted`` and ``acceptance_current`` must
    come from ImplementationService's current host verification, not a saved
    worker summary. Connection and running proof need a separate live observer;
    the current acceptance service only issues offline receipts.
    """
    plan = state.get("proposal") or state.get("blueprint")
    brief = state.get("brief") or {}
    plan_digest = proposal_hash(plan) if isinstance(plan, dict) and plan else None
    brief_digest = fingerprint(brief)
    receipt = status if isinstance(status, dict) else {}
    accepted = receipt.get("acceptance") or receipt.get("semantic_acceptance")
    if not isinstance(accepted, dict):
        accepted = None
    current = bool(receipt and current_version(receipt, state)
                   and plan_digest and receipt.get("blueprint_hash") == plan_digest)
    source_digest = receipt.get("source_digest")
    source_verified = bool(current and receipt.get("source_verified") is True and _digest(source_digest))
    acceptance_current = bool(current and accepted and _digest(source_digest)
        and receipt.get("acceptance_trusted") is True and receipt.get("acceptance_current") is True
        and accepted.get("issuer") == ACCEPTANCE_ISSUER
        and accepted.get("source_digest") == source_digest
        and accepted.get("blueprint_digest") == plan_digest
        and accepted.get("brief_digest") == brief_digest
        and accepted.get("status") in {"pass", "needs_repair", "incomplete"})
    required = _requirements(state, accepted if acceptance_current else None)
    findings = {}
    if acceptance_current:
        rows = accepted.get("requirements")
        expected = {row["id"]: row for row in required}
        if (not isinstance(rows, list) or len(rows) != len(expected)
                or any(not isinstance(row, dict) for row in rows)
                or {row.get("id") for row in rows} != set(expected)
                or any(row.get("text") != expected[row["id"]].get("outcome") for row in rows)):
            acceptance_current = False
        else:
            findings = {row["id"]: row for row in rows}

    passing = set()
    if acceptance_current and type(accepted.get("tests_run")) is int and accepted["tests_run"] > 0:
        passing = {row["id"] for row in accepted.get("test_results", [])
                   if isinstance(row, dict) and isinstance(row.get("id"), str) and row.get("status") == "pass"}
    results = []
    for row in required:
        finding = findings.get(row["id"], {})
        phase = finding.get("status", "proposal")
        paths = _strings(finding.get("source_paths"), text_limit=240)
        all_tests = _strings(finding.get("test_ids"), limit=100, text_limit=4000)
        tests = all_tests[:12]
        tested = bool(acceptance_current and phase == "tested" and all_tests and set(all_tests) <= passing)
        implemented = bool(acceptance_current and ((phase == "source" and paths) or tested))
        dependencies = _strings(finding.get("remaining_dependencies"))
        active = row.get("disposition") not in _INACTIVE
        if active and not implemented and not dependencies:
            dependencies = ["Implement and independently check this requested outcome."]
        elif active and implemented and not tested and not dependencies:
            dependencies = ["Add independent acceptance evidence for this outcome."]
        result = {"id": row["id"], "outcome": _text(row.get("outcome")),
                  "disposition": row.get("disposition", "open"),
                  "source_message_ids": list(row.get("source_message_ids", [])),
                  "decision_message_ids": list(row.get("decision_message_ids", [])),
                  "active": active, "implemented": implemented, "offline_tested": tested,
                  "connected": False, "running": False,
                  "phase": "offline-tested" if tested else "implemented" if implemented else "proposal",
                  "source_paths": paths if acceptance_current else [],
                  "test_ids": tests if tested else [],
                  "evidence": _text(finding.get("evidence"), 800) if acceptance_current else "",
                  "remaining_dependencies": dependencies}
        if row.get("tracking_only"):
            result["tracking_only"] = True
        results.append(result)

    gaps = []
    if unavailable:
        freshness, phase = "unavailable", "unavailable"
        gaps.append("Current implementation evidence is unavailable. Retry the build status before relying on earlier results.")
    elif not receipt:
        freshness, phase = "missing", "not-built"
        gaps.append("Build the current proposal to obtain source and independent acceptance evidence.")
    elif not current:
        freshness, phase = "stale", "stale"
        gaps.append("The saved build belongs to a different setup revision or proposal. Build the current specification.")
    else:
        freshness = "current"
        if isinstance(receipt.get("status"), str) and receipt["status"] in _IN_PROGRESS:
            phase = "building"
            gaps.append("Implementation and verification are still in progress.")
        elif acceptance_current:
            phase = "offline-tested" if accepted["status"] == "pass" else "needs-repair"
        else:
            phase = "source-checked" if source_verified else "needs-verification"
            gaps.append("Obtain a current independent acceptance review for the exact source and brief.")
        if not source_verified and not (isinstance(receipt.get("status"), str) and receipt["status"] in _IN_PROGRESS):
            gaps.append("Complete host source checks and a nonempty authored test suite on the current package.")
    if acceptance_current:
        gaps.extend(_strings(accepted.get("counterexamples")))
        gaps.extend(_strings(accepted.get("remaining_dependencies")))
    # These are package-to-operation steps. A passing offline receipt cannot
    # resolve them, including when model prose claims that a channel is live.
    if receipt:
        gaps.extend(["Configure and verify the selected connections and credentials on the destination.",
                     "Install, start and observe the system on the chosen deployment target."])
    ready_to_package = bool(current and source_verified and acceptance_current
        and accepted["status"] == "pass" and passing and receipt.get("ready_to_package") is True)
    return {"status": phase, "freshness": freshness, "current": current,
            "build_id": _text(receipt["build_id"], 100) if isinstance(receipt.get("build_id"), str)
                and _SCOPE_ID.fullmatch(receipt["build_id"]) else None,
            "guide_id": state.get("id"), "guide_version": receipt.get("guide_version")
                if type(receipt.get("guide_version")) is int and receipt["guide_version"] > 0 else None,
            "current_guide_version": state.get("version") if current else None,
            "blueprint_digest": plan_digest, "brief_digest": brief_digest,
            "source_digest": source_digest if current and _digest(source_digest) else None,
            "source_verified": source_verified, "acceptance_current": acceptance_current,
            "acceptance_status": accepted["status"] if acceptance_current else None,
            "review_round": accepted.get("review_round") if acceptance_current else None,
            "scope": "offline", "connected": False, "running": False,
            "ready_to_package": ready_to_package, "live_ready": False,
            "lifecycle": _implementation_lifecycle(state, status, content_current=current,
                source_verified=source_verified, unavailable=unavailable),
            "trial_replay": _trial_replay_evidence(state, receipt, current=current, source_verified=source_verified),
            "requirements": results, "remaining_gaps": list(dict.fromkeys(gaps))[:28]}


def compact_implementation_evidence(evidence):
    """Keep exact bindings and findings; the brief already carries user text."""
    result = copy.deepcopy(evidence)
    for row in result.get("requirements", []):
        for key in ("outcome", "source_message_ids", "decision_message_ids", "disposition"):
            row.pop(key, None)
    return result
