"""Offline typed replay API delegation; no lifespan or real worker is admitted."""
from dataclasses import replace
from types import SimpleNamespace
from unittest.mock import AsyncMock

import pytest
from fastapi.testclient import TestClient

from launchkit import server
from launchkit.architect import ArchitectUnavailable
from launchkit.projects import DraftConflict


GUIDE = "67f2f55d-8fd4-4d92-b999-14fb240e5730"
TRIAL = "073bd98c-35c2-4fae-a95a-b96c4cf67515"
ORIGINAL = "ddfc23ac-f3cf-4cc3-9fb4-bf71a8883b9d"
TARGET = "c29b7357-7991-4a7f-9656-155a3e9b0fe8"
ACTION = "d8377671-eccc-4f39-937a-4108a5e43594"
URL = f"/api/guide/{GUIDE}/rehearsal/{TRIAL}/replay"
BODY = {"version": 9, "original_build_id": ORIGINAL, "original_source_digest": "a" * 64,
        "build_id": TARGET, "source_digest": "b" * 64, "start_request_id": ACTION}


@pytest.fixture
def api(monkeypatch):
    service = SimpleNamespace(replay=AsyncMock())
    monkeypatch.setattr(server, "rehearsal", service)
    return TestClient(server.app), service


def test_replay_route_forwards_only_exact_bindings_and_preserves_host_result(api):
    client, service = api
    result = {"guide_id": GUIDE, "guide_version": 9, "rehearsal_id": ACTION,
              "start_request_id": ACTION, "start_request_complete": True, "current": False,
              "application_source": {"build_id": TARGET, "source_digest": "b" * 64},
              "trial_replay": {"original_rehearsal_id": TRIAL, "payload_sha256": "c" * 64}}
    service.replay.return_value = result
    response = client.post(URL, json=BODY)
    assert response.status_code == 202 and response.json() == result
    assert response.headers["cache-control"] == "no-store"
    service.replay.assert_awaited_once_with(GUIDE, 9, TRIAL, ORIGINAL, "a" * 64,
                                          TARGET, "b" * 64, ACTION)


@pytest.mark.parametrize("field,value", [
    ("version", True), ("version", "9"), ("version", 0),
    ("original_build_id", "../private"), ("original_build_id", 12),
    ("build_id", TARGET.upper()), ("build_id", TARGET.replace("-", "")),
    ("original_source_digest", "a" * 63), ("source_digest", "B" * 64),
    ("source_digest", 12), ("start_request_id", None), ("start_request_id", "unknown"),
])
def test_replay_route_rejects_coerced_or_invalid_bindings_before_admission(api, field, value):
    client, service = api
    response = client.post(URL, json={**BODY, field: value})
    assert response.status_code == 422
    service.replay.assert_not_awaited()


@pytest.mark.parametrize("field", list(BODY))
def test_replay_route_requires_every_binding(api, field):
    client, service = api
    response = client.post(URL, json={key: value for key, value in BODY.items() if key != field})
    assert response.status_code == 422
    service.replay.assert_not_awaited()


@pytest.mark.parametrize("trial", ["invalid", TRIAL.upper(), "../outside"])
def test_replay_route_rejects_noncanonical_original_trial(api, trial):
    client, service = api
    response = client.post(f"/api/guide/{GUIDE}/rehearsal/{trial}/replay", json=BODY)
    assert response.status_code in {404, 405, 422}
    service.replay.assert_not_awaited()


def test_replay_route_never_accepts_replacement_fixtures_or_echoes_unknown_private_keys(api):
    client, service = api
    secret = "private-form-value-and-key-sentinel"
    response = client.post(URL, json={**BODY, secret: {"fixtures": secret},
                                     "application_fixtures": {"token": secret}, "text": secret})
    assert response.status_code == 422 and secret not in response.text
    service.replay.assert_not_awaited()


def witness(status=409):
    from launchkit.rehearsal import ReplayNotAdmitted, replay_request_fingerprint
    payload = "c" * 64
    binding = replay_request_fingerprint(GUIDE, 9, TRIAL, ORIGINAL, "a" * 64,
                                         TARGET, "b" * 64, payload)
    return ReplayNotAdmitted(guide_id=GUIDE, guide_version=9, mode="application",
        start_request_id=ACTION, start_request_fingerprint=binding, http_status=status,
        original_rehearsal_id=TRIAL, original_build_id=ORIGINAL, original_source_digest="a" * 64,
        build_id=TARGET, source_digest="b" * 64, payload_sha256=payload)


@pytest.mark.parametrize("status,error_type", [(404, LookupError), (409, DraftConflict),
                                               (422, ValueError), (503, ArchitectUnavailable)])
def test_only_exact_durable_replay_rejection_keeps_its_http_status_and_witness(api, status, error_type):
    client, service = api
    error = error_type("This exact retry was not admitted.")
    error.trial_start_rejection = witness(status)
    service.replay.side_effect = error
    response = client.post(URL, json=BODY)
    assert response.status_code == status
    assert response.json() == {"detail": str(error), "start_rejection": error.trial_start_rejection.projection()}


@pytest.mark.parametrize("change", ["ordinary", "wrong_action", "wrong_source", "bad_hash", "unsupported_status"])
def test_unbound_rejection_and_ordinary_failure_do_not_claim_non_admission(api, change):
    client, service = api
    error = DraftConflict("The result of this retry is unknown.")
    if change != "ordinary":
        value = witness()
        if change == "wrong_action":
            value = replace(value, start_request_id=TRIAL)
        elif change == "wrong_source":
            value = replace(value, source_digest="d" * 64)
        elif change == "bad_hash":
            value = replace(value, start_request_fingerprint="f" * 64)
        else:
            value = replace(value, http_status=504)
        error.trial_start_rejection = value
    service.replay.side_effect = error
    response = client.post(URL, json=BODY)
    assert response.status_code == 409 and response.json() == {"detail": str(error)}


def test_connection_failure_cannot_claim_a_rejected_or_admitted_trial(api):
    client, service = api
    service.replay.side_effect = ConnectionError("Private worker process or fixture context")
    response = client.post(URL, json=BODY)
    assert response.status_code == 502
    assert "start_rejection" not in response.json() and "Private worker" not in response.text


def test_a_lost_response_retry_delegates_the_identical_action_without_inventing_new_inputs(api):
    client, service = api
    service.replay.side_effect = [ConnectionError("Lost response"), {"rehearsal_id": ACTION, "status": "queued"}]
    assert client.post(URL, json=BODY).status_code == 502
    assert client.post(URL, json=BODY).status_code == 202
    calls = service.replay.await_args_list
    assert len(calls) == 2 and calls[0] == calls[1]
    assert calls[0].args == (GUIDE, 9, TRIAL, ORIGINAL, "a" * 64, TARGET, "b" * 64, ACTION)
