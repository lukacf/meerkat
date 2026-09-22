"""No PortAudio, microphone, speaker, or provider is used by these tests."""

import asyncio
import base64
from contextlib import redirect_stdout
import io
import sys
import threading
import types
import unittest
from unittest.mock import patch

from main import AudioFormat, TranscriptPrinter, build_arg_parser, live_receiver, speaker_player
from playback import Playback


class Device:
    def __init__(self, **_kwargs):
        self.active = False
        self.entered = threading.Event()
        self.released = threading.Event()
        self.fresh = threading.Event()
        self.writes = []
        self.aborts = 0
        self.closed = 0

    def start(self):
        self.active = True

    def write(self, data):
        self.writes.append(data)
        if data == b"blocked":
            self.entered.set()
            if not self.released.wait(2):
                raise TimeoutError("test device did not receive a priority abort")
            raise RuntimeError("device aborted")
        self.fresh.set()

    def abort(self):
        self.aborts += 1
        self.active = False
        self.released.set()

    def stop(self):
        self.active = False

    def close(self):
        self.closed += 1


class Connection:
    def __init__(self):
        self.events = asyncio.Queue()

    async def recv(self):
        return await self.events.get()

    def audio(self, data, **identity):
        self.events.put_nowait({
            "observation": "assistant_audio_chunk",
            "data": base64.b64encode(data).decode(), **identity,
        })


class BufferedDevice(Device):
    def __init__(self, *, block_drain=False):
        super().__init__()
        self.buffered = []
        self.heard = []
        self.draining = threading.Event()
        self.drain_released = threading.Event()
        self.block_drain = block_drain
        self.drains = 0

    def write(self, data):
        self.writes.append(data)
        self.buffered.append(data)
        self.fresh.set()

    def stop(self):
        self.drains += 1
        self.draining.set()
        if self.block_drain and not self.drain_released.wait(2):
            raise TimeoutError("drain did not receive priority abort")
        self.heard.extend(self.buffered)
        self.buffered.clear()
        self.active = False

    def abort(self):
        super().abort()
        self.buffered.clear()
        self.drain_released.set()


class PlaybackTests(unittest.IsolatedAsyncioTestCase):
    async def wait_for(self, predicate):
        async with asyncio.timeout(2):
            while not predicate():
                await asyncio.sleep(0.001)

    async def exercise(self, interruption, identity):
        device = Device()
        playback = Playback()
        connection = Connection()
        stop = asyncio.Event()
        with patch.dict(sys.modules, {"sounddevice": types.SimpleNamespace(RawOutputStream=lambda **kw: device)}), redirect_stdout(io.StringIO()):
            speaker = asyncio.create_task(speaker_player(
                AudioFormat("audio/pcm", 24000, 1), playback, build_arg_parser().parse_args([]), TranscriptPrinter(),
            ))
            receiver = asyncio.create_task(live_receiver(connection, playback, TranscriptPrinter(), stop))
            try:
                connection.audio(b"blocked", **identity)
                self.assertTrue(await asyncio.to_thread(device.entered.wait, 2))
                connection.audio(b"queued", **identity)
                connection.events.put_nowait(interruption)
                connection.audio(b"late", **identity)
                connection.audio(b"fresh", response_id="new", item_id="new-item", content_index=0)
                self.assertTrue(await asyncio.to_thread(device.fresh.wait, 2))
                self.assertGreater(device.aborts, 0)
                self.assertEqual(device.writes, [b"blocked", b"fresh"])
                connection.events.put_nowait(None)
                await asyncio.wait_for(asyncio.gather(receiver, speaker), 2)
                self.assertEqual(device.closed, 1)
            finally:
                await playback.close()
                receiver.cancel()
                speaker.cancel()
                await asyncio.gather(receiver, speaker, return_exceptions=True)
        self.assertEqual(device.closed, 1)

    async def test_interrupt_bypasses_blocked_write_and_discards_stale_response(self):
        await self.exercise(
            {"observation": "turn_interrupted", "response_id": "old"},
            {"response_id": "old", "item_id": "old-item", "content_index": 0},
        )

    async def test_truncation_without_interrupt_retains_item_identity(self):
        await self.exercise(
            {"observation": "assistant_transcript_truncated", "provider_item_id": "old-item", "content_index": 0},
            {"response_id": "old", "item_id": "old-item", "content_index": 0},
        )

    async def test_missing_identity_uses_active_output(self):
        await self.exercise({"observation": "turn_interrupted"}, {})

    async def test_cancellation_aborts_and_joins_worker_before_device_close(self):
        device = Device()
        playback = Playback()
        worker = asyncio.create_task(playback.run(device))
        playback.enqueue(b"blocked", {"response_id": "old"})
        self.assertTrue(await asyncio.to_thread(device.entered.wait, 2))
        worker.cancel()
        with self.assertRaises(asyncio.CancelledError):
            await asyncio.wait_for(worker, 2)
        self.assertEqual(device.closed, 1)
        self.assertTrue(device.released.is_set())

    async def test_completed_write_buffer_is_aborted_and_other_items_survive_truncation(self):
        device = Device()
        playback = Playback()
        worker = asyncio.create_task(playback.run(device))
        try:
            old = {"response_id": "same", "item_id": "first", "content_index": 0}
            playback.enqueue(b"buffered", old)
            self.assertTrue(await asyncio.to_thread(device.fresh.wait, 2))
            await playback.interrupt({
                "observation": "assistant_transcript_truncated",
                "response_id": "same", "provider_item_id": "first", "content_index": 0,
            })
            self.assertEqual(device.aborts, 1)
            device.fresh.clear()
            playback.enqueue(b"late", old)
            playback.enqueue(b"other-item", {"response_id": "same", "item_id": "second", "content_index": 0})
            self.assertTrue(await asyncio.to_thread(device.fresh.wait, 2))
            self.assertEqual(device.writes, [b"buffered", b"other-item"])
        finally:
            await playback.close()
            await asyncio.wait_for(worker, 2)
        self.assertEqual(device.closed, 1)

    async def test_no_speaker_and_text_probe_do_not_import_or_open_sounddevice(self):
        for flag in ("--no-speaker", "--text-probe"):
            playback = Playback(enabled=False)
            with patch.dict(sys.modules, {"sounddevice": None}):
                worker = asyncio.create_task(speaker_player(
                    None, playback, build_arg_parser().parse_args([flag]), TranscriptPrinter(),
                ))
                playback.enqueue(b"discarded", {"response_id": "any"})
                await playback.interrupt({})
                await playback.close()
                await asyncio.wait_for(worker, 1)
            self.assertEqual(len(playback._queue), 0)

    async def test_response_interrupt_rejects_known_item_only_late_audio(self):
        playback = Playback()
        playback.enqueue(b"old", {"response_id": "old", "item_id": "old-item", "content_index": 0})
        await playback.interrupt({"observation": "turn_interrupted", "response_id": "old"})
        playback.enqueue(b"late-old-item", {"item_id": "old-item", "content_index": 0})
        playback.enqueue(b"late-without-index", {"item_id": "old-item"})
        playback.enqueue(b"conflicting-identity", {"item_id": "old-item", "response_id": "new"})
        playback.enqueue(b"fresh", {"response_id": "new", "item_id": "new-item", "content_index": 0})
        device = BufferedDevice()
        worker = asyncio.create_task(playback.run(device))
        try:
            await self.wait_for(lambda: len(device.writes) > 0)
            self.assertEqual(device.writes, [b"fresh"])
        finally:
            await playback.close()
            await worker

    async def test_new_scope_is_not_written_until_old_device_buffer_is_drained(self):
        device = BufferedDevice()
        playback = Playback()
        worker = asyncio.create_task(playback.run(device))
        try:
            playback.enqueue(b"old-buffered", {"response_id": "old"})
            await self.wait_for(lambda: len(device.writes) == 1)
            playback.enqueue(b"new-buffered", {"response_id": "new"})
            await self.wait_for(lambda: len(device.writes) == 2)
            self.assertEqual(device.drains, 1)
            self.assertEqual(device.heard, [b"old-buffered"])
            self.assertEqual(device.buffered, [b"new-buffered"])
            await playback.interrupt({"observation": "turn_interrupted", "response_id": "old"})
            self.assertEqual(device.buffered, [b"new-buffered"])
            self.assertEqual(device.aborts, 0, "old output has a real drain receipt; do not abort new output")
            playback.enqueue(b"new-tail", {"response_id": "new"})
            await self.wait_for(lambda: len(device.writes) == 3)
            self.assertEqual(device.buffered, [b"new-buffered", b"new-tail"])
        finally:
            await playback.close()
            await worker

    async def test_priority_interrupt_aborts_old_drain_without_losing_new_queued_response(self):
        device = BufferedDevice(block_drain=True)
        playback = Playback()
        worker = asyncio.create_task(playback.run(device))
        try:
            playback.enqueue(b"old-buffered", {"response_id": "old"})
            await self.wait_for(lambda: len(device.writes) == 1)
            playback.enqueue(b"fresh", {"response_id": "new"})
            self.assertTrue(await asyncio.to_thread(device.draining.wait, 2))
            self.assertEqual(device.writes, [b"old-buffered"])
            await playback.interrupt({"observation": "turn_interrupted", "response_id": "old"})
            await self.wait_for(lambda: len(device.writes) == 2)
            self.assertEqual(device.aborts, 1)
            self.assertEqual(device.heard, [])
            self.assertEqual(device.buffered, [b"fresh"])
        finally:
            await playback.close()
            await worker

    async def test_shutdown_aborts_blocked_drain_and_joins_worker(self):
        device = BufferedDevice(block_drain=True)
        playback = Playback()
        worker = asyncio.create_task(playback.run(device))
        playback.enqueue(b"old-buffered", {"response_id": "old"})
        await self.wait_for(lambda: len(device.writes) == 1)
        playback.enqueue(b"not-started", {"response_id": "new"})
        self.assertTrue(await asyncio.to_thread(device.draining.wait, 2))
        await asyncio.wait_for(playback.close(), 2)
        await asyncio.wait_for(worker, 2)
        self.assertEqual(device.writes, [b"old-buffered"])
        self.assertEqual(device.closed, 1)

    async def test_unrelated_drain_failure_surfaces_instead_of_claiming_playback(self):
        class FailedDrain(BufferedDevice):
            def stop(self):
                raise RuntimeError("synthetic drain failure")

        device = FailedDrain()
        playback = Playback()
        playback.enqueue(b"old", {"response_id": "old"})
        playback.enqueue(b"new", {"response_id": "new"})
        with self.assertRaisesRegex(RuntimeError, "synthetic drain failure"):
            await asyncio.wait_for(playback.run(device), 2)
        self.assertEqual(device.writes, [b"old"])
        self.assertEqual(device.closed, 1)

    async def test_item_truncation_rejects_ambiguous_partial_audio_but_preserves_identified_sibling(self):
        playback = Playback()
        playback.enqueue(b"old", {"response_id": "r", "item_id": "item", "content_index": 0})
        await playback.interrupt({
            "observation": "assistant_transcript_truncated", "response_id": "r",
            "provider_item_id": "item", "content_index": 0,
        })
        playback.enqueue(b"missing-index", {"item_id": "item"})
        playback.enqueue(b"missing-item", {"response_id": "r"})
        playback.enqueue(b"valid-content", {"item_id": "item", "content_index": 1})
        device = BufferedDevice()
        worker = asyncio.create_task(playback.run(device))
        try:
            await self.wait_for(lambda: len(device.writes) > 0)
            self.assertEqual(device.writes, [b"valid-content"])
        finally:
            await playback.close()
            await worker


if __name__ == "__main__":
    unittest.main()
