"""Bounded SDK observation controls with synthetic peers, not host acceptance."""
from pathlib import Path
import os
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from unittest import mock
from meerkat_toolkit import service_setup
from meerkat_toolkit.service_setup import ServiceSetupBinding, ServiceSetupError
from meerkat_toolkit.service_collection import ServiceCollectionClient, ServiceCollectionLauncher, ServiceCollectionTransport

PEER = r'''
import json, sys, time
from pathlib import Path
root = Path(sys.argv[1])
import os, threading
watchdog = threading.Timer(15, lambda: os._exit(74))
watchdog.daemon = True
watchdog.start()
def emit(value):
    print(json.dumps(value), flush=True)
def result(request, value):
    emit({'jsonrpc':'2.0', 'id':request['id'], 'result':value})
state = {'available':True, 'can_send':True, 'can_stop':False,
    'outcome_uncertain':False, 'active_send':None, 'last_completion':None,
    'private_notice_pending':False, 'can_collect':False, 'commissioning':False,
    'active_collection':None, 'collection_sources':['fixture-source'],
    'reusable_source_coverage':'unqualified_credential_continuity'}
capabilities = {'prompt_kinds':['approval','provider_target','provider_action',
    'provider_api_key','provider_browser','provider_device','service_secret'],
    'service_preparation':True, 'service_collections':{
        'selection':'trusted_process_sources',
        'requires':'successful_finalization_and_governed_reopen',
        'result':'historical_required_operation_observations',
        'reusable_source_coverage':'unqualified_credential_continuity',
        'slack_inbound':'unsupported'}}
initial = json.loads(sys.stdin.readline())
result(initial, {'protocol_version':7, 'scope':'fixture-scope',
    'session_id':'fixture-session', 'capabilities':capabilities, 'state':state})
for line in sys.stdin:
    request = json.loads(line)
    if request['method'] == 'connect/send':
        for index in range(512):
            emit({'jsonrpc':'2.0', 'method':'connect/event', 'params':{
                'target':{'scope':'fixture-scope'}, 'kind':'conversation',
                'text':'fixture token', 'index':index}})

        state['last_completion'] = {'send_request_id':request['id'],
                                   'completion':'completed_without_result'}
        result(request, {'scope':'fixture-scope', 'send_request_id':request['id'],
            'completion':'completed_without_result', 'state':state})
        (root/'emitted').touch()
    elif request['method'] == 'connect/close':
        result(request, {'target':request['params']['target'], 'resources_closed':True})
        break
'''

class ObservationBackpressureTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="connect-observation-")
        self.root = Path(self.temp.name)
        self.clients = []
        self.processes = []
        self.scope = {"kind": "file", "root": str(self.root / "credentials")}
        self.binding = ServiceSetupBinding("fixture", "fixture-instance", "https://forge.example",
            "fixture-user", "fixture-login", int(time.time()*1000)+60000, "fixture-realm",
            str(self.root/"context"), str(self.root/"state"), str(self.root/"config"), "e"*64, self.scope)

    def tearDown(self):
        for client in self.clients:
            # Dispose only this test's synthetic process if an assertion failed.
            process = next((p for c, p in self.processes if c is client), client._process)
            if process.poll() is None:
                process.terminate()
            process.wait(3)
            # Drain retained observations so cleanup does not silently erase them.
            try:
                while True:
                    client.next_event(.05)
            except (EOFError, ServiceSetupError, TimeoutError):
                pass
            client.close(3)
            self.assertFalse(client._reader.is_alive())
            self.assertFalse(client._writer.is_alive())
            self.assertNotEqual(process.returncode, 74, "fixture watchdog fired")
        self.temp.cleanup()

    def client(self, *, adopted=False, peer=PEER):
        args = (sys.executable, "-I", "-u", "-c", peer, str(self.root))
        if adopted:
            process = subprocess.Popen(args, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                stderr=subprocess.DEVNULL, bufsize=0, close_fds=True)
            transport = ServiceCollectionTransport(input=process.stdin, output=process.stdout,
                credential_scope=self.scope, request_retirement=lambda: None,
                wait_process=lambda timeout: process.wait(timeout=timeout))
            client = ServiceCollectionClient.adopt(transport, self.binding)
            self.processes.append((client, process))
        else:
            client = ServiceCollectionClient(ServiceCollectionLauncher(args, self.scope,
                {"PATH": os.defpath, "LANG": "C"}), self.binding)
        self.clients.append(client)
        initial = client.next_event(3)
        self.assertEqual(initial.payload["method"], "connect/initialize")
        return client

    def saturated(self, client, *, byte_bound=False):
        deadline = time.monotonic()+3
        while time.monotonic() < deadline:
            with client._condition:
                count, size = len(client._events), client._queued_bytes
                if client._failure or count == 128 or byte_bound and count > 0 and size > service_setup._MAX_FRAME-250:
                    return count, size
            time.sleep(.001)
        self.fail("observation queue did not reach its bound")

    def drain_send(self, client, request, *, slow=False):
        indices = []
        maximum = [0, 0]
        while True:
            with client._condition:
                maximum[0] = max(maximum[0], len(client._events))
                maximum[1] = max(maximum[1], client._queued_bytes)
                self.assertLessEqual(len(client._events), 128)
                self.assertLessEqual(client._queued_bytes, service_setup._MAX_FRAME)
            event = client.next_event(3)
            if event.kind == "conversation":
                indices.append(event.payload["index"])
                if slow:
                    time.sleep(.0005)
            elif event.request_id == request:
                self.assertEqual(event.payload["method"], "connect/send")
                break
        self.assertEqual(indices, list(range(512)))
        self.assertFalse(client._failure)
        return maximum

    def test_unpaced_burst_with_prestarted_dedicated_consumer(self):
        client = self.client()
        ready = threading.Event()
        observations, errors = [], []
        def consume():
            ready.set()
            try:
                for _ in range(513):
                    observations.append(client.next_event(3))
            except BaseException as error:
                errors.append(error)
        reader = threading.Thread(target=consume)
        reader.start()
        self.assertTrue(ready.wait(1))
        request = client.submit("fixture request")
        reader.join(5)
        self.assertFalse(reader.is_alive())
        self.assertFalse(errors)
        self.assertEqual([e.payload["index"] for e in observations if e.kind == "conversation"], list(range(512)))
        self.assertEqual(observations[-1].request_id, request)
        self.assertTrue(client.close(3).native_resources_closed)

    def test_slow_consumer_retains_count_bounded_ordered_burst(self):
        client = self.client()
        request = client.submit("fixture request")
        self.assertEqual(self.saturated(client)[0], 128)
        time.sleep(.03)
        self.assertFalse(client._failure)
        self.assertEqual(self.drain_send(client, request, slow=True)[0], 128)
        self.assertTrue(client.close(3).native_resources_closed)

    def test_byte_budget_blocks_and_resumes_below_count_limit(self):
        with mock.patch.object(service_setup, "_MAX_FRAME", 4096):
            client = self.client()
            request = client.submit("fixture request")
            count, size = self.saturated(client, byte_bound=True)
            self.assertLess(count, 128)
            self.assertLessEqual(size, 4096)
            time.sleep(.03)
            self.assertFalse(client._failure)
            self.drain_send(client, request, slow=True)
            self.assertTrue(client.close(3).native_resources_closed)

    def test_full_queue_close_times_out_then_same_owner_drains_and_joins(self):
        client = self.client(adopted=True)
        request = client.submit("fixture request")
        self.saturated(client)
        with self.assertRaises(TimeoutError):
            client.close(.03)
        self.assertFalse(client._failure)
        self.assertTrue(client._reader.is_alive())
        self.drain_send(client, request)
        closed = client.close(3)
        self.assertTrue(closed.native_resources_closed)
        close_event = client.next_event(1)
        self.assertEqual(close_event.payload["method"], "connect/close")
        self.assertFalse(client._reader.is_alive())
        self.assertFalse(client._writer.is_alive())

    def test_original_owner_exit_with_full_queue_retains_observed_prefix(self):
        client = self.client(adopted=True)
        client.submit("fixture request")
        self.saturated(client)
        process = self.processes[-1][1]
        process.terminate()
        process.wait(3)
        indices = []
        try:
            while True:
                event = client.next_event(3)
                if event.kind == "conversation":
                    indices.append(event.payload["index"])
        except (EOFError, ServiceSetupError):
            pass
        self.assertGreaterEqual(len(indices), 129)
        self.assertEqual(indices, list(range(len(indices))))
        closed = client.close(3)
        self.assertFalse(closed.native_resources_closed)
        self.assertFalse(client._reader.is_alive())
        self.assertFalse(client._writer.is_alive())

    def test_control_writer_progresses_while_observation_queue_is_full(self):
        client = self.client()
        request = client.submit("fixture request")
        self.saturated(client)
        started = time.monotonic()
        cancel = client.cancel(request)
        self.assertLess(time.monotonic()-started, .5)
        deadline = time.monotonic()+1
        with client._condition:
            while client._outbound_bytes and time.monotonic() < deadline:
                client._condition.wait(deadline-time.monotonic())
            self.assertEqual(client._outbound_bytes, 0)
            self.assertEqual(len(client._events), 128)
            self.assertIn(cancel, client._requests)
        # Submission and the SDK writer progressed; native completion is still
        # observed only by draining the original stream.
        self.drain_send(client, request)
        self.assertTrue(client.close(3).native_resources_closed)

    def test_writer_epipe_releases_saturated_reader_without_discarding_queued_events(self):
        peer = PEER.replace("if request['method'] == 'connect/send':",
            "if request['method'] == 'connect/send':\n        os.close(0)")
        client = self.client(peer=peer)
        request = client.submit("fixture request")
        self.assertEqual(self.saturated(client)[0], 128)
        self.assertFalse(client._failure)
        try:
            client.cancel(request)
            deadline = time.monotonic()+3
            with client._condition:
                while not (client._reader_done and client._writer_done) and time.monotonic() < deadline:
                    client._condition.wait(deadline-time.monotonic())
                self.assertTrue(client._reader_done, "writer failure must release reader capacity wait")
                self.assertTrue(client._writer_done)
                self.assertEqual(len(client._events), 128)
            if hasattr(client, "first_failure"):
                self.assertEqual(client.first_failure.code, "native_input_failed")
            events = [client.next_event(1) for _ in range(128)]
            self.assertEqual([e.payload["index"] for e in events], list(range(128)))
            with self.assertRaises(ServiceSetupError):
                client.next_event(1)
            closed = client.close(3)
            self.assertTrue(closed.process_joined)
            self.assertFalse(closed.native_resources_closed)
            self.assertFalse(client._reader.is_alive())
            self.assertFalse(client._writer.is_alive())
        finally:
            # A broken implementation still has only this fixture's original
            # reader. Drain it for cleanup without masking the assertion above.
            deadline = time.monotonic()+3
            while not client._reader_done and time.monotonic() < deadline:
                try:
                    client.next_event(.05)
                except (EOFError, ServiceSetupError, TimeoutError):
                    time.sleep(.005)

    def test_oversized_single_frame_still_fails(self):
        with mock.patch.object(service_setup, "_MAX_FRAME", 4096):
            peer = PEER.replace("'fixture token'", "('x'*5000)")
            client = self.client(peer=peer)
            client.submit("fixture request")
            with self.assertRaises(ServiceSetupError):
                client.next_event(3)
            self.assertTrue(client._failure)
            self.assertEqual(len(client._events), 0)

if __name__ == "__main__":
    unittest.main()
