"""Offline transaction proofs with owned files and explicit fake OS/native I/O.

The transaction, source ownership, lease, frozen service definition and health
checks are production code. No operating-system service or native agent runs.
"""
import asyncio
from copy import deepcopy
import json
import os
from pathlib import Path
import uuid

import pytest

from launchkit.deployment import DeploymentService, now, read_receipt, update_receipt
from launchkit import deployment_service as services
from launchkit import deployment_updates as updates
from launchkit.deployment_worker import Redactor, Supervisor, read_settings
from launchkit.projects import DraftConflict
from launchkit.templates.storage_setup import project_lock
from test_deployment_updates import prepared


class FakeManager:
    def __init__(self, directory):
        self.directory = directory
        self.observation = {"loaded": False, "pid": None, "state": "stopped"}
        self.starts = []
        self.attempts = []
        self.stops = []
        self.behaviors = []
        self.last_behavior = None
        self.cleanup = True
        self.started = asyncio.Event()
        self.block = asyncio.Event()
        # The horizon the fresh service worker's application reports at readiness.
        self.reported_horizon = None

    async def observe(self, service):
        services.verify_service(self.directory, read_receipt(self.directory))
        return deepcopy(self.observation)

    async def start(self, service):
        receipt = read_receipt(self.directory)
        # The app's own supervisor will acquire this lease. The update
        # controller must have released its authored-check lease before handoff.
        with project_lock(Path(receipt["destination"])):
            pass
        assert services.service_binding(services.verify_service(self.directory, receipt)) == services.service_binding(service)
        self.starts.append(receipt["build_id"])
        behavior = self.behaviors.pop(0) if self.behaviors else "ready"
        self.last_behavior = behavior
        pid = 4567 + len(self.starts)
        instance = str(uuid.uuid4())
        attempt = str(uuid.uuid4())
        self.observation = {"loaded": True, "pid": pid, "state": "running"}
        bootstrap = {"complete": True, "ready": True, "counts": {"broken": 0},
                     "identities": {"lead:main": {}}}
        change = {"service_supervisor_pid": pid, "service_instance_id": instance,
                  "service_cleanup_confirmed": False, "startup_attempt_id": attempt,
                  "startup_console_health": {"attempt_id": attempt, "status": "pass"},
                  "admission_health": None,
                  "runtime_pid": 9000 + len(self.starts), "bootstrap": bootstrap,
                  "status": "running", "phase": "run",
                  "logs": [{"at": now(), "phase": "runtime", "text": "fake fresh supervisor output"}]}
        if self.reported_horizon is not None:
            # The service worker records the report and widens its own unit's
            # stop budget at readiness, while the update waits for its health.
            change["gateway_shutdown_horizon_ms"] = self.reported_horizon
            services.widen_service_stop_budget(self.directory, self.reported_horizon)
        if behavior == "reused-instance":
            change["service_instance_id"] = receipt["service_instance_id"]
        elif behavior == "reused-attempt":
            change["startup_attempt_id"] = "previous-startup"
            change["startup_console_health"] = {"attempt_id": "previous-startup", "status": "pass"}
        elif behavior == "stale-health":
            change["startup_console_health"] = {"attempt_id": "previous-startup", "status": "pass"}
        elif behavior == "failed":
            change.update(status="failed", bootstrap=None, runtime_pid=None,
                          startup_console_health={"attempt_id": attempt, "status": "fail"},
                          logs=[{"at": now(), "phase": "runtime", "text":
                                 "Fake EADDRINUSE startup diagnostic at " + receipt["destination"]
                                 + " with private-api-key-value and bearer synthetic-private-token"}])
        elif behavior == "ack-only":
            change.update(status="starting", startup_attempt_id=None, startup_console_health=None,
                          bootstrap=None, runtime_pid=None)
        elif behavior == "pending":
            from test_admission_health import pending_payload
            payload = pending_payload()
            change.update(status="starting", health="pending", process_live=True,
                          console_url=None, health_checked_at=now(), startup_console_health=None,
                          bootstrap=payload["bootstrap"],
                          admission_health={"attempt_id": attempt, **payload["admission"]})
        update_receipt(self.directory, **change)
        self.attempts.append({key: change[key] for key in ("service_instance_id", "startup_attempt_id")})
        if behavior == "failed":
            target = Path(receipt["destination"])
            (target / "data/ledger.db").write_bytes(b"real fixture records; candidate attempt")
            (target / ".mobkit/state/native.db").write_bytes(b"native fixture history; candidate attempt")
        self.started.set()
        if behavior == "block":
            await self.block.wait()
        return deepcopy(self.observation)

    async def stop(self, service):
        receipt = read_receipt(self.directory)
        self.stops.append(receipt["build_id"])
        if self.last_behavior == "failed":
            update_receipt(self.directory, logs=[*receipt["logs"], {
                "at": now(), "phase": "runtime", "text": "fake late output drained during exact attempt cleanup"}])
        self.observation = {"loaded": False, "pid": None, "state": "stopped"}
        if self.cleanup:
            update_receipt(self.directory, service_cleanup_confirmed=True,
                           runtime_pid=None, process_live=False)
        return deepcopy(self.observation)


@pytest.fixture
def unit_from_earlier_forge(monkeypatch):
    """An earlier systemd unit, independent of the test runner's operating system."""
    plan = services.service_plan
    def earlier(*args, **kwargs):
        kwargs["platform"] = "linux"
        return {**plan(*args, **kwargs), "stop_timeout_seconds": 397}
    monkeypatch.setattr(services, "service_plan", earlier)


@pytest.fixture
async def background(tmp_path, monkeypatch, request):
    monkeypatch.setenv("HOME", str(tmp_path / "isolated-home"))
    Path.home().mkdir()
    # "launchd-documents": a launchd unit whose Python is in ~/Documents.
    launchd = getattr(request, "param", None) == "launchd-documents"
    if launchd:
        plan = services.service_plan
        monkeypatch.setattr(services, "service_plan", lambda identity, root, **options:
            plan(identity, root, **{**options, "platform": "darwin", "home": Path.home()}))
    service, directory, target, old, private = await prepared(tmp_path)
    update_receipt(directory, execution_mode="background", status="stopped",
                   service_supervisor_pid=4567, service_instance_id="previous-instance",
                   service_cleanup_confirmed=True, startup_attempt_id="previous-startup")
    with monkeypatch.context() as unit:
        if launchd:
            import sys
            unit.setattr(sys, "executable", str(Path.home() / "Documents/MetaForge/.venv/bin/python"))
        definition = services.prepare_service(directory, read_receipt(directory))
    manager = FakeManager(directory)
    service.service_manager = manager
    staged = await service.stage_update(service.guide.identity, old["deployment_id"], 4,
                                       service.implementation.identity)
    identity = staged["update"]["update_id"]
    async def check(self, *args, **kwargs):
        assert args[0][-1] in {"--doctor", "--application-check"}
        with pytest.raises(RuntimeError, match="already"):
            with project_lock(self.destination):
                pass
    monkeypatch.setattr(Supervisor, "command", check)
    import httpx
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
    monkeypatch.setattr(httpx, "AsyncClient", Client)
    monkeypatch.setattr(services, "SERVICE_START_TIMEOUT", 0)
    monkeypatch.setattr(services, "SERVICE_TIMEOUT", 0)
    yield service, directory, target, old, private, identity, manager, definition
    await service.shutdown()


@pytest.fixture
def unflushed_writes(monkeypatch):
    """For rows that time the update's own budgets in fractions of a second.

    The update budget also pays for the work before the candidate starts,
    21 fsyncs among it. Under host I/O pressure that work took up to 2.1 s,
    the first pending check found a 1 s update budget already spent, and the
    row saw its own rollback instead of the pending wait. These rows check
    budget handoffs, not durability, so their writes skip the flush. Other
    rows keep real fsyncs.
    """
    monkeypatch.setattr(os, "fsync", lambda descriptor: None)


@pytest.mark.parametrize("background", ["launchd-documents"], indirect=True)
async def test_an_update_start_that_never_registers_clears_folder_evidence(background):
    # The unit reached ready in its protected folder once. An update start
    # clears that record; its Python then stalls and never registers.
    service, directory, target, old, private, identity, manager, definition = background
    receipt = read_receipt(directory)
    unit = receipt["service"]
    update_receipt(directory, service_folder_evidence={"instance_id": receipt["service_instance_id"],
        **{key: unit[key] for key in ("manager", "domain", "label", "unit_path", "interpreter", "receipt_directory")},
        "destination": receipt["destination"]})
    starts = []

    async def never_registers(started):
        starts.append(started["label"])
        raise RuntimeError("The unit's Python stalled before it registered.")
    manager.start = never_registers
    try:
        await services.update_background(directory, manager, "update", update_id=identity)
    except Exception:
        pass
    assert starts and read_receipt(directory).get("service_folder_evidence") is None
    with pytest.raises(DraftConflict) as refused:
        await service.action(service.guide.identity, directory.name, "restart")
    assert str(refused.value).startswith("macOS does not let a background login item read Desktop")


async def test_apply_and_explicit_rollback_use_fresh_service_health_without_replacing_frozen_host(background):
    service, directory, target, old, private, identity, manager, definition = background
    unit = Path(definition["unit_path"]).read_bytes()
    manifest = deepcopy(definition["host_files"])
    cap = services.background_update_capabilities(directory, read_receipt(directory))
    assert cap["background_activation_supported"] is True and cap["requires_stopped"] is True
    assert cap["deployment_id"] == old["deployment_id"] and cap["package_sha256"] == old["package_sha256"]
    await services.update_background(directory, manager, "update", update_id=identity)
    current = read_receipt(directory)
    assert current["update"]["status"] == "completed" and current["status"] == "running"
    proof = current["update_service_verification"]
    assert proof["update_id"] == identity and proof["build_id"] == service.implementation.identity
    assert proof["service_instance_id"] != "previous-instance"
    assert proof["startup_attempt_id"] != "previous-startup"
    assert proof["runtime_pid"] == current["runtime_pid"]
    assert (target / "application/logic.py").read_text() == "VERSION = 'new'\n"
    assert (target / ".env").read_bytes() == private
    (target / "data/ledger.db").write_bytes(b"records produced after the update")
    (target / ".mobkit/state/native.db").write_bytes(b"native messages produced after the update")
    # Closing Launch after a committed handoff does not stop its OS-owned unit.
    await service.shutdown()
    assert manager.stops == [] and manager.observation["pid"]
    await services.stop_background(directory, manager)
    await services.update_background(directory, manager, "rollback", update_id=identity)
    result = read_receipt(directory)
    assert result["update"]["status"] == "rolled-back" and result["build_id"] == old["build_id"]
    assert "previous verified source restarted" in result["detail"]
    assert "updated owned background service" not in result["detail"]
    assert result["update_service_verification"]["service_instance_id"] != proof["service_instance_id"]
    assert result["update_service_verification"]["startup_attempt_id"] != proof["startup_attempt_id"]
    assert manager.starts == [service.implementation.identity, old["build_id"]]
    assert (target / "data/ledger.db").read_bytes() == b"records produced after the update"
    assert (target / ".mobkit/state/native.db").read_bytes() == b"native messages produced after the update"
    assert (target / ".env").read_bytes() == private
    assert Path(definition["unit_path"]).read_bytes() == unit
    assert services.verify_service(directory, result)["host_files"] == manifest


async def test_pending_update_waits_past_both_budgets_then_commits_only_fresh_ready(background, monkeypatch, unflushed_writes):
    service, directory, target, old, private, identity, manager, _ = background
    monkeypatch.setattr(services, "SERVICE_START_TIMEOUT", 2)
    # The update budget also pays for the work before the candidate starts
    # (up to 1.8 s without flushes, with 16 runs on 4 cores).
    monkeypatch.setattr(services, "SERVICE_UPDATE_TIMEOUT", 3.5)
    manager.behaviors = ["pending"]
    task = asyncio.create_task(services.update_background(directory, manager, "update", update_id=identity))
    try:
        await asyncio.wait_for(manager.started.wait(), 30)
        pending = read_receipt(directory)
        # Pending suspends both budgets for at most the update budget's length,
        # so the start budget runs again during this wait. Its 2 s keep the
        # check and the commit clear of it.
        await asyncio.sleep(services.SERVICE_UPDATE_TIMEOUT + .15)
        assert not task.done(), "A recoverable native admission is not an update timeout"
        waiting = read_receipt(directory)
        assert waiting["status"] == "starting" and waiting["health"] == "pending"
        assert waiting["bootstrap"] == pending["bootstrap"]
        assert waiting["health_checked_at"] == pending["health_checked_at"]
        assert not waiting.get("update_service_verification") and waiting["console_url"] is None
        assert manager.stops == [] and len(manager.starts) == 1
        (target / "data/ledger.db").write_bytes(b"records retained during admission recovery")
        attempt = waiting["startup_attempt_id"]
        update_receipt(directory, status="running", admission_health=None,
            bootstrap={"complete": True, "ready": True, "counts": {"broken": 0},
                       "identities": {"lead:main": {"state": "active"}}},
            startup_console_health={"attempt_id": attempt, "status": "pass"})
        await asyncio.wait_for(task, 30)
        result = read_receipt(directory)
        assert result["update"]["status"] == "completed"
        assert result["update_service_verification"]["startup_attempt_id"] == attempt
        assert result["build_id"] == service.implementation.identity and result["health"] == "ready"
        assert (target / "data/ledger.db").read_bytes() == b"records retained during admission recovery"
        assert (target / ".env").read_bytes() == private
    finally:
        if not task.done():
            task.cancel()
        await asyncio.gather(task, return_exceptions=True)


@pytest.mark.parametrize("fault", ["reused-instance", "reused-attempt", "malformed", "failed-manager", "foreign-pid"])
async def test_invalid_pending_update_cannot_suspend_timeout(background, monkeypatch, fault):
    service, directory, _, old, _, identity, manager, _ = background
    monkeypatch.setattr(services, "SERVICE_START_TIMEOUT", .15)
    manager.behaviors = ["pending", "ready"]
    original_start = manager.start

    async def altered_start(definition):
        observed = await original_start(definition)
        if len(manager.starts) == 1:
            receipt = read_receipt(directory)
            if fault == "reused-instance":
                update_receipt(directory, service_instance_id="previous-instance")
            elif fault == "reused-attempt":
                receipt["admission_health"]["attempt_id"] = "previous-startup"
                update_receipt(directory, startup_attempt_id="previous-startup",
                               admission_health=receipt["admission_health"])
            elif fault == "malformed":
                receipt["bootstrap"]["identities"]["project:one"]["restore"]["kind"] = "ordinary-failure"
                update_receipt(directory, bootstrap=receipt["bootstrap"])
            elif fault == "failed-manager":
                manager.observation["state"] = "failed"
            else:
                manager.observation["pid"] += 999
        return observed

    monkeypatch.setattr(manager, "start", altered_start)
    await asyncio.wait_for(services.update_background(directory, manager, "update", update_id=identity), 5)
    result = read_receipt(directory)
    assert result["update"]["status"] == "rolled-back" and result["build_id"] == old["build_id"]
    assert manager.starts == [service.implementation.identity, old["build_id"]]
    assert manager.stops == [service.implementation.identity]


@pytest.mark.parametrize("condition", ["stale", "silent", "never-ready"])
async def test_pending_update_is_bounded_and_rolls_back(background, monkeypatch, condition):
    service, directory, target, old, private, identity, manager, _ = background
    monkeypatch.setattr(services, "SERVICE_START_TIMEOUT", .2)
    monkeypatch.setattr(services, "SERVICE_UPDATE_TIMEOUT", 3)
    monkeypatch.setattr(services, "SERVICE_PENDING_MAX_WAIT", 2 if condition == "silent" else .3)
    manager.behaviors = ["pending", "ready"]
    original_observe = manager.observe
    entered_at, candidate_observations = None, 0
    records = (target / "data/ledger.db").read_bytes()

    async def observe(definition):
        nonlocal entered_at, candidate_observations
        if len(manager.starts) == 1 and not manager.stops:
            candidate_observations += 1
        if len(manager.starts) == 1:
            elapsed = asyncio.get_running_loop().time()
            if entered_at is None:
                entered_at = elapsed
            fresh = condition == "never-ready" or condition == "silent" and elapsed - entered_at < .3
            update_receipt(directory, health_checked_at=(now() if fresh
                else "2000-01-01T00:00:00+00:00"))
        return await original_observe(definition)

    monkeypatch.setattr(manager, "observe", observe)
    task = asyncio.create_task(services.update_background(directory, manager, "update", update_id=identity))
    try:
        # The rows observe the transaction ending by itself: a candidate attempt
        # that its own startup bound ended, then the rollback. The wait is only a
        # hang guard. Under I/O pressure the rollback took up to 3.1 s here, so a
        # 2 s wall-clock bound failed rows that ended correctly.
        try:
            await asyncio.wait_for(asyncio.shield(task), 30)
        except TimeoutError:
            pytest.fail("Pending evidence must not hold the transaction open until external cancellation")
        result = read_receipt(directory)
        assert result["update"]["status"] == "rolled-back" and result["build_id"] == old["build_id"]
        # The candidate's own startup bound ends it, in either of its two places:
        # at the first pending check, if writes were slow enough to spend the
        # start deadline first, or after pending.
        [attempt] = result["update"]["failed_attempts"]
        assert (attempt["source"], attempt["error_type"]) == ("candidate", "TimeoutError")
        assert attempt["error"] in {
            "The update startup deadline expired before admission recovery was observed.",
            "The owned background service did not supply fresh native startup and authenticated Console health in time."}
        assert manager.starts == [service.implementation.identity, old["build_id"]]
        assert manager.stops == [service.implementation.identity]
        assert (target / ".env").read_bytes() == private
        assert (target / "data/ledger.db").read_bytes() == records
        # The cap's size, without a clock: the startup loop polls every 0.2 s and
        # pending adds at most the cap to its deadline, so the candidate gets at
        # most this many observations. Slow I/O can only lower the count.
        assert candidate_observations <= int((services.SERVICE_START_TIMEOUT + services.SERVICE_PENDING_MAX_WAIT) / .2) + 2
    finally:
        if not task.done():
            task.cancel()
        await asyncio.wait_for(asyncio.gather(task, return_exceptions=True), 3)


@pytest.mark.parametrize("after_pending", ["unknown", "cancel", "failed-then-stalled-fallback", "stalled-observation"])
async def test_leaving_pending_restores_bounds_and_exact_cleanup(background, monkeypatch, unflushed_writes, after_pending):
    service, directory, target, old, private, identity, manager, _ = background
    # The first check after the start must land inside this budget, and
    # pending must outlast it before the row leaves pending.
    monkeypatch.setattr(services, "SERVICE_START_TIMEOUT", 2)
    # The update budget also pays for the work before the candidate starts
    # (up to 1.8 s without flushes, with 16 runs on 4 cores). What is left
    # bounds the stalled rows and must carry the rest of the start budget
    # and the rollback after pending.
    monkeypatch.setattr(services, "SERVICE_UPDATE_TIMEOUT", 5)
    manager.behaviors = ["pending", "block" if after_pending == "failed-then-stalled-fallback" else "ready"]
    task = asyncio.create_task(services.update_background(directory, manager, "update", update_id=identity))
    try:
        await asyncio.wait_for(manager.started.wait(), 30)
        await asyncio.sleep(services.SERVICE_START_TIMEOUT + .25)
        assert not task.done() and manager.stops == []
        if after_pending == "cancel":
            task.cancel()
            with pytest.raises(asyncio.CancelledError):
                await asyncio.wait_for(task, 30)
            assert len(manager.starts) == 1
            assert read_receipt(directory)["update"]["status"] == "restored-stopped"
        elif after_pending == "unknown":
            update_receipt(directory, admission_health=None, bootstrap=None, health="unknown")
            await asyncio.wait_for(task, 30)
            result = read_receipt(directory)
            assert result["update"]["status"] == "rolled-back"
            assert result["build_id"] == old["build_id"]
        elif after_pending == "stalled-observation":
            original_observe = manager.observe
            stalled_once = False

            async def stalled_observe(definition):
                nonlocal stalled_once
                if not stalled_once:
                    stalled_once = True
                    await asyncio.Event().wait()
                return await original_observe(definition)

            monkeypatch.setattr(manager, "observe", stalled_observe)
            await asyncio.wait_for(task, 30)
            result = read_receipt(directory)
            assert result["update"]["status"] == "rolled-back"
            assert result["build_id"] == old["build_id"]
        else:
            update_receipt(directory, status="failed", health="unavailable")
            # Shield distinguishes the update's own restored deadline from
            # this test's safety timeout; it cannot pass by cancelling it.
            with pytest.raises(TimeoutError):
                await asyncio.wait_for(asyncio.shield(task), 30)
            assert task.done(), "The transaction's own deadline must bound fallback after pending"
            assert manager.starts == [service.implementation.identity, old["build_id"]]
            assert manager.stops == [service.implementation.identity, old["build_id"]]
        assert (target / ".env").read_bytes() == private
        assert manager.stops and manager.stops[0] == service.implementation.identity
    finally:
        if not task.done():
            task.cancel()
        await asyncio.gather(task, return_exceptions=True)


@pytest.mark.parametrize("behavior", ["ack-only", "reused-instance", "reused-attempt", "stale-health", "failed"])
async def test_start_ack_pid_or_old_startup_fields_cannot_commit_candidate(background, behavior):
    service, directory, target, old, private, identity, manager, _ = background
    manager.behaviors = [behavior, "ready"]
    await services.update_background(directory, manager, "update", update_id=identity)
    result = read_receipt(directory)
    assert result["update"]["status"] == "rolled-back"
    assert result["build_id"] == old["build_id"]
    assert manager.starts == [service.implementation.identity, old["build_id"]]
    # Exact candidate stop and its native cleanup precede any old-source copy.
    assert manager.stops == [service.implementation.identity]
    assert result["update_service_verification"]["build_id"] == old["build_id"]
    assert (target / ".env").read_bytes() == private
    if behavior == "failed":
        assert (target / "data/ledger.db").read_bytes() == b"real fixture records; candidate attempt"
        assert (target / ".mobkit/state/native.db").read_bytes() == b"native fixture history; candidate attempt"


async def test_failed_candidate_diagnostics_survive_fallback_log_reset_and_receipt_projection_loss(background):
    service, directory, target, old, _, identity, manager, _ = background
    # An earlier service's retained log must not be attributed to this start.
    update_receipt(directory, logs=[{"at": now(), "phase": "runtime", "text": "old unrelated attempt"}])
    manager.behaviors = ["failed", "ready"]
    await services.update_background(directory, manager, "update", update_id=identity)
    receipt = read_receipt(directory)
    assert receipt["update"]["status"] == "rolled-back" and receipt["build_id"] == old["build_id"]
    assert receipt["logs"] == [{**receipt["logs"][0], "text": "fake fresh supervisor output"}]
    failed = receipt["update"]["failed_attempts"][0]
    assert failed["update_id"] == identity and failed["source"] == "candidate"
    assert failed["build_id"] == service.implementation.identity
    assert failed["package_sha256"] == receipt["update"]["package_sha256"]
    assert failed["source_digest"] == service.implementation.digest
    assert receipt["update"]["previous_source_digest"] == json.loads(
        updates.checked_package(updates.update_directory(directory, identity), "previous.zip",
                                old["package_sha256"])[1]["implementation-report.json"][0])["verification"]["digest"]
    assert failed["service_instance_id"] == manager.attempts[0]["service_instance_id"]
    assert failed["startup_attempt_id"] == manager.attempts[0]["startup_attempt_id"]
    assert failed["service_instance_id"] != receipt["service_instance_id"]
    assert failed["startup_attempt_id"] != receipt["startup_attempt_id"]
    assert failed["service_cleanup_confirmed"] is True
    output = json.dumps(failed)
    assert "EADDRINUSE" in output and "late output drained" in output
    assert "old unrelated attempt" not in output
    assert "private-api-key-value" not in output and "synthetic-private-token" not in output
    assert str(target) not in output and str(directory) not in output
    assert "[private]" in output and "previous verified source restarted" in receipt["detail"]
    # The update journal, not the replaceable ordinary logs/projection, retains it.
    saved = json.loads((updates.update_directory(directory, identity) / "update.json").read_text())
    assert saved["failed_attempts"] == [failed]
    update_receipt(directory, update={key: value for key, value in receipt["update"].items()
                                     if key not in {"failed_attempts", "failed_attempts_truncated"}}, logs=[])
    restored, _, _ = updates.transaction(directory, identity)
    assert restored["update"]["failed_attempts"] == [failed]


async def test_failed_attempt_diagnostics_are_bounded_and_repeated_capture_is_not_duplicate(background):
    _, directory, target, old, _, identity, _, _ = background
    redact = Redactor([*read_settings(target).values(), str(target), str(directory)])
    # Source projection was absent in earlier staged journals. The archive must
    # derive it from the already-bound previous ZIP, not the newer candidate.
    _, root, bound = updates.transaction(directory, identity)
    legacy = {key: value for key, value in bound.items() if key != "previous_source_digest"}
    updates.write_private(root / "update.json", legacy)
    update_receipt(directory, update=legacy)
    contexts = [{"attempt_id": str(uuid.uuid4()), "build_id": old["build_id"],
                 "package_sha256": old["package_sha256"]} for _ in range(10)]
    for context in contexts:
        updates.retain_failed_service_attempt(directory, identity, context,
            error=RuntimeError("failed fixture with private-api-key-value"),
            logs=[{"at": now(), "phase": "runtime", "text": "small observed fixture output"}], redact=redact)
    current = read_receipt(directory)["update"]
    assert current["previous_source_digest"] == current["failed_attempts"][-1]["source_digest"]
    assert len(current["failed_attempts"]) == updates.MAX_FAILED_ATTEMPTS
    assert current["failed_attempts_truncated"] == 2
    last = contexts[-1]
    # Multilingual output is bounded by its serialized bytes as well as rows.
    output = [{"at": now(), "phase": "runtime", "text": "\u96e8" * 4000 + "private-api-key-value"} for _ in range(200)]
    result = updates.retain_failed_service_attempt(directory, identity, last,
        error=RuntimeError("failed fixture"), logs=output, redact=redact)
    records = result["failed_attempts"]
    assert sum(item["attempt_id"] == last["attempt_id"] for item in records) == 1
    assert records[-1]["logs_truncated"] is True
    assert len(json.dumps(records[-1]["logs"], indent=2).encode()) <= updates.MAX_ATTEMPT_LOG_BYTES
    assert len(json.dumps({"failed_attempts": records}, indent=2).encode()) <= updates.MAX_FAILED_ATTEMPT_BYTES
    assert "private-api-key-value" not in json.dumps(records)


async def test_legacy_archive_authority_first_publication_recovers_after_receipt_write_interruption(background, monkeypatch):
    _, directory, target, old, private, identity, _, _ = background
    _, root, bound = updates.transaction(directory, identity)
    legacy = {key: value for key, value in bound.items() if key != "previous_source_digest"}
    updates.write_private(root / "update.json", legacy)
    update_receipt(directory, update=legacy)
    # Missing-on-both metadata remains a valid historical staged transaction.
    assert "previous_source_digest" not in updates.transaction(directory, identity)[2]
    original = updates.write_private
    def interrupted(path, value, *args, **kwargs):
        if path == directory / "receipt.json" and "failed_attempts" in value.get("update", {}):
            raise OSError("fake host interruption before receipt projection")
        return original(path, value, *args, **kwargs)
    monkeypatch.setattr(updates, "write_private", interrupted)
    context = {"attempt_id": str(uuid.uuid4()), "build_id": old["build_id"], "package_sha256": old["package_sha256"]}
    with pytest.raises(OSError, match="before receipt projection"):
        updates.retain_failed_service_attempt(directory, identity, context,
            error=RuntimeError("fake start failure"), logs=[{"at": now(), "phase": "runtime", "text": "retained output"}],
            redact=Redactor(read_settings(target).values()))
    authority = json.loads((root / "update.json").read_text())
    projection = read_receipt(directory)["update"]
    assert authority["failed_attempts"][0]["attempt_id"] == context["attempt_id"]
    assert authority["previous_source_digest"] and "previous_source_digest" not in projection
    monkeypatch.setattr(updates, "write_private", original)
    recovered, _, journal = updates.transaction(directory, identity)
    assert recovered["update"] == journal == authority
    assert recovered["build_id"] == old["build_id"] and recovered["package_sha256"] == old["package_sha256"]
    assert (target / ".env").read_bytes() == private
    assert (target / "data/ledger.db").read_bytes() == b"user application records"


@pytest.mark.parametrize("change", ["wrong-derived", "existing-disagreement", "removed-authority"])
async def test_legacy_previous_source_reconciliation_refuses_unverified_or_backward_changes(background, change):
    _, directory, _, _, _, identity, _, _ = background
    _, root, bound = updates.transaction(directory, identity)
    if change == "wrong-derived":
        projected = {key: value for key, value in bound.items() if key != "previous_source_digest"}
        authority = {**bound, "previous_source_digest": "not-the-previous-package-source"}
    elif change == "existing-disagreement":
        projected = {**bound, "previous_source_digest": "different-nonmissing-source"}
        authority = bound
    else:
        projected = bound
        authority = {key: value for key, value in bound.items() if key != "previous_source_digest"}
    updates.write_private(root / "update.json", authority)
    update_receipt(directory, update=projected)
    with pytest.raises(DraftConflict, match="previous source provenance"):
        updates.transaction(directory, identity)
    assert read_receipt(directory)["update"] == projected
    assert json.loads((root / "update.json").read_text()) == authority


@pytest.mark.parametrize("change", ["build", "package", "update", "instance"])
async def test_failed_attempt_diagnostics_cannot_relabel_another_source_or_attempt(background, change):
    _, directory, _, old, _, identity, _, _ = background
    context = {"attempt_id": str(uuid.uuid4()), "build_id": old["build_id"], "package_sha256": old["package_sha256"]}
    expected = identity
    instance = None
    if change == "build": context["build_id"] = str(uuid.uuid4())
    if change == "package": context["package_sha256"] = "f" * 64
    if change == "update": expected = str(uuid.uuid4())
    if change == "instance": instance = str(uuid.uuid4())
    before = (updates.update_directory(directory, identity) / "update.json").read_bytes()
    with pytest.raises(DraftConflict):
        updates.retain_failed_service_attempt(directory, expected, context,
            error=RuntimeError("fixture failure"), logs=[], redact=Redactor([]), service_instance_id=instance)
    assert (updates.update_directory(directory, identity) / "update.json").read_bytes() == before


async def test_diagnostic_storage_failure_still_stops_candidate_and_blocks_source_fallback(background, monkeypatch):
    service, directory, target, _, _, identity, manager, _ = background
    manager.behaviors = ["failed"]
    original = updates.checkpoint
    def failed_storage(*args, **changes):
        if "failed_attempts" in changes:
            raise OSError("fake private diagnostic write failure")
        return original(*args, **changes)
    monkeypatch.setattr(updates, "checkpoint", failed_storage)
    with pytest.raises(RuntimeError, match="diagnostic preservation"):
        await services.update_background(directory, manager, "update", update_id=identity)
    receipt = read_receipt(directory)
    assert receipt["update"]["status"] == "recovery-needed"
    assert receipt["service_cleanup_confirmed"] is True and not manager.observation["pid"]
    assert receipt["build_id"] == service.implementation.identity
    assert manager.starts == manager.stops == [service.implementation.identity]
    assert (target / "application/logic.py").read_text() == "VERSION = 'new'\n"
    assert "confirmed cleanup" in receipt["update"]["detail"]


async def test_cleanup_cannot_attach_a_replacement_service_generation_to_failed_attempt(background):
    service, directory, target, _, _, identity, manager, _ = background
    manager.behaviors = ["failed"]
    original = manager.stop
    async def replaced(definition):
        result = await original(definition)
        update_receipt(directory, service_instance_id=str(uuid.uuid4()), startup_attempt_id=str(uuid.uuid4()),
                       logs=[{"at": now(), "phase": "runtime", "text": "unrelated replacement instance"}])
        return result
    manager.stop = replaced
    with pytest.raises(RuntimeError, match="needs recovery"):
        await services.update_background(directory, manager, "update", update_id=identity)
    receipt = read_receipt(directory)
    assert receipt["update"]["status"] == "recovery-needed"
    assert not receipt["update"].get("failed_attempts")
    assert receipt["build_id"] == service.implementation.identity
    assert manager.starts == [service.implementation.identity]
    assert (target / "application/logic.py").read_text() == "VERSION = 'new'\n"


async def test_unconfirmed_attempt_cleanup_prevents_fallback_and_requires_explicit_recovery(background):
    service, directory, target, _, private, identity, manager, _ = background
    manager.behaviors = ["failed"]
    manager.cleanup = False
    with pytest.raises(RuntimeError, match="cleanup is unconfirmed"):
        await services.update_background(directory, manager, "update", update_id=identity)
    result = read_receipt(directory)
    assert result["update"]["status"] == "recovery-needed"
    assert result["update"]["failed_attempts"][0]["service_cleanup_confirmed"] is False
    assert result["build_id"] == service.implementation.identity
    assert (target / "application/logic.py").read_text() == "VERSION = 'new'\n"
    assert manager.starts == [service.implementation.identity]
    assert services.background_update_capabilities(directory, result)["background_activation_supported"] is True
    with pytest.raises(DraftConflict, match="confirm its native cleanup"):
        await services.require_stopped_service(directory, manager)
    manager.cleanup = True
    await services.stop_background(directory, manager)
    await services.update_background(directory, manager, "recover-current", update_id=identity)
    result = read_receipt(directory)
    assert result["update"]["status"] == "completed"
    assert manager.starts == [service.implementation.identity, service.implementation.identity]
    assert (target / ".env").read_bytes() == private
    assert (target / "data/ledger.db").read_bytes() == b"real fixture records; candidate attempt"


async def test_cancellation_after_service_admission_stops_exact_instance_before_source_fallback(background):
    service, directory, target, old, private, identity, manager, _ = background
    manager.behaviors = ["block"]
    task = asyncio.create_task(services.update_background(directory, manager, "update", update_id=identity))
    await asyncio.wait_for(manager.started.wait(), 30)
    assert read_receipt(directory)["build_id"] == service.implementation.identity
    (target / "data/ledger.db").write_bytes(b"state recorded before host cancellation")
    task.cancel()
    with pytest.raises(asyncio.CancelledError):
        await task
    result = read_receipt(directory)
    assert result["update"]["status"] == "restored-stopped"
    assert result["status"] == "stopped" and result["service_cleanup_confirmed"] is True
    assert result["build_id"] == old["build_id"]
    assert manager.stops == [service.implementation.identity]
    assert len(manager.starts) == 1 and not manager.observation["pid"]
    assert (target / "data/ledger.db").read_bytes() == b"state recorded before host cancellation"
    assert (target / ".env").read_bytes() == private


async def test_cancellation_during_fallback_start_reaps_second_instance_without_false_health(background):
    service, directory, target, old, _, identity, manager, _ = background
    manager.behaviors = ["failed", "block"]
    first = manager.started
    second = asyncio.Event()
    original = manager.start
    async def start(definition):
        if manager.starts:
            manager.started = second
        return await original(definition)
    manager.start = start
    task = asyncio.create_task(services.update_background(directory, manager, "update", update_id=identity))
    await asyncio.wait_for(second.wait(), 30)
    assert first.is_set() and read_receipt(directory)["build_id"] == old["build_id"]
    task.cancel()
    with pytest.raises(asyncio.CancelledError):
        await task
    receipt = read_receipt(directory)
    assert receipt["status"] == "stopped" and receipt["update"]["status"] != "completed"
    assert receipt["service_cleanup_confirmed"] is True and not manager.observation["pid"]
    failures = receipt["update"]["failed_attempts"]
    assert [item["source"] for item in failures] == ["candidate", "previous"]
    assert [item["build_id"] for item in failures] == [service.implementation.identity, old["build_id"]]
    assert len({item["attempt_id"] for item in failures}) == 2
    assert [item["service_instance_id"] for item in failures] == [item["service_instance_id"] for item in manager.attempts]
    assert manager.stops == [service.implementation.identity, old["build_id"]]
    assert (target / "data/ledger.db").read_bytes() == b"real fixture records; candidate attempt"


async def test_both_attempts_failing_stop_owned_instances_and_preserve_explicit_recovery(background):
    service, directory, target, old, _, identity, manager, _ = background
    manager.behaviors = ["failed", "failed"]
    with pytest.raises(RuntimeError, match="needs recovery"):
        await services.update_background(directory, manager, "update", update_id=identity)
    receipt = read_receipt(directory)
    assert receipt["update"]["status"] == "recovery-needed" and receipt["build_id"] == old["build_id"]
    assert receipt["service_cleanup_confirmed"] is True and not manager.observation["pid"]
    assert manager.stops == [service.implementation.identity, old["build_id"]]
    assert (target / "data/ledger.db").read_bytes() == b"real fixture records; candidate attempt"


@pytest.mark.parametrize("observation", [
    {"loaded": True, "pid": None, "state": "waiting"},
    {"loaded": True, "pid": None, "state": "activating", "substate": "auto-restart"},
    {"loaded": True, "pid": None, "state": "inactive", "control_pid": 123},
    {"loaded": True, "pid": None, "state": "inactive", "control_group": "/owned/control"},
])
async def test_manager_restart_or_control_activity_is_not_a_stopped_native_runtime(background, observation):
    _, directory, _, _, _, _, manager, _ = background
    manager.observation = observation
    with pytest.raises(DraftConflict, match="Stop this exact"):
        await services.require_stopped_service(directory, manager)
    projected = await services.background_status(directory, manager)
    assert projected["status"] != "stopped"
    assert not services.manager_stopped(observation)


async def test_stopped_observation_cannot_cross_a_new_service_generation(background):
    _, directory, _, _, _, _, manager, _ = background
    original = manager.observe
    async def raced(service):
        observation = await original(service)
        update_receipt(directory, service_instance_id="new-generation", service_supervisor_pid=6000,
                       service_cleanup_confirmed=True)
        return observation
    manager.observe = raced
    with pytest.raises(DraftConflict, match="instance changed"):
        await services.require_stopped_service(directory, manager)


async def test_actual_deployment_action_rechecks_source_and_requires_stop_then_uses_background_adapter(background):
    service, directory, _, old, _, identity, manager, _ = background
    controller = DeploymentService(service.root.parent, service.guide, service.implementation, service_manager=manager)
    manager.observation = {"loaded": True, "pid": 4567, "state": "running"}
    update_receipt(directory, status="running", service_cleanup_confirmed=False)
    with pytest.raises(DraftConflict, match="Stop and verify"):
        await controller.action(service.guide.identity, directory.name, "apply-update", update_id=identity)
    assert not controller.tasks and not manager.starts
    await services.stop_background(directory, manager)
    admitted = await controller.action(service.guide.identity, directory.name, "apply-update", update_id=identity)
    assert admitted["update_action"] == {"action": "apply-update", "update_id": identity}
    await asyncio.wait_for(asyncio.gather(*controller.tasks.values()), 3)
    result = await controller.status(service.guide.identity)
    assert result["status"] == "running" and result["update"]["status"] == "completed"
    await controller.shutdown()
    assert len(manager.starts) == 1 and manager.stops == [old["build_id"]]


async def test_user_stop_during_manager_start_cancels_activation_without_later_service_restart(background):
    service, directory, _, old, _, identity, manager, _ = background
    controller = DeploymentService(service.root.parent, service.guide, service.implementation, service_manager=manager)
    manager.behaviors = ["block"]
    await controller.action(service.guide.identity, directory.name, "apply-update", update_id=identity)
    await asyncio.wait_for(manager.started.wait(), 30)
    result = await controller.action(service.guide.identity, directory.name, "stop")
    assert result["status"] == "stopped"
    assert result["update"]["status"] == "restored-stopped" and result["build_id"] == old["build_id"]
    assert len(manager.starts) == 1 and not controller.tasks
    assert not manager.observation["pid"]


@pytest.mark.parametrize("change, error, message", [
    ("source", DraftConflict, "changed before update admission"),
    ("selection", DraftConflict, "changed before update admission"),
    ("brief", DraftConflict, "changed before update admission"),
    ("acceptance", ValueError, "independent acceptance"),
])
async def test_candidate_changed_during_final_manager_query_is_rejected_before_admission(
        background, monkeypatch, change, error, message):
    service, directory, target, old, _, identity, manager, _ = background
    phase = {"accepted_checked": False, "stopped_after_acceptance": False,
             "final_journal_checked": False, "changed": False}
    accepted = service.implementation.status
    async def checked_build(*args, **kwargs):
        value = await accepted(*args, **kwargs)
        phase["accepted_checked"] = True
        return value
    monkeypatch.setattr(service.implementation, "status", checked_build)
    stopped = services.require_stopped_service
    async def checked_stopped(*args, **kwargs):
        value = await stopped(*args, **kwargs)
        if phase["accepted_checked"]:
            phase["stopped_after_acceptance"] = True
        return value
    monkeypatch.setattr(services, "require_stopped_service", checked_stopped)
    inspect = updates.inspect_transaction
    def checked_journal(*args, **kwargs):
        value = inspect(*args, **kwargs)
        if phase["stopped_after_acceptance"]:
            phase["final_journal_checked"] = True
        return value
    monkeypatch.setattr(updates, "inspect_transaction", checked_journal)
    actual = manager.observe
    async def raced(definition):
        observed = await actual(definition)
        # The final service observation follows the accepted-build and journal
        # checks, regardless of extra status/support manager queries.
        if phase["final_journal_checked"] and not phase["changed"]:
            phase["changed"] = True
            if change == "source":
                service.implementation.digest = "changed-during-final-service-observation"
            elif change == "selection":
                service.implementation.identity = str(uuid.uuid4())
            elif change == "brief":
                service.guide.state["brief"] = {"goal": "A different accepted scope"}
            else:
                service.implementation.accepted = False
        return observed
    manager.observe = raced
    with pytest.raises(error, match=message):
        await service.action(service.guide.identity, directory.name, "apply-update", update_id=identity)
    assert phase["changed"]
    assert not service.tasks and not manager.starts
    assert read_receipt(directory)["build_id"] == old["build_id"]
    assert read_receipt(directory)["status"] == "stopped"
    assert (target / "application/logic.py").read_text() == "VERSION = 'old'\n"


async def test_source_or_frozen_supervisor_tamper_disables_activation_without_manager_start(background):
    service, directory, target, _, _, identity, manager, _ = background
    original = (target / "application/logic.py").read_bytes()
    (target / "application/logic.py").write_text("user modified source\n")
    cap = services.background_update_capabilities(directory, read_receipt(directory))
    assert cap["background_activation_supported"] is False
    with pytest.raises(DraftConflict):
        await service.action(service.guide.identity, directory.name, "apply-update", update_id=identity)
    assert not manager.starts
    (target / "application/logic.py").write_bytes(original)
    (directory / "service-host/launcher.py").write_text("print('foreign host')\n")
    cap = services.background_update_capabilities(directory, read_receipt(directory))
    assert cap["background_activation_supported"] is False and "supervisor source changed" in cap["reason"]
    with pytest.raises(DraftConflict):
        await service.action(service.guide.identity, directory.name, "apply-update", update_id=identity)
    assert not manager.starts


@pytest.mark.parametrize("metadata", [
    {}, {"update_startup_protocol": True}, {"update_startup_protocol": 9},
    {"update_startup_protocol": 1, "update_worker_sha256": "another-worker"},
])
async def test_legacy_or_unbound_frozen_startup_protocol_stays_preview_only_before_copy(background, metadata):
    service, directory, target, old, private, identity, manager, _ = background
    saved = read_receipt(directory)["service"]
    saved = {key: value for key, value in saved.items()
             if key not in {"update_startup_protocol", "update_worker_sha256"}}
    update_receipt(directory, service={**saved, **metadata})
    before = Path(saved["unit_path"]).read_bytes()
    assert services.verify_service(directory, read_receipt(directory))
    cap = services.background_update_capabilities(directory, read_receipt(directory))
    assert cap["background_activation_supported"] is False and "preview remains available" in cap["reason"]
    with pytest.raises(DraftConflict, match="preserved background supervisor"):
        await service.action(service.guide.identity, directory.name, "apply-update", update_id=identity)
    with pytest.raises(DraftConflict, match="preserved background supervisor"):
        await services.update_background(directory, manager, "update", update_id=identity)
    assert read_receipt(directory)["update"]["status"] == "staged"
    assert read_receipt(directory)["build_id"] == old["build_id"]
    assert Path(saved["unit_path"]).read_bytes() == before
    assert (target / "application/logic.py").read_text() == "VERSION = 'old'\n"
    assert (target / ".env").read_bytes() == private
    assert not manager.starts and not manager.stops


async def test_partial_owned_source_remains_recoverable_but_foreign_change_does_not(background, monkeypatch):
    _, directory, target, _, _, identity, _, _ = background
    real = updates.apply_files
    def interrupted(destination, before, after):
        updates.replace_file(destination / "application/logic.py", *after["application/logic.py"])
        raise OSError("fixture interruption during source replacement")
    monkeypatch.setattr(updates, "apply_files", interrupted)
    with pytest.raises(OSError):
        updates.activate(directory, identity)
    assert services.background_update_capabilities(directory, read_receipt(directory))["background_activation_supported"] is True
    monkeypatch.setattr(updates, "apply_files", real)
    (target / "application/logic.py").write_text("foreign source\n")
    assert services.background_update_capabilities(directory, read_receipt(directory))["background_activation_supported"] is False


def test_systemd_cwd_is_directive_specific_and_command_preserves_literal_dollars(tmp_path):
    root = tmp_path / "owned space %literal $folder"
    plan = services.service_plan(str(uuid.uuid4()), root, platform="linux", home=tmp_path)
    plan["interpreter"] = str(tmp_path / "python $executable")
    content = services.unit_bytes(plan).decode()
    lines = dict(line.split("=", 1) for line in content.splitlines() if "=" in line)
    assert lines["WorkingDirectory"] == str(root).replace("%", "%%")
    assert not lines["WorkingDirectory"].startswith('"')
    assert lines["ExecStart"].startswith('"' + plan["interpreter"] + '" ')
    assert "$$folder" in lines["ExecStart"] and "%%literal" in lines["ExecStart"]


async def test_exact_recorded_legacy_linux_definition_is_preserved_and_explicitly_blocked(background, monkeypatch):
    _, directory, _, _, _, _, manager, _ = background
    original = services.service_plan
    def linux(identity, root, **kwargs):
        return original(identity, root, platform="linux")
    monkeypatch.setattr(services, "service_plan", linux)
    receipt = read_receipt(directory)
    plan = linux(receipt["deployment_id"], directory)
    content = services.unit_bytes(plan, legacy=True)
    path = Path(plan["unit_path"])
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(content)
    path.chmod(0o600)
    service = {**receipt["service"], **plan, "unit_sha256": services.digest(content)}
    update_receipt(directory, service=service)
    with pytest.raises(DraftConflict, match="invalid quoted WorkingDirectory"):
        await services.require_stopped_service(directory, manager)
    cap = services.background_update_capabilities(directory, read_receipt(directory))
    assert cap["background_activation_supported"] is False and "migration" in cap["reason"]
    assert path.read_bytes() == content and not manager.starts and not manager.stops


@pytest.mark.parametrize("reported", [392_000, 432_000], ids=["meta-0.9.4", "meta-0.9.5"])
async def test_first_update_on_an_earlier_unit_widens_its_stop_budget_without_rollback(
        unit_from_earlier_forge, background, reported):
    # The update was staged against the earlier unit, before any drop-in existed.
    service, directory, target, old, private, identity, manager, definition = background
    assert definition["stop_timeout_seconds"] == 397
    unit = Path(definition["unit_path"]).read_bytes()
    manager.reported_horizon = reported
    await services.update_background(directory, manager, "update", update_id=identity)
    current = read_receipt(directory)
    assert current["update"]["status"] == "completed" and current["status"] == "running"
    assert manager.starts == [service.implementation.identity] and manager.stops == []
    budget = reported // 1000 + 20 + 10 + 5 + 15
    drop_in = Path(definition["unit_path"] + ".d/forge-stop-budget.conf")
    assert drop_in.read_bytes() == f"[Service]\nTimeoutStopSec={budget}\n".encode()
    # The frozen unit and the service identity the update was staged with hold.
    assert Path(definition["unit_path"]).read_bytes() == unit
    assert services.service_binding(services.verify_service(directory, current)) == services.service_binding(definition)


RELOAD = ["/usr/bin/systemctl", "--user", "daemon-reload"]


@pytest.mark.parametrize("killed", ["after-record", "before-reload", "re-widening-after-record"])
async def test_an_interrupted_stop_budget_widening_stays_controllable_and_completes(
        unit_from_earlier_forge, background, monkeypatch, killed):
    service, directory, target, old, private, identity, manager, definition = background
    drop_in = Path(definition["unit_path"] + ".d/forge-stop-budget.conf")
    if killed == "after-record":
        # The worker dies after recording the widened budget, before the drop-in exists.
        def killed_writer(stop_timeout):
            raise RuntimeError("killed")
        with monkeypatch.context() as patch:
            patch.setattr(services, "drop_in_bytes", killed_writer)
            with pytest.raises(RuntimeError, match="killed"):
                services.widen_service_stop_budget(directory, 432_000)
        assert read_receipt(directory)["service_stop_budget"]["stop_timeout_seconds"] == 482
        assert not drop_in.exists()
    elif killed == "before-reload":
        # The worker dies after writing the drop-in, before it reloads the manager.
        assert services.widen_service_stop_budget(directory, 432_000) == RELOAD
        assert drop_in.exists()
    else:
        # An earlier readiness widened the unit for a 392-second horizon. A
        # longer report records its budget, then the worker dies before its
        # drop-in replaces the earlier one.
        assert services.widen_service_stop_budget(directory, 392_000) == RELOAD
        replace = os.replace
        def killed_replace(source, destination, *args, **kwargs):
            if Path(destination).name == drop_in.name:
                raise RuntimeError("killed")
            return replace(source, destination, *args, **kwargs)
        with monkeypatch.context() as patch:
            patch.setattr(os, "replace", killed_replace)
            with pytest.raises(RuntimeError, match="killed"):
                services.widen_service_stop_budget(directory, 432_000)
        assert read_receipt(directory)["service_stop_budget"]["stop_timeout_seconds"] == 482
        assert drop_in.read_bytes() == b"[Service]\nTimeoutStopSec=442\n"
    # Status, stop and a fresh start keep working on the interrupted state.
    await services.background_status(directory, manager)
    await services.stop_background(directory, manager)
    manager.reported_horizon = 432_000
    await services.update_background(directory, manager, "update", update_id=identity)
    assert read_receipt(directory)["update"]["status"] == "completed"
    # The next readiness completes the step: the drop-in and a manager reload.
    assert drop_in.read_bytes() == b"[Service]\nTimeoutStopSec=482\n"
    assert services.widen_service_stop_budget(directory, 432_000) == RELOAD
