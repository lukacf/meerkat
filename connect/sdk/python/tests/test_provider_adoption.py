"""Regressions for the original-child adoption implementation.

Every accepted transport uses original unbuffered FileIO pipe endpoints. The
Python peer is fixture-owned, not a host application, a native provider, or a model. The two
private one-call I/O seams leave the actual original FileIO endpoints intact.
"""
import asyncio
import dataclasses
import io
import json
import math
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from unittest import mock

import meerkat_toolkit.provider_setup as provider
from meerkat_toolkit.provider_setup import (
    ProviderSetupBinding, ProviderSetupClient, ProviderSetupError,
    ProviderSetupTransport,
)


def encoded(value):
    return json.dumps(value, separators=(',', ':'), allow_nan=False).encode() + b'\n'


def initial():
    return {'kind': 'initialized', 'id': 'initialize',
            'protocol': 'connect-provider-setup/1', 'interaction_id': 'original'}


def selected():
    return {'kind': 'selected', 'selection': {'interaction_id': 'original',
        'model': 'fixture-model', 'provider': 'fixture',
        'auth_binding': {'realm': 'owned', 'binding': 'configured', 'origin': 'configured'},
        'credential': 'ready', 'live_entitlement': 'unobserved'}}


def binding(root):
    return ProviderSetupBinding('app', 'instance', 'viewer', 'login',
        int(time.time() * 1000) + 120000, 'owned', str(root), str(root / 'state'),
        str(root / 'config'), {'kind': 'file', 'root': str(root / 'credentials')})


def original_pipe():
    """Allocate original fixture endpoints, without pretending Darwin is the host.

    Linux/supporting hosts exercise pipe2(O_CLOEXEC). Darwin exercises the SDK's
    FileIO/protocol/lifetime contract using os.pipe, never private host allocation.
    """
    if callable(getattr(os, 'pipe2', None)):
        descriptors = os.pipe2(os.O_CLOEXEC)
    elif sys.platform == 'darwin':
        descriptors = os.pipe()
    else:
        raise RuntimeError('fixture host has no reviewed original-pipe allocation')
    try:
        for descriptor in descriptors:
            if os.get_inheritable(descriptor) or not os.get_blocking(descriptor):
                raise AssertionError('original fixture pipe must be blocking and noninheritable')
        return descriptors
    except BaseException as primary:
        for descriptor in descriptors:
            try:
                os.close(descriptor)
            except BaseException as error:
                OriginalChild.unsettled.append((descriptors, primary, error))
                primary.add_note('fixture pipe cleanup has uncertain close: ' + type(error).__name__)
        raise


# Parent passes separate original child-end integer FDs to Popen. Only this
# fixture peer uses those child endpoints, after Popen maps them to fd 0 and 1.
PEER = r'''
import json, os, sys, time
from pathlib import Path
root, mode, exit_code = Path(sys.argv[1]), sys.argv[2], int(sys.argv[3])
def emit(value):
    raw = json.dumps(value, separators=(',', ':')).encode() + b'\n'
    while raw:
        count = os.write(1, raw)
        if count <= 0: raise RuntimeError('fixture output made no progress')
        raw = raw[count:]
def finish():
    deadline = time.monotonic() + 10
    while not (root / 'release').exists():
        if time.monotonic() >= deadline: sys.exit(91)
        time.sleep(.005)
    sys.exit(exit_code)
(root / 'ready').touch()
if mode == 'hold-unread': finish()
first = sys.stdin.buffer.readline(1024 * 1024 + 2)
(root / 'request.raw').write_bytes(first)
request = json.loads(first)
assert request['method'] == 'initialize'
if mode == 'malformed': os.write(1, b'{malformed}\n')
elif mode == 'oversize':
    raw = b'x' * (1024 * 1024 + 2)
    while raw: raw = raw[os.write(1, raw):]
elif mode != 'eof':
    emit({'kind':'initialized','id':'initialize','protocol':'connect-provider-setup/1','interaction_id':'original'})
    if mode == 'overflow':
        for _ in range(65): emit({'kind':'notice','interaction_id':'original'})
    elif mode == 'prompt':
        emit({'kind':'prompt','interaction_id':'original','prompt_id':'p','prompt':{'kind':'choices','choices':[]}})
        second = sys.stdin.buffer.readline(1024 * 1024 + 2)
        (root / 'second.raw').write_bytes(second)
        request = json.loads(second)
        if request['method'] == 'cancel':
            assert request == {'method':'cancel','id':'close'}
            emit({'kind':'prompt_closed','prompt_id':'p'})
            emit({'kind':'cancelled','interaction_id':'original','joined':True,'join_scope':'provider_interaction'})
        else:
            assert request == {'method':'answer','id':'answer-1','prompt_id':'p','answer':{'kind':'choice','choice_id':'offered'}}
            emit({'kind':'answered','id':'answer-1','prompt_id':'p'})
            emit({'kind':'selected','selection':{'interaction_id':'original','model':'fixture-model','provider':'fixture','auth_binding':{'realm':'owned','binding':'configured','origin':'configured'},'credential':'ready','live_entitlement':'unobserved'}})
    else:
        emit({'kind':'selected','selection':{'interaction_id':'original','model':'fixture-model','provider':'fixture','auth_binding':{'realm':'owned','binding':'configured','origin':'configured'},'credential':'ready','live_entitlement':'unobserved'}})
os.close(1)
finish()
'''


class OriginalChild:
    """Test root only; no claim to implement a host's full retirement owner.

    Notification fixes one independent 30-second root deadline. Tests may cause
    earlier fixture exit explicitly; they never wait for that deadline to expire.
    The peer's 10-second test watchdog is not the host retirement deadline.
    """
    unsettled = []  # Keep a failed SDK settlement reachable; never pretend joined.

    def __init__(self, mode='selected', exit_code=0):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.child = None
        self.input = self.output = None
        self.client = None
        self.transport = None
        self.gates = []
        self.retirement_requested = threading.Event()
        self.wait_called = threading.Event()
        self.pending_observed = threading.Event()
        self.wait_threads, self.wait_bounds = [], []
        self.retirement_notifications = 0
        self.retirement_started_at = self.retirement_deadline = None
        self.root_retirement_seconds = 30.0
        self.full_root_cleanup = False
        self.false_once = False
        self.lock = threading.Lock()
        fds = set()
        allocation_error = None
        close_errors = []
        try:
            child_in, parent_in = original_pipe()
            fds.update((child_in, parent_in))
            parent_out, child_out = original_pipe()
            fds.update((parent_out, child_out))
            self.input = os.fdopen(parent_in, 'wb', buffering=0)
            fds.remove(parent_in)
            self.output = os.fdopen(parent_out, 'rb', buffering=0)
            fds.remove(parent_out)
            assert type(self.input) is io.FileIO and type(self.output) is io.FileIO
            assert self.input.writable() and self.output.readable()
            assert not os.get_inheritable(parent_in) and not os.get_inheritable(parent_out)
            self.original_fds = (parent_in, parent_out)
            self.child = subprocess.Popen(
                [sys.executable, '-u', '-c', PEER, str(self.root), mode, str(exit_code)],
                stdin=child_in, stdout=child_out, stderr=subprocess.DEVNULL,
                close_fds=True,
            )
        except BaseException as error:
            allocation_error = error
        finally:
            for fd in fds:
                try:
                    os.close(fd)
                except OSError as error:
                    close_errors.append((fd, error))
        if allocation_error is not None or close_errors:
            # Before adoption both endpoints and the exact child remain root-owned.
            # Attempt every cleanup and preserve the primary failure, without
            # retrying an ambiguously closed descriptor that could be reused.
            primary = allocation_error or close_errors[0][1]
            cleanup_errors = []
            if self.child is not None:
                try:
                    if self.child.poll() is None:
                        self.child.kill()
                    self.child.wait(timeout=2)
                except BaseException as error:
                    cleanup_errors.append(error)
            for endpoint in (self.input, self.output):
                if endpoint is not None:
                    try:
                        endpoint.close()
                    except BaseException as error:
                        cleanup_errors.append(error)
            if close_errors or cleanup_errors:
                self.unsettled.append((self, close_errors, cleanup_errors))
                primary.add_note('fixture allocation cleanup has retained uncertain failures')
            else:
                self.temporary.cleanup()
            raise primary
        self.original_pid = self.child.pid
        try:
            self.wait_file('ready')
        except BaseException as primary:
            try:
                self.cleanup()
            except BaseException as cleanup_error:
                self.unsettled.append((self, cleanup_error))
                primary.add_note('fixture readiness cleanup has retained uncertain failure')
            raise

    def wait_file(self, name, timeout=2):
        deadline = time.monotonic() + timeout
        while not (self.root / name).exists():
            if self.child.poll() is not None or time.monotonic() >= deadline:
                raise AssertionError('fixture did not reach ' + name)
            time.sleep(.005)

    def request_retirement(self):
        with self.lock:
            self.retirement_notifications += 1
            if self.retirement_started_at is None:
                self.retirement_started_at = time.monotonic()
                self.retirement_deadline = self.retirement_started_at + self.root_retirement_seconds
        self.retirement_requested.set()  # Notification only; no close or SDK join.

    def wait_process(self, seconds):
        if type(seconds) is not float or not math.isfinite(seconds) or not 0 <= seconds <= 60:
            raise AssertionError('SDK supplied invalid process observation bound')
        self.wait_threads.append(threading.get_ident())
        self.wait_bounds.append(seconds)
        self.wait_called.set()
        if self.false_once:
            self.false_once = False
            return False  # Deliberately invalid observation, never actual exit.
        try:
            return self.child.wait(timeout=seconds)
        except subprocess.TimeoutExpired:
            self.pending_observed.set()
            raise TimeoutError('same original child pending') from None

    def carrier(self, request_retirement=None):
        self.transport = ProviderSetupTransport(
            input=self.input, output=self.output,
            request_retirement=request_retirement or self.request_retirement,
            wait_process=self.wait_process,
        )
        return self.transport

    def release_original(self):
        (self.root / 'release').touch(exist_ok=True)

    def exit_original(self):
        self.release_original()
        try:
            return self.child.wait(timeout=2)
        except subprocess.TimeoutExpired:
            # A malformed-output peer may be blocked writing. Root retires its
            # exact original handle; preserve the real nonzero/signal result.
            self.child.kill()
            return self.child.wait(timeout=2)

    def cleanup(self):
        # Always unblock test-owned dispatch gates before any SDK join.
        for gate in self.gates:
            gate.set()
        self.release_original()
        try:
            self.child.wait(timeout=2)
        except subprocess.TimeoutExpired:
            self.child.kill()  # Fixture root owns this exact original handle.
            self.child.wait(timeout=2)
        if self.transport is not None:
            self.client = self.transport.client
        if self.client is not None:
            try:
                self.client.close(2)
            except ProviderSetupError:
                # The adopted close contract raises retained failure only after
                # actual SDK/process settlement. Timeout is different below.
                pass
            except BaseException:
                self.unsettled.append(self)
                raise
            if not (self.input.closed and self.output.closed):
                self.unsettled.append(self)
                raise AssertionError('SDK returned before adopted endpoint settlement')
        else:
            # No transfer occurred. Root attempts both endpoint closes.
            errors = []
            for endpoint in (self.input, self.output):
                try:
                    endpoint.close()
                except BaseException as error:
                    errors.append(error)
            if errors:
                self.unsettled.append(self)
                raise AssertionError('fixture root endpoint cleanup failed') from errors[0]
        self.temporary.cleanup()


class ProviderAdoptionFileIOTests(unittest.TestCase):
    def owner(self, mode='selected', exit_code=0):
        owner = OriginalChild(mode, exit_code)
        self.addCleanup(owner.cleanup)
        owner.carrier()
        return owner

    def adopt(self, owner, bound=None):
        with mock.patch.object(provider.subprocess, 'Popen', side_effect=AssertionError('SDK must not spawn')):
            client = ProviderSetupClient.adopt(bound or binding(owner.root), transport=owner.transport)
        self.assertIs(client, owner.transport.client)
        self.assertTrue(owner.transport.transferred)
        self.assertIs(owner.input, owner.transport.input)
        self.assertIs(owner.output, owner.transport.output)
        # A fast terminal reader may already have settled an endpoint. Do not
        # mistake a retained closed FileIO object for a replacement descriptor.
        for endpoint, original_fd in zip((owner.input, owner.output), owner.original_fds):
            if not endpoint.closed:
                self.assertEqual(endpoint.fileno(), original_fd)
        self.assertEqual(owner.child.pid, owner.original_pid)
        return client

    def events(self, client):
        self.assertEqual(client.next_event(2), initial())
        self.assertEqual(client.next_event(2), selected())

    def bounded_pending(self, owner, client):
        # Retain this observation thread and join it even on the failure path.
        done = threading.Event()
        result = []
        def observe():
            try:
                result.append(client.close(.03))
            except BaseException as error:
                result.append(error)
            finally:
                done.set()
        worker = threading.Thread(target=observe, daemon=True)
        worker.start()
        try:
            self.assertTrue(done.wait(1), 'close blocked outside its observation bound')
            self.assertEqual(len(result), 1)
            self.assertIsInstance(result[0], TimeoutError)
            self.assertIs(owner.transport.client, client)
            self.assertFalse(owner.full_root_cleanup)
        finally:
            if not done.is_set():
                for gate in owner.gates:
                    gate.set()
                owner.release_original()
            worker.join(2)
            self.assertFalse(worker.is_alive(), 'same close observer is still unjoined')

    def test_invalid_binding_and_inactive_worker_failure_do_not_transfer(self):
        owner = self.owner('hold-unread')
        bound = binding(owner.root)
        with mock.patch.object(provider, '_adopted_write_once', side_effect=AssertionError('pre-transfer write')) as write:
            for invalid in (dataclasses.replace(bound, expires_at_ms=0),
                            dataclasses.replace(bound, context_root='relative'),
                            dataclasses.replace(bound, context_root='/' + 'x' * (1024 * 1024))):
                with self.assertRaises(ValueError):
                    ProviderSetupClient.adopt(invalid, transport=owner.transport)
            with mock.patch.object(provider.threading.Thread, 'start', side_effect=RuntimeError('start refused')):
                with self.assertRaises(RuntimeError):
                    ProviderSetupClient.adopt(bound, transport=owner.transport)
            write.assert_not_called()
        self.assertIsNone(owner.transport.client)
        self.assertFalse(owner.transport.transferred)
        self.assertFalse(owner.input.closed or owner.output.closed)
        self.assertFalse(owner.retirement_requested.is_set())
        self.assertFalse(owner.wait_called.is_set())

    def test_wrong_stream_type_direction_and_async_callbacks_are_refused(self):
        owner = self.owner('hold-unread')
        async def asynchronous(*args):
            return None
        for change in ({'input': io.StringIO()}, {'input': io.BytesIO()},
                       {'output': owner.input}, {'input': owner.output},
                       {'request_retirement': asynchronous}, {'wait_process': asynchronous}):
            values = dict(input=owner.input, output=owner.output,
                request_retirement=owner.request_retirement, wait_process=owner.wait_process)
            values.update(change)
            with self.assertRaises(ValueError):
                carrier = ProviderSetupTransport(**values)
                ProviderSetupClient.adopt(binding(owner.root), transport=carrier)
        self.assertFalse(owner.input.closed or owner.output.closed)
        self.assertFalse(owner.retirement_requested.is_set())

    def test_exact_original_fileio_child_one_initialize_and_duplicate_refusal(self):
        owner = self.owner()
        bound = binding(owner.root)
        client = self.adopt(owner, bound)
        self.events(client)
        owner.exit_original()
        self.assertEqual(client.close(2), selected())
        self.assertEqual((owner.root / 'request.raw').read_bytes(),
            encoded({'method': 'initialize', 'id': 'initialize', 'binding': bound.data()}))
        with self.assertRaises(ValueError):
            ProviderSetupClient.adopt(bound, transport=owner.transport)
        self.assertEqual(owner.child.returncode, 0)
        self.assertTrue(owner.input.closed and owner.output.closed)
        self.assertFalse(owner.full_root_cleanup)

    def test_builtin_timeout_pending_retains_original_and_independent_deadline(self):
        owner = self.owner()
        bound = binding(owner.root)
        original_expiry = bound.expires_at_ms
        client = self.adopt(owner, bound)
        self.events(client)
        self.bounded_pending(owner, client)
        self.assertTrue(owner.pending_observed.wait(1))
        self.assertTrue(owner.retirement_requested.wait(1))
        deadline = owner.retirement_deadline
        started = owner.retirement_started_at
        self.assertAlmostEqual(deadline - started, 30.0)
        self.bounded_pending(owner, client)
        self.assertEqual(owner.retirement_deadline, deadline)
        self.assertEqual(owner.retirement_started_at, started)
        self.assertEqual(json.loads((owner.root / 'request.raw').read_bytes())['binding']['expires_at_ms'],
            bound.expires_at_ms)
        self.assertIsNone(owner.child.poll())
        self.assertIs(owner.transport.client, client)
        self.assertNotIn(threading.get_ident(), owner.wait_threads)
        owner.exit_original()
        self.assertEqual(client.close(2), selected())
        self.assertEqual(bound.expires_at_ms, original_expiry)
        self.assertFalse(owner.full_root_cleanup)

    def test_nonzero_original_exit_is_not_selection_success(self):
        owner = self.owner(exit_code=7)
        client = self.adopt(owner)
        self.events(client)
        self.bounded_pending(owner, client)
        self.assertEqual(owner.exit_original(), 7)
        with self.assertRaises(ProviderSetupError):
            client.close(2)

    def test_boolean_process_observation_is_not_actual_exit(self):
        # The invalid process observation can precede initialization; do not
        # require a successful protocol exchange after that retained failure.
        owner = self.owner('hold-unread')
        owner.false_once = True
        client = self.adopt(owner)
        self.assertTrue(owner.wait_called.wait(1))
        self.assertTrue(owner.retirement_requested.wait(1))
        self.bounded_pending(owner, client)
        self.assertIsNone(owner.child.poll())
        owner.exit_original()
        with self.assertRaises(ProviderSetupError):
            client.close(2)

    def test_first_write_failure_retains_original_until_actual_exit(self):
        owner = self.owner('hold-unread')
        # Private one-call seam; accepted endpoints remain actual FileIO pipes.
        with mock.patch.object(provider, '_adopted_write_once', side_effect=BrokenPipeError('injected')):
            client = self.adopt(owner)
            self.assertTrue(owner.retirement_requested.wait(1))
            self.bounded_pending(owner, client)
            owner.exit_original()
            with self.assertRaises(ProviderSetupError):
                client.close(2)
        self.assertFalse((owner.root / 'request.raw').exists())

    def test_eof_malformed_and_oversize_keep_failure_and_owner(self):
        for mode in ('eof', 'malformed', 'oversize'):
            with self.subTest(mode=mode):
                owner = self.owner(mode)
                client = self.adopt(owner)
                self.assertTrue(owner.retirement_requested.wait(2))
                with self.assertRaises(ProviderSetupError):
                    client.next_event(1)
                self.bounded_pending(owner, client)
                owner.exit_original()
                with self.assertRaises(ProviderSetupError):
                    client.close(2)

    def test_observer_queue_bound_does_not_replace_original(self):
        owner = self.owner('overflow')
        client = self.adopt(owner)
        self.assertTrue(owner.retirement_requested.wait(2))
        observed = [client.next_event(1) for _ in range(64)]
        self.assertEqual(observed[0], initial())
        with self.assertRaises(ProviderSetupError):
            client.next_event(1)
        self.bounded_pending(owner, client)
        owner.exit_original()
        with self.assertRaises(ProviderSetupError):
            client.close(2)

    def test_short_writes_complete_exact_one_frame_through_original_fileio(self):
        owner = self.owner()
        bound = binding(owner.root)
        calls = []
        def short_write(stream, data):
            self.assertIs(stream, owner.input)
            self.assertIs(type(stream), io.FileIO)
            part = bytes(data[:7])
            count = stream.write(part)  # Actual original pipe; no wrapper endpoint.
            calls.append((threading.get_ident(), part[:count]))
            return count
        with mock.patch.object(provider, '_adopted_write_once', side_effect=short_write):
            client = self.adopt(owner, bound)
            self.events(client)
            owner.exit_original()
            self.assertEqual(client.close(2), selected())
        expected = encoded({'method': 'initialize', 'id': 'initialize', 'binding': bound.data()})
        self.assertGreater(len(calls), 1)
        self.assertEqual(b''.join(part for _, part in calls), expected)
        self.assertEqual((owner.root / 'request.raw').read_bytes(), expected)
        self.assertNotIn(threading.get_ident(), [thread for thread, _ in calls])

    def test_zero_progress_after_partial_frame_is_failure_not_replay(self):
        owner = self.owner('hold-unread')
        calls = []
        def no_progress(stream, data):
            self.assertIs(stream, owner.input)
            calls.append(bytes(data))
            if len(calls) == 1:
                return stream.write(bytes(data[:7]))
            return 0
        with mock.patch.object(provider, '_adopted_write_once', side_effect=no_progress):
            client = self.adopt(owner)
            self.assertTrue(owner.retirement_requested.wait(1))
            self.bounded_pending(owner, client)
            self.assertEqual(len(calls), 2)
            self.assertEqual(calls[1], calls[0][7:])
            owner.exit_original()
            with self.assertRaises(ProviderSetupError):
                client.close(2)
        self.assertEqual(len(calls), 2, 'partial frame was retried or replayed')

    def test_retirement_can_reap_child_while_owned_writer_remains_pending(self):
        owner = self.owner('hold-unread')
        entered, release = threading.Event(), threading.Event()
        owner.gates.append(release)
        calls = []
        def held_write(stream, data):
            self.assertIs(stream, owner.input)
            calls.append(threading.get_ident())
            entered.set()
            if not release.wait(5):
                raise AssertionError('fixture writer gate not released')
            return stream.write(data)
        with mock.patch.object(provider, '_adopted_write_once', side_effect=held_write):
            try:
                client = self.adopt(owner)
                self.assertTrue(entered.wait(1))
                self.bounded_pending(owner, client)
                self.assertTrue(owner.retirement_requested.wait(1))
                deadline = owner.retirement_deadline
                owner.exit_original()  # Independent fixture root, no SDK join first.
                self.assertEqual(owner.child.returncode, 0)
                self.bounded_pending(owner, client)
                self.assertEqual(owner.retirement_deadline, deadline)
                self.assertFalse(owner.full_root_cleanup)
            finally:
                release.set()
            with self.assertRaises(ProviderSetupError):
                client.close(2)  # Original read endpoint is gone; real write fails.
        self.assertEqual(len(calls), 1)
        self.assertNotIn(threading.get_ident(), calls)

    def test_actual_child_exit_before_reader_completion_is_not_joined(self):
        owner = self.owner()
        eof_read, release = threading.Event(), threading.Event()
        owner.gates.append(release)
        def held_readline(stream, limit):
            self.assertIs(stream, owner.output)
            self.assertIs(type(stream), io.FileIO)
            raw = stream.readline(limit)  # Actual original pipe and actual EOF.
            if raw == b'':
                eof_read.set()
                if not release.wait(5):
                    raise AssertionError('fixture reader gate not released')
            return raw
        with mock.patch.object(provider, '_adopted_readline', side_effect=held_readline):
            try:
                client = self.adopt(owner)
                self.events(client)
                self.assertTrue(eof_read.wait(1))
                owner.exit_original()
                self.assertEqual(owner.child.returncode, 0)
                self.bounded_pending(owner, client)
                self.assertFalse(owner.output.closed)
            finally:
                release.set()
            self.assertEqual(client.close(2), selected())
        self.assertTrue(owner.input.closed and owner.output.closed)

    def test_interruption_after_transfer_keeps_original_client_recoverable(self):
        owner = self.owner()
        setter = object.__setattr__
        interrupted = []
        def publish_then_interrupt(carrier, name, value):
            setter(carrier, name, value)
            if carrier is owner.transport and name == '_client' and value is not None:
                interrupted.append(value)
                raise KeyboardInterrupt('controlled after-publication interruption')
        with mock.patch.object(ProviderSetupTransport, '__setattr__', publish_then_interrupt):
            with self.assertRaises(KeyboardInterrupt):
                ProviderSetupClient.adopt(binding(owner.root), transport=owner.transport)
        self.assertEqual(len(interrupted), 1)
        client = owner.transport.client
        self.assertIs(client, interrupted[0])
        self.assertTrue(owner.transport.transferred)
        self.events(client)
        owner.exit_original()
        self.assertEqual(client.close(2), selected())
        self.assertEqual(owner.child.pid, owner.original_pid)

    def test_interruption_before_transfer_cannot_start_original_io(self):
        owner = self.owner('hold-unread')
        setter = object.__setattr__
        def refuse_publication(carrier, name, value):
            if carrier is owner.transport and name == '_client' and value is not None:
                raise KeyboardInterrupt('controlled before-publication interruption')
            setter(carrier, name, value)
        with mock.patch.object(provider, '_adopted_write_once', side_effect=AssertionError('untransferred write')) as write:
            with mock.patch.object(ProviderSetupTransport, '__setattr__', refuse_publication):
                with self.assertRaises(KeyboardInterrupt):
                    ProviderSetupClient.adopt(binding(owner.root), transport=owner.transport)
            write.assert_not_called()
        self.assertFalse(owner.transport.transferred)
        self.assertIsNone(owner.transport.client)
        self.assertFalse(owner.wait_called.is_set())
        self.assertFalse(owner.retirement_requested.is_set())
        self.assertFalse(owner.input.closed or owner.output.closed)

    def test_answer_and_cancel_keep_exact_native_frames_and_terminal_scope(self):
        for action in ('answer', 'cancel'):
            with self.subTest(action=action):
                owner = self.owner('prompt')
                client = self.adopt(owner)
                self.assertEqual(client.next_event(2), initial())
                self.assertEqual(client.next_event(2)['kind'], 'prompt')
                with self.assertRaises(ProviderSetupError):
                    client.answer('foreign', {'kind': 'confirm'})
                if action == 'answer':
                    client.answer('p', {'kind': 'choice', 'choice_id': 'offered'})
                    self.assertEqual(client.next_event(2),
                        {'kind': 'answered', 'id': 'answer-1', 'prompt_id': 'p'})
                    terminal = client.next_event(2)
                    self.assertEqual(terminal, selected())
                    expected = {'method': 'answer', 'id': 'answer-1', 'prompt_id': 'p',
                                'answer': {'kind': 'choice', 'choice_id': 'offered'}}
                else:
                    self.bounded_pending(owner, client)
                    self.assertEqual(client.next_event(2), {'kind': 'prompt_closed', 'prompt_id': 'p'})
                    terminal = client.next_event(2)
                    self.assertEqual(terminal, {'kind': 'cancelled', 'interaction_id': 'original',
                                               'joined': True, 'join_scope': 'provider_interaction'})
                    expected = {'method': 'cancel', 'id': 'close'}
                self.assertEqual((owner.root / 'second.raw').read_bytes(), encoded(expected))
                owner.exit_original()
                self.assertEqual(client.close(2), terminal)
                with self.assertRaises(ProviderSetupError):
                    client.answer('p', {'kind': 'confirm'})

    def test_retirement_callback_failure_does_not_fabricate_root_settlement(self):
        owner = OriginalChild()
        self.addCleanup(owner.cleanup)
        def refused():
            owner.request_retirement()
            raise RuntimeError('controlled notification failure')
        owner.carrier(request_retirement=refused)
        client = self.adopt(owner)
        self.events(client)
        self.assertTrue(owner.retirement_requested.wait(1))
        self.bounded_pending(owner, client)
        self.assertIsNone(owner.child.poll())
        owner.exit_original()
        with self.assertRaises(ProviderSetupError):
            client.close(2)
        self.assertFalse(owner.full_root_cleanup)

    def test_invalid_observation_bound_does_not_retire_original(self):
        owner = self.owner('prompt')
        client = self.adopt(owner)
        self.assertEqual(client.next_event(2), initial())
        self.assertEqual(client.next_event(2)['kind'], 'prompt')
        for timeout in (0, -1, True, float('nan'), float('inf'), 2_147_484):
            with self.assertRaises(ValueError):
                client.close(timeout)
            with self.assertRaises(ValueError):
                client.next_event(timeout)
            self.assertFalse(client._cancelled)
            self.assertFalse(owner.retirement_requested.is_set())
            self.assertIsNone(owner.child.poll())
        self.bounded_pending(owner, client)
        owner.exit_original()
        self.assertEqual(client.close(2)['kind'], 'cancelled')


class ProviderAdoptionFileIOAsyncTests(unittest.IsolatedAsyncioTestCase):
    async def test_cancelled_observer_keeps_same_sdk_task_and_root_child(self):
        owner = OriginalChild()
        loop = asyncio.get_running_loop()
        root_thread = threading.get_ident()
        notified = asyncio.Event()
        def request():
            owner.request_retirement()
            loop.call_soon_threadsafe(notified.set)
        owner.carrier(request_retirement=request)
        close_task = None
        try:
            with mock.patch.object(provider.subprocess, 'Popen', side_effect=AssertionError('SDK must not spawn')):
                client = ProviderSetupClient.adopt(binding(owner.root), transport=owner.transport)
            self.assertEqual(await asyncio.to_thread(client.next_event, 2), initial())
            self.assertEqual(await asyncio.to_thread(client.next_event, 2), selected())
            close_task = asyncio.create_task(asyncio.to_thread(client.close, 2))
            async def observe():
                return await asyncio.shield(close_task)
            observer = asyncio.create_task(observe())
            await asyncio.wait_for(notified.wait(), 1)
            deadline = owner.retirement_deadline
            observer.cancel()
            with self.assertRaises(asyncio.CancelledError):
                await observer
            self.assertFalse(close_task.done())
            self.assertIs(owner.transport.client, client)
            self.assertIsNone(owner.child.poll())
            owner.release_original()
            self.assertEqual(await asyncio.wait_for(asyncio.shield(close_task), 2), selected())
            self.assertEqual(owner.retirement_deadline, deadline)
            self.assertNotIn(root_thread, owner.wait_threads)
            self.assertFalse(owner.full_root_cleanup)
        finally:
            owner.release_original()
            try:
                if close_task is not None:
                    await asyncio.wait_for(asyncio.shield(close_task), 3)
            finally:
                await asyncio.to_thread(owner.cleanup)
