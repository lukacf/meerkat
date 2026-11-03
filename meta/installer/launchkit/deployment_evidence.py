"""Private-data-free interview context from the owned installation observer."""
from __future__ import annotations

from datetime import datetime, timezone
import re
from urllib.parse import urlsplit

from .projects import fingerprint
from .artifact_binding import current_version, proposal_hash
from .admission_health import health_is_fresh, retained_pending_admission

_DIGEST = re.compile(r"^[0-9a-f]{64}$")
_ATTEMPT = re.compile(r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$")
_STATES = {"queued", "installing", "checking", "starting", "running", "stopping",
           "stopped", "interrupted", "failed"}
_PHASES = {"install", "knowledge", "check", "connections", "start", "service",
           "run", "stop", "update", "recovery"}
_STARTUP_STATES = {"checking", "pass", "fail", "interrupted"}
_STARTUP_CATEGORIES = {"unavailable", "authorization_configuration", "authentication",
                       "http_status", "invalid_response", "missing_identities",
                       "timeout", "probe_error", "child_exited"}
_UPDATES = {"staged", "blocked", "snapshotting", "applying", "verifying", "completed",
            "restoring", "rolled-back", "restored-stopped", "recovering-candidate", "recovering-current",
            "candidate-restored-stopped", "recovery-needed", "failed"}
_REMOTE_STATES = {"connecting", "configuration-required", "installing", "checking", "starting",
                  "running", "stopping", "restarting", "stopped", "failed", "not-installed",
                  "recovery-needed", "observation-unavailable"}
_CLOUD_REMOVAL = {"removing", "removal-required", "removed"}


def _text(value, limit=100):
    return value[:limit] if isinstance(value, str) else None


def _fresh_health(value, now):
    return health_is_fresh(value, now)


def _console(value):
    try:
        url = urlsplit(value)
        return (url.scheme == "http" and url.hostname == "127.0.0.1"
                and url.path == "/console" and not url.username and not url.password
                and not url.query and not url.fragment and url.port is not None)
    except (TypeError, ValueError):
        return False


def project_startup_health(receipt):
    """Retain only a host observation bound to this exact startup attempt.

    Older unbound health records remain historical private receipt data. Neither
    raw failure text nor logs can supply a cause for this projection.
    """
    attempt = receipt.get("startup_attempt_id")
    value = receipt.get("startup_console_health")
    if (not isinstance(attempt, str) or not _ATTEMPT.fullmatch(attempt)
            or not isinstance(value, dict) or value.get("attempt_id") != attempt
            or not isinstance(value.get("status"), str) or value["status"] not in _STARTUP_STATES
            or type(value.get("attempts")) is not int or not 0 <= value["attempts"] <= 1000):
        return None
    category = value.get("category")
    if ((category is not None and not isinstance(category, str))
            or (value["status"] in {"checking", "fail"} and category not in _STARTUP_CATEGORIES)
            or (value["status"] in {"pass", "interrupted"} and category is not None)):
        return None
    result = {"status": value["status"], "attempts": value["attempts"]}
    if category is not None:
        result["category"] = category
    if "http_status" in value:
        status = value["http_status"]
        if category not in {"authentication", "http_status"} or type(status) is not int or not 100 <= status <= 599:
            return None
        result["http_status"] = status
    return result


def project_deployment_evidence(state, receipt=None, *, unavailable=False, now=None):
    """Separate installed source, current design and observed native availability.

    Only DeploymentService's owned-package observer supplies ``source_verified``.
    Native health cannot attest any application outcome, custom UI, external
    channel, hardware or connector authentication. Neither saved user decisions
    nor offline acceptance are changed by this read projection.
    """
    receipt = receipt if isinstance(receipt, dict) else {}
    same_guide = bool(receipt and receipt.get("guide_id") == state.get("id")
                      and isinstance(receipt.get("deployment_id"), str))
    source = receipt.get("source_digest")
    native_team = receipt.get("source_kind") == "native-team"
    source_identity = (receipt.get("build_id") is None and isinstance(receipt.get("source_id"), str)
                       and _DIGEST.fullmatch(receipt["source_id"])) if native_team else isinstance(receipt.get("build_id"), str)
    owned = bool(same_guide and receipt.get("source_verified") is True
                 and isinstance(source, str) and _DIGEST.fullmatch(source)
                 and source_identity)
    proposal = state.get("proposal") or state.get("blueprint")
    current = bool(owned and proposal and current_version(receipt, state)
        and receipt.get("blueprint_hash") == proposal_hash(proposal))
    status = receipt.get("status") if same_guide and receipt.get("status") in _STATES else None
    native = receipt.get("bootstrap") or {}
    clock = now or datetime.now(timezone.utc)
    admission = None
    observed_admission = receipt.get("admission_health")
    if (owned and status == "starting" and receipt.get("process_live") is True
            and receipt.get("health") == "pending" and _fresh_health(receipt.get("health_checked_at"), clock)):
        rows = retained_pending_admission(receipt)
        if rows is not None:
            admission = {"status": "pending", "admitting_count": len(rows["admitting"]), "owed_count": len(rows["owed"])}
    available = bool(owned and status == "running" and receipt.get("process_live") is True
        and observed_admission is None
        and receipt.get("health") == "ready" and native.get("complete") is True
        and native.get("ready") is True and type((native.get("counts") or {}).get("broken")) is int
        and native["counts"]["broken"] == 0 and _console(receipt.get("console_url"))
        and _fresh_health(receipt.get("health_checked_at"), clock))
    implementation = state.get("implementation_evidence") or {}
    same_build = bool(not native_team and current and implementation.get("current") is True
        and implementation.get("source_verified") is True
        and implementation.get("build_id") == receipt.get("build_id")
        and implementation.get("source_digest") == source)
    update = receipt.get("update") or {}
    update_status = update.get("status") if same_guide and update.get("status") in _UPDATES else None
    actions = []
    if same_guide:
        actions.append("Manage application")
    if available:
        actions.append("Open your Console")
    if update_status == "recovery-needed":
        actions.append("Review recovery")
    elif not native_team and owned and not same_build and implementation.get("ready_to_package") is True:
        actions.append("Review accepted update")
    return {"freshness": "unavailable" if unavailable else "missing" if not same_guide
                else "current" if current else "different-design" if owned else "unverified",
            "current": current, "owned_source_verified": owned,
            "deployment_id": _text(receipt.get("deployment_id")) if same_guide else None,
            "build_id": _text(receipt.get("build_id")) if owned else None,
            **({"source_kind": "native-team", "source_id": receipt["source_id"]} if native_team and owned else {}),
            "source_digest": source if owned else None,
            "guide_version": receipt.get("guide_version") if owned else None,
            "current_guide_version": state.get("version") if current else None,
            "status": status, "execution_mode": receipt.get("execution_mode")
                if owned and receipt.get("execution_mode") in {"interactive", "background"} else None,
            "phase": receipt.get("phase") if owned and isinstance(receipt.get("phase"), str)
                and receipt["phase"] in _PHASES else None,
            "startup_health": project_startup_health(receipt) if owned else None,
            "admission_health": admission,
            "matches_current_application": same_build, "native_runtime_available": available,
            "health_checked_at": _text(receipt.get("health_checked_at")) if available else None,
            "update_status": update_status, "suggested_actions": actions,
            "scope": "Owned installed source and native bootstrap/Console availability only.",
            "application_outcomes_verified": False, "external_connections_verified": False,
            "custom_ui_verified": False, "live_ready": False}


def project_remote_deployment_evidence(state, receipt=None, *, unavailable=False):
    """Keep remote source custody separate from local and browser availability.

    Only the owned remote observer supplies package verification and freshness.
    This projection includes no endpoint, user, folder, URL, log or failure text.
    A native service observation does not establish a browser tunnel, application
    outcome or external connection, and cannot authorize another operation.
    """
    from .remote_browser_access import native_ready

    receipt = receipt if isinstance(receipt, dict) else {}
    identity = receipt.get("deployment_id")
    same_guide = bool(receipt and receipt.get("guide_id") == state.get("id")
                      and isinstance(identity, str) and _ATTEMPT.fullmatch(identity))
    build = receipt.get("build_id")
    source = receipt.get("accepted_source_digest")
    package = receipt.get("package_sha256")
    transferred = receipt.get("transferred_source_digest")
    controller = receipt.get("controller_sha256")
    target = receipt.get("target", "linux")
    owned = bool(same_guide and receipt.get("owned_package_verified") is True
        and type(receipt.get("guide_version")) is int and receipt["guide_version"] > 0
        and isinstance(target, str) and target in {"linux", "gcp"}
        and isinstance(build, str) and _ATTEMPT.fullmatch(build)
        and all(isinstance(value, str) and _DIGEST.fullmatch(value)
                for value in (source, package, transferred, controller)))
    current = bool(owned and current_version(
        {**receipt, "current": receipt.get("source_binding_current")}, state))
    observation = receipt.get("observation") if isinstance(receipt.get("observation"), dict) else {}
    cloud = receipt.get("cloud_status")
    cloud = cloud if isinstance(cloud, str) and cloud in _CLOUD_REMOVAL else None
    pending = bool(receipt.get("pending_operation"))
    observed = bool(owned and not unavailable and observation.get("available") is True
        and receipt.get("observation_fresh") is True and not pending and cloud is None)
    status = receipt.get("status")
    status = status if isinstance(status, str) and status in _REMOTE_STATES else None
    historical = receipt.get("last_known_status")
    historical = historical if isinstance(historical, str) and historical in _REMOTE_STATES else None
    available = bool(observed and native_ready(receipt, current))
    implementation = state.get("implementation_evidence") or {}
    same_build = bool(current and implementation.get("current") is True
        and implementation.get("source_verified") is True and implementation.get("build_id") == build
        and implementation.get("source_digest") == source)
    actions = []
    if same_guide:
        actions.append("Manage remote destination")
        if cloud is not None:
            actions.append("Review cloud removal")
        elif not observed:
            actions.append("Recheck remote status")
        elif available:
            actions.append("Check or open the browser connection")
    return {"freshness": "unavailable" if unavailable else "missing" if not same_guide
                else "current" if current else "different-design" if owned else "unverified",
            "saved_installation": same_guide, "owned_source_verified": owned, "current": current,
            "deployment_id": identity if same_guide else None,
            "build_id": build if owned else None, "source_digest": source if owned else None,
            "guide_version": receipt.get("guide_version") if owned else None,
            "current_guide_version": state.get("version") if current else None,
            "target": target if owned else None,
            "installation_admitted": bool(owned and receipt.get("remote_admission_requested") is True
                                           and receipt.get("admitted") is True),
            "observation_available": observed, "pending_operation": pending if same_guide else False,
            "status": status if observed else None,
            "last_observed_status": (status if observed else historical) if same_guide else None,
            "cloud_status": cloud if same_guide else None,
            "matches_current_application": same_build, "native_runtime_available": available,
            "suggested_actions": actions,
            "scope": "Owned remote source and freshly observed native service only; browser access is checked separately.",
            "browser_connection_verified": False, "application_outcomes_verified": False,
            "external_connections_verified": False, "custom_ui_verified": False, "live_ready": False}
