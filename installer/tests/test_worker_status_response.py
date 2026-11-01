"""A saved-selection refusal stays a typed HTTP response during polling."""
from types import SimpleNamespace
from unittest.mock import AsyncMock

from fastapi.testclient import TestClient
import pytest

from launchkit.implementation import BuildSelectionUnavailable


@pytest.mark.parametrize("kind", ["implementation", "rehearsal"])
def test_worker_status_preserves_selection_refusal(kind, monkeypatch):
    from launchkit import server
    service = SimpleNamespace(status=AsyncMock(side_effect=BuildSelectionUnavailable("Saved selection unavailable.")))
    monkeypatch.setattr(server, kind, service)
    console = AsyncMock(side_effect=AssertionError("No console lookup after a refusal"))
    monkeypatch.setattr(server, "browser_console_url", console)
    response = TestClient(server.app).get(f"/api/guide/retained-guide/{kind}")
    assert response.status_code == 409
    assert response.json() == {"detail": "Saved selection unavailable.", "code": "build_selection_unavailable"}
    assert response.headers["cache-control"] == "no-store"
    console.assert_not_awaited()


@pytest.mark.parametrize("kind", ["implementation", "rehearsal"])
def test_worker_status_still_projects_successful_receipt(kind, monkeypatch):
    from launchkit import server
    receipt = {"status": "running", "build_id": "existing-build"}
    monkeypatch.setattr(server, kind, SimpleNamespace(status=AsyncMock(return_value=receipt)))
    monkeypatch.setattr(server, "browser_console_url", AsyncMock(return_value="/api/owned-console/existing"))
    response = TestClient(server.app).get(f"/api/guide/retained-guide/{kind}")
    assert response.status_code == 200
    assert response.json() == {**receipt, "browser_console_url": "/api/owned-console/existing"}
