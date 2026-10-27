"""Human success reports route privately without admitting an implementation run."""
from copy import deepcopy
import json
from types import SimpleNamespace
from unittest.mock import AsyncMock
import uuid

from fastapi.testclient import TestClient
import pytest

from launchkit.projects import DraftConflict


@pytest.fixture(params=["deployment", "remote-deployment"])
def api(request, monkeypatch):
    from launchkit import server
    report = {"scope": "human-reported-installed-success", "recorded_at": "2026-10-03T10:00:00Z",
              "outcome": "worked", "application_outcomes_verified": False}
    success = AsyncMock(return_value=report)
    feedback = AsyncMock(side_effect=AssertionError("must not rebuild"))
    submit = AsyncMock(side_effect=AssertionError("must not run implementation agent"))
    service = SimpleNamespace(record_success=success, feedback=feedback)
    name = "deployment" if request.param == "deployment" else "remote_deployment"
    monkeypatch.setattr(server, name, service)
    monkeypatch.setattr(server.implementation, "submit", submit)
    guide_id = str(uuid.uuid4())
    return TestClient(server.app), f"/api/guide/{guide_id}/{request.param}/use/success", guide_id, success, feedback, submit


def body():
    return {"version": 4, "deployment_id": "owned-install", "build_id": "installed-build",
            "source_digest": "a" * 64, "package_sha256": "b" * 64,
            "request_id": "11111111-1111-4111-8111-111111111111",
            "task": "Plan one useful observing session."}


def test_success_routes_exact_original_body_without_feedback_or_builder(api):
    client, url, guide_id, success, feedback, submit = api
    submitted = body()
    response = client.post(url, json=submitted)
    assert response.status_code == 200
    assert response.json()["scope"] == "human-reported-installed-success"
    assert response.json()["application_outcomes_verified"] is False
    success.assert_awaited_once_with(guide_id, 4, "owned-install", "installed-build", "a" * 64,
                                    "b" * 64, request_id=submitted["request_id"], task=submitted["task"])
    feedback.assert_not_awaited()
    submit.assert_not_awaited()
    assert response.headers["cache-control"] == "no-store"
    assert response.headers["referrer-policy"] == "no-referrer"


@pytest.mark.parametrize("changes", [
    {"version": True}, {"version": "4"}, {"version": 0}, {"version": 2 ** 53},
    {"deployment_id": 3}, {"deployment_id": ""}, {"deployment_id": "x" * 65},
    {"build_id": 3}, {"build_id": ""}, {"build_id": "x" * 65},
    {"source_digest": "A" * 64}, {"source_digest": "a" * 63}, {"source_digest": 42},
    {"package_sha256": "b" * 65}, {"package_sha256": "private-source-canary"},
    {"request_id": "AAAAAAAA-AAAA-4AAA-8AAA-AAAAAAAAAAAA"}, {"request_id": "invalid"},
    {"request_id": 42}, {"task": 42}, {"task": ""}, {"task": "x" * 3001},
    {"task": " \n\t"}, {"task": "private-task-canary\x00"}, {"task": "private-task-canary\x7f"},
    {"private-extra-key": "private-value-canary"}, {"outcome": "private-outcome-canary"},
    {"settings": {"secret-private-key": "private-setting-canary"}},
    {"continue_current_candidate": True}, {"continuation_build_id": "private-candidate-canary"},
    {"remote_binding": {"private-destination-key": "private-destination-canary"}},
])
def test_invalid_success_body_is_redacted_and_admits_no_work(api, changes):
    client, url, _, success, feedback, submit = api
    submitted = deepcopy(body())
    submitted.update(changes)
    response = client.post(url, json=submitted)
    assert response.status_code == 422
    success.assert_not_awaited()
    feedback.assert_not_awaited()
    submit.assert_not_awaited()
    rendered = json.dumps(response.json())
    for canary in ("private-extra-key", "private-value-canary", "private-source-canary",
                   "private-outcome-canary", "secret-private-key", "private-setting-canary",
                   "private-candidate-canary", "private-destination-key", "private-destination-canary", "private-task-canary"):
        assert canary not in rendered
    for detail in response.json()["detail"]:
        assert "input" not in detail and "ctx" not in detail


def test_unicode_task_limit_counts_codepoints(api):
    client, url, _, success, _, _ = api
    submitted = {**body(), "task": "\U0001f30c" * 3000}
    assert client.post(url, json=submitted).status_code == 200
    assert success.await_args.kwargs["task"] == submitted["task"]


def test_multiline_task_is_preserved_exactly(api):
    client, url, _, success, _, _ = api
    submitted = {**body(), "task": "Show my saved plan.\nInputs:\tTonight\r\n"}
    assert client.post(url, json=submitted).status_code == 200
    assert success.await_args.kwargs["task"] == submitted["task"]


def test_escaped_lone_surrogate_is_rejected_before_service_admission(api):
    client, url, _, success, _, _ = api
    submitted = {**body(), "task": "private-task-canary\ud800"}
    response = client.post(url, content=json.dumps(submitted), headers={"Content-Type": "application/json"})
    assert response.status_code == 422
    assert "private-task-canary" not in response.text
    success.assert_not_awaited()


def test_conflict_preserves_pending_request_without_retry_or_rebuild(api):
    client, url, _, success, feedback, submit = api
    success.side_effect = DraftConflict("Selected installation changed. Refresh its saved details.")
    response = client.post(url, json=body())
    assert response.status_code == 409
    assert success.await_count == 1
    feedback.assert_not_awaited()
    submit.assert_not_awaited()


def test_unexpected_error_does_not_echo_private_exception(api):
    client, url, _, success, _, _ = api
    success.side_effect = OSError("private-report-storage-canary")
    response = client.post(url, json=body())
    assert response.status_code == 502
    assert "private-report-storage-canary" not in response.text
    assert success.await_count == 1


def test_foreign_origin_is_rejected_before_success_admission(api):
    client, url, _, success, _, _ = api
    response = client.post(url, json=body(), headers={"Origin": "https://unrelated.example"})
    assert response.status_code == 403
    success.assert_not_awaited()


def test_success_is_not_a_get_action(api):
    client, url, _, success, _, _ = api
    # The inherited static GET fallback may mask the method error as 404.
    assert client.get(url).status_code in (404, 405)
    success.assert_not_awaited()
