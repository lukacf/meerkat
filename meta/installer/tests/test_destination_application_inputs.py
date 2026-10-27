"""Actual destination source/default owners with inert external SDK boundaries.

The original artifact receiver, package/source intake, canonical Install, host,
private source and preparation readers remain real. The reused fixture replaces
venv/pip/application-check effects with poisoned selected bytes and full installed
prefix/SDK comparisons. Inventory stdout is a retained synthetic native specimen,
never native authority, qualification, Ready, a running app or a GCP journey.
"""
import asyncio
from copy import deepcopy
import hashlib
import json
from pathlib import Path
import threading
import sys
from types import ModuleType, SimpleNamespace
from uuid import uuid4

import pytest

from launchkit import deployment, deployment_worker, remote_product_destination_owner
from launchkit import compiler, local_application_source
from launchkit import deployment_application_bootstrap as bootstrap
from launchkit.blueprint import Blueprint
from launchkit.connect_installer_caller import CustodyError, ExactFile
from launchkit.destination_application_inputs import (
    SOURCE_DIRECTORY, DestinationApplicationInputs, create_destination_application_inputs,
    recover_destination_application_inputs, _source_contents,
)
from launchkit.forge_application_host import ApplicationPreparationInputs, ForgeApplicationHost
from launchkit.local_application_preparation_inputs import LocalApplicationPreparationOwner
from launchkit.product_installed_source import open_installed_product_source
from launchkit.remote_product_artifacts import ProductArtifactTransferError
from launchkit.runtime_candidates import CandidateArtifactError
from launchkit.templates.storage_setup import project_lock
import test_product_destination_install as install_fixture
from test_destination_application_factory import destination
from test_product_destination_install import installation, destination_source, artifact_custody, product
from test_product_destination_owner import no_destination_execution
from test_meta_toolkit_runtime_selection import no_product_execution
from test_local_application_restore import completed_specimen
from test_local_application_source import cloud_plan
from test_meta_toolkit_initial_package import setup_spec
import test_application_finalizer as finalizer_fixture
from test_application_finalizer import FinalizerFixture


@pytest.fixture
def inputs_destination(request, monkeypatch):
    # Select the real AccessControlConfig source shape before the original ZIP
    # and stage. The old byte-install fixture's nested [access] is not this new
    # explicit source choice. No accepted installed source is edited here.
    package = install_fixture.package_bytes

    def original_package(files):
        chosen = dict(files)
        chosen["config/access.toml"] = (
            b'enabled = true\nadmins = ["fixture-operator"]\nrules = []\n\n[groups]\n')
        if getattr(request, "param", None) == "two-admins":
            chosen["config/access.toml"] = b'enabled = true\nadmins = ["fixture-operator", "foreign-operator"]\n'
        elif getattr(request, "param", None) == "nested-access":
            chosen["config/access.toml"] = b'[access]\nenabled = true\nadmins = ["fixture-operator"]\n'
        return package(chosen)

    monkeypatch.setattr(install_fixture, "package_bytes", original_package)
    f = request.getfixturevalue("destination")
    f.input_owner = None
    f.parent = f.loop = f.thread = None

    def current():
        assert asyncio.current_task() is f.parent
        assert asyncio.get_running_loop() is f.loop
        assert threading.get_ident() == f.thread
        if not f.current:
            raise CustodyError("Original destination supplier owner changed.")

    f.supplier_current = current
    return f


def select_caller(f):
    f.parent, f.loop, f.thread = asyncio.current_task(), asyncio.get_running_loop(), threading.get_ident()


async def create(f):
    select_caller(f)
    async with f.service.lock:
        with project_lock(f.selected.destination) as lease:
            with remote_product_destination_owner.open_stopped_product_source(
                    f.selected.context, project_lease=lease, require_current=f.supplier_current,
                    owner_source=f.selected.source) as source:
                f.source = source
                f.input_owner = await create_destination_application_inputs(source,
                    deployment=f.service, require_current=f.supplier_current)
                f.factory = f.input_owner.factory
                assert not (f.selected.destination / ".env").exists()
                source.require_current()
                return f.input_owner


async def install(f):
    owner = await create(f)
    original = {name: (f.selected.destination / name).read_bytes() for name in f.selected.files}
    result = await owner.factory.install(settings=f.settings)
    f.held = owner.preparation_owner._inputs
    assert type(f.held) is ApplicationPreparationInputs
    assert owner.host._installation_supervisor is f.held.supervisor
    assert original == {name: (f.selected.destination / name).read_bytes() for name in f.selected.files}
    return owner, result


def snapshot(root):
    return {path.relative_to(root).as_posix(): (path.read_bytes(), path.stat().st_mode)
        for path in root.rglob("*") if path.is_file() and not path.is_symlink()}


async def reopen(f, service, *, source_view=None):
    select_caller(f)
    async with service.lock:
        with project_lock(f.selected.destination) as lease:
            async with open_installed_product_source(f.directory, project_lease=lease,
                    require_current=f.supplier_current, owner_source=f.selected.source,
                    application_startup=source_view) as source:
                assert source is not None
                recovered = await recover_destination_application_inputs(source,
                    deployment=service, require_current=f.supplier_current)
                source.require_current()
                return recovered


async def prepare_original(f, owner, monkeypatch):
    original = owner.host._inventory
    f.actual = {"schema": 1, "mob_id": original["mob_id"],
        "helper_identity": original["helper_identity"], "realm_id": "inert-destination-realm",
        "members": [{"identity": row["identity"], "member": {"mob_id": original["mob_id"],
            "role": row["profile"], "member": "inert-member-" + str(i)}}
            for i, row in enumerate(original["roster"])]}
    monkeypatch.setattr(f.held.supervisor, "command", f.inventory_command)
    result = await f.service.action(f.factory.guide_id, f.factory.deployment_id,
        "prepare-setup", action_request_id=str(uuid4()))
    assert result["action_request"]["phase"] == "settled"
    assert result["action_request"]["outcome"] == "returned" and len(f.inventory_calls) == 1
    return result


async def finalize_original_destination(f, owner, monkeypatch):
    """Real entered Fresh/finalizer readers around explicit inert native bytes."""
    f.inputs, f.host = f.held, owner.host
    f.destination, f.setup_root = f.root, owner.preparation_owner._choices_pin.path.parent
    f.fresh = json.loads(f.factory._startup_pin.read())
    specimen = completed_specimen(f)
    commands = []
    remote = ModuleType("remote_operations")
    remote.__file__ = str(f.root / "remote_operations.py")
    remote.command = lambda *args, **kwargs: pytest.fail("This finalizer specimen cannot select application code.")
    monkeypatch.setitem(sys.modules, "remote_operations", remote)
    executor = SimpleNamespace(f=SimpleNamespace(root=f.root), commands=commands,
        on_prepare=lambda: None, on_validate=lambda: None, prepare_process=None, validate_process=None)
    async def execute(argv):
        write = finalizer_fixture.write
        arguments = dict(zip(argv[2::2], argv[3::2]))
        def native_output(path, content):
            if path.name == "toolkit-local-policy.json":
                content = Path(arguments["--policy"]).read_bytes()
            return write(path, content)
        with monkeypatch.context() as patch:
            patch.setattr(finalizer_fixture, "write", native_output)
            return await FinalizerFixture.execute(executor, argv)
    async with f.service.lock:
        async with f.factory._operation(installed=True):
            source = f.factory._source
            admission, _ = bootstrap.retain_bootstrap_admission(f.directory,
                deployment.read_receipt(f.directory), f.fresh, source_custody=source)
            selected = bootstrap.BootstrapFactory(f.directory, admission, source_custody=source)
            async with selected(source._lease) as startup:
                app = SimpleNamespace(context=SimpleNamespace(root=f.root, runtime=None),
                    dynamic=False, _application_callbacks=None)
                await startup.prepare(app)
                startup.caller.execute = execute
                await startup.finalizer.finalize_selection(specimen.manifest)
            source.require_current()
    assert [row[1] for row in commands] == ["--prepare-application-policy", "--validate-application-policy"]
    return commands


@pytest.mark.parametrize("central_context", [True, False])
async def test_destination_supplier_preserves_source_and_retains_real_install_defaults(inputs_destination, central_context):
    f = inputs_destination
    if not central_context:
        # A real destination service does not receive dummy central stores.
        # Initial Install/default ownership only needs its canonical receipt.
        f.service = deployment.DeploymentService(f.selected.custody.home / ".local/state/meta-forge", None, None)
    owner, installer = await install(f)
    assert type(owner) is DestinationApplicationInputs
    assert type(owner.host) is ForgeApplicationHost
    assert type(owner.preparation_owner) is LocalApplicationPreparationOwner
    assert owner.host._destination_installation_supervisor is installer
    assert f.supervisors == [installer, f.held.supervisor]
    assert f.factory._source is None and f.held.supervisor._product_source_custody._closed
    assert f.service.application_host_selection(f.factory.guide_id, f.factory.deployment_id) is owner.host
    chosen = json.loads(owner.preparation_owner._choices_pin.read())
    assert chosen["namespace"] == f.selected.context["deployment_id"] and chosen["generation"] == 1
    assert chosen["console_operator"]["id"] == "fixture-operator"
    assert chosen["console_setup"] == {"origin": "http://127.0.0.1:" + str(f.spec["advanced"]["port"]),
        "access_management": "requester_issued_leaves"}
    assert chosen["console_setup_source"] is None
    assert deployment.read_receipt(f.directory)["phase"] == "configure"
    assert not f.held.inventory_attempt_path.exists() and not f.held.preparation_plan_path.exists()
    with project_lock(f.selected.destination):
        pass


async def test_destination_supplier_prepares_through_original_authenticated_inventory(inputs_destination, monkeypatch):
    f = inputs_destination
    owner, _ = await install(f)
    await prepare_original(f, owner, monkeypatch)
    assert f.factory._startup_pin.path == f.held.startup_selection_path
    plan = json.loads(f.held.preparation_plan_path.read_bytes())
    assert plan["startup"] == {"kind": "initialize", "namespace": f.factory.deployment_id, "generation": 1}
    assert plan["application"]["realm"] == {"realm_id": f.actual["realm_id"],
        "context_root": str(f.root), "state_root": str(f.root / ".mobkit/state")}
    assert "account_issuance_id" not in plan["authorities"]["helper_report"]
    assert f.held.supervisor._product_source_custody._closed


async def test_destination_restore_borrows_original_scope_and_recovery_never_replays_fresh(
        inputs_destination, monkeypatch):
    f = inputs_destination
    owner, _ = await install(f)
    await prepare_original(f, owner, monkeypatch)
    commands = await finalize_original_destination(f, owner, monkeypatch)
    async with f.service.lock:
        await f.factory.continue_application_startup(f.service, f.factory.guide_id, f.factory.deployment_id)
    pin = owner.preparation_owner._restore_pin
    assert type(pin) is ExactFile and json.loads(pin.read())["kind"] == "restore"
    before = snapshot(f.root), snapshot(f.directory), pin.read()
    service = deployment.DeploymentService(f.selected.custody.home / ".local/state/meta-forge",
        f.guide, f.implementation)
    def no_retain(*args, **kwargs):
        pytest.fail("Prepared recovery cannot replay successful-Install defaults.")
    async def no_command(*args, **kwargs):
        pytest.fail("Prepared recovery cannot replay inventory, Fresh or finalization commands.")
    monkeypatch.setattr(LocalApplicationPreparationOwner, "retain", no_retain)
    monkeypatch.setattr(deployment_worker.Supervisor, "command", no_command)
    recovered = await reopen(f, service, source_view=json.loads(pin.read()))
    assert recovered.preparation_owner._restore_pin == pin
    assert recovered.factory._startup_pin == pin and recovered.factory._install_entered
    assert before == (snapshot(f.root), snapshot(f.directory), pin.read())
    assert len(f.inventory_calls) == 1 and len(commands) == 2
    assert recovered.host._product_source_custody._closed


async def test_destination_supplier_reconstructs_private_owner_without_default_or_inventory_replay(
        inputs_destination, monkeypatch):
    f = inputs_destination
    original, _ = await install(f)
    before = snapshot(f.root), snapshot(f.directory)
    service = deployment.DeploymentService(f.selected.custody.home / ".local/state/meta-forge",
        f.guide, f.implementation)

    def no_retain(*args, **kwargs):
        pytest.fail("Detached recovery cannot replay initial private defaults.")
    async def no_command(*args, **kwargs):
        pytest.fail("Detached recovery cannot replay inventory or native work.")
    monkeypatch.setattr(LocalApplicationPreparationOwner, "retain", no_retain)
    monkeypatch.setattr(deployment_worker.Supervisor, "command", no_command)
    recovered = await reopen(f, service)
    assert type(recovered) is DestinationApplicationInputs and recovered is not original
    assert recovered.preparation_owner._inputs.supervisor is recovered.host._installation_supervisor
    assert recovered.host._destination_installation_supervisor is None
    assert recovered.factory._install_entered and recovered.factory._startup_pin is None
    assert recovered.host._product_source_custody._closed
    assert recovered.preparation_owner._choices_pin.read() == original.preparation_owner._choices_pin.read()
    assert before == (snapshot(f.root), snapshot(f.directory))
    assert len(f.supervisors) == 3 and not f.inventory_calls


@pytest.mark.parametrize("member", ["local-application-policy.json", "activation-template.json"])
async def test_destination_recovery_refuses_changed_original_private_inputs_before_supervisor(
        inputs_destination, monkeypatch, member):
    f = inputs_destination
    owner, _ = await install(f)
    path = owner.preparation_owner._choices_pin.path.parent / member
    path.write_bytes(path.read_bytes() + b"\n")
    service = deployment.DeploymentService(f.selected.custody.home / ".local/state/meta-forge",
        f.guide, f.implementation)
    def no_constructor(*args, **kwargs):
        pytest.fail("Changed original private inputs cannot construct a recovered Supervisor.")
    monkeypatch.setattr(deployment_worker.Supervisor, "__init__", no_constructor)
    with pytest.raises(CustodyError, match="policy or activation"):
        await reopen(f, service)
    assert service.application_host_selection is None


async def test_destination_source_projection_loss_retains_partial_files_without_owner_or_retry(
        inputs_destination, monkeypatch):
    f = inputs_destination
    from launchkit import destination_application_inputs as supplier
    previous = supplier._write_bytes
    writes = []
    def lose_owner(path, content):
        pin = previous(path, content)
        writes.append(path)
        f.current = False
        return pin
    monkeypatch.setattr(supplier, "_write_bytes", lose_owner)
    with pytest.raises(CustodyError, match="supplier owner"):
        await create(f)
    assert len(writes) == 1 and writes[0].read_bytes() == f.selected.files["blueprint.json"]
    assert f.input_owner is None and f.service.application_host_selection is None
    assert not f.directory.exists() and not (f.root / ".env").exists()
    f.current = True
    with pytest.raises(CustodyError, match="fresh"):
        await create(f)
    assert len(writes) == 1


async def test_destination_recovery_refuses_partial_preparation_without_inventory_replay(inputs_destination, monkeypatch):
    f = inputs_destination
    owner, _ = await install(f)
    partial = owner.preparation_owner._inputs.inventory_attempt_path
    partial.write_bytes(b'{"status":"entered"}\n')
    partial.chmod(0o600)
    service = deployment.DeploymentService(f.selected.custody.home / ".local/state/meta-forge",
        f.guide, f.implementation)
    def no_constructor(*args, **kwargs):
        pytest.fail("Partial preparation refuses before a recovered Supervisor.")
    monkeypatch.setattr(deployment_worker.Supervisor, "__init__", no_constructor)
    with pytest.raises(CustodyError, match="exists|fresh"):
        await reopen(f, service)
    assert partial.read_bytes() == b'{"status":"entered"}\n'
    assert service.application_host_selection is None


async def test_destination_supplier_refuses_closed_source_and_foreign_service_before_projection(inputs_destination):
    f = inputs_destination
    select_caller(f)
    async with f.service.lock:
        with project_lock(f.root) as lease:
            with remote_product_destination_owner.open_stopped_product_source(f.selected.context,
                    project_lease=lease, require_current=f.supplier_current,
                    owner_source=f.selected.source) as source:
                foreign = deployment.DeploymentService(f.selected.custody.home / "foreign-state",
                    f.guide, f.implementation)
                async with foreign.lock:
                    with pytest.raises(CustodyError, match="canonical"):
                        await create_destination_application_inputs(source, deployment=foreign,
                            require_current=f.supplier_current)
            with pytest.raises((CustodyError, ProductArtifactTransferError), match="closed|source"):
                await create_destination_application_inputs(source, deployment=f.service,
                    require_current=f.supplier_current)
    assert not (f.root / SOURCE_DIRECTORY).exists() and not f.directory.exists()


async def test_preexisting_configuration_is_not_adopted_as_initial_destination_defaults(inputs_destination):
    f = inputs_destination
    configuration = f.root / ".env"
    configuration.write_bytes(b"UNSELECTED_PRIVATE_INPUT=inert\n")
    configuration.chmod(0o600)
    with pytest.raises(CustodyError, match="owned or partial state"):
        await create(f)
    assert configuration.read_bytes() == b"UNSELECTED_PRIVATE_INPUT=inert\n"
    assert not (f.root / SOURCE_DIRECTORY).exists() and not f.directory.exists()
    assert f.service.application_host_selection is None


@pytest.mark.parametrize("member", ["application/tool-policy.json", "application/console.json"])
async def test_destination_supplier_refuses_changed_accepted_source_before_projection(inputs_destination, member):
    f = inputs_destination
    path = f.root / member
    path.write_bytes(path.read_bytes() + b"\n")
    with pytest.raises((CustodyError, ProductArtifactTransferError, CandidateArtifactError), match="source|byte|changed"):
        await create(f)
    assert not (f.root / SOURCE_DIRECTORY).exists() and f.service.application_host_selection is None


@pytest.mark.parametrize("inputs_destination", ["two-admins", "nested-access"], indirect=True)
async def test_original_ambiguous_or_nonshipping_operator_choice_refuses_before_effects(inputs_destination):
    f = inputs_destination
    with pytest.raises(CustodyError, match="single protected Console operator"):
        await create(f)
    assert not (f.root / SOURCE_DIRECTORY).exists() and not f.directory.exists()
    assert not (f.root / ".env").exists() and f.service.application_host_selection is None


@pytest.mark.parametrize("case", ["local", "cloud", "native-bundle", "helper"])
def test_destination_source_names_the_actual_missing_input_of_an_unsupported_plan(case):
    # Only a plan that selects the helper is pending connection setup support. A
    # plan without one names the input it lacks, as its local Build does.
    value = setup_spec()
    if case != "helper":
        value["acquisition"] = None
        for index, row in enumerate(value["agent_classes"]):
            row["tools"]["rust_bundles"] = ["foreign.native"] if case == "native-bundle" and index == 0 else []
    if case in {"cloud", "helper"}:
        # The helper plan targets the cloud without a selected Linux product.
        value = cloud_plan(value)
    files = {"blueprint.json": (json.dumps(Blueprint.model_validate(value).model_dump(mode="json")).encode(),)}
    expected = {"local": local_application_source.ORDINARY_SOURCE_UNAVAILABLE,
        "cloud": local_application_source.ORDINARY_TARGET_UNSUPPORTED,
        "native-bundle": compiler.NATIVE_BUNDLE_PENDING, "helper": compiler.CONNECTION_SETUP_PENDING}[case]
    with pytest.raises(CustodyError) as refused:
        _source_contents(files, None)
    assert str(refused.value) == expected
    assert (case == "helper") == ("selects a connection setup helper" in str(refused.value))


async def test_destination_input_cancel_joins_original_file_read_with_lease_held(inputs_destination, monkeypatch):
    f = inputs_destination
    from launchkit import destination_product_install
    original = destination_product_install._inputs
    entered, released = threading.Event(), threading.Event()
    worker_calls = []
    def held(source):
        result = original(source)
        worker_calls.append(threading.get_ident())
        entered.set()
        assert released.wait(5), "The owned file-read control did not release its worker."
        source.require_filesystem_current()
        return result
    monkeypatch.setattr(destination_product_install, "_inputs", held)
    async def begin():
        f.parent = asyncio.current_task()
        return await create(f)
    task = asyncio.create_task(begin())
    try:
        assert await asyncio.to_thread(entered.wait, 5)
        task.cancel()
        await asyncio.sleep(0)
        assert not task.done()
        with pytest.raises(RuntimeError, match="running"):
            with project_lock(f.root):
                pytest.fail("Cancellation released the original source lease before its worker joined.")
        task.cancel()
        released.set()
        with pytest.raises(asyncio.CancelledError):
            await task
    finally:
        released.set()
        if not task.done():
            task.cancel()
        try:
            await task
        except asyncio.CancelledError:
            pass
    assert len(worker_calls) == 1 and worker_calls[0] != f.thread
    assert not (f.root / SOURCE_DIRECTORY).exists() and f.service.application_host_selection is None
    with project_lock(f.root):
        pass


async def test_destination_scope_preserves_first_owner_refusal_on_exit(inputs_destination):
    f = inputs_destination
    owner, _ = await install(f)
    async with f.service.lock:
        with pytest.raises(CustodyError, match="Original destination supplier owner changed") as raised:
            async with owner.factory._operation(installed=True):
                source = owner.factory._source
                lease = source._lease
                f.current = False
                source.require_owner_current()
    assert type(raised.value) is CustodyError
    assert source._invalidated and source._closed and lease._closed
    assert owner.factory._source is None and owner.factory._pending_scope is None
    assert not f.inventory_calls
    f.current = True


async def test_destination_recovery_requires_original_restore_view_after_entered_fresh(
        inputs_destination, monkeypatch):
    f = inputs_destination
    owner, _ = await install(f)
    await prepare_original(f, owner, monkeypatch)
    commands = await finalize_original_destination(f, owner, monkeypatch)
    async with f.service.lock:
        await f.factory.continue_application_startup(f.service, f.factory.guide_id, f.factory.deployment_id)
    restore = owner.preparation_owner._restore_pin
    fresh = json.loads(f.held.startup_selection_path.read_bytes())
    assert fresh["kind"] == "fresh" and json.loads(restore.read())["kind"] == "restore"
    before = snapshot(f.root), snapshot(f.directory), restore.read()
    service = deployment.DeploymentService(f.selected.custody.home / ".local/state/meta-forge",
        f.guide, f.implementation)
    def no_constructor(*args, **kwargs):
        pytest.fail("An entered Fresh cannot reconstruct a Supervisor without its selected Restore view.")
    async def no_command(*args, **kwargs):
        pytest.fail("History refusal cannot replay inventory or native work.")
    monkeypatch.setattr(deployment_worker.Supervisor, "__init__", no_constructor)
    monkeypatch.setattr(deployment_worker.Supervisor, "command", no_command)
    with pytest.raises(CustodyError, match="actual completed Restore selection"):
        await reopen(f, service, source_view=fresh)
    assert before == (snapshot(f.root), snapshot(f.directory), restore.read())
    assert service.application_host_selection is None
    assert len(f.inventory_calls) == 1 and len(commands) == 2


async def test_destination_recovery_refuses_foreign_restore_with_same_output_paths(
        inputs_destination, monkeypatch):
    f = inputs_destination
    owner, _ = await install(f)
    await prepare_original(f, owner, monkeypatch)
    commands = await finalize_original_destination(f, owner, monkeypatch)
    async with f.service.lock:
        await f.factory.continue_application_startup(f.service, f.factory.guide_id, f.factory.deployment_id)
    original = json.loads(owner.preparation_owner._restore_pin.read())
    foreign = deepcopy(original)
    completed = Path(original["completed_setup"]["path"])
    alternate = f.held.startup_selection_path.parent / "foreign-completed-setup.json"
    alternate.write_bytes(completed.read_bytes())
    alternate.chmod(0o600)
    foreign["completed_setup"] = ExactFile(alternate, original["completed_setup"]["sha256"]).pin()
    assert foreign["selection_file"] == original["selection_file"]
    assert foreign["preparation_plan"] == original["preparation_plan"]
    before = snapshot(f.root), snapshot(f.directory)
    service = deployment.DeploymentService(f.selected.custody.home / ".local/state/meta-forge",
        f.guide, f.implementation)
    async def no_command(*args, **kwargs):
        pytest.fail("A foreign private input cannot cause inventory or native replay.")
    monkeypatch.setattr(deployment_worker.Supervisor, "command", no_command)
    with pytest.raises(CustodyError, match="original admitted private selection"):
        await reopen(f, service, source_view=foreign)
    assert before == (snapshot(f.root), snapshot(f.directory))
    assert service.application_host_selection is None
    assert len(f.inventory_calls) == 1 and len(commands) == 2
