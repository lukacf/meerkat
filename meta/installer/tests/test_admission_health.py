"""Admission health is an observation, never permission to publish an agent."""
from copy import deepcopy

import pytest

from launchkit.deployment_worker import readiness


def pending_payload():
    return {"pid": 42, "console_url": "http://127.0.0.1:18088/console",
        "bootstrap": {"complete": True, "ready": False, "counts": {"broken": 1},
            "identities": {"lead:main": {"state": "active"},
                "project:one": {"state": "broken", "error": "private native detail",
                    "restore": {"stage": "refused", "code": -32603, "kind": "held_completion_owed"}}}},
        "admission": {"admitting": [], "owed": ["project:one"]}}


def test_owned_owed_bootstrap_is_pending_with_original_counts():
    payload = pending_payload()
    before = deepcopy(payload)
    with pytest.raises(Exception) as caught:
        readiness(payload, {"agents": [{"identity": "lead:main"}]}, 42, 18088)
    assert type(caught.value).__name__ == "NativeAdmissionPending"
    assert caught.value.bootstrap == payload["bootstrap"]
    assert caught.value.admission == payload["admission"]
    assert payload == before
    assert "private native detail" not in str(caught.value)


def test_an_admitting_row_prevents_ready_even_when_native_roster_is_ready():
    payload = pending_payload()
    payload["bootstrap"]["identities"].pop("project:one")
    payload["bootstrap"].update(ready=True, counts={"broken": 0})
    payload["admission"] = {"admitting": ["project:one"], "owed": []}
    with pytest.raises(Exception) as caught:
        readiness(payload, {"agents": [{"identity": "lead:main"}]}, 42, 18088)
    assert type(caught.value).__name__ == "NativeAdmissionPending"


def test_lazy_owed_identity_stays_owed_and_pending_without_rewriting_native_health():
    payload = pending_payload()
    payload["bootstrap"]["identities"]["project:one"] = {"state": "dormant"}
    payload["bootstrap"].update(ready=True, counts={"broken": 0})
    before = deepcopy(payload)
    with pytest.raises(Exception) as caught:
        readiness(payload, {"agents": [{"identity": "lead:main"}]}, 42, 18088)
    assert type(caught.value).__name__ == "NativeAdmissionPending"
    assert caught.value.admission == {"admitting": [], "owed": ["project:one"]}
    assert caught.value.bootstrap == before["bootstrap"] and payload == before


@pytest.mark.parametrize("state", ["active", "warming"])
def test_extra_owed_identity_must_be_dormant(state):
    payload = pending_payload()
    payload["bootstrap"]["identities"]["project:two"] = {"state": state}
    payload["admission"]["owed"].append("project:two")
    with pytest.raises(ValueError):
        readiness(payload, {"agents": [{"identity": "lead:main"}]}, 42, 18088)


@pytest.mark.parametrize("fault", ["ordinary_broken", "error_text_only", "wrong_code",
    "wrong_stage", "missing_owed", "unrelated_owed", "overlap", "seed", "duplicate",
    "counts_bool", "counts_mismatch", "wrong_pid", "wrong_console", "malformed_console",
    "malformed_state", "malformed"])
def test_bad_pending_evidence_does_not_hide_failure(fault):
    payload = pending_payload()
    row = payload["bootstrap"]["identities"]["project:one"]
    if fault == "ordinary_broken":
        row["restore"]["kind"] = "different_failure"
    elif fault == "error_text_only":
        row.pop("restore")
        row["error"] = "held_completion_owed"
    elif fault == "wrong_code":
        row["restore"]["code"] = -32001
    elif fault == "wrong_stage":
        row["restore"]["stage"] = "resumed"
    elif fault == "missing_owed":
        payload["admission"]["owed"] = []
    elif fault == "unrelated_owed":
        payload["admission"]["owed"] = ["project:two"]
    elif fault == "overlap":
        payload["admission"]["admitting"] = ["project:one"]
    elif fault == "seed":
        payload["admission"]["admitting"] = ["lead:main"]
    elif fault == "duplicate":
        payload["admission"]["owed"] *= 2
    elif fault == "counts_bool":
        payload["bootstrap"]["counts"]["broken"] = True
    elif fault == "counts_mismatch":
        payload["bootstrap"]["counts"]["broken"] = 0
    elif fault == "wrong_pid":
        payload["pid"] = 43
    elif fault == "wrong_console":
        payload["console_url"] = "http://outside.example:18088/console"
    elif fault == "malformed_console":
        payload["console_url"] = {"url": "http://127.0.0.1:18088/console"}
    elif fault == "malformed_state":
        row["state"] = ["broken"]
    else:
        payload["admission"] = {"owed": "project:one"}
    with pytest.raises(ValueError):
        readiness(payload, {"agents": [{"identity": "lead:main"}]}, 42, 18088)


@pytest.mark.parametrize("broken", [False, True, "0", 0.0, None, [], {}])
def test_ready_requires_an_actual_integer_zero_broken_count(broken):
    payload = pending_payload()
    payload.pop("admission")
    payload["bootstrap"]["identities"]["project:one"] = {"state": "active"}
    payload["bootstrap"].update(ready=True, counts={"broken": broken})
    with pytest.raises(ValueError):
        readiness(payload, {"agents": [{"identity": "lead:main"}]}, 42, 18088)
