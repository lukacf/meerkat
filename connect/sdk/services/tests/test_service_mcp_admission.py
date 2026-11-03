"""Admission at the selected service profile's existing serial MCP owner."""

from __future__ import annotations

import asyncio
import base64
import importlib
import os
import unittest
from unittest.mock import patch

import httpx
from mcp import Client


NAMESPACE = os.environ.get("TOOLKIT_SERVICES_TEST_NAMESPACE", "meerkat_toolkit")
adapter = importlib.import_module(NAMESPACE + ".adapters.services_mcp")
config_module = importlib.import_module(NAMESPACE + ".adapters.services_config")


async def eventually(predicate):
    async with asyncio.timeout(3):
        while not predicate():
            await asyncio.sleep(0.005)


def selected_profile(effect, operations, client=None):
    config = {"token_env": "SELECTED_TOKEN", "timeout_seconds": 3, "max_response_bytes": 65536}
    if effect == "write":
        config.update(user_id="fixture@example.invalid", max_message_bytes=1024)
    selected = config_module.validate_configuration({
        "profile": "gmail" if effect == "write" else "slack",
        "operations": ["send_raw" if effect == "write" else "auth_test"], "config": config,
    })
    return adapter.ServiceProfile(selected, operations, client or httpx.AsyncClient())


def call(client, effect, label="fixture"):
    if effect == "write":
        return client.call_tool("gmail_send_raw", {"mime_base64": base64.b64encode(label.encode()).decode()})
    return client.call_tool("slack_auth_test", {})


def refusal(kind, effect):
    return {"kind": kind, "effect": effect, "operation_invoked": False,
            "write_outcome": "not_attempted" if effect == "write" else "not_applicable"}


class ServiceAdmissionTests(unittest.IsolatedAsyncioTestCase):
    async def test_default_capacity_is_shared_between_servers_and_precedes_argument_copy(self):
        for effect in ("read", "write"):
            with self.subTest(effect=effect):
                release, entered = asyncio.Event(), []

                class Operations:
                    async def auth_test(self):
                        entered.append("read")
                        await release.wait()
                        return {"ok": True}

                    async def send_raw(self, mime):
                        entered.append(mime.decode())
                        await release.wait()
                        return {"id": "m1", "threadId": "t1"}

                profile = selected_profile(effect, Operations())
                tasks = []
                try:
                    async with Client(adapter.create_server(profile)) as first, Client(adapter.create_server(profile)) as second:
                        try:
                            tasks.append(asyncio.create_task(call(first, effect, "active")))
                            await eventually(lambda: len(entered) == 1)
                            tasks.extend(asyncio.create_task(call(client, effect, str(i)))
                                         for i, client in enumerate((second, first, second)))
                            await eventually(lambda: profile._admission._admitted == 4)
                            with patch.object(adapter, "deepcopy", side_effect=AssertionError("arguments copied")) as copy:
                                for client in (first, second):
                                    result = await asyncio.wait_for(call(client, effect, "excess"), 1)
                                    self.assertEqual(result.structured_content["error"], refusal("server_busy", effect))
                            copy.assert_not_called()
                            await asyncio.wait_for(first.list_tools(), 1)
                            self.assertEqual(len(entered), 1)
                            tasks[2].cancel()
                            with self.assertRaises(asyncio.CancelledError):
                                await tasks[2]
                            await eventually(lambda: profile._admission._admitted == 3)
                            tasks.append(asyncio.create_task(call(second, effect, "replacement")))
                            await eventually(lambda: profile._admission._admitted == 4)
                            release.set()
                            results = await asyncio.wait_for(asyncio.gather(*tasks, return_exceptions=True), 2)
                            self.assertEqual(sum(not result.is_error for result in results if not isinstance(result, BaseException)), 4)
                            self.assertEqual(len(entered), 4)
                            if effect == "write":
                                self.assertEqual(entered, ["active", "0", "2", "replacement"])
                            self.assertEqual(profile._admission._admitted, 0)
                        finally:
                            release.set()
                            await asyncio.gather(*tasks, return_exceptions=True)
                finally:
                    await profile.aclose()

    async def test_queue_timeout_is_pre_entry_and_does_not_expire_executing_owner(self):
        for effect in ("read", "write"):
            with self.subTest(effect=effect):
                release, entered = asyncio.Event(), []

                class Operations:
                    async def auth_test(self):
                        entered.append("read")
                        await release.wait()
                        return {"ok": True}

                    async def send_raw(self, mime):
                        entered.append(mime.decode())
                        await release.wait()
                        return {"id": "m1"}

                profile = selected_profile(effect, Operations())
                profile._admission = adapter.CallAdmission(profile._lock, queue_timeout_seconds=0.05)
                active = None
                try:
                    async with Client(adapter.create_server(profile)) as client:
                        try:
                            active = asyncio.create_task(call(client, effect, "active"))
                            await eventually(lambda: len(entered) == 1)
                            result = await asyncio.wait_for(call(client, effect, "expired"), 1)
                            self.assertEqual(result.structured_content["error"], refusal("queue_timeout", effect))
                            self.assertFalse(active.done())
                            self.assertTrue(profile._lock.locked())
                            self.assertEqual(profile._admission._admitted, 1)
                            self.assertEqual(len(entered), 1)
                            release.set()
                            self.assertFalse((await active).is_error)
                            self.assertFalse((await call(client, effect, "following")).is_error)
                            self.assertEqual(len(entered), 2)
                            if effect == "write":
                                self.assertEqual(entered, ["active", "following"])
                        finally:
                            release.set()
                            if active is not None:
                                await asyncio.gather(active, return_exceptions=True)
                finally:
                    await profile.aclose()

    async def test_close_and_queue_deadline_refuse_waiters_while_repeated_cancel_joins_owner(self):
        release, entered, closed = asyncio.Event(), [], []

        class Operations:
            async def send_raw(self, mime):
                entered.append(mime.decode())
                await release.wait()
                return {"id": "m1"}

        class OwnedClient(httpx.AsyncClient):
            async def aclose(self):
                closed.append("closed")
                await super().aclose()

        profile = selected_profile("write", Operations(), OwnedClient())
        profile._admission = adapter.CallAdmission(profile._lock, queue_timeout_seconds=0.1)
        tasks, closing = [], None
        try:
            async with Client(adapter.create_server(profile)) as client:
                try:
                    tasks.append(asyncio.create_task(call(client, "write", "active")))
                    await eventually(lambda: entered == ["active"])
                    tasks.extend(asyncio.create_task(call(client, "write", str(i))) for i in range(2))
                    await eventually(lambda: profile._admission._admitted == 3)
                    closing = asyncio.create_task(profile.aclose())
                    await eventually(lambda: profile._close_task is not None)
                    closing.cancel()
                    await asyncio.sleep(0.01)
                    closing.cancel()
                    with patch.object(adapter, "deepcopy", side_effect=AssertionError("closed arguments copied")) as copy:
                        result = await call(client, "write", "closed")
                    copy.assert_not_called()
                    self.assertEqual(result.structured_content["error"], refusal("closed", "write"))
                    for pending in tasks[1:]:
                        result = await asyncio.wait_for(pending, 1)
                        self.assertEqual(result.structured_content["error"], refusal("closed", "write"))
                    self.assertFalse(tasks[0].done())
                    self.assertFalse(closing.done())
                    self.assertEqual(closed, [])
                    self.assertEqual(profile._admission._admitted, 1)
                    release.set()
                    self.assertFalse((await tasks[0]).is_error)
                    with self.assertRaises(asyncio.CancelledError):
                        await closing
                    self.assertEqual(entered, ["active"])
                    self.assertEqual(profile._admission._admitted, 0)
                    self.assertEqual(closed, ["closed"])
                finally:
                    release.set()
                    await asyncio.gather(*tasks, *([closing] if closing else []), return_exceptions=True)
        finally:
            await profile.aclose()
        self.assertEqual(closed, ["closed"])

    async def test_saturated_selected_profile_does_not_consume_other_profiles_capacity(self):
        release, entered = asyncio.Event(), asyncio.Event()

        class HeldOperations:
            async def auth_test(self):
                entered.set()
                await release.wait()
                return {"ok": True, "team_id": "held"}

        class OtherOperations:
            async def auth_test(self):
                return {"ok": True, "team_id": "independent"}

        first, second = selected_profile("read", HeldOperations()), selected_profile("read", OtherOperations())
        tasks = []
        try:
            async with Client(adapter.create_server(first)) as a, Client(adapter.create_server(second)) as b:
                try:
                    tasks.append(asyncio.create_task(call(a, "read")))
                    await entered.wait()
                    tasks.extend(asyncio.create_task(call(a, "read")) for _ in range(3))
                    await eventually(lambda: first._admission._admitted == 4)
                    self.assertEqual((await call(a, "read")).structured_content["error"], refusal("server_busy", "read"))
                    result = await asyncio.wait_for(call(b, "read"), 1)
                    self.assertEqual(result.structured_content["value"]["team_id"], "independent")
                    self.assertTrue(all(not task.done() for task in tasks))
                finally:
                    release.set()
                    await asyncio.gather(*tasks, return_exceptions=True)
        finally:
            await first.aclose()
            await second.aclose()
