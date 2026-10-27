"""Saved-request HTTP boundaries, without Toolkit processes or provider work."""
import asyncio
from copy import deepcopy
from types import SimpleNamespace
from unittest.mock import AsyncMock, Mock

from fastapi.testclient import TestClient
import pytest


GUIDE = "f66bcdde-8a79-40f2-bc2f-79b069d67313"
OTHER_GUIDE = "0682cc0c-1a25-4f21-a9d8-f5c27b559597"
VERSION = 7
BODY = {
    "version": VERSION,
    "manifest_sha256": "a" * 64,
    "request_id": "b" * 32,
    "request_sha256": "c" * 64,
    "host_fingerprint": "d" * 64,
    "audience_approved": True,
}
MANIFEST = b' \t{\r\n"schema_version": 1, "requests": []\r\n}\n '


@pytest.fixture
def api(monkeypatch):
    from launchkit import server

    states = {GUIDE: {"id": GUIDE, "version": VERSION}}

    async def read(service, identity):
        if identity not in states:
            raise LookupError("This saved setup could not be found.")
        return deepcopy(states[identity])

    service = SimpleNamespace(
        status=Mock(return_value={"guide_id": GUIDE, "status": "unavailable"}),
        inspect=AsyncMock(return_value={"guide_id": GUIDE, "status": "inspected"}),
        preview=AsyncMock(return_value={"guide_id": GUIDE, "status": "previewed"}),
        review=AsyncMock(return_value={"guide_id": GUIDE, "status": "reviewing"}),
    )
    guide_read = AsyncMock(side_effect=read)
    store_get = Mock(side_effect=lambda identity: deepcopy(states.get(identity)))
    store_operation = Mock(side_effect=lambda identity: deepcopy(states.get(identity, {}).get("operation")))
    monkeypatch.setattr(server, "toolkit_saved_requests", service)
    monkeypatch.setattr(server, "guide_operations", SimpleNamespace(read=guide_read, lock=asyncio.Lock()))
    monkeypatch.setattr(server.guide, "lock", asyncio.Lock())
    monkeypatch.setattr(server.guide.store, "get", store_get)
    monkeypatch.setattr(server.guide.store, "operation", store_operation)
    return SimpleNamespace(client=TestClient(server.app), server=server, service=service,
                           states=states, guide_read=guide_read, store_get=store_get,
                           store_operation=store_operation,
                           url=f"/api/guide/{GUIDE}/toolkit-saved")


def test_saved_status_reads_the_known_guide_before_service(api):
    response = api.client.get(api.url)
    assert response.status_code == 200
    api.guide_read.assert_awaited_once_with(api.server.guide, GUIDE)
    api.service.status.assert_called_once_with(GUIDE)


def test_preview_preserves_original_manifest_and_request_selection(api):
    submitted = {key: BODY[key] for key in ("version", "manifest_sha256", "request_id")}
    response = api.client.post(api.url + "/preview", json=submitted)
    assert response.status_code == 200
    api.guide_read.assert_awaited_once_with(api.server.guide, GUIDE)
    api.service.preview.assert_awaited_once_with(GUIDE, BODY["manifest_sha256"], BODY["request_id"])
    api.service.inspect.assert_not_awaited()
    api.service.review.assert_not_awaited()


def test_review_forwards_exact_guide_version_hashes_host_and_full_approval(api):
    submitted = deepcopy(BODY)
    response = api.client.post(api.url + "/review", json=submitted)
    assert response.status_code == 202
    api.guide_read.assert_awaited_once_with(api.server.guide, GUIDE)
    api.service.review.assert_awaited_once_with(
        GUIDE, VERSION, BODY["manifest_sha256"], BODY["request_id"], BODY["request_sha256"],
        host_fingerprint=BODY["host_fingerprint"], audience_approved=True)
    api.service.inspect.assert_not_awaited()
    api.service.preview.assert_not_awaited()
    assert submitted == BODY


@pytest.mark.parametrize("operation", ["manifest", "preview", "review"])
@pytest.mark.parametrize("state", [
    {"id": GUIDE, "version": VERSION + 1},
    {"id": GUIDE, "version": VERSION, "operation": {"status": "running"}},
])
def test_stale_or_running_guide_refuses_before_saved_request_work(api, operation, state):
    api.states[GUIDE] = state
    if operation == "manifest":
        response = api.client.post(api.url + f"/manifest?version={VERSION}", content=MANIFEST)
    else:
        payload = BODY if operation == "review" else {
            key: BODY[key] for key in ("version", "manifest_sha256", "request_id")}
        response = api.client.post(api.url + "/" + operation, json=payload)
    assert response.status_code == 409
    api.service.status.assert_not_called()
    api.service.inspect.assert_not_awaited()
    api.service.preview.assert_not_awaited()
    api.service.review.assert_not_awaited()


@pytest.mark.parametrize("changed_state", [
    {"id": GUIDE, "version": VERSION + 1},
    {"id": GUIDE, "version": VERSION, "operation": {"status": "running"}},
])
def test_other_tab_change_after_initial_read_refuses_locked_review_admission(api, changed_state):
    initial = deepcopy(api.states[GUIDE])

    async def read_before_other_tab_change(service, identity):
        api.states[identity] = deepcopy(changed_state)
        return initial

    api.guide_read.side_effect = read_before_other_tab_change
    response = api.client.post(api.url + "/review", json=BODY)
    assert response.status_code == 409
    api.guide_read.assert_awaited_once_with(api.server.guide, GUIDE)
    api.store_get.assert_called_once_with(GUIDE)
    api.store_operation.assert_called_once_with(GUIDE)
    api.service.status.assert_not_called()
    api.service.inspect.assert_not_awaited()
    api.service.preview.assert_not_awaited()
    api.service.review.assert_not_awaited()


def test_false_audience_approval_cannot_start_a_review(api):
    api.service.review.side_effect = api.server.ToolkitSavedRequestsError(
        "Approve sharing this saved request with the setup audience before reviewing it.")
    submitted = {**BODY, "audience_approved": False}
    response = api.client.post(api.url + "/review", json=submitted)
    assert response.status_code == 422
    # The route may refuse first or delegate the same explicit False to its owner.
    assert api.service.review.await_count in (0, 1)
    if api.service.review.await_count:
        assert api.service.review.await_args.kwargs["audience_approved"] is False
    api.service.inspect.assert_not_awaited()
    api.service.preview.assert_not_awaited()


@pytest.mark.parametrize("raw", [MANIFEST, MANIFEST + b" " * (64 * 1024 - len(MANIFEST))])
def test_manifest_upload_preserves_raw_bytes_through_the_inclusive_size_bound(api, raw):
    response = api.client.post(api.url + f"/manifest?version={VERSION}", content=raw,
                               headers={"Content-Type": "application/json"})
    assert response.status_code == 200
    api.service.inspect.assert_awaited_once_with(GUIDE, raw)
    api.service.preview.assert_not_awaited()
    api.service.review.assert_not_awaited()


@pytest.mark.parametrize("raw", [b"", b" " * (64 * 1024 + 1)])
def test_empty_or_oversized_manifest_refuses_before_owner_inspection(api, raw):
    response = api.client.post(api.url + f"/manifest?version={VERSION}", content=raw)
    assert response.status_code == 422
    api.service.inspect.assert_not_awaited()
    api.service.review.assert_not_awaited()


@pytest.mark.parametrize("operation", ["status", "manifest", "preview", "review"])
def test_unknown_guide_admits_no_saved_request_helper(api, operation):
    url = f"/api/guide/{OTHER_GUIDE}/toolkit-saved"
    if operation == "status":
        response = api.client.get(url)
    elif operation == "manifest":
        response = api.client.post(url + f"/manifest?version={VERSION}", content=MANIFEST)
    else:
        payload = BODY if operation == "review" else {
            key: BODY[key] for key in ("version", "manifest_sha256", "request_id")}
        response = api.client.post(url + "/" + operation, json=payload)
    assert response.status_code == 404
    api.service.status.assert_not_called()
    api.service.inspect.assert_not_awaited()
    api.service.preview.assert_not_awaited()
    api.service.review.assert_not_awaited()


@pytest.mark.parametrize("changes", [
    {"version": True}, {"version": "private-value-canary"},
    {"audience_approved": "private-value-canary"},
    {"manifest_sha256": "private-value-canary"},
    {"request_id": "private-value-canary"},
    {"request_sha256": "private-value-canary"},
    {"host_fingerprint": "private-value-canary"},
    {"private-key-canary": {"nested-private-key-canary": "private-value-canary"}},
])
def test_invalid_review_fields_do_not_echo_private_values_or_extra_keys(api, changes):
    response = api.client.post(api.url + "/review", json={**BODY, **changes})
    assert response.status_code == 422
    for canary in ("private-value-canary", "private-key-canary", "nested-private-key-canary"):
        assert canary not in response.text
    for detail in response.json()["detail"]:
        assert "input" not in detail and "ctx" not in detail
    api.guide_read.assert_not_awaited()
    api.service.review.assert_not_awaited()


def test_invalid_preview_and_upload_version_are_private_and_admit_no_work(api):
    preview = {key: BODY[key] for key in ("version", "manifest_sha256", "request_id")}
    response = api.client.post(api.url + "/preview", json={**preview,
        "private-extra-key-canary": "private-value-canary"})
    assert response.status_code == 422
    assert "private-extra-key-canary" not in response.text and "private-value-canary" not in response.text
    response = api.client.post(api.url + "/manifest?version=private-value-canary", content=MANIFEST)
    assert response.status_code == 422 and "private-value-canary" not in response.text
    api.guide_read.assert_not_awaited()
    api.service.inspect.assert_not_awaited()
    api.service.preview.assert_not_awaited()


def completed_review():
    return {
        "guide_id": GUIDE, "status": "review_required", "audience_approved": True,
        "manifest_sha256": BODY["manifest_sha256"],
        "selection": {"request_id": BODY["request_id"],
                      "request_sha256": BODY["request_sha256"], "version": VERSION},
        "review": {"status": "review_required", "scope": "successor_configuration_candidate",
                   "prior_request_sha256": BODY["request_sha256"], "family": "services",
                   "profile": "gmail", "values": {"token_env": "APPROVED_TOKEN_REFERENCE"},
                   "prior_operations": ["retired_operation"]},
    }


def test_evidence_contains_only_the_same_guide_approved_current_proposal(api):
    retained = completed_review()
    # Uploaded history and raw bytes are local preview data, not guide evidence.
    retained.update(request_text="UPLOADED_PRIVATE_REQUEST_CANARY",
                    request={"private-value-canary": "UPLOADED_PRIVATE_REQUEST_CANARY"},
                    current_operations=["retired_operation"], installed=True)
    api.service.status.return_value = retained
    evidence = api.server.saved_toolkit_review_evidence(GUIDE)
    assert evidence["guide_id"] == GUIDE
    assert evidence["scope"] == "saved_configuration_proposal_only"
    assert evidence["selection"] == retained["selection"]
    assert evidence["review"] == retained["review"]
    assert evidence["current_operations"] == [] and evidence["installed"] is False
    assert "request" not in evidence and "request_text" not in evidence
    assert "UPLOADED_PRIVATE_REQUEST_CANARY" not in str(evidence)
    api.service.status.assert_called_once_with(GUIDE)
    api.service.review.assert_not_awaited()


@pytest.mark.parametrize("changes", [
    {"status": "preview"}, {"status": "preparing"}, {"status": "unconfirmed"},
    {"status": "profile_unavailable"}, {"status": "unavailable"},
    {"audience_approved": False}, {"audience_approved": None},
    {"guide_id": OTHER_GUIDE},
    {"selection": None}, {"review": None},
    {"review": {**completed_review()["review"], "status": "preparation_required"}},
    {"review": {**completed_review()["review"], "scope": "historical_execution"}},
    {"review": {**completed_review()["review"], "prior_request_sha256": "e" * 64}},
])
def test_incomplete_unapproved_or_foreign_saved_request_is_not_guide_evidence(api, changes):
    retained = {**completed_review(), **changes,
                "request_text": "UPLOADED_PRIVATE_REQUEST_CANARY"}
    api.service.status.return_value = retained
    assert api.server.saved_toolkit_review_evidence(GUIDE) is None
    api.service.review.assert_not_awaited()


def test_no_uploaded_request_data_is_exposed_to_guide_before_audience_approval(api):
    api.service.status.return_value = {
        "guide_id": GUIDE, "status": "preview", "audience_approval_required": True,
        "manifest_sha256": BODY["manifest_sha256"],
        "selection": {"request_id": BODY["request_id"], "request_sha256": BODY["request_sha256"]},
        "request_text": "UPLOADED_PRIVATE_REQUEST_CANARY",
        "request": {"values": {"private-key-canary": "private-value-canary"}},
    }
    assert api.server.saved_toolkit_review_evidence(GUIDE) is None
    api.service.review.assert_not_awaited()
