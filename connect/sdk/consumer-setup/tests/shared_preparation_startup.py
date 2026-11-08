"""Public shared-preparer startup fixture, with real Controller/socket custody.

The Rust owner pins this file and fixture.json in its verified resource closure.
Only preparation output and the timing/content of Ready are controlled. The
selected local_session module owns the actual inherited pipes, peer listeners,
admission close and Controller join. No native authority or install is emulated.
"""
import argparse
import asyncio
import importlib.util
import json
import os
from pathlib import Path
import select
import signal
import sys
import time


ROOT = Path(__file__).resolve().parent
CONFIG = json.loads((ROOT / "fixture.json").read_bytes())
OUTPUT = Path(CONFIG["output"]).resolve(strict=True)
MODE = CONFIG["mode"]
BOUND = 20


def retain(name, value):
    # A duplicate launch cannot silently replace the first owner's evidence.
    temporary = OUTPUT / (name + "." + str(os.getpid()) + ".tmp")
    with temporary.open("x", encoding="utf-8") as handle:
        json.dump(value, handle, sort_keys=True, separators=(",", ":"))
    os.link(temporary, OUTPUT / name)
    temporary.unlink()


def record_launch(role):
    with (OUTPUT / "LAUNCHES.jsonl").open("a", encoding="utf-8") as handle:
        handle.write(json.dumps({"role": role, "pid": os.getpid()}) + "\n")


def prepare(args):
    assert args.destination and args.realm_state_root and args.realm
    destination = Path(args.destination)
    assert destination.is_absolute() and not destination.exists()
    assert destination.parent == destination.parent.resolve(strict=True)
    assert Path(args.realm_state_root).is_absolute()
    cancelled = None
    if MODE == "preparer-cancel":
        def stop(signum, _frame):
            nonlocal cancelled
            cancelled = signum
        # The test never requests cancellation until the started marker below.
        signal.signal(signal.SIGTERM, stop)
    record_launch("preparer")
    retain("PREPARER-STARTED.json", {"pid": os.getpid(), "destination": str(destination)})
    if MODE == "preparer-cancel":
        def held_until(predicate):
            deadline = time.monotonic() + BOUND
            while not predicate():
                if time.monotonic() >= deadline:
                    raise TimeoutError("retained preparer boundary was not released")
                time.sleep(0.01)
        held_until(lambda: cancelled is not None)
        retain("PREPARER-CANCELLED.json", {"pid": os.getpid(), "signal": cancelled})
        held_until(lambda: (OUTPUT / "RELEASE-READY").exists())
        retain("PREPARER-RETURNING.json", {"pid": os.getpid()})
        return
    connection = {"mcpServers": {"toolkit-setup": {
        "command": "/usr/bin/env",
        "args": ["-i", "PATH=/usr/bin:/bin", "PYTHONDONTWRITEBYTECODE=1",
                 CONFIG["python"], "-I", "-B", str(ROOT / "local_session.py"), "--fixture"],
    }}}
    print(json.dumps(connection, sort_keys=True, separators=(",", ":")), flush=True)
    # The Rust preparer must additionally wait for this process to exit.
    retain("PREPARER-RETURNING.json", {"pid": os.getpid()})


async def wait_for(predicate, label):
    async with asyncio.timeout(BOUND):
        while not predicate():
            await asyncio.sleep(0.01)
    return label


async def controller(count):
    assert count == 2 and MODE in {"cancel", "malformed", "ready"}
    record_launch("controller")
    # The real adapter's joined cleanup uses anyio. Check this existing
    # dependency before allocating listeners, so an incomplete selected
    # interpreter cannot reach Ready and fail only when it must join them.
    importlib.import_module("anyio")
    source = Path(CONFIG["source"]).resolve(strict=True)
    sys.path.insert(0, str(source / "tests"))
    from test_selected_setup import SelectedFixture, adapter

    spec = importlib.util.spec_from_file_location("startup_local_session", source / "local_session.py")
    local = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(local)
    fixture = SelectedFixture()
    fixture.setUp()
    owner = fixture.controller()
    paths = []
    original = local.shared_connections
    retain("CONTROLLER-STARTED.json", {"pid": os.getpid()})

    async def connections(adapter_module, actual, selected_count, stopped, ready, **kwargs):
        assert actual is owner and selected_count == count

        async def boundary(actual_paths):
            paths.extend(actual_paths)
            retain("READY-BOUNDARY.json", {"pid": os.getpid(), "paths": actual_paths})
            if MODE != "cancel":
                emitted = actual_paths if MODE == "ready" else [actual_paths[0], actual_paths[0]]
                await ready(emitted)
                retain("READY-EMITTED.json", {"valid": MODE == "ready", "paths": emitted})
            # No owner command is possible before Ready, and these tests send
            # none afterward. Read only EOF from the duplicate original fd;
            # unexpected bytes fail the fixture instead of becoming a receipt.
            # The real inherited reader independently observes the same EOF.
            def input_closed():
                if not select.select([sys.stdin.fileno()], [], [], 0)[0]:
                    return False
                assert os.read(sys.stdin.fileno(), 1) == b""
                return True
            await wait_for(input_closed, "actual inherited input closed")
            retain("OWNER-INPUT-CLOSED.json", {"pid": os.getpid()})
            await wait_for(lambda: (OUTPUT / "RELEASE-READY").exists(), "test releases Ready owner")

        return await original(adapter_module, actual, selected_count, stopped, boundary, **kwargs)

    local.shared_connections = connections
    try:
        await local.shared_stdio_owner(adapter, owner, count)
    finally:
        local.shared_connections = original
        owner.close_admission()
        await asyncio.to_thread(owner.join)
        workers = sum(thread.is_alive() for thread in owner.threads)
        removed = all(not Path(path).exists() for path in paths)
        try:
            assert workers == 0 and removed
            retain("OWNER-JOINED.json", {"pid": os.getpid(), "workers_alive": workers,
                                       "closing": owner.closing, "sockets_removed": removed,
                                       "paths": paths})
        finally:
            fixture.tearDown()


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--destination")
    parser.add_argument("--realm-state-root")
    parser.add_argument("--realm")
    parser.add_argument("--fixture", action="store_true")
    parser.add_argument("--shared-peers", type=int)
    parser.add_argument("--shared-close-id")
    # The public launcher supplies passive pre-spawn identity. These startup
    # controls exercise Ready/cancellation only and issue no abandonment proof.
    parser.add_argument("--shared-lifetime")
    parser.add_argument("--shared-lifetime-sha256")
    arguments = parser.parse_args()
    if arguments.fixture:
        asyncio.run(controller(arguments.shared_peers))
    else:
        prepare(arguments)
