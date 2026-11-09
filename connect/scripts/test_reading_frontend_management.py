"""Component checks of Reading's SQLite/actor disable recovery and sender lifecycle.

Native observation endpoints are controlled fakes. The application methods and
Toolkit close value types are real source. Reply checks use the real Toolkit
sender, presenter and HTTP transport with an isolated loopback endpoint. These
checks make no native journey, live provider, CLI or Console acceptance claim.
"""
import argparse
import asyncio
from contextlib import closing
import importlib.util
import json
import os
from pathlib import Path
import sqlite3
import sys
import tempfile
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import types
import unittest
from unittest.mock import patch
from uuid import UUID


ROOT = Path(__file__).resolve().parents[1]
APPLICATION = ROOT / "crates/meerkat-connect/tests/fixtures/forge-reading/application"
TOOLKIT_PYTHON = ROOT / "sdk/python/src"


def module_at(name, path, *, package=False):
    spec = importlib.util.spec_from_file_location(
        name, path, submodule_search_locations=[str(path.parent)] if package else None)
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


def component_imports():
    # Import-only placeholders avoid loading any installed native SDK/runtime.
    # No instance or method on these placeholders is used by the component.
    meta = types.ModuleType("meerkat_meta")
    meta.__path__ = []
    transport = types.ModuleType("meerkat_meta._transport")
    transport.PersistentTransport = type("UnusedPersistentTransport", (), {})
    runtime = types.ModuleType("meerkat_meta.runtime")
    runtime.MetaRuntime = type("UnusedMetaRuntime", (), {})
    errors = types.ModuleType("meerkat_meta.errors")
    errors.TurnUnknownError = type("ComponentTurnUnknownError", (Exception,), {})
    errors.TurnFailedError = type("ComponentTurnFailedError", (Exception,), {})
    return {"meerkat_meta": meta, "meerkat_meta._transport": transport,
            "meerkat_meta.runtime": runtime, "meerkat_meta.errors": errors}


class ControlledClose:
    """Observation endpoint only; all returned receipts are explicitly synthetic."""
    def __init__(self, sdk):
        self.sdk = sdk
        self.handle = sdk.HostCloseHandle(UUID(int=1), UUID(int=2),
                                         sdk.HostCloseScope("component", "reading-telegram"))
        self.begin_calls, self.observe_calls, self.join_calls = [], [], []
        self.unknown_entry = False
        self.lost_join = False
        self.settled = False

    def observation(self, state):
        return self.sdk.HostCloseObservation(self.handle, state, None)

    def begin_close_component(self, component, reason):
        self.begin_calls.append((component, reason))
        if self.unknown_entry:
            raise TimeoutError("controlled missing entry receipt")
        return self.observation(self.sdk.HostCloseState.CLOSING)

    def observe_close(self, handle):
        self.observe_calls.append(handle)
        return self.observation(self.sdk.HostCloseState.SETTLED if self.settled
                                else self.sdk.HostCloseState.CLOSING)

    def join_close(self, handle):
        self.join_calls.append(handle)
        if self.lost_join and not self.settled:
            raise TimeoutError("controlled lost join observation")
        self.settled = True
        return self.observation(self.sdk.HostCloseState.SETTLED)


class ControlledRuntime:
    def __init__(self, host_control):
        self.host_control = host_control
        self.completed = False
        self.observations = []
        self.dispatches = self.replies = self.provider_entries = 0

    async def turn_result(self, identity, ticket):
        self.observations.append((identity, ticket))
        value = {"identity": identity, "ticket": ticket,
                 "state": "completed" if self.completed else "pending"}
        if self.completed:
            value.update(output_status="text", output="controlled component result",
                         output_truncated=False)
        return types.SimpleNamespace(to_dict=lambda: dict(value))

    async def submit(self, *args, **kwargs):
        self.dispatches += 1
        raise AssertionError("disable recovery must not submit input")

    async def dispatch(self, *args, **kwargs):
        self.dispatches += 1
        raise AssertionError("disable recovery must not dispatch input")

    async def send_text(self, *args, **kwargs):
        self.replies += 1
        raise AssertionError("disable recovery must not send a reply")

    def provider(self, *args, **kwargs):
        self.provider_entries += 1
        raise AssertionError("disable recovery must not open a provider")


class ReadingManagementRecoveryTests(unittest.IsolatedAsyncioTestCase):
    @classmethod
    def setUpClass(cls):
        cls.modules = patch.dict(sys.modules, component_imports())
        cls.modules.start()
        cls.addClassCleanup(cls.modules.stop)
        cls.sdk = module_at("meerkat_toolkit.host_control",
                            TOOLKIT_PYTHON / "meerkat_toolkit/host_control.py")
        toolkit = types.ModuleType("meerkat_toolkit")
        toolkit.__path__ = []
        toolkit.host_control = cls.sdk
        sys.modules["meerkat_toolkit"] = toolkit
        cls.application = module_at("_reading_management_component", APPLICATION / "__init__.py",
                                    package=True)

    async def asyncSetUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="reading-management-component-")
        self.addCleanup(self.temporary.cleanup)
        self.environment = patch.dict(os.environ, {"CONSOLE_USER_ID": "component-owner"}, clear=True)
        self.environment.start()
        self.addCleanup(self.environment.stop)
        self.host = {"protocol": 1, "host_id": "forge:" + "1" * 64,
                     "application_id": "reading-component", "source_digest": "2" * 64}
        self.close = ControlledClose(self.sdk)
        self.runtime = ControlledRuntime(self.close)
        context = types.SimpleNamespace(root=self.temporary.name,
            deployment={"slug": "reading-component"}, frontend_host=lambda: dict(self.host),
            runtime=self.runtime, submit_frontend_input=self.runtime.submit,
            telegram_transport_factory=self.runtime.provider)
        self.app = self.application.Integration(context)
        self.app.frontends.start()
        self.addCleanup(self.app.frontends.stop)
        self.commit({"component_id": "reading-telegram", "profile": "private-text-dm",
            "participant_id": "reading-owner", "route_id": "reading-lead-dm",
            "telegram_user_id": 42, "telegram_dm_chat_id": 42, "native_identity": "lead:main",
            "bot_token": {"private_field": "READING_TELEGRAM_BOT_TOKEN"},
            "exclusive_polling_acknowledged": True})
        self.disable = self.commit({"component_id": "reading-telegram", "action": "disable"})
        self.actor = self.app.telegram
        self.actor.poll_interval = .01
        self.started = False

    def request(self, operation, payload, approved=False):
        return types.SimpleNamespace(operation=operation, payload=payload, approved=approved,
                                     host=dict(self.host), principal="component-owner")

    def commit(self, configuration):
        with self.app.frontends.connection(readonly=True) as db:
            revision = self.app.frontends.state(db)["revision"]
        proposal = self.app.frontends.manage(self.request("prepare",
            {"configuration": configuration, "expected_revision": revision}))["proposal"]
        return self.app.frontends.manage(self.request("submit", {"proposal": proposal,
            "approval": {"principal": "component-owner", "expires_at": proposal["expires_at"]}}, True))

    async def start_actor(self, *, pending=False):
        await self.actor.start()
        self.started = True
        # No event-loop yield occurs between schema creation and retained rows.
        # The actor's first selected state is the already committed disable.
        selection = self.actor.selected()
        with self.app.frontends.connection(readonly=False) as db, db:
            rows = [(2, "queued", None), (3, "outbound-unknown", None)]
            if pending:
                rows.append((1, "native-pending", json.dumps(
                    {"identity": "lead:main", "turn_ticket": "component-ticket"})))
            for update_id, status, admission in rows:
                db.execute("INSERT INTO reading_telegram_inbox(bot_id,update_id,payload,binding,status,admission) VALUES(?,?,?,?,?,?)",
                    ("component-bot", update_id, "{}", json.dumps(selection, sort_keys=True,
                     separators=(",", ":")), status, admission))

    async def asyncTearDown(self):
        if self.started:
            await self.actor.stop()
        self.assertEqual((self.runtime.dispatches, self.runtime.replies, self.runtime.provider_entries), (0, 0, 0))
        if self.started:
            with self.app.frontends.connection(readonly=True) as db:
                self.assertEqual(db.execute("SELECT COUNT(*) FROM reading_telegram_outbound").fetchone()[0], 0)

    def close_state(self):
        return self.app.frontends.management_observation()["native_close"]

    async def wait_for(self, predicate):
        async def wait():
            while not predicate():
                if self.actor.task.done():
                    self.actor.task.result()
                    self.fail("application owner stopped before the component condition")
                await asyncio.sleep(.005)
        await asyncio.wait_for(wait(), 2)

    def change_count(self):
        with self.app.frontends.connection(readonly=True) as db:
            return db.execute("SELECT COUNT(*) FROM reading_frontend_changes").fetchone()[0]

    async def test_submission_disposition_is_explicit_and_retains_original_command(self):
        with self.app.frontends.connection(readonly=True) as db:
            proposal = json.loads(db.execute("SELECT proposal FROM reading_frontend_changes WHERE change_id=?",
                (self.disable["change_id"],)).fetchone()[0])
        command = self.request("submit", {"proposal": proposal,
            "approval": {"principal": "component-owner", "expires_at": proposal["expires_at"]}}, True)
        declarations = []
        def formatter(original, observation, *, known):
            declarations.append((original, observation, known))
            return (original, observation, known)
        self.app.context.frontend_submission_result = formatter
        known = await self.app.frontend_management(command)
        self.assertIs(known[0], command)
        self.assertIs(known[2], True)
        self.assertEqual(known[1]["status"], "committed")
        self.assertIsNone(known[1]["management_observation"]["native_close"])
        # A missing retained command stays unknown, even in a live configured app.
        missing = self.request("submit", {"proposal": {**proposal, "change_id": "unretained"},
            "approval": command.payload["approval"]}, True)
        unknown = await self.app.frontend_management(missing)
        self.assertIs(unknown[0], missing)
        self.assertIs(unknown[2], False)
        self.assertEqual(unknown[1]["status"], "unknown")
        # An exact command conflict is a known refusal, not success or close.
        conflict = self.request("submit", {"proposal": {**proposal, "expected_revision": 99},
            "approval": command.payload["approval"]}, True)
        refused = await self.app.frontend_management(conflict)
        self.assertIs(refused[2], True)
        self.assertEqual(refused[1]["status"], "change-conflict")
        with self.assertRaises(PermissionError):
            await self.app.frontend_management(self.request("submit", command.payload, False))
        self.assertEqual(len(declarations), 3)
        # A status read is still opaque observation and cannot declare settlement.
        observed = await self.app.frontend_management(self.request("status", {"change_id": self.disable["change_id"]}))
        self.assertIs(type(observed), dict)
        self.assertEqual(len(declarations), 3)
        del self.app.context.frontend_submission_result
        legacy = await self.app.frontend_management(command)
        self.assertIs(type(legacy), dict)
        self.assertEqual(legacy, known[1])
        self.assertEqual(self.close.begin_calls, [])

    async def test_pending_ticket_settles_without_second_approval_or_dispatch(self):
        await self.start_actor(pending=True)
        await self.wait_for(lambda: (self.close_state() or {}).get("state") == "not_entered")
        self.assertEqual(self.close.begin_calls, [])
        count = self.change_count()
        # Status is observational and does not advance a pending native ticket.
        status = self.app.frontends.manage(self.request("status", {"change_id": self.disable["change_id"]}))
        self.assertEqual(status["management_observation"]["native_close"]["state"], "not_entered")
        self.runtime.completed = True
        await self.wait_for(lambda: (self.close_state() or {}).get("state") == "settled")
        self.assertEqual(self.change_count(), count)
        self.assertEqual(self.close.begin_calls, [("reading-telegram", self.sdk.HostCloseReason.OPERATOR_DISABLE)])
        self.assertTrue(self.runtime.observations)
        self.assertEqual(set(self.runtime.observations), {("lead:main", "component-ticket")})
        with self.app.frontends.connection(readonly=True) as db:
            row = db.execute("SELECT status,completion FROM reading_telegram_inbox WHERE update_id=1").fetchone()
            self.assertEqual(row["status"], "disabled-reply-stopped")
            self.assertEqual(json.loads(row["completion"])["ticket"], "component-ticket")
        self.assertEqual(self.close_state()["application_drain"],
            {"accepted_native_unsettled": 0, "provider_outcome_unknown": 1,
             "pending_input_stopped": 1, "pending_reply_stopped": 1})
        self.assertEqual(self.commit({"component_id": "reading-telegram", "action": "remove"})["management_state"], "removed")

    async def test_lost_join_recovers_only_the_same_known_handle(self):
        self.close.lost_join = True
        await self.start_actor()
        await self.wait_for(lambda: (self.close_state() or {}).get("state") == "uncertain")
        retained = self.close_state()["observation"]["handle"]
        self.assertEqual(retained, self.close.handle.wire())
        self.assertEqual(len(self.close.begin_calls), 1)
        # Recreate the application actor over its same SQLite owner. No second
        # disable or close entry is needed to observe the retained handle.
        await self.actor.stop()
        self.started = False
        self.close.settled = True
        await self.actor.start()
        self.started = True
        await self.wait_for(lambda: (self.close_state() or {}).get("state") == "settled")
        self.assertEqual(len(self.close.begin_calls), 1)
        self.assertEqual(self.close_state()["observation"]["handle"], retained)
        self.assertTrue(self.close.join_calls)
        self.assertTrue(all(handle == self.close.handle for handle in self.close.observe_calls + self.close.join_calls))

    async def test_unknown_entry_without_handle_never_starts_another_close(self):
        self.close.unknown_entry = True
        await self.start_actor()
        await self.wait_for(lambda: (self.close_state() or {}).get("state") == "uncertain")
        self.assertIsNone(self.close_state()["observation"])
        await self.actor.stop()
        self.started = False
        self.close.unknown_entry = False
        self.close.settled = True
        await self.actor.start()
        self.started = True
        await self.wait_for(lambda: self.actor.state == "disabled-close-outcome-unknown")
        self.assertEqual(len(self.close.begin_calls), 1)
        self.assertEqual((self.close.observe_calls, self.close.join_calls), ([], []))
        self.assertEqual(self.close_state()["state"], "uncertain")
        with self.assertRaises(ValueError):
            self.commit({"component_id": "reading-telegram", "action": "remove"})


class LoopbackTelegram:
    """Real Toolkit HTTP transport, with an isolated local provider endpoint."""
    def __init__(self):
        self.entered = threading.Event()
        self.release = threading.Event()
        self.sends = []
        self.missing_receipt = False
        owner = self
        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def do_POST(self):
                payload = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
                method = self.path.rsplit("/", 1)[-1]
                if method == "getMe":
                    result = {"id": 987654321, "is_bot": True, "first_name": "Loopback"}
                elif method == "getWebhookInfo":
                    result = {"url": "", "has_custom_certificate": False, "pending_update_count": 0}
                elif method == "getUpdates":
                    result = []
                elif method == "sendMessage":
                    owner.sends.append(payload)
                    owner.entered.set()
                    if not owner.release.wait(5):
                        self.close_connection = True
                        return
                    result = {} if owner.missing_receipt else {
                        "message_id": 900 + len(owner.sends), "chat": {"id": int(payload["chat_id"])}}
                else:
                    raise AssertionError("unexpected loopback method")
                body = json.dumps({"ok": True, "result": result}).encode()
                self.send_response(200)
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)
        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.thread = threading.Thread(target=self.server.serve_forever)
        self.thread.start()

    def transport(self, token):
        from meerkat_toolkit.frontend.telegram import TelegramBotApiTransport
        value = TelegramBotApiTransport(token)
        value._api_base = "http://127.0.0.1:" + str(self.server.server_port) + "/bot/"
        return value

    async def close(self):
        self.release.set()
        await asyncio.to_thread(self.server.shutdown)
        await asyncio.to_thread(self.server.server_close)
        await asyncio.to_thread(self.thread.join)
        assert not self.thread.is_alive()


class ReadingSenderLifecycleTests(unittest.IsolatedAsyncioTestCase):
    """Configured example only: native completion rows are controlled inputs."""
    @classmethod
    def setUpClass(cls):
        cls.modules = patch.dict(sys.modules, component_imports())
        cls.modules.start()
        cls.addClassCleanup(cls.modules.stop)
        cls.sdk = module_at("meerkat_toolkit.host_control",
                            TOOLKIT_PYTHON / "meerkat_toolkit/host_control.py")
        toolkit = types.ModuleType("meerkat_toolkit")
        toolkit.__path__ = [str(TOOLKIT_PYTHON / "meerkat_toolkit")]
        toolkit.host_control = cls.sdk
        sys.modules["meerkat_toolkit"] = toolkit
        cls.application = module_at("_reading_sender_component", APPLICATION / "__init__.py", package=True)

    async def asyncSetUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="reading-sender-component-")
        self.addCleanup(self.temporary.cleanup)
        self.environment = patch.dict(os.environ, {"CONSOLE_USER_ID": "component-owner",
            "READING_TELEGRAM_BOT_TOKEN": "987654321:loopback-fixture-only"}, clear=True)
        self.environment.start()
        self.addCleanup(self.environment.stop)
        self.host = {"protocol": 1, "host_id": "forge:" + "1" * 64,
                     "application_id": "reading-component", "source_digest": "2" * 64}
        self.close = ControlledClose(self.sdk)
        self.runtime = ControlledRuntime(self.close)
        self.provider = LoopbackTelegram()
        self.transports = []
        def transport(token):
            value = self.provider.transport(token)
            self.transports.append(value)
            return value
        self.context = types.SimpleNamespace(root=self.temporary.name,
            deployment={"slug": "reading-component"}, frontend_host=lambda: dict(self.host),
            runtime=self.runtime, submit_frontend_input=self.runtime.submit,
            telegram_transport_factory=transport)
        self.app = self.application.Integration(self.context)
        self.app.frontends.start()
        self.configuration = {"component_id": "reading-telegram", "profile": "private-text-dm",
            "participant_id": "reading-owner", "route_id": "reading-lead-dm",
            "telegram_user_id": 42, "telegram_dm_chat_id": 42, "native_identity": "lead:main",
            "bot_token": {"private_field": "READING_TELEGRAM_BOT_TOKEN"},
            "exclusive_polling_acknowledged": True}
        self.commit(self.configuration)
        self.actor = self.app.telegram
        self.actor.poll_interval = .01
        self.tasks = []
        self.service_store = self.app.store
        proposal = self.app.propose({"operation": "add", "request_id": "component-book-1",
            "title": "Controlled Book", "author": "Component Author", "evidence": "loopback fixture"})
        self.assertEqual(proposal["status"], "awaiting-human-confirmation")
        await self.app.on_console_input(types.SimpleNamespace(channel="host-authenticated-console",
            identity="lead:main", content=proposal["confirmation"], principal="component-owner",
            input_frame_id="component-confirmation-1"))
        self.service_snapshot = self.app.store.snapshot()
        self.assertEqual(len(self.service_snapshot["books"]), 1)
        self.assertEqual(len(self.service_snapshot["books"][0]["events"]), 1)

    def commit(self, configuration):
        with self.app.frontends.connection(readonly=True) as db:
            revision = self.app.frontends.state(db)["revision"]
        request = lambda operation, payload, approved=False: types.SimpleNamespace(
            operation=operation, payload=payload, approved=approved,
            host=dict(self.host), principal="component-owner")
        proposal = self.app.frontends.manage(request("prepare", {
            "configuration": configuration, "expected_revision": revision}))["proposal"]
        return self.app.frontends.manage(request("submit", {"proposal": proposal,
            "approval": {"principal": "component-owner", "expires_at": proposal["expires_at"]}}, True))

    def completed(self, update_id):
        # Fixture input to the application, not a fabricated native acceptance claim.
        ticket = "controlled-ticket-" + str(update_id)
        completion = {"identity": "lead:main", "ticket": ticket, "state": "completed",
            "output_status": "text", "output": "controlled result " + str(update_id), "output_truncated": False}
        selection = self.actor.selected()
        with self.app.frontends.connection(readonly=False) as db, db:
            db.execute("INSERT INTO reading_telegram_inbox(bot_id,update_id,payload,binding,status,admission,completion) VALUES(?,?,?,?,?,?,?)",
                ("987654321", update_id, "{}", json.dumps(selection, sort_keys=True, separators=(",", ":")),
                 "native-completed", json.dumps({"identity": "lead:main", "turn_ticket": ticket}), json.dumps(completion)))

    def outbound(self):
        with self.app.frontends.connection(readonly=True) as db:
            return [dict(row) for row in db.execute("SELECT * FROM reading_telegram_outbound ORDER BY update_id")]

    async def wait_for(self, predicate):
        async def wait():
            while not predicate():
                if self.actor.task is not None and self.actor.task.done():
                    self.actor.task.result()
                    self.fail("Reading supervisor ended before the condition")
                await asyncio.sleep(.005)
        await asyncio.wait_for(wait(), 2)

    async def start_completed(self, *, second=False):
        await self.actor.start()
        self.completed(1)  # start() has not yielded to the supervisor.
        if second:
            self.completed(2)  # Same pending batch, before the first read.
        await self.wait_for(self.provider.entered.is_set)
        self.assertEqual(len(self.transports), 1)
        self.assertEqual(self.outbound()[0]["state"], "entered")
        self.assertEqual(len(self.transports[0]._operations), 1)
        return self.actor.sender_authority, self.actor.sender_activation

    async def asyncTearDown(self):
        self.provider.release.set()
        try:
            await self.actor.stop()
            for task in self.tasks:
                await asyncio.gather(task, return_exceptions=True)
            self.assertTrue(all(task.done() for task in self.tasks))
            self.assertTrue(all(not transport._operations for transport in self.transports))
            self.assertIs(self.app.store, self.service_store)
            self.assertEqual(self.app.store.snapshot(), self.service_snapshot)
            self.assertEqual((self.runtime.dispatches, self.runtime.replies), (0, 0))
        finally:
            self.app.frontends.stop()
            await self.provider.close()

    async def test_entered_reply_survives_sender_retirement_and_later_reply_is_stopped(self):
        authority, activation = await self.start_completed(second=True)
        borrowed = await authority.select_dispatch(sender_id=activation.sender_id)
        disabled = self.commit({"component_id": "reading-telegram", "action": "disable"})
        await self.actor.configuration_committed(disabled["revision"])
        await self.wait_for(lambda: not authority.is_attached(activation.sender_id))
        self.assertFalse(authority.is_current_dispatch(borrowed))
        self.assertFalse(self.actor.session.done())
        self.assertEqual(len(self.transports[0]._operations), 1)
        self.assertEqual(len(self.provider.sends), 1)
        self.assertEqual(self.outbound()[0]["state"], "entered")
        self.provider.release.set()
        await self.wait_for(lambda: self.actor.state == "disabled")
        rows = self.outbound()
        self.assertEqual(len(rows), 1)
        self.assertEqual(rows[0]["state"], "provider_accepted")
        self.assertEqual(json.loads(rows[0]["receipt"])["provider_message_id"], "901")
        with self.app.frontends.connection(readonly=True) as db:
            self.assertEqual(db.execute("SELECT status FROM reading_telegram_inbox WHERE update_id=2").fetchone()[0], "disabled-reply-stopped")
        self.assertEqual(len(self.provider.sends), 1)
        self.assertIsNone(self.actor.sender_activation)
        with self.assertRaisesRegex(ValueError, "separately selected new activation"):
            self.commit(self.configuration)
        self.assertFalse(authority.is_current_dispatch(borrowed))
        self.assertEqual(self.outbound(), rows)

    async def test_cancelled_stop_joins_entered_http_and_unknown_is_not_resent_on_reopen(self):
        authority, activation = await self.start_completed()
        stopping = asyncio.create_task(self.actor.stop())
        self.tasks.append(stopping)
        await self.wait_for(lambda: not authority.is_attached(activation.sender_id))
        stopping.cancel()
        await asyncio.sleep(.02)
        self.assertFalse(stopping.done())
        self.assertEqual(len(self.transports[0]._operations), 1)
        self.provider.release.set()
        with self.assertRaises(asyncio.CancelledError):
            await stopping
        rows = self.outbound()
        self.assertEqual(rows[0]["state"], "unknown")
        self.assertEqual(len(self.provider.sends), 1)
        self.assertFalse(self.transports[0]._operations)
        # Ordinary application reopen uses the same durable records and no new input.
        self.app.frontends.stop()
        reopened = self.application.Integration(self.context)
        self.app = reopened
        self.actor = reopened.telegram
        self.actor.poll_interval = .01
        await reopened.start()
        await self.wait_for(lambda: len(self.transports) == 2 and self.actor.sender_activation is not None)
        await asyncio.sleep(.03)
        self.assertEqual(self.outbound(), rows)
        self.assertEqual(len(self.provider.sends), 1)
        # The Store wrapper is new, but the same canonical database/data survives.
        self.service_store = reopened.store

    async def test_missing_provider_receipt_stays_unknown_across_exact_configuration_reopen(self):
        self.provider.missing_receipt = True
        self.provider.release.set()
        await self.actor.start()
        self.completed(1)
        await self.wait_for(lambda: self.outbound() and self.outbound()[0]["state"] == "unknown")
        rows = self.outbound()
        authority = self.actor.sender_authority
        activation = self.actor.sender_activation
        borrowed = await authority.select_dispatch(sender_id=activation.sender_id)
        before = authority.active_owner(activation.sender_id)
        await self.actor.stop()
        self.assertFalse(authority.is_current_dispatch(borrowed))
        self.assertFalse(authority.is_attached(activation.sender_id))
        with self.assertRaises(ValueError):
            await authority.select_dispatch(sender_id=activation.sender_id)
        await self.actor.start()
        await self.wait_for(lambda: self.actor.sender_activation is not None)
        await asyncio.sleep(.03)
        self.assertEqual(authority.active_owner(activation.sender_id), before)
        self.assertEqual(authority.owner_count(activation.sender_id), 1)
        current = await authority.select_dispatch(sender_id=activation.sender_id)
        self.assertTrue(authority.is_current_dispatch(current))
        self.assertFalse(authority.is_current_dispatch(borrowed))
        self.assertIsNot(current.sender, borrowed.sender)
        with self.assertRaises(ValueError):
            await authority.close_activation(activation)
        self.assertTrue(authority.is_current_dispatch(current))
        self.assertEqual(self.outbound(), rows)
        self.assertEqual(len(self.provider.sends), 1)
        self.provider.missing_receipt = False
        self.completed(2)
        await self.wait_for(lambda: len(self.outbound()) == 2 and self.outbound()[1]["state"] == "provider_accepted")
        self.assertEqual(self.outbound()[0], rows[0])
        self.assertEqual(len(self.provider.sends), 2)


class ReadingSQLiteLifetimeTests(unittest.TestCase):
    def test_collection_connections_close_after_commit_replay_and_rollback(self):
        collection = module_at("_reading_sqlite_collection", APPLICATION / "collection.py")
        original_connect = sqlite3.connect
        opened = []

        def connect(*args, **kwargs):
            connection = original_connect(*args, **kwargs)
            opened.append(connection)
            return connection

        def assert_closed():
            self.assertTrue(opened)
            for connection in opened:
                with self.assertRaisesRegex(sqlite3.ProgrammingError, "closed database"):
                    connection.execute("SELECT 1")

        try:
            with tempfile.TemporaryDirectory(prefix="reading-sqlite-") as directory:
                integration = collection.Integration(types.SimpleNamespace(root=directory))
                store = integration.store
                with patch.object(collection.sqlite3, "connect", connect):
                    self.assertEqual(store.snapshot()["books"], [])
                    assert_closed()
                    request = dict(operation="add", request_id="connection-closure", title="Book",
                                   author="Author", evidence="controlled ledger test")
                    proposed = integration.propose(request)
                    self.assertEqual(proposed["status"], "awaiting-human-confirmation")
                    self.assertEqual(integration.receipt({"token": proposed["confirmation"][8:]}),
                                     {"status": "not-confirmed"})
                    assert_closed()
                    saved = store.apply(request)
                    self.assertEqual(saved["status"], "saved")
                    self.assertEqual(store.apply(request), saved)
                    before = store.snapshot()
                    assert_closed()
                    with closing(original_connect(store.path)) as connection, connection:
                        connection.execute("CREATE TRIGGER reject_request BEFORE INSERT ON requests "
                            "BEGIN SELECT RAISE(ABORT, 'blocked request'); END")
                    with self.assertRaisesRegex(sqlite3.IntegrityError, "blocked request"):
                        store.apply(dict(operation="finished", request_id="rollback-closure",
                                         book_id=saved["book_id"], expected_revision=1))
                    self.assertEqual(store.snapshot(), before)
                    assert_closed()
        finally:
            for connection in opened:
                connection.close()

    def test_schema_failure_closes_connection_and_preserves_original_error(self):
        collection = module_at("_reading_sqlite_failure", APPLICATION / "collection.py")
        original_connect = sqlite3.connect
        for close_fails in (False, True):
            with self.subTest(close_fails=close_fails):
                opened, errors = [], []

                class Connection(sqlite3.Connection):
                    def executescript(self, *args, **kwargs):
                        try:
                            return super().executescript(*args, **kwargs)
                        except sqlite3.DatabaseError as error:
                            errors.append(error)
                            raise

                    def close(self):
                        super().close()
                        if close_fails:
                            raise OSError("controlled close failure after actual closure")

                def connect(*args, **kwargs):
                    connection = original_connect(*args, **kwargs, factory=Connection)
                    opened.append(connection)
                    connection.set_authorizer(lambda action, *_:
                        sqlite3.SQLITE_DENY if action == sqlite3.SQLITE_CREATE_TABLE else sqlite3.SQLITE_OK)
                    return connection

                try:
                    with tempfile.TemporaryDirectory(prefix="reading-sqlite-failure-") as directory:
                        with patch.object(collection.sqlite3, "connect", connect):
                            with self.assertRaises(sqlite3.DatabaseError) as caught:
                                collection.Store(Path(directory) / "collection.sqlite").connect()
                        self.assertEqual(len(opened), 1)
                        self.assertIs(caught.exception, errors[0])
                        with self.assertRaisesRegex(sqlite3.ProgrammingError, "closed database"):
                            opened[0].execute("SELECT 1")
                finally:
                    for connection in opened:
                        sqlite3.Connection.close(connection)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(add_help=False)
    parser.add_argument("--application-directory", type=Path, default=APPLICATION)
    parser.add_argument("--toolkit-python", type=Path, default=TOOLKIT_PYTHON)
    options, unittest_args = parser.parse_known_args()
    APPLICATION, TOOLKIT_PYTHON = options.application_directory, options.toolkit_python
    unittest.main(argv=[sys.argv[0], *unittest_args])
