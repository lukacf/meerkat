"""Typed GCP wizard delegation using inert service doubles only."""
from copy import deepcopy
import json
from types import SimpleNamespace

from fastapi.testclient import TestClient
import pytest

from launchkit.gcp_access import operation_uuid
from launchkit.projects import DraftConflict

GUIDE = "6062447d-ead4-4d31-a1e1-59e940a04158"
OPERATION = "c0610fde-14c1-469c-b0b1-01185b21985a"
OTHER_OPERATION = "a3448c55-c65f-4bd2-a3a9-31bf24d77b13"
BASE = "/api/guide/" + GUIDE


def selection():
    return {"version": 4, "build_id": "accepted-build", "account": "operator@example.test", "project": "mobkit-cloud-project"}


def cloud_receipt(operation=OPERATION):
    return {"protocol": 1, "operation_id": operation, "guide_id": GUIDE,
            "status": "vm-ready", "vm_verified": True, "application_installed": False,
            "binding": {"guide_id": GUIDE, "guide_version": 4, "build_id": "accepted-build", "package_sha256": "a" * 64},
            "destination": {"project": "mobkit-cloud-project", "zone": "europe-west1-b", "instance": "atlas-operations"},
            "endpoint": {"operation_id": operation, "instance_id": "123456789", "host": "compute.123456789"}}


@pytest.fixture
def services(monkeypatch, tmp_path):
    # Constructors remain isolated even if this is the first server import in a
    # checkout. No lifespan or capability probe is admitted by this fixture.
    monkeypatch.setenv("MOBKIT_LAUNCH_HOME", str(tmp_path / "synthetic-studio"))
    from launchkit import server
    calls = []
    cloud = cloud_receipt()
    behavior = {"create_error": None, "status_error": None, "resume_error": None, "connect_error": None, "remove_error": None}

    class Preflight:
        async def check_guided(self, identity, version, build_id, account, project):
            calls.append(("check-guided", identity, version, build_id, account, project))
            if build_id == "stale-build": raise DraftConflict("The accepted package changed. Check the current source again.")
            return {"protocol": 1, "guided_supported": True, "provisioning_verified": False,
                    "account": account, "project": project}

    class Coordinator:
        async def create(self, identity, version, build_id, account, project, request_id):
            calls.append(("create", identity, version, build_id, account, project, request_id))
            if behavior["create_error"]: raise behavior["create_error"]
            return {"protocol": 1, "operation_id": request_id, "guide_id": identity,
                    "status": "provisioning", "application_installed": False}

        async def status(self, identity, operation_id=None):
            if operation_id is not None: operation_uuid(operation_id)
            calls.append(("status", identity, operation_id))
            if behavior["status_error"]: raise behavior["status_error"]
            return deepcopy(cloud) if operation_id in {None, cloud["operation_id"]} else None

        async def resume(self, identity, operation_id):
            operation_uuid(operation_id)
            calls.append(("resume", identity, operation_id))
            if behavior["resume_error"]: raise behavior["resume_error"]
            return {"protocol": 1, "guide_id": identity, "operation_id": operation_id,
                    "status": "provisioning", "application_installed": False}

        async def remove(self, identity, operation_id, request_id, acknowledge_data_loss):
            operation_uuid(operation_id)
            operation_uuid(request_id)
            calls.append(("remove", identity, operation_id, request_id, acknowledge_data_loss))
            if behavior["remove_error"]: raise behavior["remove_error"]
            return {**deepcopy(cloud), "status": "removing", "vm_verified": False,
                    "removal": {"request_id": request_id, "acknowledge_data_loss": True,
                                "resources": {kind: {"status": "waiting"} for kind in ("vm", "disk", "firewall")}}}

    class Remote:
        product_control_selection = None
        _product_control_selection = None

        async def connect_cloud(self, identity, version, build_id, selected, *, product_owner=None):
            assert product_owner is None, "No installed product owner is selected by this offline route fixture"
            calls.append(("connect-cloud", identity, version, build_id, deepcopy(selected)))
            if behavior["connect_error"]: raise behavior["connect_error"]
            if selected is None: raise DraftConflict("The original cloud computer receipt is required.")
            if selected.get("status") != "vm-ready": raise DraftConflict("The cloud computer is still being prepared.")
            return {"protocol": 1, "target": "gcp", "cloud_operation_id": selected["operation_id"],
                    "status": "connecting", "current": True, "application_installed": False}

    monkeypatch.setattr(server, "gcp_preflight", Preflight())
    monkeypatch.setattr(server, "gcp_provisioning", Coordinator())
    monkeypatch.setattr(server, "remote_deployment", Remote())
    client = TestClient(server.app)
    yield SimpleNamespace(server=server, client=client, calls=calls, cloud=cloud, behavior=behavior)
    client.close()


def test_check_guided_passes_only_exact_source_and_selected_account(services):
    response = services.client.post(BASE + "/gcp-preflight/check-guided", json=selection())
    assert response.status_code == 200
    assert response.json()["guided_supported"] is True and response.json()["provisioning_verified"] is False
    assert services.calls == [("check-guided", GUIDE, 4, "accepted-build", "operator@example.test", "mobkit-cloud-project")]


def test_creation_response_is_owned_admission_and_same_request_id_survives_http_retry(services):
    body = {**selection(), "request_id": OPERATION}
    first = services.client.post(BASE + "/gcp-deployment/create", json=body)
    second = services.client.post(BASE + "/gcp-deployment/create", json=deepcopy(body))
    assert first.status_code == second.status_code == 202
    assert first.json() == second.json()
    assert first.json()["operation_id"] == OPERATION and first.json()["application_installed"] is False
    expected = ("create", GUIDE, 4, "accepted-build", "operator@example.test", "mobkit-cloud-project", OPERATION)
    assert services.calls == [expected, expected]


def test_status_and_resume_preserve_requested_operation_without_creating_another(services):
    assert services.client.get(BASE + "/gcp-deployment").json() == services.cloud
    assert services.client.get(BASE + "/gcp-deployment", params={"operation_id": OPERATION}).json() == services.cloud
    resumed = services.client.post(BASE + "/gcp-deployment/resume", json={"operation_id": OPERATION})
    assert resumed.status_code == 202 and resumed.json()["operation_id"] == OPERATION
    assert services.calls == [("status", GUIDE, None), ("status", GUIDE, OPERATION), ("resume", GUIDE, OPERATION)]


def test_connect_uses_only_requested_saved_operation_and_forwards_exact_receipt(services):
    body = {"version": 4, "build_id": "accepted-build", "operation_id": OPERATION}
    response = services.client.post(BASE + "/gcp-deployment/connect", json=body)
    assert response.status_code == 202 and response.json()["cloud_operation_id"] == OPERATION
    assert response.json()["status"] == "connecting" and response.json()["application_installed"] is False
    assert services.calls == [("status", GUIDE, OPERATION), ("connect-cloud", GUIDE, 4, "accepted-build", services.cloud)]


def test_connect_missing_operation_cannot_fall_back_to_latest_vm(services):
    response = services.client.post(BASE + "/gcp-deployment/connect", json={
        "version": 4, "build_id": "accepted-build", "operation_id": OTHER_OPERATION})
    assert response.status_code == 409
    assert services.calls == [("status", GUIDE, OTHER_OPERATION), ("connect-cloud", GUIDE, 4, "accepted-build", None)]
    assert "cloud_operation_id" not in response.json()


def test_removal_selects_original_operation_without_current_source_and_reuses_request(services):
    body = {"operation_id": OPERATION, "request_id": OTHER_OPERATION, "acknowledge_data_loss": True}
    first = services.client.post(BASE + "/gcp-deployment/remove", json=body)
    again = services.client.post(BASE + "/gcp-deployment/remove", json=body)
    assert first.status_code == again.status_code == 202
    assert first.json() == again.json()
    assert first.json()["removal"]["request_id"] == OTHER_OPERATION
    assert services.calls == [("remove", GUIDE, OPERATION, OTHER_OPERATION, True)] * 2


@pytest.mark.parametrize("acknowledgement", [None, False, 0, 1, "true", "false", {}])
def test_removal_requires_explicit_boolean_data_loss_confirmation_before_admission(services, acknowledgement):
    body = {"operation_id": OPERATION, "request_id": OTHER_OPERATION, "acknowledge_data_loss": acknowledgement}
    response = services.client.post(BASE + "/gcp-deployment/remove", json=body)
    assert response.status_code == 422 and services.calls == []


@pytest.mark.parametrize("changed", [{"operation_id": OPERATION.upper()}, {"request_id": "private-request-canary"},
    {"settings": {"private-field-canary": "private-value-canary"}}, {"project": "private-project-canary"},
    {"version": 4}, {"endpoint": {"host": "private-host-canary"}}])
def test_removal_rejects_redirected_or_private_inputs_without_echo(services, changed):
    body = {"operation_id": OPERATION, "request_id": OTHER_OPERATION, "acknowledge_data_loss": True, **changed}
    response = services.client.post(BASE + "/gcp-deployment/remove", json=body)
    assert response.status_code == 422 and services.calls == []
    assert not any(canary in json.dumps(response.json()) for canary in (
        "private-request-canary", "private-field-canary", "private-value-canary", "private-project-canary", "private-host-canary"))


def test_removal_unavailable_custody_does_not_return_success(services):
    services.behavior["remove_error"] = DraftConflict("The original cloud resources could not be verified.")
    response = services.client.post(BASE + "/gcp-deployment/remove", json={
        "operation_id": OPERATION, "request_id": OTHER_OPERATION, "acknowledge_data_loss": True})
    assert response.status_code == 409 and "removal" not in response.json()


def test_unrelated_origin_cannot_admit_resource_removal(services):
    response = services.client.post(BASE + "/gcp-deployment/remove", json={
        "operation_id": OPERATION, "request_id": OTHER_OPERATION, "acknowledge_data_loss": True},
        headers={"Origin": "https://foreign.example"})
    assert response.status_code == 403 and services.calls == []


@pytest.mark.parametrize("path", ["create", "resume", "connect"])
@pytest.mark.parametrize("extra", [{"settings": {"private-key-canary": "private-value-canary"}},
    {"private-extra-key-canary": "private-value-canary"}, {"access_token": "private-token-canary"},
    {"endpoint": {"host": "private-host-canary", "operation_id": OTHER_OPERATION}}])
def test_private_or_routing_inputs_are_rejected_without_admission_or_echo(services, path, extra):
    body = ({**selection(), "request_id": OPERATION} if path == "create" else {"operation_id": OPERATION})
    if path == "connect": body.update(version=4, build_id="accepted-build")
    body.update(deepcopy(extra))
    response = services.client.post(BASE + "/gcp-deployment/" + path, json=body)
    assert response.status_code == 422 and services.calls == []
    assert not any(canary in json.dumps(response.json()) for canary in (
        "private-key-canary", "private-value-canary", "private-extra-key-canary", "private-token-canary", "private-host-canary"))


@pytest.mark.parametrize("path", ["create", "check-guided", "connect"])
@pytest.mark.parametrize("changed", [{"version": True}, {"version": "4"}, {"build_id": 42}, {"build_id": ""}])
def test_current_source_fields_are_strict_before_service_admission(services, path, changed):
    body = {**selection(), "request_id": OPERATION} if path == "create" else selection()
    if path == "connect": body = {"version": 4, "build_id": "accepted-build", "operation_id": OPERATION}
    body.update(changed)
    url = BASE + ("/gcp-preflight/check-guided" if path == "check-guided" else "/gcp-deployment/" + path)
    response = services.client.post(url, json=body)
    assert response.status_code == 422 and services.calls == []


@pytest.mark.parametrize("changed", [{"account": "--private-account-canary"}, {"account": "owner\nprivate-account-canary"},
    {"project": "--private-project-canary"}, {"project": "bad project private-project-canary"},
    {"request_id": "private-request-canary"}, {"request_id": 3}])
def test_creation_account_project_and_request_id_cannot_be_untyped_or_command_fragments(services, changed):
    body = {**selection(), "request_id": OPERATION, **changed}
    response = services.client.post(BASE + "/gcp-deployment/create", json=body)
    assert response.status_code == 422 and services.calls == []
    assert not any(word in json.dumps(response.json()) for word in ("private-account-canary", "private-project-canary", "private-request-canary"))


@pytest.mark.parametrize("path", ["resume", "connect"])
@pytest.mark.parametrize("operation", [None, 3, "PRIVATE-operation-CANARY", OPERATION.upper()])
def test_invalid_operation_is_controlled_and_never_admitted(services, path, operation):
    body = {"operation_id": operation}
    if path == "connect": body.update(version=4, build_id="accepted-build")
    response = services.client.post(BASE + "/gcp-deployment/" + path, json=body)
    assert response.status_code == 422 and services.calls == []
    assert "PRIVATE-operation-CANARY" not in json.dumps(response.json())


def test_status_invalid_operation_is_controlled_without_raw_input_echo(services):
    response = services.client.get(BASE + "/gcp-deployment", params={"operation_id": "PRIVATE-operation-CANARY"})
    assert response.status_code == 422 and services.calls == []
    assert "PRIVATE-operation-CANARY" not in json.dumps(response.json())


@pytest.mark.parametrize("path", ["check-guided", "create", "resume", "connect"])
def test_changed_source_or_custody_is_a_conflict_not_a_success_receipt(services, path):
    if path == "check-guided":
        body = {**selection(), "build_id": "stale-build"}; url = BASE + "/gcp-preflight/check-guided"
    elif path == "create":
        body = {**selection(), "request_id": OPERATION}; url = BASE + "/gcp-deployment/create"
        services.behavior["create_error"] = DraftConflict("The accepted cloud source changed.")
    elif path == "resume":
        body = {"operation_id": OPERATION}; url = BASE + "/gcp-deployment/resume"
        services.behavior["resume_error"] = DraftConflict("The owned resource has changed.")
    else:
        body = {"version": 4, "build_id": "accepted-build", "operation_id": OPERATION}; url = BASE + "/gcp-deployment/connect"
        services.behavior["status_error"] = DraftConflict("The owned operation is unavailable.")
    response = services.client.post(url, json=body)
    assert response.status_code == 409
    assert not any(key in response.json() for key in ("vm_verified", "operation_id", "current", "guided_supported"))
    if path == "connect": assert not any(call[0] == "connect-cloud" for call in services.calls)


@pytest.mark.parametrize("path", ["create", "resume", "connect", "check-guided"])
def test_unrelated_origin_cannot_admit_cloud_actions(services, path):
    if path == "create": body = {**selection(), "request_id": OPERATION}
    elif path == "resume": body = {"operation_id": OPERATION}
    elif path == "connect": body = {"version": 4, "build_id": "accepted-build", "operation_id": OPERATION}
    else: body = selection()
    url = BASE + ("/gcp-preflight/check-guided" if path == "check-guided" else "/gcp-deployment/" + path)
    response = services.client.post(url, json=body, headers={"Origin": "https://foreign.example"})
    assert response.status_code == 403 and services.calls == []


@pytest.mark.parametrize("failed", [None, "gcp_provisioning"])
async def test_lifespan_stops_coordinator_before_preflight_and_continues_all_owned_cleanup(services, monkeypatch, failed):
    server = services.server
    owners = ["guide_operations", "visualization_operations", "visualization_reviews", "visual_review_runtime",
              "remote_deployment", "gcp_provisioning", "gcp_preflight", "deployment", "rehearsal", "implementation", "architect"]
    observed = []
    error = RuntimeError("Synthetic owned cloud cleanup unavailable")
    for name in owners:
        async def shutdown(owner=name):
            observed.append(owner)
            if owner == failed: raise error
        fake = SimpleNamespace(shutdown=shutdown)
        if name == "visual_review_runtime":
            async def reconcile(): observed.append("reconcile")
            fake.reconcile_terminated = reconcile
        monkeypatch.setattr(server, name, fake)
    async def run():
        async with server.lifespan(server.app):
            assert observed == ["reconcile"]
    if failed:
        with pytest.raises(ExceptionGroup) as raised: await run()
        assert raised.value.exceptions == (error,)
    else: await run()
    assert observed == ["reconcile", *owners]
    assert observed.index("remote_deployment") < observed.index("gcp_provisioning") < observed.index("gcp_preflight")
