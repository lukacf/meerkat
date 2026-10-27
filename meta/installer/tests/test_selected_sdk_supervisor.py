"""Real supervised fixture children, without pip, SDK execution or admission."""
import asyncio
import hashlib
import importlib.util
import json
import marshal
import os
from pathlib import Path
import sys
import threading
import time
from time import sleep

import pytest

from launchkit import deployment_worker as worker
from launchkit.templates import install_candidate as installed
from test_sdk_wheel_pair_install import pair_files, put_installed
from test_sdk_dependency_install_sequence import sequence_files

from sdk_source_import_guard import no_selected_sdk_import


def command_owner(root):
    owner = worker.Supervisor.__new__(worker.Supervisor)
    owner.directory = owner.destination = root
    owner.stop = asyncio.Event()
    owner.child = None
    owner._child_admission = None
    owner.environment = {"PATH": os.defpath, "HOME": str(root), "FIXTURE": "owned"}
    owner.logs = []
    owner.record_log = lambda phase, text: owner.logs.append((phase, text))
    return owner


async def wait_until(predicate):
    async with asyncio.timeout(30):
        while not predicate():
            await asyncio.sleep(0.005)


def assert_reaped(process):
    assert process.returncode is not None
    with pytest.raises(ProcessLookupError):
        os.kill(process.pid, 0)


async def test_raw_stdin_drains_both_outputs_while_feeding_exact_input(tmp_path):
    owner = command_owner(tmp_path)
    payload = b"private-fixture-input\x00\xff" * 100000
    size = 600 * 1024
    script = ("import hashlib,json,os,sys\n"
        f"os.write(1,b'a'*{size});os.write(2,b'b'*{size})\n"
        "data=sys.stdin.buffer.read()\n"
        "print(json.dumps({'size':len(data),'sha256':hashlib.sha256(data).hexdigest()}))\n")
    outcome = await owner.command([sys.executable, "-I", "-c", script], "stdin fixture", 3,
        raw_outcome=True, input=payload)
    assert outcome["returncode"] == 0 and outcome["stderr"] == b"b" * size
    assert outcome["stdout"].startswith(b"a" * size)
    assert json.loads(outcome["stdout"][size:]) == {
        "size": len(payload), "sha256": hashlib.sha256(payload).hexdigest()}
    assert owner.child is None
    assert not any("private-fixture-input" in text for _, text in owner.logs)


async def test_blocked_stdin_obeys_deadline_and_reaps_exact_owned_child(tmp_path):
    owner = command_owner(tmp_path)
    children = []
    original = owner.spawn_child

    async def spawn(*args, **options):
        value = await original(*args, **options)
        children.append(value[0])
        return value

    owner.spawn_child = spawn
    with pytest.raises(TimeoutError, match="time limit"):
        await owner.command([sys.executable, "-I", "-c", "import time;time.sleep(30)"],
            "blocked input fixture", 0.15, raw_outcome=True, input=b"x" * (2 * 1024 * 1024))
    assert len(children) == 1 and owner.child is None
    assert_reaped(children[0])


@pytest.mark.parametrize("options", [
    {"input": "decoded text", "raw_outcome": True},
    {"input": b"fixture", "raw_outcome": False},
    {"input": b"fixture", "raw_outcome": True, "capture_output": True},
    {"pip": "yes", "raw_outcome": True},
])
async def test_invalid_command_options_refuse_before_child_admission(tmp_path, options):
    owner = command_owner(tmp_path)
    entered = []

    async def forbidden(*args, **kwargs):
        entered.append((args, kwargs))
        raise AssertionError("Invalid options must never create a child.")

    owner.spawn_child = forbidden
    with pytest.raises(ValueError):
        await owner.command(["not-executed"], "invalid fixture", 1, **options)
    assert entered == [] and owner.child is None


async def test_oversized_stdin_refuses_before_child_admission(tmp_path):
    owner = command_owner(tmp_path)
    entered = []

    async def forbidden(*args, **kwargs):
        entered.append((args, kwargs))
        raise AssertionError("Oversized input must never create a child.")

    owner.spawn_child = forbidden
    with pytest.raises(ValueError):
        await owner.command(["not-executed"], "oversized fixture", 1, raw_outcome=True,
            input=b"x" * (worker.MAX_PRIVATE_COMMAND_INPUT_BYTES + 1))
    assert entered == [] and owner.child is None


@pytest.fixture
def supervised_files(sequence_files, monkeypatch):
    files = sequence_files
    owner = command_owner(files.root)
    children, commands, checks = [], [], []
    files.mode = "success"
    files.cache = False
    files.current = True
    files.fence_threads = []
    original_spawn = owner.spawn_child

    def no_subprocess(*args, **kwargs):
        pytest.fail("The selected installation must use the original supervisor for every interpreter call.")

    monkeypatch.setattr(installed.subprocess, "run", no_subprocess)

    def current():
        files.fence_threads.append(threading.get_ident())
        if not files.current:
            raise installed.CandidateArtifactError("Selected fixture owner was replaced.")

    async def spawn(*argv, **options):
        commands.append((argv, options))
        assert argv[0] == str(files.python)
        if "pip" in argv:
            assert options["env"] == {**owner.environment, "PIP_CONFIG_FILE": os.devnull}
            assert options["env"] is not owner.environment
            requirements = Path(argv[-1])
            assert requirements.is_file() and requirements.parent.parent == files.root / ".venv"
            if requirements.parent.name.startswith(".forge-selected-sdk-pair-"):
                put_installed(files)
                if files.cache:
                    source = files.site / "meerkat_toolkit/host_control.py"
                    cache = Path(importlib.util.cache_from_source(str(source)))
                    cache.parent.mkdir(exist_ok=True)
                    cache.write_bytes(importlib.util.MAGIC_NUMBER + b"\x00" * 12
                        + marshal.dumps(compile(source.read_bytes(), str(source), "exec", dont_inherit=True)))
                    cache.chmod(0o644)
            else:
                for item in files.dependencies:
                    for name, content in item.values.items():
                        target = files.site / name
                        target.parent.mkdir(parents=True, exist_ok=True)
                        target.write_bytes(content)
                        target.chmod(0o644)
            code = ("import time;time.sleep(30)" if files.mode in {"blocked", "timeout"}
                else "import sys;sys.exit(23)" if files.mode == "pip-failure" else "print('fixture pip exit')")
            args = (sys.executable, "-I", "-c", code)
        else:
            assert argv[1:4] == ("-I", "-S", "-c")
            assert options["env"] == owner.environment
            checks.append(argv)
            args = (sys.executable, *argv[1:])
        value = await original_spawn(*args, **options)
        children.append(value[0])
        return value

    owner.spawn_child = spawn
    files.owner, files.children, files.commands, files.probes = owner, children, commands, checks
    files.require_current = current
    return files


def selected_install(files, *, dependencies=True, timeout=3):
    options = dict(meta=files.meta[1], toolkit=files.toolkit[1], site_packages=files.site,
        sdk_python=files.python, require_current=files.require_current, timeout=timeout)
    if dependencies:
        options.update(dependency_requirements=files.dependency_requirements,
            dependency_wheels=files.dependency_wheels)
    return files.owner.install_selected_sdk_pair(files.meta_path, files.toolkit_path, **options)


async def test_selected_pair_dependencies_and_nested_bytecode_use_original_supervisor(supervised_files):
    files = supervised_files
    files.cache = True
    loop_thread = threading.get_ident()
    result = await selected_install(files)
    assert result is files.verified
    assert len(files.commands) == len(files.children) == 4
    assert len(files.probes) == 2
    assert files.fence_threads and set(files.fence_threads) != {loop_thread}
    assert files.owner.child is None and not files.owner.stop.is_set()
    assert all(not name.startswith(".forge-selected-") for name in os.listdir(files.root / ".venv"))
    assert (files.root / "retained.txt").read_bytes() == b"application state stays owned\n"
    assert not (files.root / ".venv/forge-sdk.receipt.json").exists()
    assert (files.site / "fixture_sequence/__init__.py").read_bytes() == files.dependencies[0].values["fixture_sequence/__init__.py"]
    for process in files.children:
        assert_reaped(process)
    assert "fixture_sequence" not in sys.modules


@pytest.mark.parametrize("interruption", ["stop", "cancel"])
async def test_interruption_joins_original_child_before_staging_is_removed(supervised_files, interruption):
    files = supervised_files
    files.mode = "blocked"
    cleanup_entered, cleanup_release = asyncio.Event(), asyncio.Event()
    original = files.owner.terminate

    async def terminate(timeout=None):
        cleanup_entered.set()
        await cleanup_release.wait()
        await original(timeout=timeout)

    files.owner.terminate = terminate
    task = asyncio.create_task(selected_install(files))
    try:
        await wait_until(lambda: bool(files.children))
        if interruption == "stop":
            files.owner.stop.set()
        else:
            task.cancel()
        await asyncio.wait_for(cleanup_entered.wait(), 30)
        assert not task.done() and files.owner.child is files.children[0]
        staged = Path(files.commands[0][0][-1])
        assert staged.is_file()
        task.cancel()
        await asyncio.sleep(0.01)
        assert not task.done() and staged.is_file()
    finally:
        cleanup_release.set()
        try:
            await asyncio.wait_for(task, 4)
        except asyncio.CancelledError:
            pass
    assert task.cancelled() and files.owner.stop.is_set()
    assert files.owner.child is None and len(files.children) == 1
    assert not staged.exists()
    assert_reaped(files.children[0])


@pytest.mark.parametrize("mode", ["timeout", "timeout-after-slow-staging", "pip-failure"])
async def test_install_failure_joins_child_and_refuses_later_commands(supervised_files, mode):
    files = supervised_files
    files.mode = "timeout" if mode.startswith("timeout") else mode
    if mode == "timeout-after-slow-staging":
        # A loaded host stages the wheels slowly before the first pip child.
        fence, staged = files.require_current, []
        def slow_fence():
            if not staged:
                staged.append(True)
                time.sleep(0.2)
            return fence()
        files.require_current = slow_fence
    expected = TimeoutError if mode.startswith("timeout") else installed.CandidateArtifactError
    # The deadline covers staging too, so it must outlast slow staging and
    # expire while the timeout leg's pip child sleeps (30 s), not before it.
    with pytest.raises(expected):
        await selected_install(files, timeout=3)
    assert len(files.children) == 1 and files.owner.child is None
    assert not list((files.root / ".venv").glob(".forge-selected-*"))
    assert_reaped(files.children[0])


async def test_a_deadline_that_expires_during_staging_starts_no_child(supervised_files):
    files = supervised_files
    files.mode = "timeout"
    # Staging outlasts the whole deadline, so its fence refuses before any pip child.
    fence, staged = files.require_current, []
    def slow_fence():
        if not staged:
            staged.append(True)
            time.sleep(0.3)
        return fence()
    files.require_current = slow_fence
    with pytest.raises(TimeoutError):
        await selected_install(files, timeout=0.1)
    assert files.children == [] and files.owner.child is None
    assert not list((files.root / ".venv").glob(".forge-selected-*"))


async def test_unconfirmed_child_cleanup_retains_staging_until_original_owner_recovers(supervised_files):
    files = supervised_files
    files.mode = "blocked"
    original = files.owner.terminate
    original_exit = files.owner.parent_exit
    cleanup_entered, cleanup_release = asyncio.Event(), asyncio.Event()
    retained_error = OSError("Fixture child cleanup could not confirm exit.")

    async def failed_cleanup(timeout=None):
        cleanup_entered.set()
        await cleanup_release.wait()
        raise retained_error

    async def observe_exit(process):
        await cleanup_release.wait()
        return process.returncode

    files.owner.terminate = failed_cleanup
    files.owner.parent_exit = observe_exit
    task = asyncio.create_task(selected_install(files))
    staged = None
    try:
        await wait_until(lambda: bool(files.children))
        files.owner.stop.set()
        await asyncio.wait_for(cleanup_entered.wait(), 30)
        staged = Path(files.commands[0][0][-1])
        # Settle inherited pipes so the injected failed reap remains the only
        # cleanup failure. The actual original fixture child still runs.
        for pipe in (files.children[0].stdout, files.children[0].stderr):
            pipe.feed_eof()
        cleanup_release.set()
        with pytest.raises(BaseExceptionGroup) as caught:
            await asyncio.wait_for(task, 4)

        def contains(error):
            return error is retained_error or isinstance(error, BaseExceptionGroup) and any(
                contains(item) for item in error.exceptions)

        assert contains(caught.value)
        assert files.owner.child is files.children[0] and files.children[0].returncode is None
        assert staged.is_file() and len(files.children) == 1
        with pytest.raises(RuntimeError):
            await selected_install(files)
    finally:
        cleanup_release.set()
        files.owner.terminate = original
        files.owner.parent_exit = original_exit
        if files.owner.child is not None:
            await files.owner.cleanup_child(files.owner.child, timeout=0.05)
        if not task.done():
            task.cancel()
            await asyncio.gather(task, return_exceptions=True)
        if staged is not None and staged.exists():
            # Original-owner recovery above confirmed the fixture child exit.
            # Remove only this disposable attempt's retained staging.
            import shutil
            shutil.rmtree(staged.parent)
    assert files.owner.child is None
    assert_reaped(files.children[0])


async def test_replaced_owner_after_pip_refuses_nested_readback_and_dependencies(supervised_files):
    files = supervised_files
    files.cache = True
    original = files.owner.command

    async def command(*args, **kwargs):
        value = await original(*args, **kwargs)
        files.current = False
        return value

    files.owner.command = command
    with pytest.raises(installed.CandidateArtifactError, match="replaced"):
        await selected_install(files)
    assert len(files.commands) == len(files.children) == 1 and files.probes == []
    assert files.owner.child is None
    assert not list((files.root / ".venv").glob(".forge-selected-*"))
    assert_reaped(files.children[0])


@pytest.mark.parametrize("staging", ["prompt", "slow"])
async def test_pair_deadline_is_not_reset_for_dependency_phase(supervised_files, staging):
    files = supervised_files
    if staging == "slow":
        # A loaded host stages the wheels slowly before the first pip child.
        fence, staged = files.require_current, []
        def slow_fence():
            if not staged:
                staged.append(True)
                sleep(0.8)
            return fence()
        files.require_current = slow_fence
    loop = asyncio.get_running_loop()
    clock = [loop.time()]
    original_time = loop.time
    original = files.owner.command
    deadlines = []
    # The deadline covers staging too, so it must outlast slow staging. The
    # fixture clock then moves past it once the first child has run.
    pair_deadline = 3

    async def command(args, phase, timeout, **options):
        deadlines.append(timeout)
        result = await original(args, phase, timeout, **options)
        if len(deadlines) == 1:
            clock[0] = original_time() + pair_deadline + 1
        return result

    files.owner.command = command
    # The real first child runs with the true monotonic clock. Only the next
    # transaction fence observes the advanced fixture clock after its cleanup.
    def time():
        return max(original_time(), clock[0])

    loop.time = time
    try:
        with pytest.raises(TimeoutError):
            await selected_install(files, timeout=pair_deadline)
    finally:
        loop.time = original_time
    assert len(deadlines) == len(files.children) == 1
    assert files.owner.child is None
    assert not list((files.root / ".venv").glob(".forge-selected-*"))
    assert_reaped(files.children[0])


async def test_stopped_owner_refuses_before_staging_or_child(supervised_files):
    files = supervised_files
    files.owner.stop.set()
    with pytest.raises(asyncio.CancelledError):
        await selected_install(files)
    assert files.commands == files.children == files.fence_threads == []
    assert not list((files.root / ".venv").glob(".forge-selected-*"))


async def test_overlapping_pair_install_cannot_stage_or_enter_second_child(supervised_files):
    files = supervised_files
    files.mode = "blocked"
    task = asyncio.create_task(selected_install(files))
    try:
        await wait_until(lambda: bool(files.children))
        before = len(files.fence_threads)
        staged = list((files.root / ".venv").glob(".forge-selected-*"))
        with pytest.raises(RuntimeError):
            await selected_install(files)
        assert len(files.fence_threads) == before
        assert list((files.root / ".venv").glob(".forge-selected-*")) == staged
        assert len(files.commands) == 1 and not task.done()
    finally:
        files.owner.stop.set()
        try:
            await asyncio.wait_for(task, 4)
        except asyncio.CancelledError:
            pass
    assert task.cancelled() and files.owner.child is None
    assert_reaped(files.children[0])


@pytest.mark.parametrize("timeout", [True, 0, -1, float("inf"), float("nan")])
async def test_invalid_install_deadline_refuses_before_staging(supervised_files, timeout):
    files = supervised_files
    with pytest.raises(ValueError):
        await selected_install(files, timeout=timeout)
    assert files.commands == files.children == files.fence_threads == []
    assert not list((files.root / ".venv").glob(".forge-selected-*"))
