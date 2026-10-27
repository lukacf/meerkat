"""Actual private pipes with inert replies, not native approval or grant evidence."""
from concurrent.futures import ThreadPoolExecutor
import dataclasses
import pytest

from meerkat_toolkit.host_control import HostControlError, RemoteControllerLogin
from test_host_control import pipes, receive, respond, handle, observation
from test_host_submit import submission, finish


def begin(client, **kwargs):
    return client.begin_remote_controller_review(*submission(), RemoteControllerLogin("paired-login", 1900000000000), **kwargs)


def selected(route):
    return {"state": "selected", "route": route, "purpose": "controller", "lineage": [{
        "root_authority": {"kind": "human", "id": "operator", "qualification": {"kind": "fixture"}},
        "authority_namespace": "fixture-owner", "authority_generation": 1,
        "authority_incarnation": "00000000-0000-4000-8000-000000000001", "grant_id": "fixture-child", "issued_revision": 1,
    }]}


def control_reply(request, state, **extra):
    return {"kind": "controller_review", "review_id": request["command"]["review_id"], "status": {"state": state, **extra}}


def test_remote_controls_do_not_wait_for_or_invent_the_final_native_outcome():
    client, reader, writer = pipes()
    try:
        review = begin(client)
        start = receive(reader)
        assert review.review_id == start["id"] == 1
        assert start["command"]["kind"] == "review_remote_controller_parent"
        assert start["command"]["login"] == {"scope_id": "paired-login", "expires_at_ms": 1900000000000}
        assert start["command"]["submission"] == {"original": dataclasses.asdict(submission()[0]), "content": submission()[1], "transport_authentication": dataclasses.asdict(submission()[2])}
        with ThreadPoolExecutor(max_workers=2) as executor:
            final = executor.submit(review.result)
            observing = executor.submit(client.observe_controller_review, review.review_id)
            request = receive(reader)
            prompt = {"kind": "approval", "approval_id": "approval-1", "summary": "Review"}
            respond(writer, request, control_reply(request, "prompt", prompt=prompt))
            assert observing.result(timeout=2) == {"state": "prompt", "prompt": prompt}
            answering = executor.submit(client.answer_controller_review, review.review_id, "approval-1", "approve")
            request = receive(reader)
            assert request["id"] == 3
            assert request["command"] == {"kind": "answer_controller_review", "review_id": 1, "approval_id": "approval-1", "decision": "approve"}
            respond(writer, request, control_reply(request, "answer_forwarded"))
            assert answering.result(timeout=2) == {"state": "answer_forwarded"}
            assert not final.done()
            outcome = selected(submission()[0].route)
            respond(writer, start, {"kind": "controller_parent_reviewed", "outcome": outcome})
            first = final.result(timeout=2)
            assert first == outcome
            first["lineage"].clear()
            assert review.result() == outcome  # Cached exact observation, no resend.
    finally:
        finish(client, reader, writer)


def test_retiring_a_review_does_not_close_shared_host():
    client, reader, writer = pipes()
    try:
        review = begin(client)
        start = receive(reader)
        with ThreadPoolExecutor(max_workers=1) as executor:
            retiring = executor.submit(client.retire_controller_review, review.review_id)
            request = receive(reader)
            respond(writer, request, control_reply(request, "retired"))
            assert retiring.result(timeout=2) == {"state": "retired"}
            respond(writer, start, {"kind": "controller_parent_reviewed", "outcome": {"state": "unavailable"}})
            assert review.result() == {"state": "unavailable"}
            close_handle = handle()
            observing = executor.submit(client.observe_close, close_handle)
            request = receive(reader)
            respond(writer, request, {"kind": "close", "observation": observation(close_handle, "closing")})
            assert observing.result(timeout=2).state.value == "closing"
    finally:
        finish(client, reader, writer)


def test_timed_out_review_is_never_resent_and_late_result_does_not_break_controls():
    client, reader, writer = pipes()
    try:
        review = begin(client, timeout=0.03)
        start = receive(reader)
        with pytest.raises(TimeoutError, match="settlement is unknown"):
            review.result()
        with pytest.raises(TimeoutError):
            review.result()
        with ThreadPoolExecutor(max_workers=1) as executor:
            retiring = executor.submit(client.retire_controller_review, review.review_id)
            request = receive(reader)
            assert request["id"] == 2
            respond(writer, start, {"kind": "controller_parent_reviewed", "outcome": {"state": "unavailable"}})
            respond(writer, request, control_reply(request, "retired"))
            assert retiring.result(timeout=2) == {"state": "retired"}
    finally:
        finish(client, reader, writer)


@pytest.mark.parametrize("bad", ["route", "purpose", "empty_lineage", "issued_revision"])
def test_malformed_native_selection_closes_client_and_never_becomes_permission(bad):
    client, reader, writer = pipes()
    try:
        review = begin(client)
        start = receive(reader)
        outcome = selected(submission()[0].route)
        if bad == "route": outcome["route"] = "someone-else"
        elif bad == "purpose": outcome["purpose"] = "operations"
        elif bad == "empty_lineage": outcome["lineage"] = []
        else: outcome["lineage"][0]["issued_revision"] = True
        respond(writer, start, {"kind": "controller_parent_reviewed", "outcome": outcome})
        with pytest.raises(HostControlError): review.result()
        with pytest.raises(HostControlError): client.observe_controller_review(review.review_id)
    finally:
        finish(client, reader, writer)


def test_wrong_review_correlation_closes_client():
    client, reader, writer = pipes()
    try:
        review = begin(client)
        receive(reader)
        with ThreadPoolExecutor(max_workers=1) as executor:
            observing = executor.submit(client.observe_controller_review, review.review_id)
            request = receive(reader)
            reply = control_reply(request, "awaiting_prompt")
            reply["review_id"] += 1
            respond(writer, request, reply)
            with pytest.raises(HostControlError): observing.result(timeout=2)
        with pytest.raises(HostControlError): review.result()
    finally:
        finish(client, reader, writer)


@pytest.mark.parametrize("expiry", [True, 0, -1, 2**64])
def test_login_rejects_invalid_unsigned_expiry(expiry):
    with pytest.raises(HostControlError): RemoteControllerLogin("paired-login", expiry)
