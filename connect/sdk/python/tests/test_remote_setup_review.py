"""Private-pipe setup-review protocol controls, not native permission evidence."""
from concurrent.futures import ThreadPoolExecutor
import copy
import dataclasses
import json
import os
import select
import time

import pytest

from meerkat_toolkit.host_control import (
    HostTransportError, RemoteControllerLogin,
)
from test_host_control import pipes, receive, respond, handle, observation
from test_host_submit import submission, finish
from test_remote_review import selected as controller_selected, control_reply


def begin(client, **kwargs):
    return client.begin_remote_setup_request_review(
        *submission(), RemoteControllerLogin("paired-login", 1900000000000), **kwargs)


def selected():
    return {
        "state": "setup_request_selected",
        "selection": {
            "original": dataclasses.asdict(submission()[0]),
            "session_id": "00000000-0000-4000-8000-000000000002",
            "parent": controller_selected(submission()[0].route)["lineage"],
        },
    }


def helper_targets():
    # Native-shaped observations only, with distinct parents and bounds. These
    # fixture references are not issued grants or a native authorization test.
    rows = []
    for name, operation in (("accounts", "toolkit_request_service_account"), ("results", "setup_report")):
        parent = copy.deepcopy(selected()["selection"]["parent"][0])
        parent["grant_id"] = "fixture-" + name
        exact = lambda values: {"bound": {"bound": "exact", "values": values}, "unresolved": []}
        rows.append({
            "target": {"kind": "registered_tool_bundle_operation", "bundle": "meerkat_toolkit.setup_" + name,
                       "operation": operation},
            "parent": parent,
            "restrictions": {
                "actions": exact([{"feature": "fixture", "action": operation}]),
                "resource_domains": exact([{"authority": parent["root_authority"], "namespace": "original/" + name}]),
                "processors": exact([{"processor": "principal", "principal": parent["root_authority"]}]),
                "audiences": exact([{"audience": "principal", "principal": parent["root_authority"]}]),
                "lifetime": {"bound": {"bound": "unrestricted"}, "unresolved": []},
                "delegation_depth": {"bound": {"bound": "remaining", "edges": 1}, "unresolved": []},
            },
        })
    return rows


def with_targets(outcome, mode):
    if mode == "omitted":
        return outcome
    rows = [] if mode == "empty" else helper_targets()
    if mode == "all":
        base, mcp = copy.deepcopy(rows)
        base["target"] = None; base["parent"]["grant_id"] = "fixture-base"
        mcp["target"] = {"kind": "mcp_server", "server": "selected-setup"}
        mcp["parent"]["grant_id"] = "fixture-mcp"
        rows = [base, mcp, *rows]
    outcome["selection"]["helper_targets"] = rows
    return outcome


@pytest.mark.parametrize("targets", ["omitted", "empty", "correlated", "all"])
def test_setup_review_exact_wire_and_controls_do_not_submit_original(targets):
    client, reader, writer = pipes()
    try:
        review = begin(client)
        start = receive(reader)
        assert start == {"version": 1, "id": review.review_id, "command": {
            "kind": "review_remote_setup_request",
            "submission": {
                "original": dataclasses.asdict(submission()[0]), "content": submission()[1],
                "transport_authentication": dataclasses.asdict(submission()[2]),
            },
            "login": {"scope_id": "paired-login", "expires_at_ms": 1900000000000},
        }}
        with ThreadPoolExecutor(max_workers=2) as executor:
            final = executor.submit(review.result)
            observing = executor.submit(client.observe_controller_review, review.review_id)
            request = receive(reader)
            assert request["command"] == {
                "kind": "observe_controller_review", "review_id": start["id"]}
            prompt = {"kind": "approval", "approval_id": "approval-1", "summary": "Setup request"}
            respond(writer, request, control_reply(request, "prompt", prompt=prompt))
            assert observing.result(timeout=2) == {"state": "prompt", "prompt": prompt}
            answering = executor.submit(
                client.answer_controller_review, review.review_id, "approval-1", "approve")
            request = receive(reader)
            assert request["command"] == {
                "kind": "answer_controller_review", "review_id": start["id"],
                "approval_id": "approval-1", "decision": "approve",
            }
            respond(writer, request, control_reply(request, "answer_forwarded"))
            assert answering.result(timeout=2) == {"state": "answer_forwarded"}
            assert not final.done(), "Forwarding approval is not the selected native result"
            expected = with_targets(selected(), targets)
            respond(writer, start, {"kind": "setup_request_reviewed", "outcome": expected})
            result = final.result(timeout=2)
            assert result == expected
            result["selection"]["parent"].clear()
            if result["selection"].get("helper_targets"):
                result["selection"]["helper_targets"][0]["parent"]["grant_id"] = "caller-substitution"
                result["selection"]["helper_targets"][0]["restrictions"]["actions"].clear()
            assert review.result() == expected
            close = handle()
            observing = executor.submit(client.observe_close, close)
            request = receive(reader)
            respond(writer, request, {"kind": "close", "observation": observation(close, "closing")})
            assert observing.result(timeout=2).state.value == "closing"
    finally:
        # This also rejects any implicit submit, duplicate review or extra command.
        finish(client, reader, writer)


@pytest.mark.parametrize("state", ["denied", "unavailable", "pending_reconciliation"])
def test_setup_review_preserves_nonselected_outcomes(state):
    client, reader, writer = pipes()
    try:
        review = begin(client)
        start = receive(reader)
        respond(writer, start, {"kind": "setup_request_reviewed", "outcome": {"state": state}})
        assert review.result() == {"state": state}
        assert review.result() == {"state": state}
    finally:
        finish(client, reader, writer)


BAD_SELECTIONS = (
    "outcome_extra", "selection_extra", "missing_parent", "wrong_route", "wrong_original",
    "wrong_digest", "invalid_session", "nonstring_session", "empty_parent", "multiple_parents",
    "parent_object", "parent_extra", "root_array", "invalid_incarnation", "boolean_generation",
    "zero_revision", "unknown_state", "controller_shape",
    "targets_null", "targets_object", "targets_limit", "target_row_extra", "target_unknown",
    "target_extra", "target_missing_name", "target_empty_name", "target_nonstring_name",
    "target_duplicate", "target_parent_array", "target_parent_extra", "target_parent_revision",
    "target_restrictions_null", "target_restrictions_extra", "target_dimension_scalar",
)


def corrupt(outcome, case):
    chosen = outcome["selection"]
    if case.startswith(("target_", "targets_")):
        chosen["helper_targets"] = rows = helper_targets()
        row = rows[0]
        if case == "targets_null": chosen["helper_targets"] = None
        elif case == "targets_object": chosen["helper_targets"] = row
        elif case == "targets_limit":
            # Stay within the frame limit so the target-count boundary is exercised.
            row["restrictions"] = {key: {} for key in row["restrictions"]}
            chosen["helper_targets"] = [
                {**copy.deepcopy(row), "target": {"kind": "mcp_server", "server": f"fixture-{i:02d}"}}
                for i in range(64)
            ]
        elif case == "target_row_extra": row["permission"] = "all"
        elif case == "target_unknown": row["target"] = {"kind": "future_target", "operation": "setup_report"}
        elif case == "target_extra": row["target"]["fallback"] = True
        elif case == "target_missing_name": del row["target"]["operation"]
        elif case == "target_empty_name": row["target"]["bundle"] = ""
        elif case == "target_nonstring_name": row["target"]["operation"] = 1
        elif case == "target_duplicate": rows.append(copy.deepcopy(row))
        elif case == "target_parent_array": row["parent"] = [row["parent"]]
        elif case == "target_parent_extra": row["parent"]["permission"] = "all"
        elif case == "target_parent_revision": row["parent"]["issued_revision"] = 0
        elif case == "target_restrictions_null": row["restrictions"] = None
        elif case == "target_restrictions_extra": row["restrictions"]["permission"] = "all"
        elif case == "target_dimension_scalar": row["restrictions"]["actions"] = True
        else: raise AssertionError(case)
        return outcome
    if case == "outcome_extra": outcome["admitted"] = True
    elif case == "selection_extra": chosen["turn_ticket"] = chosen["session_id"]
    elif case == "missing_parent": del chosen["parent"]
    elif case == "wrong_route": chosen["original"]["route"] = "another-route"
    elif case == "wrong_original": chosen["original"]["original"] = "another-original"
    elif case == "wrong_digest": chosen["original"]["event_sha256"] = "9" * 64
    elif case == "invalid_session": chosen["session_id"] = "not-a-uuid"
    elif case == "nonstring_session": chosen["session_id"] = 7
    elif case == "empty_parent": chosen["parent"] = []
    elif case == "multiple_parents": chosen["parent"].append(copy.deepcopy(chosen["parent"][0]))
    elif case == "parent_object": chosen["parent"] = chosen["parent"][0]
    elif case == "parent_extra": chosen["parent"][0]["permission"] = "all"
    elif case == "root_array": chosen["parent"][0]["root_authority"] = []
    elif case == "invalid_incarnation": chosen["parent"][0]["authority_incarnation"] = "bad"
    elif case == "boolean_generation": chosen["parent"][0]["authority_generation"] = True
    elif case == "zero_revision": chosen["parent"][0]["issued_revision"] = 0
    elif case == "unknown_state": outcome["state"] = "admitted"
    elif case == "controller_shape": return controller_selected(submission()[0].route)
    else: raise AssertionError(case)
    return outcome


@pytest.mark.parametrize("case", BAD_SELECTIONS)
def test_setup_review_rejects_wrong_original_or_malformed_native_selection(case):
    client, reader, writer = pipes()
    try:
        review = begin(client)
        start = receive(reader)
        reply = {"kind": "setup_request_reviewed", "outcome": corrupt(selected(), case)}
        if case == "targets_limit":
            # Exact respond() envelope and encoding, including its newline.
            frame = json.dumps({"version": 1, "id": start["id"], "result": reply}).encode() + b"\n"
            assert len(frame) <= 1_048_576, "exercise target count, not oversized-frame refusal"
        respond(writer, start, reply)
        with pytest.raises(HostTransportError):
            review.result()
        with pytest.raises(HostTransportError):
            client.observe_controller_review(review.review_id)
    finally:
        finish(client, reader, writer)


@pytest.mark.parametrize("case", ["wrong_reply_kind", "wrong_review_id", "nonselected_extra"])
def test_setup_review_rejects_reply_shape_and_control_correlation(case):
    client, reader, writer = pipes()
    try:
        review = begin(client)
        start = receive(reader)
        if case == "wrong_review_id":
            with ThreadPoolExecutor(max_workers=1) as executor:
                observing = executor.submit(client.observe_controller_review, review.review_id)
                request = receive(reader)
                reply = control_reply(request, "awaiting_prompt")
                reply["review_id"] += 1
                respond(writer, request, reply)
                with pytest.raises(HostTransportError):
                    observing.result(timeout=2)
        else:
            reply = {"kind": "setup_request_reviewed", "outcome": selected()}
            if case == "wrong_reply_kind": reply["kind"] = "controller_parent_reviewed"
            else: reply["outcome"] = {"state": "denied", "selection": selected()["selection"]}
            respond(writer, start, reply)
        with pytest.raises(HostTransportError):
            review.result()
    finally:
        finish(client, reader, writer)


@pytest.mark.parametrize("case", ["setup_state", "setup_purpose"])
def test_existing_controller_decoder_stays_controller_only(case):
    client, reader, writer = pipes()
    try:
        review = client.begin_remote_controller_review(
            *submission(), RemoteControllerLogin("paired-login", 1900000000000))
        start = receive(reader)
        outcome = selected()
        if case == "setup_purpose":
            outcome = controller_selected(submission()[0].route)
            outcome["purpose"] = "setup_request"
        respond(writer, start, {"kind": "controller_parent_reviewed", "outcome": outcome})
        with pytest.raises(HostTransportError):
            review.result()
    finally:
        finish(client, reader, writer)


def test_setup_review_timeout_is_cached_and_late_selected_reply_does_not_retry():
    client, reader, writer = pipes()
    try:
        review = begin(client, timeout=0.05)
        start = receive(reader)
        with pytest.raises(TimeoutError, match="settlement is unknown") as first:
            review.result()
        with pytest.raises(TimeoutError) as repeated:
            review.result()
        assert repeated.value is first.value
        with ThreadPoolExecutor(max_workers=1) as executor:
            retiring = executor.submit(client.retire_controller_review, review.review_id)
            request = receive(reader)
            assert request["id"] == start["id"] + 1
            assert request["command"] == {
                "kind": "retire_controller_review", "review_id": start["id"]}
            # A late success does not rewrite the caller's cached unknown observation.
            respond(writer, start, {"kind": "setup_request_reviewed", "outcome": selected()})
            respond(writer, request, control_reply(request, "retired"))
            assert retiring.result(timeout=2) == {"state": "retired"}
            with pytest.raises(TimeoutError):
                review.result()
            close = handle()
            observing = executor.submit(client.observe_close, close)
            request = receive(reader)
            assert request["id"] == start["id"] + 2
            respond(writer, request, {"kind": "close", "observation": observation(close, "closing")})
            assert observing.result(timeout=2).state.value == "closing"
    finally:
        finish(client, reader, writer)


def test_setup_review_lost_response_is_transport_loss_and_never_resends():
    client, reader, writer = pipes()
    try:
        review = begin(client)
        receive(reader)
        writer.close()  # Actual response EOF, not an inferred native refusal.
        with pytest.raises(HostTransportError) as first:
            review.result()
        with pytest.raises(HostTransportError) as repeated:
            review.result()
        assert repeated.value is first.value
        with pytest.raises(HostTransportError):
            client.retire_controller_review(review.review_id)
    finally:
        finish(client, reader, writer)


# Service-parent controls use the existing anonymous-pipe fixture. These inert
# replies do not establish real administrative authorization or native effects.
from meerkat_toolkit.host_control import (
    CurrentLoginDecision, StandingRuleDecision, ConsoleLoginSelection,
    HostControlError, HostReplyError, HostReplyCode,
)


def service_proposal():
    return {
        "operator": {"kind": "fixture"},
        "rule": {"kind": "application_policy_tasks", "routes": ["member-route"]},
        "issuance_id": "parent-1", "deployment": "deployment-1",
        "source_policy_sha256": "a" * 64,
        "original": dataclasses.asdict(submission()[0]),
        "acquisition": {"realm": "realm-1", "owner": "owner-1", "id": "acquisition-1"},
        "instruction_sha256": "b" * 64, "expires_in_ms": 60000,
        "application": {"kind": "fixture"}, "represented_subject": None,
        "decision": {"route": "member-route", "decision": "decision-1"},
        "connector_selection": {"server": "fixture", "connector": None},
        "restrictions": {},
    }


class ServiceGuard:
    def __init__(self, digest="d" * 64):
        self.proposal_sha256 = digest
        self.authorization_ref = "actual-app-decision-1"
        self.provenance = {"rule_revision": "1"}
        self.released = 0
        self.current = True

    def check_current(self):
        if not self.current:
            raise HostReplyError(HostReplyCode.DENIED)

    def release(self):
        self.released += 1


def service_receive(reader):
    # Bound the complete frame, including an absent or partial command. Reading
    # one byte at a time leaves later frames with the existing pipe fixture.
    deadline = time.monotonic() + 2
    line = bytearray()
    while not line.endswith(b"\n"):
        remaining = deadline - time.monotonic()
        assert remaining > 0 and select.select([reader], [], [], remaining)[0], "service command did not finish before the fixture deadline"
        part = os.read(reader.fileno(), 1)
        assert part, "service command pipe closed before a complete frame"
        line.extend(part)
    return json.loads(line)


def service_request(client, reader, writer, executor, proposal=None):
    pending = executor.submit(client.next_service_parent_administration)
    call = service_receive(reader)
    assert call["command"] == {"kind": "next_service_parent_administration"}
    respond(writer, call, {"kind": "service_parent_administration_request",
        "exchange_id": 41, "proposal_sha256": "d" * 64, "proposal": service_proposal() if proposal is None else proposal})
    return pending.result(timeout=2)


def service_observation(outcome="failed_after_effects", *, reason="denied"):
    return {"outcome": outcome, "custody": "joined", "reason": reason,
        "parent_issuance_id": "parent-1", "parent_issued": outcome != "refused_before_entry",
        "child": None, "source": None}


def service_reply(writer, call, suffix, observed=None):
    reply = {"kind": "service_parent_administration_" + suffix, "exchange_id": 41}
    if observed is not None:
        reply["observation"] = observed
    respond(writer, call, reply)


def test_service_parent_next_unavailable_releases_only_unselected_query():
    client, reader, writer = pipes()
    try:
        with ThreadPoolExecutor(max_workers=2) as executor:
            first = executor.submit(client.next_service_parent_administration)
            refused = service_receive(reader)
            respond(writer, refused, {"kind": "error", "error": "unavailable"})
            with pytest.raises(HostReplyError) as error:
                first.result(timeout=2)
            assert error.value.code == HostReplyCode.UNAVAILABLE
            assert client._service_exchange is None
            following = executor.submit(client.next_service_parent_administration)
            call = service_receive(reader)
            assert call["id"] > refused["id"]
            assert call["command"] == refused["command"] == {"kind": "next_service_parent_administration"}
            respond(writer, call, {"kind": "service_parent_administration_request",
                "exchange_id": 41, "proposal_sha256": "d" * 64, "proposal": service_proposal()})
            request = following.result(timeout=2)
            assert request.attempt is None
            retiring = executor.submit(request.retire)
            retire = service_receive(reader)
            assert retire["command"]["kind"] == "retire_service_parent_administration"
            service_reply(writer, retire, "retired", service_observation("refused_before_entry"))
            retiring.result(timeout=2)
            assert not select.select([reader], [], [], 0)[0], "no answer or replay was emitted"
    finally:
        finish(client, reader, writer)


def test_service_parent_next_timeout_retains_the_same_unanswered_query():
    client, reader, writer = pipes()
    try:
        with ThreadPoolExecutor(max_workers=2) as executor:
            first = executor.submit(client.next_service_parent_administration, timeout=0.02)
            call = service_receive(reader)
            with pytest.raises(TimeoutError):
                first.result(timeout=2)
            retained = client._service_exchange
            assert retained is not None
            following = executor.submit(client.next_service_parent_administration)
            assert not select.select([reader], [], [], 0.02)[0], "timeout must not send another Next"
            respond(writer, call, {"kind": "service_parent_administration_request",
                "exchange_id": 41, "proposal_sha256": "d" * 64, "proposal": service_proposal()})
            request = following.result(timeout=2)
            assert request is retained and request.attempt is None
            retiring = executor.submit(request.retire)
            retire = service_receive(reader)
            assert retire["command"]["kind"] == "retire_service_parent_administration"
            service_reply(writer, retire, "retired", service_observation("refused_before_entry"))
            retiring.result(timeout=2)
            assert not select.select([reader], [], [], 0)[0], "only one Next query was sent"
    finally:
        finish(client, reader, writer)


@pytest.mark.parametrize("outcome", ["failed_after_effects", "uncertain", "succeeded"])
def test_service_parent_local_transfer_once_submit_and_joined_outcome(outcome):
    client, reader, writer = pipes()
    try:
        with ThreadPoolExecutor(max_workers=3) as executor:
            request = service_request(client, reader, writer, executor)
            assert client.next_service_parent_administration() is request
            copied = request.proposal
            copied["rule"]["routes"].clear()
            assert request.proposal["rule"]["routes"] == ["member-route"]
            guard = ServiceGuard()
            attempt = request.authorize(StandingRuleDecision(guard))
            assert request.attempt is attempt
            assert attempt.result() == {"state": "not_submitted"}
            assert guard.released == 0
            sending = executor.submit(attempt.submit)
            call = service_receive(reader)
            assert call["command"] == {"kind": "submit_service_parent_administration",
                "exchange_id": 41, "proposal_sha256": "d" * 64, "decision": {
                    "kind": "authorized", "source": {"kind": "standing_rule_decision"},
                    "authorization_ref": guard.authorization_ref, "provenance": guard.provenance}}
            service_reply(writer, call, "submitted")
            assert sending.result(timeout=2) == attempt.submit() == {"state": "answer_forwarded"}
            pending = executor.submit(attempt.result)
            result_call = service_receive(reader)
            assert result_call["command"] == {"kind": "result_service_parent_administration", "exchange_id": 41}
            assert client.next_service_parent_administration() is request
            observed = service_observation(outcome, reason=None if outcome == "succeeded" else "denied")
            service_reply(writer, result_call, "result", observed)
            assert pending.result(timeout=2) == observed
            assert guard.released == 1
            assert request.result() == observed
            assert client.next_service_parent_administration() is request, "Joined result does not retire the exchange"
            retiring = executor.submit(request.retire)
            retire_call = service_receive(reader)
            service_reply(writer, retire_call, "retired", observed)
            assert retiring.result(timeout=2) == attempt.retire() == observed
            assert request.result() == observed and guard.released == 1
            assert client._service_exchange is None
    finally:
        finish(client, reader, writer)


@pytest.mark.parametrize("method, reason", [("requires_human", "requires_human"), ("deny", "denied"), ("unavailable", "unavailable")])
def test_service_parent_negative_answer_has_same_result_and_retirement(method, reason):
    client, reader, writer = pipes()
    try:
        with ThreadPoolExecutor(max_workers=2) as executor:
            request = service_request(client, reader, writer, executor)
            answering = executor.submit(getattr(request, method))
            call = service_receive(reader)
            assert call["command"]["decision"] == {"kind": reason}
            service_reply(writer, call, "submitted")
            answering.result(timeout=2)
            assert request.attempt is None
            observed = service_observation("refused_before_entry", reason=reason)
            pending = executor.submit(request.result)
            call = service_receive(reader)
            service_reply(writer, call, "result", observed)
            assert pending.result(timeout=2) == observed
            retiring = executor.submit(request.retire)
            call = service_receive(reader)
            service_reply(writer, call, "retired", observed)
            assert retiring.result(timeout=2) == observed
    finally:
        finish(client, reader, writer)


def test_service_parent_retire_before_submit_fences_and_retains_guard_until_join():
    client, reader, writer = pipes()
    try:
        with ThreadPoolExecutor(max_workers=2) as executor:
            request = service_request(client, reader, writer, executor)
            guard = ServiceGuard()
            attempt = request.authorize(StandingRuleDecision(guard))
            retiring = executor.submit(attempt.retire, timeout=0.02)
            call = service_receive(reader)
            assert call["command"] == {"kind": "retire_service_parent_administration", "exchange_id": 41}
            with pytest.raises(TimeoutError): retiring.result(timeout=2)
            with pytest.raises(HostControlError, match="retired before submission"): attempt.submit()
            assert guard.released == 0 and client.next_service_parent_administration() is request
            observed = service_observation("refused_before_entry", reason="cancelled")
            service_reply(writer, call, "retired", observed)
            assert attempt.retire() == observed
            assert guard.released == 1
    finally:
        finish(client, reader, writer)


def test_service_parent_lost_ack_and_result_waiter_keep_original_correlations():
    client, reader, writer = pipes()
    try:
        with ThreadPoolExecutor(max_workers=2) as executor:
            request = service_request(client, reader, writer, executor)
            guard = ServiceGuard()
            attempt = request.authorize(StandingRuleDecision(guard))
            sending = executor.submit(attempt.submit, timeout=0.02)
            submitted = service_receive(reader)
            with pytest.raises(TimeoutError): sending.result(timeout=2)
            result = executor.submit(attempt.result, timeout=0.02)
            result_call = service_receive(reader)
            with pytest.raises(TimeoutError): result.result(timeout=2)
            assert guard.released == 0 and request.attempt is attempt
            service_reply(writer, submitted, "submitted")
            observed = service_observation("uncertain", reason="not_settled")
            service_reply(writer, result_call, "result", observed)
            assert attempt.submit() == {"state": "answer_forwarded"}
            assert attempt.result() == observed
            retiring = executor.submit(attempt.retire)
            call = service_receive(reader)
            service_reply(writer, call, "retired", observed)
            assert retiring.result(timeout=2) == observed and guard.released == 1
    finally:
        finish(client, reader, writer)


def test_service_parent_interrupted_authorize_return_retains_attempt_alias():
    client, reader, writer = pipes()
    try:
        with ThreadPoolExecutor(max_workers=2) as executor:
            request = service_request(client, reader, writer, executor)
            guard = ServiceGuard()
            import sys
            previous = sys.gettrace()
            def interrupt_return(frame, event, argument):
                if event == "return" and frame.f_code is request.authorize.__func__.__code__:
                    raise KeyboardInterrupt("interrupt before the caller receives the attempt")
                return interrupt_return
            try:
                sys.settrace(interrupt_return)
                with pytest.raises(KeyboardInterrupt):
                    request.authorize(StandingRuleDecision(guard))
            finally:
                sys.settrace(previous)
            assert request.attempt is not None and request._guard is guard
            retiring = executor.submit(request.retire)
            call = service_receive(reader)
            service_reply(writer, call, "retired", service_observation("refused_before_entry", reason="cancelled"))
            retiring.result(timeout=2)
            assert guard.released == 1
    finally:
        finish(client, reader, writer)


def test_service_parent_response_eof_and_partial_write_do_not_release_guard(monkeypatch):
    import os
    client, reader, writer = pipes()
    try:
        with ThreadPoolExecutor(max_workers=1) as executor:
            request = service_request(client, reader, writer, executor)
        guard = ServiceGuard()
        attempt = request.authorize(StandingRuleDecision(guard))
        actual_write = os.write
        writes = []
        def partial(fd, data):
            writes.append(bytes(data))
            if len(writes) == 1:
                return actual_write(fd, data[:7])
            raise OSError("controlled partial write failure")
        with monkeypatch.context() as local:
            local.setattr(os, "write", partial)
            with pytest.raises(HostTransportError): attempt.submit()
        assert request._answer.correlation is not None
        assert request.attempt is attempt and request._guard is guard and guard.released == 0
        with pytest.raises(HostTransportError): attempt.submit()
        with pytest.raises(HostTransportError): attempt.retire()
        client.close()
        assert reader.read() == writes[0][:7]
        assert request._guard is guard and guard.released == 0
    finally:
        finish(client, reader, writer)


@pytest.mark.parametrize("bad", ["running", "wrong_parent", "extra_field"])
def test_service_parent_malformed_join_never_releases_application_guard(bad):
    client, reader, writer = pipes()
    try:
        with ThreadPoolExecutor(max_workers=2) as executor:
            request = service_request(client, reader, writer, executor)
            guard = ServiceGuard()
            request.authorize(StandingRuleDecision(guard))
            retiring = executor.submit(request.retire)
            call = service_receive(reader)
            observed = service_observation()
            if bad == "running": observed["custody"] = "running"
            elif bad == "wrong_parent": observed["parent_issuance_id"] = "another-parent"
            else: observed["success"] = True
            service_reply(writer, call, "retired", observed)
            with pytest.raises(HostTransportError): retiring.result(timeout=2)
            assert guard.released == 0 and request._guard is guard
    finally:
        finish(client, reader, writer)


@pytest.mark.parametrize("wire_bytes", [12 * 1024, 12 * 1024 + 1])
def test_service_parent_provenance_wire_limit_rejects_before_transfer_or_write(wire_bytes):
    client, reader, writer = pipes()
    guard = ServiceGuard()
    overhead = len(json.dumps({"padding": ""}, separators=(",", ":")).encode("utf-8"))
    guard.provenance = {"padding": "x" * (wire_bytes - overhead)}
    assert len(json.dumps(guard.provenance, separators=(",", ":")).encode("utf-8")) == wire_bytes
    try:
        with ThreadPoolExecutor(max_workers=1) as executor:
            request = service_request(client, reader, writer, executor)
            if wire_bytes > 12 * 1024:
                with pytest.raises(HostControlError, match="provenance is empty or exceeds its bound"):
                    request.authorize(StandingRuleDecision(guard))
                assert request.attempt is None and request._guard is None
                assert guard.released == 0
                assert request.result() == {"state": "not_submitted"}
                assert client.next_service_parent_administration() is request
            else:
                attempt = request.authorize(StandingRuleDecision(guard))
                assert request.attempt is attempt and guard.released == 0
                sending = executor.submit(attempt.submit)
                call = service_receive(reader)
                assert call["command"]["decision"]["provenance"] == guard.provenance
                service_reply(writer, call, "submitted")
                assert sending.result(timeout=2) == {"state": "answer_forwarded"}
                retiring = executor.submit(attempt.retire)
                call = service_receive(reader)
                observed = service_observation("uncertain", reason="not_settled")
                service_reply(writer, call, "retired", observed)
                assert retiring.result(timeout=2) == observed and guard.released == 1
    finally:
        # The rejected boundary leaves no command after the proposal request.
        finish(client, reader, writer)
    assert guard.released == (0 if wire_bytes > 12 * 1024 else 1)


def test_service_parent_current_login_none_refuses_without_transfer_or_write():
    client, reader, writer = pipes()
    guard = ServiceGuard()
    try:
        with ThreadPoolExecutor(max_workers=1) as executor:
            request = service_request(client, reader, writer, executor)
            with pytest.raises(HostControlError, match="acknowledged same-client login"):
                request.authorize(CurrentLoginDecision(guard, None))
            assert request.attempt is None and request._guard is None
            assert guard.released == 0
            assert request.result() == {"state": "not_submitted"}
            assert client.next_service_parent_administration() is request
    finally:
        # finish asserts the command pipe is empty after the one observed
        # proposal request: no answer, fallback, retirement or other write.
        finish(client, reader, writer)
    assert guard.released == 0


def test_service_parent_login_requires_actual_same_client_ack_and_current_guard():
    client, reader, writer = pipes()
    other, other_reader, other_writer = pipes()
    try:
        with ThreadPoolExecutor(max_workers=2) as executor:
            request = service_request(client, reader, writer, executor)
            guard = ServiceGuard()
            enrollment = other.begin_console_login(ConsoleLoginSelection("issuer", "sub", "scope", 9000000000000))
            call = service_receive(other_reader)
            respond(other_writer, call, {"kind": "console_login_enrolled", "enrollment_id": enrollment.enrollment_id})
            enrollment.result()
            with pytest.raises(HostControlError): request.authorize(CurrentLoginDecision(guard, enrollment))
            assert request.attempt is None and guard.released == 0
            with pytest.raises(HostControlError): request.authorize(StandingRuleDecision(ServiceGuard("e" * 64)))
            guard.current = False
            with pytest.raises(HostReplyError): request.authorize(StandingRuleDecision(guard))
            assert request.attempt is None
            guard.current = True
            actual_login = client.begin_console_login(ConsoleLoginSelection("issuer", "sub", "scope", 9000000000000))
            call = service_receive(reader)
            respond(writer, call, {"kind": "console_login_enrolled", "enrollment_id": actual_login.enrollment_id})
            actual_login.result()
            attempt = request.authorize(CurrentLoginDecision(guard, actual_login))
            sending = executor.submit(attempt.submit)
            call = service_receive(reader)
            assert call["command"]["decision"]["source"] == {
                "kind": "current_login_decision", "enrollment_id": actual_login.enrollment_id}
            service_reply(writer, call, "submitted")
            sending.result(timeout=2)
            retiring = executor.submit(request.retire)
            call = service_receive(reader)
            service_reply(writer, call, "retired", service_observation("uncertain", reason="not_settled"))
            retiring.result(timeout=2)
            assert guard.released == 1
    finally:
        finish(client, reader, writer)
        finish(other, other_reader, other_writer)


# Canonical-policy-shaped protocol data only, never a native policy acceptance.
def service_tool_policy(padding=0):
    import hashlib
    canonical = '{"fixture":"' + ("x" * padding if padding else "complete policy including preserved permissions") + '"}\n'
    return {"provider_id": "application", "policy_id": "tools", "member": "assistant:main",
        "selection": {"tools": [{"name": "get_top_hackernews_stories", "consequence": "r0"}], "catalog": "r0"},
        "catalog_session": "00000000-0000-4000-8000-000000000002",
        "prior": {"revision": 1, "digest": "sha256:" + "a" * 64},
        "candidate": {"revision": 2, "digest": "sha256:" + hashlib.sha256(canonical.encode()).hexdigest()},
        "candidate_canonical_json": canonical, "lifetime": "until_superseded"}


@pytest.mark.parametrize("padding", [0, 100_000])
def test_service_parent_tool_policy_requires_explicit_complete_candidate_acknowledgment(padding):
    import hashlib
    client, reader, writer = pipes()
    try:
        with ThreadPoolExecutor(max_workers=2) as executor:
            proposal = service_proposal()
            proposal["tool_policy"] = service_tool_policy(padding)
            if padding:
                frame = json.dumps({"version": 1, "id": 1, "result": {
                    "kind": "service_parent_administration_request", "exchange_id": 41,
                    "proposal_sha256": "d" * 64, "proposal": proposal}}).encode() + b"\n"
                assert 65_536 < len(frame) < 1_048_576
            request = service_request(client, reader, writer, executor, proposal)
            expected = hashlib.sha256(json.dumps(proposal["tool_policy"], sort_keys=True,
                separators=(",", ":"), ensure_ascii=False).encode()).hexdigest()
            assert request.tool_policy_sha256 == expected
            guard = ServiceGuard()
            for approve in (lambda: request.authorize(StandingRuleDecision(guard)),
                            lambda: request.authorize_tool_policy(StandingRuleDecision(guard), "0" * 64)):
                with pytest.raises(HostControlError): approve()
                assert request.attempt is None and guard.released == 0
                assert not select.select([reader], [], [], 0)[0]
            attempt = request.authorize_tool_policy(StandingRuleDecision(guard), expected)
            submitting = executor.submit(attempt.submit)
            call = service_receive(reader)
            assert call["command"]["decision"] == {"kind": "authorized_with_tool_policy",
                "source": {"kind": "standing_rule_decision"}, "authorization_ref": guard.authorization_ref,
                "provenance": guard.provenance, "tool_policy_sha256": expected}
            service_reply(writer, call, "submitted")
            assert submitting.result(timeout=2) == {"state": "answer_forwarded"}
            assert guard.released == 0
            observing = executor.submit(attempt.result)
            call = service_receive(reader)
            observed = service_observation("failed_after_effects", reason="unavailable")
            observed["tool_policy"] = {"proposal_sha256": expected, "candidate": proposal["tool_policy"]["candidate"],
                "native": "accepted", "retained_as_current": False}
            service_reply(writer, call, "result", observed)
            assert observing.result(timeout=2) == observed
            assert guard.released == 1, "only Joined releases guard, partial policy acceptance is not task success"
            retiring = executor.submit(attempt.retire)
            service_reply(writer, service_receive(reader), "retired", observed)
            assert retiring.result(timeout=2) == observed
    finally:
        finish(client, reader, writer)


@pytest.mark.parametrize("revision", [0, 1])
def test_service_parent_policy_observation_rejects_boolean_revision(revision):
    client, reader, writer = pipes()
    try:
        with ThreadPoolExecutor(max_workers=2) as executor:
            proposal = service_proposal()
            proposal["tool_policy"] = service_tool_policy()
            proposal["tool_policy"]["candidate"]["revision"] = revision
            request = service_request(client, reader, writer, executor, proposal)
            guard = ServiceGuard()
            attempt = request.authorize_tool_policy(StandingRuleDecision(guard), request.tool_policy_sha256)
            sending = executor.submit(attempt.submit)
            service_reply(writer, service_receive(reader), "submitted")
            sending.result(timeout=2)
            pending = executor.submit(attempt.result)
            call = service_receive(reader)
            candidate = copy.deepcopy(proposal["tool_policy"]["candidate"])
            candidate["revision"] = bool(revision)
            observed = service_observation("failed_after_effects", reason="unavailable")
            observed["tool_policy"] = {"proposal_sha256": request.tool_policy_sha256,
                "candidate": candidate, "native": "accepted", "retained_as_current": False}
            service_reply(writer, call, "result", observed)
            with pytest.raises(HostTransportError):
                pending.result(timeout=2)
            assert guard.released == 0, "a malformed observation cannot establish joined custody"
    finally:
        finish(client, reader, writer)


@pytest.mark.parametrize("drift", ["digest", "bare_digest", "revision_negative", "revision_string", "revision_unsafe", "lifetime", "catalog_session", "extra"])
def test_service_parent_invalid_tool_policy_projection_does_not_create_attempt(drift):
    client, reader, writer = pipes()
    try:
        with ThreadPoolExecutor(max_workers=1) as executor:
            proposal = service_proposal()
            policy = proposal["tool_policy"] = service_tool_policy()
            if drift == "digest": policy["candidate"]["digest"] = "sha256:" + "0" * 64
            elif drift == "bare_digest": policy["candidate"]["digest"] = policy["candidate"]["digest"][7:]
            elif drift == "revision_negative": policy["candidate"]["revision"] = -1
            elif drift == "revision_string": policy["candidate"]["revision"] = "2"
            elif drift == "revision_unsafe": policy["candidate"]["revision"] = 2**53
            elif drift == "lifetime": policy["lifetime"] = "task"
            elif drift == "catalog_session": policy["catalog_session"] = None
            else: policy["scope"] = "all"
            with pytest.raises(HostTransportError): service_request(client, reader, writer, executor, proposal)
    finally:
        finish(client, reader, writer)
