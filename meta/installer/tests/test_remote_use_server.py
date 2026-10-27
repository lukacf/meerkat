"""First-use routes preserve explicit remote ownership and private reports."""
from copy import deepcopy
import json
import uuid

from fastapi.testclient import TestClient
import pytest


@pytest.fixture
def api(monkeypatch):
    from launchkit import server
    calls = []

    async def use(guide_id, deployment_id):
        calls.append(("use", guide_id, deployment_id))
        return {"scope": "frozen-remote-package-source", "application_outcomes_verified": False}

    async def feedback(*args, **kwargs):
        calls.append(("feedback", args, kwargs))
        return {"status": "queued", "build_id": "new-candidate"}

    monkeypatch.setattr(server.remote_deployment, "use", use)
    monkeypatch.setattr(server.remote_deployment, "feedback", feedback)
    return TestClient(server.app), calls, str(uuid.uuid4())


def body():
    return {"version": 4, "remote_binding": {"deployment_id": str(uuid.uuid4())},
        "continuation_build_id": str(uuid.uuid4()), "continuation_source_digest": "a" * 64,
        "outcome": "blocked", "request": "Make the sign-in instructions usable.",
        "task": "Prepare a summary of this week's requests.", "continue_current_candidate": True}


def test_use_requires_an_explicit_remote_owner_and_only_reads(api):
    client, calls, guide_id = api
    url = f"/api/guide/{guide_id}/remote-deployment/use"
    assert client.get(url).status_code == 422
    assert calls == []
    deployment_id = str(uuid.uuid4())
    response = client.get(url, params={"deployment_id": deployment_id})
    assert response.status_code == 200
    assert response.json()["application_outcomes_verified"] is False
    assert calls == [("use", guide_id, deployment_id)]


def test_feedback_preserves_exact_owner_and_human_report_for_builder(api):
    client, calls, guide_id = api
    submitted = body()
    response = client.post(f"/api/guide/{guide_id}/remote-deployment/feedback", json=submitted)
    assert response.status_code == 202
    assert response.json() == {"status": "queued", "build_id": "new-candidate"}
    assert calls == [("feedback", (guide_id, 4, submitted["remote_binding"],
        submitted["continuation_build_id"], submitted["continuation_source_digest"]),
        {key: submitted[key] for key in ("request", "outcome", "task", "continue_current_candidate")})]


@pytest.mark.parametrize("changes", [
    {"request": 42}, {"task": 42}, {"outcome": "private-outcome-canary"},
    {"continue_current_candidate": "true"}, {"version": True},
    {"remote_binding": "private-binding-canary"}, {"remote_binding": {}},
    {"extra-private-key": "private-value-canary"},
    {"continuation_source_digest": "private-source-canary"},
    {"settings": {"secret-private-key": "private-setting-canary"}},
])
def test_invalid_feedback_never_echoes_private_report_or_admits_work(api, changes):
    client, calls, guide_id = api
    submitted = deepcopy(body())
    submitted.update(changes)
    response = client.post(f"/api/guide/{guide_id}/remote-deployment/feedback", json=submitted)
    assert response.status_code == 422
    assert calls == []
    rendered = json.dumps(response.json())
    for canary in ("private-outcome-canary", "private-binding-canary", "extra-private-key",
                   "private-value-canary", "private-source-canary", "secret-private-key",
                   "private-setting-canary"):
        assert canary not in rendered
