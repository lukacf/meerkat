"""Finite queue behavior around a real asyncio execution lock."""

from __future__ import annotations

import asyncio
import time
import unittest

from meerkat_toolkit.adapters._mcp_admission import AdmissionRefused, CallAdmission


async def eventually(predicate) -> None:
    async with asyncio.timeout(2):
        while not predicate():
            await asyncio.sleep(0)


class AdmissionTests(unittest.IsolatedAsyncioTestCase):
    async def test_default_capacity_refuses_excess_and_cancelled_waiter_returns_capacity(self):
        admission = CallAdmission(asyncio.Lock())
        entered = []

        async def wait(label):
            async with admission.enter():
                entered.append(label)

        pending = []
        try:
            async with admission.enter():
                pending = [asyncio.create_task(wait(label)) for label in range(3)]
                await eventually(lambda: admission._admitted == 4)
                with self.assertRaises(AdmissionRefused) as raised:
                    async with admission.enter():
                        self.fail("excess request entered")
                self.assertEqual(raised.exception.kind, "server_busy")
                pending[1].cancel()
                with self.assertRaises(asyncio.CancelledError):
                    await pending[1]
                pending.append(asyncio.create_task(wait(3)))
                await eventually(lambda: admission._admitted == 4)
                self.assertEqual(entered, [])
            await asyncio.gather(*pending, return_exceptions=True)
            self.assertEqual(entered, [0, 2, 3])
            self.assertEqual(admission._admitted, 0)
            async with admission.enter():
                self.assertTrue(admission._lock.locked())
        finally:
            for task in pending:
                task.cancel()
            await asyncio.gather(*pending, return_exceptions=True)

    async def test_queue_deadline_does_not_time_out_executing_owner(self):
        admission = CallAdmission(asyncio.Lock(), queue_timeout_seconds=0.03)
        entered = []

        async def wait():
            async with admission.enter():
                entered.append("expired")

        async with admission.enter():
            pending = asyncio.create_task(wait())
            with self.assertRaises(AdmissionRefused) as raised:
                await asyncio.wait_for(pending, 1)
            self.assertEqual(raised.exception.kind, "queue_timeout")
            self.assertTrue(admission._lock.locked())
            self.assertEqual(admission._admitted, 1)
            await asyncio.sleep(0.04)
            self.assertEqual(entered, [])
        async with admission.enter():
            entered.append("following")
        self.assertEqual(entered, ["following"])

    async def test_deadline_is_rechecked_when_acquisition_returns_before_timer_callback(self):
        class DelayedLock(asyncio.Lock):
            async def acquire(self):
                acquired = await super().acquire()
                # Model non-preemptible event-loop work between grant and
                # return. The timeout callback cannot run during this interval.
                time.sleep(0.03)
                return acquired

        lock = DelayedLock()
        admission = CallAdmission(lock, queue_timeout_seconds=0.01)
        with self.assertRaises(AdmissionRefused) as raised:
            async with admission.enter():
                self.fail("an overdue lock grant entered the operation")
        self.assertEqual(raised.exception.kind, "queue_timeout")
        self.assertFalse(lock.locked())
        self.assertEqual(admission._admitted, 0)

    async def test_cancel_at_lock_release_does_not_leak_capacity_or_execution_lock(self):
        lock = asyncio.Lock()
        admission = CallAdmission(lock, maximum_admitted=1)
        await lock.acquire()

        async def wait():
            async with admission.enter():
                self.fail("canceled waiter entered")

        pending = asyncio.create_task(wait())
        await eventually(lambda: admission._admitted == 1)
        lock.release()
        pending.cancel()
        with self.assertRaises(asyncio.CancelledError):
            await pending
        self.assertEqual(admission._admitted, 0)
        async with admission.enter():
            self.assertTrue(lock.locked())

    async def test_close_during_wait_prevents_entry_and_refuses_new_calls(self):
        lock = asyncio.Lock()
        admission = CallAdmission(lock)
        closed = False
        await lock.acquire()

        async def wait():
            async with admission.enter(is_closed=lambda: closed):
                self.fail("closed owner admitted work")

        pending = asyncio.create_task(wait())
        await eventually(lambda: admission._admitted == 1)
        closed = True
        with self.assertRaises(AdmissionRefused) as raised:
            await wait()
        self.assertEqual(raised.exception.kind, "closed")
        lock.release()
        with self.assertRaises(AdmissionRefused) as raised:
            await pending
        self.assertEqual(raised.exception.kind, "closed")
        self.assertEqual(admission._admitted, 0)
        self.assertFalse(lock.locked())

    async def test_body_timeout_is_preserved_and_releases_owner(self):
        admission = CallAdmission(asyncio.Lock())
        failure = TimeoutError("provider operation timeout")
        with self.assertRaises(TimeoutError) as raised:
            async with admission.enter():
                raise failure
        self.assertIs(raised.exception, failure)
        async with admission.enter():
            self.assertEqual(admission._admitted, 1)

    def test_invalid_limits_are_refused(self):
        for value in (True, 0, -1, 1.5, "4"):
            with self.subTest(maximum=value), self.assertRaises(ValueError):
                CallAdmission(asyncio.Lock(), maximum_admitted=value)
        for value in (True, 0, -1, float("nan"), float("inf"), "30"):
            with self.subTest(timeout=value), self.assertRaises(ValueError):
                CallAdmission(asyncio.Lock(), queue_timeout_seconds=value)
