"""Original Install completion with inert package/prefix and worker evidence.

The reused fixture forbids actual processes, SDK imports and network access.
No native inventory, Fresh completion, real factory or backend is activated.
"""
import asyncio
from copy import deepcopy
from dataclasses import replace
import hashlib
import json
import logging
from pathlib import Path
from types import SimpleNamespace

import pytest

from launchkit import deployment, deployment_worker
from launchkit.connect_installer_caller import CustodyError, ExactFile
from launchkit.forge_application_host import ApplicationPreparationInputs, ForgeApplicationHost
from launchkit.projects import DraftConflict
from test_deployment_application_preparation import (prepared, implementation_service,
    workspace, selected, product, no_product_execution, no_processes)
from test_forge_application_host_server import api, select


@pytest.fixture
def retaining(prepared, monkeypatch):
    f = prepared
    f.held = None
    f.retainer_calls = []
    def supplier(guide, installation):
        assert (guide, installation) == (f.guide_id, f.receipt["deployment_id"])
        return f.held
    def retain(guide, installation, supervisor):
        assert f.service.lock.locked()
        assert (guide, installation) == (f.guide_id, f.receipt["deployment_id"])
        assert installation not in f.service.tasks and installation not in f.service.processes
        assert supervisor is not f.supervisor and supervisor.child is None and not supervisor.stop.is_set()
        f.retainer_calls.append(supervisor)
        f.held = ApplicationPreparationInputs(**{name: value for name, value in
            {**f.kwargs, "supervisor": supervisor}.items() if name != "application_host"})
        return f.held
    def startup(guide, installation):
        assert (guide, installation) == (f.guide_id, f.receipt["deployment_id"])
        return ExactFile(f.selection_path, hashlib.sha256(f.selection_path.read_bytes()).hexdigest())
    f.host = ForgeApplicationHost(guide_id=f.guide_id, runtime_target=f.target,
        acquisition_selection=f.acquisition, viewer_selection=f.viewer,
        require_current=f.host._require_owner_current, startup_selection=startup,
        preparation_inputs=supplier, retain_preparation_inputs=retain)
    f.service.application_setup_retention = f.host.retain_installation_inputs
    f.service.application_setup_selection = f.host._selected_startup
    f.service.application_setup_preparation = f.host.prepare_application
    f.exited = asyncio.Event()
    f.release = asyncio.Event()
    f.exit_code = 0
    f.cleanup_boundary = "owned-process-groups-absent"
    f.process = SimpleNamespace(pid=981237, returncode=None, stdin=None)
    async def wait():
        await f.release.wait()
        receipt = deployment.read_receipt(f.directory)
        cleanup = {name: receipt.get(name) for name in ("guide_id", "deployment_id", "build_id",
            "guide_version", "blueprint_hash", "destination", "package_sha256")}
        cleanup.update(deployment.native_fields(receipt))
        cleanup.update(deployment.interactive_source_binding(f.directory, receipt))
        cleanup.update(confirmed=True, worker_exit_confirmed=False,
            lifecycle_id=receipt["interactive_lifecycle_id"], startup_attempt_id=None,
            supervisor_pid=f.process.pid, return_code=f.exit_code, boundary=f.cleanup_boundary)
        deployment.update_receipt(f.directory, status="stopped", phase="configure",
            runtime_pid=None, process_live=False, interactive_cleanup=cleanup,
            installed_candidate_runtime_proof={"fixture": "inert-worker-completion-not-runtime-qualification"})
        f.process.returncode = f.exit_code
        f.exited.set()
        return f.exit_code
    f.process.wait = wait
    async def spawn(*args, **kwargs):
        return f.process
    monkeypatch.setattr(asyncio, "create_subprocess_exec", spawn)
    def absent(pid, sig):
        assert (pid, sig) == (f.process.pid, 0)
        raise ProcessLookupError
    monkeypatch.setattr(deployment.os, "kill", absent)
    def launch():
        deployment.DeploymentService._launch(f.service, f.directory, "install")
        return f.service.tasks[f.directory.name]
    f.launch = launch
    return f


async def finished(f):
    task = f.launch()
    f.release.set()
    await asyncio.wait_for(task, 5)
    return task


async def waiting(f):
    await f.service.lock.acquire()
    task = f.launch()
    f.release.set()
    await asyncio.wait_for(f.exited.wait(), 5)
    # The worker has exited, but the exact completion task remains owned.
    await asyncio.sleep(0)
    assert f.service.tasks[f.directory.name] is task
    assert f.service.processes[f.directory.name] is f.process
    assert not f.retainer_calls
    return task


async def test_successful_install_retains_one_exact_supplier_without_preparing(retaining, monkeypatch):
    f = retaining
    await finished(f)
    receipt = deployment.read_receipt(f.directory)
    assert receipt["status"] == "stopped" and receipt["phase"] == "configure"
    assert deployment.interactive_cleanup_matches(receipt)
    assert len(f.retainer_calls) == 1 and f.held.supervisor is f.retainer_calls[0]
    assert f.host._installation_supervisor is f.held.supervisor
    assert f.host._retained_preparation_inputs is f.held
    assert f.service.tasks == {} and f.service.processes == {}
    assert not f.calls and not f.plan_path.exists() and not f.selection_path.exists()
    assert not f.inventory_attempt.exists() and not Path(f.paths["provision_attempt_path"]).exists()
    def no_constructor(*args):
        pytest.fail("Read-only setup cannot construct a Supervisor.")
    monkeypatch.setattr(deployment_worker.Supervisor, "__init__", no_constructor)
    ready = await f.host.setup_status(f.service, f.guide_id, f.directory.name)
    assert ready["available"] and ready["action"] == "prepare-setup"
    assert not f.calls and len(f.retainer_calls) == 1
    with pytest.raises(CustodyError, match="only once"):
        async with f.service.lock:
            f.host.retain_installation_inputs(f.service, f.guide_id, f.directory.name)


async def test_stop_joins_original_completion_waiter_without_lock_reentry(retaining):
    f = retaining
    task = await waiting(f)
    try:
        result = await asyncio.wait_for(f.service._action_locked(
            f.guide_id, f.directory.name, "stop"), 5)
        assert result["status"] == "stopped" and result["phase"] == "configure"
        assert task.cancelled() and deployment.interactive_cleanup_matches(deployment.read_receipt(f.directory))
        assert f.service.tasks == {} and f.service.processes == {} and not f.retainer_calls
    finally:
        f.service.lock.release()
    assert f.held is None and not f.selection_path.exists()


async def test_shutdown_joins_original_completion_waiter(retaining):
    f = retaining
    task = await waiting(f)
    shutdown = asyncio.create_task(f.service.shutdown())
    await asyncio.sleep(0)
    f.service.lock.release()
    await asyncio.wait_for(shutdown, 5)
    assert task.cancelled() and not f.retainer_calls
    assert f.service.tasks == {} and f.service.processes == {}


@pytest.mark.parametrize("invalid", ["missing", "malformed", "non-object"])
async def test_invalid_shutdown_observation_joins_original_tasks_before_refusal(retaining, invalid):
    f = retaining
    completion = await waiting(f)
    entered, stopping, release, joined = (asyncio.Event() for _ in range(4))
    async def original_worker():
        entered.set()
        try:
            await asyncio.Event().wait()
        except asyncio.CancelledError:
            stopping.set()
            await release.wait()
            raise
        finally:
            joined.set()
    other_id = "00000000-0000-4000-8000-000000000001"
    original = asyncio.create_task(original_worker())
    f.service.tasks[other_id] = original
    await entered.wait()
    receipt_path = f.directory / "receipt.json"
    if invalid == "missing":
        receipt_path.unlink()
        refusal = FileNotFoundError
    else:
        receipt_path.write_text("{" if invalid == "malformed" else "[]")
        refusal = json.JSONDecodeError if invalid == "malformed" else TypeError
    expected = receipt_path.read_bytes() if receipt_path.exists() else None
    # The actual completion may retire its own handle before shutdown's
    # fair lock acquisition. This second original task keeps an invalid
    # receipt in the locked final pass while its cancellation drain waits.
    other_directory = f.directory.parent / other_id
    other_directory.mkdir(mode=0o700)
    other_receipt = other_directory / "receipt.json"
    if expected is not None:
        other_receipt.write_bytes(expected)
    shutdown = asyncio.create_task(f.service.shutdown())
    try:
        await asyncio.sleep(0)
        assert not shutdown.done() and not completion.done() and not original.done()
        f.service.lock.release()
        await asyncio.wait_for(stopping.wait(), 5)
        assert not shutdown.done() and not joined.is_set() and f.service.lock.locked()
        release.set()
        with pytest.raises(refusal):
            await asyncio.wait_for(shutdown, 5)
        assert completion.done() and original.cancelled() and joined.is_set()
        assert not f.service.lock.locked() and not f.retainer_calls and not f.calls
        assert f.host._installation_supervisor is None and not f.selection_path.exists()
        assert (receipt_path.read_bytes() if receipt_path.exists() else None) == expected
        assert (other_receipt.read_bytes() if other_receipt.exists() else None) == expected
        # Failed final evidence retains the original settled handle; no
        # successful shutdown or replacement task is fabricated.
        assert f.service.tasks[other_id] is original
    finally:
        release.set()
        if f.service.lock.locked() and not shutdown.done():
            f.service.lock.release()
        for task in (completion, original, shutdown):
            if not task.done():
                task.cancel()
        await asyncio.gather(completion, original, shutdown, return_exceptions=True)


async def test_cancelled_completion_retires_only_exited_handles(retaining):
    f = retaining
    task = await waiting(f)
    try:
        task.cancel()
        with pytest.raises(asyncio.CancelledError):
            await asyncio.wait_for(task, 5)
        assert not f.retainer_calls and f.service.tasks == {} and f.service.processes == {}
        receipt = deployment.read_receipt(f.directory)
        assert receipt["status"] == "stopped" and receipt["phase"] == "configure"
        assert deployment.interactive_cleanup_matches(receipt)
    finally:
        f.service.lock.release()


@pytest.mark.parametrize("change", ["host", "source"])
async def test_queue_capture_refuses_changed_owner_before_worker_admission(retaining, change):
    f = retaining
    task = f.launch()
    if change == "host":
        f.service.application_setup_retention = lambda *args: pytest.fail("Changed callback cannot run.")
    else:
        deployment.update_receipt(f.directory, build_id="changed-before-worker-admission")
    await asyncio.wait_for(task, 5)
    assert not f.exited.is_set() and not f.retainer_calls
    assert f.process.returncode is None and f.service.tasks == {} and f.service.processes == {}
    assert deployment.read_receipt(f.directory)["status"] == "failed"


@pytest.mark.parametrize("change", ["receipt", "host", "latest", "prefix", "source", "guide"])
async def test_changed_owner_while_completion_waits_refuses_retention(retaining, change):
    f = retaining
    task = await waiting(f)
    if change == "receipt":
        selection = deepcopy(deployment.read_receipt(f.directory)["runtime_candidate"])
        selection["assembly"]["sha256"] = "0" * 64
        deployment.update_receipt(f.directory, runtime_candidate=selection)
    elif change == "host":
        f.service.application_setup_retention = lambda *args: pytest.fail("Changed owner cannot run.")
    elif change == "latest":
        (f.directory.parent / "latest.json").write_text('{"deployment_id":"00000000-0000-4000-8000-000000000000"}')
    elif change == "prefix":
        next((f.destination / ".bin").glob("*/bin/toolkit_gateway")).write_bytes(b"changed selected prefix")
    elif change == "source":
        (f.destination / "config/mob.toml").write_bytes(b"changed installed source")
    else:
        def changed():
            raise DraftConflict("Original guide selection changed.")
        f.host._require_owner_current = changed
    f.service.lock.release()
    with pytest.raises((CustodyError, DraftConflict, ValueError, RuntimeError)):
        await asyncio.wait_for(task, 5)
    assert not f.retainer_calls and f.service.tasks == {} and f.service.processes == {}
    assert f.held is None and not f.selection_path.exists()


@pytest.mark.parametrize("change", ["exit", "cleanup"])
async def test_unsuccessful_or_unconfirmed_worker_never_retains(retaining, change):
    f = retaining
    if change == "exit":
        f.exit_code = 1
        await finished(f)
    else:
        f.cleanup_boundary = "worker-not-admitted"
        with pytest.raises(DraftConflict):
            await finished(f)
    assert not f.retainer_calls and f.held is None
    assert f.service.tasks == {} and f.service.processes == {}


async def test_ordinary_unselected_install_keeps_original_completion(retaining):
    f = retaining
    f.service.application_setup_retention = None
    await finished(f)
    assert not f.retainer_calls and f.host._installation_supervisor is None
    assert deployment.read_receipt(f.directory)["status"] == "stopped"
    assert f.service.tasks == {} and f.service.processes == {}


@pytest.mark.parametrize("failure", ["callback", "identity", "public-input", "stopped", "coroutine"])
async def test_retainer_refusal_preserves_original_install_and_single_handle(retaining, failure):
    f = retaining
    original = f.host._retain_preparation_inputs
    def refused(guide, installation, supervisor):
        if failure == "callback":
            raise OSError("inert-original-retainer-error")
        selected = original(guide, installation, supervisor)
        if failure == "identity":
            f.held = replace(selected)
        elif failure == "public-input":
            f.private.chmod(0o755)
        elif failure == "stopped":
            supervisor.stop.set()
        elif failure == "coroutine":
            async def not_executed():
                pytest.fail("The retainer's returned coroutine cannot execute.")
            f.returned_coroutine = not_executed()
            return f.returned_coroutine
        return selected
    f.host._retain_preparation_inputs = refused
    with pytest.raises((CustodyError, OSError)) as raised:
        await finished(f)
    if failure == "callback":
        assert str(raised.value) == "inert-original-retainer-error"
    elif failure == "coroutine":
        assert f.returned_coroutine.cr_frame is None
    receipt = deployment.read_receipt(f.directory)
    assert receipt["status"] == "stopped" and receipt["phase"] == "configure"
    assert deployment.interactive_cleanup_matches(receipt)
    assert receipt["installed_candidate_runtime_proof"]["fixture"].startswith("inert")
    assert f.host._installation_supervisor is not None and not f.calls
    assert f.service.tasks == {} and f.service.processes == {}
    assert "inert-original-retainer-error" not in receipt["detail"]
    ready = await f.host.setup_status(f.service, f.guide_id, f.directory.name)
    assert ready["available"] is False and ready["reason"] == "source_unavailable"
    async with f.service.lock:
        with pytest.raises(CustodyError, match="only once"):
            f.host.retain_installation_inputs(f.service, f.guide_id, f.directory.name)


@pytest.mark.parametrize("kind", ["custody", "draft", "other"])
async def test_retention_refusal_shows_only_the_installers_own_typed_text(retaining, caplog, kind):
    # The installer's typed refusals are authored, user-facing text: the journal
    # and the Forge log show it. Any other error keeps only its type name.
    f = retaining
    text = {"custody": "The inert typed custody refusal names its remedy.",
        "draft": "The inert typed draft refusal names its remedy.",
        "other": "inert-private-retainer-detail"}[kind]
    error = {"custody": CustodyError, "draft": DraftConflict, "other": OSError}[kind](text)
    def refused(guide, installation, supervisor):
        raise error
    f.host._retain_preparation_inputs = refused
    with caplog.at_level(logging.WARNING):
        with pytest.raises(type(error)):
            await finished(f)
        await asyncio.sleep(0)
    receipt = deployment.read_receipt(f.directory)
    assert receipt["status"] == "stopped" and receipt["phase"] == "configure"
    logged = [record.getMessage() for record in caplog.records if record.levelno == logging.WARNING]
    if kind == "other":
        assert receipt["detail"] == ("Trusted setup inputs were not retained (OSError). "
            "Your files are preserved. Check setup status before continuing.")
        assert len(logged) == 1 and "(OSError)" in logged[0]
        assert text not in receipt["detail"] and not any(text in line for line in logged)
    else:
        assert receipt["detail"] == ("Trusted setup inputs were not retained: " + text
            + " Your files are preserved. Check setup status before continuing.")
        assert receipt["progress"][-1]["message"] == receipt["detail"]
        assert len(logged) == 1 and text in logged[0]
    assert f.service.tasks == {} and f.service.processes == {}


async def test_retention_refusal_text_is_one_bounded_sentence(retaining):
    f = retaining
    def refused(guide, installation, supervisor):
        raise CustodyError("A typed refusal\nover two lines" + " without an end" * 400)
    f.host._retain_preparation_inputs = refused
    with pytest.raises(CustodyError):
        await finished(f)
    detail = deployment.read_receipt(f.directory)["detail"]
    shown = detail.removeprefix("Trusted setup inputs were not retained: ").removesuffix(
        " Your files are preserved. Check setup status before continuing.")
    assert shown != detail and "\n" not in detail and len(shown) <= 2001
    assert shown.startswith("A typed refusal over two lines without an end") and shown.endswith(".")


async def test_later_read_only_supplier_cannot_replace_retained_object(retaining):
    f = retaining
    await finished(f)
    f.held = replace(f.held)
    ready = await f.host.setup_status(f.service, f.guide_id, f.directory.name)
    assert ready["available"] is False and ready["reason"] == "source_unavailable"
    assert not f.calls and len(f.retainer_calls) == 1


@pytest.mark.parametrize("when", ["first-read", "later-read"])
async def test_same_typed_object_cannot_change_original_fresh_paths(retaining, when):
    f = retaining
    if when == "first-read":
        def changed(*args):
            f.held.fresh_paths["bundle_path"] = str(f.destination / "different-bundle")
            return f.held
        f.host._preparation_inputs = changed
        with pytest.raises(CustodyError, match="Fresh paths"):
            await finished(f)
    else:
        await finished(f)
        f.held.fresh_paths["bundle_path"] = str(f.destination / "different-bundle")
        ready = await f.host.setup_status(f.service, f.guide_id, f.directory.name)
        assert ready["available"] is False and ready["reason"] == "source_unavailable"
    assert not f.calls and not f.selection_path.exists() and len(f.retainer_calls) == 1
    assert deployment.read_receipt(f.directory)["status"] == "stopped"


@pytest.mark.parametrize("kind", ["async-call", "async-generator", "wrong-shape"])
def test_retainer_contract_refuses_before_invocation(selected, kind):
    if kind == "async-call":
        class Retainer:
            async def __call__(self, *args):
                pytest.fail("Async callable cannot execute.")
        callback = Retainer()
    elif kind == "async-generator":
        async def callback(*args):
            pytest.fail("Async generator cannot execute.")
            yield None
    else:
        callback = lambda: pytest.fail("Wrong-shaped callback cannot execute.")
    with pytest.raises(CustodyError):
        ForgeApplicationHost(guide_id="original-guide", runtime_target=selected.target,
            acquisition_selection=selected.acquisition, viewer_selection=selected.viewer,
            require_current=lambda: None, startup_selection=lambda *args: None,
            preparation_inputs=lambda *args: None, retain_preparation_inputs=callback)


def test_existing_selector_joins_and_preserves_original_retention_callback(api):
    select(api)
    assert api.deployment.application_setup_retention == api.selected.host.retain_installation_inputs
    api.deployment.application_setup_retention = lambda *args: None
    with pytest.raises(DraftConflict, match="callbacks changed"):
        select(api)
