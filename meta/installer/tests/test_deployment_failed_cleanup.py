"""Owned real child/loopback fixtures, not live native deployment evidence."""
import asyncio
import hashlib
import io
import json
import os
from pathlib import Path
import signal
import socket
import sys
import uuid
from unittest.mock import AsyncMock
import zipfile

import pytest

from launchkit import deployment, deployment_worker
from launchkit.deployment import DeploymentService, OWNER_FILE, materialize, read_receipt, update_receipt
from launchkit.projects import DraftConflict
from launchkit.templates.storage_setup import project_lock
from test_deployment import Guide, Implementation, Service, submit
from test_deployment_settings import prepared, body


APP = '''import json,os,signal,subprocess,sys,time
from http.server import BaseHTTPRequestHandler,HTTPServer
from pathlib import Path
from storage_setup import project_lock
with project_lock(Path('.')):
    values={}
    for line in Path('.env').read_text().splitlines():
        key,sep,value=line.partition('=')
        if sep and not key.startswith('#'): values[key]=json.loads(value)
    child=subprocess.Popen([sys.executable,'-c','import time; time.sleep(60)'])
    Path('.mobkit/test-child.json').write_text(json.dumps({'parent':os.getpid(),'child':child.pid}))
    def stop(*_):
        child.terminate()
        child.wait(timeout=5)
        raise SystemExit(0)
    signal.signal(signal.SIGTERM,stop)
    fd=int(os.environ['MOBKIT_LAUNCH_READY_FD'])
    if values.get('APP_READY') != 'yes':
        if values.get('APP_READY') != 'hold': os.close(fd)
        while True: time.sleep(1)
    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            content=json.dumps({'identities':[{'identity':'lead:main'}]}).encode()
            self.send_response(200); self.send_header('Content-Length',str(len(content))); self.end_headers()
            self.wfile.write(content)
        def log_message(self,*_): pass
    port=int(values['MOBKIT_PORT'])
    server=HTTPServer(('127.0.0.1',port),Handler)
    evidence={'pid':os.getpid(),'bootstrap':{'complete':True,'ready':True,'counts':{'broken':0},'identities':{'lead:main':{}}},'console_url':f'http://127.0.0.1:{port}/console'}
    os.write(fd,(json.dumps(evidence)+'\\n').encode()); os.close(fd)
    server.serve_forever()
'''


class ChildImplementation(Implementation):
    async def package(self, identity, build_id):
        payload, slug = await super().package(identity, build_id)
        source = Path(deployment.__file__).parent / "templates/storage_setup.py"
        result = io.BytesIO()
        with zipfile.ZipFile(io.BytesIO(payload)) as old, zipfile.ZipFile(result, "w") as archive:
            for name in old.namelist():
                archive.writestr(zipfile.ZipInfo(name), APP if name == "app.py" else old.read(name))
            archive.writestr(zipfile.ZipInfo("storage_setup.py"), source.read_bytes())
        return result.getvalue(), slug


async def real_fixture(tmp_path):
    guide = Guide()
    implementation = ChildImplementation(guide)
    implementation.digest = "a" * 64
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        implementation.definition["port"] = probe.getsockname()[1]
    implementation.application_configuration = {"fields": [
        {"key": "APP_READY", "label": "Fixture ready", "required": True, "secret": False}]}
    held = Service(tmp_path / "server", guide, implementation)
    target = tmp_path / "application"
    initial = await submit(held, target, settings={"OPENAI_API_KEY": "fixture-private", "APP_READY": "no"})
    directory = held._directory(guide.identity)
    materialize(directory)
    await held.shutdown()
    update_receipt(directory, status="stopped", interactive_lifecycle_id=None, interactive_cleanup=None, runtime_pid=None, process_live=False)
    interpreter = target / ".venv/bin/python"
    interpreter.parent.mkdir(parents=True)
    interpreter.symlink_to(sys.executable)
    (target / "data").mkdir()
    (target / "data/records.json").write_text('{"kept":"domain record"}')
    (target / ".mobkit/state").mkdir()
    (target / ".mobkit/state/native.json").write_text('{"kept":"native record"}')
    service = DeploymentService(tmp_path / "server", guide, implementation)
    return service, directory, target, initial


async def wait_status(service, wanted):
    async with asyncio.timeout(12):
        while True:
            # A read that overlaps a task's last receipt write is projected as
            # not current, without cleanup recovery. A settled state needs a
            # read that starts and ends with no deployment task.
            idle = not service.tasks
            receipt = await service.status(service.guide.identity)
            # A concurrent readiness transition can conservatively project
            # running/unknown until the next stable source observation. Wait
            # for the actual checked health required by the assertions below.
            ready = (receipt.get("health") == "ready"
                and (receipt.get("startup_console_health") or {}).get("status") == "pass")
            if receipt["status"] == wanted and (ready if wanted == "running" else idle and not service.tasks):
                return receipt
            await asyncio.sleep(0.02)


def absent(pid):
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return True
    return False


async def test_real_failed_child_cleanup_stop_settings_retry_preserves_installed_state(tmp_path):
    service, directory, target, initial = await real_fixture(tmp_path)
    before = {name: (target / name).read_bytes() for name in [
        "app.py", "deployment.json", OWNER_FILE, "data/records.json", ".mobkit/state/native.json"]}
    try:
        await service.action(service.guide.identity, initial["deployment_id"], "restart")
        failed = await wait_status(service, "failed")
        child = json.loads((target / ".mobkit/test-child.json").read_text())
        assert all(absent(pid) for pid in child.values())
        assert deployment.interactive_cleanup_matches(failed)
        assert failed["interactive_cleanup"]["return_code"] == 1
        assert failed["cleanup_recovery"]["available"] is True
        assert failed["cleanup_recovery"]["source_digest"] == "a" * 64
        with pytest.raises(DraftConflict, match="Stop this installed"):
            await service.settings_options(service.guide.identity, initial["deployment_id"])
        # A newer terminal unaccepted candidate supplies neither installed bytes nor settings.
        service.implementation.identity = str(uuid.uuid4())
        service.implementation.status = AsyncMock(return_value={"build_id": service.implementation.identity, "status": "failed"})
        service.implementation.package = AsyncMock(side_effect=AssertionError("installed source must remain authoritative"))
        stopped = await service.action(service.guide.identity, initial["deployment_id"], "stop")
        assert stopped["status"] == "stopped" and stopped["runtime_pid"] is None
        options = await service.settings_options(service.guide.identity, initial["deployment_id"])
        assert options["build_id"] == initial["build_id"] and options["source_digest"] == "a" * 64
        await service.configure(service.guide.identity, **body(service, initial), settings={"APP_READY": "yes"})
        configured = read_receipt(directory)
        assert configured["startup_attempt_id"] is None and configured["startup_console_health"] is None
        assert configured["interactive_cleanup_attempt_id"] == failed["startup_attempt_id"]
        assert configured["interactive_cleanup"] == failed["interactive_cleanup"]
        assert deployment.interactive_cleanup_matches(configured)
        assert all((target / name).read_bytes() == content for name, content in before.items())
        previous_lifecycle = failed["interactive_lifecycle_id"]
        await service.action(service.guide.identity, initial["deployment_id"], "restart")
        running = await wait_status(service, "running")
        assert running["health"] == "ready" and running["startup_console_health"]["status"] == "pass"
        assert running["interactive_lifecycle_id"] != previous_lifecycle and running["interactive_cleanup"] is None
        await service.action(service.guide.identity, initial["deployment_id"], "stop")
        final = await wait_status(service, "stopped")
        assert deployment.interactive_cleanup_matches(final)
        assert all(absent(pid) for pid in json.loads((target / ".mobkit/test-child.json").read_text()).values())
        assert all((target / name).read_bytes() == content for name, content in before.items())
        assert "fixture-private" not in json.dumps(final)
    finally:
        await service.shutdown()


async def test_failed_wait_skips_a_status_read_spanning_the_final_cleanup_write(tmp_path, monkeypatch):
    service, directory, target, initial = await real_fixture(tmp_path)
    exited, guide, spanned = asyncio.subprocess.Process.wait, service._guide, []
    async def confirm_exit_later(process):
        # Keep the worker's failed receipt visible before the task confirms its exit.
        code = await exited(process)
        await asyncio.sleep(0.3)
        return code
    async def read_across_final_write(identity):
        task = service.tasks.get(directory.name)
        if not spanned and task is not None and read_receipt(directory)["status"] == "failed":
            # Hold one status read until the task writes its exit confirmation and ends.
            spanned.append(task)
            await asyncio.wait({task})
        return await guide(identity)
    monkeypatch.setattr(asyncio.subprocess.Process, "wait", confirm_exit_later)
    monkeypatch.setattr(service, "_guide", read_across_final_write)
    try:
        await service.action(service.guide.identity, initial["deployment_id"], "restart")
        failed = await wait_status(service, "failed")
        assert spanned and spanned[0].done()
        assert failed["cleanup_recovery"]["available"] is True and failed["current"] is True
    finally:
        await service.shutdown()


async def test_unknown_cleanup_with_free_lease_cannot_stop_change_settings_or_retry(tmp_path):
    service, directory, target, initial = await prepared(tmp_path)
    update_receipt(directory, status="failed", interactive_lifecycle_id=str(uuid.uuid4()), interactive_cleanup=None)
    with project_lock(target):
        pass  # An abandoned parent can release this while native descendants still exist.
    before = (directory / "receipt.json").read_bytes(), (target / ".env").read_bytes()
    service._launch = lambda *_: pytest.fail("unknown cleanup must not start another runtime")
    for action in ["stop", "retry", "restart"]:
        with pytest.raises(DraftConflict, match="cleanup is unconfirmed"):
            await service.action(service.guide.identity, initial["deployment_id"], action)
    with pytest.raises(DraftConflict, match="Stop this installed"):
        await service.settings_options(service.guide.identity, initial["deployment_id"])
    receipt = await service.status(service.guide.identity)
    assert receipt["status"] == "failed" and receipt["cleanup_recovery"]["available"] is False
    assert ((directory / "receipt.json").read_bytes(), (target / ".env").read_bytes()) == before


async def test_same_package_form_cannot_overwrite_private_values_or_launch_with_unknown_cleanup(tmp_path):
    service, directory, target, initial = await real_fixture(tmp_path)
    service.implementation.package = AsyncMock(return_value=((directory / "package.zip").read_bytes(), "test-team"))
    update_receipt(directory, status="failed", interactive_cleanup=None)
    before = (directory / "receipt.json").read_bytes(), (target / ".env").read_bytes()
    service._launch = lambda *_: pytest.fail("resubmitting the form must not bypass cleanup")
    with pytest.raises(DraftConflict, match="Stop and verify cleanup"):
        await submit(service, target, settings={"OPENAI_API_KEY": "replacement-private", "APP_READY": "yes"})
    assert ((directory / "receipt.json").read_bytes(), (target / ".env").read_bytes()) == before


async def test_same_package_form_rechecks_stopped_boundary_after_awaited_preparation(tmp_path, monkeypatch):
    service, directory, target, initial = await real_fixture(tmp_path)
    service.implementation.package = AsyncMock(return_value=((directory / "package.zip").read_bytes(), "test-team"))
    original = deployment.prepare_private_settings
    def changed(*args, **kwargs):
        result = original(*args, **kwargs)
        update_receipt(directory, status="starting", runtime_pid=123, process_live=True)
        return result
    monkeypatch.setattr(deployment, "prepare_private_settings", changed)
    service._launch = lambda *_: pytest.fail("changed runtime ownership must not admit a second worker")
    before = (target / ".env").read_bytes()
    with pytest.raises(DraftConflict, match="Stop this installed"):
        await submit(service, target, settings={"OPENAI_API_KEY": "replacement-private", "APP_READY": "yes"})
    assert (target / ".env").read_bytes() == before


@pytest.mark.parametrize("action", ["retry", "restart", "submit"])
async def test_stopped_receipt_with_owned_cleanup_task_cannot_admit_a_replacement(tmp_path, action):
    service, directory, target, initial = await real_fixture(tmp_path)
    service.implementation.package = AsyncMock(return_value=((directory / "package.zip").read_bytes(), "test-team"))
    task = asyncio.create_task(asyncio.Event().wait())
    service.tasks[initial["deployment_id"]] = task
    before = (directory / "receipt.json").read_bytes(), (target / ".env").read_bytes()
    service._launch = lambda *_: pytest.fail("the original task must not be replaced during teardown")
    try:
        with pytest.raises(DraftConflict):
            if action == "submit":
                await submit(service, target, settings={"OPENAI_API_KEY": "replacement-private", "APP_READY": "yes"})
            else:
                await service.action(service.guide.identity, initial["deployment_id"], action)
        assert service.tasks[initial["deployment_id"]] is task
        assert ((directory / "receipt.json").read_bytes(), (target / ".env").read_bytes()) == before
    finally:
        task.cancel()
        await asyncio.gather(task, return_exceptions=True)
        service.tasks.clear()


async def test_active_stop_cleanup_error_does_not_publish_stopped_or_settings_access(tmp_path):
    service, directory, target, initial = await real_fixture(tmp_path)
    identity = str(uuid.uuid4())
    update_receipt(directory, status="starting", interactive_lifecycle_id=identity, interactive_cleanup=None)
    service.worker_admissions[initial["deployment_id"]] = {"lifecycle_id": identity, "spawn_started": True}
    admitted = asyncio.Event()
    async def cleanup_error():
        admitted.set()
        try:
            await asyncio.Event().wait()
        finally:
            raise RuntimeError("fixture cleanup failed")
    task = asyncio.create_task(cleanup_error())
    service.tasks[initial["deployment_id"]] = task
    await admitted.wait()
    with pytest.raises(DraftConflict, match="cleanup is unconfirmed"):
        await service.action(service.guide.identity, initial["deployment_id"], "stop")
    assert read_receipt(directory)["status"] == "interrupted"
    with pytest.raises(DraftConflict, match="Stop this installed"):
        await service.settings_options(service.guide.identity, initial["deployment_id"])


async def test_actual_spawn_admission_race_stop_waits_for_wrapper_child_cleanup(tmp_path, monkeypatch):
    service, directory, target, initial = await real_fixture(tmp_path)
    await service.configure(service.guide.identity, **body(service, initial), settings={"APP_READY": "hold"})
    original = asyncio.create_subprocess_exec
    admitted, release = asyncio.Event(), asyncio.Event()
    async def delayed_handle(*args, **kwargs):
        process = await original(*args, **kwargs)
        admitted.set()
        await release.wait()
        return process
    monkeypatch.setattr(asyncio, "create_subprocess_exec", delayed_handle)
    try:
        await service.action(service.guide.identity, initial["deployment_id"], "restart")
        await admitted.wait()
        async with asyncio.timeout(5):
            while not (target / ".mobkit/test-child.json").exists():
                await asyncio.sleep(0.01)
        assert initial["deployment_id"] not in service.processes
        stopping = asyncio.create_task(service.action(service.guide.identity, initial["deployment_id"], "stop"))
        await asyncio.sleep(0)
        release.set()
        stopped = await asyncio.wait_for(stopping, 8)
        assert stopped["status"] == "stopped" and deployment.interactive_cleanup_matches(stopped)
        assert stopped["interactive_cleanup"]["boundary"] == "owned-process-groups-absent"
        assert all(absent(pid) for pid in json.loads((target / ".mobkit/test-child.json").read_text()).values())
    finally:
        release.set()
        await service.shutdown()


async def test_worker_stopped_receipt_without_marker_is_interrupted_before_handles_disappear(tmp_path, monkeypatch):
    from types import SimpleNamespace
    service, directory, target, initial = await real_fixture(tmp_path)
    identity = str(uuid.uuid4())
    update_receipt(directory, interactive_lifecycle_id=identity, interactive_cleanup=None)
    async def stopped_without_proof(*args, **kwargs):
        update_receipt(directory, status="stopped", interactive_lifecycle_id=None, interactive_cleanup=None)
        return SimpleNamespace(pid=123, returncode=0, wait=AsyncMock(return_value=0))
    monkeypatch.setattr(asyncio, "create_subprocess_exec", stopped_without_proof)
    await service._run(directory, "start")
    assert read_receipt(directory)["status"] == "interrupted" and not service.tasks and not service.processes
    with pytest.raises(DraftConflict, match="cleanup is unconfirmed"):
        await service.action(service.guide.identity, initial["deployment_id"], "restart")


async def bound_failed(tmp_path):
    service, directory, target, initial = await prepared(tmp_path)
    identity = str(uuid.uuid4())
    receipt = update_receipt(directory, status="failed", interactive_lifecycle_id=identity)
    binding = {key: receipt.get(key) for key in ("deployment_id", "guide_id", "build_id", "guide_version",
        "blueprint_hash", "destination", "package_sha256", "startup_attempt_id")}
    cleanup = {**binding, "lifecycle_id": identity, "confirmed": True, "source_digest": "a" * 64,
        "boundary": "owned-process-groups-absent",
        "owner_sha256": hashlib.sha256((target / OWNER_FILE).read_bytes()).hexdigest(),
        "supervisor_pid": 99999999, "return_code": 1, "worker_exit_confirmed": True}
    update_receipt(directory, interactive_cleanup=cleanup, interactive_cleanup_attempt_id=receipt.get("startup_attempt_id"))
    return service, directory, target, initial, cleanup


@pytest.mark.parametrize("field,value", [
    ("lifecycle_id", str(uuid.uuid4())), ("build_id", str(uuid.uuid4())), ("package_sha256", "b" * 64),
    ("source_digest", "b" * 64), ("owner_sha256", "b" * 64), ("startup_attempt_id", "different-attempt"),
    ("destination", "/another-owned-folder"), ("supervisor_pid", True), ("return_code", -9), ("confirmed", False)])
async def test_stale_or_nonpositive_cleanup_marker_is_not_recovery_authority(tmp_path, field, value):
    service, directory, target, initial, cleanup = await bound_failed(tmp_path)
    update_receipt(directory, interactive_cleanup={**cleanup, field: value})
    before = (target / ".env").read_bytes()
    with pytest.raises(DraftConflict):
        await service.action(service.guide.identity, initial["deployment_id"], "stop")
    assert read_receipt(directory)["status"] == "failed"
    assert (target / ".env").read_bytes() == before


@pytest.mark.parametrize("change", ["source", "owner", "package", "lease", "service"])
async def test_confirmed_cleanup_cannot_bypass_installed_source_lease_or_service_ownership(tmp_path, change):
    service, directory, target, initial, _ = await bound_failed(tmp_path)
    if change == "source": (target / "app.py").write_text("edited source")
    if change == "owner": (target / OWNER_FILE).write_text("{}")
    if change == "package": (directory / "package.zip").write_bytes(b"changed")
    if change == "service": update_receipt(directory, service={"manager": "foreign"})
    before = (target / ".env").read_bytes()
    def lock():
        return project_lock(target) if change == "lease" else deployment.receipt_lock(directory)
    # Use no held receipt journal lock around awaited service methods.
    if change == "lease":
        with lock(), pytest.raises((DraftConflict, ValueError)):
            await service.action(service.guide.identity, initial["deployment_id"], "stop")
    else:
        with pytest.raises((DraftConflict, ValueError)):
            await service._stop_failed_interactive(service.guide.identity, initial["deployment_id"])
    assert read_receipt(directory)["status"] == "failed" and (target / ".env").read_bytes() == before


async def test_selected_installation_change_while_waiting_cleanup_does_not_publish_stopped(tmp_path):
    service, directory, _, initial, _ = await bound_failed(tmp_path)
    release = asyncio.Event()
    task = asyncio.create_task(release.wait())
    service.tasks[initial["deployment_id"]] = task
    recovery = asyncio.create_task(service._stop_failed_interactive(service.guide.identity, initial["deployment_id"]))
    await asyncio.sleep(0)
    (directory.parent / "latest.json").write_text(json.dumps({"deployment_id": str(uuid.uuid4())}))
    release.set()
    try:
        with pytest.raises(DraftConflict, match="different installation"):
            await recovery
        assert read_receipt(directory)["status"] == "failed"
    finally:
        service.tasks.clear()
        await task


async def test_force_killed_original_worker_cannot_reuse_a_positive_marker(tmp_path):
    from types import SimpleNamespace
    service, directory, _, initial, cleanup = await bound_failed(tmp_path)
    service.processes[initial["deployment_id"]] = SimpleNamespace(
        pid=cleanup["supervisor_pid"], returncode=-signal.SIGKILL,
        wait=AsyncMock(return_value=-signal.SIGKILL))
    try:
        with pytest.raises(DraftConflict, match="cleanup is unconfirmed"):
            await service.action(service.guide.identity, initial["deployment_id"], "stop")
        assert read_receipt(directory)["status"] == "failed"
    finally:
        service.processes.clear()


@pytest.mark.parametrize("status", ["stopped", "failed", "interrupted"])
async def test_worker_killed_after_marker_cannot_unlock_recovery_after_handle_is_gone(tmp_path, monkeypatch, status):
    from types import SimpleNamespace
    service, directory, _, initial, cleanup = await bound_failed(tmp_path)
    update_receipt(directory, status=status)
    process = SimpleNamespace(pid=cleanup["supervisor_pid"], returncode=-signal.SIGKILL,
        wait=AsyncMock(return_value=-signal.SIGKILL))
    monkeypatch.setattr(asyncio, "create_subprocess_exec", AsyncMock(return_value=process))
    await DeploymentService._run(service, directory, "start")
    assert read_receipt(directory)["status"] == "interrupted"
    assert read_receipt(directory)["interactive_cleanup"] is None
    assert not service.processes and not service.tasks
    assert (await service.status(service.guide.identity))["cleanup_recovery"]["available"] is False
    with pytest.raises(DraftConflict, match="Stop this installed"):
        await service.settings_options(service.guide.identity, initial["deployment_id"])
    for action in ("stop", "retry", "restart"):
        with pytest.raises(DraftConflict, match="cleanup is unconfirmed"):
            await service.action(service.guide.identity, initial["deployment_id"], action)


@pytest.mark.parametrize("boundary", ["provisional", "live-wrapper", "unobservable-wrapper"])
async def test_fresh_host_requires_actual_wrapper_exit_and_absence(tmp_path, monkeypatch, boundary):
    service, directory, _, initial, cleanup = await bound_failed(tmp_path)
    if boundary == "provisional":
        update_receipt(directory, interactive_cleanup={**cleanup, "worker_exit_confirmed": False})
    elif boundary == "live-wrapper":
        update_receipt(directory, interactive_cleanup={**cleanup, "supervisor_pid": os.getpid()})
    else:
        def inaccessible(*_): raise PermissionError("fixture private process observation")
        monkeypatch.setattr(deployment.os, "kill", inaccessible)
    assert (await service.status(service.guide.identity))["cleanup_recovery"]["available"] is False
    for action in ("stop", "retry", "restart"):
        with pytest.raises(DraftConflict):
            await service.action(service.guide.identity, initial["deployment_id"], action)
    assert read_receipt(directory)["status"] == "failed"
    update_receipt(directory, status="stopped")
    with pytest.raises(DraftConflict):
        await service.settings_options(service.guide.identity, initial["deployment_id"])
    assert "fixture private process observation" not in json.dumps(await service.status(service.guide.identity))


async def test_completion_storage_failure_keeps_provisional_proof_and_releases_finished_bookkeeping(tmp_path, monkeypatch):
    from types import SimpleNamespace
    service, directory, _, initial, cleanup = await bound_failed(tmp_path)
    update_receipt(directory, interactive_cleanup={**cleanup, "worker_exit_confirmed": False})
    process = SimpleNamespace(pid=cleanup["supervisor_pid"], returncode=1, wait=AsyncMock(return_value=1))
    monkeypatch.setattr(asyncio, "create_subprocess_exec", AsyncMock(return_value=process))
    original = deployment.update_receipt
    def failed_completion(root, **changes):
        if changes.get("interactive_cleanup", {}).get("worker_exit_confirmed") is True:
            raise OSError("fixture private storage failure")
        return original(root, **changes)
    monkeypatch.setattr(deployment, "update_receipt", failed_completion)
    with pytest.raises(OSError, match="fixture private storage failure"):
        await DeploymentService._run(service, directory, "start")
    assert not service.tasks and not service.processes
    assert read_receipt(directory)["interactive_cleanup"]["worker_exit_confirmed"] is False
    assert (await service.status(service.guide.identity))["cleanup_recovery"]["available"] is False
    with pytest.raises(DraftConflict, match="cleanup is unconfirmed"):
        await service.action(service.guide.identity, initial["deployment_id"], "stop")


@pytest.mark.parametrize("mode", ["update", "rollback", "recover-current"])
async def test_transaction_worker_admission_discards_old_interactive_completion_proof(tmp_path, mode):
    service, directory, _, _, cleanup = await bound_failed(tmp_path)
    update_receipt(directory, status="queued")
    admitted = asyncio.Event()
    async def paused(*_):
        admitted.set()
        await asyncio.Event().wait()
    service._run = paused
    service._launch(directory, mode)
    await admitted.wait()
    receipt = read_receipt(directory)
    assert receipt["interactive_lifecycle_id"] != cleanup["lifecycle_id"]
    assert str(uuid.UUID(receipt["interactive_lifecycle_id"])) == receipt["interactive_lifecycle_id"]
    assert receipt["interactive_cleanup_attempt_id"] is None and receipt["interactive_cleanup"] is None
    assert service.worker_admissions[directory.name] == {
        "lifecycle_id": receipt["interactive_lifecycle_id"], "spawn_started": False, "mode": mode}
    await service.shutdown()


async def test_later_no_child_teardown_does_not_mask_an_unconfirmed_captured_group(tmp_path, monkeypatch):
    _, directory, _, _, _ = await bound_failed(tmp_path)
    update_receipt(directory, interactive_cleanup=None)
    original = deployment_worker.Supervisor
    async def incomplete_terminate(self, *args, **kwargs):
        self.child = None
    async def observed_failure(_):
        raise ValueError("owned group remains")
    async def swallowed(root, mode):
        from types import SimpleNamespace
        supervisor = deployment_worker.Supervisor(root)
        supervisor.child = SimpleNamespace(pid=123, returncode=0)
        try:
            await supervisor.terminate()
        except ValueError:
            await supervisor.terminate()
            return None
    monkeypatch.setattr(original, "terminate", incomplete_terminate)
    monkeypatch.setattr(deployment, "require_owned_group_absent", observed_failure)
    monkeypatch.setattr(deployment_worker, "run", swallowed)
    receipt = read_receipt(directory)
    with pytest.raises(ValueError, match="uncancelled cleanup"):
        await deployment.interactive_worker(directory, "start", receipt["interactive_lifecycle_id"])
    assert read_receipt(directory)["interactive_cleanup"] is None


@pytest.mark.parametrize("failure", [RuntimeError("cleanup failed"), asyncio.CancelledError()])
async def test_wrapper_cleanup_exception_or_cancellation_never_publishes_confirmation(tmp_path, monkeypatch, failure):
    _, directory, _, _, _ = await bound_failed(tmp_path)
    update_receipt(directory, interactive_cleanup=None)
    receipt = read_receipt(directory)
    monkeypatch.setattr(deployment_worker, "run", AsyncMock(side_effect=failure))
    with pytest.raises(type(failure)):
        await deployment.interactive_worker(directory, "start", receipt["interactive_lifecycle_id"])
    assert read_receipt(directory)["interactive_cleanup"] is None


async def test_swallowed_external_cancellation_is_not_a_positive_cleanup_marker(tmp_path, monkeypatch):
    _, directory, target, _, _ = await bound_failed(tmp_path)
    update_receipt(directory, interactive_cleanup=None)
    admitted = asyncio.Event()
    async def swallowed(root, mode):
        supervisor = deployment_worker.Supervisor(root)
        admitted.set()
        try:
            await asyncio.Event().wait()
        except asyncio.CancelledError:
            await supervisor.terminate()
            return None
    monkeypatch.setattr(deployment_worker, "run", swallowed)
    receipt = read_receipt(directory)
    task = asyncio.create_task(deployment.interactive_worker(directory, "start", receipt["interactive_lifecycle_id"]))
    await admitted.wait()
    task.cancel()
    with pytest.raises(ValueError, match="uncancelled cleanup"):
        await task
    assert read_receipt(directory)["interactive_cleanup"] is None


async def test_real_exited_parent_with_living_grandchild_is_not_group_cleanup(tmp_path):
    child_file = tmp_path / "grandchild-pid"
    script = ("import os,subprocess,sys,time\n"
              "child=subprocess.Popen([sys.executable,'-c','import time; time.sleep(60)'])\n"
              f"open({str(child_file)!r},'w').write(str(child.pid))\n")
    process = await asyncio.create_subprocess_exec(sys.executable, "-c", script, start_new_session=True)
    try:
        await process.wait()
        grandchild = int(child_file.read_text())
        assert process.returncode == 0 and not absent(grandchild)
        with pytest.raises(ValueError, match="group has not confirmed exit"):
            await deployment.require_owned_group_absent(process, timeout=0.05)
    finally:
        try: os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError: pass
        await process.wait()


async def test_group_observation_permission_error_never_confirms_cleanup(monkeypatch):
    from types import SimpleNamespace
    monkeypatch.setattr(deployment.os, "killpg", lambda *_: (_ for _ in ()).throw(PermissionError()))
    with pytest.raises(ValueError, match="could not be observed"):
        await deployment.require_owned_group_absent(SimpleNamespace(pid=123, returncode=0))
