"""Cloud checks admit only an explicit current source and local account selection."""
from copy import deepcopy
import json

from fastapi.testclient import TestClient
import pytest

from launchkit.projects import DraftConflict


@pytest.fixture
def api(monkeypatch):
    from launchkit import server
    calls = []

    class Preflight:
        async def options(self, guide_id, build_id):
            calls.append(("options", guide_id, build_id))
            return {"protocol": 1, "accounts": [], "cli": {"status": "blocked"},
                    "provisioning_verified": False}

        async def projects(self, guide_id, version, build_id, account):
            calls.append(("projects", guide_id, version, build_id, account))
            return {"protocol": 1, "account": account, "projects": [],
                    "provisioning_verified": False}

        async def check(self, guide_id, version, build_id, account, project):
            calls.append(("check", guide_id, version, build_id, account, project))
            if build_id == "stale-build":
                raise DraftConflict("The accepted application changed. Reload its cloud setup.")
            return {"protocol": 1, "account": account, "project": project,
                    "ready_for_plan": False, "provisioning_verified": False,
                    "checks": [{"id": "billing", "status": "blocked", "detail": "Enable billing for this project."}]}

    monkeypatch.setattr(server, "gcp_preflight", Preflight())
    return TestClient(server.app), calls


def selection():
    return {"version": 4, "build_id": "accepted-build", "account": "owner@example.test", "project": "owned-project"}


def test_account_discovery_requires_explicit_build_selection(api):
    client, calls = api
    url = "/api/guide/synthetic-guide/gcp-preflight/options"
    assert client.get(url).status_code == 422
    assert calls == []
    response = client.get(url, params={"build_id": "accepted-build"})
    assert response.status_code == 200
    assert response.json()["provisioning_verified"] is False
    assert calls == [("options", "synthetic-guide", "accepted-build")]


def test_project_discovery_and_check_keep_selected_account_and_build(api):
    client, calls = api
    submitted = selection()
    account = {key: value for key, value in submitted.items() if key != "project"}
    response = client.post("/api/guide/synthetic-guide/gcp-preflight/projects", json=account)
    assert response.status_code == 200
    response = client.post("/api/guide/synthetic-guide/gcp-preflight/check", json=submitted)
    assert response.status_code == 200
    assert response.json()["ready_for_plan"] is False
    assert response.json()["provisioning_verified"] is False
    assert calls == [("projects", "synthetic-guide", 4, "accepted-build", "owner@example.test"),
                     ("check", "synthetic-guide", 4, "accepted-build", "owner@example.test", "owned-project")]


@pytest.mark.parametrize("changes", [
    {"version": True}, {"version": "4"}, {"build_id": 12}, {"build_id": ""},
    {"account": "--impersonate-service-account=private-canary"},
    {"account": "owner\nprivate-canary"}, {"account": 12},
    {"project": "--project=private-canary"}, {"project": "owned-project private-canary"},
    {"project": True}, {"settings": {"private-key-canary": "private-value-canary"}},
    {"access_token": "private-token-canary"}, {"private-extra-key-canary": "private-value-canary"},
])
def test_invalid_or_private_inputs_never_admit_cloud_checks_or_echo_values(api, changes):
    client, calls = api
    submitted = deepcopy(selection())
    submitted.update(changes)
    response = client.post("/api/guide/synthetic-guide/gcp-preflight/check", json=submitted)
    assert response.status_code == 422
    assert calls == []
    rendered = json.dumps(response.json())
    for canary in ("private-canary", "private-key-canary", "private-value-canary", "private-token-canary", "private-extra-key-canary"):
        assert canary not in rendered


def test_a_changed_build_is_a_conflict_and_does_not_report_readiness(api):
    client, calls = api
    submitted = {**selection(), "build_id": "stale-build"}
    response = client.post("/api/guide/synthetic-guide/gcp-preflight/check", json=submitted)
    assert response.status_code == 409
    assert "ready_for_plan" not in response.json()
    assert len(calls) == 1


@pytest.mark.parametrize("path", ["projects", "check"])
def test_unrelated_web_origin_cannot_trigger_authenticated_cloud_reads(api, path):
    client, calls = api
    submitted = selection()
    if path == "projects":
        submitted.pop("project")
    response = client.post(f"/api/guide/synthetic-guide/gcp-preflight/{path}", json=submitted,
                           headers={"Origin": "https://unrelated.example"})
    assert response.status_code == 403
    assert calls == []


def test_project_discovery_cannot_smuggle_provisioning_or_credentials(api):
    client, calls = api
    submitted = {key: value for key, value in selection().items() if key != "project"}
    submitted.update({"provision": True, "credential_file": "private-credential-canary"})
    response = client.post("/api/guide/synthetic-guide/gcp-preflight/projects", json=submitted)
    assert response.status_code == 422
    assert calls == []
    assert "private-credential-canary" not in json.dumps(response.json())
