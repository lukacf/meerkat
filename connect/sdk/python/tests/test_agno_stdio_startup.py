"""Offline stdio startup ownership controls with real profiles and joined workers."""

from __future__ import annotations

import asyncio
from contextlib import asynccontextmanager
import threading
import unittest
from unittest.mock import AsyncMock, patch

from mcp.server import Server

from meerkat_toolkit.adapters import agno_mcp
from meerkat_toolkit.adapters.agno_profiles import AgnoMcpProfile, validate_configuration


class StdioStartupTests(unittest.IsolatedAsyncioTestCase):
    def setUp(self) -> None:
        self.configuration = validate_configuration(
            "hackernews", operations=["get_top_hackernews_stories"], config={"timeout": 2},
        )
        self.streams = (object(), object())
        self.events = []
        self.active = False
        self.profiles = []
        self.closed = []
        self.build_profile = agno_mcp.build_profile
        self.close_profile = AgnoMcpProfile.close

    @asynccontextmanager
    async def stdio(self):
        self.active = True
        self.events.append("stdio entered")
        try:
            yield self.streams
        finally:
            self.events.append("stdio exited")
            self.active = False

    def construct(self, name, *, operations, config):
        self.assertTrue(self.active, "stdio must be owned before selected donor construction")
        self.events.append("construct")
        profile = self.build_profile(name, operations=operations, config=config)
        self.profiles.append(profile)
        return profile

    def close(self, profile):
        self.assertTrue(self.active, "stdio must remain owned until profile cleanup completes")
        self.events.append("close")
        self.closed.append(profile)
        self.close_profile(profile)

    async def test_construction_allows_event_loop_progress_before_returning(self) -> None:
        loop = asyncio.get_running_loop()
        heartbeat = threading.Event()

        def construct(*args, **kwargs):
            loop.call_soon_threadsafe(heartbeat.set)
            # This is a deadlock guard, not a timing assertion. The callback
            # must run while synchronous construction is still in progress.
            self.assertTrue(heartbeat.wait(3), "selected construction blocked the event loop")
            return self.construct(*args, **kwargs)

        with patch("mcp.server.stdio.stdio_server", self.stdio), \
             patch.object(agno_mcp, "build_profile", side_effect=construct), \
             patch.object(AgnoMcpProfile, "close", autospec=True, side_effect=self.close), \
             patch.object(Server, "run", new_callable=AsyncMock):
            await agno_mcp.serve_stdio(self.configuration)
        self.assertEqual(self.closed, self.profiles)
        self.assertEqual(len(self.closed), 1)

    async def test_normal_serving_retains_selected_profile_and_closes_once(self) -> None:
        real_create_server = agno_mcp.create_server

        def create_server(profile):
            self.assertIs(profile, self.profiles[0])
            self.assertTrue(profile.thread_handoff_safe)
            self.assertEqual([item["name"] for item in profile.descriptions],
                             ["get_top_hackernews_stories"])
            self.events.append("create server")
            return real_create_server(profile)

        async def run(read_stream, write_stream, options):
            self.assertEqual((read_stream, write_stream), self.streams)
            self.assertFalse(self.closed)
            self.events.append("serve")

        with patch("mcp.server.stdio.stdio_server", self.stdio), \
             patch.object(agno_mcp, "build_profile", side_effect=self.construct) as constructor, \
             patch.object(agno_mcp, "create_server", side_effect=create_server), \
             patch.object(AgnoMcpProfile, "close", autospec=True, side_effect=self.close), \
             patch.object(Server, "run", new_callable=AsyncMock, side_effect=run) as serving:
            await agno_mcp.serve_stdio(self.configuration)
        constructor.assert_called_once_with("hackernews", operations=self.configuration.operations,
                                            config=self.configuration.config)
        serving.assert_awaited_once()
        self.assertEqual(self.closed, self.profiles)
        self.assertEqual(self.events,
                         ["stdio entered", "construct", "create server", "serve", "close", "stdio exited"])

    async def test_cancel_during_construction_retains_late_profile_through_repeated_cancel(self) -> None:
        loop = asyncio.get_running_loop()
        constructing = asyncio.Event()
        closing = asyncio.Event()
        release_construction = threading.Event()
        release_close = threading.Event()

        def construct(*args, **kwargs):
            loop.call_soon_threadsafe(constructing.set)
            if not release_construction.wait(3):
                raise AssertionError("test did not release construction")
            return self.construct(*args, **kwargs)

        def close(profile):
            self.close(profile)
            loop.call_soon_threadsafe(closing.set)
            if not release_close.wait(3):
                raise AssertionError("test did not release cleanup")

        with patch("mcp.server.stdio.stdio_server", self.stdio), \
             patch.object(agno_mcp, "build_profile", side_effect=construct), \
             patch.object(agno_mcp, "create_server") as create_server, \
             patch.object(AgnoMcpProfile, "close", autospec=True, side_effect=close):
            task = asyncio.create_task(agno_mcp.serve_stdio(self.configuration))
            try:
                await asyncio.wait_for(constructing.wait(), 3)
                task.cancel()
                await asyncio.sleep(0)
                task.cancel()
                await asyncio.sleep(0)
                self.assertFalse(task.done(), "cancellation must join construction")
                self.assertTrue(self.active)
                self.assertFalse(self.closed)
                create_server.assert_not_called()
                release_construction.set()
                await asyncio.wait_for(closing.wait(), 3)
                self.assertEqual(self.closed, self.profiles)
                self.assertEqual(len(self.closed), 1)
                task.cancel()
                await asyncio.sleep(0)
                task.cancel()
                await asyncio.sleep(0)
                self.assertFalse(task.done(), "repeated cancellation must join cleanup")
                self.assertTrue(self.active)
                create_server.assert_not_called()
                release_close.set()
                with self.assertRaises(asyncio.CancelledError):
                    await task
                self.assertFalse(self.active)
                self.assertEqual(len(self.closed), 1)
            finally:
                release_construction.set()
                release_close.set()
                await asyncio.gather(task, return_exceptions=True)
        self.assertEqual(self.events, ["stdio entered", "construct", "close", "stdio exited"])

    async def test_constructor_failure_preserves_failure_without_inventing_a_profile(self) -> None:
        failure = RuntimeError("controlled constructor failure")

        def fail(*args, **kwargs):
            self.assertTrue(self.active)
            raise failure

        with patch("mcp.server.stdio.stdio_server", self.stdio), \
             patch.object(agno_mcp, "build_profile", side_effect=fail), \
             patch.object(agno_mcp, "create_server") as create_server, \
             patch.object(AgnoMcpProfile, "close", autospec=True) as close:
            with self.assertRaises(RuntimeError) as raised:
                await agno_mcp.serve_stdio(self.configuration)
        self.assertIs(raised.exception, failure)
        create_server.assert_not_called()
        close.assert_not_called()
        self.assertFalse(self.active)
        self.assertEqual(self.events, ["stdio entered", "stdio exited"])


if __name__ == "__main__":
    unittest.main()
