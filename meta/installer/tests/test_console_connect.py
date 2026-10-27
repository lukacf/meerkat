"""Offline private-subprocess boundary controls, not native/presenter acceptance."""
from __future__ import annotations

import asyncio
import copy
import json
import os
from pathlib import Path
import sys
import tempfile
import time
import unittest

from launchkit.templates_v2.console_connect import ConsoleConnect, ConnectError, _answer, _bootstrap, _decode, _prompt, _state


# This is an external protocol peer fixture. It is not a Connect implementation,
# native session, approval owner, provider or application acceptance substitute.
PEER = r'''
import asyncio, json, os, sys

MODE = os.environ.get("CONNECT_FIXTURE_MODE", "normal")
scope = "fixture-scope"
state = dict(available=True, can_send=True, can_stop=False, outcome_uncertain=False,
             active_send=None, last_completion=None)
pending = None
initial = None
answers = 0
prompt_kind = os.environ.get("CONNECT_FIXTURE_PROMPT", "approval")

def emit(value):
    print(json.dumps(value, separators=(",", ":")), flush=True)

def response(identity, result):
    emit(dict(jsonrpc="2.0", id=identity, result=result))

def error(identity, code):
    emit(dict(jsonrpc="2.0", id=identity,
              error=dict(code=-32010, message="PRIVATE_ERROR_CANARY", data=dict(code=code))))

def event(value):
    emit(dict(jsonrpc="2.0", method="connect/event", params=dict(scope=scope, **value)))

def prompt(identity="p:one"):
    if prompt_kind == "approval":
        value = dict(kind="approval", approval_id="retained-approval", summary="Complete proposal",
                     body=dict(warning="Queued messages may be consumed", nested=["PRIVATE_PROPOSAL_CANARY", {"exact": True}]),
                     allowed_decisions=["approve", "deny"], expires_at=None)
    elif prompt_kind == "forge_password":
        value = dict(kind="forge_password", origin="http://127.0.0.1:4110", mode="initial", min_chars=12, max_chars=4096)
    elif prompt_kind == "forge_origin":
        value = dict(kind="forge_origin", permitted_origin="http://127.0.0.1:4110")
    else:
        value = dict(kind="forge_recovery", choices=[dict(choice_id="entry-1", application_id="reading",
                     change_id="known-change", origin="http://127.0.0.1:4110")], more=True, skipped_entries=2)
    emit(dict(jsonrpc="2.0", id=identity, method="connect/prompt", params=dict(scope=scope, prompt=value)))

def finish(completion="completed"):
    global pending
    if pending is None:
        return
    identity = pending
    pending = None
    state.update(can_send=True, can_stop=False, active_send=None,
                 last_completion=dict(send_request_id=identity, completion=completion))
    event(dict(kind="state", state=state))
    response(identity, dict(scope=scope, send_request_id=identity, completion=completion, state=state))

async def main():
    global initial, pending, answers
    if sys.argv[1:] != ["--stdio"]:
        sys.exit(8)
    print("PRIVATE_STDERR_CANARY", file=sys.stderr, flush=True)
    reader = asyncio.StreamReader()
    await asyncio.get_running_loop().connect_read_pipe(lambda: asyncio.StreamReaderProtocol(reader), sys.stdin)
    while True:
        line = await reader.readline()
        if not line:
            return
        value = json.loads(line)
        identity = value["id"]
        method = value.get("method")
        params = value.get("params", {})
        if method == "connect/initialize":
            initial = identity
            if MODE == "duplicate_keys":
                print('{"jsonrpc":"2.0","id":"' + identity + '","id":"' + identity + '","result":{}}', flush=True)
                return
            if MODE == "unexpected_null":
                response(identity, None)
                return
            if MODE == "batch":
                emit([dict(jsonrpc="2.0", id=identity, result={})])
                return
            if MODE in {"pending_initialize", "pending_initialize_no_reply"}:
                continue
            if MODE == "native_auth_required":
                error(identity, "native_auth_required")
                continue
            response(identity, dict(protocol_version=1, scope=scope, session_id="actual-fixture-observation",
                capabilities=dict(prompt_kinds=["forge_recovery", "forge_origin", "forge_password", "approval"],
                                  forge_configuration=True, service_preparation=False, homecore=False), state=state))
        elif method == "connect/send":
            pending = identity
            state.update(can_send=False, can_stop=True,
                         active_send=dict(send_request_id=identity, cancellation_requested=False))
            event(dict(kind="state", state=state))
            event(dict(kind="conversation", send_request_id=identity, event=dict(kind="run_observed")))
            if MODE == "bad_event":
                event(dict(kind="conversation", send_request_id=identity, event=dict(kind="raw_tool", password="PRIVATE_BAD_EVENT_CANARY")))
                return
            if MODE == "mutation_notification":
                emit(dict(jsonrpc="2.0", method="connect/send", params=dict(scope=scope, text="PRIVATE_MUTATION_CANARY")))
                return
            if MODE == "wrong_scope":
                emit(dict(jsonrpc="2.0", method="connect/event", params=dict(scope="other-scope", kind="state", state=state)))
                return
            if MODE == "wrong_send":
                response(identity, dict(scope=scope, send_request_id="h:other", completion="completed", state=state))
                return
            if MODE in {"prompt", "pending", "standalone_unavailable_pending"}:
                if MODE in {"prompt", "standalone_unavailable_pending"}:
                    prompt()
            else:
                event(dict(kind="conversation", send_request_id=identity,
                           event=dict(kind="text_delta", text="Provisional visible text", messageId="visible")))
                finish(os.environ.get("CONNECT_FIXTURE_COMPLETION", "completed"))
        elif method == "connect/history":
            if MODE in {"standalone_unavailable", "standalone_unavailable_pending"}:
                event(dict(kind="unavailable", reason=os.environ.get("CONNECT_FIXTURE_REASON", "retired")))
                prompt("p:late")
            if MODE == "history_unavailable":
                error(identity, "history_unavailable")
                continue
            if MODE == "bad_history":
                response(identity, dict(history=dict(messages=[dict(role="tool", text="PRIVATE_TOOL_CANARY")], offset=0, total=1, hasMore=False), state=state))
                return
            def history(identity=identity, offset=params.get("offset", 17)):
                response(identity, dict(history=dict(messages=[dict(role="assistant", text="Safe canonical fixture history; answers=" + str(answers))],
                                          offset=offset, total=71, hasMore=True), state=state))
            if MODE == "delayed_history":
                asyncio.get_running_loop().call_later(0.08, history)
            else:
                history()
        elif method == "connect/cancel":
            if pending != params["send_request_id"]:
                response(identity, dict(scope=scope, send_request_id=params["send_request_id"], status="not_current"))
            else:
                state["active_send"]["cancellation_requested"] = True
                response(identity, dict(scope=scope, send_request_id=pending, status="cancellation_requested"))
                asyncio.get_running_loop().call_later(0.08, finish, "cancelled")
        elif method == "connect/retire":
            response(identity, dict(target=params["target"], retirement_id=params["retirement_id"],
                                    admission_closed=True, prompts_retired=True, entered_work="settling"))
        elif method == "connect/close":
            if MODE in {"failed_close", "delayed_failed_close"}:
                error(identity, "shutdown_unconfirmed")
                if MODE == "delayed_failed_close":
                    await asyncio.sleep(0.12)
                return
            if pending is not None:
                finish("cancelled")
            if MODE == "pending_initialize":
                error(initial, "binding_retired")
            response(identity, dict(target=params["target"], resources_closed=True))
            if MODE == "delayed_close_exit":
                await asyncio.sleep(0.12)
            return
        elif method is None and identity == "p:one":
            if "error" in value:
                continue
            answer = value["result"]["answer"]
            if prompt_kind == "forge_password" and answer.get("password") != os.environ["CONNECT_FIXTURE_PASSWORD"]:
                sys.exit(9)
            answers += 1
            event(dict(kind="prompt_closed", prompt_id="p:one", disposition="decision_recorded" if prompt_kind == "approval" else "answered"))
            finish()

asyncio.run(main())
'''


class ConsoleConnectTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.binary = self.root / "fixture-connect"
        self.binary.write_text("#!" + sys.executable + "\n" + PEER)
        self.binary.chmod(0o700)
        self.current = True
        self.children = []

    async def asyncTearDown(self):
        for child in self.children:
            try:
                await asyncio.wait_for(child.close(), 4)
            except ConnectError:
                pass
        self.temporary.cleanup()

    def bootstrap(self):
        return {
            "protocol_version": 1,
            "application": {"application_id": "reading", "instance_id": "fresh-lifetime", "origin": "http://127.0.0.1:4110"},
            "viewer": {"principal_id": "fixture-reader", "login_scope_id": "actual-login", "expires_at_ms": int(time.time() * 1000) + 60000},
            "realm": {"realm_id": "fixture-realm", "state_root": str(self.root / "state"),
                      "context_root": str(self.root / "context"), "user_config_root": str(self.root / "config")},
        }

    def child(self, mode="normal", **environment):
        child = ConsoleConnect(self.binary, self.bootstrap(), is_current=lambda: self.current,
            environment={**os.environ, "CONNECT_FIXTURE_MODE": mode, **environment})
        self.children.append(child)
        return child

    async def prompt(self, observer):
        while True:
            item = await asyncio.wait_for(anext(observer), 2)
            if item["type"] == "prompt":
                return item

    async def test_constructor_is_inactive_and_bootstrap_is_trusted_copy(self):
        value = self.bootstrap()
        child = ConsoleConnect(self.binary, value, is_current=lambda: self.current)
        self.assertIsNone(child._process)
        value["viewer"]["principal_id"] = "browser-nomination"
        self.assertEqual(child.bootstrap["viewer"]["principal_id"], "fixture-reader")
        with self.assertRaises(ValueError):
            ConsoleConnect("unacquired-connect", value, is_current=lambda: True)
        for key, mutation in [("unexpected", True), ("model", None)]:
            invalid = copy.deepcopy(value); invalid[key] = mutation
            with self.assertRaises(ValueError):
                _bootstrap(invalid)
        self.assertEqual(await child.close(), {"resources_closed": True})

    async def test_strict_json_duplicate_nonfinite_batch_and_unicode_controls(self):
        for frame in [b'{"a":1,"a":2}', b'{"body":NaN}', b'{"body":1e999}']:
            with self.assertRaises(ValueError):
                _decode(frame)
        value = self.bootstrap(); value["viewer"]["login_scope_id"] = "bad\x00identity"
        with self.assertRaises(ValueError):
            _bootstrap(value)
        value = self.bootstrap(); value["application"]["origin"] = "http://127.0.0.1:04110"
        with self.assertRaises(ValueError):
            _bootstrap(value)

    async def test_history_is_native_offset_projection_and_no_second_transcript(self):
        child = self.child(); await child.start()
        result = await child.history(8)
        self.assertEqual((result["history"]["offset"], result["history"]["total"], result["history"]["hasMore"]), (8, 71, True))
        self.assertEqual(len(result["history"]["messages"]), 1)
        self.assertNotIn("messages", child.__dict__)
        self.assertEqual((await child.history())["history"]["offset"], 17)
        with self.assertRaises(ConnectError) as error:
            await child.history(True)
        self.assertEqual(error.exception.code, "invalid_message")

    async def test_public_events_exclude_complete_private_prompt_and_remount_same_request(self):
        child = self.child("prompt"); await child.start()
        public = await child.attach(); private = await child.attach(private=True)
        self.assertEqual((await anext(private))["event"]["kind"], "state")
        send = await child.begin_send("Configure the optional channel")
        first = await self.prompt(private)
        self.assertIn("PRIVATE_PROPOSAL_CANARY", json.dumps(first))
        self.assertEqual(first["prompt"]["body"]["warning"], "Queued messages may be consumed")
        await private.close()
        remount = await child.attach(private=True)
        self.assertEqual((await anext(remount))["event"]["state"]["active_send"]["send_request_id"], send)
        self.assertEqual(await self.prompt(remount), first)
        await child.answer(first["id"], {"kind": "approval", "decision": "approve"})
        result = await child.observe_result(send)
        self.assertEqual(result["completion"], "completed")
        public_items = []
        while not public.queue.empty():
            public_items.append(public.queue.get_nowait())
        self.assertNotIn("PRIVATE_PROPOSAL_CANARY", json.dumps(public_items))
        self.assertNotIn("PRIVATE_STDERR_CANARY", json.dumps(public_items))
        self.assertFalse(any(item["type"] == "prompt" for item in public_items))
        with self.assertRaises(ConnectError) as duplicate:
            await child.answer(first["id"], {"kind": "approval", "decision": "deny"})
        self.assertEqual(duplicate.exception.code, "stale_prompt")
        self.assertIn("answers=1", (await child.history())["history"]["messages"][0]["text"])

    async def test_password_scalar_exactness_and_malformed_answer_is_not_denial(self):
        password = "  Exact \U0001f642 password \n"
        child = self.child("prompt", CONNECT_FIXTURE_PROMPT="forge_password", CONNECT_FIXTURE_PASSWORD=password)
        await child.start(); private = await child.attach(private=True)
        send = await child.begin_send("Sign into this origin")
        item = await self.prompt(private)
        for invalid in ["short", "abcdefghijkl\ud800"]:
            with self.assertRaises(ConnectError) as failure:
                await child.answer(item["id"], {"kind": "forge_password", "password": invalid})
            self.assertEqual(failure.exception.code, "invalid_answer")
        with self.assertRaises(ConnectError):
            await child.answer(item["id"], {"kind": "forge_password", "password": password, "actor": "other"})
        self.assertEqual(child.retained_result(send)["status"], "pending")
        await child.answer(item["id"], {"kind": "forge_password", "password": password})
        self.assertEqual((await child.observe_result(send))["completion"], "completed")
        self.assertNotIn(password, json.dumps(await child.history()))

    async def test_recovery_choice_and_origin_are_retained_not_browser_nominated(self):
        for kind, bad, good in [
            ("forge_recovery", {"kind": "forge_recovery", "choice": {"kind": "existing", "choice_id": "other-entry"}},
             {"kind": "forge_recovery", "choice": {"kind": "existing", "choice_id": "entry-1"}}),
            ("forge_origin", {"kind": "forge_origin", "origin": "http://127.0.0.1:4111"},
             {"kind": "forge_origin", "origin": "http://127.0.0.1:4110"}),
        ]:
            child = self.child("prompt", CONNECT_FIXTURE_PROMPT=kind)
            await child.start(); observer = await child.attach(private=True)
            send = await child.begin_send("Continue")
            item = await self.prompt(observer)
            with self.assertRaises(ConnectError) as failure:
                await child.answer(item["id"], bad)
            self.assertEqual(failure.exception.code, "invalid_answer")
            await child.answer(item["id"], good)
            await child.observe_result(send)

    async def test_disconnected_send_waiter_preserves_original_future_and_completion(self):
        child = self.child("pending"); await child.start()
        send = await child.begin_send("One actual input")
        self.assertEqual(child.snapshot()["state"]["active_send"], {
            "send_request_id": send, "cancellation_requested": False})
        self.assertFalse(child.snapshot()["state"]["can_send"])
        self.assertTrue(child.snapshot()["state"]["can_stop"])
        observer = await child.attach()
        await anext(observer)
        waiter = asyncio.create_task(child.observe_result(send))
        await asyncio.sleep(0)
        waiter.cancel()
        with self.assertRaises(asyncio.CancelledError):
            await waiter
        await observer.close()
        self.assertEqual(child.retained_result(send)["status"], "pending")
        with self.assertRaises(ConnectError) as busy:
            await child.begin_send("No second input")
        self.assertEqual(busy.exception.code, "busy")
        ack = await child.cancel(send)
        self.assertEqual(ack["status"], "cancellation_requested")
        self.assertEqual(child.retained_result(send)["status"], "pending")
        self.assertIsNone(child._state["last_completion"])
        result = await child.observe_result(send)
        self.assertEqual(result["completion"], "cancelled")
        remount = await child.attach()
        first = await anext(remount)
        self.assertEqual(first["event"]["state"]["last_completion"], {"send_request_id": send, "completion": "cancelled"})
        self.assertEqual(child.retained_result(send)["result"], result)

    async def test_all_completion_distinctions_preserved_without_payload_or_stopped_inference(self):
        kinds = ["completed", "completed_without_result", "callback_pending", "callback_batch_pending",
                 "cancelled", "abandoned", "abandoned_with_error", "completed_with_finalization_failure", "runtime_terminated"]
        for kind in kinds:
            child = self.child(CONNECT_FIXTURE_COMPLETION=kind); await child.start()
            result = await child.send("An exact input")
            self.assertEqual(result["completion"], kind)
            self.assertEqual(set(result), {"scope", "send_request_id", "completion", "state"})
            self.assertEqual(child.snapshot()["state"]["last_completion"]["completion"], kind)
            self.assertEqual((await child.cancel(result["send_request_id"]))["status"], "not_current")
            await child.close()

    async def test_late_history_and_history_refusal_keep_known_exact_completion(self):
        child = self.child("delayed_history"); await child.start()
        history = asyncio.create_task(child.history())
        await asyncio.sleep(0)
        result = await child.send("Complete while history is pending")
        await history
        self.assertEqual(child.snapshot()["state"]["last_completion"]["send_request_id"], result["send_request_id"])
        child = self.child("history_unavailable"); await child.start()
        result = await child.send("Complete before failed read")
        with self.assertRaises(ConnectError) as failure:
            await child.history()
        self.assertEqual(failure.exception.code, "history_unavailable")
        self.assertNotIn("PRIVATE_ERROR_CANARY", str(failure.exception))
        self.assertEqual(child.snapshot()["state"]["last_completion"]["send_request_id"], result["send_request_id"])
        self.assertFalse(child.snapshot()["state"]["outcome_uncertain"])

    async def test_retirement_fences_queued_answer_and_ack_is_not_joined_close(self):
        child = self.child("prompt"); await child.start()
        observer = await child.attach(private=True)
        send = await child.begin_send("Entered prompt")
        item = await self.prompt(observer)
        self.current = False
        ack = await child.retire("logout")
        self.assertEqual(ack["entered_work"], "settling")
        self.assertIsNone(child._process.returncode)
        self.assertFalse(child._resources_closed)
        with self.assertRaises(ConnectError) as failure:
            await child.answer(item["id"], {"kind": "approval", "decision": "approve"})
        self.assertEqual(failure.exception.code, "binding_retired")
        with self.assertRaises(StopAsyncIteration):
            await anext(observer)
        closed = await child.close()
        self.assertTrue(closed["resources_closed"])
        self.assertEqual(child._process.returncode, 0)
        self.assertTrue(all(task.done() for task in [child._reader_task, child._writer_task, child._stderr_task]))
        self.assertEqual(child._requests[send]["future"].result()["completion"], "cancelled")

    async def test_expiry_refuses_protected_projection_and_answer(self):
        child = self.child("prompt"); await child.start()
        private = await child.attach(private=True)
        await child.begin_send("Await protected input")
        item = await self.prompt(private)
        child.bootstrap["viewer"]["expires_at_ms"] = 0
        with self.assertRaises(ConnectError):
            await child.answer(item["id"], {"kind": "approval", "decision": "approve"})
        with self.assertRaises(ConnectError):
            await child.attach(private=True)
        await child.retire("login_expired")
        self.assertTrue((await child.close())["resources_closed"])

    async def test_pending_initialization_retire_and_close_are_responsive(self):
        child = self.child("pending_initialize")
        starting = asyncio.create_task(child.start())
        while child._initialize_id is None:
            await asyncio.sleep(0.001)
        starting.cancel()
        with self.assertRaises(asyncio.CancelledError):
            await starting
        ack = await asyncio.wait_for(child.retire("application_stopping"), 2)
        self.assertEqual(ack["target"], {"initialize_request_id": child._initialize_id})
        self.assertTrue((await asyncio.wait_for(child.close(), 2))["resources_closed"])
        self.assertTrue(child._start_task.done())

    async def test_pipe_failure_never_claims_shutdown_or_forwards_raw_data(self):
        for mode in ["duplicate_keys", "unexpected_null", "batch", "mutation_notification", "bad_event", "wrong_scope", "wrong_send", "bad_history"]:
            child = self.child(mode)
            try:
                await child.start()
                if mode == "bad_history":
                    await child.history()
                else:
                    await child.send("A single original input")
            except ConnectError as failure:
                self.assertNotIn("PRIVATE", str(failure))
            else:
                self.fail("Malformed protocol was accepted: " + mode)
            with self.assertRaises(ConnectError) as failure:
                await child.close()
            self.assertEqual(failure.exception.code, "shutdown_unconfirmed")
            self.assertEqual(child._process.returncode, 0)

    async def test_native_auth_refusal_remains_safe_and_failure_of_close_is_not_ack(self):
        child = self.child("native_auth_required")
        with self.assertRaises(ConnectError) as failure:
            await child.start()
        self.assertEqual(failure.exception.code, "native_auth_required")
        self.assertNotIn("PRIVATE_ERROR_CANARY", str(failure.exception))
        self.assertTrue((await child.close())["resources_closed"])
        child = self.child("failed_close"); await child.start()
        with self.assertRaises(ConnectError) as failure:
            await child.close()
        self.assertEqual(failure.exception.code, "shutdown_unconfirmed")
        self.assertEqual(child._process.returncode, 0)

    async def test_close_ack_and_failed_close_both_join_actual_child_after_caller_disconnect(self):
        for mode in ["delayed_close_exit", "delayed_failed_close"]:
            child = self.child(mode); await child.start()
            caller = asyncio.create_task(child.close())
            if mode == "delayed_close_exit":
                while not child._resources_closed:
                    await asyncio.sleep(0.001)
            else:
                while not any(row["method"] == "connect/close" and row["future"].done() for row in child._requests.values()):
                    await asyncio.sleep(0.001)
            self.assertIsNone(child._process.returncode)
            self.assertFalse(child._close_task.done())
            original_close = child._close_task
            caller.cancel()
            with self.assertRaises(asyncio.CancelledError):
                await caller
            if mode == "delayed_close_exit":
                self.assertTrue((await child.close())["resources_closed"])
            else:
                with self.assertRaises(ConnectError) as failure:
                    await child.close()
                self.assertEqual(failure.exception.code, "shutdown_unconfirmed")
            self.assertIs(child._close_task, original_close)
            self.assertEqual(child._process.returncode, 0)
            self.assertTrue(all(task.done() for task in [child._reader_task, child._writer_task, child._stderr_task]))

    async def test_queued_old_answer_rechecks_login_under_retirement_order(self):
        child = self.child("prompt"); await child.start()
        private = await child.attach(private=True)
        send = await child.begin_send("Await human approval")
        item = await self.prompt(private)
        async with child._ordering:
            answer = asyncio.create_task(child.answer(item["id"], {"kind": "approval", "decision": "approve"}))
            retire = asyncio.create_task(child.retire("logout"))
            await asyncio.sleep(0)
            self.current = False
        with self.assertRaises(ConnectError):
            await answer
        self.assertTrue((await retire)["prompts_retired"])
        await child.close()
        self.assertEqual(child._requests[send]["future"].result()["completion"], "cancelled")

    async def test_queued_prompt_is_not_disclosed_after_local_one_shot_claim(self):
        child = self.child("prompt"); await child.start()
        first = await child.attach(private=True)
        send = await child.begin_send("Await decision")
        prompt = await self.prompt(first)
        second = await child.attach(private=True)
        self.assertEqual((await anext(second))["event"]["kind"], "state")
        await child.answer(prompt["id"], {"kind": "approval", "decision": "deny"})
        await child.observe_result(send)
        # The queued private request is skipped after its one-shot claim. The
        # next view item is public closure/state, not stale protected content.
        self.assertEqual((await asyncio.wait_for(anext(second), 2))["type"], "event")

    async def test_initialization_transport_response_loss_does_not_block_joined_close(self):
        child = self.child("pending_initialize_no_reply")
        starting = asyncio.create_task(child.start())
        while child._initialize_id is None:
            await asyncio.sleep(0.001)
        future = child._requests[child._initialize_id]["future"]
        self.assertTrue((await asyncio.wait_for(child.close(), 2))["resources_closed"])
        with self.assertRaises(ConnectError) as failure:
            await starting
        self.assertEqual(failure.exception.code, "unavailable")
        self.assertTrue(future.done())

    async def test_original_forwarding_control_survives_earlier_child_state(self):
        child = self.child("pending"); await child.start()
        earlier = copy.deepcopy(child.snapshot()["state"])
        send = await child.begin_send("Retain despite a lost browser reply")
        # Exercise the ordered frame-retention boundary before any subprocess
        # admission notification can replace this original forwarding control.
        child._retain_state(earlier)
        self.assertEqual(child.snapshot()["state"]["active_send"]["send_request_id"], send)
        self.assertFalse(child.snapshot()["state"]["can_send"])
        self.assertTrue(child.snapshot()["state"]["can_stop"])
        observer = await child.attach()
        self.assertEqual((await anext(observer))["event"]["state"]["active_send"]["send_request_id"], send)
        self.assertEqual((await child.cancel(send))["status"], "cancellation_requested")
        self.assertEqual((await child.observe_result(send))["completion"], "cancelled")

    async def test_approval_expiry_accepts_only_timezone_bound_rfc3339(self):
        value = {"kind": "approval", "approval_id": "exact", "summary": "Complete",
                 "body": {"warnings": ["Retained complete private body"]},
                 "allowed_decisions": ["approve", "deny"], "expires_at": None}
        for text in ["2026-10-09T00:00:00Z", "2026-10-09t00:00:00.123z", "2026-10-09T00:00:00+02:00"]:
            value["expires_at"] = text
            self.assertEqual(_prompt(value, "http://127.0.0.1:4110")["expires_at"], text)
        for text in ["2026-10-09 00:00:00+00:00", "20261009T000000+0000", "2026-10-09T00:00:00", "2026-10-09T00:00:00+0000"]:
            value["expires_at"] = text
            with self.assertRaises(ValueError):
                _prompt(value, "http://127.0.0.1:4110")

    async def test_standalone_unavailable_fences_remount_and_original_pending_work(self):
        for reason in ["retired", "expired", "closing", "failed"]:
            for mode in ["standalone_unavailable", "standalone_unavailable_pending"]:
                with self.subTest(reason=reason, mode=mode):
                    child = self.child(mode, CONNECT_FIXTURE_REASON=reason)
                    await child.start()
                    original = None
                    if mode == "standalone_unavailable_pending":
                        private = await child.attach(private=True)
                        original = await child.begin_send("Await one protected decision")
                        prompt = await self.prompt(private)
                    received_history = await child.history()
                    self.assertFalse(received_history["state"]["available"])
                    state = child.snapshot()["state"]
                    self.assertFalse(state["available"])
                    self.assertFalse(state["can_send"])
                    self.assertFalse(state["can_stop"])
                    self.assertIsNone(state["last_completion"])
                    self.assertFalse(state["outcome_uncertain"])
                    remount = await child.attach(private=True)
                    self.assertFalse((await anext(remount))["event"]["state"]["available"])
                    self.assertTrue(remount.queue.empty())
                    count = len(child._requests)
                    with self.assertRaises(ConnectError) as send:
                        await child.begin_send("Do not enqueue after native unavailability")
                    self.assertEqual(send.exception.code, "unavailable")
                    with self.assertRaises(ConnectError) as history:
                        await child.history()
                    self.assertEqual(history.exception.code, "unavailable")
                    if original is not None:
                        self.assertEqual(state["active_send"]["send_request_id"], original)
                        self.assertEqual(child.retained_result(original)["status"], "pending")
                        with self.assertRaises(ConnectError) as cancel:
                            await child.cancel(original)
                        self.assertEqual(cancel.exception.code, "stop_unavailable")
                        with self.assertRaises(ConnectError) as answer:
                            await child.answer(prompt["id"], {"kind": "approval", "decision": "approve"})
                        self.assertEqual(answer.exception.code, "stale_prompt")
                    self.assertEqual(len(child._requests), count)
                    self.assertTrue((await child.close())["resources_closed"])
                    self.assertEqual(child._process.returncode, 0)
                    self.assertFalse(child._state["available"])
                    if original is not None:
                        self.assertEqual(child._requests[original]["future"].result()["completion"], "cancelled")
                        self.assertEqual(child._state["last_completion"]["send_request_id"], original)


SERVICE_PEER = r'''
import asyncio, json, os, sys

target = None
initial = None
shown = dict(model="fixture-model", provider="fixture-provider", realm="fixture-realm", binding="configured", profile="private")

def emit(value):
    print(json.dumps(value, separators=(",", ":")), flush=True)

async def main():
    global target, initial
    if sys.argv[1:] != ["--stdio"]:
        sys.exit(8)
    reader = asyncio.StreamReader()
    await asyncio.get_running_loop().connect_read_pipe(lambda: asyncio.StreamReaderProtocol(reader), sys.stdin)
    while True:
        line = await reader.readline()
        if not line:
            return
        value = json.loads(line)
        with open(os.environ["CONNECT_SERVICE_WIRE"], "a", encoding="utf-8") as record:
            record.write(json.dumps(value, ensure_ascii=False) + "\n")
        identity = value["id"]
        method = value.get("method")
        if method == "connect/initialize":
            initial = identity
            target = dict(initialize_request_id=identity)
            emit(dict(jsonrpc="2.0", id="p:pre-session", method="connect/prompt",
                      params=dict(target=target, prompt=dict(kind="provider_api_key", target=shown, max_bytes=8192))))
        elif method == "connect/retire":
            params = value["params"]
            emit(dict(jsonrpc="2.0", id=identity, result=dict(target=params["target"], retirement_id=params["retirement_id"],
                      admission_closed=True, prompts_retired=True, entered_work="settled")))
        elif method == "connect/close":
            emit(dict(jsonrpc="2.0", id=initial, error=dict(code=-32010, message="Fixture private auth was not completed", data=dict(code="native_auth_required"))))
            emit(dict(jsonrpc="2.0", id=identity, result=dict(target=value["params"]["target"], resources_closed=True)))
        # An answered prompt does not fabricate an initialized native session,
        # stored credential, native auth completion or successful service work.

asyncio.run(main())
'''


CANCEL_ORDER_PEER = r'''
import asyncio, json, os, sys

initial = None
target = None
state = dict(available=False, can_send=False, can_stop=False, outcome_uncertain=False,
             active_send=None, last_completion=None, private_notice_pending=False)

def emit(value):
    print(json.dumps(value, separators=(",", ":")), flush=True)

def event(kind, **fields):
    emit(dict(jsonrpc="2.0", method="connect/event", params=dict(target=target, kind=kind, **fields)))

async def main():
    global initial, target
    if sys.argv[1:] != ["--stdio"]:
        sys.exit(8)
    reader = asyncio.StreamReader()
    await asyncio.get_running_loop().connect_read_pipe(lambda: asyncio.StreamReaderProtocol(reader), sys.stdin)
    while True:
        line = await reader.readline()
        if not line:
            return
        value = json.loads(line)
        with open(os.environ["CONNECT_SERVICE_WIRE"], "a", encoding="utf-8") as record:
            record.write(json.dumps(value, ensure_ascii=False) + "\n")
        identity = value["id"]
        method = value.get("method")
        if method == "connect/initialize":
            initial = identity
            target = dict(initialize_request_id=identity)
            event("state", state=state)
            emit(dict(jsonrpc="2.0", id="p:cancel-order", method="connect/prompt",
                      params=dict(target=target, prompt=dict(kind="provider_api_key",
                          target=dict(model="fixture-model", provider="fixture-provider", realm="fixture-realm", binding="configured"), max_bytes=8192))))
        elif identity == "p:cancel-order" and value.get("result") == dict(answer=dict(kind="cancel")):
            # Same source order as Toolkit Driver::answer -> cancel_current ->
            # fence. Keep the original initialization pending for joined close.
            event("unavailable", reason="retired")
            event("state", state=state)
        elif method == "connect/close":
            event("prompt_closed", prompt_id="p:cancel-order", disposition="cancelled")
            emit(dict(jsonrpc="2.0", id=initial, error=dict(code=-32010,
                      message="Fixture initialization ended", data=dict(code="binding_retired"))))
            emit(dict(jsonrpc="2.0", id=identity, result=dict(target=target, resources_closed=True)))

asyncio.run(main())
'''


class ConsoleServiceContractTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.binary = self.root / "service-protocol-peer"
        self.binary.write_text("#!" + sys.executable + "\n" + SERVICE_PEER)
        self.binary.chmod(0o700)
        self.current = True
        self.children = []
        self.shown = {"model": "fixture-model", "provider": "fixture-provider", "realm": "fixture-realm", "binding": "configured", "profile": "private"}

    async def asyncTearDown(self):
        for child in self.children:
            try:
                await asyncio.wait_for(child.close(), 4)
            except ConnectError:
                pass
        self.temporary.cleanup()

    def bootstrap(self):
        return {"protocol_version": 3, "adapter": "forge_services",
            "application": {"application_id": "reading", "instance_id": "separate-lifetime", "origin": "http://127.0.0.1:4110"},
            "viewer": {"principal_id": "fixture-reader", "login_scope_id": "separate-login", "expires_at_ms": int(time.time() * 1000) + 60000},
            "realm": {"realm_id": "fixture-realm", "state_root": str(self.root / "state"), "context_root": str(self.root / "context"), "user_config_root": str(self.root / "user")},
            "services": {"resource_manifest_sha256": "a" * 64, "preparation": "permitted"}}

    def child(self):
        child = ConsoleConnect(self.binary, self.bootstrap(), is_current=lambda: self.current,
            environment={"CONNECT_SERVICE_WIRE": str(self.root / "wire.jsonl")})
        self.children.append(child)
        return child

    def approval(self):
        return {"kind": "approval", "approval_id": "native-record", "summary": "Exact native change", "body": {"warnings": ["Retain all original warnings"]}, "allowed_decisions": ["deny"], "expires_at": None}

    async def test_service_v3_closed_bootstrap_does_not_widen_v1_or_select_arbitrary_resources(self):
        source = self.bootstrap(); accepted = _bootstrap(source)
        source["services"]["resource_manifest_sha256"] = "b" * 64
        self.assertEqual(accepted["services"]["resource_manifest_sha256"], "a" * 64)
        for mutation in (
            {"adapter": "homecore"}, {"protocol_version": 2}, {"protocol_version": True},
            {"services": {"resource_manifest_sha256": "A" * 64, "preparation": "permitted"}},
            {"services": {"resource_manifest_sha256": "a" * 63, "preparation": "permitted"}},
            {"services": {"resource_manifest_sha256": "a" * 64, "preparation": True}},
            {"services": {"resource_manifest_sha256": "a" * 64, "preparation": "permitted", "path": "/browser-nominated"}},
            {"provider_key": "not-a-bootstrap-secret"},
        ):
            with self.subTest(mutation=mutation), self.assertRaises(ValueError):
                _bootstrap({**self.bootstrap(), **mutation})
        legacy = self.bootstrap(); legacy["protocol_version"] = 1
        with self.assertRaises(ValueError):
            _bootstrap(legacy)
        legacy.pop("services"); legacy.pop("adapter")
        self.assertEqual(_bootstrap(legacy)["protocol_version"], 1)

    async def test_service_v3_prompt_unions_preserve_owner_values_and_reject_extra_authority(self):
        prompts = [
            {"kind": "provider_target", "choices": [{"choice_id": "offered", "target": self.shown, "status": "Native display label only"}]},
            {"kind": "provider_action", "target": self.shown, "actions": ["use", "enter_key", "sign_in", "back"]},
            {"kind": "provider_api_key", "target": self.shown, "max_bytes": 8192},
            {"kind": "provider_browser", "target": self.shown, "authorization_url": "https://provider.invalid/exact?state=owner"},
            {"kind": "provider_device", "target": self.shown, "verification_uri": "https://provider.invalid/device", "user_code": "EXACT CODE"}, self.approval(),
        ]
        for prompt in prompts:
            with self.subTest(kind=prompt["kind"]):
                self.assertEqual(_prompt(prompt, "http://127.0.0.1:4110", protocol_version=3), prompt)
                with self.assertRaises(ValueError):
                    _prompt({**prompt, "grant": "browser-authority"}, "http://127.0.0.1:4110", protocol_version=3)
                if prompt["kind"] != "approval":
                    with self.assertRaises(ValueError):
                        _prompt(prompt, "http://127.0.0.1:4110")
        bad = {"kind": "provider_target", "choices": [{"choice_id": "one", "target": self.shown, "status": ""}] * 2}
        with self.assertRaises(ValueError): _prompt(bad, "http://127.0.0.1:4110", protocol_version=3)
        for actions in [["install"], ["use", "use"], [True]]:
            with self.assertRaises(ValueError): _prompt({"kind": "provider_action", "target": self.shown, "actions": actions}, "http://127.0.0.1:4110", protocol_version=3)

    async def test_service_v3_answers_select_only_offered_objects_and_never_supply_auth_success(self):
        choice = {"kind": "provider_target", "choices": [{"choice_id": "offered", "target": self.shown, "status": "display only"}]}
        action = {"kind": "provider_action", "target": self.shown, "actions": ["enter_key", "back"]}
        self.assertEqual(_answer({"kind": "provider_target", "choice_id": "offered"}, choice, protocol_version=3), {"kind": "provider_target", "choice_id": "offered"})
        for answer, prompt in [({"kind": "provider_target", "choice_id": "other"}, choice),
                               ({"kind": "provider_target", "choice_id": "offered", "target": self.shown}, choice),
                               ({"kind": "provider_action", "action": "sign_in"}, action),
                               ({"kind": "approval", "decision": "approve"}, self.approval())]:
            with self.assertRaises(ValueError): _answer(answer, prompt, protocol_version=3)
        for kind in ("provider_browser", "provider_device"):
            prompt = {"kind": kind, "target": self.shown}
            self.assertEqual(_answer({"kind": "cancel"}, prompt, protocol_version=3), {"kind": "cancel"})
            with self.assertRaises(ValueError): _answer({"kind": kind, "success": True}, prompt, protocol_version=3)

    async def test_service_v3_api_key_is_exact_utf8_bounded_nonblank_and_not_a_console_password(self):
        prompt = {"kind": "provider_api_key", "target": self.shown, "max_bytes": 8192}
        exact = "  exact-fixture-key-\U0001f30c  "
        self.assertEqual(_answer({"kind": "provider_api_key", "key": exact}, prompt, protocol_version=3)["key"], exact)
        self.assertEqual(len(_answer({"kind": "provider_api_key", "key": "\u00e9" * 4096}, prompt, protocol_version=3)["key"].encode()), 8192)
        for key in ("", "   ", "\u00e9" * 4097, "x\n", "x\t", "x\x00", "x\x7f", "x\x85"):
            with self.assertRaises(ValueError): _answer({"kind": "provider_api_key", "key": key}, prompt, protocol_version=3)
        with self.assertRaises(ValueError): _answer({"kind": "provider_api_key", "key": exact, "store": "browser-path"}, prompt, protocol_version=3)
        with self.assertRaises(ValueError): _prompt({"kind": "forge_password", "origin": "http://127.0.0.1:4110", "mode": "initial", "min_chars": 12, "max_chars": 4096}, "http://127.0.0.1:4110", protocol_version=3)

    async def test_service_v3_notice_state_is_required_typed_and_not_silently_a_v1_field(self):
        state = {"available": True, "can_send": False, "can_stop": False, "outcome_uncertain": False, "active_send": None, "last_completion": None, "private_notice_pending": True}
        self.assertEqual(_state(state, protocol_version=3), state)
        with self.assertRaises(ValueError): _state(state)
        invalid = copy.deepcopy(state); invalid.pop("private_notice_pending")
        with self.assertRaises(ValueError): _state(invalid, protocol_version=3)
        invalid = copy.deepcopy(state); invalid["private_notice_pending"] = "true"
        with self.assertRaises(ValueError): _state(invalid, protocol_version=3)

    async def test_service_v3_pre_session_private_binding_precedes_same_retained_prompt_and_one_reply(self):
        child = self.child(); private = await child.attach(private=True)
        self.assertIsNone(child.presentation_binding()); self.assertTrue(private.queue.empty())
        with self.assertRaises(ConnectError): await child.attach()
        entered = asyncio.create_task(child.start())
        binding = await asyncio.wait_for(anext(private), 2)
        self.assertEqual(binding, {"type": "binding", "protocol_version": 3, "target": {"initialize_request_id": child._initialize_id}})
        item = await asyncio.wait_for(anext(private), 2)
        self.assertEqual(item["type"], "prompt"); self.assertEqual(item["target"], binding["target"])
        self.assertEqual(item["protocol_version"], 3); self.assertNotIn("scope", item)
        self.assertFalse(entered.done()); self.assertIsNone(child._scope)
        await private.close(); remount = await child.attach(private=True)
        self.assertEqual(await anext(remount), binding); self.assertEqual(await anext(remount), item)
        with self.assertRaises(ConnectError): await child.answer(item["id"], {"kind": "provider_api_key", "key": "wrong\nkey"})
        self.assertIn(item["id"], child._prompts)
        exact = "  synthetic-private-key-\U0001f30c  "
        await child.answer(item["id"], {"kind": "provider_api_key", "key": exact})
        with self.assertRaises(ConnectError): await child.answer(item["id"], {"kind": "provider_api_key", "key": exact})
        closed = await child.close(); self.assertTrue(closed["resources_closed"])
        with self.assertRaises(ConnectError) as result: await entered
        self.assertEqual(result.exception.code, "native_auth_required")
        wire = [json.loads(line) for line in (self.root / "wire.jsonl").read_text().splitlines()]
        self.assertEqual(sum(row.get("method") == "connect/initialize" for row in wire), 1)
        answers = [row for row in wire if row["id"] == item["id"] and "result" in row]
        self.assertEqual(answers, [{"jsonrpc": "2.0", "id": item["id"], "result": {"answer": {"kind": "provider_api_key", "key": exact}}}])
        self.assertEqual(child._process.returncode, 0)

    async def test_service_v3_wrong_initialization_or_scope_target_never_rebinds_or_discloses(self):
        child = self.child(); observer = await child.attach(private=True)
        child._initialize_id = "h:retained"
        def incoming(identity, target):
            child._receive({"jsonrpc": "2.0", "id": identity, "method": "connect/prompt", "params": {"target": target, "prompt": self.approval()}})
        for index, target in enumerate(({"initialize_request_id": "h:other"}, {"scope": "browser-scope"}, {"initialize_request_id": "h:retained", "scope": "extra"})):
            incoming(f"p:wrong-{index}", target)
            self.assertTrue(observer.queue.empty()); self.assertFalse(child._prompts); self.assertIsNone(child._scope)
        incoming("p:exact", {"initialize_request_id": "h:retained"})
        self.assertEqual((await anext(observer))["target"], {"initialize_request_id": "h:retained"})
        # This is a direct scope-validation control, not a native session receipt.
        child._scope = "retained-native-scope"; child._initialized = True
        with self.assertRaises(ConnectError): await child.answer("p:exact", {"kind": "approval", "decision": "deny"})
        incoming("p:old-init", {"initialize_request_id": "h:retained"})
        incoming("p:other-scope", {"scope": "other-native-scope"})
        self.assertTrue(observer.queue.empty())
        incoming("p:scope-exact", {"scope": "retained-native-scope"})
        self.assertEqual((await anext(observer))["target"], {"scope": "retained-native-scope"})
        self.assertEqual(child._scope, "retained-native-scope")

    async def test_service_v3_retirement_refuses_late_private_answer_and_close_joins_original_initialization(self):
        child = self.child(); observer = await child.attach(private=True)
        entered = asyncio.create_task(child.start())
        binding = await asyncio.wait_for(anext(observer), 2); item = await asyncio.wait_for(anext(observer), 2)
        ack = await child.retire("logout")
        self.assertEqual(ack["target"], binding["target"]); self.assertEqual(ack["entered_work"], "settled")
        with self.assertRaises(ConnectError): await child.answer(item["id"], {"kind": "provider_api_key", "key": "late-synthetic-key"})
        self.assertFalse(entered.done()); self.assertIsNone(child._process.returncode)
        self.assertTrue((await child.close())["resources_closed"])
        with self.assertRaises(ConnectError): await entered
        wire = [json.loads(line) for line in (self.root / "wire.jsonl").read_text().splitlines()]
        self.assertFalse(any(row["id"] == item["id"] and "result" in row for row in wire))
        self.assertEqual(sum(row.get("method") == "connect/close" for row in wire), 1)
        self.assertEqual(child._process.returncode, 0)

    async def test_service_v3_cancel_closure_preserves_original_refusal_after_unavailable_and_joined_close(self):
        self.binary.write_text("#!" + sys.executable + "\n" + CANCEL_ORDER_PEER)
        child = self.child(); observer = await child.attach(private=True)
        original = asyncio.create_task(child.start())
        original.add_done_callback(lambda done: done.exception() if not done.cancelled() else None)
        binding = await asyncio.wait_for(anext(observer), 2)
        initial_state = await asyncio.wait_for(anext(observer), 2)
        prompt = await asyncio.wait_for(anext(observer), 2)
        self.assertEqual(initial_state["event"]["target"], binding["target"])
        self.assertEqual(prompt["target"], binding["target"])
        await child.answer(prompt["id"], {"kind": "cancel"})
        frames = []
        while not frames or frames[-1]["event"]["kind"] != "unavailable":
            frames.append(await asyncio.wait_for(anext(observer), 2))
        self.assertTrue(child._unavailable); self.assertFalse(child._failed)
        self.assertFalse(original.done()); self.assertIsNone(child._scope)
        retained = child._prompts.get(prompt["id"])
        self.assertIsNotNone(retained, "The unavailable fence must preserve exact native closure custody.")
        self.assertTrue(retained["answered"]); self.assertTrue(retained["retired"])
        with self.assertRaises(ConnectError) as duplicate_answer:
            await child.answer(prompt["id"], {"kind": "cancel"})
        self.assertEqual(duplicate_answer.exception.code, "stale_prompt")
        with self.assertRaises(ConnectError): await child.begin_send("No second input")
        remount = await child.attach(private=True)
        self.assertEqual(await anext(remount), binding)
        replayed_state = await anext(remount)
        self.assertFalse(replayed_state["event"]["state"]["can_send"])
        self.assertFalse(replayed_state["event"]["state"]["can_stop"])
        self.assertFalse(any(item["type"] == "prompt" for item in list(remount.queue._queue)))
        for target in ({"initialize_request_id": "h:other"}, {"scope": "guessed-scope"}):
            with self.assertRaises(ValueError):
                child._event({"target": target, "kind": "prompt_closed", "prompt_id": prompt["id"], "disposition": "cancelled"})
            self.assertIs(child._prompts[prompt["id"]], retained)
        closed = await asyncio.wait_for(child.close(), 4)
        self.assertTrue(closed["resources_closed"]); self.assertFalse(child._failed)
        with self.assertRaises(ConnectError) as completed: await original
        self.assertEqual(completed.exception.code, "binding_retired")
        self.assertEqual(child._requests[child._initialize_id]["future"].exception().code, "binding_retired")
        self.assertIsNone(child._scope); self.assertNotIn(prompt["id"], child._prompts)
        with self.assertRaises(ValueError):
            child._event({"target": binding["target"], "kind": "prompt_closed", "prompt_id": prompt["id"], "disposition": "cancelled"})
        wire = [json.loads(line) for line in (self.root / "wire.jsonl").read_text().splitlines()]
        self.assertEqual([row.get("method") for row in wire if "method" in row], ["connect/initialize", "connect/close"])
        self.assertEqual([row for row in wire if row["id"] == prompt["id"]], [{"jsonrpc": "2.0", "id": prompt["id"], "result": {"answer": {"kind": "cancel"}}}])
        self.assertEqual(child._process.returncode, 0)
        # Direct legacy projection control; no legacy subprocess/session exists.
        legacy_bootstrap = self.bootstrap(); legacy_bootstrap["protocol_version"] = 1
        legacy_bootstrap.pop("adapter"); legacy_bootstrap.pop("services")
        legacy = ConsoleConnect(self.binary, legacy_bootstrap, is_current=lambda: self.current)
        self.children.append(legacy)
        legacy._scope = "fixture-legacy-scope"
        legacy._prompts["p:legacy"] = self.approval(); legacy._seen_prompts.add("p:legacy")
        legacy._event({"scope": legacy._scope, "kind": "unavailable", "reason": "retired"})
        self.assertFalse(legacy._prompts)


if __name__ == "__main__":
    unittest.main()
