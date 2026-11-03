"""Inert command-output failures retain the original child cleanup owner."""
import ast
import asyncio
import hashlib
import os
from pathlib import Path
from types import MethodType, SimpleNamespace

import pytest

from launchkit import deployment_worker as worker


class Pipe:
    def __init__(self):
        self.chunks = []
        self.eof = asyncio.Event()
        self.started = asyncio.Event()
        self.reader = None
        self.read_error = None
        self.late_error = None

    async def read(self, size):
        assert size == 4096
        self.reader = asyncio.current_task()
        self.started.set()
        if self.read_error is not None:
            raise self.read_error
        if self.chunks:
            return self.chunks.pop(0)
        await self.eof.wait()
        if self.late_error is not None:
            raise self.late_error
        return b""


class Stop(asyncio.Event):
    async def wait(self):
        self.waiter = asyncio.current_task()
        return await super().wait()


@pytest.fixture
def command_owner(tmp_path, monkeypatch):
    pipe = Pipe()
    process = SimpleNamespace(pid=7777777, returncode=None, stdout=pipe)
    owner = worker.Supervisor.__new__(worker.Supervisor)
    owner.directory = owner.destination = tmp_path
    owner.environment = {}
    owner.stop = Stop()
    owner.child = None
    logs, spawns, terminations, exits = [], [], [], []
    cleanup_started, cleanup_release, parent_exited = (asyncio.Event() for _ in range(3))
    cleanup_release.set()
    owner.record_log = lambda phase, line: logs.append((phase, line))

    async def spawn(*arguments, **options):
        assert arguments == ("inert-install",)
        assert options == {"cwd": tmp_path, "env": {}, "stdout": asyncio.subprocess.PIPE,
                           "stderr": asyncio.subprocess.STDOUT}
        spawns.append((arguments, options))
        assert owner.child is None
        owner.child = process
        return process, False

    async def parent_exit(actual):
        assert actual is process
        exits.append(asyncio.current_task())
        await parent_exited.wait()
        return process.returncode

    async def terminate(timeout=None):
        assert owner.child is process
        terminations.append((process, timeout))
        cleanup_started.set()
        await cleanup_release.wait()
        if process.returncode is None:
            process.returncode = 0
        parent_exited.set()
        pipe.eof.set()

    def exited(code=0):
        process.returncode = code
        parent_exited.set()

    def forbidden(*args, **kwargs):
        pytest.fail("Command-output controls cannot spawn or signal OS processes.")

    monkeypatch.setattr(asyncio, "create_subprocess_exec", forbidden)
    monkeypatch.setattr(os, "killpg", forbidden)
    owner.spawn_child, owner.parent_exit, owner.terminate = spawn, parent_exit, terminate
    if os.environ.get("FORGE_TEST_COMMAND_OUTPUT_BASELINE") == "1":
        # Only this fixture binds the exact retained pre-fix command method.
        # It does not substitute production files or set child environment hooks.
        before = Path(__file__).parents[1] / ".artifacts/forge-command-output-recovery-20261010/deployment_worker.py.before"
        payload = before.read_bytes()
        assert hashlib.sha256(payload).hexdigest() == "573f3bf37058567fce06c5e2cfd82f1381533c72e8938b140e1f496608390310"
        cls = next(node for node in ast.parse(payload).body if isinstance(node, ast.ClassDef) and node.name == "Supervisor")
        method = next(node for node in cls.body if isinstance(node, ast.AsyncFunctionDef) and node.name == "command")
        namespace = dict(worker.__dict__)
        exec(compile(ast.Module(body=[method], type_ignores=[]), str(before), "exec"), namespace)
        owner.command = MethodType(namespace["command"], owner)
    return SimpleNamespace(owner=owner, pipe=pipe, process=process, logs=logs, spawns=spawns,
        terminations=terminations, exits=exits, cleanup_started=cleanup_started,
        cleanup_release=cleanup_release, parent_exited=parent_exited, exited=exited)


def assert_joined(trial):
    assert len(trial.spawns) == 1
    assert trial.terminations == [(trial.process, 10)]
    assert trial.owner.child is None
    assert trial.process.returncode is not None
    assert trial.pipe.reader is not None and trial.pipe.reader.done()
    assert len(trial.exits) == 1 and trial.exits[0].done()
    assert trial.owner.stop.waiter.done()


@pytest.mark.parametrize("failure", ["log", "read"])
async def test_command_output_failure_ends_alive_command_and_joins_cleanup(command_owner, failure):
    trial = command_owner
    original = OSError("Synthetic private log write failed." if failure == "log" else "Synthetic pipe read failed.")
    if failure == "log":
        trial.pipe.chunks = [b"Before failure\n"]

        def log_error(*args):
            assert trial.process.returncode is None
            raise original

        trial.owner.record_log = log_error
    else:
        trial.pipe.read_error = original
    with pytest.raises(OSError) as caught:
        await trial.owner.command(["inert-install"], "install", 0.05)
    assert caught.value is original
    assert_joined(trial)


async def test_command_late_drain_failure_after_parent_exit_cannot_return_success(command_owner):
    trial = command_owner
    original = OSError("Synthetic reader failed only during cleanup.")
    trial.pipe.late_error = original
    trial.exited(0)
    with pytest.raises(OSError) as caught:
        await trial.owner.command(["inert-install"], "install", 1)
    assert caught.value is original
    assert_joined(trial)


async def test_normal_early_output_eof_keeps_parent_wait_and_original_deadline(command_owner, monkeypatch):
    trial = command_owner
    trial.pipe.eof.set()
    observed, clock = [], [100.0]
    monkeypatch.setattr(worker.asyncio, "get_running_loop", lambda: SimpleNamespace(time=lambda: clock[0]))

    async def wait(tasks, *, timeout, return_when):
        assert return_when == asyncio.FIRST_COMPLETED
        observed.append((set(tasks), timeout))
        if len(observed) == 1:
            await trial.pipe.started.wait()
            await trial.pipe.reader
            assert trial.pipe.reader in tasks
            clock[0] = 104.0
            return {trial.pipe.reader}, set(tasks) - {trial.pipe.reader}
        assert len(observed) == 2, "Completed EOF must not cause a busy loop."
        assert trial.pipe.reader not in tasks
        assert trial.exits[0] in tasks and not trial.exits[0].done()
        return set(), set(tasks)

    monkeypatch.setattr(worker.asyncio, "wait", wait)
    with pytest.raises(TimeoutError, match="time limit"):
        await trial.owner.command(["inert-install"], "install", 10)
    assert [timeout for _, timeout in observed] == [10.0, 6.0]
    assert_joined(trial)


async def test_command_capture_returns_joined_complete_and_partial_lines(command_owner):
    trial = command_owner
    trial.pipe.chunks = [b"First line\nSecond", b" line\nTrailing fragment"]
    trial.exited(0)
    result = await trial.owner.command(["inert-install"], "observation", 1, capture_output=True)
    assert result == "First line\nSecond line\nTrailing fragment"
    assert trial.logs == [("observation", "First line"), ("observation", "Second line"),
                          ("observation", "Trailing fragment")]
    assert_joined(trial)


@pytest.mark.parametrize("interruption", ["stop", "cancel"])
async def test_interrupted_command_waits_original_cleanup_and_drain_tasks(command_owner, interruption):
    trial = command_owner
    trial.cleanup_release.clear()
    task = asyncio.create_task(trial.owner.command(["inert-install"], "install", 30))
    await asyncio.wait_for(trial.pipe.started.wait(), 30)
    if interruption == "stop":
        trial.owner.stop.set()
    else:
        task.cancel()
    await asyncio.wait_for(trial.cleanup_started.wait(), 30)
    assert not task.done() and trial.owner.child is trial.process
    assert not trial.pipe.reader.done() and not trial.exits[0].done()
    # Repeated caller cancellation cannot release the original retained child.
    task.cancel()
    await asyncio.sleep(0)
    assert not task.done() and trial.owner.child is trial.process
    trial.cleanup_release.set()
    with pytest.raises(asyncio.CancelledError):
        await task
    assert_joined(trial)
