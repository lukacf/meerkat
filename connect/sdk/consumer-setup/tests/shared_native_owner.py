"""Inert physical worker for the public SDK two-child native control.

Only the installer/validator output is controlled. The real Controller, peer
MCP transports, native dispatch, review and canonical P owners remain in use.
This does not establish package activation or usable service operations.

The worker runs under the interpreter the native controls select
(TOOLKIT_SHARED_TEST_PYTHON or TOOLKIT_D5_TEST_PYTHON), which must hold the
whole pinned set of scripts/connect-test-python-requirements.txt: mcp 2.2.0
and its closure, among them httpx2, anyio, jsonschema and referencing.
scripts/connect-test-python prepares that interpreter. The setup modules
import those packages only when a call first needs them, so the worker checks
the set before it imports anything else and stops, naming each distribution
that is missing or at another version.
"""
import argparse
import asyncio
import importlib.metadata
import importlib.util
import json
from pathlib import Path
import re
import sys
import threading
import time

HERE = Path(__file__).resolve().parent
# The pinned set scripts/connect-test-python installs, in this checkout.
REQUIREMENTS = HERE.parents[3] / "scripts" / "connect-test-python-requirements.txt"


def unmet_requirements(requirements=REQUIREMENTS):
    """Each pinned distribution this interpreter lacks or holds at another version."""
    unmet = []
    for line in requirements.read_text(encoding="utf-8").splitlines():
        line = line.split("#", 1)[0].strip()
        if not line:
            continue
        name, version = line.split()[0].split("==")
        try:
            found = importlib.metadata.version(name)
        except importlib.metadata.PackageNotFoundError:
            found = "absent"
        if found != version:
            unmet.append(f"{re.sub(r'[-_.]+', '-', name).lower()}=={version} ({found})")
    return unmet


def require_pinned_python():
    try:
        unmet = unmet_requirements()
    except (OSError, ValueError) as error:
        raise SystemExit(f"shared_native_owner: cannot read the pinned Python set {REQUIREMENTS}: {error}")
    if unmet:
        raise SystemExit(f"shared_native_owner: {sys.executable} lacks the pinned Python set of "
                         "scripts/connect-test-python-requirements.txt (prepare it with "
                         "scripts/connect-test-python): " + ", ".join(unmet))
    # The modules the setup paths import on demand, at those exact versions.
    import anyio, jsonschema, referencing  # noqa: E401, F401
    import mcp.server.stdio  # noqa: F401
    from mcp import types  # noqa: F401
    from mcp.server import Server  # noqa: F401


require_pinned_python()
sys.path.insert(0, str(HERE))
from test_selected_setup import SelectedFixture, SelectedRunner, adapter, backend

spec = importlib.util.spec_from_file_location("sdk_shared_local_session", HERE.parent / "local_session.py")
local = importlib.util.module_from_spec(spec)
spec.loader.exec_module(local)


async def main(output, lifetime_args=None):
    output = Path(output).resolve(strict=True)
    fixture = SelectedFixture()
    fixture.setUp()
    physical = []
    lock = threading.Lock()

    def retain(name, value):
        raw = json.dumps(value, sort_keys=True, separators=(",", ":")).encode()
        temporary = output / (name + ".tmp")
        temporary.write_bytes(raw)
        temporary.replace(output / name)

    class HeldRunner(SelectedRunner):
        def run(self, argv, capture=False):
            if "--package" not in argv and "--verify-existing" not in argv:
                with lock:
                    physical.append(list(argv))
                    retain("WORKER-ENTERED.json", {"entries": physical})
                deadline = time.monotonic() + 90
                while not (output / "RELEASE-WORKER").exists():
                    if time.monotonic() >= deadline:
                        retain("WORKER-BOUND-FAILED.json", {"reason": "test owner did not release worker"})
                        return 2, b"test owner release absent"
                    time.sleep(0.01)
            return super().run(argv, capture)

    controller = fixture.controller(runner_factory=HeldRunner)
    original_connections = local.shared_connections
    lifetime = None
    try:
        if lifetime_args is not None:
            lifetime_args.state_root = str(output / "prepared/local-session")
            lifetime_args.shared_peers = 2
            lifetime_args.shared_restore_sha256 = None
            lifetime = local.selected_shared_lifetime(lifetime_args)
            controller._shared_entry_census = adapter.SharedEntryCensus(
                output / ("shared-entries-" + lifetime_args.shared_close_id),
                local.digest(lifetime[1]))
            Path(lifetime_args.state_root).mkdir(parents=True, mode=0o700)

            async def connections(module, actual, count, stopped, ready, **kwargs):
                async def with_request(paths):
                    # This is an actual Controller transport request, deliberately
                    # not represented as a native tool invocation or approval.
                    owner = kwargs["_owner"]
                    command = {"operation": "prepare_entry", "peer": 0,
                        "connection": "1", "tool": "setup_state",
                        "arguments": {}, "preparation": None}
                    token = owner.owner_control(command)["entry"]
                    if lifetime_args.census_mode == "entered":
                        await owner._connections[0].dispatch("/api/state", {}, _entry=token)
                    owner.owner_control({"operation": "close_entry", "peer": 0, "entry": token})
                    owner.owner_control({"operation": "finish_entry", "peer": 0, "entry": token})
                    retain("CENSUS-REQUEST.json", {"entry": token, "request": command})
                    await ready(paths)
                return await original_connections(module, actual, count, stopped, with_request, **kwargs)
            local.shared_connections = connections
        selected = fixture.stage("python")
        retain("SELECTED-INPUT.json", selected)
        joined = await local.shared_stdio_owner(adapter, controller, 2)
        if lifetime is not None:
            local.observe_shared_abandonment(Path(lifetime_args.state_root), joined, lifetime)
    finally:
        local.shared_connections = original_connections
        # Failure cleanup still joins the real worker before the process exits.
        # This release is not the successful cancellation-before-release proof.
        if not (output / "RELEASE-WORKER").exists():
            (output / "RELEASE-WORKER").write_text("failure cleanup\n")
        await asyncio.to_thread(controller.join)
        retain("OWNER-JOINED.json", {
            "physical_entries": len(physical),
            "workers_alive": sum(thread.is_alive() for thread in controller.threads),
            "closing": controller.closing,
        })
        fixture.tearDown()


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("output")
    parser.add_argument("--shared-close-id")
    parser.add_argument("--shared-lifetime")
    parser.add_argument("--shared-lifetime-sha256")
    parser.add_argument("--census-mode", choices=("not_entered", "entered"))
    args = parser.parse_args()
    selected = args if args.census_mode is not None else None
    assert selected is not None or all(value is None for value in (
        args.shared_close_id, args.shared_lifetime, args.shared_lifetime_sha256))
    asyncio.run(main(args.output, selected))
