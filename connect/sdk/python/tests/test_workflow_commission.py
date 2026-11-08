"""Real pipe/listing ownership with synthetic replies, not native G acceptance."""
import asyncio
import dataclasses
import copy
import json

import pytest

from meerkat_toolkit.host_control import (
    HostControlError, HostReplyError, HostTransportError, PrivateAuthenticatedEvent,
    PrivateOriginalSubmit, PrivateSourceEvidence, PrivateOriginalRef,
)
from meerkat_toolkit.member_classes import MemberClassListingUnknown, MemberClassObservationTimeout
from meerkat_toolkit.workflow_commission import WorkflowCommissionRequest
from test_host_control import pipes, receive, respond
from test_member_classes import selection as ordinary_selection


def selection():
    evidence = PrivateSourceEvidence("event", "1", "a" * 64)
    return WorkflowCommissionRequest("admit-G", "project-one",
        PrivateOriginalSubmit(PrivateOriginalRef("route-P", "P", "b" * 64), "parent", evidence),
        ordinary_selection().instance,
        PrivateAuthenticatedEvent("route-G", "G", "occurrence", evidence, evidence, ("P",), "c" * 64),
        "actual G input", "workflow", {"realm_id": "realm", "namespace": "work", "item_id": "parent"},
        {"kind": "agent", "id": "parent-owner"}, "cycle", 1, 99999, 88888, 2, 60000, "controller", "root", "leaf")


def native():
    return {"state": "settled", "native": {"state": "result", "result": {
        "outcomes": {"project-one": {"restore": {"stage": "minted"}, "detail": -0.5}}, "managed_edges": []}},
        "authorization": {"state": "succeeded"}, "cleanup": {"state": "succeeded"}}


def outcome(request):
    return {"state": "prepared", "parent": dataclasses.asdict(request.parent.original),
            "planning": {"route": "route-G", "original": "G", "event_sha256": "d" * 64},
            "admission_id": request.admission_id, "native": native()}


def listing(writer, command, request):
    respond(writer, command, {"kind": "member_class_listing_required", "operation_id": command["id"],
        "admission_id": request.admission_id, "identity": request.identity, "class": request.instance["class"]})


def acknowledge(writer, ack):
    operation = ack["command"]["operation"]
    respond(writer, ack, {"kind": "member_class_listing_acknowledged", **{key: operation[key]
        for key in ("operation_id", "admission_id", "identity", "class")}})


@pytest.mark.asyncio
async def test_actual_listing_must_commit_before_ack_and_prepared_is_exact():
    client, reader, writer = pipes(); request = selection(); entered = asyncio.Event(); release = asyncio.Event()
    async def publish(): entered.set(); await release.wait()
    try:
        attempt = client.begin_workflow_commission(request, publish=publish)
        command = await asyncio.to_thread(receive, reader)
        assert command["command"]["kind"] == "commission_workflow"
        assert command["command"]["request"]["admission_id"] == request.admission_id
        request.instance["class"] = "changed-after-write"
        listing(writer, command, dataclasses.replace(request, instance=ordinary_selection().instance))
        await entered.wait(); release.set()
        ack = await asyncio.to_thread(receive, reader)
        assert ack["command"]["operation"]["accepted"] is True
        assert ack["command"]["operation"]["class"] == "project"
        acknowledge(writer, ack); await attempt.join_listing()
        response = outcome(request); respond(writer, command, {"kind": "workflow_commissioned", "outcome": response})
        assert await asyncio.to_thread(attempt.result) == response
        assert await asyncio.to_thread(attempt.result) == response
    finally:
        client.close(); assert reader.read() == b""; reader.close(); writer.close()


@pytest.mark.asyncio
@pytest.mark.parametrize("state", ["native", "created_without_planning", "add_not_accepted", "preparation_refused"])
async def test_distinct_failure_and_created_outcomes_are_not_upgraded(state):
    client, reader, writer = pipes(); request = selection(); calls = []
    async def publish(): calls.append(1)
    try:
        attempt = client.begin_workflow_commission(request, publish=publish)
        command = await asyncio.to_thread(receive, reader)
        response = {"native": {"state": "native", "outcome": {"state": "preflight_refused", "error": "denied", "cleanup": {"state": "succeeded"}}},
            "created_without_planning": {**outcome(request), "state": state, "error": "unavailable"},
            "add_not_accepted": {"state": state, "native": {"state": "reservation_not_accepted", "error": "denied"}, "retirement": {"state": "refused", "error": "not_settled"}},
            "preparation_refused": {"state": state, "error": "unavailable", "retirement": {"state": "succeeded"}, "physical_cleanup": None}}[state]
        respond(writer, command, {"kind": "workflow_commissioned", "outcome": response})
        assert await asyncio.to_thread(attempt.result) == response
        await attempt.join_listing(); assert calls == []
    finally:
        client.close(); assert reader.read() == b""; reader.close(); writer.close()


@pytest.mark.asyncio
@pytest.mark.parametrize("mutation", ["parent", "planning", "admission", "native_identity", "cleanup", "extra"])
async def test_mismatched_or_overstated_result_closes_channel(mutation):
    client, reader, writer = pipes(); request = selection()
    async def publish(): pass
    try:
        attempt = client.begin_workflow_commission(request, publish=publish)
        command = await asyncio.to_thread(receive, reader); response = outcome(request)
        if mutation == "parent": response["parent"]["original"] = "other"
        elif mutation == "planning": response["planning"]["route"] = "other"
        elif mutation == "admission": response["admission_id"] = "other"
        elif mutation == "native_identity": response["native"]["native"]["result"]["outcomes"] = {"other": {}}
        elif mutation == "cleanup": response["native"]["cleanup"] = {"state": "refused", "error": "not_settled"}
        else: response["completed"] = True
        respond(writer, command, {"kind": "workflow_commissioned", "outcome": response})
        with pytest.raises(HostTransportError): await asyncio.to_thread(attempt.result)
        with pytest.raises(HostTransportError): client.begin_workflow_commission(request, publish=publish)
    finally:
        client.close(); reader.close(); writer.close()


@pytest.mark.asyncio
async def test_timeout_does_not_cancel_entered_listing_or_repeat_command():
    client, reader, writer = pipes(); request = selection(); entered = asyncio.Event(); release = asyncio.Event(); writes = []
    async def publish(): writes.append(1); entered.set(); await release.wait()
    try:
        attempt = client.begin_workflow_commission(request, publish=publish, timeout=.03)
        command = await asyncio.to_thread(receive, reader); listing(writer, command, request); await entered.wait()
        with pytest.raises(MemberClassObservationTimeout) as error: await asyncio.to_thread(attempt.result)
        assert error.value.attempt is attempt
        joining = asyncio.create_task(attempt.join_listing()); await asyncio.sleep(0); joining.cancel()
        with pytest.raises(asyncio.CancelledError): await joining
        release.set(); ack = await asyncio.to_thread(receive, reader); acknowledge(writer, ack)
        await attempt.join_listing(); assert writes == [1]
        respond(writer, command, {"kind": "workflow_commissioned", "outcome": outcome(request)})
    finally:
        client.close(); assert reader.read() == b""; reader.close(); writer.close()


@pytest.mark.asyncio
async def test_typed_native_refusal_stays_local_and_same_id_replay_is_not_retried():
    client, reader, writer = pipes(); request = selection()
    async def publish(): raise AssertionError("unentered")
    try:
        for _ in range(2):
            attempt = client.begin_workflow_commission(request, publish=publish)
            command = await asyncio.to_thread(receive, reader)
            respond(writer, command, {"kind": "error", "error": "denied"})
            with pytest.raises(HostReplyError): await asyncio.to_thread(attempt.result)
            await attempt.join_listing()
    finally:
        client.close(); assert reader.read() == b""; reader.close(); writer.close()


@pytest.mark.asyncio
async def test_pipe_loss_joins_entered_write_and_reports_unknown_ack():
    client, reader, writer = pipes(); request = selection(); entered = asyncio.Event(); release = asyncio.Event(); done = []
    async def publish(): entered.set(); await release.wait(); done.append(1)
    try:
        attempt = client.begin_workflow_commission(request, publish=publish)
        command = await asyncio.to_thread(receive, reader); listing(writer, command, request); await entered.wait()
        writer.close()
        with pytest.raises(HostTransportError): await asyncio.to_thread(attempt.result)
        assert not done; release.set()
        with pytest.raises(MemberClassListingUnknown): await attempt.join_listing()
        assert done == [1]
    finally:
        client.close(); reader.close(); writer.close()


@pytest.mark.asyncio
@pytest.mark.parametrize("change", ["authority", "number", "instance_extra"])
async def test_invalid_request_never_writes(change):
    client, reader, writer = pipes(); request = selection()
    if change == "authority": request.parent_work["grant"] = "invented"
    elif change == "number": request = dataclasses.replace(request, max_waiters=True)
    else: request.instance["grant"] = "invented"
    try:
        with pytest.raises(HostControlError): client.begin_workflow_commission(request, publish=lambda: asyncio.sleep(0))
    finally:
        client.close(); assert reader.read() == b""; reader.close(); writer.close()

@pytest.mark.asyncio
async def test_async_convenience_sends_once_and_preserves_native_refusal():
    client, reader, writer = pipes()
    try:
        task = asyncio.create_task(client.commission_workflow(selection(), publish=lambda: asyncio.sleep(0)))
        command = await asyncio.to_thread(receive, reader)
        respond(writer, command, {"kind": "error", "error": "denied"})
        with pytest.raises(HostReplyError): await task
    finally:
        client.close(); assert reader.read() == b""; reader.close(); writer.close()

@pytest.mark.asyncio
async def test_late_typed_refusal_after_timeout_releases_unentered_listing():
    client, reader, writer = pipes()
    try:
        attempt = client.begin_workflow_commission(selection(), publish=lambda: asyncio.sleep(0), timeout=.02)
        command = await asyncio.to_thread(receive, reader)
        with pytest.raises(MemberClassObservationTimeout): await asyncio.to_thread(attempt.result)
        respond(writer, command, {"kind": "error", "error": "denied"})
        await asyncio.wait_for(attempt.join_listing(), 1)
        assert attempt.operation_id not in client._member_listings
    finally:
        client.close(); assert reader.read() == b""; reader.close(); writer.close()

@pytest.mark.asyncio
async def test_unsendable_listing_ack_wakes_native_waiter_through_existing_pipe_failure(monkeypatch):
    client, reader, writer = pipes(); request = selection()
    try:
        attempt = client.begin_workflow_commission(request, publish=lambda: asyncio.sleep(0))
        command = await asyncio.to_thread(receive, reader)
        original_start = client._start_call
        def start(command, **options):
            if options.get("_listing_ack"): raise HostControlError("reserved slots full")
            return original_start(command, **options)
        monkeypatch.setattr(client, "_start_call", start)
        listing(writer, command, request)
        with pytest.raises(MemberClassListingUnknown): await attempt.join_listing()
        with pytest.raises(HostTransportError): await asyncio.to_thread(attempt.result)
        assert client._failure is not None
    finally:
        client.close(); reader.close(); writer.close()

@pytest.mark.asyncio
@pytest.mark.parametrize("reply_kind", ["matching", "wrong_parent", "refused"])
async def test_parent_preparation_preserves_exact_selection_and_refusal(reply_kind):
    client, reader, writer = pipes(); request = selection()
    try:
        task = asyncio.create_task(asyncio.to_thread(client.prepare_workflow_parent, request,
            parent_root_issuance_id="p-root", parent_leaf_issuance_id="p-leaf"))
        command = await asyncio.to_thread(receive, reader)
        expected = {"kind": "prepare_workflow_parent", "request": {
            "commission": request.wire(), "parent_root_issuance_id": "p-root", "parent_leaf_issuance_id": "p-leaf"}}
        # The real peer decodes JSON arrays; asdict preserves contributor tuples.
        assert command["command"] == json.loads(json.dumps(expected, allow_nan=False))
        if reply_kind == "refused":
            respond(writer, command, {"kind": "error", "error": "denied"})
            with pytest.raises(HostReplyError): await task
        else:
            parent = dataclasses.asdict(request.parent.original)
            if reply_kind == "wrong_parent": parent["original"] = "other"
            respond(writer, command, {"kind": "workflow_parent_prepared", "original": parent})
            if reply_kind == "matching": assert await task == request.parent.original
            else:
                with pytest.raises(HostTransportError): await task
    finally:
        client.close(); reader.close(); writer.close()


def test_parent_preparation_refuses_aliased_issuance_ids_before_transport():
    client, reader, writer = pipes()
    try:
        with pytest.raises(HostControlError):
            client.prepare_workflow_parent(selection(), parent_root_issuance_id="root", parent_leaf_issuance_id="p-leaf")
    finally:
        client.close(); reader.close(); writer.close()
