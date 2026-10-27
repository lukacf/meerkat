"""Proposed offline regressions, to install as tests/test_owned_worker.py.

Not executed during patch preparation. Real child processes below are harmless
Python fixtures with no SDK, model, network, application source or credentials.
"""
import asyncio
from contextvars import copy_context
import os
import signal
import sys
from types import SimpleNamespace

import pytest

from launchkit import owned_worker as lifecycle
from launchkit.implementation import read_receipt, update_receipt
from launchkit.templates.storage_setup import write_private

pytestmark = pytest.mark.asyncio


async def test_delayed_cooperative_parent_preserves_child_callback_protocol(tmp_path, monkeypatch):
    child_code = '''import signal, sys, time
signal.signal(signal.SIGTERM, lambda *args: sys.exit(91))
print('ready', flush=True)
assert sys.stdin.readline().strip() == 'shutdown'
time.sleep(.12)
print('ack', flush=True)
'''
    parent_code = '''import pathlib, signal, subprocess, sys
child = subprocess.Popen([sys.executable, '-I', '-c', sys.argv[1]],
                         stdin=subprocess.PIPE, stdout=subprocess.PIPE)
assert child.stdout.readline().strip() == b'ready'
def stop(*args):
    child.stdin.write(b'shutdown\\n'); child.stdin.flush()
    assert child.stdout.readline().strip() == b'ack'
    assert child.wait(timeout=2) == 0
    pathlib.Path(sys.argv[2]).write_text('callback cleanup acknowledged')
    sys.exit(0)
signal.signal(signal.SIGTERM, stop)
print('ready', flush=True)
signal.pause()
'''
    marker = tmp_path / 'ack.txt'
    process, cancelled = await lifecycle.spawn_owned_worker(sys.executable, '-I', '-c', parent_code,
        child_code, str(marker), stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.PIPE)
    assert not cancelled
    group_signals, original = [], lifecycle.os.killpg
    def killpg(pid, selected):
        if selected:
            group_signals.append((pid, selected))
        return original(pid, selected)
    monkeypatch.setattr(lifecycle.os, 'killpg', killpg)
    monkeypatch.setattr(lifecycle, 'WORKER_STOP_TIMEOUT', 1)
    monkeypatch.setattr(lifecycle, 'PROCESS_FORCE_REAP_TIMEOUT', .5)
    try:
        assert await asyncio.wait_for(process.stdout.readline(), 2) == b'ready\n'
        await lifecycle.stop_owned_worker(process)
        assert marker.read_text() == 'callback cleanup acknowledged'
        assert process.returncode == 0 and group_signals == []
        with pytest.raises(ProcessLookupError):
            original(process.pid, 0)
    finally:
        if process.returncode is None:
            original(process.pid, signal.SIGKILL)
            await process.wait()


async def test_reaped_parent_requires_remaining_owned_group_to_disappear(monkeypatch):
    child = SimpleNamespace(pid=7777777, returncode=0, stdin=None)
    async def wait():
        return child.returncode
    child.wait = wait
    child.terminate = lambda: pytest.fail('The parent was already reaped.')
    present, selected = True, []
    def killpg(pid, sig):
        nonlocal present
        assert pid == child.pid
        if not present:
            raise ProcessLookupError
        if sig:
            selected.append(sig)
        if sig == signal.SIGKILL:
            present = False
    monkeypatch.setattr(lifecycle.os, 'killpg', killpg)
    monkeypatch.setattr(lifecycle, 'PROCESS_FORCE_REAP_TIMEOUT', .1)
    await lifecycle.stop_owned_worker(child)
    assert selected == [signal.SIGTERM, signal.SIGKILL] and not present


@pytest.mark.parametrize('inaccessible', [False, True])
async def test_unknown_or_surviving_group_is_not_confirmed(monkeypatch, inaccessible):
    child = SimpleNamespace(pid=7777777, returncode=0, stdin=None)
    async def wait():
        return 0
    child.wait = wait
    def killpg(pid, sig):
        assert pid == child.pid
        if inaccessible:
            raise PermissionError('Synthetic inspection failure')
    monkeypatch.setattr(lifecycle.os, 'killpg', killpg)
    monkeypatch.setattr(lifecycle, 'PROCESS_FORCE_REAP_TIMEOUT', .02)
    with pytest.raises(PermissionError if inaccessible else TimeoutError):
        await lifecycle.stop_owned_worker(child)


async def test_transient_group_permission_race_requires_observed_absence(monkeypatch):
    child = SimpleNamespace(pid=7777777, returncode=0)
    inspections = []
    def killpg(pid, sig):
        inspections.append(sig)
        if sig:
            raise PermissionError('Exited group race')
        raise ProcessLookupError
    monkeypatch.setattr(lifecycle.os, 'killpg', killpg)
    await lifecycle._signal_group(child, signal.SIGTERM)
    assert inspections == [signal.SIGTERM, 0]


async def test_repeated_spawn_cancellation_retains_actual_child(monkeypatch):
    admitted, release = asyncio.Event(), asyncio.Event()
    actual = SimpleNamespace(pid=7777777, returncode=None)
    async def spawn(*args, **kwargs):
        assert kwargs['start_new_session'] is True
        admitted.set()
        await release.wait()
        return actual
    monkeypatch.setattr(lifecycle.asyncio, 'create_subprocess_exec', spawn)
    task = asyncio.create_task(lifecycle.spawn_owned_worker('synthetic-child'))
    await admitted.wait()
    task.cancel()
    await asyncio.sleep(0)
    task.cancel()
    await asyncio.sleep(0)
    release.set()
    assert await task == (actual, True)


@pytest.mark.parametrize('cancellations', [1, 3])
@pytest.mark.parametrize('failure_type', [OSError, RuntimeError])
async def test_late_admission_failure_retains_first_cancellation_and_exact_error(monkeypatch, cancellations, failure_type):
    admitted, release = asyncio.Event(), asyncio.Event()
    failure = failure_type('Inert admission failed before returning a child handle.')
    interruptions, calls = [], []
    original_shield = asyncio.shield

    async def observed_shield(awaitable):
        try:
            return await original_shield(awaitable)
        except asyncio.CancelledError as error:
            interruptions.append(error)
            raise

    async def spawn(*args, **options):
        assert options['start_new_session'] is True
        calls.append(True)
        admitted.set()
        await release.wait()
        raise failure

    monkeypatch.setattr(lifecycle.asyncio, 'create_subprocess_exec', spawn)
    monkeypatch.setattr(lifecycle.asyncio, 'shield', observed_shield)
    monkeypatch.setattr(lifecycle.os, 'killpg', lambda *_: pytest.fail('Inert admission controls cannot signal a group.'))
    task = asyncio.create_task(lifecycle.spawn_owned_worker('inert-late-error', preserve_admission_errors=True))
    try:
        await admitted.wait()
        for number in range(cancellations):
            task.cancel(f'caller-cancellation-{number}')
            await asyncio.sleep(0)
        assert not task.done() and len(interruptions) == cancellations
        release.set()
        with pytest.raises(BaseExceptionGroup) as caught:
            await task
        assert caught.value.exceptions == (interruptions[0], failure)
        assert interruptions[0].args == ('caller-cancellation-0',)
        assert len(calls) == 1
    finally:
        release.set()
        if not task.done():
            task.cancel()
        await asyncio.gather(task, return_exceptions=True)


@pytest.mark.parametrize('failure_type', [OSError, RuntimeError])
async def test_uncancelled_admission_failure_keeps_original_error(monkeypatch, failure_type):
    failure = failure_type('Inert failed admission.')
    calls = []

    async def spawn(*args, **options):
        assert options['start_new_session'] is True
        calls.append(True)
        raise failure

    monkeypatch.setattr(lifecycle.asyncio, 'create_subprocess_exec', spawn)
    monkeypatch.setattr(lifecycle.os, 'killpg', lambda *_: pytest.fail('Inert admission controls cannot signal a group.'))
    with pytest.raises(failure_type) as caught:
        await lifecycle.spawn_owned_worker('inert-error', preserve_admission_errors=True)
    assert caught.value is failure and len(calls) == 1


@pytest.mark.parametrize('preserve', [None, False])
async def test_default_admission_failure_keeps_existing_caller_exception_contract(monkeypatch, preserve):
    admitted, release = asyncio.Event(), asyncio.Event()
    failure = OSError('Inert late admission failure for an unchanged caller.')
    calls = []

    async def spawn(*args, **options):
        assert options['start_new_session'] is True
        assert 'preserve_admission_errors' not in options
        calls.append(True)
        admitted.set()
        await release.wait()
        raise failure

    monkeypatch.setattr(lifecycle.asyncio, 'create_subprocess_exec', spawn)
    monkeypatch.setattr(lifecycle.os, 'killpg', lambda *_: pytest.fail('Inert admission controls cannot signal a group.'))
    options = {} if preserve is None else {'preserve_admission_errors': preserve}
    task = asyncio.create_task(lifecycle.spawn_owned_worker('inert-compatible-error', **options))
    try:
        await admitted.wait()
        task.cancel('first-cancellation')
        await asyncio.sleep(0)
        task.cancel('second-cancellation')
        await asyncio.sleep(0)
        assert not task.done()
        release.set()
        with pytest.raises(OSError) as caught:
            await task
        assert caught.value is failure and len(calls) == 1
    finally:
        release.set()
        if not task.done():
            task.cancel()
        await asyncio.gather(task, return_exceptions=True)


async def test_final_cleanup_outlives_job_deadline_without_unbounded_work():
    stop = asyncio.Event()
    async with asyncio.timeout(.01) as deadline:
        token = lifecycle.owned_job_deadline.set(deadline)
        try:
            assert lifecycle.begin_worker_cleanup(stop) is False
            await asyncio.sleep(.03)
        finally:
            lifecycle.owned_job_deadline.reset(token)
    assert stop.is_set() and not deadline.expired()


async def test_job_deadline_still_cancels_work_then_allows_cleanup():
    stop, cleaned = asyncio.Event(), asyncio.Event()
    with pytest.raises(TimeoutError):
        async with asyncio.timeout(.01) as deadline:
            token = lifecycle.owned_job_deadline.set(deadline)
            try:
                try:
                    await asyncio.Event().wait()
                finally:
                    lifecycle.begin_worker_cleanup(stop)
                    await asyncio.sleep(.03)
                    cleaned.set()
            finally:
                lifecycle.owned_job_deadline.reset(token)
    assert cleaned.is_set() and stop.is_set()


@pytest.mark.parametrize('trigger', ['signal', 'eof', 'timeout'])
async def test_registered_stop_context_disarms_timer_and_does_not_recancel_cleanup(tmp_path, monkeypatch, trigger):
    from launchkit import rehearsal_worker as worker
    write_private(tmp_path / 'receipt.json', {'status': 'queued', 'native_console_url': None})
    entered, cleanup_started, release, cleaned = (asyncio.Event() for _ in range(4))
    registered = {}
    loop = asyncio.get_running_loop()
    def register(sig, callback):
        registered[sig] = (callback, copy_context())
    monkeypatch.setattr(loop, 'add_signal_handler', register)
    monkeypatch.setattr(worker, 'REHEARSAL_SECONDS', .03)
    read_fd, write_fd = os.pipe()
    monkeypatch.setattr(worker.sys, 'stdin', SimpleNamespace(fileno=lambda: read_fd))
    async def execute(directory, stop):
        entered.set()
        try:
            await asyncio.Event().wait()
        finally:
            # Models an admitted SDK bootstrap or storage drain before the
            # worker's final cleanup hook. The stop callback must disarm first.
            cleanup_started.set()
            await release.wait()
            cleaned.set()
            lifecycle.begin_worker_cleanup(stop)
    monkeypatch.setattr(worker, 'execute', execute)
    task = asyncio.create_task(worker.run(tmp_path, owned_stdin=True))
    try:
        await entered.wait()
        callback, context = registered[signal.SIGTERM]
        assert context.get(lifecycle.owned_job_deadline) is None
        if trigger == 'signal':
            loop.call_soon(callback, context=context)
        elif trigger == 'eof':
            os.close(write_fd)
            write_fd = None
        await cleanup_started.wait()
        for sig in (signal.SIGTERM, signal.SIGINT):
            following, captured = registered[sig]
            loop.call_soon(following, context=captured)
        await asyncio.sleep(.06)
        assert not task.done() and not cleaned.is_set()
        release.set()
        await asyncio.wait_for(task, 1)
        assert cleaned.is_set() and read_receipt(tmp_path)['status'] == ('failed' if trigger == 'timeout' else 'interrupted')
    finally:
        release.set()
        if not task.done():
            task.cancel()
        await asyncio.gather(task, return_exceptions=True)
        if write_fd is not None:
            os.close(write_fd)
        os.close(read_fd)
