"""Selected replacement dispatch with real files and inert runtime boundaries.

Packages, native inputs and gateway bytes are synthetic custody specimens.
These tests run no gateway, SDK, application, service or provider and establish
neither native policy acceptance, definition activation nor member adoption.
"""
import asyncio
from copy import deepcopy
import hashlib
import json
import os
from pathlib import Path
import socket
import subprocess
import stat
import sys
import threading
from types import ModuleType, SimpleNamespace
import uuid

import pytest

from launchkit import deployment, deployment_application_startup as module
from launchkit import deployment_updates as updates, deployment_worker as worker
from launchkit.connect_installer_caller import CustodyError, ExactFile
from launchkit.projects import DraftConflict
from launchkit.templates.storage_setup import project_lock
from test_deployment import Guide, Implementation, Service, submit


CONFIG = '''[mob]
id = "selected-app"
[profiles.helper-source]
model = "selected-model"
provider = "openai"
[profiles.helper-source.tools]
comms = true
shell = false
'''
POLICY = json.dumps({"provider_id": "application", "policy_id": "selected-tools",
    "revision": 1, "members": [{"member_identity": "lead:main", "grants": []}]})


def encoded(value):
    return (json.dumps(value, ensure_ascii=True, sort_keys=True,
        separators=(",", ":")) + "\n").encode()


def write(path, value, mode=0o600):
    content = value if isinstance(value, bytes) else encoded(value)
    path.write_bytes(content)
    path.chmod(mode)
    return ExactFile(path, hashlib.sha256(content).hexdigest())


@pytest.fixture(autouse=True)
def no_runtime_operations(monkeypatch):
    def forbidden(*args, **kwargs):
        raise AssertionError("Replacement dispatch controls cannot run a process or contact a provider.")
    for owner, name in ((subprocess, "run"), (subprocess, "Popen"), (os, "system"),
            (os, "kill"), (os, "killpg"), (socket, "create_connection"),
            (asyncio, "create_subprocess_exec"), (asyncio, "create_subprocess_shell")):
        monkeypatch.setattr(owner, name, forbidden)


class Inputs:
    def __init__(self, directory, destination, identity):
        self.directory, self.destination, self.identity = directory, destination, identity
        private = destination / ".mobkit/update-inputs"
        private.mkdir(mode=0o700)
        bundle = destination / "original-bundle"
        bundle.mkdir(mode=0o700)
        self.activation = write(destination / "original-activation.json", {"synthetic": "original activation"})
        self.manifest = write(bundle / "manifest.json", {"synthetic": "original native manifest"})
        self.local_policy = write(bundle / "policy.json", {"synthetic": "original bootstrap policy"})
        (destination / "current").symlink_to(self.activation.path.name)
        member = {"mob_id": "selected-app", "role": "helper-source", "member": "synthetic-native-member"}
        update = updates.transaction(directory, identity)[2]
        policy_input = json.loads(POLICY)
        policy_input["members"][0]["member_identity"] = member["member"]
        policy_input["source"] = {"source_id": update["to_build_id"],
            "source_digest": "sha256:" + updates.digest(POLICY.encode())}
        application = {"schema": 4, "definition": {"format": "toml", "text": CONFIG},
            "realm": {"realm_id": "selected-realm", "context_root": str(destination),
                "state_root": str(destination / ".mobkit/state")},
            "roster": [{"identity": "lead:main", "profile": "helper-source"}],
            "members": [{"member": member}], "helper": {"member": member},
            "helper_profile": "dedicated", "helper_tools": {"synthetic": "compiler input"},
            "tool_policy": policy_input}
        self.plan_value = {"schema": 1, "deployment_root": str(destination), "application": application}
        self.compilation = write(private / "compilation.json", application)
        self.plan = write(private / "candidate-plan.json", self.plan_value)
        self.candidate_policy = write(private / "candidate-native-policy.json", {"synthetic": "candidate native policy"})
        self.prior = write(private / "actual-prior-policy.json", {"synthetic": "actual prior after intervening update"})
        self.gateway = write(private / "gateway", b"synthetic executable bytes, never run\n", 0o700)
        self.selection_value = {"update_id": identity, "predecessor": {
            "deployment_root": str(destination), "activation": self.activation.pin(),
            "policy_manifest": self.manifest.pin(), "local_policy": self.local_policy.pin()},
            "compilation_input": self.compilation.pin(), "application_tool_policy": self.candidate_policy.pin()}
        self.selection = write(private / "selection.json", self.selection_value)
        self.associations = private / "associations"
        self.associations.mkdir(mode=0o700)
        self.value = {"kind": "replacement", "selection_file": self.selection.pin(),
            "prior_tool_policy": self.prior.pin(), "preparation_plan": self.plan.pin(),
            "gateway": self.gateway.pin(), "association_directory": str(self.associations),
            "expected_revision": 7}

    def repin_selection(self):
        self.selection = write(self.selection.path, self.selection_value)
        self.value["selection_file"] = self.selection.pin()

    def repin_plan(self):
        self.plan = write(self.plan.path, self.plan_value)
        self.value["preparation_plan"] = self.plan.pin()

    def repin_application(self, application):
        self.compilation = write(self.compilation.path, application)
        self.selection_value["compilation_input"] = self.compilation.pin()
        self.plan_value["application"] = application
        self.repin_plan()
        self.repin_selection()

    def admission(self):
        update = updates.transaction(self.directory, self.identity)[2]
        return updates.retain_update_admission(self.directory, update, "update",
            application_startup=self.value)

    def selected(self, admission):
        return updates.read_update_admission(self.directory, "update", admission)

    def dispatch(self):
        admission = self.admission()
        selected = self.selected(admission)
        updates.activate(self.directory, self.identity, expected_binding=selected["binding"],
            expected_installation=selected["installation"])
        return admission, selected, module.retain_attempt(self.directory, admission, selected)


@pytest.fixture
async def inputs(tmp_path):
    guide = Guide()
    implementation = Implementation(guide)
    implementation.digest = hashlib.sha256(b"synthetic accepted previous source").hexdigest()
    implementation.definition["agents"][0]["profile"] = "helper-source"
    implementation.extra_files = {"config/mob.toml": CONFIG,
        "application/tool-policy.json": POLICY, "application/logic.py": "VERSION = 'old'\n"}
    service = Service(tmp_path / "private-server", guide, implementation)
    destination = tmp_path / "application"
    await submit(service, destination)
    directory = service._directory(guide.identity)
    deployment.materialize(directory)
    await service.shutdown()
    deployment.update_receipt(directory, status="stopped")
    (destination / "data").mkdir(mode=0o700)
    (destination / "data/ledger.db").write_bytes(b"retained user records")
    (destination / "workspace").mkdir(mode=0o700)
    (destination / ".mobkit/state").mkdir(mode=0o700)
    implementation.identity = str(uuid.uuid4())
    implementation.digest = hashlib.sha256(b"synthetic accepted candidate source").hexdigest()
    implementation.extra_files["application/logic.py"] = "VERSION = 'new'\n"
    receipt = await service.stage_update(guide.identity, deployment.read_receipt(directory)["deployment_id"],
        guide.state["version"], implementation.identity)
    assert receipt["update"]["status"] == "staged" and receipt["update"]["blockers"] == []
    try:
        yield Inputs(directory, destination, receipt["update"]["update_id"])
    finally:
        await service.shutdown()


def images(f):
    return {path: path.read_bytes() for path in (f.directory / "package.zip",
        f.destination / "application/logic.py", f.destination / ".env", f.destination / "data/ledger.db")}


@pytest.mark.parametrize("revision", [7, None])
async def test_original_admission_freezes_explicit_inputs_without_native_qualification(inputs, revision):
    f = inputs
    f.value["expected_revision"] = revision
    original = deepcopy(f.value)
    admission = f.admission()
    selected = f.selected(admission)
    f.value["expected_revision"] = 999
    assert selected["application_startup"] == original
    assert selected["binding"] == updates.update_binding(updates.transaction(f.directory, f.identity)[2])
    assert f.prior.read() != f.local_policy.read() != f.candidate_policy.read()
    assert not (updates.update_directory(f.directory, f.identity) / module.ATTEMPT_NAME).exists()


@pytest.mark.parametrize("source_id", ["original retained native policy source", "", "   "])
async def test_original_native_policy_provenance_is_preserved_without_new_revision(inputs, source_id):
    f = inputs
    application = json.loads(f.compilation.read())
    application["tool_policy"]["source"]["source_id"] = source_id
    f.repin_application(application)
    if source_id.strip():
        selected = f.selected(f.admission())
        assert selected["application_startup"]["selection_file"] == f.selection.pin()
        retained = json.loads(f.compilation.read())["tool_policy"]
        assert retained["source"]["source_id"] == source_id and retained["revision"] == 1
        assert retained["source"]["source_digest"] == "sha256:" + updates.digest(POLICY.encode())
    else:
        with pytest.raises(CustodyError, match="source provenance"):
            f.admission()


async def test_rich_fixed_roster_fields_remain_in_original_selected_compilation(inputs):
    f = inputs
    application = json.loads(f.compilation.read())
    application["roster"][0].update(display_name="Your usual contact", addressability="default",
        runtime_mode_override="autonomous", labels={"agent_identity": "lead:main", "domain": "reading"})
    f.repin_application(application)
    selected = f.selected(f.admission())
    assert selected["application_startup"]["selection_file"] == f.selection.pin()
    assert json.loads(f.compilation.read())["roster"] == application["roster"]


@pytest.mark.parametrize("mutation", ["update-id", "plan", "definition", "roster", "revision", "associations", "gateway",
    "grant", "member", "source-digest"])
async def test_mismatched_original_inputs_refuse_before_copy_or_attempt(inputs, mutation, tmp_path):
    f = inputs
    if mutation == "update-id":
        f.selection_value["update_id"] = "another-update"
        f.repin_selection()
    elif mutation == "plan":
        f.plan_value["application"]["helper_tools"] = {"different": "callback plan"}
        f.repin_plan()
    elif mutation in {"definition", "roster", "grant", "member", "source-digest"}:
        application = json.loads(f.compilation.read())
        if mutation == "definition":
            application["definition"]["text"] = CONFIG.replace("selected-app", "other-app")
        elif mutation == "roster":
            application["roster"][0]["identity"] = "another:person"
        elif mutation == "grant":
            application["tool_policy"]["members"][0]["grants"] = [
                {"tool_name": "unexpected:execution", "action": "invoke", "consequence": "r3"}]
        elif mutation == "member":
            application["tool_policy"]["members"][0]["member_identity"] = "another-native-member"
        else:
            application["tool_policy"]["source"]["source_digest"] = "sha256:" + "0" * 64
        f.compilation = write(f.compilation.path, application)
        f.selection_value["compilation_input"] = f.compilation.pin()
        f.plan_value["application"] = application
        f.repin_plan()
        f.repin_selection()
    elif mutation == "revision":
        f.value["expected_revision"] = True
    elif mutation == "associations":
        path = tmp_path / "other-associations"
        path.mkdir(mode=0o700)
        f.value["association_directory"] = str(path)
    else:
        f.gateway.path.write_bytes(b"changed gateway")
    before = images(f)
    with pytest.raises((CustodyError, DraftConflict)):
        f.admission()
    assert images(f) == before
    root = updates.update_directory(f.directory, f.identity)
    assert not list(root.glob("admission-*.json"))
    assert not (root / module.ATTEMPT_NAME).exists()


async def test_changed_package_policy_stays_refused_even_with_explicit_native_pins(inputs):
    f = inputs
    root = updates.update_directory(f.directory, f.identity)
    update = updates.transaction(f.directory, f.identity)[2]
    _, candidate = updates.checked_package(root, "candidate.zip", update["package_sha256"])
    candidate["application/tool-policy.json"] = (POLICY.replace('"revision": 1', '"revision": 2').encode(), 0o644)
    import io
    import zipfile
    stream = io.BytesIO()
    with zipfile.ZipFile(stream, "w") as archive:
        for name, (content, mode) in candidate.items():
            item = zipfile.ZipInfo(name)
            item.external_attr = (mode | 0o100000) << 16
            archive.writestr(item, content)
    payload = stream.getvalue()
    updates.replace_file(root / "candidate.zip", payload, 0o600)
    updates.checkpoint(f.directory, root, update, package_sha256=updates.digest(payload), blockers=[])
    before = images(f)
    with pytest.raises(DraftConflict, match="tool access|permission.update"):
        f.admission()
    assert images(f) == before
    assert not (root / module.ATTEMPT_NAME).exists()


async def test_retained_envelope_and_attempt_bytes_cannot_be_reselected(inputs):
    f = inputs
    admission, selected, attempt = f.dispatch()
    path = Path(admission["path"])
    original = path.read_bytes()
    altered = json.loads(original)
    altered["application_startup"]["expected_revision"] = None
    path.write_bytes(encoded(altered))
    with pytest.raises(DraftConflict, match="admission changed"):
        module.ReplacementFactory(f.directory, "update", admission, attempt)
    path.write_bytes(original)
    attempt.path.write_bytes(attempt.read() + b" ")
    with pytest.raises(CustodyError, match="selected bytes"):
        module.ReplacementFactory(f.directory, "update", admission, attempt)
    assert selected["application_startup"]["expected_revision"] == 7


async def test_fresh_admission_cannot_reset_original_effect_possible_attempt(inputs):
    f = inputs
    admission, selected, attempt = f.dispatch()
    before = attempt.read()
    with pytest.raises((CustodyError, DraftConflict), match="retained attempt|already has"):
        f.admission()
    with pytest.raises(CustodyError, match="retained attempt"):
        module.retain_attempt(f.directory, admission, selected)
    assert attempt.read() == before
    for mode in ("rollback", "recover-current", "start", "install"):
        with pytest.raises(DraftConflict):
            updates.retain_update_admission(f.directory, selected["binding"], mode,
                application_startup=f.value)


@pytest.mark.parametrize("revision", [7, None])
async def test_factory_keeps_actual_child_lease_source_and_selected_inputs(inputs, revision):
    f = inputs
    f.value["expected_revision"] = revision
    admission, _, attempt = f.dispatch()
    factory = module.ReplacementFactory(f.directory, "update", admission, attempt)
    with project_lock(f.destination) as lease:
        async with factory(lease) as startup:
            assert startup.lease is lease
            assert startup.source is factory.source
            startup.source.require_owner_current()
            await startup.source.require_current_async()
            assert startup.selection_file.pin() == f.selection.pin()
            assert startup.prior_tool_policy.pin() == f.prior.pin()
            assert startup.expected_revision is revision or startup.expected_revision == revision
            assert not factory.source._closed
        assert factory.source._closed
        lease.require_current()


async def test_same_factory_and_new_factory_cannot_reenter_original_selected_child(inputs):
    f = inputs
    admission, _, attempt = f.dispatch()
    first = module.ReplacementFactory(f.directory, "update", admission, attempt)
    second = module.ReplacementFactory(f.directory, "update", admission, attempt)
    with project_lock(f.destination) as lease:
        async with first(lease):
            pass
        with pytest.raises(CustodyError, match="already entered"):
            async with first(lease):
                pytest.fail("Object-local factory reuse entered selected startup.")
        with pytest.raises(CustodyError, match="already entered"):
            async with second(lease):
                pytest.fail("A fresh factory reused the original replacement entry.")
        assert second.source is None or second.source._closed


async def test_factory_preserves_original_failure_and_failed_same_runtime_cleanup(inputs):
    f = inputs
    admission, _, attempt = f.dispatch()
    factory = module.ReplacementFactory(f.directory, "update", admission, attempt)
    original = ValueError("original selected application failure")
    cleanup = RuntimeError("original runtime cleanup unconfirmed")
    calls = []
    class Runtime:
        async def shutdown(self):
            calls.append(self)
            raise cleanup
    runtime = Runtime()
    with project_lock(f.destination) as lease:
        try:
            with pytest.raises(BaseExceptionGroup) as retained:
                async with factory(lease) as startup:
                    startup.runtime = runtime
                    raise original
            assert retained.value.exceptions == (original, cleanup)
            assert factory.startup.runtime is runtime and not factory.source._closed
            with pytest.raises(RuntimeError) as repeated:
                await factory.startup.shutdown(runtime)
            assert repeated.value is cleanup and calls == [runtime]
            lease.require_current()
            assert attempt.read()
        finally:
            # The no-effect specimen has no physical resource. Release only
            # the test's retained file descriptors after checking uncertainty.
            factory.source.close()


async def test_input_swap_after_joined_child_read_refuses_before_runtime_preparation(inputs, monkeypatch):
    f = inputs
    admission, _, attempt = f.dispatch()
    factory = module.ReplacementFactory(f.directory, "update", admission, attempt)
    original = module.selected_inputs
    def changed(*args, **kwargs):
        value = original(*args, **kwargs)
        f.prior.path.write_bytes(b"swapped actual prior")
        return value
    monkeypatch.setattr(module, "selected_inputs", changed)
    app = SimpleNamespace(context=SimpleNamespace(root=f.destination, runtime=None),
        dynamic=False, _application_callbacks=None)
    with project_lock(f.destination) as lease:
        with pytest.raises(CustodyError):
            async with factory(lease) as startup:
                await startup.prepare(app)
                pytest.fail("A changed prior completed selected runtime preparation.")
    assert factory.startup is None or factory.startup.runtime is None
    assert app.context.runtime is None
    assert factory.source is None or factory.source._closed


async def test_fixed_child_withholds_authored_import_after_valid_previous_source_restore(inputs, monkeypatch):
    f = inputs
    admission, selected, attempt = f.dispatch()
    monkeypatch.setattr(updates, "require_no_replacement_replay", lambda root: None)
    # An independently owned source action selects the still-valid previous
    # archive. The child must reject that source before invoking authored main.
    updates.restore(f.directory, f.identity, restore_data=False,
        expected_binding=selected["binding"], expected_installation=selected["installation"])
    calls = []
    app = ModuleType("app")
    app.__file__ = str(f.destination / "app.py")
    app.main = lambda **kwargs: calls.append(kwargs)
    monkeypatch.setitem(sys.modules, "app", app)
    with pytest.raises(CustodyError, match="candidate changed|admitted candidate"):
        module.run_child(str(f.directory), "update", admission["path"], admission["sha256"],
            str(attempt.path), attempt.sha256)
    assert calls == []


class InertSupervisor(worker.Supervisor):
    def __init__(self, directory):
        super().__init__(directory)
        self.commands, self.starts, self.observations = [], [], []
        self.start_error = RuntimeError("Inert selected startup stops at the dispatch boundary.")

    async def command(self, argv, *args, **kwargs):
        self.commands.append(tuple(argv))

    async def native_definition_revision(self, *args, **kwargs):
        self.observations.append((args, kwargs))
        raise AssertionError("Selected replacement must not open a second native observation runtime.")

    async def start(self, definition, *, on_ready=None, application_startup=None):
        self.starts.append((deepcopy(definition), application_startup))
        if self.start_error is not None:
            raise self.start_error
        # This is only the existing owner's callback seam, never native Ready.
        on_ready()


async def test_selected_dispatch_skips_separate_activation_and_blocks_uncertain_fallback(inputs):
    f = inputs
    root, update = updates.transaction(f.directory, f.identity)[1:]
    # Source-only owner specimen marks a native definition transition. Its
    # literal candidate TOML/roster remain authenticated by production reads.
    updates.checkpoint(f.directory, root, update, native_definition_update=True)
    admission = f.admission()
    supervisor = InertSupervisor(f.directory)
    before_records = (f.destination / "data/ledger.db").read_bytes()
    with pytest.raises(RuntimeError, match="Inert selected startup"):
        await worker.update_runtime(supervisor, "update", update_selection=admission)
    assert supervisor.observations == []
    assert all("--activate-revision" not in argv for argv in supervisor.commands)
    assert len(supervisor.starts) == 1
    selected = supervisor.starts[0][1]
    assert selected["admission"] == admission and selected["mode"] == "update"
    assert selected["attempt"].path == root / module.ATTEMPT_NAME
    receipt = deployment.read_receipt(f.directory)
    assert receipt["build_id"] == receipt["update"]["to_build_id"]
    assert receipt["update"]["status"] == "recovery-needed"
    assert not receipt["update"]["rollback_available"]
    assert (f.destination / "application/logic.py").read_text() == "VERSION = 'new'\n"
    assert (f.destination / "data/ledger.db").read_bytes() == before_records


async def test_replacement_unknown_history_cannot_be_hidden_by_new_ordinary_update(inputs):
    f = inputs
    admission = f.admission()
    supervisor = InertSupervisor(f.directory)
    supervisor.start_error = None
    await worker.update_runtime(supervisor, "update", update_selection=admission)
    receipt = deployment.read_receipt(f.directory)
    assert receipt["update"]["status"] == "completed"
    payload = (f.directory / "package.zip").read_bytes()
    _, source = updates.checked_package(f.directory, "package.zip", receipt["package_sha256"])
    report = json.loads(source["implementation-report.json"][0])
    before = (f.directory / "receipt.json").read_bytes()
    with pytest.raises(DraftConflict, match="retained native attempt|native reconciliation"):
        updates.stage(f.directory, payload, report)
    assert (f.directory / "receipt.json").read_bytes() == before
    assert deployment.read_receipt(f.directory)["update"]["update_id"] == f.identity


async def test_exclusive_attempt_persists_parent_directory_before_dispatch(inputs, monkeypatch):
    f = inputs
    admission = f.admission()
    selected = f.selected(admission)
    original = os.fsync
    synced = []
    def observed(fd):
        synced.append(stat.S_IFMT(os.fstat(fd).st_mode))
        return original(fd)
    monkeypatch.setattr(os, "fsync", observed)
    attempt = module.retain_attempt(f.directory, admission, selected)
    assert attempt.read()
    assert stat.S_IFREG in synced and stat.S_IFDIR in synced


@pytest.mark.parametrize("pin", ["selection", "gateway", "prior", "activation"])
async def test_selected_input_handoff_swap_refuses_before_source_copy_or_command(inputs, monkeypatch, pin):
    f = inputs
    admission = f.admission()
    supervisor = InertSupervisor(f.directory)
    original, activate = module.selected_inputs, updates.activate
    copied = []
    changed = [False]
    def swapped(*args, **kwargs):
        value = original(*args, **kwargs)
        if not changed[0]:
            changed[0] = True
            selected = getattr(f, pin)
            selected.path.write_bytes(selected.path.read_bytes() + b" ")
        return value
    def copy(*args, **kwargs):
        copied.append(True)
        return activate(*args, **kwargs)
    monkeypatch.setattr(module, "selected_inputs", swapped)
    monkeypatch.setattr(updates, "activate", copy)
    before = images(f)
    with pytest.raises((CustodyError, DraftConflict, RuntimeError)):
        await worker.update_runtime(supervisor, "update", update_selection=admission)
    assert copied == [], "Changed selected inputs must refuse before admitted source replacement."
    assert supervisor.commands == [] and supervisor.starts == [] and supervisor.observations == []
    assert images(f) == before


async def test_selected_input_thread_stays_joined_through_repeated_owner_cancellation(inputs, monkeypatch):
    f = inputs
    admission = f.admission()
    supervisor = InertSupervisor(f.directory)
    original = module.selected_inputs
    release, entered, exited = threading.Event(), asyncio.Event(), []
    loop = asyncio.get_running_loop()
    def held(*args, **kwargs):
        value = original(*args, **kwargs)
        loop.call_soon_threadsafe(entered.set)
        assert release.wait(5), "Test-owned file reader was not released."
        exited.append(True)
        return value
    monkeypatch.setattr(module, "selected_inputs", held)
    before = images(f)
    task = asyncio.create_task(worker.update_runtime(supervisor, "update", update_selection=admission))
    try:
        await asyncio.wait_for(entered.wait(), 5)
        task.cancel()
        await asyncio.sleep(0)
        task.cancel()
        await asyncio.sleep(0)
        assert not task.done(), "Original owner must retain the pending file worker."
        assert supervisor.commands == [] and supervisor.starts == []
    finally:
        release.set()
    with pytest.raises(asyncio.CancelledError):
        await asyncio.wait_for(task, 5)
    assert exited == [True] and images(f) == before
    assert not (updates.update_directory(f.directory, f.identity) / module.ATTEMPT_NAME).exists()


@pytest.mark.parametrize("pin", ["selection", "gateway", "prior", "activation"])
async def test_selected_snapshot_return_swap_refuses_before_application_source_copy(inputs, monkeypatch, pin):
    f = inputs
    admission = f.admission()
    supervisor = InertSupervisor(f.directory)
    original, apply = updates.snapshot, updates.apply_files
    copies = []
    changed = [False]
    def snapshot_then_swap(*args, **kwargs):
        result = original(*args, **kwargs)
        if not changed[0]:
            changed[0] = True
            selected = getattr(f, pin)
            selected.path.write_bytes(selected.path.read_bytes() + b" ")
        return result
    def copy(*args, **kwargs):
        copies.append(True)
        return apply(*args, **kwargs)
    monkeypatch.setattr(updates, "snapshot", snapshot_then_swap)
    monkeypatch.setattr(updates, "apply_files", copy)
    before = images(f)
    with pytest.raises((CustodyError, DraftConflict, RuntimeError)):
        await worker.update_runtime(supervisor, "update", update_selection=admission)
    assert copies == [], "Selected native input changes during snapshot must refuse before source copy."
    assert supervisor.commands == [] and supervisor.starts == [] and supervisor.observations == []
    assert images(f) == before


async def test_ordinary_update_uses_original_dispatch_without_selected_native_inputs(inputs):
    f = inputs
    update = updates.transaction(f.directory, f.identity)[2]
    admission = updates.retain_update_admission(f.directory, update, "update")
    supervisor = InertSupervisor(f.directory)
    supervisor.start_error = None
    await worker.update_runtime(supervisor, "update", update_selection=admission)
    assert len(supervisor.starts) == 1 and supervisor.starts[0][1] is None
    assert supervisor.observations == []
    assert not (updates.update_directory(f.directory, f.identity) / module.ATTEMPT_NAME).exists()
    assert deployment.read_receipt(f.directory)["update"]["status"] == "completed"


async def test_fixed_launcher_imports_trusted_owner_before_authored_path(inputs):
    f = inputs
    admission, _, attempt = f.dispatch()
    arguments = module.child_arguments(f.directory, "update", admission, attempt)
    assert arguments[:2] == ("-I", "-c") and arguments[3:] == (
        str(f.directory.absolute()), "update", admission["path"], admission["sha256"],
        str(attempt.path), attempt.sha256)
    trusted = str(Path(module.__file__).resolve().parent.parent)
    assert repr(trusted) in arguments[2]
    assert arguments[2].index("sys.path.insert") < arguments[2].index("from launchkit")
    assert str(f.destination) not in arguments[2]


async def test_selected_sdk_imports_precede_authored_project_shadow_packages(inputs, monkeypatch, tmp_path):
    f = inputs
    admission, _, attempt = f.dispatch()
    sdk = tmp_path / "isolated-selected-sdk"
    sdk.mkdir(mode=0o700)
    specimens = {
        "meerkat_meta/__init__.py": "",
        "meerkat_meta/builder.py": "class MetaBuilderConfig: pass\n",
        "meerkat_toolkit/__init__.py": "",
        "meerkat_toolkit/host_control.py": "class ApplicationUpdateStartup: pass\nclass SelectedHostArtifact: pass\n",
        "meerkat_toolkit/application_update.py": "class ToolkitApplicationUpdateRuntime: pass\n",
    }
    for name, content in specimens.items():
        genuine = sdk / name
        genuine.parent.mkdir(mode=0o700, exist_ok=True)
        genuine.write_text(content)
        shadow = f.destination / name
        shadow.parent.mkdir(mode=0o700, exist_ok=True)
        shadow.write_text("raise AssertionError('Authored project shadowed the selected SDK import.')\n")
    for name in list(sys.modules):
        if name == "meerkat_meta" or name.startswith("meerkat_meta.") or name == "meerkat_toolkit" or name.startswith("meerkat_toolkit."):
            monkeypatch.delitem(sys.modules, name)
    monkeypatch.setattr(sys, "path", [str(sdk), *sys.path])
    monkeypatch.setattr(sys, "argv", list(sys.argv))
    origins = []
    app = ModuleType("app")
    app.__file__ = str(f.destination / "app.py")
    def authored_main(**kwargs):
        from meerkat_meta.builder import MetaBuilderConfig
        from meerkat_toolkit.application_update import ToolkitApplicationUpdateRuntime
        from meerkat_toolkit.host_control import ApplicationUpdateStartup, SelectedHostArtifact
        for cls in (MetaBuilderConfig, ToolkitApplicationUpdateRuntime, ApplicationUpdateStartup, SelectedHostArtifact):
            origins.append(Path(sys.modules[cls.__module__].__file__))
        assert kwargs["application_startup_factory"].selected["binding"]["update_id"] == f.identity
    app.main = authored_main
    monkeypatch.setitem(sys.modules, "app", app)
    try:
        module.run_child(str(f.directory), "update", admission["path"], admission["sha256"],
            str(attempt.path), attempt.sha256)
        assert len(origins) == 4 and all(sdk in path.parents for path in origins)
    finally:
        # Modules imported by this test are its synthetic SDK specimens.
        for name in list(sys.modules):
            if name == "meerkat_meta" or name.startswith("meerkat_meta.") or name == "meerkat_toolkit" or name.startswith("meerkat_toolkit."):
                sys.modules.pop(name, None)
