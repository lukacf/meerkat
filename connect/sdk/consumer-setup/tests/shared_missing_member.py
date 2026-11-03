"""Actual Controller boundary fed the real Rust P owner's leave disposition.

This is controlled package I/O and a private source-protocol test. It does not
establish native authorization, SDK entry, user approval or service readiness.
"""
import asyncio
import json
from pathlib import Path
import sys
import threading
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
from test_selected_setup import SelectedFixture, adapter
from test_shared_preparation import transport_call


def emit(value):
    print(json.dumps(value, sort_keys=True, separators=(",", ":")), flush=True)


async def main():
    fixture = SelectedFixture()
    fixture.setUp()
    controller = fixture.controller()
    owner = adapter.ControllerConnections(controller)
    a, b = owner.open(), owner.open()
    waiting = None
    report = {"stage": "result", "ok": False}
    try:
        request = fixture.stage("node")
        pending_a = await transport_call(a, "/api/selected_check", request)
        pending_b = await transport_call(b, "/api/selected_check", request)
        inspect_a = owner.owner_control({"operation": "inspect_selection", "peer": 0,
                                         "ticket": pending_a["ticket"]})
        inspect_b = owner.owner_control({"operation": "inspect_selection", "peer": 1,
                                         "ticket": pending_b["ticket"]})
        assert inspect_a["selection"] == inspect_b["selection"]
        assert not controller.jobs
        emit({"stage": "selected", "a": inspect_a["selection"], "b": inspect_b["selection"]})
        async with asyncio.timeout(15):
            line = await asyncio.to_thread(sys.stdin.readline)
        command = json.loads(line)
        assert set(command) == {"preparation", "last"}
        assert type(command["last"]) is bool
        preparation = command["preparation"]
        assert isinstance(preparation, str) and len(preparation) == 64
        assert all(char in "0123456789abcdef" for char in preparation)
        # Rust has now committed B to this exact P. A still has only its pending
        # selection; its absent-P disposition was computed before B joined.
        checked_b = owner.owner_control({"operation": "release_selection", "peer": 1,
            "ticket": pending_b["ticket"], "selection_sha256": inspect_b["selection_sha256"],
            "preparation": preparation})
        controller.runner.block = threading.Event()
        waiting = asyncio.create_task(transport_call(b, "/api/selected_install", fixture.selection(checked_b)))
        assert await asyncio.to_thread(controller.runner.entered.wait, 3), "B worker did not enter"
        job = controller.validations[checked_b["validation_id"]]["job_id"]
        assert controller._job_threads[job].is_alive()
        assert not controller._job_controls[job]["requested"].is_set()
        assert a in owner._pending and b not in owner._pending
        with patch.object(controller, "stop", wraps=controller.stop) as stopping:
            callback = owner.owner_control({"operation": "leave_preparation", "peer": 0,
                "validation_id": inspect_a["selection"]["result"]["validation_id"],
                "preparation": preparation, "last": command["last"]})
            report.update(stop_calls=stopping.call_count,
                stop_requested_while_held=controller._job_controls[job]["requested"].is_set(),
                worker_alive_after_callback=controller._job_threads[job].is_alive(),
                callback={key: callback[key] for key in
                          ("scope", "last_member", "physical_stop_requested", "worker_joined")})
            assert stopping.call_count == 0, "absent A's delayed callback stopped B's physical work"
            assert not report["stop_requested_while_held"]
            assert report["worker_alive_after_callback"]
            assert not callback["physical_stop_requested"] and not callback["worker_joined"]
            assert a not in owner._pending
            assert not controller.closing and not waiting.done()
            controller.runner.block.set()
            installed = await asyncio.wait_for(asyncio.shield(waiting), 3)
            report["b_status"] = installed["job"]["status"]
            assert report["b_status"] == "succeeded"
            assert stopping.call_count == 0
        report["ok"] = True
    except BaseException as error:
        report["error"] = f"{type(error).__name__}: {error}"
    finally:
        if controller.runner.block is not None:
            controller.runner.block.set()
        if waiting is not None:
            settled = await asyncio.gather(waiting, return_exceptions=True)
            if isinstance(settled[0], dict):
                report.setdefault("b_status", settled[0].get("job", {}).get("status"))
        try:
            async with asyncio.timeout(5):
                await owner.aclose()
            report["owner_closed"] = True
        except BaseException as error:
            report["ok"] = False
            report["cleanup_error"] = f"{type(error).__name__}: {error}"
        fixture.tearDown()
        report["workers_alive_after_close"] = sum(thread.is_alive() for thread in controller.threads)
        report["pending_waiters_after_close"] = len(owner._job_waits)
        report["physical_entries"] = sum("--package" not in argv and "--verify-existing" not in argv
                                         for argv, _ in controller.runner.calls)
        if report["workers_alive_after_close"] or report["pending_waiters_after_close"]:
            report["ok"] = False
        emit(report)
    return 0 if report["ok"] else 1


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
