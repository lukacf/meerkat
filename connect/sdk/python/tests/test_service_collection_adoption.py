"""Original-pipe SDK controls with real subprocesses, not protected-host proof."""
from __future__ import annotations

import dataclasses
import io
import json
import os
from pathlib import Path
import select
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from unittest import mock

from meerkat_toolkit.service_collection import (
    CollectionStatus, ServiceCollectionClient, ServiceCollectionTransport,
)
from meerkat_toolkit.service_setup import (
    ServiceSetupBinding, ServiceSetupError, ServiceSetupLaunchError,
)
from test_service_collection import PEER


@unittest.skipUnless(os.name == "posix", "original pipe adoption requires POSIX descriptors")
class OriginalCollectionPipeTests(unittest.TestCase):
    def setUp(self):
        self.root = Path(tempfile.mkdtemp(prefix="connect-original-pipe-test-"))
        self.processes = []
        self.clients = []
        self.binding = ServiceSetupBinding(
            "app", "instance", "http://127.0.0.1:1234", "viewer", "login",
            int(time.time() * 1000) + 60_000, "realm", str(self.root / "context"),
            str(self.root / "state"), str(self.root / "config"), "a" * 64,
            {"kind": "file", "root": str(self.root / "credentials")},
        )

    def tearDown(self):
        # This test is the external process owner. It disposes only its fixtures.
        for process, peer in self.processes:
            peer.with_suffix(".resume").touch()
        for client in self.clients:
            try:
                client.close(2)
            except (ServiceSetupError, TimeoutError):
                pass
        for process, _ in self.processes:
            if process.poll() is None:
                process.terminate()
            process.wait(3)
        for client in self.clients:
            client.close(3)
            self.assertFalse(client._reader.is_alive())
            self.assertFalse(client._writer.is_alive())
        for process, _ in self.processes:
            self.assertNotEqual(process.returncode, 74, "fixture watchdog fired")
            process.stdin.close()
            process.stdout.close()
        shutil.rmtree(self.root)

    def prepare(self, mode="normal", *, retire=None, wait=None):
        peer = self.root / ("peer-" + str(len(self.processes)))
        peer.write_text(PEER)
        peer.with_suffix(".mode").write_text(mode)
        peer.with_suffix(".frames").write_text("")
        process = subprocess.Popen(
            [sys.executable, str(peer)], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL, bufsize=0, close_fds=True,
        )
        self.processes.append((process, peer))
        observations = []

        def retirement():
            observations.append({"output_closed": process.stdout.closed})
            if retire is not None:
                retire(process)

        transport = ServiceCollectionTransport(
            input=process.stdin, output=process.stdout, request_retirement=retirement,
            wait_process=wait or (lambda timeout: process.wait(timeout=timeout)),
            credential_scope=self.binding.credential_scope,
        )
        return transport, process, peer, observations

    def adopt(self, transport, *, ready=True):
        try:
            client = ServiceCollectionClient.adopt(transport, self.binding)
        except ServiceSetupLaunchError as error:
            self.clients.append(error.owner)
            raise
        self.clients.append(client)
        if ready:
            self.until(client, lambda e: e.kind == "state" and e.payload["state"]["can_collect"])
        return client

    def until(self, client, predicate):
        deadline = time.monotonic() + 3
        while True:
            event = client.next_event(max(.001, deadline - time.monotonic()))
            if predicate(event):
                return event
            if time.monotonic() >= deadline:
                self.fail("expected wire observation missing")

    def test_original_pid_pipes_and_reports_without_spawn(self):
        for mode, status in (("normal", CollectionStatus.HISTORICAL_CHECKS_COMPLETED),
                             ("failed", CollectionStatus.TOOL_FAILED),
                             ("cleanup", CollectionStatus.CLEANUP_UNCONFIRMED)):
            with self.subTest(mode=mode):
                transport, process, peer, retired = self.prepare(mode)
                pid = process.pid
                os.set_inheritable(process.stdin.fileno(), True)
                os.set_inheritable(process.stdout.fileno(), True)
                with mock.patch("meerkat_toolkit.service_setup.subprocess.Popen", side_effect=AssertionError("spawn")):
                    client = self.adopt(transport)
                    self.assertIs(client._process.stdin, process.stdin)
                    self.assertIs(client._process.stdout, process.stdout)
                    self.assertFalse(os.get_inheritable(process.stdin.fileno()))
                    self.assertFalse(os.get_inheritable(process.stdout.fileno()))
                    request = client.collect(job_id="native-job", source_id="opaque-a")
                    event = self.until(client, lambda e: e.request_id == request and e.report is not None)
                    self.assertEqual(event.report.status, status)
                    self.assertEqual(event.report.source_id, "opaque-a")
                    self.assertTrue(client.close(3).native_resources_closed)
                self.assertEqual(json.loads(peer.with_suffix(".record").read_text())["pid"], pid)
                self.assertEqual(retired, [{"output_closed": True}])
                self.assertTrue(process.stdin.closed and process.stdout.closed)

    def test_preclaim_binding_refusal_leaves_original_usable_and_single_claim(self):
        transport, process, _, _ = self.prepare()
        wrong = dataclasses.replace(self.binding, credential_scope={"kind": "native_default"})
        with self.assertRaises(ValueError):
            ServiceCollectionClient.adopt(transport, wrong)
        self.assertFalse(process.stdin.closed or process.stdout.closed)
        self.assertIsNone(process.poll())
        client = self.adopt(transport)
        with self.assertRaises(ServiceSetupError):
            ServiceCollectionClient.adopt(transport, self.binding)
        self.assertTrue(client.close(3).native_resources_closed)

    def test_close_timeout_keeps_owner_and_does_not_fence_graceful_close(self):
        transport, process, _, retired = self.prepare("slow_close")
        client = self.adopt(transport)
        identities = client._process, client._reader, client._writer
        with self.assertRaises(TimeoutError):
            client.close(.01)
        self.assertEqual((client._process, client._reader, client._writer), identities)
        self.assertEqual(retired, [])
        self.assertIsNone(process.poll())
        self.assertTrue(client.close(3).native_resources_closed)
        self.assertEqual(retired, [{"output_closed": True}])

    def test_malformed_output_disposes_reader_before_retirement_and_wait(self):
        transport, process, _, retired = self.prepare("bad_state_extra")
        client = self.adopt(transport, ready=False)
        with self.assertRaises(ServiceSetupError):
            self.until(client, lambda e: e.kind == "result")
        # Parser failure notifies observers before its reader finally runs.
        # Isolate that cleanup from close's independent failed-enqueue path.
        deadline = time.monotonic() + 2
        while not retired and time.monotonic() < deadline:
            time.sleep(.005)
        self.assertEqual(retired, [{"output_closed": True}])
        result = client.close(3)
        self.assertFalse(result.native_resources_closed)
        self.assertTrue(result.process_joined)
        self.assertEqual(retired, [{"output_closed": True}])
        self.assertTrue(process.stdout.closed)

    def test_failed_close_enqueue_retires_root_and_joins_blocked_writer(self):
        transport, process, peer, retired = self.prepare("backpressure", retire=lambda p: p.terminate())
        client = self.adopt(transport)
        deadline = time.monotonic() + 2
        while not peer.with_suffix(".blocked").exists() and time.monotonic() < deadline:
            time.sleep(.005)
        self.assertTrue(peer.with_suffix(".blocked").exists())
        writing = threading.Event()
        written = threading.Event()
        original_write = client._write

        def write(frame):
            writing.set()
            try:
                original_write(frame)
            finally:
                written.set()

        with mock.patch.object(client, "_write", write):
            with client._condition:
                client._enqueue(b"x" * (1024 * 1024))
                client._condition.notify_all()
            self.assertTrue(writing.wait(1))
            deadline = time.monotonic() + 2
            while select.select([], [process.stdin], [], 0)[1] and time.monotonic() < deadline:
                time.sleep(.005)
            self.assertFalse(select.select([], [process.stdin], [], 0)[1], "pipe never reached OS backpressure")
            self.assertFalse(written.is_set(), "frame was not retained in flight")
            self.assertIsNone(process.poll())
            with mock.patch.object(client, "_request", side_effect=ServiceSetupError("queue unavailable")):
                result = client.close(3)
        self.assertTrue(result.process_joined)
        self.assertFalse(result.native_resources_closed)
        self.assertEqual(len(retired), 1)
        self.assertTrue(process.stdin.closed and process.stdout.closed)

    def test_cancel_enqueue_failure_requests_retirement_outside_condition(self):
        holder = {}
        unlocked = []

        def retire(process):
            unlocked.append(not holder["client"]._condition._is_owned())
            process.terminate()

        transport, _, _, retired = self.prepare("pending", retire=retire)
        client = self.adopt(transport)
        holder["client"] = client
        request = client.collect(job_id="native-job", source_id="opaque-a")
        self.until(client, lambda e: e.kind == "state" and e.payload["state"]["active_collection"] == request)
        with mock.patch.object(client, "_request", side_effect=ServiceSetupError("queue unavailable")):
            with self.assertRaises(ServiceSetupError):
                client.cancel(request)
        self.assertFalse(client.close(3).native_resources_closed)
        self.assertEqual(unlocked, [True])
        self.assertEqual(len(retired), 1)

    def test_cancel_answer_enqueue_failure_retires_outside_condition(self):
        holder = {}
        unlocked = []

        def retire(process):
            unlocked.append(not holder["client"]._condition._is_owned())
            process.terminate()

        transport, _, _, retired = self.prepare("early", retire=retire)
        client = self.adopt(transport, ready=False)
        holder["client"] = client
        event = self.until(client, lambda e: e.kind == "prompt")
        with mock.patch.object(client, "_enqueue", side_effect=ServiceSetupError("queue unavailable")):
            with self.assertRaises(ServiceSetupError):
                client.answer(event.request_id, {"kind": "cancel"})
        self.assertFalse(client.close(3).native_resources_closed)
        self.assertEqual(unlocked, [True])
        self.assertEqual(len(retired), 1)

    def test_stdout_eof_retires_original_without_native_close_credit(self):
        transport, process, _, retired = self.prepare("eof")
        client = self.adopt(transport, ready=False)
        # Observe EOF before close can introduce a competing failed write.
        with self.assertRaises(EOFError):
            self.until(client, lambda e: False)
        result = client.close(3)
        self.assertTrue(result.process_joined)
        self.assertFalse(result.native_resources_closed)
        self.assertEqual(retired, [{"output_closed": True}])
        self.assertTrue(process.stdin.closed and process.stdout.closed)

    def test_partial_worker_start_failure_retains_the_exact_owner(self):
        original_start = threading.Thread.start
        for failing_worker in ("connect-services-input", "connect-services-owner"):
            with self.subTest(worker=failing_worker):
                transport, process, _, retired = self.prepare(retire=lambda p: p.terminate())

                def start(worker):
                    if worker.name == failing_worker:
                        raise RuntimeError("start unavailable")
                    return original_start(worker)

                with mock.patch.object(threading.Thread, "start", start):
                    with self.assertRaises(ServiceSetupLaunchError) as raised:
                        self.adopt(transport, ready=False)
                client = raised.exception.owner
                self.assertIs(client._process.stdin, process.stdin)
                self.assertIs(client._process.stdout, process.stdout)
                self.assertFalse(client.close(3).native_resources_closed)
                self.assertEqual(len(retired), 1)
                self.assertTrue(process.stdin.closed and process.stdout.closed)

    def test_interrupt_after_claim_recovers_only_same_adoption(self):
        transport, process, _, retired = self.prepare(retire=lambda p: p.terminate())
        original_claim = ServiceCollectionTransport._claim

        def interrupted(value, token):
            original_claim(value, token)
            raise KeyboardInterrupt()

        with mock.patch.object(ServiceCollectionTransport, "_claim", interrupted):
            with self.assertRaises(ServiceSetupLaunchError) as raised:
                self.adopt(transport, ready=False)
        self.assertIs(raised.exception.owner._process.stdin, process.stdin)
        self.assertFalse(raised.exception.owner.close(3).native_resources_closed)
        self.assertEqual(len(retired), 1)

    def test_retirement_exception_is_value_free_and_latched_unconfirmed(self):
        def fail(_):
            raise RuntimeError("PRIVATE_TOKEN")
        transport, _, _, retired = self.prepare(retire=fail)
        client = self.adopt(transport)
        self.assertFalse(client.close(3).native_resources_closed)
        self.assertFalse(client.close(3).native_resources_closed)
        self.assertEqual(len(retired), 1)

    def test_exit_observer_serialized_cached_and_remaining_timeout_bounded(self):
        entered, release = threading.Event(), threading.Event()
        calls = []
        transport, process, _, _ = self.prepare()
        process.terminate()
        process.wait(3)

        def wait(timeout):
            calls.append(timeout)
            entered.set()
            release.wait(timeout)
            return process.wait(timeout=0)

        transport._owner._wait = wait
        results = []
        worker = threading.Thread(target=lambda: results.append(transport._owner.wait(2)))
        worker.start()
        try:
            self.assertTrue(entered.wait(1))
            start = time.monotonic()
            with self.assertRaises(subprocess.TimeoutExpired):
                transport._owner.wait(.02)
            self.assertLess(time.monotonic() - start, .3)
        finally:
            release.set()
            worker.join(3)
        self.assertFalse(worker.is_alive())
        self.assertEqual(results, [process.returncode])
        self.assertEqual(transport._owner.wait(0), process.returncode)
        self.assertEqual(len(calls), 1)
        self.assertGreaterEqual(calls[0], 0)
        self.assertLessEqual(calls[0], 2)

    def test_exit_observer_failure_can_be_reobserved_but_not_qualified(self):
        transport, process, _, _ = self.prepare()
        process.terminate()
        process.wait(3)
        transport._owner._wait = mock.Mock(side_effect=[RuntimeError("PRIVATE_TOKEN"), process.returncode])
        with self.assertRaises(ServiceSetupError) as raised:
            transport._owner.wait(1)
        self.assertNotIn("PRIVATE_TOKEN", str(raised.exception))
        self.assertEqual(transport._owner.wait(1), process.returncode)
        self.assertFalse(transport._owner.healthy)

    def test_descriptor_refusals_never_take_host_ownership(self):
        transport, process, _, _ = self.prepare()
        arguments = dict(request_retirement=lambda: None, wait_process=lambda timeout: 0,
                         credential_scope=self.binding.credential_scope)
        borrowed = io.FileIO(process.stdin.fileno(), "wb", closefd=False)
        regular = (self.root / "regular").open("wb", buffering=0)
        # A named FIFO has one reported kernel identity on both platforms.
        # Darwin anonymous pipe ends have different inode identities, so their
        # pairing must remain the trusted launcher's responsibility.
        fifo = self.root / "shared-fifo"
        os.mkfifo(fifo, 0o600)
        reader = os.open(fifo, os.O_RDONLY | os.O_NONBLOCK)
        writer = os.open(fifo, os.O_WRONLY | os.O_NONBLOCK)
        os.set_blocking(reader, True)
        os.set_blocking(writer, True)
        same_reader, same_writer = io.FileIO(reader, "rb"), io.FileIO(writer, "wb")
        try:
            for label, input, output in (("borrowed", borrowed, process.stdout),
                                        ("regular", regular, process.stdout),
                                        ("same kernel object", same_writer, same_reader),
                                        ("reversed", process.stdout, process.stdin)):
                with self.subTest(case=label):
                    with self.assertRaises(ValueError):
                        ServiceCollectionTransport(input=input, output=output, **arguments)
                    self.assertFalse(input.closed or output.closed)
            os.set_blocking(process.stdin.fileno(), False)
            with self.assertRaises(ValueError):
                ServiceCollectionTransport(input=process.stdin, output=process.stdout, **arguments)
            with self.assertRaises(ServiceSetupError):
                ServiceCollectionClient.adopt(transport, self.binding)
            self.assertFalse(process.stdin.closed or process.stdout.closed)
        finally:
            os.set_blocking(process.stdin.fileno(), True)
            borrowed.close()
            regular.close()
            same_reader.close()
            same_writer.close()
        self.assertTrue(self.adopt(transport).close(3).native_resources_closed)
