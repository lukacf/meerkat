"""A worker bounds its own stop once its owner, the Forge server, is gone.

Each row runs a real worker process in its own session, as Forge spawns it,
declared an owned worker the way its entry point declares it, closes its stdin
the way Forge's stop or exit does, and keeps the worker's cleanup from ever
finishing.
"""
import ast
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time

import pytest

INSTALLER = Path(__file__).resolve().parents[1]

DRIVER = r'''
import asyncio, json, os, signal, subprocess, sys
from pathlib import Path
from types import SimpleNamespace
directory, kind, bound = Path(sys.argv[1]), sys.argv[2], float(sys.argv[3])
# A helper in the worker's own group, like the SDK gateway a worker runs. It
# outlives SIGTERM, so only the bound's SIGKILL ends it.
helper = subprocess.Popen([sys.executable, "-c",
    "import signal, time; signal.signal(signal.SIGTERM, signal.SIG_IGN); time.sleep(60)"])
from launchkit import owned_worker
owned_worker.WORKER_STOP_TIMEOUT = bound
getattr(owned_worker, "serve_as_owned_worker", lambda: None)()
if kind.endswith("teardown-hung"):
    # The stop itself ends, but interpreter exit then waits for a thread that
    # never ends, after asyncio.run restored SIGTERM to its default.
    import threading, time
    threading.Thread(target=time.sleep, args=(60,)).start()

async def cleanup(*_args, **_kwargs):
    # Ordinary work until the stop; then a native cleanup that either ends at
    # once or holds through every cancellation and never ends.
    try:
        await asyncio.Event().wait()
    finally:
        while kind.endswith("hung") and not kind.endswith("teardown-hung"):
            try:
                await asyncio.Event().wait()
            except asyncio.CancelledError:
                continue

(directory / "helper.json").write_text(json.dumps({"helper": helper.pid}))
if kind.startswith("implementation"):
    from launchkit import implementation_worker as worker
    worker.execute = cleanup
    asyncio.run(worker.run(directory, owned_stdin=True))
elif kind.startswith("rehearsal"):
    from launchkit import rehearsal_worker as worker
    worker.execute = cleanup
    asyncio.run(worker.run(directory, owned_stdin=True))
else:
    from launchkit import deployment, deployment_worker
    def stop_budget(root):
        assert Path(root) == directory
        return bound
    deployment.worker_stop_timeout = stop_budget

    async def main():
        supervisor = SimpleNamespace(stop=asyncio.Event(), directory=directory)
        loop = asyncio.get_running_loop()
        for sig in (signal.SIGINT, signal.SIGTERM):
            loop.add_signal_handler(sig, supervisor.stop.set)
        reading = asyncio.create_task(deployment_worker.owner_input(supervisor))
        await supervisor.stop.wait()
        await cleanup()
    asyncio.run(main())
'''


def absent(pid):
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return True
    return False


def run_worker(tmp_path, kind, bound, *, own_session=True):
    os.chmod(tmp_path, 0o700)
    (tmp_path / "receipt.json").write_text(json.dumps({"status": "queued", "native_console_url": None}))
    os.chmod(tmp_path / "receipt.json", 0o600)
    return subprocess.Popen([sys.executable, "-c", DRIVER, str(tmp_path), kind, str(bound)], cwd=INSTALLER,
        env={**os.environ, "PYTHONPATH": str(INSTALLER)}, stdin=subprocess.PIPE, start_new_session=own_session)


def started(tmp_path, process):
    deadline = time.monotonic() + 20
    while not (tmp_path / "helper.json").exists():
        assert process.poll() is None and time.monotonic() < deadline
        time.sleep(.02)
    time.sleep(.3)  # The worker reads its owner's stdin before ordinary work waits.
    return json.loads((tmp_path / "helper.json").read_text())["helper"]


@pytest.mark.parametrize("kind", ["deployment-hung", "deployment-stop-then-eof-hung", "implementation-hung",
                                  "implementation-teardown-hung", "rehearsal-hung"])
def test_worker_whose_stop_hangs_after_owner_loss_ends_its_own_group_at_the_bound(tmp_path, kind):
    bound = 2.0
    process = run_worker(tmp_path, kind, bound)
    helper = None
    try:
        helper = started(tmp_path, process)
        if kind == "deployment-stop-then-eof-hung":
            # Stop first; the owner is lost while that stop still runs.
            process.stdin.write(b'{"action":"stop"}\n')
            process.stdin.flush()
            time.sleep(.5)
        process.stdin.close()
        lost = time.monotonic()
        time.sleep(bound - .6)
        # Before its bound the worker keeps its own stop: nothing signalled it.
        assert process.poll() is None and not absent(helper)
        process.wait(timeout=bound + 10)
        elapsed = time.monotonic() - lost
        assert bound - .6 <= elapsed < bound + 5
        # SIGTERM to its own group first when its stop handler absorbs it,
        # and SIGKILL to that group always.
        assert process.returncode == -signal.SIGKILL
        deadline = time.monotonic() + 5
        while not absent(helper):
            assert time.monotonic() < deadline, "the worker's own group outlived its bound"
            time.sleep(.02)
    finally:
        if process.poll() is None:
            os.killpg(process.pid, signal.SIGKILL)
        process.wait()
        if helper is not None and not absent(helper):
            os.kill(helper, signal.SIGKILL)


def test_worker_whose_stop_settles_after_owner_loss_exits_normally_before_the_bound(tmp_path):
    bound = 5.0
    process = run_worker(tmp_path, "implementation-settles", bound)
    helper = None
    try:
        helper = started(tmp_path, process)
        process.stdin.close()
        lost = time.monotonic()
        process.wait(timeout=bound)
        assert process.returncode == 0 and time.monotonic() - lost < bound
        receipt = json.loads((tmp_path / "receipt.json").read_text())
        assert receipt["status"] == "interrupted"
    finally:
        if process.poll() is None:
            os.killpg(process.pid, signal.SIGKILL)
        process.wait()
        if helper is not None and not absent(helper):
            os.kill(helper, signal.SIGKILL)


@pytest.mark.parametrize("stop_first", [False, True])
async def test_deployment_owner_input_reads_on_after_a_stop_and_answers_stopped_at_eof(monkeypatch, stop_first):
    from types import SimpleNamespace
    import asyncio
    from launchkit import deployment_worker
    read_fd, write_fd = os.pipe()
    monkeypatch.setattr(deployment_worker.sys, "stdin", SimpleNamespace(buffer=os.fdopen(read_fd, "rb", buffering=0)))
    supervisor = SimpleNamespace(stop=asyncio.Event(), directory=Path("/nonexistent-owned-folder"))
    reading = asyncio.create_task(deployment_worker.owner_input(supervisor))
    try:
        if stop_first:
            os.write(write_fd, b'{"action":"stop"}\n')
            await asyncio.wait_for(supervisor.stop.wait(), 5)
            await asyncio.sleep(.1)
            # The owner is still there: the reader waits for its loss.
            assert not reading.done()
        os.close(write_fd)
        write_fd = None
        assert await asyncio.wait_for(reading, 5) == ("stopped" if stop_first else "owner_lost")
        assert supervisor.stop.is_set()
    finally:
        if write_fd is not None:
            os.close(write_fd)
        if not reading.done():
            reading.cancel()


def test_session_leader_that_is_not_an_owned_worker_never_arms_the_owner_loss_bound(tmp_path):
    """A test process can lead its own session (setsid, a container's PID 1)."""
    script = ("import sys, time\nfrom launchkit import owned_worker\n"
              "assert owned_worker.end_after_owner_loss(.2) is None\ntime.sleep(1)\n")
    process = subprocess.Popen([sys.executable, "-c", script], cwd=INSTALLER,
        env={**os.environ, "PYTHONPATH": str(INSTALLER)}, start_new_session=True)
    assert process.wait(timeout=20) == 0


def test_only_the_owned_worker_entry_points_declare_an_owned_worker():
    launchkit = INSTALLER / "launchkit"
    # Forge spawns these modules as "python -m launchkit.<module>"; the
    # deployment worker's own entry runs every non-interactive mode but service.
    for name in ("implementation_worker.py", "rehearsal_worker.py", "deployment_worker.py"):
        tree = ast.parse((launchkit / name).read_text())
        entry = [node for node in tree.body if isinstance(node, ast.If) and ast.unparse(node.test) == "__name__ == '__main__'"]
        assert len(entry) == 1, name
        calls = [ast.unparse(node.func) for node in ast.walk(entry[0]) if isinstance(node, ast.Call)]
        assert "serve_as_owned_worker" in calls and calls.index("serve_as_owned_worker") < calls.index("asyncio.run"), name
    # The two "python -c" entries that run an interactive deployment worker.
    scripts = [node.value for node in ast.walk(ast.parse((launchkit / "deployment.py").read_text()))
               if isinstance(node, ast.Constant) and isinstance(node.value, str)
               and node.value.startswith("import asyncio,sys; ")]
    assert len(scripts) == 2 and all("interactive_worker" in script for script in scripts)
    for script in scripts:
        assert "serve_as_owned_worker()" in script, script
        assert script.index("serve_as_owned_worker()") < script.index("asyncio.run("), script
    # Nothing else declares one: in-process callers, tests and the service
    # unit, which never reads its stdin, never arm the bound.
    entries = ("implementation_worker.py", "rehearsal_worker.py", "deployment_worker.py", "deployment.py", "owned_worker.py")
    for path in launchkit.glob("*.py"):
        if path.name not in entries:
            assert "serve_as_owned_worker" not in path.read_text(), path.name


def test_the_deployment_worker_module_entry_declares_an_owned_worker_before_it_runs(tmp_path):
    script = ("import asyncio, runpy, sys\nfrom launchkit import owned_worker\n"
              "def intercepted(coroutine):\n    coroutine.close()\n"
              "    print('declared:', owned_worker._owned_worker_entry, flush=True)\n    return 0\n"
              "asyncio.run = intercepted\n"
              "sys.argv = ['deployment_worker', sys.argv[1], 'install']\n"
              "runpy.run_module('launchkit.deployment_worker', run_name='__main__')\n")
    result = subprocess.run([sys.executable, "-c", script, str(tmp_path / "no-deployment")], cwd=INSTALLER,
        env={**os.environ, "PYTHONPATH": str(INSTALLER)}, capture_output=True, text=True, timeout=60)
    assert (result.returncode, result.stdout) == (0, "declared: True\n"), result.stderr[-2000:]


def test_process_outside_its_own_session_never_arms_the_owner_loss_bound(tmp_path):
    script = ("import sys, time\nfrom launchkit import owned_worker\n"
              "assert owned_worker.end_after_owner_loss(.2) is None\ntime.sleep(1)\n")
    process = subprocess.Popen([sys.executable, "-c", script], cwd=INSTALLER,
        env={**os.environ, "PYTHONPATH": str(INSTALLER)}, start_new_session=False)
    assert process.wait(timeout=20) == 0
