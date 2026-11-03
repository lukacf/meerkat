import copy
from datetime import datetime, timedelta, timezone

import pytest

from launchkit.deployment_evidence import project_deployment_evidence, project_remote_deployment_evidence
from launchkit.projects import fingerprint

NOW = datetime(2026, 9, 30, 12, tzinfo=timezone.utc)


def sample():
    state = {"id": "guide", "version": 4, "proposal": {"name": "custom mob"},
             "implementation_evidence": {"current": True, "source_verified": True,
                 "build_id": "current-build", "source_digest": "a" * 64, "ready_to_package": True}}
    receipt = {"guide_id": "guide", "guide_version": 4, "current": True,
        "blueprint_hash": fingerprint(state["proposal"]), "deployment_id": "installed",
        "build_id": "current-build", "source_verified": True, "source_digest": "a" * 64,
        "status": "running", "execution_mode": "interactive", "process_live": True,
        "health": "ready", "health_checked_at": NOW.isoformat(),
        "console_url": "http://127.0.0.1:18123/console",
        "bootstrap": {"complete": True, "ready": True, "counts": {"broken": 0}},
        "destination": "/private/user/folder", "settings": {"API_KEY": "private-token"},
        "logs": ["secret output"], "checks": [{"detail": "private path"}],
        "detail": "Everything works!", "update": {"status": "completed", "detail": "secret update"}}
    return state, receipt


def project(state, receipt):
    return project_deployment_evidence(state, receipt, now=NOW)


def test_fresh_owned_pending_admission_projects_counts_without_claiming_readiness():
    from test_admission_health import pending_payload
    state, receipt = sample()
    pending = pending_payload()
    attempt = "12345678-1234-1234-1234-123456789abc"
    receipt.update(status="starting", health="pending", console_url=None,
        startup_attempt_id=attempt, bootstrap=pending["bootstrap"],
        admission_health={"attempt_id": attempt, **pending["admission"]})
    before = copy.deepcopy(receipt)
    evidence = project(state, receipt)
    assert evidence["admission_health"] == {"status": "pending", "admitting_count": 0, "owed_count": 1}
    assert not evidence["native_runtime_available"]
    assert "Open your Console" not in evidence["suggested_actions"]
    assert "private native detail" not in str(evidence) and "project:one" not in str(evidence)
    assert receipt == before
    for change in ({"process_live": False}, {"source_verified": False}, {"status": "failed"},
                   {"health_checked_at": (NOW - timedelta(seconds=31)).isoformat()},
                   {"startup_attempt_id": "different"}):
        assert project(state, {**receipt, **change})["admission_health"] is None


def test_pending_marker_cannot_be_overridden_by_stale_ready_flags():
    state, receipt = sample()
    receipt["admission_health"] = {"admitting": ["project:one"], "owed": []}
    assert not project(state, receipt)["native_runtime_available"]


def test_owned_native_runtime_does_not_certify_application_or_connections():
    state, receipt = sample()
    before = copy.deepcopy((state, receipt))
    evidence = project(state, receipt)
    assert evidence["current"] and evidence["owned_source_verified"]
    assert evidence["matches_current_application"] and evidence["native_runtime_available"]
    assert evidence["suggested_actions"] == ["Manage application", "Open your Console"]
    assert not any(evidence[key] for key in ("application_outcomes_verified", "external_connections_verified",
                                            "custom_ui_verified", "live_ready"))
    for private in ("private-token", "/private/user/folder", "secret output", "secret update", "Everything works!"):
        assert private not in str(evidence)
    assert (state, receipt) == before


@pytest.mark.parametrize("changes", [{"source_verified": False}, {"source_digest": "model said yes"},
    {"guide_id": "other"}, {"process_live": False}, {"health": "unknown"}, {"status": "stopped"},
    {"console_url": "https://outside.example/console"}, {"console_url": "http://127.0.0.1:1/console?token=secret"},
    {"bootstrap": {"complete": True, "ready": True, "counts": {"broken": False}}},
    {"health_checked_at": (NOW - timedelta(seconds=31)).isoformat()},
    {"health_checked_at": (NOW + timedelta(seconds=1)).isoformat()},
    {"health_checked_at": "2026-09-30T12:00:00"}, {"health_checked_at": None}])
def test_stale_or_unverified_health_cannot_claim_native_availability(changes):
    state, receipt = sample()
    receipt.update(changes)
    assert not project(state, receipt)["native_runtime_available"]


def test_older_installed_source_remains_distinct_from_current_design_and_build():
    state, receipt = sample()
    receipt.update(guide_version=3, current=False)
    evidence = project(state, receipt)
    assert evidence["freshness"] == "different-design" and evidence["owned_source_verified"]
    assert evidence["native_runtime_available"] and not evidence["current"]
    assert not evidence["matches_current_application"]
    assert "Review accepted update" in evidence["suggested_actions"]
    receipt.update(guide_version=4, current=True, source_digest="b" * 64, build_id="previous")
    evidence = project(state, receipt)
    assert evidence["current"] and not evidence["matches_current_application"]


def test_recovery_state_asks_for_recovery_without_restarting_or_reading_private_details():
    state, receipt = sample()
    receipt.update(status="failed", health="unknown", update={"status": "recovery-needed", "blockers": ["secret"]})
    evidence = project(state, receipt)
    assert evidence["update_status"] == "recovery-needed"
    assert evidence["suggested_actions"] == ["Manage application", "Review recovery"]
    assert "secret" not in str(evidence)


def test_missing_or_foreign_receipts_do_not_import_another_setup():
    state, receipt = sample()
    receipt["guide_id"] = "other"
    value = project(state, receipt)
    assert value["freshness"] == "missing" and value["deployment_id"] is None
    assert value["build_id"] is None and value["suggested_actions"] == []
    assert project_deployment_evidence(state, unavailable=True)["freshness"] == "unavailable"


def test_host_current_scope_projection_preserves_original_install_provenance():
    state, receipt = sample()
    state["version"] = 5
    receipt["current_guide_version"] = 5
    evidence = project(state, receipt)
    assert evidence["current"] and evidence["matches_current_application"]
    assert evidence["guide_version"] == 4
    receipt["current_guide_version"] = 4
    assert not project(state, receipt)["current"]


def test_owned_failed_start_projects_bound_cause_without_private_details_or_success_claims():
    state, receipt = sample()
    attempt = "12345678-1234-1234-1234-123456789abc"
    receipt.update(status="failed", phase="start", health="unknown", bootstrap=None,
        startup_attempt_id=attempt, startup_console_health={"attempt_id": attempt,
            "status": "fail", "category": "authentication", "http_status": 401, "attempts": 1,
            "detail": "private-token", "headers": {"Authorization": "secret output"},
            "url": "https://private.example/?token=private-token"})
    before = copy.deepcopy((state, receipt))
    evidence = project(state, receipt)
    assert evidence["phase"] == "start"
    assert evidence["startup_health"] == {"status": "fail", "category": "authentication", "http_status": 401, "attempts": 1}
    assert not evidence["native_runtime_available"] and not evidence["live_ready"]
    for private in ("private-token", "secret output", "private.example", "/private/user/folder", "Everything works!"):
        assert private not in str(evidence)
    assert (state, receipt) == before


def test_legacy_startup_eof_does_not_infer_a_model_cause_or_promote_passed_host_checks():
    state, receipt = sample()
    receipt.update(status="failed", phase="start", bootstrap=None, health="unknown",
        detail="model 'openai:gpt-4.1-mini' is absent from the effective model registry",
        checks=[{"name": "Local prerequisites", "status": "pass"}, {"name": "Application contract", "status": "pass"}],
        startup_console_health={"status": "fail", "category": "authentication", "attempts": 1})
    evidence = project(state, receipt)
    assert evidence["phase"] == "start" and evidence["startup_health"] is None
    assert not evidence["native_runtime_available"] and not evidence["live_ready"]
    assert "gpt-4.1-mini" not in str(evidence) and "checks" not in evidence


@pytest.mark.parametrize("changes", [
    {"startup_attempt_id": None}, {"startup_attempt_id": "new attempt"},
    {"startup_attempt_id": "87654321-1234-1234-1234-123456789abc"},
    {"startup_console_health": {"attempt_id": "12345678-1234-1234-1234-123456789abc", "status": ["fail"], "attempts": 1}},
    {"startup_console_health": {"attempt_id": "12345678-1234-1234-1234-123456789abc", "status": "fail", "category": ["authentication"], "attempts": 1}},
    {"startup_console_health": {"attempt_id": "12345678-1234-1234-1234-123456789abc", "status": "fail", "category": "secret output", "attempts": 1}},
    {"startup_console_health": {"attempt_id": "12345678-1234-1234-1234-123456789abc", "status": "fail", "category": "authentication", "attempts": True}},
    {"startup_console_health": {"attempt_id": "12345678-1234-1234-1234-123456789abc", "status": "fail", "category": "authentication", "attempts": 1, "http_status": "private-token"}},
    {"source_verified": False}, {"guide_id": "other"}])
def test_stale_unowned_and_malformed_startup_observations_are_not_guide_causes(changes):
    state, receipt = sample()
    attempt = "12345678-1234-1234-1234-123456789abc"
    receipt.update(status="failed", phase="start", startup_attempt_id=attempt,
        startup_console_health={"attempt_id": attempt, "status": "fail", "category": "authentication", "attempts": 1})
    receipt.update(changes)
    evidence = project(state, receipt)
    assert evidence["startup_health"] is None
    assert "private-token" not in str(evidence) and "secret output" not in str(evidence)
    if receipt.get("source_verified") is False or receipt.get("guide_id") == "other":
        assert evidence["phase"] is None


@pytest.mark.parametrize("phase", ["private-token", ["start"], {"private": "/private/user/folder"}])
def test_non_host_phase_is_not_projected(phase):
    state, receipt = sample()
    receipt.update(status="failed", phase=phase)
    assert project(state, receipt)["phase"] is None


def remote_sample():
    state, _ = sample()
    build = "11111111-1234-1234-1234-123456789abc"
    state["implementation_evidence"]["build_id"] = build
    receipt = {"guide_id": "guide", "guide_version": 4,
        "deployment_id": "22222222-1234-1234-1234-123456789abc", "build_id": build,
        "accepted_source_digest": "a" * 64, "package_sha256": "b" * 64,
        "transferred_source_digest": "c" * 64, "controller_sha256": "d" * 64,
        "owned_package_verified": True, "source_binding_current": True,
        "observation_fresh": True, "observation": {"available": True},
        "remote_admission_requested": True, "admitted": True, "pending_operation": None,
        "status": "running", "last_known_status": "running", "ready": True,
        "worker_live": False, "source_current": True, "service_owned": True,
        "service_observed": True, "service": {"status": "running", "ready": True,
            "service": "mobkit-launch.33333333-1234-1234-1234-123456789abc.service",
            "definition_verified": True, "source_current": True,
            "scope": "native-bootstrap-and-console", "source_digest": "c" * 64,
            "console_url": "http://127.0.0.1:18088/console"},
        "endpoint": {"host": "private-host", "user": "private-user"},
        "destination": "/private/folder", "detail": "private-error", "logs": ["secret-token"]}
    return state, receipt


def test_remote_interview_evidence_preserves_separate_checked_installation_without_private_details():
    state, receipt = remote_sample()
    before = copy.deepcopy((state, receipt))
    evidence = project_remote_deployment_evidence(state, receipt)
    assert evidence["saved_installation"] and evidence["installation_admitted"]
    assert evidence["current"] and evidence["matches_current_application"]
    assert evidence["observation_available"] and evidence["status"] == "running"
    assert evidence["native_runtime_available"]
    assert evidence["suggested_actions"] == ["Manage remote destination", "Check or open the browser connection"]
    assert not any(evidence[key] for key in ("browser_connection_verified", "application_outcomes_verified",
        "external_connections_verified", "custom_ui_verified", "live_ready"))
    assert not project(state, None)["owned_source_verified"]
    for value in ("private-host", "private-user", "/private/folder", "private-error", "secret-token", "http://"):
        assert value not in str(evidence)
    assert (state, receipt) == before
    state["version"] = 5
    receipt["current_guide_version"] = 5
    continued = project_remote_deployment_evidence(state, receipt)
    assert continued["current"] and continued["guide_version"] == 4
    assert continued["current_guide_version"] == 5
    receipt["current_guide_version"] = 4
    assert not project_remote_deployment_evidence(state, receipt)["current"]


@pytest.mark.parametrize("change", [
    {"observation_fresh": False}, {"observation": {"available": False}},
    {"pending_operation": {"action": "stop", "id": "private-operation"}},
    {"cloud_status": "removing"}, {"cloud_status": "removal-required"}, {"cloud_status": "removed"},
    {"source_binding_current": False}, {"source_current": False}, {"worker_live": True},
    {"service_owned": False}, {"service_observed": False}, {"owned_package_verified": False},
    {"guide_version": 3}, {"service": {"ready": True}},
])
def test_remote_unknown_pending_removal_or_unowned_readiness_never_claims_current_use(change):
    state, receipt = remote_sample()
    receipt.update(change)
    evidence = project_remote_deployment_evidence(state, receipt)
    assert not evidence["native_runtime_available"]
    assert evidence["saved_installation"]
    if "cloud_status" in change:
        assert evidence["cloud_status"] == change["cloud_status"]
        assert evidence["status"] is None and not evidence["observation_available"]
        assert evidence["last_observed_status"] == "running"
        assert evidence["suggested_actions"] == ["Manage remote destination", "Review cloud removal"]
    if "pending_operation" in change or "observation" in change or "observation_fresh" in change:
        assert evidence["status"] is None and not evidence["observation_available"]
        assert "Recheck remote status" in evidence["suggested_actions"]
    assert "private-operation" not in str(evidence)


@pytest.mark.parametrize("status", ["stopped", "failed", "recovery-needed"])
def test_checked_remote_terminal_state_remains_actionable_without_reinstall_claim(status):
    state, receipt = remote_sample()
    receipt.update(status=status, ready=False, last_known_status=status)
    evidence = project_remote_deployment_evidence(state, receipt)
    assert evidence["status"] == status and evidence["installation_admitted"]
    assert not evidence["native_runtime_available"]
    assert evidence["suggested_actions"] == ["Manage remote destination"]


@pytest.mark.parametrize("change", [{"guide_id": "another"}, {"deployment_id": "/private/folder"},
    {"build_id": "private-user"}, {"package_sha256": "private-token"}, {"target": ["gcp"]}])
def test_foreign_or_malformed_remote_evidence_cannot_gain_source_or_use_claims(change):
    state, receipt = remote_sample()
    receipt.update(change)
    evidence = project_remote_deployment_evidence(state, receipt)
    assert not evidence["owned_source_verified"] and not evidence["native_runtime_available"]
    if "target" in change:
        assert evidence["target"] is None
    assert "/private/folder" not in str(evidence) and "private-user" not in str(evidence)
    assert "private-token" not in str(evidence)
