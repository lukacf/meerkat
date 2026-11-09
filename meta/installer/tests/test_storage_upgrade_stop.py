"""Stopped storage upgrades fence new apply and settle entered native work."""
import ast
import asyncio
from contextlib import contextmanager
import hashlib
import json
import os
from pathlib import Path
import threading
from types import SimpleNamespace

import pytest

from launchkit import deployment_worker as worker
from launchkit import runtime_dependencies
from launchkit.templates import storage_setup as storage
from test_storage_setup import native_upgrade_report


@pytest.fixture
def project(tmp_path, monkeypatch):
    root = tmp_path.resolve()
    (root / "deployment.json").write_text(json.dumps({"slug": "test-team", "mobkitVersion": "0.8.46"}))
    (root / ".mobkit").mkdir(mode=0o700)
    (root / ".mobkit/state").mkdir(mode=0o700)
    retained = [root / ".mobkit/state/continuity.sqlite3", root / ".mobkit/state/blobs/history.bin", root / ".env"]
    for path in retained:
        path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        path.write_bytes(b"Original synthetic private history and configuration.\n")
    marker = root / ".mobkit/launch-storage.json"
    storage.write_private(marker, {"format": 1, "phase": "ready", "project": "test-team", "version": "0.8.42"})
    (root / ".bin").mkdir(mode=0o700)
    (root / ".bin/mobkit_gateway").write_bytes(b"Inert native input, never executed.\n")
    before = {path: (path.read_bytes(), path.stat().st_ino) for path in retained}
    calls = []
    observation = SimpleNamespace(root=root, marker=marker, original_marker=marker.read_bytes(),
        before=before, calls=calls, read_version=1, on_read=lambda: None, on_apply=lambda: None,
        on_transaction=lambda: None)

    def native(command, **options):
        assert options == {"capture_output": True, "text": True, "timeout": 120, "check": False}
        applying = command[-1] == "--apply"
        assert command == [str(root / ".bin/mobkit_gateway"), "storage-migrate", "--state-dir",
                           str(root / ".mobkit/state"), "--json", *(["--apply"] if applying else [])]
        calls.append(command)
        observation.on_apply() if applying else observation.on_read()
        return SimpleNamespace(stdout=json.dumps(native_upgrade_report(root,
            apply=applying, version=2 if applying else observation.read_version)), returncode=0)

    monkeypatch.setattr(storage.subprocess, "run", native)
    monkeypatch.setattr(asyncio, "create_subprocess_exec", lambda *args, **kwargs: pytest.fail("No OS subprocess may execute."))
    return observation


def retained_reports(project, *, applied):
    attempts = list((project.root / ".mobkit/storage-upgrades").iterdir())
    assert len(attempts) == 1
    read = attempts[0] / "read.json"
    assert json.loads(read.read_bytes()) == native_upgrade_report(project.root, version=project.read_version)
    assert read.stat().st_mode & 0o777 == 0o600
    apply = attempts[0] / "apply.json"
    assert apply.exists() is applied
    if applied:
        assert json.loads(apply.read_bytes()) == native_upgrade_report(project.root, apply=True)
        assert apply.stat().st_mode & 0o777 == 0o600
    assert all(path.read_bytes() == content and path.stat().st_ino == inode
               for path, (content, inode) in project.before.items())


def checked_start(project, monkeypatch):
    """Execute the exact nested caller body with inert surrounding admissions."""
    source = Path(worker.__file__)
    if os.environ.get("FORGE_TEST_STORAGE_STOP_BASELINE") == "1":
        source = Path(__file__).parents[1] / ".artifacts/forge-storage-upgrade-stop-20261010/tests/deployment_worker.py.before"
        assert hashlib.sha256(source.read_bytes()).hexdigest() == "7d56475cec8ae11e739c9d5cac2d59f256c0bf0cdaa2ea9740e48f9ef47bc231"
    outer = next(node for node in ast.parse(source.read_bytes()).body
                 if isinstance(node, ast.AsyncFunctionDef) and node.name == "update_runtime")
    function = next(node for node in outer.body if isinstance(node, ast.AsyncFunctionDef) and node.name == "checked_start")
    supervisor = SimpleNamespace(destination=project.root, stop=asyncio.Event())
    commands, checkpoints, leases, releases = [], [], [], []
    original_lock = storage.project_lock

    @contextmanager
    def held(root):
        with original_lock(root) as lease:
            leases.append(lease)
            try:
                yield lease
            finally:
                lease.require_current()
                releases.append({"marker": project.marker.read_bytes(),
                    "apply_reports": [path.read_bytes() for path in
                        (project.root / ".mobkit/storage-upgrades").glob("*/apply.json")]})

    async def command(arguments, phase, timeout, **options):
        if supervisor.stop.is_set():
            raise asyncio.CancelledError
        commands.append((arguments, phase))
        if options.get("capture_output"):
            return json.dumps({"package": "meerkat-mobkit", "version": "0.8.46"})

    async def start(*args, **kwargs):
        pytest.fail("An interrupted upgrade cannot start the application.")

    supervisor.start = start
    updates = SimpleNamespace(_require_runtime_framework_configuration=lambda *args: None,
        checkpoint=lambda *args, **fields: checkpoints.append(fields), _same_toml=lambda left, right: left == right)
    monkeypatch.setattr(runtime_dependencies, "retain_runtime", lambda *args, **kwargs: {})
    monkeypatch.setattr(runtime_dependencies, "verify_runtime", lambda *args, **kwargs: {})

    def current_transaction():
        project.on_transaction()
        return None, project.root, {}

    namespace = {**worker.__dict__, "supervisor": supervisor, "directory": project.root,
        "recover_current": False, "product_update": False, "dependency_update": {"from_version": "0.8.42", "to_version": "0.8.46"},
        "verification_kwargs": {}, "current_transaction": current_transaction,
        "expected_binding": None, "expected_installation": None, "updates": updates,
        "journal": lambda *args, **kwargs: None, "verify_destination": lambda *args: None,
        "read_receipt": lambda *args: {}, "read_settings": lambda *args: {}, "command": command,
        "project_lock": held, "native_update": False, "replacement": None, "registry_update": False,
        "owner_start": False}
    exec(compile(ast.Module(body=[function], type_ignores=[]), str(source), "exec"), namespace)
    files = {"deployment.json": ((project.root / "deployment.json").read_text(), 0o644)}
    return SimpleNamespace(run=lambda: namespace["checked_start"](files, False), supervisor=supervisor,
        commands=commands, checkpoints=checkpoints, leases=leases, releases=releases)


async def thread_entered(event):
    async def wait():
        while not event.is_set():
            await asyncio.sleep(0.001)
    await asyncio.wait_for(wait(), 30)


async def test_stop_during_native_dry_run_never_enters_apply_and_keeps_old_history(project, monkeypatch):
    caller = checked_start(project, monkeypatch)
    entered, release = threading.Event(), threading.Event()

    def read():
        entered.set()
        assert release.wait(2), "The inert dry-run was not released."

    project.on_read = read
    task = asyncio.create_task(caller.run())
    try:
        await thread_entered(entered)
        assert len(caller.leases) == 1
        caller.supervisor.stop.set()
        release.set()
        with pytest.raises(asyncio.CancelledError):
            await task
        assert len(project.calls) == 1, "Stop during dry-run must prevent a distinct new apply."
        assert project.marker.read_bytes() == project.original_marker
        retained_reports(project, applied=False)
        assert not any("runtime_storage_upgrade" in row for row in caller.checkpoints)
        assert not any(phase in {"Update prerequisites", "Updated application contract"} for _, phase in caller.commands)
    finally:
        release.set()
        await asyncio.gather(task, return_exceptions=True)
    with pytest.raises(RuntimeError, match="no longer current"):
        caller.leases[0].require_current()


def test_stop_after_no_apply_read_preserves_old_marker_and_retained_report(project):
    stopped = threading.Event()
    project.read_version = 2
    project.on_read = stopped.set

    def current():
        if stopped.is_set():
            raise asyncio.CancelledError

    with storage.project_lock(project.root), pytest.raises(asyncio.CancelledError):
        storage.upgrade_storage(project.root, "0.8.42", "0.8.46", require_owner_current=current)
    assert len(project.calls) == 1
    assert project.marker.read_bytes() == project.original_marker
    retained_reports(project, applied=False)


async def test_repeated_cancellation_during_dry_run_joins_original_lease_without_apply(project, monkeypatch):
    caller = checked_start(project, monkeypatch)
    entered, release = threading.Event(), threading.Event()

    def read():
        entered.set()
        assert release.wait(2), "The inert dry-run was not released."
        caller.leases[0].require_current()

    project.on_read = read
    task = asyncio.create_task(caller.run())
    try:
        await thread_entered(entered)
        task.cancel()
        await asyncio.sleep(0)
        task.cancel()
        await asyncio.sleep(0)
        assert caller.supervisor.stop.is_set() and not task.done()
        caller.leases[0].require_current()
        with pytest.raises(RuntimeError, match="already running"):
            with storage.project_lock(project.root):
                pytest.fail("Cancelled dry-run lost its original project lease.")
        assert project.marker.read_bytes() == project.original_marker
        release.set()
        with pytest.raises(asyncio.CancelledError):
            await task
        assert len(project.calls) == 1
        assert project.marker.read_bytes() == project.original_marker
        retained_reports(project, applied=False)
        assert len(caller.releases) == 1
        assert caller.releases[0] == {"marker": project.original_marker, "apply_reports": []}
        assert not any("runtime_storage_upgrade" in row for row in caller.checkpoints)
    finally:
        release.set()
        await asyncio.gather(task, return_exceptions=True)
    with pytest.raises(RuntimeError, match="no longer current"):
        caller.leases[0].require_current()


def test_lost_original_project_lease_before_apply_refuses_new_native_work(project):
    with storage.project_lock(project.root) as lease:
        project.on_read = lambda: os.close(lease._descriptor)
        with pytest.raises(RuntimeError, match="no longer current"):
            storage.upgrade_storage(project.root, "0.8.42", "0.8.46", require_owner_current=lease.require_current)
    assert len(project.calls) == 1
    assert project.marker.read_bytes() == project.original_marker
    retained_reports(project, applied=False)


async def test_stop_set_by_current_transaction_at_apply_boundary_refuses_new_native_work(project, monkeypatch):
    caller = checked_start(project, monkeypatch)

    def transaction():
        if list((project.root / ".mobkit/storage-upgrades").glob("*/read.json")):
            caller.supervisor.stop.set()

    project.on_transaction = transaction
    with pytest.raises(asyncio.CancelledError):
        await caller.run()
    assert len(project.calls) == 1
    assert project.marker.read_bytes() == project.original_marker
    retained_reports(project, applied=False)
    assert not any("runtime_storage_upgrade" in row for row in caller.checkpoints)
    with pytest.raises(RuntimeError, match="no longer current"):
        caller.leases[0].require_current()


@pytest.mark.parametrize("interruption", ["stop", "cancel"])
async def test_entered_apply_settles_report_and_marker_before_original_lease_release(project, monkeypatch, interruption):
    caller = checked_start(project, monkeypatch)
    entered, release = threading.Event(), threading.Event()

    def apply():
        caller.leases[0].require_current()
        entered.set()
        assert release.wait(2), "The inert apply was not released."
        caller.leases[0].require_current()
        with pytest.raises(RuntimeError, match="already running"):
            with storage.project_lock(project.root):
                pytest.fail("Entered apply lost its original project lease.")

    project.on_apply = apply
    task = asyncio.create_task(caller.run())
    try:
        await thread_entered(entered)
        if interruption == "stop":
            caller.supervisor.stop.set()
        else:
            task.cancel()
            await asyncio.sleep(0)
            task.cancel()
        await asyncio.sleep(0)
        assert not task.done()
        caller.leases[0].require_current()
        assert project.marker.read_bytes() == project.original_marker
        release.set()
        with pytest.raises(asyncio.CancelledError):
            await task
        assert len(project.calls) == 2 and project.calls[-1][-1] == "--apply"
        assert json.loads(project.marker.read_bytes())["version"] == "0.8.46"
        retained_reports(project, applied=True)
        assert len(caller.releases) == 1
        assert caller.releases[0]["marker"] == project.marker.read_bytes()
        assert [json.loads(report) for report in caller.releases[0]["apply_reports"]] == [
            native_upgrade_report(project.root, apply=True)]
    finally:
        release.set()
        await asyncio.gather(task, return_exceptions=True)
    with pytest.raises(RuntimeError, match="no longer current"):
        caller.leases[0].require_current()


def test_apply_boundary_preserves_original_callback_error(project):
    original = OSError("Synthetic original owner callback failure.")
    checks = []

    def reject():
        checks.append(True)
        if len(checks) == 2:
            raise original

    with storage.project_lock(project.root), pytest.raises(OSError) as caught:
        storage.upgrade_storage(project.root, "0.8.42", "0.8.46", require_owner_current=reject)
    assert caught.value is original and len(project.calls) == 1 and len(checks) == 2
    assert project.marker.read_bytes() == project.original_marker
    retained_reports(project, applied=False)


@pytest.mark.parametrize("invalid", ["async", "noncallable", "false"])
def test_invalid_owner_fence_refuses_native_read_and_keeps_private_state(project, invalid):
    entered = []

    async def asynchronous():
        entered.append(True)

    callback = {"async": asynchronous, "noncallable": object(), "false": lambda: False}[invalid]
    with storage.project_lock(project.root), pytest.raises(RuntimeError, match="owner|synchronous"):
        storage.upgrade_storage(project.root, "0.8.42", "0.8.46", require_owner_current=callback)
    assert not entered and not project.calls
    assert project.marker.read_bytes() == project.original_marker
    assert not list((project.root / ".mobkit/storage-upgrades").rglob("*.json"))
    assert all(path.read_bytes() == content and path.stat().st_ino == inode
               for path, (content, inode) in project.before.items())


def test_legacy_upgrade_without_callback_retains_native_apply_contract(project):
    with storage.project_lock(project.root):
        result = storage.upgrade_storage(project.root, "0.8.42", "0.8.46")
    assert result["applied"] is True and len(project.calls) == 2
    assert result["marker"] == json.loads(project.marker.read_bytes())
    assert result["marker"]["version"] == "0.8.46"
    retained_reports(project, applied=True)
