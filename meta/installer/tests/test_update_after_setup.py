"""Source updates of a product application after its normal setup lifecycle.

The original local setup fixture finishes Fresh, publishes its completed
Restore on a normal Stop and keeps the Fresh entry as lineage. Native outputs
are inert specimens; no application, provider or native runtime executes. The
real worker runs with inert process seams: it records application checks and
fails if it ever starts the application itself.
"""
import asyncio
from copy import deepcopy
import json
from pathlib import Path
from uuid import uuid4

import pytest

from launchkit import deployment, deployment_application_bootstrap as bootstrap
from launchkit import deployment_updates as updates, deployment_worker as worker
from launchkit.connect_installer_caller import CustodyError
from launchkit.projects import DraftConflict
from launchkit.templates.storage_setup import project_lock
from test_candidate_deployment_updates import packed
from test_deployment_application_bootstrap import inputs, no_runtime_operations, write
from test_deployment_updates import staged
from test_local_application_factory import (local, retain, prepare, source_bytes,
    retaining, prepared, implementation_service, workspace, selected, product,
    no_product_execution, no_processes)
from test_local_application_restore import local_restore, finalize_original, action, publish
from test_managed_model_startup import model_selection


def candidate_update(f, change=None):
    """The installed package plus one accepted application file, as a new build."""
    receipt = deployment.read_receipt(f.directory)
    _, previous = updates.checked_package(f.directory, "package.zip", receipt["package_sha256"])
    candidate = dict(previous)
    candidate["application/update_marker.py"] = (b"# Accepted application change after setup.\n", 0o644)
    if change is not None:
        change(candidate)
    report = json.loads(previous["implementation-report.json"][0])
    report.update(build_id=str(uuid4()),
        verification={**report.get("verification", {}), "digest": uuid4().hex + uuid4().hex})
    candidate["implementation-report.json"] = (json.dumps(report).encode(), 0o644)
    return candidate, report


async def completed_setup(f, monkeypatch):
    """Install, prepare, Fresh, normal Stop: the retained completed Restore."""
    await finalize_original(f, monkeypatch)
    stopped = await action(f, "stop")
    assert stopped["action_request"]["outcome"] == "returned"
    assert f.owner._restore_pin is not None
    return (f.directory / bootstrap.FRESH_ENTRY_NAME).read_bytes()


def owner_restore(f):
    """The trusted owner's published completed Restore pin, as Stage reads it."""
    pin = f.service._setup_update_restore(f.guide_id, f.directory.name)
    assert pin == f.owner._restore_pin.pin()
    return pin


def stage_candidate(f, change=None):
    candidate, report = candidate_update(f, change)
    staged = updates.stage(f.directory, packed(candidate), report, restore_selection=owner_restore(f))
    return staged["update"], report


def admit(f, update, mode="update"):
    """The user action's retained admission, as its worker reads it."""
    selection = updates.retain_update_admission(f.directory, update, mode)
    deployment.update_receipt(f.directory, update_action={
        "action": "apply-update" if mode == "update" else mode, "update_id": update["update_id"]})
    return selection


async def commit_candidate(f, monkeypatch):
    """A completed update as the real worker settles it after setup: no worker start."""
    update, _ = stage_candidate(f)
    worker_type = await run_worker(f, monkeypatch, "update", admit(f, update))
    assert worker_type.starts == []
    assert deployment.read_receipt(f.directory)["update"]["status"] == "completed"
    return update["update_id"]


def accept_candidate(f, monkeypatch, report):
    """The accepted-build fences of Apply select this inert candidate build."""
    implementation = f.service.implementation
    original = implementation.status

    async def status(guide_id, *args, **kwargs):
        value = await original(guide_id, *args, **kwargs)
        return {**value, "build_id": report["build_id"], "source_digest": report["verification"]["digest"]}

    monkeypatch.setattr(implementation, "status", status)
    monkeypatch.setattr(implementation, "selected_build_id", lambda guide_id: report["build_id"])


def new_lifecycle(f):
    """As DeploymentService._launch does, every worker owns a new lifecycle."""
    lifecycle = str(uuid4())
    deployment.update_receipt(f.directory, interactive_lifecycle_id=lifecycle,
        interactive_cleanup_attempt_id=None, interactive_cleanup=None)
    return lifecycle


async def run_worker(f, monkeypatch, mode, selection, *, failing_builds=(), lifecycle=None):
    """The real interactive worker entry with inert application checks; it must never start the app.

    Only this run sees the inert Supervisor seams; owners keep the real class.
    No worker process runs here, so the fixed wrapper's confirmation of its
    exit is an inert specimen, as DeploymentService._run records it.
    """
    class SetupWorker(worker.Supervisor):
        checks, starts = [], []

        async def command(self, args, *other, **kwargs):
            build = deployment.read_receipt(self.directory)["build_id"]
            SetupWorker.checks.append((build, args[-1]))
            if build in failing_builds:
                raise RuntimeError("The inert application contract check failed.")

        async def start(self, *args, **kwargs):
            SetupWorker.starts.append(args)
            raise AssertionError("After setup, the update worker never starts the application.")

    async def owned_input(supervisor):
        await supervisor.stop.wait()
        return "stopped"

    lifecycle = new_lifecycle(f) if lifecycle is None else lifecycle
    with monkeypatch.context() as patch:
        patch.setattr(worker, "Supervisor", SetupWorker)
        patch.setattr(worker, "owner_input", owned_input)
        SetupWorker.result = await deployment.interactive_worker(f.directory, mode, lifecycle,
            selection["path"], selection["sha256"])
    # The entry ran in this test process; the fixture's inert worker process
    # stands for the one whose exit the wrapper confirms.
    cleanup = deployment.read_receipt(f.directory)["interactive_cleanup"]
    deployment.update_receipt(f.directory, interactive_cleanup={**cleanup,
        "supervisor_pid": f.process.pid, "worker_exit_confirmed": True})
    return SetupWorker


async def owner_start(f):
    """The next user Start: the owner's existing Restore admission."""
    f.service.launched.clear()
    started = await action(f, "finish-setup")
    assert started["action_request"]["outcome"] == "returned", started["action_request"]
    directory, mode, options = f.service.launched[-1]
    assert mode == "start"
    admitted = bootstrap.read_bootstrap_admission(directory, options["bootstrap_selection"])
    assert admitted["application_startup"] == json.loads(f.owner._restore_pin.read())
    # Native execution is outside this fixture; the launched runtime stopped.
    deployment.update_receipt(f.directory, status="stopped")
    return options["bootstrap_selection"]


async def test_completed_setup_stages_an_accepted_source_update(local_restore, monkeypatch):
    f = local_restore
    entry = await completed_setup(f, monkeypatch)
    update, _ = stage_candidate(f)
    assert update["status"] == "staged" and update["blockers"] == []
    pin = f.owner._restore_pin.pin()
    assert update["setup_restore_sha256"] == pin["sha256"]
    root = updates.update_directory(f.directory, update["update_id"])
    assert updates.setup_restore_pin(f.directory, update) == pin
    assert (root / updates.SETUP_RESTORE_NAME).stat().st_mode & 0o077 == 0
    # Status projections carry only the digest, never the private pin path.
    status = await f.service.status(f.guide_id)
    assert status["update"]["setup_restore_sha256"] == pin["sha256"]
    assert pin["path"] not in json.dumps(status)
    assert (f.directory / bootstrap.FRESH_ENTRY_NAME).read_bytes() == entry


async def test_entered_fresh_without_a_completed_restore_still_refuses_stage(local_restore, monkeypatch):
    f = local_restore
    await finalize_original(f, monkeypatch)
    assert f.owner._restore_pin is None
    assert f.service._setup_update_restore(f.guide_id, f.directory.name) is None
    candidate, report = candidate_update(f)
    with pytest.raises(CustodyError, match="retains a Fresh bootstrap attempt"):
        updates.stage(f.directory, packed(candidate), report)
    with pytest.raises(CustodyError, match="retains a Fresh bootstrap attempt"):
        updates.stage(f.directory, packed(candidate), report, restore_selection=f.fresh_pin.pin())
    assert deployment.read_receipt(f.directory).get("update") is None


@pytest.mark.parametrize("defect", ["changed-restore", "removed-restore", "replacement-history", "native-team"])
async def test_completed_setup_stage_keeps_custody_refusals(local_restore, monkeypatch, defect):
    f = local_restore
    await completed_setup(f, monkeypatch)
    pin = owner_restore(f)
    if defect == "changed-restore":
        path = f.owner._restore_pin.path
        path.write_bytes(path.read_bytes().replace(b"}", b' }', 1))
    elif defect == "removed-restore":
        f.owner._restore_pin.path.unlink()
    elif defect == "replacement-history":
        root = f.directory / "updates" / str(uuid4())
        root.mkdir(parents=True, mode=0o700)
        (root / "application-replacement-attempt.json").write_text("retained native attempt\n")
    else:
        deployment.update_receipt(f.directory, source_kind="native-team")
    candidate, report = candidate_update(f)
    with pytest.raises((CustodyError, DraftConflict, ValueError)) as refused:
        updates.stage(f.directory, packed(candidate), report, restore_selection=pin)
    if defect.endswith("-restore"):
        assert str(refused.value) == bootstrap.SETUP_RESTORE_CHANGED
    assert (deployment.read_receipt(f.directory).get("update") or {}).get("status") != "staged"


def changed_policy(candidate):
    policy = json.loads(candidate["application/tool-policy.json"][0])
    policy["revision"] = policy.get("revision", 1) + 1
    candidate["application/tool-policy.json"] = (json.dumps(policy).encode(), 0o644)


def changed_definition(candidate):
    content, mode = candidate["config/mob.toml"]
    candidate["config/mob.toml"] = (content + b"\n# A changed native definition after setup.\n[skills.added]\n"
        b"source = \"inline\"\ncontent = \"An added native skill.\"\n", mode)


@pytest.mark.parametrize("change", [changed_policy, changed_definition], ids=["tool-policy", "mob-definition"])
async def test_setup_update_that_changes_native_inputs_is_blocked_at_stage(local_restore, monkeypatch, change):
    f = local_restore
    await completed_setup(f, monkeypatch)
    update, _ = stage_candidate(f, change)
    assert update["status"] == "blocked" and updates.SETUP_REPLACEMENT_BLOCKER in update["blockers"]
    with pytest.raises(DraftConflict, match="cannot be activated"):
        updates.activate(f.directory, update["update_id"])
    assert deployment.read_receipt(f.directory)["build_id"] == update["from_build_id"]


async def test_apply_and_worker_install_a_set_up_update_that_the_owner_then_starts(local_restore, monkeypatch):
    f = local_restore
    entry = await completed_setup(f, monkeypatch)
    update, report = stage_candidate(f)
    accept_candidate(f, monkeypatch, report)
    f.service.launched.clear()
    applied = await f.service.action(f.guide_id, f.directory.name, "apply-update", update["update_id"])
    assert applied["update"]["status"] == "staged"
    directory, mode, options = f.service.launched[-1]
    assert (directory, mode) == (f.directory, "update")
    worker_type = await run_worker(f, monkeypatch, "update", options["update_selection"])
    assert worker_type.result is None
    receipt = deployment.read_receipt(f.directory)
    assert receipt["status"] == "stopped" and receipt["build_id"] == report["build_id"]
    assert receipt["update"]["status"] == "completed" and receipt["update"]["rollback_available"] is True
    assert "native bootstrap" not in receipt["update"]["detail"]
    assert "The application is stopped. Start restores its completed setup" in receipt["update"]["detail"]
    assert worker_type.checks == [(report["build_id"], "--application-check")]
    assert worker_type.starts == []
    # The next Start is the owner's Restore through lineage, then a later
    # Forge restart re-publishes the same Restore from the original finalizer
    # history at Stop, and Start admits it again.
    first = await owner_start(f)
    retained = f.owner._restore_pin
    monkeypatch.setattr(f.owner, "_restore_pin", None)
    republished = await publish(f)
    assert republished == retained and f.owner._restore_pin == retained
    second = await owner_start(f)
    assert first != second
    assert (f.directory / bootstrap.FRESH_ENTRY_NAME).read_bytes() == entry


async def test_failed_contract_check_restores_previous_source_without_a_start(local_restore, monkeypatch):
    f = local_restore
    await completed_setup(f, monkeypatch)
    update, report = stage_candidate(f)
    selection = admit(f, update)
    worker_type = await run_worker(f, monkeypatch, "update", selection, failing_builds={report["build_id"]})
    assert worker_type.result is None
    receipt = deployment.read_receipt(f.directory)
    assert receipt["status"] == "stopped" and receipt["build_id"] == update["from_build_id"]
    assert receipt["update"]["status"] == "rolled-back" and receipt["update"]["rollback_available"] is False
    assert "native bootstrap" not in receipt["update"]["detail"]
    assert worker_type.checks == [(report["build_id"], "--application-check"),
        (update["from_build_id"], "--application-check")]
    assert worker_type.starts == []
    await owner_start(f)


async def test_manual_rollback_after_setup_restores_without_a_start(local_restore, monkeypatch):
    f = local_restore
    await completed_setup(f, monkeypatch)
    identity = await commit_candidate(f, monkeypatch)
    previous = deployment.read_receipt(f.directory)["update"]["from_build_id"]
    f.service.launched.clear()
    await f.service.action(f.guide_id, f.directory.name, "rollback", identity)
    directory, mode, options = f.service.launched[-1]
    assert (directory, mode) == (f.directory, "rollback")
    worker_type = await run_worker(f, monkeypatch, "rollback", options["update_selection"])
    assert worker_type.result is None
    receipt = deployment.read_receipt(f.directory)
    assert receipt["update"]["status"] == "rolled-back" and receipt["build_id"] == previous
    assert worker_type.checks == [(previous, "--application-check")] and worker_type.starts == []
    await owner_start(f)


async def test_recover_current_after_setup_settles_its_own_unfinished_transaction(local_restore, monkeypatch):
    f = local_restore
    await completed_setup(f, monkeypatch)
    update, report = stage_candidate(f)
    admit(f, update)
    updates.activate(f.directory, update["update_id"])
    _, root, current = updates.transaction(f.directory, update["update_id"])
    updates.checkpoint(f.directory, root, current, status="recovery-needed", rollback_available=True)
    assert deployment.read_receipt(f.directory)["build_id"] == report["build_id"]
    # Only the recovery's own transaction may be read as lineage before it
    # completes. Another Start of this candidate refuses.
    with pytest.raises(CustodyError, match="has not finished"):
        await owner_start(f)
    accept_candidate(f, monkeypatch, report)
    f.service.launched.clear()
    await f.service.action(f.guide_id, f.directory.name, "recover-current", update["update_id"])
    directory, mode, options = f.service.launched[-1]
    assert (directory, mode) == (f.directory, "recover-current")
    worker_type = await run_worker(f, monkeypatch, "recover-current", options["update_selection"])
    assert worker_type.result is None
    receipt = deployment.read_receipt(f.directory)
    assert receipt["update"]["status"] == "completed" and receipt["build_id"] == report["build_id"]
    assert worker_type.checks == [(report["build_id"], "--application-check")] and worker_type.starts == []
    await owner_start(f)


@pytest.mark.parametrize("status", ["verifying", "applying"])
async def test_start_of_an_unfinished_candidate_refuses_with_recovery_text(local_restore, monkeypatch, status):
    f = local_restore
    await completed_setup(f, monkeypatch)
    update, _ = stage_candidate(f)
    admit(f, update)
    updates.activate(f.directory, update["update_id"])
    if status != "verifying":
        _, root, current = updates.transaction(f.directory, update["update_id"])
        updates.checkpoint(f.directory, root, current, status=status)
    f.service.launched.clear()
    with pytest.raises(CustodyError) as refused:
        await action(f, "finish-setup")
    assert str(refused.value) == bootstrap.UNFINISHED_UPDATE
    assert not f.service.launched


async def test_in_flight_lineage_belongs_only_to_its_own_transaction(local_restore, monkeypatch):
    f = local_restore
    await completed_setup(f, monkeypatch)
    pin = owner_restore(f)
    update, _ = stage_candidate(f)
    admit(f, update)
    updates.activate(f.directory, update["update_id"])
    binding = updates.update_binding(update)
    assert bootstrap.require_update_bootstrap_history(f.directory, pin, in_flight=binding)["kind"] == "restore"
    other = {**binding, "update_id": str(uuid4())}
    for in_flight in (None, other):
        with pytest.raises(CustodyError) as refused:
            bootstrap.require_update_bootstrap_history(f.directory, pin, in_flight=in_flight)
        assert str(refused.value) == bootstrap.UNFINISHED_UPDATE


@pytest.mark.parametrize("change", ["changed-restore", "removed-restore", "removed-record"])
async def test_worker_rereads_the_recorded_restore_before_changing_source(local_restore, monkeypatch, change):
    f = local_restore
    await completed_setup(f, monkeypatch)
    update, _ = stage_candidate(f)
    selection = admit(f, update)
    if change == "changed-restore":
        path = f.owner._restore_pin.path
        path.write_bytes(path.read_bytes().replace(b"}", b' }', 1))
    elif change == "removed-restore":
        f.owner._restore_pin.path.unlink()
    else:
        (updates.update_directory(f.directory, update["update_id"]) / updates.SETUP_RESTORE_NAME).unlink()
    lifecycle = new_lifecycle(f)
    before = source_bytes(f.destination), deployment.read_receipt(f.directory)
    # The refusal comes before the worker admits its Supervisor or any change.
    with pytest.raises(CustodyError) as refused:
        await run_worker(f, monkeypatch, "update", selection, lifecycle=lifecycle)
    assert str(refused.value) == bootstrap.SETUP_RESTORE_CHANGED
    assert (source_bytes(f.destination), deployment.read_receipt(f.directory)) == before


async def test_apply_requires_the_owner_restore_recorded_at_stage(local_restore, monkeypatch):
    f = local_restore
    await completed_setup(f, monkeypatch)
    update, report = stage_candidate(f)
    accept_candidate(f, monkeypatch, report)
    # The owner now publishes another completed Restore than Stage recorded.
    path = f.owner._restore_pin.path
    other = path.with_name("other-restore-selection.json")
    other.write_bytes(path.read_bytes().replace(b"}", b' }', 1))
    other.chmod(0o600)
    monkeypatch.setattr(f.owner, "_restore_pin", type(f.owner._restore_pin)(other,
        updates.digest(other.read_bytes())))
    monkeypatch.setattr(f.owner, "_restore_value", json.loads(other.read_bytes()))
    f.service.launched.clear()
    with pytest.raises(DraftConflict) as refused:
        await f.service.action(f.guide_id, f.directory.name, "apply-update", update["update_id"])
    assert str(refused.value) == bootstrap.SETUP_RESTORE_CHANGED
    assert not f.service.launched
    assert deployment.read_receipt(f.directory)["update"]["status"] == "staged"


async def test_next_start_after_a_committed_update_authenticates_the_original_restore(local_restore, monkeypatch):
    f = local_restore
    entry = await completed_setup(f, monkeypatch)
    await commit_candidate(f, monkeypatch)
    await owner_start(f)
    assert (f.directory / bootstrap.FRESH_ENTRY_NAME).read_bytes() == entry


async def test_two_completed_updates_chain_from_the_original_setup(local_restore, monkeypatch):
    f = local_restore
    await completed_setup(f, monkeypatch)
    await commit_candidate(f, monkeypatch)
    await commit_candidate(f, monkeypatch)
    await owner_start(f)
    # The walk is bounded; a longer history than its bound refuses.
    monkeypatch.setattr(bootstrap, "MAX_LINEAGE_UPDATES", 1)
    with pytest.raises(CustodyError) as refused:
        await action(f, "finish-setup")
    assert str(refused.value) == bootstrap.LINEAGE_CHANGED


def forge(f, identity, forgery):
    root = updates.update_directory(f.directory, identity)
    if forgery == "foreign-previous":
        journal = json.loads((root / "update.json").read_text())
        journal["previous_package_sha256"] = "f" * 64
        (root / "update.json").write_text(json.dumps(journal))
    elif forgery == "missing-journal":
        for path in sorted(root.rglob("*"), reverse=True):
            path.unlink() if path.is_file() else path.rmdir()
        root.rmdir()
    elif forgery in {"foreign-installation", "installation-key"}:
        # A self-consistent journal and packages, admitted for another
        # installation: the copied admission binds that installation.
        for path in root.glob("admission-*.json"):
            value = json.loads(path.read_bytes())
            if forgery == "foreign-installation":
                value["installation"]["destination"] = str(f.destination.parent / "another installation")
            else:
                value["installation"]["execution_mode"] = "background"
            path.write_bytes(json.dumps(value, sort_keys=True, separators=(",", ":")).encode())
    elif forgery == "rolled-back-edge":
        _, _, current = updates.transaction(f.directory, identity)
        updates.checkpoint(f.directory, root, current, status="rolled-back")
    else:
        candidate = updates.checked_package(root, "candidate.zip", json.loads(
            (root / "update.json").read_text())["package_sha256"])[1]
        report = json.loads(candidate["implementation-report.json"][0])
        report["build_id"] = str(uuid4())
        candidate["implementation-report.json"] = (json.dumps(report).encode(), 0o644)
        payload = packed(candidate)
        (root / "candidate.zip").write_bytes(payload)
        journal = json.loads((root / "update.json").read_text())
        journal["package_sha256"] = updates.digest(payload)
        (root / "update.json").write_text(json.dumps(journal))


@pytest.mark.parametrize("forgery", ["foreign-previous", "missing-journal", "foreign-installation",
    "installation-key", "rolled-back-edge", "foreign-report"])
async def test_next_start_refuses_a_forged_update_lineage(local_restore, monkeypatch, forgery):
    f = local_restore
    await completed_setup(f, monkeypatch)
    identity = await commit_candidate(f, monkeypatch)
    forge(f, identity, forgery)
    f.service.launched.clear()
    with pytest.raises((CustodyError, DraftConflict)):
        await action(f, "finish-setup")
    assert not f.service.launched


@pytest.mark.parametrize("forgery", ["foreign-installation", "foreign-report"])
async def test_a_verified_lineage_edge_is_verified_again_after_it_changes(local_restore, monkeypatch, forgery):
    f = local_restore
    await completed_setup(f, monkeypatch)
    identity = await commit_candidate(f, monkeypatch)
    await owner_start(f)
    edge = str(updates.update_directory(f.directory, identity))
    assert any(key[1] == edge for key in bootstrap._verified_edges)
    forge(f, identity, forgery)
    f.service.launched.clear()
    with pytest.raises((CustodyError, DraftConflict)):
        await action(f, "finish-setup")
    assert not f.service.launched


async def test_a_memoized_lineage_edge_still_verifies_the_journal_the_walk_read(local_restore, monkeypatch):
    f = local_restore
    await completed_setup(f, monkeypatch)
    identity = await commit_candidate(f, monkeypatch)
    await owner_start(f)
    edge = updates.update_directory(f.directory, identity)
    assert any(key[1] == str(edge) for key in bootstrap._verified_edges)
    # The journal a walk reads can differ from the bytes the memo observed
    # (a racing write). Only the edge verification compares source_digest.
    original = bootstrap._lineage_journals

    def raced(directory, in_flight):
        edges = original(directory, in_flight)
        for rows in edges.values():
            for index, (child, journal) in enumerate(rows):
                if child == edge:
                    rows[index] = (child, {**journal, "source_digest": "0" * 64})
        return edges

    monkeypatch.setattr(bootstrap, "_lineage_journals", raced)
    f.service.launched.clear()
    with pytest.raises(CustodyError) as refused:
        await action(f, "finish-setup")
    assert str(refused.value) == bootstrap.LINEAGE_CHANGED
    assert not f.service.launched


async def test_gap_in_a_two_update_chain_refuses(local_restore, monkeypatch):
    f = local_restore
    await completed_setup(f, monkeypatch)
    first = await commit_candidate(f, monkeypatch)
    await commit_candidate(f, monkeypatch)
    forge(f, first, "missing-journal")
    f.service.launched.clear()
    with pytest.raises(CustodyError) as refused:
        await action(f, "finish-setup")
    assert str(refused.value) == bootstrap.LINEAGE_CHANGED
    assert not f.service.launched


async def test_pre_stage_bootstrap_admission_is_not_reused_after_stage(local_restore, monkeypatch):
    f = local_restore
    await completed_setup(f, monkeypatch)
    before = await owner_start(f)
    stage_candidate(f)
    with pytest.raises(CustodyError, match="installation changed"):
        bootstrap.read_bootstrap_admission(f.directory, before)
    with pytest.raises(CustodyError, match="installation changed"):
        bootstrap.require_bootstrap_history_settled(f.directory, before)
    with pytest.raises(CustodyError, match="installation changed"):
        bootstrap.BootstrapFactory(f.directory, before)
    # A Start while the update is only staged admits a new Restore admission.
    after = await owner_start(f)
    assert after != before


async def test_apply_with_an_action_request_on_a_host_without_a_replacement_owner(local_restore, monkeypatch):
    """A selected host without a replacement supplier binds no replacement owner.

    Apply with the browser's action request UUID then takes the same update
    path as without one: the worker installs the set-up application's update.
    """
    f = local_restore
    assert f.service.application_replacement_selection is None
    await completed_setup(f, monkeypatch)
    update, report = stage_candidate(f)
    accept_candidate(f, monkeypatch, report)
    f.service.launched.clear()
    applied = await f.service.action(f.guide_id, f.directory.name, "apply-update", update["update_id"],
        action_request_id=str(uuid4()))
    assert applied["action_request"]["outcome"] == "returned", applied["action_request"]
    directory, mode, options = f.service.launched[-1]
    assert (directory, mode) == (f.directory, "update")
    worker_type = await run_worker(f, monkeypatch, "update", options["update_selection"])
    assert worker_type.starts == [] and worker_type.checks == [(report["build_id"], "--application-check")]
    assert deployment.read_receipt(f.directory)["update"]["status"] == "completed"


@pytest.mark.parametrize("with_request", [False, True], ids=["no-action-request", "action-request"])
async def test_apply_before_the_first_setup_refuses_on_a_host_without_a_replacement_owner(local, monkeypatch,
        with_request):
    """A Meta Toolkit application before its first setup has no update worker that can start it.

    On a selected host without a replacement owner, Apply refuses with guidance
    before any worker is queued: no failed receipt and no recovery-needed
    transaction. The installed source and the staged update stay as they were.
    """
    f = local
    f.select()
    f.service.application_setup_continuation = f.host.continue_application_startup
    await retain(f)
    assert f.service.application_replacement_selection is None
    candidate, report = candidate_update(f)
    update = updates.stage(f.directory, packed(candidate), report)["update"]
    assert update["status"] == "staged" and "setup_restore_sha256" not in update
    accept_candidate(f, monkeypatch, report)
    keys = ("status", "build_id", "package_sha256", "update", "update_action")
    before = deployment.read_receipt(f.directory)
    f.service.launched.clear()
    options = {"action_request_id": str(uuid4())} if with_request else {}
    with pytest.raises(DraftConflict, match="has not finished its setup, so it cannot take a staged update in place yet"):
        await f.service.action(f.guide_id, f.directory.name, "apply-update", update["update_id"], **options)
    assert not f.service.launched
    after = deployment.read_receipt(f.directory)
    assert {key: after.get(key) for key in keys} == {key: before.get(key) for key in keys}


@pytest.mark.parametrize("with_request", [False, True], ids=["no-action-request", "action-request"])
async def test_apply_in_a_forge_without_a_selected_host_takes_the_ordinary_update_path(tmp_path, with_request):
    """A Forge started from source selects no application host and no replacement owner."""
    service, directory, _, previous, _, identity = await staged(tmp_path)
    assert service.application_replacement_selection is None
    options = {"action_request_id": str(uuid4())} if with_request else {}
    try:
        await service.action(service.guide.identity, previous["deployment_id"], "apply-update",
            update_id=identity, **options)
        assert deployment.read_receipt(directory)["update_action"] == {"action": "apply-update", "update_id": identity}
        assert directory.name in service.tasks
    finally:
        await service.shutdown()


async def test_update_staged_before_the_first_setup_keeps_that_setup_available(local, monkeypatch):
    """Stage right after Install, then run the first setup.

    The install-time Supervisor and the Toolkit preparation owner are the real
    ones. A staged update changes nothing installed, so the first setup stays
    available: status offers it, Prepare retains its Fresh draft and Finish
    starts it with the installed source. The update stays staged.
    """
    f = local
    f.select()
    f.service.application_setup_continuation = f.host.continue_application_startup
    inputs = await retain(f)
    installed = deployment.read_receipt(f.directory)
    candidate, report = candidate_update(f)
    update = updates.stage(f.directory, packed(candidate), report)["update"]
    assert update["status"] == "staged" and "setup_restore_sha256" not in update
    # Each read and Prepare see the same original stopped owner, every time.
    for _ in range(2):
        status = await f.host.setup_status(f.service, f.guide_id, f.directory.name)
        assert status["available"] and status["action"] == "prepare-setup", status.get("detail")
    fresh = await prepare(f, monkeypatch)
    assert json.loads(fresh.read())["kind"] == "fresh"
    assert f.host._installation_supervisor is inputs.supervisor
    f.service.launched.clear()
    started = await action(f, "finish-setup")
    assert started["action_request"]["outcome"] == "returned", started["action_request"]
    directory, mode, options = f.service.launched[-1]
    assert (directory, mode) == (f.directory, "start")
    admitted = bootstrap.read_bootstrap_admission(directory, options["bootstrap_selection"])
    assert admitted["application_startup"] == json.loads(fresh.read())
    receipt = deployment.read_receipt(f.directory)
    assert (receipt["build_id"], receipt["package_sha256"]) == (installed["build_id"], installed["package_sha256"])
    assert (receipt["update"]["update_id"], receipt["update"]["status"]) == (update["update_id"], "staged")


@pytest.mark.parametrize("with_request", [False, True], ids=["no-action-request", "action-request"])
async def test_apply_after_the_first_setup_asks_to_stage_an_update_staged_before_it_again(local_restore,
        monkeypatch, with_request):
    """Stage before the first setup, then finish that setup and Stop normally.

    Stage recorded no completed Restore, so Apply refuses with stage-again
    guidance before anything changes, not with the Fresh refusal's reconcile
    guidance. The next Stage records the owner's completed Restore.
    """
    f = local_restore
    candidate, report = candidate_update(f)
    update = updates.stage(f.directory, packed(candidate), report)["update"]
    assert update["status"] == "staged" and "setup_restore_sha256" not in update
    await completed_setup(f, monkeypatch)
    accept_candidate(f, monkeypatch, report)
    keys = ("status", "build_id", "package_sha256", "update", "update_action")
    before = deployment.read_receipt(f.directory)
    assert before["update"]["update_id"] == update["update_id"]
    f.service.launched.clear()
    options = {"action_request_id": str(uuid4())} if with_request else {}
    with pytest.raises(DraftConflict) as refused:
        await f.service.action(f.guide_id, f.directory.name, "apply-update", update["update_id"], **options)
    assert str(refused.value) == bootstrap.UPDATE_STAGED_BEFORE_SETUP
    assert not f.service.launched
    after = deployment.read_receipt(f.directory)
    assert {key: after.get(key) for key in keys} == {key: before.get(key) for key in keys}
    restaged = updates.stage(f.directory, packed(candidate), report, restore_selection=owner_restore(f))["update"]
    assert restaged["status"] == "staged" and restaged["setup_restore_sha256"] == f.owner._restore_pin.pin()["sha256"]


async def test_apply_of_an_update_staged_before_an_unfinished_first_setup_keeps_the_fresh_refusal(local_restore,
        monkeypatch):
    """Fresh entered without a completed Restore: Apply keeps the Fresh refusal."""
    f = local_restore
    candidate, report = candidate_update(f)
    update = updates.stage(f.directory, packed(candidate), report)["update"]
    await finalize_original(f, monkeypatch)
    assert f.service._setup_update_restore(f.guide_id, f.directory.name) is None
    accept_candidate(f, monkeypatch, report)
    f.service.launched.clear()
    with pytest.raises(CustodyError) as refused:
        await f.service.action(f.guide_id, f.directory.name, "apply-update", update["update_id"],
            action_request_id=str(uuid4()))
    assert str(refused.value) == bootstrap.FRESH_RETAINED
    assert not f.service.launched
    assert deployment.read_receipt(f.directory)["update"]["status"] == "staged"


async def test_ordinary_update_keeps_its_doctor_check_and_worker_start(tmp_path):
    _, directory, _, previous, _, identity = await staged(tmp_path)
    calls = []

    class Ready(worker.Supervisor):
        async def command(self, args, *other, **kwargs):
            calls.append(args[-1])

        async def start(self, definition, *, on_ready):
            calls.append("start")
            on_ready()

    await worker.update_runtime(Ready(directory), "update", update_id=identity)
    receipt = deployment.read_receipt(directory)
    assert calls == ["--doctor", "--application-check", "start"]
    assert receipt["update"]["status"] == "completed" and "setup_restore_sha256" not in receipt["update"]
    assert receipt["update"]["detail"].startswith("The accepted update passed native bootstrap and live Console health.")


@pytest.mark.parametrize("change", [None, "removed", "other-root"])
async def test_update_after_setup_keeps_the_original_model_configuration(inputs, monkeypatch, change):
    selection = model_selection(inputs.private, monkeypatch)
    inputs.fresh["model_configuration"] = selection
    admission, _ = inputs.admit("fresh")
    factory = bootstrap.BootstrapFactory(inputs.directory, admission)
    with project_lock(inputs.destination) as lease:
        async with factory(lease):
            pass
    # Only private finite lineage is exercised. These are inert output files,
    # not a qualified native finalization or runtime Restore.
    bundle = Path(inputs.fresh["bundle_path"])
    bundle.mkdir(mode=0o700)
    activation = write(Path(inputs.fresh["activation_path"]), {"inert": "activation"})
    policy = write(bundle / "policy.json", {"inert": "policy"})
    setup = write(bundle / "setup.json", {"inert": "completed setup"})
    manifest = write(bundle / "manifest.json", {"artifacts": [
        {"path": policy.path.name, "sha256": policy.sha256},
        {"path": setup.path.name, "sha256": setup.sha256}]})
    selected = write(inputs.private / "actual-restore.json", {"deployment_root": str(inputs.destination),
        "activation": activation.pin(), "policy_manifest": manifest.pin(), "local_policy": policy.pin()})
    restored = {"kind": "restore", "gateway": inputs.fresh["gateway"],
        "preparation_plan": inputs.fresh["preparation_plan"], "selection_file": selected.pin(),
        "completed_setup": setup.pin(), "model_configuration": deepcopy(selection)}
    if change == "removed":
        restored.pop("model_configuration")
    elif change == "other-root":
        other = inputs.private / "other-native-root"
        other.mkdir(mode=0o700)
        restored["model_configuration"]["credential_config_root"] = str(other)
    owner = write(inputs.private / "owner-restore.json", restored)
    f = type("Installed", (), {"directory": inputs.directory})
    candidate, report = candidate_update(f)
    if change is not None:
        with pytest.raises(CustodyError, match="original Fresh model configuration"):
            updates.stage(inputs.directory, packed(candidate), report, restore_selection=owner.pin())
        return
    update = updates.stage(inputs.directory, packed(candidate), report, restore_selection=owner.pin())["update"]
    assert update["status"] == "staged" and update["blockers"] == []
    updates.retain_update_admission(inputs.directory, update, "update")
    updates.activate(inputs.directory, update["update_id"])
    updates.committed(inputs.directory, update["update_id"], started=False)
    assert bootstrap.require_update_bootstrap_history(inputs.directory, owner.pin()) == restored
