"""Generated host and fixture-owned durable Add/list with the real SDK.

The anonymous pipe peer supplies native observations; it is not a native
runtime and cannot qualify provider use, authorization or cold native restore.
"""
import asyncio
from contextlib import asynccontextmanager
import json
import os
import queue
import sqlite3
import threading
from types import SimpleNamespace

import pytest

sdk = pytest.importorskip("meerkat_toolkit.member_classes")
from meerkat_meta.agent_builder import CallbackDispatcher
from meerkat_toolkit.host_control import PrivateHostClient
from test_selected_member_classes import generated_dynamic


class Peer:
    def __init__(self, app):
        child_read, parent_write = os.pipe()
        parent_read, child_write = os.pipe()
        self.host = PrivateHostClient(parent_write, parent_read, timeout=3,
            member_mcp=app.member_mcp_runtime_options()["member_mcp"])
        self.client = sdk.MemberClassClient(self.host)
        self.reader = os.fdopen(child_read, "rb")
        self.writer = os.fdopen(child_write, "wb", buffering=0)
        self.commands = []
        self.incoming = queue.Queue()
        self.reader_thread = threading.Thread(target=self._read, daemon=True)
        self.reader_thread.start()

    def _read(self):
        for line in self.reader:
            self.incoming.put(json.loads(line))

    async def receive(self):
        value = await asyncio.to_thread(self.incoming.get, True, 3)
        self.commands.append(value["command"])
        return value

    def send(self, identifier, result):
        self.writer.write(json.dumps({"version": 1, "id": identifier, "result": result}).encode()+b"\n")

    async def retain(self, selection, bindings=None):
        self.send(0, {"kind": "member_class_mcp_required", "exchange_id": 1,
            "selection": selection, "bindings": [] if bindings is None else bindings})
        ack = await self.receive()
        retained = ack["command"]["operation"]["retained"]
        assert retained == {"selection": selection, "bindings": [] if bindings is None else bindings}
        self.send(ack["id"], {"kind": "member_class_mcp_acknowledged", "exchange_id": 1})

    def ready(self, request, selection):
        self.send(request["id"], {"kind": "member_class_listing_required",
            "operation_id": request["id"], **selection})

    async def acknowledge_listing(self, request, selection, *, accepted=True):
        ack = await self.receive()
        assert ack["command"]["operation"] == {"kind": "listing_result", "operation_id": request["id"],
            "accepted": accepted, **selection}
        self.send(ack["id"], {"kind": "member_class_listing_acknowledged",
            "operation_id": request["id"], **selection})

    def terminal(self, request, identity, *, stage="minted"):
        value = {"state": "settled", "native": {"state": "result", "result": {
            "outcomes": {identity: {"restore": {"stage": stage}}}, "managed_edges": []}},
            "authorization": {"state": "succeeded"}, "cleanup": {"state": "succeeded"}}
        self.send(request["id"], {"kind": "member_class", "observation": value})
        return value

    async def close(self):
        await asyncio.to_thread(self.host.close)
        await asyncio.to_thread(self.reader_thread.join, 3)
        assert not self.reader_thread.is_alive()
        self.reader.close()
        self.writer.close()


@asynccontextmanager
async def connected_fixture(tmp_path, monkeypatch):
    _, app, context = await generated_dynamic(tmp_path, monkeypatch, durable=True)
    peer = Peer(app)
    runtime = SimpleNamespace(is_running=True, member_classes=peer.client)
    app.context.runtime = runtime
    try:
        yield app, context, runtime, peer
    finally:
        await peer.close()
        await app.stop()


async def reserve(app, context):
    selection = {"admission_id": "add-alice", "identity": "resident:alice", "class": "resident"}
    app.integration.begin_identity(selection["admission_id"], {"identity": selection["identity"],
        "classId": selection["class"], "context": {"owner": "alice"}})
    await app._prepare_snapshot(runtime=app.context.runtime)
    assert [row.identity for row in await app.roster(context)] == ["lead:main"]
    spec = next(row for row in app._snapshot.buildable_specs if row.identity == selection["identity"])
    app.integration.bind_spec(selection, spec.to_dict())
    member = {"mob_id": context.mob_definition["id"], "member": "opaque-alice-member", "role": spec.profile}
    owner = {"kind": "actor", "id": "fixture-owner"}
    request = sdk.MemberClassAddRequest(selection["admission_id"], {"class": "resident", "member": {
        "member": member, "logical_owner": owner, "context_generation": 1,
        "audience": {"kind": "principal", "principal": owner}}}, spec,
        {"id": "alice-console", "origin": {"kind": "local_console", "member": member,
            "requester": owner, "ingress_actor": owner}, "logical_executor": owner,
            "represented_subject": None, "source_authority": owner, "authentication_domain": "console", "event_domain": "console"},
        {"target": {"kind": "original_route", "route": "alice-console"}, "issuance_id": "alice-controller"})
    return selection, request


async def read_identity(app, spec, scope):
    dispatcher = CallbackDispatcher()
    dispatcher.register_agent_customizer(app)
    await dispatcher.handle_callback("callback/agent_customizer/customize_build", {
        "scope_id": scope, "context": {"identity": spec.identity}, "spec": spec.to_dict(), "draft": {}})
    return await dispatcher.handle_callback("callback/call_tool", {
        "scope_id": scope, "tool": "read_resident", "arguments": {}})


async def test_durable_row_precedes_add_listing_joins_and_cold_host_reopens_exact_bindings(tmp_path, monkeypatch):
    async with connected_fixture(tmp_path, monkeypatch) as (app, context, runtime, peer):
        selection, request = await reserve(app, context)
        attempt = runtime.member_classes.begin_add(request, publish=lambda: app.integration.publish(selection, runtime))
        wire = await peer.receive()
        assert wire["command"] == {"kind": "member_class", "operation": request.wire()}
        with sqlite3.connect(tmp_path / "fixture-admissions.sqlite") as independent:
            assert independent.execute("SELECT phase FROM admissions").fetchone() == ("admitting",)
            assert json.loads(independent.execute("SELECT spec FROM admissions").fetchone()[0]) == request.spec.to_dict()
        await peer.retain(selection)
        assert app.integration.lookup_member_mcp(selection)["bindings"] == []
        # Mimic native's actual identity callback while it is still held.
        dispatcher = CallbackDispatcher()
        dispatcher.register_agent_customizer(app)
        await dispatcher.handle_callback("callback/agent_customizer/customize_build", {
            "scope_id": "held", "context": {"identity": request.spec.identity}, "spec": request.spec.to_dict(), "draft": {}})
        assert app.integration.phase(request.id) == "admitting"
        async with app._reconcile_lock:
            peer.ready(wire, selection)
            await peer.acknowledge_listing(wire, selection)
            terminal = peer.terminal(wire, selection["identity"])
            assert await asyncio.to_thread(attempt.result) == terminal
            await attempt.join_listing()
        assert app.integration.publish_calls == 1
        assert {row.identity for row in await app.roster(context)} == {"lead:main", "resident:alice"}
        assert (await read_identity(app, request.spec, "live"))["content"] == {
            "identity": "resident:alice", "context": {"owner": "alice"}}
    # New generated host and application connection, same committed fixture DB.
    # This proves application restoration, not native runtime restoration.
    async with connected_fixture(tmp_path, monkeypatch) as (cold, context, _, peer):
        participant = cold.member_mcp_runtime_options()["member_mcp"]
        retained = await participant._lookup(sdk.MemberMcpAdmissionSelection.from_wire(selection))
        assert retained.bindings == () and retained.selection.wire() == selection
        spec = next(row for row in await cold.roster(context) if row.identity == "resident:alice")
        assert spec.to_dict() == request.spec.to_dict()
        assert (await read_identity(cold, spec, "cold"))["content"]["context"] == {"owner": "alice"}
        await cold.integration.void(selection)
        with pytest.raises(ValueError): await participant._lookup(retained.selection)
        assert [row.identity for row in await cold.roster(context)] == ["lead:main"]
        assert peer.commands == []


async def test_cancelled_waiter_and_unsettled_keep_one_durable_listing_until_explicit_join(tmp_path, monkeypatch):
    async with connected_fixture(tmp_path, monkeypatch) as (app, context, runtime, peer):
        selection, request = await reserve(app, context)
        entered, release = asyncio.Event(), asyncio.Event()
        async def publish():
            entered.set()
            await release.wait()
            await app.integration.publish(selection, runtime)
        attempt = runtime.member_classes.begin_add(request, publish=publish)
        wire = await peer.receive()
        await peer.retain(selection)
        peer.ready(wire, selection)
        await asyncio.wait_for(entered.wait(), 2)
        observer = asyncio.create_task(attempt.join_listing())
        await asyncio.sleep(0)
        observer.cancel()
        with pytest.raises(asyncio.CancelledError): await observer
        peer.send(wire["id"], {"kind": "member_class", "observation": {
            "state": "unsettled", "settlement_id": wire["id"]}})
        initial = await asyncio.to_thread(attempt.result)
        assert initial["state"] == "unsettled" and app.integration.phase(request.id) == "admitting"
        join_task = asyncio.create_task(asyncio.to_thread(runtime.member_classes.join, initial["settlement_id"]))
        join = await peer.receive()
        assert join["command"]["operation"] == {"kind": "join", "settlement_id": wire["id"]}
        release.set()
        await peer.acknowledge_listing(wire, selection)
        peer.terminal(join, selection["identity"])
        assert (await join_task)["native"]["result"]["outcomes"][selection["identity"]]["restore"]["stage"] == "minted"
        await attempt.join_listing()
        assert app.integration.publish_calls == 1
        assert sum(row["operation"]["kind"] == "add" for row in peer.commands) == 1
        assert app.integration.phase(request.id) == "listed"


async def test_duplicate_native_reservation_refusal_cannot_void_the_original_durable_row(tmp_path, monkeypatch):
    async with connected_fixture(tmp_path, monkeypatch) as (app, context, runtime, peer):
        selection, request = await reserve(app, context)
        with pytest.raises(sqlite3.IntegrityError):
            app.integration.begin_identity(request.id, {"identity": request.spec.identity, "classId": "resident"})
        attempt = runtime.member_classes.begin_add(request, publish=lambda: app.integration.publish(selection, runtime))
        wire = await peer.receive()
        peer.send(wire["id"], {"kind": "member_class", "observation": {
            "state": "reservation_not_accepted", "error": "unavailable"}})
        assert (await asyncio.to_thread(attempt.result))["state"] == "reservation_not_accepted"
        await attempt.join_listing()
        assert app.integration.phase(request.id) == "admitting"
        assert app.integration.publish_calls == 0
        assert len(peer.commands) == 1


async def test_failed_listing_keeps_durable_fact_until_caller_joins_and_voids(tmp_path, monkeypatch):
    async with connected_fixture(tmp_path, monkeypatch) as (app, context, runtime, peer):
        selection, request = await reserve(app, context)
        async def publish():
            await app.integration.publish(selection, runtime)
            raise RuntimeError("Fixture loses confirmation after its durable commit")
        attempt = runtime.member_classes.begin_add(request, publish=publish)
        wire = await peer.receive()
        await peer.retain(selection)
        peer.ready(wire, selection)
        await peer.acknowledge_listing(wire, selection, accepted=False)
        terminal = peer.terminal(wire, selection["identity"], stage="refused")
        assert await asyncio.to_thread(attempt.result) == terminal
        with pytest.raises(sdk.MemberClassListingUnknown): await attempt.join_listing()
        # An uncertain/refused operation is not evidence that its app write
        # disappeared. Only the application owner changes its durable row.
        assert app.integration.phase(request.id) == "listed"
        await app.integration.void(selection)
        assert app.integration.phase(request.id) == "void"
        assert [row.identity for row in await app.roster(context)] == ["lead:main"]
        with pytest.raises(ValueError):
            await app.member_mcp_runtime_options()["member_mcp"]._lookup(
                sdk.MemberMcpAdmissionSelection.from_wire(selection))
        assert sum(row["operation"]["kind"] == "add" for row in peer.commands) == 1
