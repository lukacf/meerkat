"""Bounded requests through the official MCP client and a real owned worker."""

from __future__ import annotations

import asyncio
from contextlib import asynccontextmanager
import threading
import unittest
from unittest.mock import patch

from agno.tools import Toolkit
from jsonschema import Draft202012Validator
from mcp import Client

from meerkat_toolkit.adapters import agno_mcp
from meerkat_toolkit.adapters.agno import AgnoToolkitAdapter
from meerkat_toolkit.adapters.agno_profiles import AgnoMcpProfile


async def eventually(predicate):
    async with asyncio.timeout(3):
        while not predicate():
            await asyncio.sleep(0.005)


def profile_for(operation, effect="read"):
    bridge = AgnoToolkitAdapter(Toolkit(tools=[operation]), functions={"operation": effect})
    return AgnoMcpProfile("fixture", bridge, tuple(bridge.describe()), thread_handoff_safe=True)


def observed_server(profile, seconds=30.0):
    original = agno_mcp.CallAdmission
    gates = []

    def create(lock):
        gate = original(lock, queue_timeout_seconds=seconds)
        gates.append(gate)
        return gate

    with patch.object(agno_mcp, "CallAdmission", side_effect=create):
        server = agno_mcp.create_server(profile)
    return server, gates[0]


@asynccontextmanager
async def joined_client(server, release, tasks):
    async with Client(server) as client:
        try:
            yield client
        finally:
            # Release the fixture before MCP shutdown starts joining requests,
            # including when an assertion fails while a worker is held.
            release.set()
            await asyncio.gather(*tasks, return_exceptions=True)


class AgnoAdmissionTests(unittest.IsolatedAsyncioTestCase):
    async def test_default_saturation_refuses_before_schema_work_for_reads_and_writes(self):
        for effect in ("read", "write"):
            with self.subTest(effect=effect):
                entered = []
                release = threading.Event()

                def operation(label: str) -> str:
                    entered.append(label)
                    if label == "active" and not release.wait(5):
                        raise RuntimeError("fixture release absent")
                    return label

                profile = profile_for(operation, effect)
                server, gate = observed_server(profile)
                tasks = []
                try:
                    async with joined_client(server, release, tasks) as client:
                        tasks.append(asyncio.create_task(client.call_tool("operation", {"label": "active"})))
                        await eventually(lambda: entered == ["active"])
                        tasks.extend(asyncio.create_task(client.call_tool("operation", {"label": str(i)})) for i in range(3))
                        await eventually(lambda: gate._admitted == 4)
                        self.assertTrue(all(not task.done() for task in tasks))
                        with patch.object(Draft202012Validator, "validate", side_effect=AssertionError("unexpected schema work")) as validate:
                            result = await asyncio.wait_for(client.call_tool("operation", {"label": 42}), 1)
                        validate.assert_not_called()
                        self.assertEqual(result.structured_content["error"], {
                            "kind": "server_busy", "effect": effect, "entered": False,
                            "write_outcome": "not_applicable" if effect == "read" else "not_started",
                        })
                        self.assertEqual(entered, ["active"])
                        await asyncio.wait_for(client.list_tools(), 1)
                        release.set()
                        values = await asyncio.wait_for(asyncio.gather(*tasks), 2)
                        self.assertEqual([value.structured_content["value"] for value in values], ["active", "0", "1", "2"])
                        self.assertEqual(entered, ["active", "0", "1", "2"])
                finally:
                    release.set()
                    await asyncio.gather(*tasks, return_exceptions=True)
                    profile.close()

    async def test_expired_write_never_enters_and_active_call_outlives_queue_budget(self):
        entered = []
        release = threading.Event()

        def operation(label: str) -> str:
            entered.append(label)
            if label == "active" and not release.wait(5):
                raise RuntimeError("fixture release absent")
            return label

        profile = profile_for(operation, "write")
        server, gate = observed_server(profile, seconds=0.1)
        tasks = []
        active = None
        try:
            async with joined_client(server, release, tasks) as client:
                active = asyncio.create_task(client.call_tool("operation", {"label": "active"}))
                tasks.append(active)
                await eventually(lambda: entered == ["active"])
                result = await asyncio.wait_for(client.call_tool("operation", {"label": "expired"}), 1)
                self.assertEqual(result.structured_content["error"], {
                    "kind": "queue_timeout", "effect": "write", "entered": False, "write_outcome": "not_started",
                })
                self.assertFalse(active.done())
                self.assertEqual(gate._admitted, 1)
                self.assertEqual(entered, ["active"])
                release.set()
                await active
                result = await client.call_tool("operation", {"label": "following"})
                self.assertEqual(result.structured_content["value"], "following")
                self.assertEqual(entered, ["active", "following"])
                self.assertEqual(gate._admitted, 0)
        finally:
            release.set()
            if active is not None:
                await asyncio.gather(active, return_exceptions=True)
            profile.close()

    async def test_repeated_active_cancellation_retains_slot_until_worker_joins(self):
        entered = []
        release = threading.Event()

        def operation(label: str) -> str:
            entered.append(label)
            if label == "active" and not release.wait(5):
                raise RuntimeError("fixture release absent")
            return label

        profile = profile_for(operation)
        server, gate = observed_server(profile, seconds=0.1)
        tasks = []
        active = None
        try:
            async with joined_client(server, release, tasks) as client:
                active = asyncio.create_task(client.call_tool("operation", {"label": "active"}))
                tasks.append(active)
                await eventually(lambda: entered == ["active"])
                active.cancel()
                await asyncio.sleep(0.02)
                active.cancel()
                result = await asyncio.wait_for(client.call_tool("operation", {"label": "expired"}), 1)
                self.assertEqual(result.structured_content["error"]["kind"], "queue_timeout")
                self.assertFalse(active.done())
                self.assertEqual(gate._admitted, 1)
                self.assertEqual(entered, ["active"])
                release.set()
                with self.assertRaises(asyncio.CancelledError):
                    await active
                result = await client.call_tool("operation", {"label": "following"})
                self.assertEqual(result.structured_content["value"], "following")
                self.assertEqual(entered, ["active", "following"])
        finally:
            release.set()
            if active is not None:
                await asyncio.gather(active, return_exceptions=True)
            profile.close()

    async def test_busy_profile_does_not_consume_another_profiles_capacity(self):
        release = threading.Event()
        entered = threading.Event()

        def operation(label: str) -> str:
            if label == "held":
                entered.set()
                if not release.wait(5):
                    raise RuntimeError("fixture release absent")
            return label

        first, second = profile_for(operation), profile_for(operation)
        tasks = []
        active = None
        try:
            async with joined_client(agno_mcp.create_server(first), release, tasks) as a, Client(agno_mcp.create_server(second)) as b:
                active = asyncio.create_task(a.call_tool("operation", {"label": "held"}))
                tasks.append(active)
                await eventually(entered.is_set)
                result = await asyncio.wait_for(b.call_tool("operation", {"label": "independent"}), 1)
                self.assertEqual(result.structured_content["value"], "independent")
                self.assertFalse(active.done())
                release.set()
                await active
        finally:
            release.set()
            if active is not None:
                await asyncio.gather(active, return_exceptions=True)
            first.close()
            second.close()
