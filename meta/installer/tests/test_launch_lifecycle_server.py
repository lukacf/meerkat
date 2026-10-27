"""Launch API delegation and private configuration error boundaries."""

import asyncio
import logging
import json
from types import SimpleNamespace
from unittest.mock import AsyncMock

import pytest
from fastapi.exceptions import RequestValidationError
from fastapi.testclient import TestClient
from starlette.requests import Request

from launchkit import server
from launchkit.architect import ArchitectUnavailable
from launchkit.projects import DraftConflict
from launchkit.rehearsal import RehearsalService, StartNotAdmitted, start_request_fingerprint
from launchkit.blueprint import starter_blueprint


GUIDE_ID = "67f2f55d-8fd4-4d92-b999-14fb240e5730"
BASE = f"/api/guide/{GUIDE_ID}"
SECRET = "private-credential-do-not-echo-9a128d"


@pytest.fixture
def services(monkeypatch):
    deployment = SimpleNamespace(**{
        name: AsyncMock(name=f"deployment.{name}")
        for name in ("options", "settings_options", "configure", "status", "submit", "action", "stage_update", "experiences")
    })
    rehearsal = SimpleNamespace(**{
        name: AsyncMock(name=f"rehearsal.{name}")
        for name in ("submit", "status", "timeline", "cancel", "continue_trial", "end")
    })
    operations = SimpleNamespace(submit=AsyncMock(name="guide_operations.submit"))
    guide = SimpleNamespace(apply=AsyncMock(name="guide.apply"),
        attachments=SimpleNamespace(resolve=lambda identity, values: values), input_surface=lambda: {})
    deployment.agent_team_availability = lambda: {"available": True}
    monkeypatch.setattr(server, "deployment", deployment)
    monkeypatch.setattr(server, "rehearsal", rehearsal)
    monkeypatch.setattr(server, "guide_operations", operations)
    monkeypatch.setattr(server, "guide", guide)
    return SimpleNamespace(deployment=deployment, rehearsal=rehearsal,
                           operations=operations, guide=guide)


@pytest.fixture
def client(services):
    # No lifespan admission: every tested service call is monkeypatched, and no
    # native worker, provider, deployment or canonical saved state is touched.
    return TestClient(server.app)


@pytest.mark.parametrize("build_id", [None, "checked-build"])
def test_deployment_options_delegates_exact_selected_build(client, services, build_id):
    result = {"build_id": "checked-build", "current": True,
              "fields": [{"key": "API_TOKEN", "secret": True, "required": True}],
              "host_provider_keys": ["OPENAI_API_KEY"]}
    services.deployment.options.return_value = result
    response = client.get(BASE + "/deployment/options",
                          params={} if build_id is None else {"build_id": build_id})
    assert response.status_code == 200, response.text
    assert response.json() == result
    services.deployment.options.assert_awaited_once_with(GUIDE_ID, build_id)
    assert response.headers["cache-control"] == "no-store"


def test_deployment_status_preserves_receipt_and_readiness_scope(client, services):
    result = {"deployment_id": "deployment-a", "status": "running", "health": "ready",
              "current": False, "console_url": "http://127.0.0.1:4318/console",
              "unresolved": ["No user-outcome transaction has been observed."]}
    services.deployment.status.return_value = result
    response = client.get(BASE + "/deployment")
    assert response.status_code == 200, response.text
    assert response.json() == result
    services.deployment.status.assert_awaited_once_with(GUIDE_ID)


def test_installed_settings_options_use_explicit_owned_deployment_not_current_candidate(client, services):
    result = {"deployment_id": "installed-a", "build_id": "installed-build", "version": 9,
              "source_verified": True, "scope": "verified-installed-private-configuration", "fields": []}
    services.deployment.settings_options.return_value = result
    response = client.get(BASE + "/deployment/settings", params={"deployment_id": "installed-a"})
    assert response.status_code == 200 and response.json() == result
    assert response.headers["cache-control"] == "no-store"
    services.deployment.settings_options.assert_awaited_once_with(GUIDE_ID, "installed-a")
    services.deployment.options.assert_not_awaited()


def test_installed_private_save_delegates_exact_source_and_explicit_removal_without_launch(client, services):
    services.deployment.configure.return_value = {"deployment_id": "installed-a", "status": "stopped"}
    body = {"version": 9, "deployment_id": "installed-a", "build_id": "installed-build",
            "source_digest": "a" * 64, "package_sha256": "b" * 64,
            "settings": {"OPTIONAL_TOKEN": "", "REQUIRED_TOKEN": SECRET}, "remove_fields": ["OPTIONAL_TOKEN"]}
    response = client.post(BASE + "/deployment/settings", json=body)
    assert response.status_code == 200 and response.json()["status"] == "stopped"
    assert SECRET not in response.text
    services.deployment.configure.assert_awaited_once_with(GUIDE_ID, 9, "installed-a", "installed-build",
        "a" * 64, "b" * 64, settings=body["settings"], remove_fields=["OPTIONAL_TOKEN"],
        use_host_provider_credentials=False)
    services.deployment.submit.assert_not_awaited()
    services.deployment.action.assert_not_awaited()


@pytest.mark.parametrize("changes", [{"version": True}, {"source_digest": SECRET},
    {"package_sha256": SECRET}, {"remove_fields": [SECRET]}, {"remove_fields": [False]},
    {"remove_fields": ["OPTIONAL", "OPTIONAL"]}, {"remove_fields": ["X"] * 101},
    {"use_host_provider_credentials": "false"}, {"private": SECRET}])
def test_installed_settings_validation_drops_private_input_and_requires_exact_fields(client, services, changes):
    body = {"version": 9, "deployment_id": "installed-a", "build_id": "installed-build",
            "source_digest": "a" * 64, "package_sha256": "b" * 64}
    response = client.post(BASE + "/deployment/settings", json={**body, **changes})
    assert response.status_code == 422 and SECRET not in response.text
    assert all("input" not in item and "ctx" not in item for item in response.json()["detail"])
    services.deployment.configure.assert_not_awaited()


def test_private_setting_names_in_validation_locations_are_not_echoed(client, services):
    body = {"version": 9, "deployment_id": "installed-a", "build_id": "installed-build",
            "source_digest": "a" * 64, "package_sha256": "b" * 64, "settings": {SECRET: []}}
    response = client.post(BASE + "/deployment/settings", json=body)
    assert response.status_code == 422 and SECRET not in response.text
    assert response.json()["detail"][0]["loc"] == ["body", "settings"]
    services.deployment.configure.assert_not_awaited()


def test_declared_application_experiences_remain_source_bound_and_unverified(client, services):
    result = {"deployment_id": "owned", "build_id": "installed", "source_digest": "a" * 64,
              "scope": "declared-source", "verified": False, "entries": []}
    services.deployment.experiences.return_value = result
    response = client.get(BASE + "/deployment/experiences")
    assert response.status_code == 200 and response.json() == result
    assert response.headers["cache-control"] == "no-store"
    services.deployment.experiences.assert_awaited_once_with(GUIDE_ID)


def test_trial_repair_forwards_exact_source_binding_to_the_host_service(client, monkeypatch):
    repair = AsyncMock(return_value={"build_id": "repair", "status": "queued"})
    monkeypatch.setattr("launchkit.trial_feedback.repair_trial", repair)
    body = {"version": 3, "rehearsal_id": "trial", "build_id": "build", "source_digest": "a" * 64}
    response = client.post(BASE + "/rehearsal/repair", json=body)
    assert response.status_code == 202 and response.json()["build_id"] == "repair"
    repair.assert_awaited_once_with(server.rehearsal, server.implementation,
        GUIDE_ID, 3, "trial", "build", "a" * 64, request="")
    repair.reset_mock()
    response = client.post(BASE + "/rehearsal/repair", json={**body, "source_digest": "bad"})
    assert response.status_code == 422
    repair.assert_not_awaited()


def test_trial_improvement_forwards_actual_user_feedback_and_rejects_unsafe_validation_echo(client, monkeypatch):
    repair = AsyncMock(return_value={"build_id": "improved", "status": "queued"})
    monkeypatch.setattr("launchkit.trial_feedback.repair_trial", repair)
    body = {"version": 3, "rehearsal_id": "trial", "build_id": "build", "source_digest": "a" * 64,
            "request": "I could see proposals but could not try confirmation. Add explicit fake channel support."}
    response = client.post(BASE + "/rehearsal/repair", json=body)
    assert response.status_code == 202 and response.json()["build_id"] == "improved"
    repair.assert_awaited_once_with(server.rehearsal, server.implementation,
        GUIDE_ID, 3, "trial", "build", "a" * 64, request=body["request"])
    repair.reset_mock()
    for invalid in [{"private": SECRET}, SECRET * 700]:
        response = client.post(BASE + "/rehearsal/repair", json={**body, "request": invalid})
        assert response.status_code == 422 and SECRET not in response.text
        assert all("input" not in item and "ctx" not in item for item in response.json().get("detail", []))
    repair.assert_not_awaited()


@pytest.mark.parametrize("use_host", [None, False, True])
def test_deployment_submit_forwards_private_settings_without_exposing_them(
    client, services, use_host,
):
    result = {"deployment_id": "deployment-a", "status": "queued", "guide_version": 9}
    services.deployment.submit.return_value = result
    settings = {"OPENAI_API_KEY": SECRET, "CUSTOM_ORIGIN": "http://localhost:9000"}
    body = {"version": 9, "build_id": "checked-build", "destination": "/tmp/owned-app",
            "settings": settings}
    if use_host is not None:
        body["use_host_provider_credentials"] = use_host
    response = client.post(BASE + "/deployment", json=body)
    assert response.status_code == 202, response.text
    assert response.json() == result
    assert SECRET not in response.text
    services.deployment.submit.assert_awaited_once_with(
        GUIDE_ID, 9, "checked-build", "/tmp/owned-app", settings, bool(use_host),
        execution_mode="interactive",
    )


def test_deployment_submit_default_configuration_is_explicit(client, services):
    services.deployment.submit.return_value = {"status": "queued"}
    response = client.post(BASE + "/deployment", json={
        "version": 9, "build_id": "checked-build", "destination": "/tmp/owned-app",
    })
    assert response.status_code == 202, response.text
    services.deployment.submit.assert_awaited_once_with(
        GUIDE_ID, 9, "checked-build", "/tmp/owned-app", {}, False,
        execution_mode="interactive",
    )


@pytest.mark.parametrize("action", ["stop", "restart", "retry"])
def test_deployment_actions_delegate_exact_owned_deployment(client, services, action):
    result = {"deployment_id": "deployment-a", "status": "queued", "action": action}
    services.deployment.action.return_value = result
    response = client.post(BASE + "/deployment/action", json={
        "deployment_id": "deployment-a", "action": action,
    })
    assert response.status_code == 200, response.text
    assert response.json() == result
    services.deployment.action.assert_awaited_once_with(GUIDE_ID, "deployment-a", action)


def test_update_staging_forwards_exact_owned_deployment_and_current_build(client, services):
    result = {"deployment_id": "deployment-a", "update": {"update_id": "update-a", "status": "staged"}}
    services.deployment.stage_update.return_value = result
    response = client.post(BASE + "/deployment/update-stage", json={
        "deployment_id": "deployment-a", "version": 9, "build_id": "checked-build"})
    assert response.status_code == 202 and response.json() == result
    services.deployment.stage_update.assert_awaited_once_with(GUIDE_ID, "deployment-a", 9, "checked-build")


@pytest.mark.parametrize("action", ["apply-update", "rollback", "recover-current"])
def test_update_actions_require_and_forward_the_reviewed_update_identity(client, services, action):
    services.deployment.action.return_value = {"deployment_id": "deployment-a", "status": "queued"}
    body = {"deployment_id": "deployment-a", "action": action}
    assert client.post(BASE + "/deployment/action", json=body).status_code == 422
    services.deployment.action.assert_not_awaited()
    response = client.post(BASE + "/deployment/action", json={**body, "update_id": "update-a"})
    assert response.status_code == 200
    services.deployment.action.assert_awaited_once_with(GUIDE_ID, "deployment-a", action, update_id="update-a")


@pytest.mark.parametrize("action", ["stop", "restart", "retry"])
def test_operating_action_cannot_silently_ignore_a_staged_update_identity(client, services, action):
    response = client.post(BASE + "/deployment/action", json={
        "deployment_id": "deployment-a", "action": action, "update_id": "update-a"})
    assert response.status_code == 422
    services.deployment.action.assert_not_awaited()


def test_rehearsal_submit_delegates_native_flow_and_fixture_inputs(client, services):
    result = {"rehearsal_id": "trial-a", "status": "queued", "live_ready": False,
              "scope": "Native design rehearsal; application adapters are unavailable."}
    services.rehearsal.submit.return_value = result
    params = {"topic": "reading list", "items": 3}
    fixtures = {"files": {"reading-list.txt": "One sample item"}, "notes": ["A sample note"]}
    response = client.post(BASE + "/rehearsal", json={
        "version": 9, "text": "Summarize my reading list", "flow_id": "reading-flow",
        "params": params, "fixtures": fixtures,
    })
    assert response.status_code == 202, response.text
    assert response.json() == result
    services.rehearsal.submit.assert_awaited_once_with(
        GUIDE_ID, 9, "Summarize my reading list", flow_id="reading-flow",
        params=params, fixtures=fixtures, mode="design", build_id=None, application_fixtures=None, keep_open=False, start_request_id=None,
    )


def test_rehearsal_submit_does_not_invent_flow_or_fixture_inputs(client, services):
    services.rehearsal.submit.return_value = {"rehearsal_id": "trial-a", "status": "queued"}
    response = client.post(BASE + "/rehearsal", json={"version": 9, "text": "Hello"})
    assert response.status_code == 202, response.text
    services.rehearsal.submit.assert_awaited_once_with(
        GUIDE_ID, 9, "Hello", flow_id=None, params=None, fixtures=None,
        mode="design", build_id=None, application_fixtures=None, keep_open=False, start_request_id=None,
    )


def test_rehearsal_start_replay_support_and_exact_action_are_explicit(client, services, monkeypatch):
    monkeypatch.setattr(server, "planner_capability", lambda: {"available": False})
    monkeypatch.setattr(server, "cloud_capability", lambda: {"available": False})
    capability = client.get("/api/capabilities")
    assert capability.status_code == 200
    assert capability.json()["rehearsal"] == {"start_request_replay": True}
    action = "d8377671-eccc-4f39-937a-4108a5e43594"
    result = {"rehearsal_id": action, "start_request_id": action, "start_request_complete": True,
              "start_request_fingerprint": "a" * 64, "status": "completed", "current": False}
    services.rehearsal.submit.return_value = result
    response = client.post(BASE + "/rehearsal", json={"version": 9, "text": "The original job", "start_request_id": action})
    assert response.status_code == 202 and response.json() == result
    services.rehearsal.submit.assert_awaited_once_with(GUIDE_ID, 9, "The original job",
        flow_id=None, params=None, fixtures=None, mode="design", build_id=None,
        application_fixtures=None, keep_open=False, start_request_id=action)


def test_real_host_rejection_route_preserves_http_error_and_exact_durable_non_admission(client, tmp_path, monkeypatch):
    state = {"id": GUIDE_ID, "version": 9, "proposal": starter_blueprint().model_dump(mode="json")}
    service = RehearsalService(tmp_path, SimpleNamespace(get=AsyncMock(return_value=state)))
    monkeypatch.setattr(server, "rehearsal", service)
    action = "d8377671-eccc-4f39-937a-4108a5e43594"
    body = {"version": 9, "text": "Original exact job", "start_request_id": action,
            "fixtures": {"files": {"sample-context.txt": "界" * 80_000}}}
    first = client.post(BASE + "/rehearsal", json=body)
    assert first.status_code == 422
    witness = first.json()["start_rejection"]
    assert witness == StartNotAdmitted(GUIDE_ID, 9, "design", action,
        start_request_fingerprint(GUIDE_ID, 9, body["text"], fixtures=body["fixtures"]), 422).projection()
    path = service._rejected_start_path(GUIDE_ID, action)
    before = path.read_bytes()
    repeated = client.post(BASE + "/rehearsal", json=body)
    assert repeated.status_code == 422 and repeated.json()["start_rejection"] == witness
    changed = client.post(BASE + "/rehearsal", json={**body, "fixtures": {}})
    assert changed.status_code == 409 and "start_rejection" not in changed.json()
    assert path.read_bytes() == before and not service.tasks and not service.processes
    assert not (service.root / GUIDE_ID).exists()


@pytest.mark.parametrize("typed", [False, True])
def test_unbound_or_ordinary_route_error_does_not_claim_non_admission(client, services, typed):
    action = "d8377671-eccc-4f39-937a-4108a5e43594"
    error = ValueError("Unconfirmed request failure")
    if typed:
        error.trial_start_rejection = StartNotAdmitted(GUIDE_ID, 9, "design", action, "a" * 64, 422)
    services.rehearsal.submit.side_effect = error
    response = client.post(BASE + "/rehearsal", json={"version": 9, "text": "Exact job", "start_request_id": action})
    assert response.status_code == 422 and response.json() == {"detail": "Unconfirmed request failure"}


@pytest.mark.parametrize("action", ["", "not-a-uuid", "../escape", 12, True,
    "D8377671-ECCC-4F39-937A-4108A5E43594", "d8377671eccc4f39937a4108a5e43594",
    "{d8377671-eccc-4f39-937a-4108a5e43594}"])
def test_rehearsal_start_action_rejects_noncanonical_or_coerced_ids(client, services, action):
    response = client.post(BASE + "/rehearsal", json={"version": 9, "text": "Sample", "start_request_id": action})
    assert response.status_code == 422
    services.rehearsal.submit.assert_not_awaited()


def test_rehearsal_status_and_cancel_preserve_service_receipts(client, services):
    running = {"rehearsal_id": "trial-a", "status": "running", "connected": False}
    services.rehearsal.status.return_value = running
    response = client.get(BASE + "/rehearsal")
    assert response.status_code == 200, response.text
    assert response.json() == {**running, "browser_console_url": None}
    services.rehearsal.status.assert_awaited_once_with(GUIDE_ID)
    interrupted = {**running, "status": "interrupted", "trace_complete": False}
    services.rehearsal.cancel.return_value = interrupted
    response = client.post(BASE + "/rehearsal/cancel", json={"rehearsal_id": "trial-a"})
    assert response.status_code == 200, response.text
    assert response.json() == interrupted
    services.rehearsal.cancel.assert_awaited_once_with(GUIDE_ID, "trial-a")


def test_open_trial_and_followup_routes_preserve_explicit_trial_turn_and_text(client, services):
    services.rehearsal.submit.return_value = {"rehearsal_id": "trial-a", "keep_open": True, "status": "queued"}
    response = client.post(BASE + "/rehearsal", json={"version": 9, "text": "A sample job", "keep_open": True})
    assert response.status_code == 202
    assert services.rehearsal.submit.await_args.kwargs["keep_open"] is True
    receipt = {"rehearsal_id": "trial-a", "turn_count": 1, "status": "running",
        "turn_command": {"id": "host-command", "turn": 2, "status": "sent"}}
    services.rehearsal.continue_trial.return_value = receipt
    response = client.post(BASE + "/rehearsal/continue", json={"version": 9, "rehearsal_id": "trial-a", "expected_turn": 1, "text": "Read it back"})
    assert response.status_code == 202 and response.json() == receipt
    services.rehearsal.continue_trial.assert_awaited_once_with(GUIDE_ID, 9, "trial-a", 1, "Read it back")
    services.rehearsal.end.return_value = {"rehearsal_id": "trial-a", "status": "ending"}
    response = client.post(BASE + "/rehearsal/end", json={"rehearsal_id": "trial-a"})
    assert response.status_code == 202 and response.json()["status"] == "ending"
    services.rehearsal.end.assert_awaited_once_with(GUIDE_ID, "trial-a")


@pytest.mark.parametrize("bad", [{"expected_turn": True}, {"expected_turn": 0}, {"expected_turn": 10},
    {"version": True}, {"text": "x" * 6001}, {"conversation_id": "invented"}, {"rehearsal_id": ""}])
def test_followup_request_rejects_ambiguous_or_unsupported_fields_before_native_admission(client, services, bad):
    body = {"version": 9, "rehearsal_id": "trial-a", "expected_turn": 1, "text": "Next"}
    response = client.post(BASE + "/rehearsal/continue", json={**body, **bad})
    assert response.status_code == 422
    services.rehearsal.continue_trial.assert_not_awaited()


@pytest.mark.parametrize("suffix,body", [("/continue", {"version": 9, "rehearsal_id": "trial-a", "expected_turn": 1, "text": "PRIVATE-CANARY-" * 500}),
    ("/continue", {"version": 9, "rehearsal_id": "trial-a", "expected_turn": 1, "text": {"secret": "PRIVATE-CANARY"}}),
    ("", {"version": 9, "text": "PRIVATE-CANARY-" * 500})])
def test_rehearsal_request_validation_never_echoes_rejected_private_context(client, services, suffix, body):
    response = client.post(BASE + "/rehearsal" + suffix, json=body)
    assert response.status_code == 422 and "PRIVATE-CANARY" not in response.text
    assert all(set(row) == {"loc", "type", "msg"} for row in response.json()["detail"])
    services.rehearsal.submit.assert_not_awaited()
    services.rehearsal.continue_trial.assert_not_awaited()


@pytest.mark.parametrize("explicit", [False, True])
def test_rehearsal_timeline_preserves_native_frame_authority(client, services, explicit):
    # The API must relay real native IDs, versions, payloads and completion
    # semantics instead of replacing them with application-authored events.
    frame = {"id": "native-frame-a", "cursor": "console:19", "version": 4,
             "kind": "interaction_complete", "agent_identity": "librarian:main",
             "interaction_id": "native-turn-a",
             "payload": {"source_event_type": "run_completed", "extra_native_field": "kept"}}
    page = {"available": True, "rehearsal_id": "trial-a", "frames": [frame],
            "next_cursor": "rehearsal:3", "exhausted": True, "trace_complete": True}
    services.rehearsal.timeline.return_value = page
    params = {"after": "rehearsal:2", "limit": 37, "rehearsal_id": "trial-a"} if explicit else {}
    response = client.get(BASE + "/rehearsal/timeline", params=params)
    assert response.status_code == 200, response.text
    assert response.json() == page
    services.rehearsal.timeline.assert_awaited_once_with(
        GUIDE_ID, "rehearsal:2" if explicit else None, 37 if explicit else 200,
        "trial-a" if explicit else None,
    )


SERVICE_ROUTES = [
    ("get", "/deployment/options", "deployment", "options", None),
    ("get", "/deployment", "deployment", "status", None),
    ("post", "/deployment", "deployment", "submit",
     {"version": 9, "build_id": "checked-build", "destination": "/tmp/owned-app"}),
    ("post", "/deployment/action", "deployment", "action",
     {"deployment_id": "deployment-a", "action": "stop"}),
    ("post", "/deployment/update-stage", "deployment", "stage_update",
     {"deployment_id": "deployment-a", "version": 9, "build_id": "checked-build"}),
    ("post", "/rehearsal", "rehearsal", "submit", {"version": 9, "text": "Hello"}),
    ("get", "/rehearsal", "rehearsal", "status", None),
    ("get", "/rehearsal/timeline", "rehearsal", "timeline", None),
    ("post", "/rehearsal/cancel", "rehearsal", "cancel", {"rehearsal_id": "trial-a"}),
]


@pytest.mark.parametrize("method,path,service,operation,body", SERVICE_ROUTES)
@pytest.mark.parametrize("exception,status,detail", [
    (DraftConflict("This proposal changed. Reload before continuing."), 409,
     "This proposal changed. Reload before continuing."),
    (LookupError("This setup could not be found."), 404, "This setup could not be found."),
    (ValueError("A current checked proposal is required."), 422,
     "A current checked proposal is required."),
    (ArchitectUnavailable("The native runtime is unavailable."), 503,
     "The native runtime is unavailable."),
    (TimeoutError(SECRET), 504, None),
    (RuntimeError(SECRET), 502, None),
])
def test_lifecycle_service_errors_use_guide_conflict_routing(
    client, services, method, path, service, operation, body, exception, status, detail,
):
    call = getattr(getattr(services, service), operation)
    call.side_effect = exception
    response = client.request(method, BASE + path, **({"json": body} if body is not None else {}))
    assert response.status_code == status, response.text
    if detail is not None:
        assert response.json() == {"detail": detail}
    assert SECRET not in response.text
    call.assert_awaited_once()


@pytest.mark.parametrize("operation", ["respond", "apply"])
def test_guide_revision_conflict_is_not_reported_as_success(client, services, operation):
    call = services.operations.submit if operation == "respond" else services.guide.apply
    call.side_effect = DraftConflict("This guide revision is stale.")
    response = client.post(BASE + "/" + operation, json={"version": 9})
    assert response.status_code == 409
    assert response.json() == {"detail": "This guide revision is stale."}
    if operation == "respond":
        args = call.await_args.args
        assert args[:2] == (services.guide, GUIDE_ID)
        assert args[2].version == 9
    else:
        call.assert_awaited_once_with(GUIDE_ID, 9)


PRIVATE_INVALID_REQUESTS = [
    {"settings": SECRET},
    {"settings": {"API_TOKEN": {"value": SECRET}}},
    {"settings": {"API_TOKEN": [SECRET]}},
    {"settings": {"API_TOKEN": None, "OTHER_TOKEN": SECRET}},
    {"settings": {f"TOKEN_{index}": SECRET for index in range(201)}},
    {"version": SECRET},
    {"version": True},
    {"destination": {"private": SECRET}},
    {"destination": SECRET * 200},
    {"build_id": SECRET * 3},
    {"use_host_provider_credentials": SECRET},
    {"execution_mode": SECRET},
    {"unexpected_private_field": SECRET},
]


def assert_private_validation(response):
    assert response.status_code == 422, response.text
    assert SECRET not in response.text
    errors = response.json()["detail"]
    assert errors
    for error in errors:
        assert set(error) == {"loc", "type", "msg"}
        assert error["loc"]
        assert error["type"] and error["msg"]


@pytest.mark.parametrize("changes", PRIVATE_INVALID_REQUESTS)
def test_deployment_validation_never_echoes_rejected_credential_input_or_context(
    client, services, changes,
):
    body = {"version": 9, "build_id": "checked-build", "destination": "/tmp/owned-app",
            "settings": {"API_TOKEN": SECRET}, **changes}
    response = client.post(BASE + "/deployment", json=body)
    assert_private_validation(response)
    services.deployment.submit.assert_not_awaited()


@pytest.mark.parametrize("changes", [
    {"deployment_id": SECRET * 3}, {"action": SECRET}, {"credential": SECRET},
])
def test_deployment_action_validation_also_hides_private_input(client, services, changes):
    body = {"deployment_id": "deployment-a", "action": "stop", **changes}
    response = client.post(BASE + "/deployment/action", json=body)
    assert_private_validation(response)
    services.deployment.action.assert_not_awaited()


@pytest.mark.parametrize("changes", [{"version": True}, {"version": "9"},
    {"build_id": SECRET * 3}, {"deployment_id": SECRET * 3}, {"private": SECRET}])
def test_update_stage_validation_hides_rejected_input_and_never_admits_a_job(client, services, changes):
    response = client.post(BASE + "/deployment/update-stage", json={
        "deployment_id": "deployment-a", "version": 9, "build_id": "checked-build", **changes})
    assert_private_validation(response)
    services.deployment.stage_update.assert_not_awaited()


def test_deployment_malformed_json_does_not_echo_private_document(client, services):
    body = json.dumps({"version": 9, "build_id": "checked-build", "destination": "/tmp/app",
                       "settings": {"API_TOKEN": SECRET}})[:-1]
    response = client.post(BASE + "/deployment", content=body,
                           headers={"Content-Type": "application/json"})
    assert_private_validation(response)
    services.deployment.submit.assert_not_awaited()


def test_deployment_validation_handler_drops_embedded_exception_context():
    # Current fields produce scalar type/length errors; retain the stronger
    # boundary if a future field validator embeds credentials in its ctx.
    error = RequestValidationError([{
        "type": "value_error", "loc": ("body", "settings", "API_TOKEN"),
        "msg": "Invalid private configuration", "input": SECRET,
        "ctx": {"error": ValueError(SECRET), "credential": SECRET},
    }])
    request = Request({"type": "http", "method": "POST", "scheme": "http",
                       "path": BASE + "/deployment", "query_string": b"", "headers": [],
                       "server": ("testserver", 80)})
    response = asyncio.run(server.request_validation_error(request, error))
    assert response.status_code == 422
    assert SECRET.encode() not in response.body
    assert json.loads(response.body) == {"detail": [{
        "type": "value_error", "loc": ["body", "settings"],
        "msg": "Invalid private configuration",
    }]}


@pytest.mark.parametrize("path,body,service,operation", [
    ("/rehearsal", {"version": True, "text": "Hello"}, "rehearsal", "submit"),
    ("/rehearsal", {"version": 9, "text": "Hello", "shell": "ignored"}, "rehearsal", "submit"),
    ("/rehearsal/cancel", {"rehearsal_id": ""}, "rehearsal", "cancel"),
])
def test_rehearsal_invalid_contract_is_rejected_before_service_admission(
    client, services, path, body, service, operation,
):
    response = client.post(BASE + path, json=body)
    assert response.status_code == 422
    getattr(getattr(services, service), operation).assert_not_awaited()


def test_rehearsal_timeline_noninteger_limit_is_rejected_before_delegation(client, services):
    response = client.get(BASE + "/rehearsal/timeline", params={"limit": "not-a-number"})
    assert response.status_code == 422
    services.rehearsal.timeline.assert_not_awaited()


def test_background_mode_is_explicit_and_forwarded(client, services):
    services.deployment.submit.return_value = {"status": "queued", "execution_mode": "background"}
    response = client.post(BASE + "/deployment", json={"version": 9, "build_id": "checked-build",
        "destination": "/tmp/owned-app", "execution_mode": "background"})
    assert response.status_code == 202
    services.deployment.submit.assert_awaited_once_with(GUIDE_ID, 9, "checked-build", "/tmp/owned-app", {}, False,
        execution_mode="background")


def test_application_trial_forwards_exact_source_selection_and_fake_io(client, services):
    services.rehearsal.submit.return_value = {"status": "queued", "mode": "application"}
    fixtures = {"forecast": {"cloud": 80}}
    response = client.post(BASE + "/rehearsal", json={"version": 9, "text": "Find a good night",
        "mode": "application", "build_id": "checked-build", "application_fixtures": fixtures})
    assert response.status_code == 202
    services.rehearsal.submit.assert_awaited_once_with(GUIDE_ID, 9, "Find a good night", flow_id=None,
        params=None, fixtures=None, mode="application", build_id="checked-build", application_fixtures=fixtures, keep_open=False, start_request_id=None)


@pytest.mark.parametrize("failed_owner", [None, "guide_operations", "rehearsal", "architect"])
@pytest.mark.parametrize("body_fails", [False, True])
def test_lifespan_attempts_every_cleanup_after_owner_or_body_failure(monkeypatch, failed_owner, body_fails):
    owners = ["guide_operations", "visualization_operations", "remote_deployment", "deployment",
              "rehearsal", "implementation", "architect"]
    calls = []
    cleanup_error = RuntimeError("owned cleanup unavailable")
    body_error = ValueError("lifespan body failed")
    for name in owners:
        async def shutdown(owner=name):
            calls.append(owner)
            if owner == failed_owner:
                raise cleanup_error
        monkeypatch.setattr(server, name, SimpleNamespace(shutdown=shutdown))

    async def run():
        async with server.lifespan(server.app):
            assert calls == []
            if body_fails:
                raise body_error

    if failed_owner:
        with pytest.raises(ExceptionGroup) as raised:
            asyncio.run(run())
        assert raised.value.exceptions == (cleanup_error,)
    elif body_fails:
        with pytest.raises(ValueError) as raised:
            asyncio.run(run())
        assert raised.value is body_error
    else:
        asyncio.run(run())
    assert calls == owners


def test_lifespan_logs_each_owner_stop_in_order(monkeypatch, caplog):
    failure = RuntimeError("owned cleanup unavailable")
    for name in ["guide_operations", "deployment", "rehearsal", "architect"]:
        async def shutdown(owner=name):
            if owner == "rehearsal":
                raise failure
        monkeypatch.setattr(server, name, SimpleNamespace(shutdown=shutdown))

    async def run():
        async with server.lifespan(server.app):
            pass

    caplog.set_level(logging.INFO, logger="uvicorn.error")
    with pytest.raises(ExceptionGroup):
        asyncio.run(run())
    lines = [record.getMessage() for record in caplog.records if record.name == "uvicorn.error"]
    stopping = [line.removeprefix("Meta Forge: stopping ") for line in lines
                if line.startswith("Meta Forge: stopping ") and not line.endswith(")")]
    assert stopping == ["guide operations", "visualization operations", "visualization reviews",
        "visual critic runtime", "remote deployment", "GCP resource preparation", "GCP access checks",
        "deployment", "rehearsal", "implementation", "checker preparation", "saved Toolkit requests", "architect"]
    assert any(line.startswith("Meta Forge: stopped deployment in ") and line.endswith(" s") for line in lines)
    assert "Meta Forge: stopping rehearsal failed (RuntimeError)" in lines
    assert not any(line.startswith("Meta Forge: stopped rehearsal") for line in lines)


def test_lifespan_retains_multiple_cleanup_failures_and_cancellation(monkeypatch):
    owners = ["guide_operations", "visualization_operations", "remote_deployment", "deployment",
              "rehearsal", "implementation", "architect"]
    calls = []
    errors = {"visualization_operations": asyncio.CancelledError(),
              "rehearsal": RuntimeError("worker cleanup unconfirmed")}
    for name in owners:
        async def shutdown(owner=name):
            calls.append(owner)
            if owner in errors:
                raise errors[owner]
        monkeypatch.setattr(server, name, SimpleNamespace(shutdown=shutdown))

    async def run():
        async with server.lifespan(server.app):
            pass

    with pytest.raises(BaseExceptionGroup) as raised:
        asyncio.run(run())
    assert raised.value.exceptions == tuple(errors.values())
    assert calls == owners
