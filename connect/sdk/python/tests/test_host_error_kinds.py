"""Inert private pipes: error classification is not native effect evidence."""
from __future__ import annotations

from concurrent.futures import ThreadPoolExecutor
from contextlib import contextmanager

import pytest

from meerkat_toolkit.host_control import (
    HostControlError, HostReplyCode, HostReplyError, HostTransportError,
)
from test_host_control import handle, observation, pipes, receive, respond
from test_remote_review import begin, control_reply
from test_host_write_deadlines import PausedPipe


@contextmanager
def owned_pipes():
    client, reader, writer = pipes()
    try:
        yield client, reader, writer
    finally:
        client.close()
        assert not client._reader.is_alive()
        assert client._command_fd is None
        if not reader.closed:
            assert reader.read() == b"", "the client emitted an unrequested retry"
            reader.close()
        writer.close()


def healthy_observation(client, reader, writer, executor, expected_id):
    close = handle()
    future = executor.submit(client.observe_close, close)
    request = receive(reader)
    assert request["id"] == expected_id
    assert request["command"]["kind"] == "observe_close"
    respond(writer, request, {"kind": "close", "observation": observation(close, "closing")})
    assert future.result(timeout=2).state.value == "closing"
    assert client._failure is None
    assert client._pending == {}


@pytest.mark.parametrize("code", list(HostReplyCode))
def test_exact_native_error_has_closed_code_and_keeps_same_client_usable(code):
    with owned_pipes() as (client, reader, writer):
        with ThreadPoolExecutor(max_workers=1) as executor:
            pending = executor.submit(client.open_component, "fixture")
            request = receive(reader)
            respond(writer, request, {"kind": "error", "error": code.value})
            with pytest.raises(HostReplyError) as caught:
                pending.result(timeout=2)
            assert caught.value.code is code
            assert isinstance(caught.value, HostControlError)
            assert not isinstance(caught.value, HostTransportError)
            assert str(caught.value) == f"native host refused: {code.value}"
            healthy_observation(client, reader, writer, executor, 2)


@pytest.mark.parametrize("bad", [
    {"kind": "error", "error": "future_unknown_code"},
    {"kind": "error", "error": ["denied"]},
    {"kind": "error", "error": "denied", "extra": "not-in-protocol"},
])
def test_unknown_or_malformed_error_is_transport_failure(bad):
    with owned_pipes() as (client, reader, writer):
        with ThreadPoolExecutor(max_workers=1) as executor:
            pending = executor.submit(client.open_component, "fixture")
            request = receive(reader)
            respond(writer, request, bad)
            with pytest.raises(HostTransportError) as caught:
                pending.result(timeout=2)
            assert not isinstance(caught.value, HostReplyError)
        with pytest.raises(HostTransportError):
            client.open_component("another")
        assert client._next_id == 2


def test_response_eof_is_host_loss_for_waiter_and_future_calls():
    with owned_pipes() as (client, reader, writer):
        with ThreadPoolExecutor(max_workers=1) as executor:
            pending = executor.submit(client.open_component, "fixture")
            receive(reader)
            writer.close()
            with pytest.raises(HostTransportError, match="settlement is unknown"):
                pending.result(timeout=2)
        with pytest.raises(HostTransportError):
            client.open_component("another")
        assert client._next_id == 2


def test_broken_command_writer_is_host_loss_without_replay():
    with owned_pipes() as (client, reader, writer):
        reader.close()
        with pytest.raises(HostTransportError, match="settlement is unknown"):
            client.open_component("fixture")
        assert client._failure is not None
        with pytest.raises(HostTransportError):
            client.open_component("another")
        assert client._next_id == 2


def test_wrong_review_correlation_is_transport_failure():
    with owned_pipes() as (client, reader, writer):
        with ThreadPoolExecutor(max_workers=1) as executor:
            pending = executor.submit(client.observe_controller_review, 7)
            request = receive(reader)
            reply = control_reply(request, "awaiting_prompt")
            reply["review_id"] = 8
            respond(writer, request, reply)
            with pytest.raises(HostTransportError):
                pending.result(timeout=2)
        with pytest.raises(HostTransportError):
            client.observe_controller_review(7)


@pytest.mark.parametrize("field,value", [("version", 2), ("id", 999)])
def test_outer_version_and_request_correlation_still_fail_shared_client(field, value):
    import json
    with owned_pipes() as (client, reader, writer):
        with ThreadPoolExecutor(max_workers=1) as executor:
            pending = executor.submit(client.open_component, "fixture")
            request = receive(reader)
            response = {"version": 1, "id": request["id"],
                        "result": {"kind": "error", "error": "denied"}}
            response[field] = value
            writer.write(json.dumps(response).encode() + b"\n")
            with pytest.raises(HostTransportError):
                pending.result(timeout=2)


def test_observation_timeout_is_not_host_loss_and_late_reply_is_not_resent():
    with owned_pipes() as (client, reader, writer):
        review = begin(client, timeout=0.04)
        request = receive(reader)
        with pytest.raises(TimeoutError) as first:
            review.result()
        with pytest.raises(TimeoutError) as second:
            review.result()
        assert first.value is second.value
        assert not isinstance(first.value, HostControlError)
        assert client._failure is None
        assert review.review_id in client._pending
        respond(writer, request, {"kind": "controller_parent_reviewed", "outcome": {"state": "unavailable"}})
        with ThreadPoolExecutor(max_workers=1) as executor:
            healthy_observation(client, reader, writer, executor, 2)
        with pytest.raises(TimeoutError) as third:
            review.result()
        assert third.value is first.value
        assert client._next_id == 3


@pytest.mark.parametrize("invalid", ["oversize", "serialization", "choice"])
def test_local_validation_does_not_publish_id_or_poison_client(invalid):
    with owned_pipes() as (client, reader, writer):
        with pytest.raises(HostControlError) as caught:
            if invalid == "oversize":
                client.open_component("x" * 1_048_576)
            elif invalid == "serialization":
                client.open_component(object())
            else:
                client.answer_controller_review(7, "approval", "invalid")
        assert not isinstance(caught.value, (HostReplyError, HostTransportError))
        assert client._failure is None
        assert client._pending == {}
        assert client._abandoned == set()
        assert client._next_id == 1
        assert not client._write_lock.locked()
        with ThreadPoolExecutor(max_workers=1) as executor:
            healthy_observation(client, reader, writer, executor, 1)


def test_real_backpressure_poison_has_transport_type_and_no_queued_resend():
    pipe = PausedPipe(timeout=0.3)
    try:
        worker, outcome = pipe.saturate()
        retiring, retired = pipe.run(lambda: pipe.client.retire_controller_review(1))
        worker.join(timeout=2)
        retiring.join(timeout=2)
        assert not worker.is_alive() and not retiring.is_alive()
        assert outcome == [None]
        assert len(pipe.errors) == 1 and isinstance(pipe.errors[0], HostTransportError)
        assert len(retired) == 1 and isinstance(retired[0], HostTransportError)
        for review in pipe.handles:
            with pytest.raises(HostTransportError):
                review.result()
        assert pipe.client._failure is not None
    finally:
        pipe.finish()
