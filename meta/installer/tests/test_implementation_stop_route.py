"""The novice stop action must name the exact selected build and guide revision."""

from types import SimpleNamespace
from unittest.mock import AsyncMock

import pytest
from fastapi.testclient import TestClient

from launchkit import server
from launchkit.projects import DraftConflict


@pytest.fixture
def stop_api(monkeypatch):
    service = SimpleNamespace(request_stop=AsyncMock(), submit=AsyncMock())
    monkeypatch.setattr(server, "implementation", service)
    # No lifespan, native work or saved user store is admitted by these tests.
    return TestClient(server.app), service


def test_stop_forwards_exact_build_and_version_and_preserves_cleanup_state(stop_api):
    client, service = stop_api
    result = {"build_id": "selected-build", "status": "running", "stop_requested": True,
              "cleanup_pending": True, "ready_to_package": False}
    service.request_stop.return_value = result
    response = client.post("/api/guide/selected-guide/implementation/stop",
                           json={"version": 8, "build_id": "selected-build"})
    assert response.status_code == 202
    assert response.json() == result
    assert response.headers["cache-control"] == "no-store"
    service.request_stop.assert_awaited_once_with("selected-guide", 8, "selected-build")


def test_stale_stop_returns_conflict_without_claiming_worker_cleanup(stop_api):
    client, service = stop_api
    service.request_stop.side_effect = DraftConflict("The selected build changed. Refresh its status.")
    response = client.post("/api/guide/selected-guide/implementation/stop",
                           json={"version": 8, "build_id": "old-build"})
    assert response.status_code == 409
    assert "stop_requested" not in response.json()


@pytest.mark.parametrize("selected", [None, "displayed-failed-build"])
def test_build_fences_displayed_selection_even_when_no_checked_source_exists(stop_api, selected):
    client, service = stop_api
    service.submit.return_value = {"build_id": "new-build", "status": "queued"}
    response = client.post("/api/guide/selected-guide/implementation",
        json={"version": 8, "request": "Finish my saved work.", "previous_build_id": selected})
    assert response.status_code == 202
    service.submit.assert_awaited_once_with("selected-guide", 8, "Finish my saved work.",
        expected_selected_build_id=selected, refresh_framework=False)


@pytest.mark.parametrize("body", [
    {"version": 8}, {"version": 8, "build_id": ""},
    {"version": True, "build_id": "selected-build"},
    {"version": "8", "build_id": "selected-build"},
    {"version": 0, "build_id": "selected-build"},
    {"version": 8, "build_id": 123},
    {"version": 8, "build_id": "x" * 65},
    {"version": 8, "build_id": "selected-build", "force": True},
])
def test_stop_rejects_missing_or_ambiguous_target_before_delegation(stop_api, body):
    client, service = stop_api
    response = client.post("/api/guide/selected-guide/implementation/stop", json=body)
    assert response.status_code == 422
    service.request_stop.assert_not_awaited()


@pytest.mark.parametrize("refresh", [False, True])
def test_canonical_build_forwards_explicit_framework_refresh(stop_api, refresh):
    client, service = stop_api
    service.submit.return_value = {"build_id": "new-build", "status": "queued", "refresh_framework": refresh}
    response = client.post("/api/guide/selected-guide/implementation", json={
        "version": 8, "request": "Rebuild this application.", "previous_build_id": "selected-build",
        "refresh_framework": refresh})
    assert response.status_code == 202 and response.json()["refresh_framework"] is refresh
    service.submit.assert_awaited_once_with("selected-guide", 8, "Rebuild this application.",
        expected_selected_build_id="selected-build", refresh_framework=refresh)


@pytest.mark.parametrize("refresh", [None, 0, 1, "true", "false", [], {}])
def test_canonical_build_rejects_ambiguous_framework_refresh(stop_api, refresh):
    client, service = stop_api
    response = client.post("/api/guide/selected-guide/implementation", json={
        "version": 8, "previous_build_id": "selected-build", "refresh_framework": refresh})
    assert response.status_code == 422
    service.submit.assert_not_awaited()
