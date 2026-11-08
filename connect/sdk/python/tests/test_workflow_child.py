"""Actual private-pipe ownership controls, not native workflow/authority proof."""
import dataclasses
from concurrent.futures import ThreadPoolExecutor

import pytest

from meerkat_toolkit.host_control import (
    ApplicationPreparation, HostControlError, HostReplyError, HostTransportError,
    PrivateOriginalRef, PrivateOriginalSubmit, PrivateSourceEvidence,
    SelectedHostArtifact, ToolkitPersistentTransport, WorkflowChildLaunchRequest,
    ConsoleLoginSelection, ConsoleWorkflowChildRequest,
)
from test_host_control import pipes, receive, respond, handle, observation


def selection():
    original = lambda name: PrivateOriginalRef("route", name, "a" * 64)
    return WorkflowChildLaunchRequest(
        parent=original("P"), planning=original("G"),
        submission=PrivateOriginalSubmit(original("A"), "actual child request", PrivateSourceEvidence("event", "1", "b" * 64)),
        child={"realm_id": "realm", "namespace": "work", "item_id": "child"},
        child_owner={"kind": "agent", "id": "actual-child-owner"}, issuance_id="child-root",
        targets=({"target": {"kind": "registered_tool_bundle_operation", "bundle": "meerkat_toolkit.setup_accounts",
                             "operation": "toolkit_request_public_service"}, "issuance_id": "child-effect"},),
        output_audiences=("actual-audience",))


def test_exact_child_preparation_is_snapshotted_and_never_submits():
    client, reader, writer = pipes()
    selected = selection()
    try:
        with ThreadPoolExecutor(max_workers=1) as executor:
            future = executor.submit(client.prepare_workflow_child, selected)
            request = receive(reader)
            assert request["command"] == {"kind": "prepare_workflow_child", "request": {
                **dataclasses.asdict(selected), "targets": list(selected.targets), "output_audiences": list(selected.output_audiences)}}
            selected.child["item_id"] = "changed-after-write"
            selected.targets[0]["issuance_id"] = "changed-after-write"
            respond(writer, request, {"kind": "workflow_child_prepared", "original": dataclasses.asdict(selected.submission.original)})
            assert future.result(timeout=2) == selected.submission.original
    finally:
        client.close()
        assert reader.read() == b""
        reader.close(); writer.close()


@pytest.mark.parametrize("mutation", ["original", "digest", "kind", "extra"])
def test_foreign_or_overstated_reply_fails_channel_not_success(mutation):
    client, reader, writer = pipes(); selected = selection()
    try:
        with ThreadPoolExecutor(max_workers=1) as executor:
            future = executor.submit(client.prepare_workflow_child, selected)
            request = receive(reader)
            result = {"kind": "workflow_child_prepared", "original": dataclasses.asdict(selected.submission.original)}
            if mutation == "original": result["original"]["original"] = "other"
            elif mutation == "digest": result["original"]["event_sha256"] = "c" * 64
            elif mutation == "kind": result["kind"] = "original_submitted"
            else: result["admitted"] = True
            respond(writer, request, result)
            with pytest.raises(HostControlError): future.result(timeout=2)
            with pytest.raises(HostTransportError): client.prepare_workflow_child(selected)
    finally:
        client.close(); assert reader.read() == b""; reader.close(); writer.close()


def test_empty_native_selection_preserved_and_refusal_does_not_poison_channel():
    client, reader, writer = pipes(); selected = dataclasses.replace(selection(), targets=(), output_audiences=())
    try:
        with ThreadPoolExecutor(max_workers=1) as executor:
            future = executor.submit(client.prepare_workflow_child, selected)
            request = receive(reader)
            assert request["command"]["request"]["targets"] == []
            assert request["command"]["request"]["output_audiences"] == []
            respond(writer, request, {"kind": "error", "error": "denied"})
            with pytest.raises(HostReplyError) as failure: future.result(timeout=2)
            assert failure.value.code == "denied"
            # Independent read proves the typed refusal did not close this pipe.
            close = handle()
            observing = executor.submit(client.observe_close, close)
            request = receive(reader)
            respond(writer, request, {"kind": "close", "observation": observation(close, "closing")})
            assert observing.result(timeout=2).state == "closing"
    finally:
        client.close(); assert reader.read() == b""; reader.close(); writer.close()


@pytest.mark.parametrize("code", ["denied", "unavailable", "observation_changed"])
def test_child_refusal_is_exact_and_never_retries_or_submits(code):
    client, reader, writer = pipes()
    try:
        with ThreadPoolExecutor(max_workers=1) as executor:
            pending = executor.submit(client.prepare_workflow_child, selection())
            request = receive(reader)
            respond(writer, request, {"kind": "error", "error": code})
            with pytest.raises(HostReplyError) as failure:
                pending.result(timeout=2)
            assert failure.value.code.value == code
            close = handle()
            observing = executor.submit(client.observe_close, close)
            following = receive(reader)
            assert following["id"] == request["id"] + 1
            assert following["command"]["kind"] == "observe_close"
            respond(writer, following, {"kind": "close", "observation": observation(close, "closing")})
            assert observing.result(timeout=2).state == "closing"
    finally:
        client.close(); assert reader.read() == b""; reader.close(); writer.close()


def test_timeout_drains_exact_late_reply_without_replay_or_submission():
    client, reader, writer = pipes(); selected = selection()
    try:
        with ThreadPoolExecutor(max_workers=1) as executor:
            future = executor.submit(client.prepare_workflow_child, selected, timeout=.02)
            request = receive(reader)
            with pytest.raises(TimeoutError, match="settlement is unknown"): future.result(timeout=2)
            respond(writer, request, {"kind": "workflow_child_prepared", "original": dataclasses.asdict(selected.submission.original)})
            close = handle()
            observing = executor.submit(client.observe_close, close)
            next_request = receive(reader)
            assert next_request["id"] == request["id"] + 1
            assert next_request["command"]["kind"] == "observe_close"
            respond(writer, next_request, {"kind": "close", "observation": observation(close, "closing")})
            assert observing.result(timeout=2).state == "closing"
    finally:
        client.close(); assert reader.read() == b""; reader.close(); writer.close()


def test_pipe_loss_is_unknown_without_automatic_cleanup_or_replay():
    client, reader, writer = pipes()
    try:
        with ThreadPoolExecutor(max_workers=1) as executor:
            future = executor.submit(client.prepare_workflow_child, selection())
            assert receive(reader)["command"]["kind"] == "prepare_workflow_child"
            writer.close()
            with pytest.raises(HostTransportError): future.result(timeout=2)
    finally:
        client.close(); assert reader.read() == b""; reader.close(); writer.close()


@pytest.mark.parametrize("change", ["child_extra", "owner", "grant", "audience"])
def test_invalid_wire_shape_refuses_before_any_write(change):
    client, reader, writer = pipes(); selected = selection()
    if change == "child_extra": selected.child["permission"] = True
    elif change == "owner": selected.child_owner["kind"] = "invented"
    elif change == "grant": selected.targets[0]["grant"] = {"authorized": True}
    else: selected = dataclasses.replace(selected, output_audiences=(True,))
    try:
        with pytest.raises(HostControlError): client.prepare_workflow_child(selected)
    finally:
        client.close(); assert reader.read() == b""; reader.close(); writer.close()


@pytest.mark.parametrize("enabled", [False, True])
def test_workflow_launch_is_explicit_fresh_transport_selection(enabled, monkeypatch):
    from meerkat_meta._transport import PersistentTransport
    preparation = ApplicationPreparation(SelectedHostArtifact("/selected/plan", "d" * 64), 2)
    observed = []
    def before_spawn(self):
        observed.append(self._extra_args)
        raise RuntimeError("inert pre-spawn stop")
    monkeypatch.setattr(PersistentTransport, "start", before_spawn)
    transport = ToolkitPersistentTransport("/not-started", preparation, private_workflow_launch=enabled)
    with pytest.raises(RuntimeError, match="inert pre-spawn"): transport.start()
    assert observed[0].count("--private-workflow-launch") == int(enabled)
    with pytest.raises(HostTransportError, match="already started"): transport.start()
    assert len(observed) == 1


def test_workflow_launch_refuses_nonboolean_and_nonfresh_selection():
    from test_application_preparation import selection as bootstrap
    for selected, enabled in ((bootstrap(), True), (bootstrap(), "true")):
        with pytest.raises(HostControlError, match="explicit fresh"):
            ToolkitPersistentTransport("/not-started", selected, private_workflow_launch=enabled)


def console_selection():
    selected = selection()
    return ConsoleWorkflowChildRequest(selected.parent, selected.planning, "route", "one-send", "b" * 64,
        selected.child, selected.child_owner, selected.issuance_id, selected.targets, selected.output_audiences)


def enrolled(client, reader, writer):
    login = client.begin_console_login(ConsoleLoginSelection("issuer", "subject", "scope", 9_000_000_000_000))
    request = receive(reader)
    respond(writer, request, {"kind": "console_login_enrolled", "enrollment_id": login.enrollment_id})
    login.result()
    return login


def test_console_proposal_snapshots_selectors_and_observes_same_source_without_submitting():
    client, reader, writer = pipes()
    selected = console_selection()
    expected = selected.wire()
    expected["targets"] = list(expected["targets"])
    expected["output_audiences"] = list(expected["output_audiences"])
    original = PrivateOriginalRef("route", "actual-native-input", "c" * 64)
    try:
        login = enrolled(client, reader, writer)
        with ThreadPoolExecutor(max_workers=1) as executor:
            for observation_value in (None, original, original):
                future = executor.submit(client.prepare_console_workflow_child, selected, login)
                request = receive(reader)
                assert request["command"] == {"kind": "prepare_console_workflow_child",
                    "request": expected, "enrollment_id": login.enrollment_id}
                # Changing the caller's mutable nested object cannot change a
                # command already written to the private transport.
                selected.child["item_id"] = "mutated-after-write"
                assert request["command"]["request"]["child"]["item_id"] == "child"
                selected.child["item_id"] = "child"
                respond(writer, request, {"kind": "console_workflow_child_selected",
                    "original": None if observation_value is None else dataclasses.asdict(observation_value)})
                assert future.result(timeout=2) == observation_value
    finally:
        client.close(); assert reader.read() == b""; reader.close(); writer.close()


@pytest.mark.parametrize("mutation", ["route", "digest", "extra", "kind"])
def test_console_proposal_rejects_foreign_or_overstated_source_reply(mutation):
    client, reader, writer = pipes()
    try:
        login = enrolled(client, reader, writer)
        with ThreadPoolExecutor(max_workers=1) as executor:
            future = executor.submit(client.prepare_console_workflow_child, console_selection(), login)
            request = receive(reader)
            reply = {"kind": "console_workflow_child_selected", "original":
                dataclasses.asdict(PrivateOriginalRef("route", "actual-native-input", "c" * 64))}
            if mutation == "route": reply["original"]["route"] = "foreign"
            elif mutation == "digest": reply["original"]["event_sha256"] = "invalid"
            elif mutation == "extra": reply["admitted"] = True
            else: reply["kind"] = "original_submitted"
            respond(writer, request, reply)
            with pytest.raises(HostControlError): future.result(timeout=2)
            with pytest.raises(HostTransportError): client.prepare_console_workflow_child(console_selection(), login)
    finally:
        client.close(); assert reader.read() == b""; reader.close(); writer.close()


def test_console_proposal_requires_acknowledged_same_client_and_valid_shape_before_write():
    client, reader, writer = pipes()
    other, other_reader, other_writer = pipes()
    try:
        login = client.begin_console_login(ConsoleLoginSelection("issuer", "subject", "scope", 9_000_000_000_000))
        enrollment = receive(reader)
        with pytest.raises(HostControlError): client.prepare_console_workflow_child(console_selection(), login)
        respond(writer, enrollment, {"kind": "console_login_enrolled", "enrollment_id": login.enrollment_id})
        login.result()
        with pytest.raises(HostControlError): other.prepare_console_workflow_child(console_selection(), login)
        for selected in (dataclasses.replace(console_selection(), send_key=""),
                         dataclasses.replace(console_selection(), content_sha256="invalid"),
                         dataclasses.replace(console_selection(), output_audiences=(True,))):
            with pytest.raises(HostControlError): client.prepare_console_workflow_child(selected, login)
    finally:
        client.close(); other.close()
        assert reader.read() == other_reader.read() == b""
        reader.close(); writer.close(); other_reader.close(); other_writer.close()


def test_console_proposal_refusal_and_late_observation_preserve_channel_without_retry():
    client, reader, writer = pipes()
    try:
        login = enrolled(client, reader, writer)
        with ThreadPoolExecutor(max_workers=1) as executor:
            selected = console_selection()
            future = executor.submit(client.prepare_console_workflow_child, selected, login, timeout=.02)
            request = receive(reader)
            with pytest.raises(TimeoutError): future.result(timeout=2)
            respond(writer, request, {"kind": "console_workflow_child_selected", "original": None})
            following = executor.submit(client.prepare_console_workflow_child, selected, login)
            next_request = receive(reader)
            assert next_request["id"] == request["id"] + 1
            assert next_request["command"] == request["command"]
            respond(writer, next_request, {"kind": "error", "error": "denied"})
            with pytest.raises(HostReplyError) as failure: following.result(timeout=2)
            assert failure.value.code.value == "denied"
            close = handle()
            observation_future = executor.submit(client.observe_close, close)
            next_request = receive(reader)
            assert next_request["command"]["kind"] == "observe_close"
            respond(writer, next_request, {"kind": "close", "observation": observation(close, "closing")})
            assert observation_future.result(timeout=2).state == "closing"
    finally:
        client.close(); assert reader.read() == b""; reader.close(); writer.close()


def test_retired_console_enrollment_can_observe_locator_but_native_refusal_remains_exact():
    client, reader, writer = pipes()
    try:
        login = enrolled(client, reader, writer)
        with ThreadPoolExecutor(max_workers=1) as executor:
            retirement = executor.submit(login.retire)
            request = receive(reader)
            respond(writer, request, {"kind": "console_login_retired", "enrollment_id": login.enrollment_id,
                "observation": {"fenced": True, "reservations_drained": True, "uncertain": False}})
            assert retirement.result(timeout=2).fenced
            original = PrivateOriginalRef("route", "claimed-before-partial-failure", "c" * 64)
            for reply in ({"kind": "console_workflow_child_selected", "original": dataclasses.asdict(original)},
                          {"kind": "error", "error": "denied"}):
                pending = executor.submit(client.prepare_console_workflow_child, console_selection(), login)
                request = receive(reader)
                assert request["command"]["enrollment_id"] == login.enrollment_id
                assert request["command"]["kind"] == "prepare_console_workflow_child"
                respond(writer, request, reply)
                if reply["kind"] == "error":
                    with pytest.raises(HostReplyError) as failure: pending.result(timeout=2)
                    assert failure.value.code.value == "denied"
                else:
                    assert pending.result(timeout=2) == original
    finally:
        client.close(); assert reader.read() == b""; reader.close(); writer.close()
