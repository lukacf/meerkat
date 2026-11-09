"""Real process constituent controls, not native authority or final adoption.

The owner uses the real Controller, Runner, ControllerConnections, listener and
census. Only its package input is inert fixture data; no install is requested.
The bridge-only mode isolates the actual native stdio bridge's own OS lease.
"""
import asyncio
import importlib.util
import json
import os
from pathlib import Path
import select
import socket
import subprocess
import sys
import tempfile


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value


HERE = Path(__file__).resolve().parent
local = load("physical_local", HERE.parent / "local_session.py")
adapter = load("physical_adapter", HERE.parent / "mcp_adapter.py")
backend = adapter.backend


def output(value):
    sys.stdout.buffer.write(local.encoded(value))
    sys.stdout.buffer.flush()


def line():
    ready, _, _ = select.select([sys.stdin], [], [], 30)
    if not ready:
        raise RuntimeError("test command deadline")
    raw = sys.stdin.buffer.readline(1024 * 1024)
    if not raw:
        return {"op": "close"}
    return json.loads(raw)


async def owner(path, sha, mode):
    directory = path.parent
    physical = local.PhysicalLease(path, sha)
    controller = None
    descendant = None
    primary = None
    try:
        arguments = ["--bundle", str(directory / "fixture-bundle"), "--families", "agno"]
        if mode == "unsupported":
            arguments += ["--compose-profile", "unsupported-for-cold-classification"]
        physical.startup(arguments, False)
        if mode == "initialization-gap":
            output({"ready": True, "initialization_gap": True})
            os._exit(77)
        bundle = directory / "fixture-bundle"
        bundle.mkdir(mode=0o700)
        files = {"install-library-consumer.py": b"# no installer execution in this control\n"}
        for family in backend.FAMILIES:
            files[family + "/PROFILE-GUIDE.md"] = ("Exact " + family + " guide\n").encode()
        for key, raw in files.items():
            destination = bundle / key
            destination.parent.mkdir(mode=0o700, exist_ok=True)
            local.write_new(destination, raw)
        manifest = backend.encoded({"files": {key: {"sha256": backend.digest(raw)} for key, raw in files.items()}})
        local.write_new(bundle / "CONTENTS.json", manifest)
        # The real Runner is retained. No FakeRunner or simulated operation
        # outcome participates in this physical-only control.
        controller = backend.Controller(bundle, backend.digest(manifest), directory / "fixture-work",
            backend.FAMILIES, sys.executable, sys.executable, sys.executable)
        controller._shared_physical = physical
        controller._shared_entry_census = adapter.SharedEntryCensus(
            directory / ("shared-entries-" + physical.value["owner"]), sha, _physical=physical)
        if mode == "framed":
            # The actual inherited owner command loop, not fixture-generated
            # response envelopes, exercises the Rust shared call-context door.
            await local.shared_stdio_owner(adapter, controller, physical.value["peers"])
            return
        actual = adapter.ControllerConnections(controller, _entry_census=controller._shared_entry_census)

        async def ready(paths):
            output({"ready": True, "paths": paths, "controller": actual._identity})

        async def stopped():
            nonlocal descendant
            while True:
                request = await asyncio.to_thread(line)
                action = request.get("op")
                if action == "close":
                    return
                if action == "crash":
                    os._exit(77)
                if action == "detached-after-entry":
                    # The harness, not a production state read, creates this
                    # detached writer. It deliberately closes every lease fd.
                    # This proves exclusive lease availability is insufficient
                    # to classify an entered operation as no effect.
                    code = (
                        "import pathlib,sys,time; p=pathlib.Path(sys.argv[1]); "
                        "(p/'descendant-live').write_text('live'); "
                        "deadline=time.monotonic()+30; "
                        "exec(\"while time.monotonic()<deadline and not (p/'release-descendant').exists():\\n time.sleep(0.01)\"); "
                        "(p/'descendant-done').write_text('done')"
                    )
                    descendant = subprocess.Popen([sys.executable, "-I", "-B", "-c", code, str(directory)],
                        stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                        close_fds=True, start_new_session=True)
                    deadline = asyncio.get_running_loop().time() + 10
                    while not (directory / "descendant-live").exists():
                        if asyncio.get_running_loop().time() >= deadline:
                            raise RuntimeError("descendant did not start")
                        await asyncio.sleep(0.01)
                    # Exit the real shared owner loop first. Its finally joins
                    # transports and Controller; the test supervisor alone stays
                    # alive to waitpid its detached writer after Rust inspects.
                    return
                if action == "enter":
                    # A real Controller state route is still an entered call,
                    # so the nonentry reader must refuse it without heuristics.
                    actual.dispatch(actual._connections[0], "/api/state", {}, request["entry"])
                    output({"entered": True})
                else:
                    output(actual.owner_control(request))
        await local.shared_connections(adapter, controller, physical.value["peers"], stopped, ready, _owner=actual)
    except BaseException as error:
        primary = error
        raise
    finally:
        failures = []
        if controller is not None:
            try:
                controller.close_admission()
            except BaseException as error:
                failures.append(error)
            try:
                await asyncio.wait_for(asyncio.to_thread(controller.join), 10)
            except BaseException as error:
                failures.append(error)
        try:
            physical.close()
        except BaseException as error:
            failures.append(error)
        if descendant is not None:
            try:
                if primary is None and not failures:
                    output({"detached": True, "tracked_joined": True})
                    deadline = asyncio.get_running_loop().time() + 30
                    while not (directory / "release-descendant").exists() and descendant.poll() is None:
                        if asyncio.get_running_loop().time() >= deadline:
                            break
                        await asyncio.sleep(0.01)
                (directory / "release-descendant").write_text("release")
                status = await asyncio.to_thread(descendant.wait, 10)
            except BaseException as error:
                failures.append(error)
                # This is a test-owned Popen only. Always reap after force and
                # retain the failure; forced cleanup is never a passing oracle.
                try:
                    descendant.kill()
                except ProcessLookupError:
                    pass
                status = await asyncio.to_thread(descendant.wait)
            try:
                local.write_new(directory / "descendant-joined.json", local.encoded({"returncode": status,
                    "scope": "actual_test_supervisor_waitpid"}))
            except BaseException as error:
                failures.append(error)
        if failures:
            if primary is not None:
                primary.add_note("physical fixture cleanup also failed")
            else:
                raise failures[0]



def bridge_endpoint(path, sha):
    """An inert socket isolates bridge custody; never emits startup proof."""
    with tempfile.TemporaryDirectory(prefix="physical-bridge-", dir="/tmp") as temporary:
        target = Path(temporary).resolve() / "0.sock"
        with socket.socket(socket.AF_UNIX) as listener:
            listener.bind(str(target))
            os.chmod(target, 0o600)
            listener.listen(1)
            listener.settimeout(15)
            with local.PhysicalLease(path, sha) as physical:
                physical.peer(0, target, target.lstat())
            output({"ready": True, "paths": [str(target)]})
            with listener.accept()[0] as connection:
                output({"connected": True})
                action = line().get("op")
                if action == "close":
                    return
                if action != "disconnect":
                    raise RuntimeError("unexpected endpoint command")
            # Preserve this exact socket inode/roster after the first bridge.
            # A late child must be refused by the seal, not a missing endpoint.
            output({"disconnected": True})
            action = line().get("op")
            if action == "probe-late":
                readable, _, _ = select.select([listener], [], [], 0)
                output({"connection_pending": bool(readable)})
            elif action != "close":
                raise RuntimeError("unexpected endpoint command")


if __name__ == "__main__":
    selected, expected, mode = Path(sys.argv[1]), sys.argv[2], sys.argv[3]
    if mode == "bridge":
        bridge_endpoint(selected, expected)
    else:
        asyncio.run(owner(selected, expected, mode))
