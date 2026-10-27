"""Real destination Install ownership with poisoned product/source specimens.

The staged source and artifact readers, selected host, receipt publisher,
Supervisor install lane and SDK/full-prefix comparators remain production code.
Only venv/pip/application-check boundaries are inert. Cancellation uses the
existing Supervisor's disposable stdlib child, never an SDK/native/application.
These controls confer no qualification, setup completion or running application.
"""
import asyncio
from contextlib import contextmanager
from copy import deepcopy
from datetime import datetime
import json
from pathlib import Path
import sys
import threading
from types import SimpleNamespace
import zipfile

import pytest

from launchkit import compiler, deployment, deployment_ports, deployment_worker as worker
from launchkit import remote_product_destination_owner as owner
from launchkit import runtime_selection, sdk_wheel_integrity
from launchkit.artifact_binding import proposal_hash
from launchkit.blueprint import Blueprint
from launchkit.connect_installer_caller import CustodyError
from launchkit.forge_application_host import ForgeApplicationHost
from launchkit.product_runtime_custody import require_product_installation
from launchkit.projects import DraftConflict
from launchkit.templates import install_candidate as adapter
from launchkit.templates.storage_setup import project_lock
from test_forge_application_host import put
from test_meta_toolkit_initial_package import setup_spec
from test_meta_toolkit_runtime_selection import product, digest
from test_product_artifact_transfer import custody as artifact_custody
from test_product_destination_owner import destination_source, gcp_destination_source, package_bytes, stage
from test_selected_sdk_supervisor import assert_reaped, wait_until


@pytest.fixture
def installation(destination_source, monkeypatch, request):
    cloud = getattr(request, "param", None) == "gcp"
    selected = request.getfixturevalue("gcp_destination_source") if cloud else destination_source
    spec = setup_spec()
    spec["target"] = "gcp" if cloud else "linux"
    if cloud:
        disposition = selected.context["source_disposition"]
        spec["gcp"].update(project=disposition["project"], zone=disposition["zone"])
    spec = Blueprint.model_validate(spec).model_dump(mode="json")
    definition = compiler.mob_toml(spec).encode()
    inventory = compiler.application_inventory_input(spec)
    policy = {"provider_id": "application", "policy_id": "inert-original-policy", "revision": 1,
        "members": [{"member_identity": row["identity"], "grants": []}
            for row in json.loads(inventory)["roster"]]}
    configuration = {"fields": [
        {"key": "CONSOLE_USER_ID", "label": "Console user", "required": True, "secret": False},
        {"key": "CONSOLE_LOGIN_PASSWORD", "label": "Console password", "required": True, "secret": True}]}
    protected = {"application/tool-policy.json": owner._json(policy),
        "application/configuration.json": owner._json(configuration),
        "application/console.json": b'{"authentication":"local-password"}\n',
        "config/access.toml": b'[access]\nenabled = true\nadmins = ["fixture-operator"]\n'}
    report = json.loads(selected.files["implementation-report.json"])
    report["blueprint_hash"] = proposal_hash(spec)
    report["unresolved"] = ["Inert external delivery remains unverified."]
    deployed = compiler._runtime(Blueprint.model_validate(spec),
        native_version=selected.custody.plan.origin.meta.version)
    deployed["runtimeCandidate"] = runtime_selection.selected_runtime_marker(selected.custody.plan.origin)
    files = {**selected.files, **protected,
        "blueprint.json": owner._json(spec), "config/mob.toml": definition,
        "deployment.json": owner._json(deployed), "implementation-report.json": owner._json(report),
        "runtime_candidates.py": b"raise RuntimeError('inert accepted reader must never import')\n",
        "install_candidate.py": b"raise RuntimeError('inert accepted installer must never import')\n",
        "requirements.application.txt": b"# No additional application dependencies\n"}
    selected.package, selected.files = package_bytes(files)
    selected.context.update(package_sha256=digest(selected.package), package_size=len(selected.package))
    if cloud:
        disposition["instance"] = deployed["slug"]
        disposition["binding"].update(package_sha256=digest(selected.package),
            blueprint_hash=report["blueprint_hash"])
    root = selected.custody.home / "original selected host inputs"
    root.mkdir(mode=0o700)
    blueprint = put(root / "blueprint.json", owner._json(spec))
    definition_pin = put(root / "mob.toml", definition)
    inventory_pin = put(root / "application-inventory.json", inventory)
    pins = {name: put(root / name, value) for name, value in protected.items()}
    acquisition = put(root / "source-selection.json", {"blueprint": blueprint.pin(),
        "definition": definition_pin.pin(), "original_inventory": inventory_pin.pin(),
        "source_policy": pins["application/tool-policy.json"].pin()})
    viewer = put(root / "viewer-selection.json", {"base_source_selection": acquisition.pin(),
        "operator": {"subject": "fixture-operator", "private_configuration_key": "CONSOLE_USER_ID",
            "authenticated_evidence": None}, "source_inputs": {name: pin.pin()
                for name, pin in pins.items() if name != "application/tool-policy.json"}})
    staged = stage(selected)
    assert staged["status"] == "product-source-staged"
    state = SimpleNamespace(selected=selected, spec=spec, acquisition=acquisition,
        viewer=viewer, pins=pins, events=[], owner_calls=[], current=True,
        settings={"OPENAI_API_KEY": "inert-not-a-provider-key", "CONSOLE_USER_ID": "fixture-operator",
            "CONSOLE_LOGIN_PASSWORD": "inert-private-fixture-password"}, on_command=lambda phase: None,
        after_install=lambda supervisor: None)
    state.directory = (selected.custody.home / ".local/state/meta-forge/deployments"
        / selected.context["guide_id"] / selected.context["deployment_id"])
    state.python = selected.destination / ".venv/bin/python"
    state.site = selected.destination / ".venv/lib/python-fixture/site-packages"

    def locations(root, sdk_python=None, site_packages=None):
        assert Path(root) == selected.destination
        assert sdk_python is None or Path(sdk_python) == state.python
        assert site_packages is None or Path(site_packages) == state.site
        return state.python, state.site, selected.destination / ".venv/forge-sdk.receipt.json"

    monkeypatch.setattr(adapter, "_sdk_location", locations)
    monkeypatch.setattr(deployment, "require_available_console_port", lambda *args, **kwargs: None)
    monkeypatch.setattr(deployment_ports, "require_available_console_port", lambda *args, **kwargs: None)
    original_init = worker.Supervisor.__init__
    original_install = worker.Supervisor.install
    original_command = worker.Supervisor.command
    state.supervisors = []

    def initialize(supervisor, *args, **kwargs):
        original_init(supervisor, *args, **kwargs)
        state.supervisors.append(supervisor)

    async def install(supervisor, definition, *, product_inputs=None):
        assert type(supervisor) is worker.Supervisor
        assert supervisor._product_source_custody is state.source
        assert supervisor.directory == state.directory
        assert supervisor.destination == selected.destination
        assert asyncio.current_task() is state.parent and threading.get_ident() == state.thread
        state.source._lease.require_current()
        queued = deployment.read_receipt(state.directory)
        assert queued["status"] == "queued" and queued["phase"] == "install"
        assert queued["runtime_candidate"] == selected.origin
        assert "scope" not in queued and queued["deployment_id"] == selected.context["deployment_id"]
        assert queued["unresolved"] == report["unresolved"]
        assert all(datetime.fromisoformat(queued[name]).utcoffset() is not None
            for name in ("created_at", "updated_at"))
        assert product_inputs[1] is state.source.artifacts
        assert json.loads((selected.destination / deployment.OWNER_FILE).read_bytes()) == deployment.ownership(queued)
        state.events.append("queued-original-supervisor")
        result = await original_install(supervisor, definition, product_inputs=product_inputs)
        state.after_install(supervisor)
        return result

    async def command(supervisor, argv, phase, timeout, **options):
        assert supervisor is state.host._destination_installation_supervisor
        assert phase in {"Selected SDK environment", "Selected SDK site path", "Installed application contract"}
        state.events.append(phase)
        if phase == "Selected SDK environment":
            state.python.parent.mkdir(mode=0o700, parents=True)
            state.python.write_bytes(b"INERT interpreter must never execute\n")
            state.python.chmod(0o700)
            state.site.mkdir(mode=0o700, parents=True)
        state.on_command(phase)
        return json.dumps(str(state.site)) if phase == "Selected SDK site path" else ""

    async def pair(supervisor, meta_path, toolkit_path, **options):
        assert supervisor is state.host._destination_installation_supervisor
        assert (meta_path, toolkit_path) == (state.source.relocated.meta_wheel.path,
            state.source.relocated.toolkit_wheel.path)
        options["require_owner_current"]()
        options["require_current"]()
        facts = sdk_wheel_integrity.check_meta_toolkit_wheels(Path(meta_path).read_bytes(),
            Path(toolkit_path).read_bytes(), meta=options["meta"], toolkit=options["toolkit"])
        for path in (meta_path, toolkit_path):
            with zipfile.ZipFile(path) as archive:
                for name in archive.namelist():
                    target = state.site / name
                    target.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
                    target.write_bytes(archive.read(name))
                    target.chmod(0o600)
        actual = adapter.compare_installed_meta_toolkit_wheels(*facts,
            deployment_root=selected.destination, site_packages=state.site, sdk_python=state.python)
        state.events.append("exact-sdk-byte-comparison")
        options["require_current"]()
        options["require_owner_current"]()
        return actual

    monkeypatch.setattr(worker.Supervisor, "__init__", initialize)
    monkeypatch.setattr(worker.Supervisor, "install", install)
    monkeypatch.setattr(worker.Supervisor, "command", command)
    monkeypatch.setattr(worker.Supervisor, "install_selected_sdk_pair", pair)
    state.original_command = original_command
    state.commands = command
    return state


@contextmanager
def selected_host(state):
    selected = state.selected
    state.parent, state.loop = asyncio.current_task(), asyncio.get_running_loop()
    state.thread = threading.get_ident()

    def current():
        assert asyncio.get_running_loop() is state.loop
        assert asyncio.current_task() is state.parent
        assert threading.get_ident() == state.thread
        state.owner_calls.append(state.parent)
        if not state.current:
            raise CustodyError("Original destination installation owner changed.")

    def unavailable_inputs(*args):
        pytest.fail("Byte installation must not select or fabricate native setup inputs.")

    with project_lock(selected.destination) as lease:
        with owner.open_stopped_product_source(selected.context, project_lease=lease,
                require_current=current, owner_source=selected.source) as source:
            state.source = source
            state.host = ForgeApplicationHost(guide_id=selected.context["guide_id"],
                runtime_target=source.origin, acquisition_selection=state.acquisition,
                viewer_selection=state.viewer, require_current=current, source_custody=source,
                startup_selection=unavailable_inputs, preparation_inputs=unavailable_inputs,
                retain_preparation_inputs=unavailable_inputs)
            state.current_owner = current
            yield state


def source_bytes(state):
    return {name: (state.selected.destination / name).read_bytes() for name in state.selected.files}


async def test_true_destination_entry_publishes_original_receipt_and_only_stopped_byte_install(installation):
    with selected_host(installation) as f:
        before = source_bytes(f)
        with pytest.raises(CustodyError, match="existing installation"):
            require_product_installation(f.source, f.source.receipt)
        supervisor = await f.host.install_staged_source(settings=f.settings, require_current=f.current_owner)
        assert supervisor is f.host._destination_installation_supervisor
        assert f.supervisors == [supervisor]
        assert f.host._installation_supervisor is None
        assert f.host._retained_preparation_inputs is None
        receipt = deployment.read_receipt(f.directory)
        assert receipt["status"] == "stopped" and receipt["phase"] == "configure"
        assert all(receipt[key] == f.selected.context[key] for key in
            ("guide_id", "deployment_id", "build_id", "guide_version", "package_sha256"))
        assert receipt["destination"] == str(f.selected.destination)
        assert receipt["runtime_candidate"] == f.selected.origin
        assert receipt["unresolved"] == ["Inert external delivery remains unverified."]
        assert receipt.get("runtime_pid") is None and receipt.get("console_url") is None
        assert not receipt.get("process_live") and not receipt.get("bootstrap")
        assert receipt["installed_candidate_runtime_proof"]["source"] == "meta-toolkit-product"
        assert receipt["installed_candidate_runtime_proof"]["assembly_sha256"] == f.source.origin.assembly.sha256
        assert (f.directory / "package.zip").read_bytes() == f.selected.package
        assert source_bytes(f) == before
        values, _ = deployment.private_configuration_snapshot(f.selected.destination)
        assert all(values[key] == value for key, value in f.settings.items())
        assert (f.selected.destination / ".env").stat().st_mode & 0o777 == 0o600
        assert f.events == ["queued-original-supervisor", "Selected SDK environment",
            "Selected SDK site path", "exact-sdk-byte-comparison", "Installed application contract"]
        require_product_installation(f.source, receipt)
        f.source.require_current()
        assert not (f.directory / "application-bootstrap-entry.json").exists()
        assert not (f.directory / "application-preparation-plan.json").exists()
        assert supervisor.child is None and supervisor._selected_sdk_install_task is None
        assert len(f.owner_calls) >= 3
        with pytest.raises((CustodyError, DraftConflict, RuntimeError), match="already|original|attempt|retain"):
            await f.host.install_staged_source(settings=f.settings, require_current=f.current_owner)
        assert f.host._destination_installation_supervisor is supervisor


@pytest.mark.parametrize("change", ["subject", "unknown-setting", "blueprint", "definition", "policy",
    "configuration", "viewer", "access", "guide", "runtime", "owner", "lease"])
async def test_destination_admission_refuses_substitution_before_private_or_receipt_publication(installation, change):
    with selected_host(installation) as f:
        submitted = deepcopy(f.settings)
        if change == "subject": submitted["CONSOLE_USER_ID"] = "foreign-operator"
        elif change == "unknown-setting": submitted["UNDECLARED_PRIVATE_KEY"] = "inert-value"
        elif change in {"blueprint", "definition", "policy", "configuration", "viewer", "access"}:
            name = {"blueprint": "blueprint.json", "definition": "config/mob.toml",
                "policy": "application/tool-policy.json", "configuration": "application/configuration.json",
                "viewer": "application/console.json",
                "access": "config/access.toml"}[change]
            path = f.selected.destination / name
            path.write_bytes(path.read_bytes() + b"\n")
        elif change == "guide": f.host.guide_id = "33333333-3333-4333-8333-333333333333"
        elif change == "runtime": f.host._target = f.source.relocated
        elif change == "owner": f.current = False
        else: f.source._lease._closed = True
        with pytest.raises((CustodyError, DraftConflict, ValueError, RuntimeError)):
            await f.host.install_staged_source(settings=submitted, require_current=f.current_owner)
        assert not f.directory.exists()
        assert not (f.selected.destination / deployment.OWNER_FILE).exists()
        assert not (f.selected.destination / ".env").exists()
        assert not (f.selected.destination / ".venv").exists()
        assert f.events == [] and getattr(f.host, "_destination_installation_supervisor", None) is None
        # The caller owns release of the actual original lease even on refusal.
        if change == "lease": f.source._lease._closed = False
        if change == "owner": f.current = True


@pytest.mark.parametrize("change", ["source", "settings", "owner", "command-error"])
async def test_entered_destination_attempt_preserves_same_installer_and_never_retries_changed_owner(installation, change):
    with selected_host(installation) as f:
        def changed(phase):
            if phase != "Selected SDK environment": return
            if change == "source":
                path = f.selected.destination / "app.py"
                path.write_bytes(path.read_bytes() + b"# changed accepted source\n")
            elif change == "settings":
                (f.selected.destination / ".env").write_text("MOBKIT_PORT=19991\n")
                (f.selected.destination / ".env").chmod(0o600)
            elif change == "owner": f.current = False
            else: raise OSError("Original byte installer could not finish its command.")
        f.on_command = changed
        with pytest.raises((CustodyError, DraftConflict, ValueError, RuntimeError, OSError)):
            await f.host.install_staged_source(settings=f.settings, require_current=f.current_owner)
        supervisor = f.host._destination_installation_supervisor
        assert type(supervisor) is worker.Supervisor
        assert f.supervisors == [supervisor]
        assert supervisor._product_source_custody is f.source
        assert f.host._installation_supervisor is None
        assert f.events == ["queued-original-supervisor", "Selected SDK environment"]
        assert (f.directory / "package.zip").read_bytes() == f.selected.package
        retained = (f.directory / "receipt.json").read_bytes()
        assert deployment.read_receipt(f.directory)["status"] != "stopped" or deployment.read_receipt(f.directory)["phase"] != "configure"
        f.current = True
        with pytest.raises((CustodyError, DraftConflict, ValueError, RuntimeError)):
            await f.host.install_staged_source(settings=f.settings, require_current=f.current_owner)
        assert f.host._destination_installation_supervisor is supervisor
        assert (f.directory / "receipt.json").read_bytes() == retained
        assert f.events == ["queued-original-supervisor", "Selected SDK environment"]


@pytest.mark.parametrize("failure", ["application-check", "completion-proof"])
async def test_final_application_check_and_completion_proof_are_required_before_stopped_configure(installation, failure):
    with selected_host(installation) as f:
        def command(phase):
            if phase == "Installed application contract" and failure == "application-check":
                raise RuntimeError("Original application-check boundary refused the installed source.")

        def completed(supervisor):
            assert supervisor is f.host._destination_installation_supervisor
            if failure == "completion-proof":
                receipt = deployment.read_receipt(f.directory)
                assert receipt["installed_candidate_runtime_proof"]["source"] == "meta-toolkit-product"
                deployment.update_receipt(f.directory, installed_candidate_runtime_proof=None)

        f.on_command, f.after_install = command, completed
        with pytest.raises((CustodyError, RuntimeError), match="application-check|completion"):
            await f.host.install_staged_source(settings=f.settings, require_current=f.current_owner)
        supervisor = f.host._destination_installation_supervisor
        assert f.supervisors == [supervisor] and supervisor.stop.is_set()
        assert f.events == ["queued-original-supervisor", "Selected SDK environment",
            "Selected SDK site path", "exact-sdk-byte-comparison", "Installed application contract"]
        receipt = deployment.read_receipt(f.directory)
        assert receipt["status"] != "stopped" or receipt["phase"] != "configure"
        assert not receipt.get("installed_candidate_runtime_proof")
        assert receipt.get("runtime_pid") is None and receipt.get("console_url") is None
        retained = (f.directory / "receipt.json").read_bytes()
        with pytest.raises(CustodyError, match="already|original|attempt|retain"):
            await f.host.install_staged_source(settings=f.settings, require_current=f.current_owner)
        assert f.host._destination_installation_supervisor is supervisor
        assert (f.directory / "receipt.json").read_bytes() == retained


async def test_partial_private_publication_retains_original_identity_without_admitting_another_attempt(installation, monkeypatch):
    original = deployment.write_private_settings
    refusal = OSError("Original private publication stopped before byte installation.")

    def publish(destination, values):
        original(destination, values)
        raise refusal

    monkeypatch.setattr(deployment, "write_private_settings", publish)
    with selected_host(installation) as f:
        with pytest.raises(OSError) as caught:
            await f.host.install_staged_source(settings=f.settings, require_current=f.current_owner)
        assert caught.value is refusal and f.host._destination_installation_entered
        assert f.host._destination_installation_supervisor is None and f.supervisors == []
        assert f.events == []
        receipt = deployment.read_receipt(f.directory)
        assert receipt["deployment_id"] == f.selected.context["deployment_id"]
        assert receipt["package_sha256"] == f.selected.context["package_sha256"]
        assert receipt["status"] != "stopped" or receipt["phase"] != "configure"
        assert (f.directory / "package.zip").read_bytes() == f.selected.package
        retained = {path: path.read_bytes() for path in (f.directory / "receipt.json",
            f.selected.destination / deployment.OWNER_FILE, f.selected.destination / ".env")}
        with pytest.raises(CustodyError, match="already|original|attempt|retain"):
            await f.host.install_staged_source(settings=f.settings, require_current=f.current_owner)
        assert all(path.read_bytes() == content for path, content in retained.items())
        assert f.supervisors == [] and f.events == []


async def test_failed_application_child_cleanup_retains_exact_drains_and_same_owner_continuation(installation, monkeypatch):
    f = installation
    original_output = worker.Supervisor.output
    original_cleanup = worker.Supervisor.cleanup_child
    output_entered, output_release = asyncio.Event(), asyncio.Event()
    recovery_entered = asyncio.Event()
    children, cleanup_calls = [], []
    refusal = OSError("Original application-check cleanup could not settle its drain.")

    async def output(supervisor, pipe, phase, captured=None):
        await original_output(supervisor, pipe, phase, captured)
        if phase == "Destination inert application-check child":
            output_entered.set()
            await output_release.wait()

    async def cleanup(supervisor, process, tasks=(), **options):
        assert supervisor is f.host._destination_installation_supervisor
        cleanup_calls.append((process, tuple(tasks)))
        if len(cleanup_calls) == 1:
            assert len(tasks) == 3 and any(not task.done() for task in tasks)
            raise refusal
        assert process is cleanup_calls[0][0] and tuple(tasks) == cleanup_calls[0][1]
        assert supervisor.stop.is_set()
        recovery_entered.set()
        return await original_cleanup(supervisor, process, tasks, **options)

    async def command(supervisor, argv, phase, timeout, **options):
        if phase != "Installed application contract":
            return await f.commands(supervisor, argv, phase, timeout, **options)
        f.events.append(phase)
        original_spawn = supervisor.spawn_child

        async def spawn(*args, **options):
            assert args == (sys.executable, "-I", "-c", "print('inert application-check child')")
            result = await original_spawn(*args, **options)
            children.append(result[0])
            return result

        supervisor.spawn_child = spawn
        return await f.original_command(supervisor,
            [sys.executable, "-I", "-c", "print('inert application-check child')"],
            "Destination inert application-check child", 5)

    monkeypatch.setattr(worker.Supervisor, "output", output)
    monkeypatch.setattr(worker.Supervisor, "cleanup_child", cleanup)
    monkeypatch.setattr(worker.Supervisor, "command", command)
    with selected_host(f):
        continuation = None
        try:
            with pytest.raises(OSError) as caught:
                await f.host.install_staged_source(settings=f.settings, require_current=f.current_owner)
            assert caught.value is refusal
            await asyncio.wait_for(output_entered.wait(), 5)
            supervisor = f.host._destination_installation_supervisor
            retained = supervisor._destination_install_cleanup
            first_task, drains, child = retained.cleanup_task, retained.drain_tasks, retained.child
            assert f.supervisors == [supervisor] and children == [child]
            assert supervisor.child is child and child is cleanup_calls[0][0]
            assert len(drains) == 3 and drains == cleanup_calls[0][1]
            assert first_task.done() and first_task.exception() is refusal
            assert any(not task.done() for task in drains)
            assert supervisor.stop.is_set() and not f.source._closed
            f.source._lease.require_current()
            receipt = (f.directory / "receipt.json").read_bytes()
            with pytest.raises(CustodyError, match="already|original|attempt|retain"):
                await f.host.install_staged_source(settings=f.settings, require_current=f.current_owner)
            assert retained.cleanup_task is first_task and retained.drain_tasks is drains
            continuation = asyncio.create_task(f.host.cleanup_staged_installation(supervisor=supervisor))
            await asyncio.wait_for(recovery_entered.wait(), 5)
            pending = retained.cleanup_task
            assert pending is not first_task and not pending.done()
            assert retained.child is child and retained.drain_tasks is drains
            continuation.cancel()
            await asyncio.sleep(0)
            continuation.cancel()
            await asyncio.sleep(0)
            assert not continuation.done() and retained.cleanup_task is pending
            assert supervisor.child is child and not f.source._closed
            assert (f.directory / "receipt.json").read_bytes() == receipt
            output_release.set()
            with pytest.raises(asyncio.CancelledError):
                await asyncio.wait_for(continuation, 5)
            assert pending.done() and pending.exception() is None
            assert supervisor.child is None and all(task.done() for task in drains)
            await f.host.cleanup_staged_installation(supervisor=supervisor)
            assert retained.cleanup_task is pending and retained.drain_tasks is drains
            assert len(cleanup_calls) == 2 and len(children) == 1
            assert_reaped(child)
            assert f.host._installation_supervisor is None
            assert (f.directory / "receipt.json").read_bytes() == receipt
            assert not deployment.read_receipt(f.directory).get("installed_candidate_runtime_proof")
            assert f.events == ["queued-original-supervisor", "Selected SDK environment",
                "Selected SDK site path", "exact-sdk-byte-comparison", "Installed application contract"]
        finally:
            output_release.set()
            if continuation is not None and not continuation.done():
                continuation.cancel()
                await asyncio.gather(continuation, return_exceptions=True)
            supervisor = f.host._destination_installation_supervisor
            if supervisor is not None and supervisor.child is not None:
                await f.host.cleanup_staged_installation(supervisor=supervisor)


async def test_destination_cancel_stops_and_drains_original_supervisor_before_source_lease_release(installation, monkeypatch):
    f = installation
    original_terminate = worker.Supervisor.terminate
    cleanup_entered, cleanup_release = asyncio.Event(), asyncio.Event()
    children = []
    entered = asyncio.Event()

    async def command(supervisor, argv, phase, timeout, **options):
        assert phase == "Selected SDK environment"
        assert supervisor is f.host._destination_installation_supervisor
        assert supervisor._product_source_custody is f.source
        f.events.append(phase)
        original_spawn = supervisor.spawn_child

        async def spawn(*args, **kwargs):
            # Exactly one original supervised stdlib child, never the selected
            # interpreter, pip, application source or a native product program.
            assert args == (sys.executable, "-I", "-c", "import time;time.sleep(30)")
            child = await original_spawn(*args, **kwargs)
            children.append(child[0])
            entered.set()
            return child

        supervisor.spawn_child = spawn
        return await f.original_command(supervisor,
            [sys.executable, "-I", "-c", "import time;time.sleep(30)"],
            "Destination inert installer child", 5)

    async def held_terminate(supervisor, timeout=None):
        assert supervisor is f.host._destination_installation_supervisor
        assert supervisor.child is children[0]
        assert supervisor.stop.is_set(), "Stop must be visible before the original child drain."
        cleanup_entered.set()
        await cleanup_release.wait()
        return await original_terminate(supervisor, timeout=timeout)

    monkeypatch.setattr(worker.Supervisor, "command", command)
    monkeypatch.setattr(worker.Supervisor, "terminate", held_terminate)

    async def install():
        with selected_host(f):
            try:
                return await f.host.install_staged_source(settings=f.settings, require_current=f.current_owner)
            except BaseException:
                original = f.host._destination_installation_supervisor
                retained = (f.directory / "receipt.json").read_bytes()
                with pytest.raises((CustodyError, DraftConflict, RuntimeError)):
                    await f.host.install_staged_source(settings=f.settings, require_current=f.current_owner)
                assert f.host._destination_installation_supervisor is original
                assert (f.directory / "receipt.json").read_bytes() == retained
                raise

    task = asyncio.create_task(install())
    try:
        await asyncio.wait_for(entered.wait(), 5)
        await wait_until(lambda: bool(children))
        supervisor = f.host._destination_installation_supervisor
        assert f.supervisors == [supervisor]
        child = children[0]
        task.cancel()
        await asyncio.wait_for(cleanup_entered.wait(), 5)
        assert supervisor.child is child and child.returncode is None
        assert supervisor.stop.is_set() and not task.done()
        assert not f.source._closed
        f.source._lease.require_current()
        task.cancel()
        await asyncio.sleep(0)
        assert not task.done() and supervisor.child is child
        assert f.events == ["queued-original-supervisor", "Selected SDK environment"]
        assert (f.directory / "package.zip").read_bytes() == f.selected.package
        cleanup_release.set()
        with pytest.raises(asyncio.CancelledError):
            await asyncio.wait_for(task, 5)
        assert supervisor.child is None and supervisor._selected_sdk_install_task is None
        assert f.host._destination_installation_supervisor is supervisor
        assert f.host._installation_supervisor is None
        assert len(children) == 1
        assert_reaped(child)
        receipt = deployment.read_receipt(f.directory)
        assert receipt["status"] != "stopped" or receipt["phase"] != "configure"
        assert receipt.get("runtime_pid") is None and receipt.get("console_url") is None
        assert (f.selected.destination / ".env").exists()
        assert f.source._closed
    finally:
        cleanup_release.set()
        if not task.done():
            task.cancel()
            await asyncio.gather(task, return_exceptions=True)
