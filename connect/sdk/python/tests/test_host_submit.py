"""Private-pipe submission protocol only; no native admission or completion claim."""
from __future__ import annotations

import dataclasses
import json
import os
import select
import time
from concurrent.futures import ThreadPoolExecutor

import pytest

from meerkat_toolkit.host_control import HostControlError, HostCloseState, PrivateSourceEvidence, HostReplyError
from test_host_control import pipes, receive, respond, handle, observation, original_for, source_event


def submission():
    return original_for(source_event()), "Latest messages.\nRäksmörgås 😀", PrivateSourceEvidence("transport", "4", "c" * 64)


def admitted(original):
    return {"state": "admitted", "session_id": "native-session", "input_id": "input-" + original.original,
            "deduplicated": False, "turn_ticket": "00000000-0000-0000-0000-000000000003",
            "source_settlement": "fresh_acceptance_promoted"}


def finish(client, reader, writer):
    client.close()
    assert not client._reader.is_alive()
    assert reader.read() == b""  # No hidden resend or new command after the observed request.
    reader.close()
    writer.close()


@pytest.mark.parametrize("state", ["fresh", "deduplicated", "publication_failed", "unresolved", "unavailable"])
def test_submit_exact_wire_preserves_admission_source_settlement_and_uncertainty(state):
    client, reader, writer = pipes()
    original, content, evidence = submission()
    outcome = admitted(original)
    if state == "deduplicated":
        outcome.update(deduplicated=True, turn_ticket=None, source_settlement="retry_withdrawn")
    elif state == "publication_failed":
        outcome["source_settlement"] = "post_admission_publication_failed"
    elif state == "unresolved":
        outcome = {"state": "admission_unresolved"}
    elif state == "unavailable":
        outcome = {"state": "native_outcome_unavailable"}
    try:
        with ThreadPoolExecutor(max_workers=1) as executor:
            pending = executor.submit(client.submit_authenticated_original, original, content, evidence)
            request = receive(reader)
            assert request == {"version": 1, "id": 1, "command": {"kind": "submit_authenticated_original", "submission": {
                "original": dataclasses.asdict(original), "content": content,
                "transport_authentication": dataclasses.asdict(evidence)}}}
            respond(writer, request, {"kind": "original_submitted", "original": dataclasses.asdict(original), "outcome": outcome})
            assert pending.result(timeout=2) == outcome
    finally:
        finish(client, reader, writer)


@pytest.mark.parametrize("malformation", ["wrong_original", "completion_field", "unknown_state", "invalid_ticket"])
def test_submit_rejects_mismatched_or_untyped_reply_without_resend(malformation):
    client, reader, writer = pipes()
    original, content, evidence = submission()
    returned = dataclasses.asdict(original)
    outcome = admitted(original)
    if malformation == "wrong_original":
        returned["original"] = "somebody-else"
    elif malformation == "completion_field":
        outcome["completed"] = True
    elif malformation == "unknown_state":
        outcome = {"state": "completed"}
    else:
        outcome["turn_ticket"] = "invalid"
    try:
        with ThreadPoolExecutor(max_workers=1) as executor:
            pending = executor.submit(client.submit_authenticated_original, original, content, evidence)
            request = receive(reader)
            respond(writer, request, {"kind": "original_submitted", "original": returned, "outcome": outcome})
            with pytest.raises(HostControlError, match="settlement is unknown"):
                pending.result(timeout=2)
        with pytest.raises(HostControlError):
            client.submit_authenticated_original(original, content, evidence)
    finally:
        finish(client, reader, writer)


def test_submit_timeout_drains_late_reply_without_reissuing_work():
    client, reader, writer = pipes()
    original, content, evidence = submission()
    try:
        with ThreadPoolExecutor(max_workers=1) as executor:
            pending = executor.submit(client.submit_authenticated_original, original, content, evidence, timeout=0.05)
            request = receive(reader)
            with pytest.raises(TimeoutError, match="settlement is unknown"):
                pending.result(timeout=2)
            respond(writer, request, {"kind": "original_submitted", "original": dataclasses.asdict(original), "outcome": admitted(original)})
            close = handle()
            next_pending = executor.submit(client.observe_close, close)
            next_request = receive(reader)
            assert next_request["id"] == request["id"] + 1
            assert next_request["command"]["kind"] == "observe_close"
            respond(writer, next_request, {"kind": "close", "observation": observation(close, "closing")})
            assert next_pending.result(timeout=2).state == HostCloseState.CLOSING
    finally:
        finish(client, reader, writer)


def test_submit_eof_is_unknown_and_does_not_resend():
    client, reader, writer = pipes()
    original, content, evidence = submission()
    try:
        with ThreadPoolExecutor(max_workers=1) as executor:
            pending = executor.submit(client.submit_authenticated_original, original, content, evidence)
            receive(reader)
            writer.close()
            with pytest.raises(HostControlError, match="settlement is unknown"):
                pending.result(timeout=2)
        with pytest.raises(HostControlError):
            client.submit_authenticated_original(original, content, evidence)
    finally:
        finish(client, reader, writer)


def test_submit_concurrent_originals_keep_native_input_correlation():
    client, reader, writer = pipes()
    first, content, evidence = submission()
    second = dataclasses.replace(first, original="second-original", event_sha256="d" * 64)
    try:
        with ThreadPoolExecutor(max_workers=2) as executor:
            first_pending = executor.submit(client.submit_authenticated_original, first, content, evidence)
            first_request = receive(reader)
            second_pending = executor.submit(client.submit_authenticated_original, second, content, evidence)
            second_request = receive(reader)
            assert first_request["id"] != second_request["id"]
            respond(writer, second_request, {"kind": "original_submitted", "original": dataclasses.asdict(second), "outcome": admitted(second)})
            respond(writer, first_request, {"kind": "original_submitted", "original": dataclasses.asdict(first), "outcome": admitted(first)})
            assert first_pending.result(timeout=2) == admitted(first)
            assert second_pending.result(timeout=2) == admitted(second)
    finally:
        finish(client, reader, writer)


def receipt_receive(reader):
    # Bound the complete frame, including an absent or partial command. Reading
    # one byte at a time leaves later frames with the existing pipe fixture.
    deadline = time.monotonic() + 2
    line = bytearray()
    while not line.endswith(b"\n"):
        remaining = deadline - time.monotonic()
        assert remaining > 0 and select.select([reader], [], [], remaining)[0], "receipt command did not finish before the fixture deadline"
        part = os.read(reader.fileno(), 1)
        assert part, "receipt command pipe closed before a complete frame"
        line.extend(part)
    return json.loads(line)


def service_receipt(case):
    """Protocol snapshots only; these values are not native completion evidence."""
    if case == "unresolved":
        return {"state": "admission_unresolved"}
    observed = {"state": "admitted", "session_id": "session-b", "input_id": "input-b", "receipt": None}
    if case == "absent":
        return observed
    report = {"status": "pending", "input_id": "input-b", "phase": "consumed",
              "terminal": {"outcome_type": "consumed"}, "last_run_id": "historical-run", "attempt_count": 2}
    if case == "without":
        del report["phase"]
        report["status"] = "terminal_without_receipt"
    elif case in {"batch", "finalization_failure", "terminated"}:
        report = {"status": "finalized", "input_id": "input-b", "terminal": {"outcome_type": "consumed"},
                  "attempt_count": 2, "scope": {"kind": "run", "run_id": "run-b"}, "run_id": "run-b",
                  "owner_input_id": "input-a", "recipient_input_ids": ["input-a", "input-b"],
                  "outcome": {"completion_type": "completed_without_result"}}
        if case == "finalization_failure":
            report["outcome"] = {"completion_type": "completed_with_finalization_failure", "error": {"fixture_payload": "preserved"}}
        elif case == "terminated":
            report.update(scope={"kind": "runtime_termination"}, run_id=None, outcome={"completion_type": "cancelled"})
    observed["receipt"] = {"source": "durable_store", "report": report}
    return observed


@pytest.mark.parametrize("case", ["unresolved", "absent", "pending", "without", "batch", "finalization_failure", "terminated"])
def test_service_receipt_read_preserves_native_states_witness_batch_and_outcome(case):
    client, reader, writer = pipes()
    original, _, _ = submission()
    observed = service_receipt(case)
    try:
        with ThreadPoolExecutor(max_workers=1) as executor:
            pending = executor.submit(client.observe_service_input_receipt, original)
            request = receipt_receive(reader)
            assert request["command"] == {"kind": "observe_service_input_receipt", "original": dataclasses.asdict(original)}
            respond(writer, request, {"kind": "service_input_receipt", "original": dataclasses.asdict(original), "observation": observed})
            assert pending.result(timeout=2) == observed
    finally:
        finish(client, reader, writer)


@pytest.mark.parametrize("case", ["original", "input", "owner", "scope", "witness", "status", "extra", "count"])
def test_service_receipt_rejects_mismatched_or_invented_projection(case):
    client, reader, writer = pipes()
    original, _, _ = submission()
    returned = dataclasses.asdict(original)
    observed = service_receipt("batch")
    report = observed["receipt"]["report"]
    if case == "original":
        returned["original"] = "another"
    elif case == "input":
        report["input_id"] = "another"
    elif case == "owner":
        report["owner_input_id"] = "input-b"
    elif case == "scope":
        report["scope"]["run_id"] = "another"
    elif case == "witness":
        observed["receipt"]["source"] = "model_text"
    elif case == "status":
        report["status"] = "completed"
    elif case == "extra":
        observed["success"] = True
    else:
        report["attempt_count"] = True
    try:
        with ThreadPoolExecutor(max_workers=1) as executor:
            pending = executor.submit(client.observe_service_input_receipt, original)
            request = receipt_receive(reader)
            respond(writer, request, {"kind": "service_input_receipt", "original": returned, "observation": observed})
            with pytest.raises(HostControlError, match="receipt response was rejected"):
                pending.result(timeout=2)
    finally:
        finish(client, reader, writer)


def test_unavailable_service_receipt_does_not_close_shared_channel():
    client, reader, writer = pipes()
    original, _, _ = submission()
    try:
        with ThreadPoolExecutor(max_workers=1) as executor:
            pending = executor.submit(client.observe_service_input_receipt, original)
            request = receipt_receive(reader)
            respond(writer, request, {"kind": "error", "error": "unavailable"})
            with pytest.raises(HostReplyError):
                pending.result(timeout=2)
            close = handle()
            following = executor.submit(client.observe_close, close)
            follow = receipt_receive(reader)
            assert follow["id"] == request["id"] + 1
            respond(writer, follow, {"kind": "close", "observation": observation(close, "closing")})
            assert following.result(timeout=2).state == HostCloseState.CLOSING
    finally:
        finish(client, reader, writer)


def test_service_receipt_timeout_drains_late_read_without_hidden_resubmission():
    client, reader, writer = pipes()
    original, _, _ = submission()
    try:
        with ThreadPoolExecutor(max_workers=1) as executor:
            pending = executor.submit(client.observe_service_input_receipt, original, timeout=0.05)
            request = receipt_receive(reader)
            with pytest.raises(TimeoutError):
                pending.result(timeout=2)
            respond(writer, request, {"kind": "service_input_receipt", "original": dataclasses.asdict(original), "observation": service_receipt("pending")})
            close = handle()
            following = executor.submit(client.observe_close, close)
            follow = receipt_receive(reader)
            assert follow["id"] == request["id"] + 1 and follow["command"]["kind"] == "observe_close"
            respond(writer, follow, {"kind": "close", "observation": observation(close, "closing")})
            assert following.result(timeout=2).state == HostCloseState.CLOSING
    finally:
        finish(client, reader, writer)
