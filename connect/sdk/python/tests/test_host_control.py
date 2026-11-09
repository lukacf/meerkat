"""Actual anonymous-pipe protocol tests; no native policy/custody claim."""
from __future__ import annotations

import json
import dataclasses
import hashlib
import os
import threading
from concurrent.futures import ThreadPoolExecutor
from uuid import uuid4

import pytest

from meerkat_toolkit.host_control import (
    HostCloseHandle, HostCloseReason, HostCloseScope, HostCloseState,
    HostControlError, PrivateHostClient, validate_native_original_operations_request,
    PrivateAuthenticatedEvent, PrivateSourceEvidence, PrivateOriginalRef, PrivateOriginalSubmit,
    PrivateServiceAccessDecision, PrivateServiceGrantState,
)


def pipes():
    child_read, parent_write = os.pipe()
    parent_read, child_write = os.pipe()
    return PrivateHostClient(parent_write, parent_read, timeout=2), os.fdopen(child_read, "rb"), os.fdopen(child_write, "wb", buffering=0)


def receive(reader):
    return json.loads(reader.readline())


def respond(writer, request, result):
    writer.write(json.dumps({"version": 1, "id": request["id"], "result": result}).encode() + b"\n")


def handle():
    return HostCloseHandle(uuid4(), uuid4(), HostCloseScope("activation"))


def observation(close, state="settled"):
    return {"handle": close.wire(), "state": state, "failure": None}


def source_event():
    evidence = PrivateSourceEvidence("provider-event-1", "1", "a" * 64)
    return PrivateAuthenticatedEvent("member-route", "original-1", "occurrence-1",
                                     evidence, evidence, (), "b" * 64)


def original_for(event):
    encoded = json.dumps(dataclasses.asdict(event), sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()
    return PrivateOriginalRef(event.route, event.original, hashlib.sha256(encoded).hexdigest())


def native_operations_request():
    return {"not_before_ms": 1, "expires_at_ms": 99999, "targets": [
        {"issuance_id": "peer-root", "target": {"kind": "peer", "route": "actual-recipient-route", "actions": ["message", "request"]}},
        {"issuance_id": "directory-root", "target": {"kind": "peer_directory"}},
        {"issuance_id": "memory-root", "target": {"kind": "recorder_identity", "actions": ["read", "write", "delete"]}},
    ]}


@pytest.mark.parametrize("route", ["actual-recipient-route", "realm/peer:worker@host_1.2-3", "p" * 192])
def test_native_operations_public_validation_returns_isolated_retention_snapshot(route):
    request = native_operations_request()
    request["targets"][0]["target"]["route"] = route
    selected = validate_native_original_operations_request(request)
    assert isinstance(selected, dict)
    assert selected == request
    assert validate_native_original_operations_request(selected) == selected
    retained = json.loads(json.dumps(selected))
    request["targets"][0]["target"]["route"] = "changed-before-retention"
    request["targets"][2]["target"]["actions"].clear()
    assert selected == retained
    client, reader, writer = pipes()
    event = source_event()
    submission = PrivateOriginalSubmit(original_for(event), "untouched original text", event.original_authentication)
    try:
        with ThreadPoolExecutor(max_workers=1) as executor:
            pending = executor.submit(client.prepare_original_native_operations, submission, selected)
            command = receive(reader)
            assert command["command"] == {"kind": "prepare_original_native_operations",
                "submission": dataclasses.asdict(submission), "request": retained}
            respond(writer, command, {"kind": "native_operations_prepared",
                "original": dataclasses.asdict(submission.original)})
            assert pending.result(timeout=2) == submission.original
        assert selected == retained
    finally:
        client.close()
        assert reader.read() == b"", "validation and preparation cannot submit or retry an original"
        reader.close(); writer.close()
    selected["targets"][0]["target"]["actions"].clear()
    assert request["targets"][0]["target"]["actions"] == ["message", "request"]


@pytest.mark.parametrize("reply_kind", ["matching", "foreign", "refused"])
def test_native_operations_preserve_exact_new_original_selection(reply_kind):
    from meerkat_toolkit.host_control import HostReplyError
    client, reader, writer = pipes()
    event = source_event()
    submission = PrivateOriginalSubmit(original_for(event), "untouched original text", event.original_authentication)
    selection = native_operations_request()
    try:
        with ThreadPoolExecutor(max_workers=1) as executor:
            pending = executor.submit(client.prepare_original_native_operations, submission, selection)
            command = receive(reader)
            assert command["command"] == {"kind": "prepare_original_native_operations",
                "submission": dataclasses.asdict(submission), "request": selection}
            # The written intent is already a snapshot; mutation cannot retarget it.
            selection["targets"][0]["target"]["route"] = "mutated-later"
            if reply_kind == "refused":
                respond(writer, command, {"kind": "error", "error": "denied"})
            else:
                reference = dataclasses.asdict(submission.original)
                if reply_kind == "foreign": reference["original"] = "other"
                respond(writer, command, {"kind": "native_operations_prepared", "original": reference})
            if reply_kind == "matching": assert pending.result(timeout=2) == submission.original
            else:
                with pytest.raises(HostReplyError if reply_kind == "refused" else HostControlError):
                    pending.result(timeout=2)
    finally:
        client.close()
        assert reader.read() == b"", "preparation cannot submit or retry an original"
        reader.close(); writer.close()


_INVALID_NATIVE_PEER_ROUTES = {
    "route_whitespace": "peer one",
    "route_trailing_newline": "peer\n",
    "route_line_separator": "peer\u2028",
    "route_paragraph_separator": "peer\u2029",
    "route_empty_segment": "peer//two",
    "route_dot_segment": "peer/./two",
    "route_parent_segment": "peer/../two",
    "route_non_ascii": "peer/\u00e5",
    "route_too_long": "p" * 193,
    "route_leading_symbol": "_peer",
}


@pytest.mark.parametrize("change", ["mob", "audience", "peer_key", "duplicate", "actions", "infinite", "unordered", "boolean", "non_json", *_INVALID_NATIVE_PEER_ROUTES])
def test_native_operations_invalid_or_invented_scope_never_writes(change):
    client, reader, writer = pipes()
    event = source_event()
    submission = PrivateOriginalSubmit(original_for(event), "original", event.original_authentication)
    request = native_operations_request()
    if change == "mob": request["targets"][2]["target"]["kind"] = "recorder_mob"
    elif change == "audience": request["targets"][0]["target"]["audience"] = "invented"
    elif change == "peer_key": request["targets"][0]["target"]["peer"] = "invented"
    elif change == "duplicate": request["targets"][1]["issuance_id"] = "peer-root"
    elif change == "actions": request["targets"][2]["target"]["actions"] = ["propose"]
    elif change == "infinite": request["expires_at_ms"] = 2**64 - 1
    elif change == "unordered": request["targets"][2]["target"]["actions"] = ["write", "read"]
    elif change == "boolean": request["not_before_ms"] = True
    elif change in _INVALID_NATIVE_PEER_ROUTES: request["targets"][0]["target"]["route"] = _INVALID_NATIVE_PEER_ROUTES[change]
    else: request["targets"] = object()
    try:
        with pytest.raises(HostControlError): validate_native_original_operations_request(request)
        with pytest.raises(HostControlError): client.prepare_original_native_operations(submission, request)
    finally:
        client.close(); assert reader.read() == b""; reader.close(); writer.close()


def test_authenticated_event_retention_is_pending_and_does_not_submit_work():
    client, reader, writer = pipes()
    event = source_event()
    original = original_for(event)
    with ThreadPoolExecutor(max_workers=1) as executor:
        pending = executor.submit(client.retain_authenticated_event, event)
        request = receive(reader)
        assert request["command"] == {"kind": "retain_authenticated_event", "event": json.loads(json.dumps(dataclasses.asdict(event)))}
        respond(writer, request, {"kind": "authenticated_event_retained", "original": dataclasses.asdict(original), "state": "retained_pending_native_admission"})
        observation = pending.result(timeout=2)
        assert observation.original == original
        assert observation.state == "retained_pending_native_admission"
    client.close()
    # No native-submit command or hidden retry was emitted after retention.
    assert reader.read() == b""
    reader.close()
    writer.close()


def test_authenticated_event_reply_cannot_replace_original():
    client, reader, writer = pipes()
    event = source_event()
    wrong = dataclasses.replace(original_for(event), original="another-original")
    with ThreadPoolExecutor(max_workers=1) as executor:
        pending = executor.submit(client.retain_authenticated_event, event)
        request = receive(reader)
        respond(writer, request, {"kind": "authenticated_event_retained", "original": dataclasses.asdict(wrong), "state": "retained_pending_native_admission"})
        with pytest.raises(HostControlError, match="source response was rejected"):
            pending.result(timeout=2)
    assert reader.read() == b""
    client.close()
    reader.close()
    writer.close()


@pytest.mark.parametrize("state", ["operations_cancelled", "operations_cancelled_cleanup_pending", "operation_grant_revoked", "settled"])
def test_original_cancellation_reports_durable_disposition_not_legacy_grant_revoke(state):
    client, reader, writer = pipes()
    original = original_for(source_event())
    with ThreadPoolExecutor(max_workers=1) as executor:
        pending = executor.submit(client.cancel_original_operation, original)
        request = receive(reader)
        respond(writer, request, {"kind": "original_operation_cancelled", "original": dataclasses.asdict(original), "state": state})
        if state in ("operations_cancelled", "operations_cancelled_cleanup_pending"):
            result = pending.result(timeout=2)
            assert result.original == original
            assert result.state == state
        else:
            with pytest.raises(HostControlError, match="cancellation response was rejected"):
                pending.result(timeout=2)
    client.close()
    reader.close()
    writer.close()


def test_original_operation_cancel_drains_late_response_without_retry():
    client, reader, writer = pipes()
    original = original_for(source_event())
    with ThreadPoolExecutor(max_workers=1) as executor:
        pending = executor.submit(client.cancel_original_operation, original, timeout=0.05)
        request = receive(reader)
        assert request["command"] == {"kind": "cancel_original_operation", "original": dataclasses.asdict(original)}
        with pytest.raises(TimeoutError, match="settlement is unknown"):
            pending.result(timeout=2)
        respond(writer, request, {"kind": "original_operation_cancelled", "original": dataclasses.asdict(original), "state": "operations_cancelled"})
        # An independent lifecycle observation proves late source completion
        # was drained without issuing another cancellation or claiming a run ended.
        close = handle()
        observed = executor.submit(client.observe_close, close)
        next_request = receive(reader)
        assert next_request["id"] == request["id"] + 1
        assert next_request["command"]["kind"] == "observe_close"
        respond(writer, next_request, {"kind": "close", "observation": observation(close, "closing")})
        assert observed.result(timeout=2).state == HostCloseState.CLOSING
    client.close()
    assert reader.read() == b""
    reader.close()
    writer.close()


def test_parent_ends_are_non_inheritable_and_close_produces_real_eof():
    client, reader, writer = pipes()
    assert not os.get_inheritable(client._command_fd)
    assert not os.get_inheritable(client._response_fd)
    client.close()
    assert reader.read() == b""
    assert not client._reader.is_alive()
    reader.close()
    writer.close()


def test_pending_join_does_not_block_concurrent_begin_close():
    client, reader, writer = pipes()
    close = handle()
    with ThreadPoolExecutor(max_workers=2) as executor:
        joining = executor.submit(client.join_close, close)
        join_request = receive(reader)
        assert join_request["command"]["kind"] == "join_close"
        disabling = executor.submit(client.begin_close_component, "device", HostCloseReason.QUARANTINE)
        begin_request = receive(reader)
        assert begin_request["command"]["kind"] == "begin_close_component"
        component_close = HostCloseHandle(close.incarnation, uuid4(), HostCloseScope("component", "device"))
        respond(writer, begin_request, {"kind": "close", "observation": observation(component_close, "closing")})
        assert disabling.result(timeout=2).state == HostCloseState.CLOSING
        assert not joining.done()
        respond(writer, join_request, {"kind": "close", "observation": observation(close)})
        assert joining.result(timeout=2).state == HostCloseState.SETTLED
    client.close()
    reader.close()
    writer.close()


def test_native_eof_wakes_existing_waiter_without_success_or_harmless_absence():
    client, reader, writer = pipes()
    with ThreadPoolExecutor(max_workers=1) as executor:
        waiting = executor.submit(client.join_close, handle())
        receive(reader)
        writer.close()
        with pytest.raises(HostControlError, match="settlement is unknown"):
            waiting.result(timeout=2)
    client.close()
    assert reader.read() == b""
    reader.close()


def test_timeout_retains_late_correlation_without_reissuing_operation():
    client, reader, writer = pipes()
    close = handle()
    with ThreadPoolExecutor(max_workers=1) as executor:
        waiting = executor.submit(client.join_close, close, timeout=0.05)
        request = receive(reader)
        with pytest.raises(TimeoutError, match="settlement is unknown"):
            waiting.result(timeout=2)
        respond(writer, request, {"kind": "close", "observation": observation(close)})
        pending = executor.submit(client.observe_close, close)
        next_request = receive(reader)
        assert next_request["id"] == request["id"] + 1
        assert next_request["command"]["kind"] == "observe_close"
        respond(writer, next_request, {"kind": "close", "observation": observation(close)})
        assert pending.result(timeout=2).state == HostCloseState.SETTLED
    client.close()
    reader.close()
    writer.close()


def test_unknown_response_correlation_closes_command_endpoint():
    client, reader, writer = pipes()
    writer.write(b'{"version":1,"id":999,"result":{"kind":"bootstrap_accepted"}}\n')
    client._reader.join(timeout=2)
    assert not client._reader.is_alive()
    with pytest.raises(HostControlError):
        client.open_component("device")
    assert reader.read() == b""
    client.close()
    reader.close()
    writer.close()


def test_close_observation_preserves_uncertainty_and_exact_owner_handle():
    client, reader, writer = pipes()
    close = handle()
    with ThreadPoolExecutor(max_workers=1) as executor:
        pending = executor.submit(client.observe_close, close)
        request = receive(reader)
        observed = observation(close, "uncertain")
        observed["failure"] = "physical_settlement_uncertain"
        respond(writer, request, {"kind": "close", "observation": observed})
        result = pending.result(timeout=2)
        assert result.handle == close
        assert result.state == HostCloseState.UNCERTAIN
        assert result.failure == "physical_settlement_uncertain"
    client.close()
    reader.close()
    writer.close()


def test_oversize_response_closes_pipe_and_wakes_waiter():
    import select
    import time
    client, reader, writer = pipes()
    producer = None
    try:
        with ThreadPoolExecutor(max_workers=1) as executor:
            try:
                pending = executor.submit(client.open_component, "device")
                deadline = time.monotonic() + 2
                command = bytearray()
                while not command.endswith(b"\n"):
                    remaining = deadline - time.monotonic()
                    assert remaining > 0 and select.select([reader], [], [], remaining)[0]
                    part = os.read(reader.fileno(), 1)
                    assert part
                    command.extend(part)
                # The writer and independent SDK reader run concurrently. Generate
                # the payload here, not in argv, and always release/join on failure.
                def flood():
                    try:
                        writer.write(b"x" * 1_048_577)
                    except (BrokenPipeError, ValueError):
                        pass
                producer = threading.Thread(target=flood)
                producer.start()
                with pytest.raises(HostControlError):
                    pending.result(timeout=2)
            finally:
                # Wake the pending SDK call before executor shutdown waits for it.
                client.close()
    finally:
        client.close()
        reader.close()
        writer.close()
        if producer is not None:
            producer.join(timeout=2)
    if producer is not None:
        assert not producer.is_alive()


def test_wrong_close_handle_refuses_and_closes_private_admission():
    client, reader, writer = pipes()
    with ThreadPoolExecutor(max_workers=1) as executor:
        pending = executor.submit(client.observe_close, handle())
        request = receive(reader)
        respond(writer, request, {"kind": "close", "observation": observation(handle())})
        with pytest.raises(HostControlError, match="observation was rejected"):
            pending.result(timeout=2)
    assert reader.read() == b""
    client.close()
    reader.close()
    writer.close()


def test_forced_native_child_exit_is_not_success_before_public_init(monkeypatch):
    from meerkat_meta._transport import PersistentTransport, TransportTermination
    from meerkat_toolkit.host_control import ToolkitPersistentTransport
    transport = ToolkitPersistentTransport("/inert/not-spawned", object())
    class ReapedProcess:
        returncode = -9
    transport._process = ReapedProcess()
    transport.host_control = None
    def observed_native_stop(owner):
        owner._process = None
        owner._last_termination = TransportTermination("exited", None, "SIGKILL", "sigkill", "not_negotiated")
    monkeypatch.setattr(PersistentTransport, "stop", observed_native_stop)
    with pytest.raises(HostControlError, match="without complete settlement"):
        transport.stop()


def test_lost_bootstrap_reply_consumes_first_use_before_automatic_respawn(monkeypatch):
    from meerkat_meta._transport import PersistentTransport
    from meerkat_toolkit.host_control import ToolkitPersistentTransport
    launches = []
    monkeypatch.setattr(PersistentTransport, "start", lambda self: launches.append(self._extra_args))
    monkeypatch.setattr(PersistentTransport, "stop", lambda self: None)

    def lost_reply(self, selection):
        raise HostControlError("bootstrap reply lost")

    monkeypatch.setattr(PrivateHostClient, "bootstrap", lost_reply)
    # This exercises the real Toolkit pipe/client/launch-choice cleanup. No
    # native process or actual native owner initialization is claimed here.
    transport = ToolkitPersistentTransport("fixture-gateway", object(), initialize_native_owner=True)
    for _ in range(2):
        with pytest.raises(HostControlError, match="bootstrap reply lost"):
            transport.start()
        assert transport.host_control is None
    assert "--initialize-native-owner" in launches[0]
    assert "--initialize-native-owner" not in launches[1]


def access_decision():
    return PrivateServiceAccessDecision(
        "member-route", "access-1", PrivateSourceEvidence("access-panel", "1", "a" * 64),
        "approve", {"target": "agent", "connection": "gmail-primary",
                    "operation": "read-messages", "resource": "account-one",
                    "lifetime": {"scope": {"kind": "member", "member": "agent"},
                                 "expires_at_ms": None}}, "binding-1")


def access_observation(decision, state="grant_issued"):
    wire = decision.wire()
    encoded = json.dumps(wire, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()
    return {"grant": {"route": decision.route, "decision": decision.decision,
                      "decision_sha256": hashlib.sha256(encoded).hexdigest()}, "state": state}


def test_service_access_reports_exact_issuance_state_without_submitting_original():
    client, reader, writer = pipes()
    decision = access_decision()
    with ThreadPoolExecutor(max_workers=1) as executor:
        pending = executor.submit(client.apply_service_access_decision, decision)
        request = receive(reader)
        assert request["command"] == {"kind": "apply_service_access_decision", "decision": decision.wire()}
        respond(writer, request, {"kind": "service_access_decision_applied",
                                  "observation": access_observation(decision, "pending_reconciliation")})
        result = pending.result(timeout=2)
        assert result.state == PrivateServiceGrantState.PENDING_RECONCILIATION
    client.close()
    assert reader.read() == b""
    reader.close()
    writer.close()


def test_service_access_reply_cannot_substitute_decision():
    client, reader, writer = pipes()
    decision = access_decision()
    with ThreadPoolExecutor(max_workers=1) as executor:
        pending = executor.submit(client.apply_service_access_decision, decision)
        request = receive(reader)
        result = access_observation(decision)
        result["grant"]["decision"] = "other-decision"
        respond(writer, request, {"kind": "service_access_decision_applied", "observation": result})
        with pytest.raises(HostControlError, match="service access response was rejected"):
            pending.result(timeout=2)
    assert reader.read() == b""
    client.close()
    reader.close()
    writer.close()


def test_service_access_timeout_does_not_reissue_native_grant():
    client, reader, writer = pipes()
    decision = access_decision()
    with ThreadPoolExecutor(max_workers=1) as executor:
        pending = executor.submit(client.apply_service_access_decision, decision, timeout=0.05)
        request = receive(reader)
        with pytest.raises(TimeoutError, match="settlement is unknown"):
            pending.result(timeout=2)
        respond(writer, request, {"kind": "service_access_decision_applied",
                                  "observation": access_observation(decision)})
        close = handle()
        observing = executor.submit(client.observe_close, close)
        next_request = receive(reader)
        assert next_request["id"] == request["id"] + 1
        assert next_request["command"]["kind"] == "observe_close"
        respond(writer, next_request, {"kind": "close", "observation": observation(close, "closing")})
        assert observing.result(timeout=2).state == HostCloseState.CLOSING
    client.close()
    assert reader.read() == b""
    reader.close()
    writer.close()


def test_console_login_retirement_uses_original_correlation_after_timeout_and_late_reply():
    from meerkat_toolkit.host_control import ConsoleLoginSelection
    client, reader, writer = pipes()
    login = client.begin_console_login(ConsoleLoginSelection("issuer", "verified-sub", "scope-a", 9_000_000_000_000), timeout=0.02)
    request = receive(reader)
    assert request["command"] == {"kind": "enroll_console_login", "login": {
        "issuer": "issuer", "subject": "verified-sub", "login_scope_id": "scope-a", "expires_at_ms": 9_000_000_000_000}}
    with pytest.raises(TimeoutError):
        login.result()
    respond(writer, request, {"kind": "console_login_enrolled", "enrollment_id": login.enrollment_id})
    with ThreadPoolExecutor(max_workers=1) as executor:
        retiring = executor.submit(login.retire)
        retire_request = receive(reader)
        assert retire_request["command"] == {"kind": "retire_console_login", "enrollment_id": request["id"]}
        respond(writer, retire_request, {"kind": "console_login_retired", "enrollment_id": request["id"],
            "observation": {"fenced": True, "reservations_drained": True, "uncertain": True}})
        observed = retiring.result(timeout=2)
    assert observed.uncertain
    assert login.retire() is observed
    with pytest.raises(TimeoutError):
        login.result()  # The discarded late reply does not authenticate the login.
    client.close()
    assert reader.read() == b""  # Neither method resubmits an enrollment or retirement.
    reader.close(); writer.close()


def test_console_retirement_reserved_capacity_bypasses_32_ordinary_waiters():
    from meerkat_toolkit.host_control import ConsoleLoginSelection
    client, reader, writer = pipes()
    login = client.begin_console_login(ConsoleLoginSelection("issuer", "sub", "scope", 9_000_000_000_000))
    request = receive(reader)
    respond(writer, request, {"kind": "console_login_enrolled", "enrollment_id": request["id"]})
    login.result()
    # Use the actual writer/reader/correlation owner without 32 executor threads.
    pending = []
    for _ in range(32):
        pending.append(client._start_call({"kind": "observe_close", "handle": handle().wire()}))
        receive(reader)
    with pytest.raises(HostControlError, match="limit"):
        client.open_component("still-full")
    with ThreadPoolExecutor(max_workers=1) as executor:
        retiring = executor.submit(login.retire)
        request = receive(reader)
        assert request["command"]["kind"] == "retire_console_login"
        assert len(client._pending) == 33
        respond(writer, request, {"kind": "console_login_retired", "enrollment_id": login.enrollment_id,
            "observation": {"fenced": True, "reservations_drained": True, "uncertain": False}})
        assert not retiring.result(timeout=2).uncertain
    client.close()
    assert reader.read() == b""
    reader.close(); writer.close()


def test_console_retirement_exhausted_control_capacity_closes_pipe_as_unknown():
    from meerkat_toolkit.host_control import ConsoleLoginSelection, HostTransportError
    client, reader, writer = pipes()
    logins = [client.begin_console_login(ConsoleLoginSelection("issuer", "sub", scope, 9_000_000_000_000)) for scope in ("a", "b")]
    for login in logins:
        request = receive(reader)
        respond(writer, request, {"kind": "console_login_enrolled", "enrollment_id": request["id"]})
        login.result()
    for _ in range(32):
        client._start_call({"kind": "observe_close", "handle": handle().wire()}); receive(reader)
    with ThreadPoolExecutor(max_workers=1) as executor:
        first = executor.submit(logins[0].retire)
        assert receive(reader)["command"]["kind"] == "retire_console_login"
        with pytest.raises(HostTransportError):
            logins[1].retire()
        with pytest.raises(HostTransportError):
            first.result(timeout=2)
    assert reader.read() == b""
    client.close(); reader.close(); writer.close()


@pytest.mark.parametrize("observation", [
    {"fenced": False, "reservations_drained": True, "uncertain": False},
    {"fenced": True, "reservations_drained": False, "uncertain": False},
    {"fenced": True, "reservations_drained": True, "uncertain": 0},
    {"fenced": True, "reservations_drained": True, "uncertain": False, "admitted": False},
])
def test_console_retirement_rejects_invented_cleanup_or_malformed_observation(observation):
    from meerkat_toolkit.host_control import ConsoleLoginSelection, HostTransportError
    client, reader, writer = pipes()
    login = client.begin_console_login(ConsoleLoginSelection("issuer", "sub", "scope", 9_000_000_000_000))
    request = receive(reader)
    respond(writer, request, {"kind": "console_login_enrolled", "enrollment_id": request["id"]})
    login.result()
    with ThreadPoolExecutor(max_workers=1) as executor:
        retiring = executor.submit(login.retire)
        request = receive(reader)
        respond(writer, request, {"kind": "console_login_retired", "enrollment_id": login.enrollment_id, "observation": observation})
        with pytest.raises(HostTransportError):
            retiring.result(timeout=2)
    assert reader.read() == b""
    client.close(); reader.close(); writer.close()
