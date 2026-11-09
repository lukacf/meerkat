"""Shipping action handlers extracted without global/native/server construction."""
import ast
from copy import deepcopy
import logging
from pathlib import Path
import uuid

from fastapi import FastAPI, HTTPException
from fastapi.responses import JSONResponse
from fastapi.testclient import TestClient
from pydantic import BaseModel, ConfigDict, Field, model_validator
import pytest

from launchkit.projects import DraftConflict
from launchkit.implementation import BuildSelectionUnavailable
from launchkit.remote_deployment import RemoteProductActionNotSent


@pytest.fixture
def api():
    calls = []
    class Service:
        async def action(self, *args, **kwargs):
            calls.append(("action", args, deepcopy(kwargs)))
            return {"deployment_id": args[1], "status": "starting"}
        async def action_request(self, *args):
            calls.append(("readback", args, {}))
            if args[-1] == "missing":
                raise LookupError("This deployment action request has not been recorded.")
            if args[-1] == "changed":
                raise DraftConflict("This action request UUID belongs to a different deployment action.")
            return {"action_request": {"phase": "pending", "action_request_id": args[-1]}}
    source = Path(__file__).resolve().parents[1] / "launchkit/server.py"
    tree = ast.parse(source.read_text())
    selected = {"guide_result", "DeploymentAction", "operate_deployment", "deployment_action_request"}
    body = [node for node in tree.body if isinstance(node, (ast.AsyncFunctionDef, ast.ClassDef)) and node.name in selected]
    assert len(body) == 4
    app = FastAPI()
    namespace = {"app": app, "deployment": Service(), "HTTPException": HTTPException,
                 "JSONResponse": JSONResponse, "BaseModel": BaseModel, "ConfigDict": ConfigDict,
                 "Field": Field, "model_validator": model_validator, "DraftConflict": DraftConflict,
                 "BuildSelectionUnavailable": BuildSelectionUnavailable,
                 "RemoteProductActionNotSent": RemoteProductActionNotSent,
                 "ArchitectUnavailable": type("Unavailable", (RuntimeError,), {}),
                 "InstalledFeedbackNotAdmitted": type("FeedbackNotAdmitted", (RuntimeError,), {}),
                 "InstalledSuccessNotAdmitted": type("SuccessNotAdmitted", (RuntimeError,), {}),
                 "LOG": logging.getLogger(__name__)}
    exec(compile(ast.Module(body=body, type_ignores=[]), str(source), "exec"), namespace)
    app.state.deployment = namespace["deployment"]
    return TestClient(app), calls


@pytest.mark.parametrize("action,update_id", [("restart", None), ("apply-update", "selected-update")])
def test_action_request_uuid_forwarding_and_legacy_omission(api, action, update_id):
    client, calls = api
    body = {"deployment_id": "installed", "action": action}
    options = {}
    if update_id is not None:
        body["update_id"] = update_id
        options["update_id"] = update_id
    assert client.post("/api/guide/owned/deployment/action", json=body).status_code == 200
    assert calls[-1] == ("action", ("owned", "installed", action), options)
    request_id = str(uuid.uuid4())
    body["action_request_id"] = request_id
    assert client.post("/api/guide/owned/deployment/action", json=body).status_code == 200
    assert calls[-1] == ("action", ("owned", "installed", action), {**options, "action_request_id": request_id})


@pytest.mark.parametrize("request_id", ["not-a-uuid", "", 12, True, "AAAAAAAA-AAAA-AAAA-AAAA-AAAAAAAAAAAA"])
def test_invalid_action_request_id_never_calls_deployment(api, request_id):
    client, calls = api
    result = client.post("/api/guide/owned/deployment/action", json={
        "deployment_id": "installed", "action": "restart", "action_request_id": request_id})
    assert result.status_code == 422 and calls == []


def test_exact_readback_is_get_only_and_preserves_missing_or_changed_refusal(api):
    client, calls = api
    request_id = str(uuid.uuid4())
    path = f"/api/guide/owned/deployment/action/installed/{request_id}"
    response = client.get(path)
    assert response.status_code == 200
    assert response.json() == {"action_request": {"phase": "pending", "action_request_id": request_id}}
    assert calls == [("readback", ("owned", "installed", request_id), {})]
    assert client.post(path, json={}).status_code == 405
    assert client.get("/api/guide/owned/deployment/action/installed/missing").status_code == 404
    assert client.get("/api/guide/owned/deployment/action/installed/changed").status_code == 409
    assert all(kind == "readback" for kind, _, _ in calls)


@pytest.mark.parametrize("error,status", [(RuntimeError, 502), (TimeoutError, 504)])
def test_action_failure_does_not_invent_progress_or_recommend_repeating_stop(api, monkeypatch, error, status):
    client, calls = api

    async def failed(*args, **kwargs):
        calls.append(("action", args, deepcopy(kwargs)))
        raise error("private-provider-payload-not-for-the-browser")

    monkeypatch.setattr(client.app.state.deployment, "action", failed)
    request_id = str(uuid.uuid4())
    response = client.post("/api/guide/owned/deployment/action", json={
        "deployment_id": "installed", "action": "stop", "action_request_id": request_id})
    assert response.status_code == status
    message = response.json()["detail"]
    assert "installation command" in message and "Check installation status" in message
    assert "setup agent" not in message and "retry" not in message.lower()
    assert "stopped" not in message and "private-provider" not in message
    assert calls == [("action", ("owned", "installed", "stop"), {"action_request_id": request_id})]


@pytest.mark.parametrize("action,resume", [("prepare-support", False), ("prepare-support", True),
                                          ("apply-support", False), ("apply-support", True)])
def test_exact_support_action_fields_reach_the_existing_owner(api, action, resume):
    client, calls = api
    request_id, revision = str(uuid.uuid4()), str(uuid.uuid4())
    body = {"deployment_id": "installed", "action": action, "action_request_id": request_id}
    fields = {"expected_service_digest": None, "revision": None, "descriptor_sha256": None}
    if action == "prepare-support":
        fields["expected_service_digest"] = "a" * 64
    else:
        fields.update(revision=revision, descriptor_sha256="b" * 64)
    body.update(fields)
    if resume:
        body["resume"] = True
    assert client.post("/api/guide/owned/deployment/action", json=body).status_code == 200
    assert calls == [("action", ("owned", "installed", action),
        {**fields, "action_request_id": request_id, **({"resume": True} if resume else {})})]


@pytest.mark.parametrize("changes", [
    {"action_request_id": None}, {"expected_service_digest": None},
    {"expected_service_digest": True}, {"expected_service_digest": "A" * 64},
    {"revision": "not-a-uuid"}, {"descriptor_sha256": "b" * 64},
    {"update_id": "other-source"}, {"resume": 1}, {"resume": "true"},
    {"source": "/arbitrary/host"}, {"action": "restart", "resume": True},
    {"action": "apply-support", "expected_service_digest": None},
])
def test_support_validation_refuses_incomplete_mixed_or_untyped_requests(api, changes):
    client, calls = api
    body = {"deployment_id": "installed", "action": "prepare-support",
        "action_request_id": str(uuid.uuid4()), "expected_service_digest": "a" * 64, **changes}
    assert client.post("/api/guide/owned/deployment/action", json=body).status_code == 422
    assert calls == []
