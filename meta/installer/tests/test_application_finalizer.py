"""Inert finalizer sequence and actual local pointer publication controls.

The executor supplies native-shaped specimens. These tests prove caller custody
and local filesystem behavior, not native finish, policy authority or Ready.
"""
import asyncio
import json
from pathlib import Path
import sys
import threading

import pytest

import launchkit.connect_application_finalizer as module
from launchkit.connect_application_finalizer import ApplicationPolicyFinalizer, application_command_executor
from launchkit.connect_installer_caller import CustodyError, ProcessOutcome
from test_completed_application_policy import fixture
from test_connect_completed_application_setup import encoded, write
from test_remote_command_raw_outcome import load_command_owner


class FinalizerFixture:
    def __init__(self, tmp_path, *, previous=False):
        self.f = fixture(tmp_path)
        self.f.gateway.path.chmod(0o700)
        self.workspace = self.f.root / "finalizer"
        self.workspace.mkdir(mode=0o700)
        self.template = write(self.f.root / "activation-template.json",
            encoded({"source": "original-source", "revision": "retained-revision"}))
        self.previous = write(self.f.root / "previous.json", b'{"original":"activation"}\n') if previous else None
        if self.previous:
            (self.f.root / "current").symlink_to(self.previous.path.name)
        self.commands = []
        self.on_prepare = lambda: None
        self.on_validate = lambda: None
        self.prepare_process = None
        self.validate_process = None
        self.f.caller.execute = self.execute
        self.finalizer = ApplicationPolicyFinalizer(self.f.caller,
            preparation_plan=self.f.plan, source_policy=self.f.source_policy,
            activation_template=self.template, working_directory=self.workspace,
            bundle_path=self.f.root / "policy-bundle", activation_path=self.f.root / "activation.json",
            source_files=(self.f.source,))

    async def execute(self, argv):
        self.commands.append(argv)
        if argv[1] == "--prepare-application-policy":
            if self.prepare_process:
                return self.prepare_process
            arguments = dict(zip(argv[2::2], argv[3::2]))
            directory = Path(arguments["--artifacts"])
            bundle = Path(arguments["--bundle"])
            bundle.mkdir(mode=0o700)
            artifacts = [write(bundle / p.name, p.read_bytes()) for p in sorted(directory.iterdir())]
            policy = write(bundle / "toolkit-local-policy.json", b'{"native_policy":"specimen"}\n')
            artifacts.append(policy)
            manifest = write(bundle / "manifest.json", encoded({"artifacts": [
                {"path": p.path.name, "sha256": p.sha256} for p in artifacts]}))
            value = json.loads(Path(arguments["--activation-template"]).read_bytes())
            value["policy_bundle"] = {"path": bundle.name,
                "policy_digest": "hc-policy-v1:sha256:" + manifest.sha256}
            activation = write(Path(arguments["--activation"]), encoded(value))
            self.selection = {"deployment_root": str(self.f.root), "activation": activation.pin(),
                "policy_manifest": manifest.pin(), "local_policy": policy.pin()}
            self.on_prepare()
            return ProcessOutcome(0, encoded({"kind": "application_policy_prepared", "selection": self.selection}) + b"\n", b"")
        assert argv[1] == "--validate-application-policy"
        self.on_validate()
        if self.validate_process:
            return self.validate_process
        return ProcessOutcome(0, encoded({"kind": "application_policy_validated", "selection": self.selection}) + b"\n", b"")

    async def run(self):
        return await self.finalizer.finalize_selection(self.f.manifest)


@pytest.mark.asyncio
@pytest.mark.parametrize("previous", [False, True])
async def test_exact_candidate_is_validated_then_published_without_reprovision(tmp_path, previous):
    f = FinalizerFixture(tmp_path, previous=previous)
    result = await f.run()
    assert result == f.selection == f.finalizer.selection
    assert (f.f.root / "current").resolve() == f.finalizer.activation_path
    assert [argv[1] for argv in f.commands] == ["--prepare-application-policy", "--validate-application-policy"]
    assert not f.f.caller.provision_attempt_path.exists()
    if previous:
        assert f.previous.read() == b'{"original":"activation"}\n'
    assert json.loads((f.workspace / "finalization.outcome.json").read_bytes())["status"] == "published"
    acquisition = json.loads((f.workspace / "artifacts/native-setup.json").read_bytes())
    assert acquisition["backend_generation"] == f.f.plan_value["acquisition"]["backend_generation"]
    assert acquisition["proposal_ceiling"] == f.f.plan_value["acquisition"]["proposal_ceiling"]
    for pin in (f.f.source_policy, f.f.bundle.setup, f.f.bundle.application_tool_policy,
            *(output for _, output in f.f.bundle.member_surfaces)):
        assert (f.workspace / "artifacts" / pin.path.name).read_bytes() == pin.read()
    with pytest.raises(CustodyError, match="already entered"):
        await f.run()
    assert len(f.commands) == 2


@pytest.mark.asyncio
@pytest.mark.parametrize("phase", ["prepare", "validate"])
@pytest.mark.parametrize("result", ["refused", "unknown"])
async def test_unverified_native_outcomes_do_not_publish_or_repeat(tmp_path, phase, result):
    f = FinalizerFixture(tmp_path, previous=True)
    process = ProcessOutcome(1, b'{"unrecognized":"refusal"}\n', b"") if result == "refused" else ProcessOutcome(0, b"", b"")
    setattr(f, phase + "_process", process)
    with pytest.raises(CustodyError, match="verified"):
        await f.run()
    assert (f.f.root / "current").resolve() == f.previous.path
    assert f.finalizer.selection is None
    assert not (f.workspace / "publication.json").exists()
    assert len(f.commands) == (1 if phase == "prepare" else 2)
    with pytest.raises(CustodyError, match="already entered"):
        await f.run()


@pytest.mark.asyncio
@pytest.mark.parametrize("changed", ["source", "manifest", "surface", "template", "owner", "pointer", "selected-policy", "copied-artifact"])
async def test_changes_after_native_validation_block_publication(tmp_path, changed):
    f = FinalizerFixture(tmp_path, previous=True)
    def mutate():
        if changed == "owner":
            f.f.current = False
        elif changed == "pointer":
            other = write(f.f.root / "other-activation.json", b'{"other":true}\n')
            (f.f.root / "current").unlink()
            (f.f.root / "current").symlink_to(other.path.name)
        else:
            path = {"source": f.f.source.path, "manifest": f.f.manifest.path,
                "surface": f.f.bundle.member_surfaces[0][1].path,
                "template": f.template.path,
                "selected-policy": f.finalizer.bundle_path / "toolkit-local-policy.json",
                "copied-artifact": f.finalizer.bundle_path / "policy.json"}[changed]
            path.write_bytes(path.read_bytes() + b"\n")
    f.on_validate = mutate
    with pytest.raises(CustodyError):
        await f.run()
    assert (f.f.root / "current").resolve() != f.finalizer.activation_path
    assert f.finalizer.selection is None
    assert len(f.commands) == 2


@pytest.mark.asyncio
async def test_existing_entry_is_never_completed_by_a_reconstructed_caller(tmp_path):
    f = FinalizerFixture(tmp_path)
    original = write(f.workspace / "finalization.json", b'{"status":"entered","original":true}\n')
    with pytest.raises(CustodyError, match="fresh"):
        await f.run()
    assert original.read() == b'{"status":"entered","original":true}\n'
    assert not f.commands and not (f.workspace / "finalization.outcome.json").exists()


@pytest.mark.parametrize("pointer", ["dangling", "regular", "outside"])
def test_invalid_existing_pointer_is_not_treated_as_absence(tmp_path, pointer):
    f = fixture(tmp_path)
    current = f.root / "current"
    if pointer == "regular":
        write(current, b"invalid current")
    elif pointer == "dangling":
        current.symlink_to("absent.json")
    else:
        current.symlink_to("../outside.json")
    workspace = f.root / "finalizer"
    workspace.mkdir(mode=0o700)
    template = write(f.root / "template.json", b"{}\n")
    with pytest.raises((CustodyError, FileNotFoundError)):
        ApplicationPolicyFinalizer(f.caller, preparation_plan=f.plan, source_policy=f.source_policy,
            activation_template=template, working_directory=workspace,
            bundle_path=f.root / "new-bundle", activation_path=f.root / "new-activation.json")


@pytest.mark.parametrize("bundle_name", ["current", "new-activation.json.current.next"])
def test_reserved_publication_paths_cannot_be_native_bundle_outputs(tmp_path, bundle_name):
    f = fixture(tmp_path)
    workspace = f.root / "finalizer"
    workspace.mkdir(mode=0o700)
    template = write(f.root / "template.json", b"{}\n")
    with pytest.raises(CustodyError, match="separate fresh"):
        ApplicationPolicyFinalizer(f.caller, preparation_plan=f.plan, source_policy=f.source_policy,
            activation_template=template, working_directory=workspace,
            bundle_path=f.root / bundle_name, activation_path=f.root / "new-activation.json")
    assert not f.calls and not (workspace / "finalization.json").exists()


@pytest.mark.asyncio
async def test_uncertainty_after_replacement_retains_selection_and_never_rolls_back(tmp_path, monkeypatch):
    f = FinalizerFixture(tmp_path, previous=True)
    original_sync = module._sync_directory
    def fail_after_replace(path):
        if path == f.f.root and (path / "current").resolve() == f.finalizer.activation_path:
            raise OSError("inert directory-sync failure after actual replacement")
        original_sync(path)
    monkeypatch.setattr(module, "_sync_directory", fail_after_replace)
    with pytest.raises(OSError, match="directory-sync"):
        await f.run()
    assert (f.f.root / "current").resolve() == f.finalizer.activation_path
    assert f.finalizer.selection == f.selection
    outcome = json.loads((f.workspace / "finalization.outcome.json").read_bytes())
    assert outcome["status"] == "outcome_unknown" and outcome["publication_entered"]
    assert outcome["selection"] == f.selection
    assert f.previous.read() == b'{"original":"activation"}\n'
    with pytest.raises(CustodyError, match="already entered"):
        await f.run()


@pytest.mark.asyncio
async def test_cancelled_waiter_joins_the_original_finalizer_without_successor(tmp_path):
    f = FinalizerFixture(tmp_path)
    entered, release = asyncio.Event(), asyncio.Event()
    execute = f.f.caller.execute
    async def held(argv):
        if argv[1] == "--prepare-application-policy":
            entered.set()
            await release.wait()
        return await execute(argv)
    f.f.caller.execute = held
    task = asyncio.create_task(f.run())
    await entered.wait()
    task.cancel()
    await asyncio.sleep(0)
    assert not task.done()
    with pytest.raises(CustodyError, match="already entered"):
        await f.run()
    task.cancel()
    release.set()
    with pytest.raises(asyncio.CancelledError):
        await task
    assert len(f.commands) == 2
    assert (f.f.root / "current").resolve() == f.finalizer.activation_path
    assert json.loads((f.workspace / "finalization.outcome.json").read_bytes())["status"] == "published"


@pytest.mark.asyncio
async def test_application_executor_uses_actual_existing_command_owner_and_exact_streams(tmp_path):
    owner = load_command_owner()
    execute = application_command_executor(owner.command, root=tmp_path, timeout=2)
    result = await execute((sys.executable, "-I", "-c",
        "import os,sys;os.write(1,b'\\xffreceipt\\n');os.write(2,b'\\xfeerror\\n');sys.exit(19)"))
    assert result == ProcessOutcome(19, b'\xffreceipt\n', b'\xfeerror\n')


@pytest.mark.asyncio
async def test_application_executor_joins_existing_command_future_when_waiter_is_cancelled(tmp_path):
    entered, release, exited = threading.Event(), threading.Event(), threading.Event()
    calls = []
    def command(argv, **options):
        calls.append((argv, options))
        entered.set()
        assert release.wait(timeout=3)
        exited.set()
        return {"returncode": 0, "stdout": b"specimen", "stderr": b""}
    execute = application_command_executor(command, root=tmp_path, timeout=2)
    task = asyncio.create_task(execute(("existing-command-lane",)))
    assert await asyncio.to_thread(entered.wait, 30)
    task.cancel()
    await asyncio.sleep(0)
    task.cancel()
    assert not task.done() and not exited.is_set()
    release.set()
    with pytest.raises(asyncio.CancelledError):
        await task
    assert exited.is_set() and len(calls) == 1
    assert calls[0][1] == {"root": tmp_path, "timeout": 2, "raw_outcome": True}


@pytest.mark.asyncio
async def test_staged_pointer_replacement_before_publish_is_rejected(tmp_path, monkeypatch):
    f = FinalizerFixture(tmp_path, previous=True)
    original_write = module._write_new
    def changed(path, value):
        original_write(path, value)
        if path.name == "publication.json":
            f.finalizer.next_pointer.unlink()
            f.finalizer.next_pointer.symlink_to(f.previous.path)
    monkeypatch.setattr(module, "_write_new", changed)
    with pytest.raises(CustodyError, match="staged publication pointer"):
        await f.run()
    assert (f.f.root / "current").resolve() == f.previous.path
    assert f.finalizer.selection is None


@pytest.mark.asyncio
async def test_live_owner_fences_and_publication_stay_on_the_owner_loop(tmp_path, monkeypatch):
    f = FinalizerFixture(tmp_path, previous=True)
    owner_thread = threading.get_ident()
    original_current = f.f.caller.require_current
    original_replace = module.os.replace
    owner_checks, replacements = [], []

    def require_current():
        observed = threading.get_ident()
        assert observed == owner_thread, "The live selection owner must be checked on its event loop."
        owner_checks.append(observed)
        original_current()

    def replace(source, destination):
        assert threading.get_ident() == owner_thread, "Publication belongs to the checked owner loop."
        assert owner_checks
        replacements.append((source, destination))
        return original_replace(source, destination)

    f.f.caller.require_current = require_current
    monkeypatch.setattr(module.os, "replace", replace)
    selected = await f.run()
    assert selected == f.selection
    assert replacements == [(f.finalizer.next_pointer, f.f.root / "current")]
    assert len(owner_checks) >= 4 and set(owner_checks) == {owner_thread}
    assert len(f.commands) == 2


@pytest.mark.asyncio
async def test_finalization_file_reads_and_hashes_remain_off_loop(tmp_path, monkeypatch):
    f = FinalizerFixture(tmp_path, previous=True)
    owner_thread = threading.get_ident()
    original_read = module.ExactFile.read
    reads = []

    def read(pin):
        observed = threading.get_ident()
        assert observed != owner_thread, "Full file validation must not block the live SDK loop."
        reads.append((pin.path, observed))
        return original_read(pin)

    # The native-shaped executor is a filesystem specimen, not an SDK callback.
    original_execute = f.f.caller.execute
    async def execute(argv):
        return await asyncio.to_thread(lambda: asyncio.run(original_execute(argv)))
    f.f.caller.execute = execute
    monkeypatch.setattr(module.ExactFile, "read", read)
    selected = await f.run()
    assert selected == f.selection
    assert reads and all(observed != owner_thread for _, observed in reads)
    assert len(f.commands) == 2


@pytest.mark.asyncio
@pytest.mark.parametrize("changed", ["source", "surface", "native-membership", "current", "staged-pointer", "owner"])
async def test_worker_to_loop_publication_handoff_rejects_late_changes(tmp_path, changed, monkeypatch):
    f = FinalizerFixture(tmp_path, previous=True)
    loop = asyncio.get_running_loop()
    owner_thread = threading.get_ident()
    preflight = f.finalizer._prepare_publication
    mutations = []

    def mutate():
        assert threading.get_ident() == owner_thread
        mutations.append(changed)
        if changed == "owner":
            f.f.current = False
        elif changed == "current":
            (f.f.root / "current").unlink()
            (f.f.root / "current").symlink_to(f.previous.path)
        elif changed == "staged-pointer":
            f.finalizer.next_pointer.unlink()
            f.finalizer.next_pointer.symlink_to(f.previous.path)
        elif changed == "native-membership":
            write(f.finalizer.bundle_path / "unselected.json", b"{}\n")
        else:
            pin = f.f.source if changed == "source" else f.f.bundle.member_surfaces[0][1]
            pin.path.write_bytes(pin.path.read_bytes() + b"\n")

    def prepared(*args):
        publication = preflight(*args)
        # Queue a real loop-side change after byte authentication and before
        # the original worker returns to the awaiting publication owner.
        loop.call_soon_threadsafe(mutate)
        return publication

    monkeypatch.setattr(f.finalizer, "_prepare_publication", prepared)
    with pytest.raises(CustodyError):
        await f.run()
    assert mutations == [changed]
    assert (f.f.root / "current").resolve() == f.previous.path
    assert f.finalizer.selection is None and len(f.commands) == 2
    outcome = json.loads((f.workspace / "finalization.outcome.json").read_bytes())
    assert outcome["status"] == "outcome_unknown" and outcome["publication_entered"]


@pytest.mark.asyncio
async def test_trusted_publication_fence_covers_source_outside_preparation_pins(tmp_path, monkeypatch):
    f = FinalizerFixture(tmp_path, previous=True)
    source = write(f.f.root / "other-accepted-resource.txt", b"accepted resource\n")
    identity = module._identity(source.path.lstat())
    loop = asyncio.get_running_loop()
    owner_thread = threading.get_ident()
    checks = []

    def require_publication_current():
        assert threading.get_ident() == owner_thread
        checks.append("original-source")
        if module._identity(source.path.lstat()) != identity:
            raise CustodyError("The original accepted source closure changed.")

    f.finalizer.require_publication_current = require_publication_current
    preflight = f.finalizer._prepare_publication
    def prepared(*args):
        publication = preflight(*args)
        loop.call_soon_threadsafe(source.path.write_bytes, b"changed accepted resource\n")
        return publication
    monkeypatch.setattr(f.finalizer, "_prepare_publication", prepared)
    with pytest.raises(CustodyError, match="accepted source closure"):
        await f.run()
    assert checks == ["original-source"]
    assert (f.f.root / "current").resolve() == f.previous.path
    assert f.finalizer.selection is None and len(f.commands) == 2


@pytest.mark.asyncio
async def test_cancelled_waiter_joins_original_publication_worker(tmp_path, monkeypatch):
    f = FinalizerFixture(tmp_path, previous=True)
    entered, release, settled = threading.Event(), threading.Event(), threading.Event()
    preflight = f.finalizer._prepare_publication
    calls = []
    def held(*args):
        calls.append("original-preflight")
        publication = preflight(*args)
        entered.set()
        assert release.wait(timeout=3)
        settled.set()
        return publication
    monkeypatch.setattr(f.finalizer, "_prepare_publication", held)
    task = asyncio.create_task(f.run())
    try:
        assert await asyncio.to_thread(entered.wait, 30)
        task.cancel()
        await asyncio.sleep(0)
        task.cancel()
        assert not task.done() and not settled.is_set()
    finally:
        release.set()
    with pytest.raises(asyncio.CancelledError):
        await task
    assert settled.is_set() and calls == ["original-preflight"]
    assert (f.f.root / "current").resolve() == f.finalizer.activation_path
    assert len(f.commands) == 2
    assert json.loads((f.workspace / "finalization.outcome.json").read_bytes())["status"] == "published"


@pytest.mark.asyncio
@pytest.mark.parametrize("cancelled", [False, True])
async def test_finalizer_fences_and_publication_use_original_task_across_cancellation(tmp_path, monkeypatch, cancelled):
    f = FinalizerFixture(tmp_path, previous=True)
    loop = asyncio.get_running_loop()
    entered, release = asyncio.Event(), asyncio.Event()
    parent, checks, replacements = [], [], []
    original_current, original_execute = f.f.caller.require_current, f.f.caller.execute
    original_replace = module.os.replace
    existing = asyncio.all_tasks()

    def current():
        assert asyncio.get_running_loop() is loop
        assert asyncio.current_task() is parent[0]
        checks.append(asyncio.current_task())
        original_current()

    def replace(source, destination):
        assert asyncio.get_running_loop() is loop
        assert asyncio.current_task() is parent[0]
        replacements.append((source, destination))
        return original_replace(source, destination)

    async def execute(argv):
        if argv[1] == "--prepare-application-policy":
            entered.set()
            await release.wait()
        return await original_execute(argv)

    async def run():
        parent.append(asyncio.current_task())
        return await f.run()

    f.f.caller.require_current, f.f.caller.execute = current, execute
    monkeypatch.setattr(module.os, "replace", replace)
    task = asyncio.create_task(run())
    try:
        await asyncio.wait_for(entered.wait(), 30)
        original_operation = f.finalizer._finalize_task
        assert original_operation is not task and not original_operation.done()
        if cancelled:
            task.cancel()
            await asyncio.sleep(0)
            task.cancel()
            assert not task.done() and not original_operation.done()
    finally:
        release.set()
    if cancelled:
        with pytest.raises(asyncio.CancelledError):
            await task
    else:
        assert await task == f.selection
    assert f.finalizer._finalize_task is original_operation and original_operation.done()
    assert original_operation.result() == f.selection
    assert len(checks) >= 4 and all(owner is task for owner in checks)
    assert replacements == [(f.finalizer.next_pointer, f.f.root / "current")]
    assert (f.f.root / "current").resolve() == f.finalizer.activation_path
    assert len(f.commands) == 2
    assert json.loads((f.workspace / "finalization.outcome.json").read_bytes())["status"] == "published"
    assert asyncio.all_tasks() - existing == set()


@pytest.mark.asyncio
@pytest.mark.parametrize("cancelled", [False, True])
async def test_finalizer_owner_refusal_preserves_original_error_and_settles_requests(tmp_path, cancelled):
    f = FinalizerFixture(tmp_path, previous=True)
    loop = asyncio.get_running_loop()
    entered, release = asyncio.Event(), asyncio.Event()
    parent, checks = [], []
    original_current, original_execute = f.f.caller.require_current, f.f.caller.execute
    sentinel = CustodyError("Original task refused the candidate before atomic publication.")
    existing = asyncio.all_tasks()

    def current():
        assert asyncio.get_running_loop() is loop
        assert asyncio.current_task() is parent[0]
        checks.append(asyncio.current_task())
        if f.finalizer.publication_entered:
            raise sentinel
        original_current()

    async def execute(argv):
        if argv[1] == "--prepare-application-policy":
            entered.set()
            await release.wait()
        return await original_execute(argv)

    async def run():
        parent.append(asyncio.current_task())
        return await f.run()

    f.f.caller.require_current, f.f.caller.execute = current, execute
    task = asyncio.create_task(run())
    try:
        await asyncio.wait_for(entered.wait(), 30)
        original_operation = f.finalizer._finalize_task
        if cancelled:
            task.cancel()
            await asyncio.sleep(0)
            task.cancel()
            assert not task.done() and not original_operation.done()
    finally:
        release.set()
    with pytest.raises(BaseException) as raised:
        await task
    if cancelled:
        assert isinstance(raised.value, BaseExceptionGroup)
        assert raised.value.exceptions[1] is sentinel
        assert isinstance(raised.value.exceptions[0], asyncio.CancelledError)
    else:
        assert raised.value is sentinel
    assert f.finalizer._finalize_task is original_operation and original_operation.done()
    assert original_operation.exception() is sentinel
    assert all(owner is task for owner in checks)
    assert (f.f.root / "current").resolve() == f.previous.path
    assert f.finalizer.selection is None and len(f.commands) == 2
    outcome = json.loads((f.workspace / "finalization.outcome.json").read_bytes())
    assert outcome["status"] == "outcome_unknown" and outcome["publication_entered"]
    assert outcome["error_type"] == "CustodyError"
    assert asyncio.all_tasks() - existing == set()
