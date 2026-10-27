import asyncio
from copy import deepcopy
import json
import os
from pathlib import Path
import plistlib
import sys
import time
import uuid

import pytest

from launchkit.deployment import DeploymentService, materialize, now, read_receipt, update_receipt
from launchkit.deployment_service import (UserServiceManager, background_status, digest,
    prepare_service, service_plan, stop_background, unit_bytes, verify_service)
from launchkit.deployment_worker import console_health_headers, run
from launchkit.projects import DraftConflict
from launchkit.deployment_deadlines import USER_SERVICE_STOP_TIMEOUT
from test_deployment import Guide, Implementation, Service, submit


class Manager:
    def __init__(self):
        self.observation = {"loaded": False, "pid": None, "state": "stopped"}
        self.calls = []
        self.cleanup = None

    async def capability(self):
        return {"available": True, "manager": "launchd", "persistence": "After login"}

    async def observe(self, service):
        self.calls.append(("observe", service["label"]))
        return deepcopy(self.observation)

    async def start(self, service):
        self.calls.append(("start", service["label"]))
        self.observation = {"loaded": True, "pid": 4567, "state": "running"}
        return deepcopy(self.observation)

    async def stop(self, service):
        self.calls.append(("stop", service["label"]))
        self.observation = {"loaded": False, "pid": None, "state": "stopped"}
        if self.cleanup:
            update_receipt(self.cleanup, service_cleanup_confirmed=True, runtime_pid=None, process_live=False)
        return deepcopy(self.observation)


@pytest.fixture
async def owned(tmp_path, monkeypatch):
    # Unit files exist only under the test's isolated HOME. No OS manager runs.
    monkeypatch.setenv("HOME", str(tmp_path / "isolated-home"))
    Path.home().mkdir()
    guide = Guide()
    manager = Manager()
    service = Service(tmp_path / "private", guide, Implementation(guide), service_manager=manager)
    receipt = await submit(service, tmp_path / "application", execution_mode="background")
    directory = service._directory(guide.identity)
    materialize(directory)
    service.tasks[receipt["deployment_id"]].cancel()
    await asyncio.gather(*service.tasks.values(), return_exceptions=True)
    service.tasks.clear()
    plan = prepare_service(directory, read_receipt(directory))
    yield service, manager, directory, plan
    await service.shutdown()


@pytest.mark.parametrize("stale", [False, True])
async def test_background_observation_preserves_owned_pending_admission(owned, stale):
    from test_admission_health import pending_payload
    _, manager, directory, _ = owned
    payload = pending_payload()
    attempt = str(uuid.uuid4())
    manager.observation = {"loaded": True, "pid": 4567, "state": "running"}
    update_receipt(directory, status="starting", service_supervisor_pid=4567,
        runtime_pid=6789, process_live=True, startup_attempt_id=attempt,
        bootstrap=payload["bootstrap"], health="pending", health_checked_at="2000-01-01T00:00:00+00:00" if stale else now(),
        console_url=None, admission_health={"attempt_id": attempt, **payload["admission"]})
    before = read_receipt(directory)
    observed = await background_status(directory, manager)
    if stale:
        assert observed["health"] != "pending" and observed["process_live"] is False
        assert observed["console_url"] is None
        assert observed["bootstrap"] == before["bootstrap"]
        return
    assert observed["status"] == "starting" and observed["health"] == "pending"
    assert observed["process_live"] is True and observed["console_url"] is None
    assert observed["health_checked_at"] == before["health_checked_at"], "Manager liveness cannot freshen native evidence"
    assert observed["bootstrap"] == before["bootstrap"] and observed["admission_health"] == before["admission_health"]
    manager.observation["pid"] = 9876
    foreign = await background_status(directory, manager)
    assert foreign["health"] != "pending" and foreign["process_live"] is False
    assert not any(call[0] in {"start", "stop"} for call in manager.calls)


def test_definitions_quote_paths_keep_secrets_out_and_never_enable_linger(tmp_path):
    identity = str(uuid.uuid4())
    special = tmp_path / 'folder "quoted" with % text'
    mac = service_plan(identity, special, platform="darwin", home=tmp_path)
    value = plistlib.loads(unit_bytes(mac))
    assert value["ProgramArguments"] == [sys.executable, "-I", str(special / "service-host/launcher.py"), str(special)]
    assert value["RunAtLoad"] and value["KeepAlive"] == {"SuccessfulExit": False}
    assert not {"OPENAI_API_KEY", "PYTHONPATH", "HTTPS_PROXY"}.intersection(value["EnvironmentVariables"])
    linux = unit_bytes(service_plan(identity, special, platform="linux", home=tmp_path)).decode()
    assert "%%" in linux and '\\"quoted\\"' in linux
    assert value["ExitTimeOut"] == USER_SERVICE_STOP_TIMEOUT
    assert "KillMode=mixed" in linux and f"TimeoutStopSec={USER_SERVICE_STOP_TIMEOUT}" in linux
    assert not any(word in linux for word in ["sudo", "linger", "OPENAI_API_KEY", "PYTHONPATH"])


async def test_prepared_control_snapshot_is_private_source_bound_and_manager_not_touched(owned):
    service, manager, directory, plan = owned
    assert not any(call[0] in {"start", "stop"} for call in manager.calls)
    assert verify_service(directory, read_receipt(directory))["unit_sha256"] == digest(Path(plan["unit_path"]).read_bytes())
    assert all((directory / "service-host" / name).stat().st_mode & 0o777 == 0o600 for name in plan["host_files"])
    assert "private-api-key-value" not in json.dumps(plan)
    assert "private-api-key-value" not in Path(plan["unit_path"]).read_text()
    assert "sys.dont_write_bytecode = True" in (directory / "service-host/launcher.py").read_text()


async def test_forge_recognizes_the_recorded_pre_rename_systemd_unit(owned, monkeypatch):
    import launchkit.deployment_service as module
    _, manager, directory, retained = owned
    receipt = read_receipt(directory)
    linux = service_plan(receipt["deployment_id"], directory, platform="linux", home=Path.home())
    rendered = unit_bytes(linux)
    previous = rendered.replace(b"Description=Meta Forge owned deployment",
                                b"Description=MobKit Launch owned deployment")
    path = Path(linux["unit_path"])
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(previous)
    path.chmod(0o600)
    update_receipt(directory, service={**retained, **linux, "unit_sha256": digest(previous)})
    original_plan = module.service_plan
    monkeypatch.setattr(module, "service_plan", lambda identity, root:
                        original_plan(identity, root, platform="linux", home=Path.home()))

    checked = verify_service(directory, read_receipt(directory))
    assert checked["unit_sha256"] == digest(previous)
    assert path.read_bytes() == previous
    assert not any(call[0] in {"start", "stop"} for call in manager.calls)


async def test_changed_unit_or_snapshot_refuses_any_manager_control(owned):
    service, manager, directory, plan = owned
    path = Path(plan["unit_path"])
    original = path.read_bytes()
    path.write_bytes(original + b"tampered")
    with pytest.raises(DraftConflict, match="definition changed"):
        await service.action(service.guide.identity, directory.name, "stop")
    assert not any(call[0] in {"start", "stop"} for call in manager.calls)
    path.write_bytes(original)
    (directory / "service-host/launcher.py").write_text("raise SystemExit(0)\n")
    with pytest.raises(DraftConflict, match="source changed"):
        verify_service(directory, read_receipt(directory))


async def test_links_and_extra_snapshot_code_are_not_owned(owned, tmp_path):
    _, _, directory, plan = owned
    file = directory / "service-host/launchkit/deployment_worker.py"
    original = file.read_bytes()
    other = tmp_path / "foreign.py"
    other.write_bytes(original)
    other.chmod(0o600)
    file.unlink()
    file.symlink_to(other)
    with pytest.raises(DraftConflict, match="links"):
        verify_service(directory, read_receipt(directory))
    file.unlink()
    file.write_bytes(original)
    file.chmod(0o600)
    (directory / "service-host/launchkit/injected.py").write_text("print('extra')")
    with pytest.raises(DraftConflict, match="files changed"):
        verify_service(directory, read_receipt(directory))


async def test_reconnect_requires_actual_manager_pid_and_actual_native_health(owned, monkeypatch):
    service, manager, directory, plan = owned
    manager.observation = {"loaded": True, "pid": 4567, "state": "running"}
    bootstrap = {"complete": True, "ready": True, "counts": {"broken": 0}, "identities": {"lead:main": {}}}
    update_receipt(directory, status="running", service_supervisor_pid=4567, runtime_pid=9000,
                   bootstrap=bootstrap, process_live=True, console_url="http://127.0.0.1:18088/console")
    class Response:
        def raise_for_status(self):
            pass
        def json(self):
            return {"identities": [{"identity": "lead:main"}]}
    class Client:
        def __init__(self, **kwargs):
            assert kwargs["trust_env"] is False
        async def __aenter__(self):
            return self
        async def __aexit__(self, *args):
            pass
        async def get(self, url, **kwargs):
            assert url == "http://127.0.0.1:18088/console/identities"
            return Response()
    import httpx
    monkeypatch.setattr(httpx, "AsyncClient", Client)
    # A new Launch instance has no in-memory task/process handles.
    reopened = DeploymentService(service.root.parent, service.guide, service.implementation, service_manager=manager)
    recovered = await reopened.status(service.guide.identity)
    assert recovered["status"] == "running" and recovered["health"] == "ready"
    assert recovered["service"]["public_summary"] == {"manager": plan["manager"], "state": "running",
        "pid": 4567, "definition_verified": True, "persistence": plan["persistence"]}
    manager.observation["pid"] = 4568
    unbound = await reopened.status(service.guide.identity)
    assert unbound["status"] == "starting" and unbound["console_url"] is None and not unbound["process_live"]
    assert unbound["bootstrap"] is None


@pytest.mark.parametrize("changed", ["status", "runtime_pid"])
async def test_status_refuses_stale_support_after_late_canonical_runtime_change(owned, monkeypatch, changed):
    from launchkit import deployment_supervisor
    service, manager, directory, _ = owned
    update_receipt(directory, status="stopped", service_supervisor_pid=None,
        service_cleanup_confirmed=True, runtime_pid=None, process_live=False, bootstrap=None)
    support_projection = deployment_supervisor.support_projection
    published = []

    async def changed_after_support(*args, **kwargs):
        support = await support_projection(*args, **kwargs)
        update_receipt(directory, **{changed: "failed" if changed == "status" else 9001})
        published.append((directory / "receipt.json").read_bytes())
        return support

    async def forbidden(*args, **kwargs):
        pytest.fail("Reading status cannot start an OS service or application process.")

    monkeypatch.setattr(deployment_supervisor, "support_projection", changed_after_support)
    monkeypatch.setattr(asyncio, "create_subprocess_exec", forbidden)
    reopened = DeploymentService(service.root.parent, service.guide, service.implementation,
        service_manager=manager)
    result = await reopened.status(service.guide.identity)
    assert len(published) == 1 and (directory / "receipt.json").read_bytes() == published[0]
    assert result[changed] == ("failed" if changed == "status" else 9001)
    assert result["current"] is False and result["current_guide_version"] is None
    assert result["health"] == "unknown" and result["console_url"] is None and not result["process_live"]
    support = result["support_request"]
    assert not any(support[key] for key in ("available", "prepare_available", "apply_available", "stopped_verified"))
    assert support["selected"] is None and support["phase"] == "unavailable"
    assert support["reason"] == "The installation changed while checking its support. Check its current state again."
    assert result["update_capabilities"]["background_activation_supported"] is False
    assert not any(call[0] in {"start", "stop"} for call in manager.calls)


@pytest.mark.parametrize("boundary,changed", [
    ("health", "status"), ("health", "runtime_pid"),
    ("health", "startup_attempt_id"), ("health", "bootstrap"),
    ("health", "admission_health"),
    ("manager", "startup_attempt_id"), ("manager-error", "status"),
    ("record", "status"),
    ("consumer", "status"), ("consumer", "runtime_pid"),
    ("support", "bootstrap"), ("consumer-join", "bootstrap"),
])
async def test_background_health_rejects_late_worker_state_without_overwriting_it(
        owned, monkeypatch, boundary, changed):
    from launchkit import deployment_service as module
    import httpx
    service, manager, directory, _ = owned
    manager.observation = {"loaded": True, "pid": 4567, "state": "running"}
    bootstrap = {"complete": True, "ready": True, "counts": {"broken": 0}, "identities": {"lead:main": {}}}
    update_receipt(directory, status="running", service_supervisor_pid=4567,
        service_instance_id="original-service-owner", startup_attempt_id=str(uuid.uuid4()),
        runtime_pid=9000, bootstrap=bootstrap, detail="Original native startup passed.")
    replacements = {"status": "failed", "runtime_pid": 9001,
        "startup_attempt_id": str(uuid.uuid4()),
        "admission_health": {"admitting": ["project:one"], "owed": []},
        "bootstrap": {**bootstrap, "ready": False, "counts": {"broken": 1}}}
    published, probes = [], []

    def worker_update():
        update_receipt(directory, **{changed: replacements[changed]}, detail="Newer worker evidence.")
        published.append((directory / "receipt.json").read_bytes())

    class Response:
        def raise_for_status(self):
            pass
        def json(self):
            return {"identities": [{"identity": "lead:main"}]}

    class Client:
        def __init__(self, **kwargs):
            assert kwargs["trust_env"] is False
        async def __aenter__(self):
            return self
        async def __aexit__(self, *args):
            pass
        async def get(self, url, **kwargs):
            probes.append(url)
            if boundary in {"health", "consumer"}:
                worker_update()
            return Response()

    monkeypatch.setattr(httpx, "AsyncClient", Client)
    if boundary in {"manager", "manager-error"}:
        observe = manager.observe
        async def changed_during_manager(service):
            worker_update()
            observation = await observe(service)
            if boundary == "manager-error":
                raise RuntimeError("Older manager query failed.")
            return observation
        manager.observe = changed_during_manager
    elif boundary == "record":
        record = module._record_service_observation
        def changed_before_record(*args, **kwargs):
            worker_update()
            return record(*args, **kwargs)
        monkeypatch.setattr(module, "_record_service_observation", changed_before_record)

    elif boundary == "support":
        from launchkit import deployment_supervisor
        project = deployment_supervisor.support_projection
        async def changed_after_support(*args, **kwargs):
            result = await project(*args, **kwargs)
            worker_update()
            return result
        monkeypatch.setattr(deployment_supervisor, "support_projection", changed_after_support)
    elif boundary == "consumer-join":
        observe = module.background_status
        async def changed_before_consumer_join(*args, **kwargs):
            result = await observe(*args, **kwargs)
            worker_update()
            return result
        monkeypatch.setattr(module, "background_status", changed_before_consumer_join)

    if boundary in {"consumer", "support", "consumer-join"}:
        reopened = DeploymentService(service.root.parent, service.guide, service.implementation, service_manager=manager)
        result = await reopened.status(service.guide.identity)
    else:
        result = await background_status(directory, manager)
    assert len(published) == 1
    assert result["health"] == "unknown" and result["console_url"] is None
    assert result["process_live"] is False
    assert result[changed] == replacements[changed]
    assert result["detail"] == "Newer worker evidence."
    assert (directory / "receipt.json").read_bytes() == published[0], (
        "An old observation cannot publish metadata over a newer canonical worker transition.")
    assert len(probes) == (0 if boundary in {"manager", "manager-error"} else 1)
    assert all(call[0] == "observe" for call in manager.calls)


async def test_shutdown_does_not_stop_admitted_background_service(owned):
    service, manager, directory, _ = owned
    update_receipt(directory, status="running", service_supervisor_pid=4567)
    await service.shutdown()
    assert read_receipt(directory)["status"] == "running"
    assert not any(call[0] == "stop" for call in manager.calls)


async def test_manager_auto_restart_wait_does_not_hide_a_real_failed_native_attempt(owned):
    _, manager, directory, _ = owned
    manager.observation = {"loaded": True, "pid": None, "state": "waiting"}
    update_receipt(directory, status="failed", detail="The native agents failed startup.", service_cleanup_confirmed=True)
    receipt = await background_status(directory, manager)
    assert receipt["status"] == "failed" and receipt["health"] == "unavailable"
    assert receipt["detail"] == "The native agents failed startup."
    assert receipt["service"]["public_summary"]["state"] == "waiting"


@pytest.mark.parametrize("saved_status,observation,cleanup,expected,detail", [
    ("stopped", {"loaded": True, "pid": None, "state": "waiting"}, True, "starting",
        "The background service is waiting. Agent and Console startup has not been confirmed."),
    ("running", {"loaded": True, "pid": 4568, "state": "running"}, False, "starting",
        "The background service has started, but this application's startup and Console health are not confirmed."),
    ("running", {"loaded": False, "pid": None, "state": "stopped"}, False, "interrupted",
        "The background service is stopped, but cleanup of the previous application is unconfirmed. Check its saved activity before restarting."),
    ("starting", {"loaded": False, "pid": None, "state": "stopped"}, True, "stopped",
        "The background service is stopped. Installed files and saved records are preserved."),
    ("running", {"loaded": True, "pid": None, "state": "failed"}, False, "failed",
        "The background service reports a failure. Check its saved activity before restarting."),
])
async def test_projected_service_status_uses_current_detail_without_rewriting_worker_state(
        owned, saved_status, observation, cleanup, expected, detail):
    _, manager, directory, _ = owned
    manager.observation = observation
    update_receipt(directory, status=saved_status, detail="Earlier worker report from another phase.",
        service_supervisor_pid=4567, runtime_pid=6789, service_cleanup_confirmed=cleanup,
        bootstrap={"retained": "startup evidence"})
    before = read_receipt(directory)
    projected = await background_status(directory, manager)
    assert projected["status"] == expected
    assert projected["detail"] == detail
    assert projected["health"] == "unavailable" and projected["console_url"] is None
    assert projected["process_live"] is False and projected["runtime_pid"] is None
    assert projected["bootstrap"] is None
    stored = read_receipt(directory)
    for key in ("status", "detail", "runtime_pid", "service_supervisor_pid", "bootstrap", "service_cleanup_confirmed"):
        assert stored[key] == before[key], "A current status projection cannot replace the worker's lifecycle report."
    assert all(call[0] == "observe" for call in manager.calls)


async def test_restart_requires_old_native_cleanup_then_uses_exact_owned_manager(owned):
    service, manager, directory, _ = owned
    manager.observation = {"loaded": True, "pid": 4567, "state": "running"}
    attempt = str(uuid.uuid4())
    update_receipt(directory, status="running", service_supervisor_pid=4567, service_instance_id="generation-one",
                   service_cleanup_confirmed=False, startup_attempt_id=attempt,
                   startup_console_health={"attempt_id": attempt, "status": "fail", "category": "authentication", "attempts": 1})
    manager.cleanup = directory
    restarted = await service.action(service.guide.identity, directory.name, "restart")
    assert [item[0] for item in manager.calls if item[0] != "observe"] == ["stop", "start"]
    assert restarted["execution_mode"] == "background"
    assert restarted["status"] == "starting" and restarted["console_url"] is None
    assert restarted["startup_attempt_id"] is None and restarted["startup_console_health"] is None


async def test_lost_action_reply_and_status_read_do_not_repeat_background_start(owned, monkeypatch):
    service, manager, directory, _ = owned
    manager.observation = {"loaded": True, "pid": 4567, "state": "running"}
    manager.cleanup = directory
    update_receipt(directory, status="running", service_supervisor_pid=4567,
                   service_instance_id="original-owner", service_cleanup_confirmed=False)
    request_id = str(uuid.uuid4())
    # The successful POST response is lost, then an ordinary status read fails.
    await service.action(service.guide.identity, directory.name, "restart", action_request_id=request_id)
    status = service.status
    async def failed_status(*args, **kwargs):
        raise ConnectionError("offline simulated lost status response")
    monkeypatch.setattr(service, "status", failed_status)
    with pytest.raises(ConnectionError):
        await service.status(service.guide.identity)
    exact = await service.action_request(service.guide.identity, directory.name, request_id)
    assert exact == {"action_request": {"version": 1, "action_request_id": request_id,
        "guide_id": service.guide.identity, "deployment_id": directory.name, "action": "restart",
        "update_id": None, "phase": "settled", "outcome": "returned"}}
    # Dedupe does not depend on the observational GET being available.
    repeated = await service.action(service.guide.identity, directory.name, "restart", action_request_id=request_id)
    assert repeated["action_request"] == exact["action_request"]
    assert repeated["status"] == "starting"  # Host settlement is not native readiness.
    assert [item[0] for item in manager.calls if item[0] != "observe"] == ["stop", "start"]
    path = service.root / "action-requests" / (request_id + ".json")
    assert path.stat().st_mode & 0o777 == 0o600
    assert "private-api-key-value" not in path.read_text()
    monkeypatch.setattr(service, "status", status)


async def test_pending_action_is_readable_before_effect_and_another_host_cannot_replay_it(owned, monkeypatch):
    service, manager, directory, _ = owned
    manager.cleanup = directory
    request_id = str(uuid.uuid4())
    admitted, release = asyncio.Event(), asyncio.Event()
    start = manager.start
    async def held_start(plan):
        # The durable exact admission is visible before this manager effect.
        value = await service.action_request(service.guide.identity, directory.name, request_id)
        assert value["action_request"]["phase"] == "pending"
        await start(plan)
        admitted.set()
        await release.wait()
    monkeypatch.setattr(manager, "start", held_start)
    pending = asyncio.create_task(service.action(service.guide.identity, directory.name,
                                                "restart", action_request_id=request_id))
    await asyncio.wait_for(admitted.wait(), 5)
    reopened = DeploymentService(service.root.parent, service.guide, service.implementation, service_manager=manager)
    calls = list(manager.calls)
    try:
        exact = await reopened.action_request(service.guide.identity, directory.name, request_id)
        assert exact["action_request"]["phase"] == "pending"
        assert exact["action_request"]["outcome"] is None
        duplicate = await reopened.action(service.guide.identity, directory.name, "restart", action_request_id=request_id)
        assert duplicate["action_request"] == exact["action_request"]
        assert manager.calls == calls  # No status reconciliation, stop or start.
    finally:
        release.set()
        result = await pending
    assert result["action_request"]["phase"] == "settled"
    assert [item[0] for item in manager.calls if item[0] != "observe"] == ["stop", "start"]


async def test_cancelled_action_retains_cold_pending_and_cannot_be_replayed(owned, monkeypatch):
    service, manager, directory, _ = owned
    manager.cleanup = directory
    request_id = str(uuid.uuid4())
    admitted = asyncio.Event()
    start = manager.start
    async def interrupted_start(plan):
        await start(plan)
        admitted.set()
        await asyncio.Event().wait()
    monkeypatch.setattr(manager, "start", interrupted_start)
    pending = asyncio.create_task(service.action(service.guide.identity, directory.name,
                                                "restart", action_request_id=request_id))
    await asyncio.wait_for(admitted.wait(), 5)
    pending.cancel()
    with pytest.raises(asyncio.CancelledError):
        await pending
    reopened = DeploymentService(service.root.parent, service.guide, service.implementation, service_manager=manager)
    calls = list(manager.calls)
    exact = await reopened.action_request(service.guide.identity, directory.name, request_id)
    assert exact["action_request"]["phase"] == "pending" and exact["action_request"]["outcome"] is None
    duplicate = await reopened.action(service.guide.identity, directory.name, "restart", action_request_id=request_id)
    assert duplicate["action_request"] == exact["action_request"]
    assert manager.calls == calls


async def test_terminal_host_error_is_exactly_readable_without_repeating_prior_effects(owned, monkeypatch):
    service, manager, directory, _ = owned
    manager.cleanup = directory
    request_id = str(uuid.uuid4())
    status, reads = service.status, 0
    async def failed_after_start(identity):
        nonlocal reads
        reads += 1
        if reads == 2:
            raise ConnectionError("offline simulated post-start status failure")
        return await status(identity)
    monkeypatch.setattr(service, "status", failed_after_start)
    with pytest.raises(ConnectionError):
        await service.action(service.guide.identity, directory.name, "restart", action_request_id=request_id)
    exact = await service.action_request(service.guide.identity, directory.name, request_id)
    assert exact["action_request"]["phase"] == "settled" and exact["action_request"]["outcome"] == "error"
    calls = list(manager.calls)
    repeated = await service.action(service.guide.identity, directory.name, "restart", action_request_id=request_id)
    assert repeated["action_request"] == exact["action_request"]
    assert manager.calls == calls
    assert [item[0] for item in calls if item[0] != "observe"] == ["stop", "start"]


@pytest.mark.parametrize("changed", ["guide_id", "deployment_id", "action", "update_id"])
async def test_action_request_uuid_cannot_be_rebound(owned, changed):
    service, manager, directory, _ = owned
    manager.cleanup = directory
    request_id = str(uuid.uuid4())
    await service.action(service.guide.identity, directory.name, "restart", action_request_id=request_id)
    path = service.root / "action-requests" / (request_id + ".json")
    original, calls = path.read_bytes(), list(manager.calls)
    binding = {"guide_id": service.guide.identity, "deployment_id": directory.name,
               "action": "restart", "update_id": None}
    binding[changed] = "stop" if changed == "action" else str(uuid.uuid4())
    with pytest.raises(DraftConflict, match="different deployment action"):
        await service.action(**binding, action_request_id=request_id)
    assert path.read_bytes() == original and manager.calls == calls
    if changed in {"guide_id", "deployment_id"}:
        with pytest.raises(DraftConflict, match="different deployment action"):
            await service.action_request(binding["guide_id"], binding["deployment_id"], request_id)


@pytest.mark.parametrize("failed_phase", ["pending", "settled"])
async def test_action_request_storage_failure_never_claims_settlement_or_replays(owned, monkeypatch, failed_phase):
    import launchkit.deployment as module
    service, manager, directory, _ = owned
    manager.cleanup = directory
    request_id = str(uuid.uuid4())
    write = module.write_private
    def failed_write(path, value):
        if Path(path).parent.name == "action-requests" and value.get("phase") == failed_phase:
            raise OSError("offline simulated action receipt storage failure")
        write(path, value)
    monkeypatch.setattr(module, "write_private", failed_write)
    with pytest.raises(OSError, match="receipt storage failure"):
        await service.action(service.guide.identity, directory.name, "restart", action_request_id=request_id)
    effects = [item[0] for item in manager.calls if item[0] != "observe"]
    if failed_phase == "pending":
        assert effects == []
        with pytest.raises(LookupError, match="not been recorded"):
            await service.action_request(service.guide.identity, directory.name, request_id)
    else:
        assert effects == ["stop", "start"]
        exact = await service.action_request(service.guide.identity, directory.name, request_id)
        assert exact["action_request"]["phase"] == "pending" and exact["action_request"]["outcome"] is None
        repeated = await service.action(service.guide.identity, directory.name, "restart", action_request_id=request_id)
        assert repeated["action_request"] == exact["action_request"]
        assert [item[0] for item in manager.calls if item[0] != "observe"] == effects


async def test_exact_action_readback_is_read_only_for_missing_unknown_and_damaged_metadata(owned):
    service, manager, directory, _ = owned
    request_id = str(uuid.uuid4())
    receipt, calls = (directory / "receipt.json").read_bytes(), list(manager.calls)
    with pytest.raises(LookupError, match="not been recorded"):
        await service.action_request(service.guide.identity, directory.name, request_id)
    parent = service.root / "action-requests"
    assert not parent.exists()
    parent.mkdir()
    path = parent / (request_id + ".json")
    value = {"version": 1, "action_request_id": request_id, "guide_id": service.guide.identity,
             "deployment_id": directory.name, "action": "restart", "update_id": None,
             "phase": "unknown", "outcome": None}
    path.write_text(json.dumps(value))
    original = path.read_bytes()
    assert await service.action_request(service.guide.identity, directory.name, request_id) == {"action_request": value}
    assert path.read_bytes() == original
    duplicate = await service.action(service.guide.identity, directory.name, "restart", action_request_id=request_id)
    assert duplicate["action_request"] == value
    assert manager.calls == calls and (directory / "receipt.json").read_bytes() == receipt
    path.write_text("damaged action metadata")
    damaged = path.read_bytes()
    with pytest.raises(DraftConflict, match="effects remain uncertain"):
        await service.action(service.guide.identity, directory.name, "restart", action_request_id=request_id)
    assert path.read_bytes() == damaged
    assert (directory / "receipt.json").read_bytes() == receipt and manager.calls == calls


async def test_manager_owned_automatic_restart_clears_old_health_before_private_config_failure(owned, monkeypatch):
    _, _, directory, _ = owned
    attempt = str(uuid.uuid4())
    receipt = read_receipt(directory)
    update_receipt(directory, phase="start", startup_attempt_id=attempt,
        startup_console_health={"attempt_id": attempt, "status": "fail", "category": "authentication", "attempts": 1})
    (Path(receipt["destination"]) / ".env").write_text("invalid private configuration\n")

    async def observe(self, service):
        return {"loaded": True, "pid": os.getpid(), "state": "running"}

    monkeypatch.setattr(UserServiceManager, "observe", observe)
    with pytest.raises(ValueError, match="invalid name"):
        await run(directory, "service")
    stored = read_receipt(directory)
    assert stored["status"] == "failed" and stored["phase"] == "service"
    assert stored["startup_attempt_id"] is None and stored["startup_console_health"] is None
    assert stored["runtime_pid"] is None and stored["service_cleanup_confirmed"] is True


async def test_a_new_service_lifetime_starts_without_folder_evidence(owned, monkeypatch):
    # Only reaching ready again re-records it; a lifetime that fails first has none.
    _, _, directory, _ = owned
    update_receipt(directory, service_folder_evidence={"instance_id": "an-earlier-lifetime"})
    (Path(read_receipt(directory)["destination"]) / ".env").write_text("invalid private configuration\n")

    async def observe(self, service):
        return {"loaded": True, "pid": os.getpid(), "state": "running"}

    monkeypatch.setattr(UserServiceManager, "observe", observe)
    with pytest.raises(ValueError, match="invalid name"):
        await run(directory, "service")
    stored = read_receipt(directory)
    assert stored["service_supervisor_pid"] == os.getpid() and stored["service_folder_evidence"] is None


async def test_unconfirmed_cleanup_never_allows_replacement(owned, monkeypatch):
    import launchkit.deployment_service as module
    service, manager, directory, _ = owned
    manager.observation = {"loaded": True, "pid": 4567, "state": "running"}
    update_receipt(directory, status="running", service_supervisor_pid=4567, service_instance_id="generation-one",
                   service_cleanup_confirmed=False)
    monkeypatch.setattr(module, "SERVICE_TIMEOUT", 0)
    with pytest.raises(DraftConflict, match="confirm shutdown"):
        await service.action(service.guide.identity, directory.name, "restart")
    assert not any(call[0] == "start" for call in manager.calls)
    assert read_receipt(directory)["status"] == "interrupted"


NEVER_STARTED = ("The background service is stopped. It ended before it started this application, so there was "
    "nothing to clean up. Installed files and saved records are preserved.")


def ended_pid():
    """The PID of a process that has exited and been reaped."""
    import subprocess
    process = subprocess.Popen([sys.executable, "-I", "-c", "pass"])
    process.wait()
    return process.pid


def admitted_unregistered(directory):
    # The installer's handoff before the unit's own worker registers its lifetime.
    update_receipt(directory, status="starting", phase="service", service_cleanup_confirmed=False)
    stored = read_receipt(directory)
    assert stored.get("service_supervisor_pid") is None and stored.get("service_instance_id") is None


async def test_stop_settles_a_lifetime_that_ended_before_it_registered(owned, monkeypatch):
    import launchkit.deployment_service as module
    service, manager, directory, _ = owned
    monkeypatch.setattr(module, "SERVICE_TIMEOUT", 2)
    admitted_unregistered(directory)
    # The unit's Python never got past start-up, then the stop removed it.
    manager.observation = {"loaded": True, "pid": ended_pid(), "state": "running"}
    stopped = await service.action(service.guide.identity, directory.name, "stop")
    assert (stopped["status"], stopped["detail"]) == ("stopped", NEVER_STARTED)
    stored = read_receipt(directory)
    assert stored["status"] == "stopped" and stored["service_cleanup_confirmed"] is True
    assert stored.get("service_supervisor_pid") is None and stored.get("service_instance_id") is None
    assert stored["process_live"] is False and stored["runtime_pid"] is None


async def test_stop_of_a_unit_that_reported_no_process_settles_as_before(owned, monkeypatch):
    import launchkit.deployment_service as module
    service, manager, directory, _ = owned
    monkeypatch.setattr(module, "SERVICE_TIMEOUT", 2)
    admitted_unregistered(directory)
    manager.observation = {"loaded": True, "pid": None, "state": "waiting"}

    def no_probe(pid):
        pytest.fail("A stop without a reported process needs no process check")
    monkeypatch.setattr(module, "service_process_absent", no_probe, raising=False)
    stopped = await service.action(service.guide.identity, directory.name, "stop")
    assert stopped["status"] == "stopped" and stopped["detail"] != NEVER_STARTED
    assert read_receipt(directory)["service_cleanup_confirmed"] is False


async def test_stop_after_a_restart_whose_successor_never_registered(owned, monkeypatch):
    import launchkit.deployment_service as module
    service, manager, directory, _ = owned
    monkeypatch.setattr(module, "SERVICE_TIMEOUT", 2)
    manager.observation = {"loaded": True, "pid": 4567, "state": "running"}
    update_receipt(directory, status="running", service_supervisor_pid=4567, service_instance_id="generation-one",
                   service_cleanup_confirmed=False)
    manager.cleanup = directory
    restarted = await service.action(service.guide.identity, directory.name, "restart")
    assert restarted["status"] == "starting"
    # The successor's Python stalls before it registers, so it never confirms.
    manager.cleanup = None
    manager.observation = {"loaded": True, "pid": ended_pid(), "state": "running"}
    stopped = await service.action(service.guide.identity, directory.name, "stop")
    assert stopped["status"] == "stopped"
    stored = read_receipt(directory)
    assert (stored["service_supervisor_pid"], stored["service_instance_id"]) == (4567, "generation-one")
    assert stored["service_cleanup_confirmed"] is True
    again = await service.action(service.guide.identity, directory.name, "restart")
    assert again["status"] == "starting"


async def test_stop_of_a_registered_lifetime_without_cleanup_stays_interrupted(owned, monkeypatch):
    import launchkit.deployment_service as module
    _, manager, directory, _ = owned
    monkeypatch.setattr(module, "SERVICE_TIMEOUT", 0.5)
    manager.observation = {"loaded": True, "pid": ended_pid(), "state": "running"}
    update_receipt(directory, status="running", service_supervisor_pid=manager.observation["pid"],
                   service_instance_id="generation-one", service_cleanup_confirmed=False)
    stopped = await stop_background(directory, manager)
    assert stopped["status"] == "interrupted" and stopped["service_cleanup_confirmed"] is False


async def test_stop_keeps_custody_while_the_unit_is_still_loaded(owned, monkeypatch):
    import launchkit.deployment_service as module
    _, manager, directory, _ = owned
    monkeypatch.setattr(module, "SERVICE_TIMEOUT", 0.5)
    admitted_unregistered(directory)
    manager.observation = {"loaded": True, "pid": ended_pid(), "state": "running"}

    async def stop_without_effect(service):
        manager.calls.append(("stop", service["label"]))
        manager.observation = {"loaded": True, "pid": None, "state": "waiting"}
        return deepcopy(manager.observation)
    manager.stop = stop_without_effect
    stopped = await stop_background(directory, manager)
    assert stopped["status"] == "interrupted" and stopped["service_cleanup_confirmed"] is False


async def test_stop_keeps_custody_when_a_registration_appears_during_stop(owned, monkeypatch):
    import launchkit.deployment_service as module
    _, manager, directory, _ = owned
    monkeypatch.setattr(module, "SERVICE_TIMEOUT", 0.5)
    admitted_unregistered(directory)
    pid = ended_pid()
    manager.observation = {"loaded": True, "pid": pid, "state": "running"}
    stop = manager.stop

    async def registered_then_stopped(service):
        update_receipt(directory, service_supervisor_pid=pid, service_instance_id=str(uuid.uuid4()),
                       service_cleanup_confirmed=False)
        return await stop(service)
    manager.stop = registered_then_stopped
    stopped = await stop_background(directory, manager)
    assert stopped["status"] == "interrupted" and stopped["service_cleanup_confirmed"] is False


async def test_stop_keeps_custody_while_a_reported_pid_is_alive(owned, monkeypatch):
    import launchkit.deployment_service as module
    _, manager, directory, _ = owned
    monkeypatch.setattr(module, "SERVICE_TIMEOUT", 0.5)
    admitted_unregistered(directory)
    # A live PID (this test process) may be the unit's or a reused one.
    manager.observation = {"loaded": True, "pid": os.getpid(), "state": "running"}
    stopped = await stop_background(directory, manager)
    assert stopped["status"] == "interrupted" and stopped["service_cleanup_confirmed"] is False


async def test_stop_keeps_custody_for_a_supervisor_without_its_frozen_lifetime_record(owned, monkeypatch):
    import launchkit.deployment_service as module
    _, manager, directory, _ = owned
    monkeypatch.setattr(module, "SERVICE_TIMEOUT", 0.5)
    admitted_unregistered(directory)
    receipt = read_receipt(directory)
    service = {key: value for key, value in receipt["service"].items() if key != "update_startup_protocol"}
    update_receipt(directory, service=service)
    monkeypatch.setattr(module, "verify_service", lambda directory, receipt: service)
    manager.observation = {"loaded": True, "pid": ended_pid(), "state": "running"}
    stopped = await stop_background(directory, manager)
    assert stopped["status"] == "interrupted" and stopped["service_cleanup_confirmed"] is False


async def test_options_are_read_only_and_background_selection_explicit(tmp_path, monkeypatch):
    monkeypatch.setenv("HOME", str(tmp_path))
    guide = Guide()
    manager = Manager()

    class StablePackage(Implementation):
        async def package(self, identity, build_id):
            # This is one unchanged accepted build. Cache its exact bytes so
            # wall-clock ZIP timestamps cannot turn it into another package.
            assert identity == self.guide.identity and build_id == self.identity
            await self.guide.get(identity)
            if not hasattr(self, "accepted_package"):
                self.accepted_package = await super().package(identity, build_id)
            return self.accepted_package

    service = Service(tmp_path / "private", guide, StablePackage(guide), service_manager=manager)
    options = await service.options(guide.identity)
    assert options["default_execution_mode"] == "interactive"
    assert [mode["id"] for mode in options["execution_modes"]] == ["interactive", "background"]
    assert not list(tmp_path.rglob("*.plist")) and not manager.calls
    receipt = await submit(service, tmp_path / "team")
    assert receipt["execution_mode"] == "interactive"
    with pytest.raises(DraftConflict, match="another lifecycle mode"):
        await submit(service, tmp_path / "team", execution_mode="background")
    retained = read_receipt(service._directory(guide.identity))
    assert retained["package_sha256"] == receipt["package_sha256"]
    assert retained["execution_mode"] == "interactive" and not manager.calls
    await service.shutdown()


async def test_service_worker_cannot_be_started_as_unmanaged_detached_process(owned, monkeypatch):
    _, _, directory, _ = owned
    async def observe(self, service):
        return {"loaded": True, "pid": os.getpid() + 100, "state": "running"}
    monkeypatch.setattr(UserServiceManager, "observe", observe)
    with pytest.raises(ValueError, match="Only the recorded"):
        await run(directory, "service")
    assert read_receipt(directory).get("service_supervisor_pid") is None


async def test_manager_interprets_actual_launchd_paths_and_systemd_states(owned):
    class Queries(UserServiceManager):
        def __init__(self, responses):
            self.responses = iter(responses)
            self.arguments = []
            self.options = []
        async def command(self, arguments, **kwargs):
            self.arguments.append(arguments)
            self.options.append(kwargs)
            return next(self.responses)
    service = service_plan(str(uuid.uuid4()), "/owned", platform="darwin", home="/owned-home")
    manager = Queries([(0, "path = " + service["unit_path"] + "\nstate = running\npid = 4567\n")])
    assert (await manager.observe(service))["pid"] == 4567
    assert manager.arguments[0][-1] == service["domain"] + "/" + service["label"]
    with pytest.raises(DraftConflict, match="another definition"):
        await Queries([(0, "path = /foreign.plist\npid = 4567\n")]).observe(service)
    missing = Queries([(113, "not found"), (0, "domain exists")])
    assert not (await missing.observe(service))["loaded"]
    assert missing.options[1]["capture_output"] is False
    hostile = ("Could not print domain: 141: Reentrancy avoided\n"
               "OPENAI_API_KEY=credential-sentinel /private/hostile-manager-path <script>unsafe</script>")
    failures = [{"query": "service", "return_code": 141, "classification": "reentrancy"},
                {"query": "domain", "return_code": 113, "classification": "unknown"}]
    inaccessible = Queries([(141, hostile), (113, "")])
    with pytest.raises(RuntimeError, match="could not be queried") as error:
        await inaccessible.observe(service)
    assert inaccessible.options[1]["capture_output"] is False
    assert str(error.value) == "The user service manager could not be queried."
    assert error.value.query_failures == failures
    # Unavailable manager access is observation failure, not native terminality.
    _, _, directory, owned_service = owned
    owned_failures = failures if owned_service["manager"] == "launchd" else failures[:1]
    bootstrap = {"complete": True, "ready": True, "counts": {"broken": 0}, "identities": {"lead:main": {}}}
    update_receipt(directory, status="running", health="ready", runtime_pid=9000,
                   service_supervisor_pid=4567, service_instance_id="current-generation",
                   bootstrap=bootstrap, process_live=True)
    unavailable = await background_status(directory, Queries([(141, hostile), (113, "")]))
    assert unavailable["health"] == "unknown" and not unavailable["process_live"]
    assert unavailable["service_observation"]["query_failures"] == owned_failures
    assert unavailable["service_observation"]["owned"] is False
    assert unavailable["service"]["public_summary"]["definition_verified"] is False
    assert unavailable["detail"] == "The user service manager could not be queried."
    stored = read_receipt(directory)
    assert stored["service_observation"]["query_failures"] == owned_failures
    assert stored["status"] == "running" and stored["health"] == "ready"
    assert stored["runtime_pid"] == 9000 and stored["bootstrap"] == bootstrap
    assert stored["service_instance_id"] == "current-generation" and stored["process_live"]
    assert not any(value in json.dumps(unavailable) for value in
                   ["credential-sentinel", "/private/hostile-manager-path", "OPENAI_API_KEY", "<script>"])
    linux = service_plan(str(uuid.uuid4()), "/owned", platform="linux", home="/owned-home")
    observation = await Queries([(0, "LoadState=loaded\nActiveState=active\nSubState=running\nMainPID=4567\nFragmentPath=" + linux["unit_path"] + "\n")]).observe(linux)
    assert observation == {"loaded": True, "pid": 4567, "state": "active", "substate": "running",
                           "control_pid": None, "control_group": ""}
    with pytest.raises(RuntimeError, match="could not be queried") as error:
        await Queries([(77, "Permission denied: credential-sentinel /private/hostile-manager-path")]).observe(linux)
    assert error.value.query_failures == [{"query": "service", "return_code": 77, "classification": "permission"}]
    assert "credential-sentinel" not in str(error.value) + json.dumps(error.value.query_failures)


async def test_launchd_explicit_stop_is_disabled_across_login_then_start_enables_exact_uuid():
    class Control(UserServiceManager):
        def __init__(self):
            self.arguments = []
            self.loaded = True
        async def observe(self, service):
            return {"loaded": self.loaded, "pid": 4567 if self.loaded else None, "state": "running" if self.loaded else "stopped"}
        async def command(self, arguments, **kwargs):
            self.arguments.append(arguments)
            if arguments[1] == "bootout":
                self.loaded = False
            if arguments[1] == "bootstrap":
                self.loaded = True
            return 0, ""
    service = service_plan(str(uuid.uuid4()), "/owned", platform="darwin", home="/owned-home")
    manager = Control()
    await manager.stop(service)
    await manager.start(service)
    assert [arguments[1] for arguments in manager.arguments] == ["disable", "bootout", "enable", "bootstrap"]
    assert all(arguments[2] == service["domain"] + "/" + service["label"] for arguments in manager.arguments[:3])


async def test_slow_manager_probe_cannot_discard_new_native_bootstrap(owned):
    _, manager, directory, _ = owned
    bootstrap = {"complete": True, "ready": True, "counts": {"broken": 0}, "identities": {"lead:main": {}}}
    update_receipt(directory, status="starting", service_supervisor_pid=None, service_instance_id=None)
    async def observe(service):
        update_receipt(directory, status="running", service_supervisor_pid=4567, service_instance_id="new-generation",
                       runtime_pid=9000, bootstrap=bootstrap)
        return {"loaded": True, "pid": 4567, "state": "running"}
    manager.observe = observe
    # HTTP failure affects only projection; source startup evidence survives.
    await background_status(directory, manager)
    stored = read_receipt(directory)
    assert stored["status"] == "running" and stored["runtime_pid"] == 9000
    assert stored["bootstrap"] == bootstrap and stored["service_instance_id"] == "new-generation"


async def test_frozen_supervisor_imports_in_explicit_isolated_interpreter_without_repo_cwd(owned, tmp_path):
    _, _, directory, plan = owned
    code = ("import sys; sys.dont_write_bytecode=True; sys.path.insert(0, "
            + repr(str(directory / "service-host"))
            + "); import launchkit.deployment_worker; print(launchkit.deployment_worker.__file__)")
    process = await asyncio.create_subprocess_exec(plan["interpreter"], "-I", "-c", code, cwd=tmp_path,
        env={"PATH": "/usr/bin:/bin", "HOME": str(tmp_path), "PYTHONPATH": "/invalid/malicious"},
        stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE)
    stdout, stderr = await asyncio.wait_for(process.communicate(), 10)
    assert process.returncode == 0, stderr.decode()
    assert str(directory / "service-host/launchkit/deployment_worker.py") in stdout.decode()
    assert not list((directory / "service-host").rglob("__pycache__"))
    verify_service(directory, read_receipt(directory))


async def test_frozen_supervisor_compiler_generates_exact_review_assets_and_rejects_tampering(owned, tmp_path):
    _, manager, directory, plan = owned
    host = directory / "service-host"
    assets = ("controller_review.mjs", "controller_review.css")
    code = (
        "import sys, json, hashlib; sys.dont_write_bytecode=True; sys.path.insert(0, "
        + repr(str(host))
        + "); from launchkit import compiler; from launchkit.blueprint import starter_blueprint; "
        "files=compiler.generate_files(starter_blueprint()); "
        "assets=('controller_review.mjs','controller_review.css'); "
        "print(json.dumps({'compiler':compiler.__file__, "
        "'assets':{name:hashlib.sha256(files[name].encode()).hexdigest() for name in assets}, "
        "'checksums':files['SHA256SUMS']}))"
    )
    process = await asyncio.create_subprocess_exec(plan["interpreter"], "-I", "-c", code,
        cwd=tmp_path, env={"PATH": "/usr/bin:/bin", "HOME": str(tmp_path),
                           "PYTHONPATH": "/invalid/malicious"},
        stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE)
    stdout, stderr = await asyncio.wait_for(process.communicate(), 10)
    assert process.returncode == 0, stderr.decode()
    generated = json.loads(stdout)
    assert generated["compiler"] == str(host / "launchkit/compiler.py")
    checksums = {path: checksum for checksum, path in
                 (line.split("  ", 1) for line in generated["checksums"].splitlines())}
    for asset in assets:
        name = "launchkit/templates_v2/" + asset
        path = host / name
        content = path.read_bytes()
        assert generated["assets"][asset] == checksums[asset] == plan["host_files"][name] == digest(content)
        assert path.stat().st_mode & 0o777 == 0o600
        path.write_bytes(content + b"tampered")
        try:
            with pytest.raises(DraftConflict, match="source changed"):
                verify_service(directory, read_receipt(directory))
        finally:
            path.write_bytes(content)
    assert not list(host.rglob("__pycache__"))
    verify_service(directory, read_receipt(directory))
    assert not any(call[0] in {"start", "stop"} for call in manager.calls)


def test_authenticated_console_health_uses_host_helper_without_loading_application(tmp_path, monkeypatch):
    application = tmp_path / "application"
    application.mkdir()
    (application / "__init__.py").write_text("raise RuntimeError('Application import is forbidden in a read-only health probe')\n")
    assert console_health_headers(tmp_path) == {}
    (application / "console.json").write_text('{"authentication":"local-password"}')
    from launchkit.templates_v2 import console_auth
    def headers(root):
        assert root == tmp_path
        return {"Authorization": "Bearer private-health-token"}
    monkeypatch.setattr(console_auth, "health_headers", headers)
    assert console_health_headers(tmp_path) == {"Authorization": "Bearer private-health-token"}
    def unavailable(root):
        raise ValueError("must-not-leak-private-key")
    monkeypatch.setattr(console_auth, "health_headers", unavailable)
    with pytest.raises(ValueError, match="could not be verified") as error:
        console_health_headers(tmp_path)
    assert "must-not-leak" not in str(error.value)


async def test_service_manager_output_is_bounded_and_timeout_reaps_owned_command(monkeypatch):
    import launchkit.deployment_service as module
    manager = UserServiceManager()
    monkeypatch.setattr(module, "MAX_CONTROL_OUTPUT", 64)
    with pytest.raises(RuntimeError, match="output bound"):
        await manager.command([sys.executable, "-I", "-c", "print('x' * 65536)"])
    code, output = await manager.command([sys.executable, "-I", "-c", "print('x' * 65536)"], capture_output=False)
    assert code == 0 and output == ""
    original = asyncio.create_subprocess_exec
    admitted = []
    async def capture(*args, **kwargs):
        process = await original(*args, **kwargs)
        admitted.append(process)
        return process
    monkeypatch.setattr(asyncio, "create_subprocess_exec", capture)
    monkeypatch.setattr(module, "SERVICE_TIMEOUT", 0.05)
    with pytest.raises(TimeoutError):
        await manager.command([sys.executable, "-I", "-c", "import time; time.sleep(10)"])
    assert len(admitted) == 1 and admitted[0].returncode is not None


async def test_cancellation_during_manager_command_admission_recovers_and_stops_handle(monkeypatch):
    original = asyncio.create_subprocess_exec
    admitted = asyncio.Event()
    release = asyncio.Event()
    processes = []
    async def delayed(*args, **kwargs):
        process = await original(*args, **kwargs)
        processes.append(process)
        admitted.set()
        await release.wait()
        return process
    monkeypatch.setattr(asyncio, "create_subprocess_exec", delayed)
    task = asyncio.create_task(UserServiceManager().command([sys.executable, "-I", "-c", "import time; time.sleep(10)"]))
    await admitted.wait()
    task.cancel()
    release.set()
    with pytest.raises(asyncio.CancelledError):
        await task
    assert processes[0].returncode is not None


async def test_required_static_resource_is_exact_private_manifested_and_tamper_rejected(owned):
    _, manager, directory, plan = owned
    from launchkit import deployment_service
    name = "launchkit/blueprint-limits.json"
    source = Path(deployment_service.__file__).with_name("blueprint-limits.json")
    frozen = directory / "service-host" / name
    assert frozen.read_bytes() == source.read_bytes()
    assert plan["host_files"][name] == digest(source.read_bytes())
    assert frozen.stat().st_mode & 0o777 == 0o600
    expected_json = {"launchkit/" + resource for resource in deployment_service.STATIC_RESOURCES
                     if resource.endswith(".json")}
    assert {key for key in plan["host_files"] if key.endswith(".json")} == expected_json
    for key in expected_json:
        source_resource = Path(deployment_service.__file__).parent / key.removeprefix("launchkit/")
        frozen_resource = directory / "service-host" / key
        assert frozen_resource.read_bytes() == source_resource.read_bytes()
        assert plan["host_files"][key] == digest(source_resource.read_bytes())
        assert frozen_resource.stat().st_mode & 0o777 == 0o600
    dispatcher = "launchkit/deployment_application_startup.py"
    source_dispatcher = Path(deployment_service.__file__).with_name("deployment_application_startup.py")
    frozen_dispatcher = directory / "service-host" / dispatcher
    assert frozen_dispatcher.read_bytes() == source_dispatcher.read_bytes()
    assert plan["host_files"][dispatcher] == digest(source_dispatcher.read_bytes())
    assert frozen_dispatcher.stat().st_mode & 0o777 == 0o600
    for asset in ("console_shell.mjs", "console_shell.css"):
        source_asset = Path(deployment_service.__file__).parent / "templates_v2" / asset
        key = "launchkit/templates_v2/" + asset
        frozen_asset = directory / "service-host" / key
        assert frozen_asset.read_bytes() == source_asset.read_bytes()
        assert plan["host_files"][key] == digest(source_asset.read_bytes())
        assert frozen_asset.stat().st_mode & 0o777 == 0o600
    frozen.write_bytes(frozen.read_bytes() + b" ")
    with pytest.raises(DraftConflict, match="source changed"):
        verify_service(directory, read_receipt(directory))
    assert not any(call[0] in {"start", "stop"} for call in manager.calls)


async def test_options_unpack_leaves_the_event_loop_free(tmp_path, monkeypatch):
    import launchkit.deployment as deployment_module
    monkeypatch.setenv("HOME", str(tmp_path))
    guide = Guide()
    service = Service(tmp_path / "private", guide, Implementation(guide), service_manager=Manager())
    original = deployment_module.package_files
    def slowed(payload):
        time.sleep(1.0)  # A large package on a small host.
        return original(payload)
    monkeypatch.setattr(deployment_module, "package_files", slowed)
    stalls, done = [], asyncio.Event()
    async def other_request():
        while not done.is_set():
            started = time.monotonic()
            await asyncio.sleep(0.01)
            stalls.append(time.monotonic() - started)
    probe = asyncio.create_task(other_request())
    await asyncio.sleep(0)  # The other request is waiting on the loop.
    try:
        options = await service.options(guide.identity)
    finally:
        done.set()
        await probe
    assert options["current"] and stalls and max(stalls) < 0.5
    await service.shutdown()
