"""Real registry Build/Install owner joins with inert external boundaries.

The actual GuideStore, services, reserved workspace, package and completion
readers are used. Builder execution, signed acceptance results and worker exit
are explicit synthetic fixtures, not native/application acceptance or useful
work. No SDK, provider, gateway, service or network executes here.
"""
import asyncio
import json
import os
from pathlib import Path
from types import SimpleNamespace
from uuid import uuid4

import pytest

from launchkit import deployment, implementation as builds, local_application_source
from launchkit.acceptance import SemanticAcceptance, _json, _sha
from launchkit.blueprint import starter_blueprint
from launchkit.connect_installer_caller import CustodyError
from launchkit.guide import GuideService
from launchkit.guide_store import GuideStore
from launchkit.implementation import ImplementationService
from launchkit.implementation_workspace import ImplementationWorkspace
from launchkit.local_application_registry import LocalApplicationRegistry
from launchkit.projects import DraftConflict
from launchkit.runtime_selection import selected_runtime_artifacts
from launchkit.templates import install_candidate as adapter
from test_implementation import decision_brief, host_receipt
from test_local_application_factory import source_bytes
from test_meta_toolkit_initial_install import AdmissionOnly, no_processes
from test_meta_toolkit_initial_package import setup_spec
from test_meta_toolkit_runtime_selection import product, no_product_execution
from test_meta_toolkit_runtime_update import catalog_target


MANAGED_MODELS_WITHOUT_CUSTODY = ("This plan's agents use host-managed models, and this Forge was started "
    "without an application model configuration for them")


def shipping_callbacks():
    """The deployment callbacks that server.select_local_application_runtime binds.

    Read from that function's own source: every product launcher and the
    packaged entry select a registry through it, so registry rows use the
    same callbacks, including the installed replacement selection.
    """
    import ast
    source = Path(__file__).resolve().parents[1] / "launchkit/server.py"
    function = next(node for node in ast.parse(source.read_text()).body
        if isinstance(node, ast.FunctionDef) and node.name == "select_local_application_runtime")
    assignment = next(node for node in ast.walk(function) if isinstance(node, ast.Assign)
        and any(isinstance(target, ast.Name) and target.id == "callbacks" for target in node.targets))
    return ast.literal_eval(assignment.value)


def bind_registry(service, owner):
    """Bind a registry's callbacks to its deployment service, as the launcher does."""
    for name, method in shipping_callbacks():
        setattr(service, name, getattr(owner, method))


@pytest.fixture
async def registry(tmp_path, product, monkeypatch):
    target = catalog_target(product)
    # The inert product and starter proposal both select Apple policy. Keep
    # that declared policy separate from the runner's real filesystem/kernel.
    from launchkit import runtime_dependencies
    from launchkit.runtime_selection import selected_runtime_platform
    selected_platform = selected_runtime_platform(target)
    monkeypatch.setattr(runtime_dependencies, "platform_target", lambda: selected_platform)
    monkeypatch.setattr(deployment, "_local_host_target", lambda: "mac")
    store = GuideStore(tmp_path / "original-guide-store")
    monkeypatch.setattr("launchkit.planner.planner_capability", lambda: {"available": True})
    monkeypatch.setattr("launchkit.guide.planner_capability", lambda: {"available": True})
    monkeypatch.setattr("launchkit.implementation_workspace._sandbox_executable", lambda: "/inert/sandbox")
    guide = GuideService(store, architect=None)
    implementation = ImplementationService(tmp_path / "original-forge-owner", guide)
    service = AdmissionOnly(tmp_path / "original-forge-owner", guide, implementation)
    service.launched = []
    owner = LocalApplicationRegistry(guide_store=store, implementation=implementation,
        deployment=service, runtime_target=target)
    # Bind the actual trusted callbacks, as the launcher's runtime selection
    # does. No proxy service or file-selected host enters this fixture.
    implementation.prepare_workspace = owner.prepare_workspace
    bind_registry(service, owner)

    async def inert_builder(directory):
        await asyncio.Event().wait()

    monkeypatch.setattr(implementation, "_run", inert_builder)
    monkeypatch.setattr(deployment, "require_available_console_port", lambda *args, **kwargs: None)
    monkeypatch.setattr(deployment, "observe_local_console_port", lambda *args, **kwargs: None)
    f = SimpleNamespace(store=store, guide=guide, implementation=implementation,
        service=service, registry=owner, target=target, root=tmp_path)
    yield f
    await implementation.shutdown()
    assert not implementation.processes and not service.processes and not service.tasks


def new_guide(f, name="First local application", *, ordinary=False):
    spec = starter_blueprint().model_dump(mode="json") if ordinary else setup_spec()
    spec["project"]["name"] = name
    brief = decision_brief()
    brief["requirements"] = [{"id": "requested-outcome", "outcome": "Review the requested local job.",
        "disposition": "supported", "source_message_ids": ["request"]}]
    return f.store.create({"proposal": spec, "status": "accepted", "stage": "review", "brief": brief,
        "messages": [{"id": "request", "role": "user", "text": "Review the requested local job."}]})


async def build(f, saved, request="", *, ordinary=False, previous=None):
    options = {} if ordinary else f.registry.initial_build_options(saved["id"], saved["version"])
    if previous is not None:
        options["expected_selected_build_id"] = previous["build_id"]
    result = await f.implementation.submit(saved["id"], saved["version"], request, **options)
    directory = f.implementation._directory(saved["id"], result["build_id"])
    assert directory / "workspace" == workspace(f.implementation, result).root
    assert builds.read_receipt(directory)["build_id"] == result["build_id"]
    return result


def workspace(implementation, result):
    directory = implementation._directory(result["guide_id"], result["build_id"])
    frozen = json.loads((directory / "input.json").read_bytes())
    return ImplementationWorkspace(directory / "workspace", frozen["blueprint"])


async def accept_source_fixture(f, result):
    """Retain the existing synthetic acceptance pattern without executing it."""
    task = f.implementation.tasks.pop(result["build_id"])
    task.cancel()
    await asyncio.gather(task, return_exceptions=True)
    project = workspace(f.implementation, result)
    project.write_file("application/inert.txt", "Synthetic source admission fixture; no application ran.\n")
    os.chmod(project.project / "application/inert.txt", 0o600)
    check = host_receipt(project)
    directory = f.implementation._directory(result["guide_id"], result["build_id"])
    builds.update_receipt(directory, status="completed", receipt=check, checks=check["checks"],
        cleanup_pending=False, cleanup_in_progress=False, worker_stop_confirmed=True)
    frozen = json.loads((directory / "input.json").read_bytes())
    acceptance = SemanticAcceptance(directory / "acceptance", project, frozen)
    acceptance.begin_round()
    run = "synthetic-registry-admission"
    acceptance.bind_run(run)
    acceptance.write_test("test_inert.py", "# Synthetic signed admission specimen; never executed.\n")
    with acceptance._locked():
        state = acceptance._state()
        row = acceptance._round(state)
        execution = {"id": "synthetic-host-result", "status": "pass", "tests_run": 1,
            "source_digest": row["source_digest"], "test_digest": _sha(_json(acceptance._tests(row))),
            "test_results": [{"id": "inert-host-result", "status": "pass"}],
            "detail": "Synthetic fixture only; no application, SDK or native runtime executed."}
        execution["signature"] = acceptance._signed(execution)
        row["execution"] = execution
        acceptance._save(state)
    acceptance.publish({"summary": "Synthetic source/owner admission fixture, not runtime acceptance.",
        "requirements": [{"id": item["id"], "status": "tested", "source_paths": ["application/inert.txt"],
            "test_ids": ["inert-host-result"], "evidence": "Synthetic signed host fixture only.",
            "remaining_dependencies": []} for item in acceptance.requirements],
        "counterexamples": [], "remaining_dependencies": ["All real behavior remains untested."]})
    acceptance.complete_run(run, {"kind": "interaction_complete", "interaction_id": run, "cursor": "fixture:1"})
    current = await f.implementation.status(result["guide_id"])
    assert current["build_id"] == result["build_id"] and current["ready_to_package"]
    return project


async def install_fixture(f, saved, result, monkeypatch, *, ordinary=False):
    """Reach original queued Install and successful completion retention once."""
    settings = {"OPENAI_API_KEY": "inert-not-a-provider-key"}
    if not ordinary:
        settings.update(CONSOLE_USER_ID="forge-" + saved["id"],
            CONSOLE_LOGIN_PASSWORD="inert-private-fixture-password")
    f.registry.validate_settings(saved["id"], settings, build_id=result["build_id"])
    receipt = await f.service.submit(saved["id"], saved["version"], result["build_id"],
        str(f.root / ("installed-" + saved["id"])), settings=settings)
    directory = f.service._directory(saved["id"], receipt["deployment_id"])
    destination = Path(receipt["destination"])
    deployment.materialize(directory)
    if not ordinary:
        adapter.install_meta_toolkit_prefix(selected_runtime_artifacts(f.target),
            deployment_root=destination, require_current=lambda: None)
    process = SimpleNamespace(pid=981237, returncode=None, stdin=None)

    async def wait():
        current = deployment.read_receipt(directory)
        cleanup = {name: current.get(name) for name in ("guide_id", "deployment_id", "build_id",
            "guide_version", "blueprint_hash", "destination", "package_sha256")}
        cleanup.update(deployment.native_fields(current))
        cleanup.update(deployment.interactive_source_binding(directory, current))
        cleanup.update(confirmed=True, worker_exit_confirmed=False,
            lifecycle_id=current["interactive_lifecycle_id"], startup_attempt_id=None,
            supervisor_pid=process.pid, return_code=0, boundary="owned-process-groups-absent")
        deployment.update_receipt(directory, status="stopped", phase="configure", runtime_pid=None,
            process_live=False, interactive_cleanup=cleanup,
            **({"installed_candidate_runtime_proof": {"fixture": "inert-worker-completion-only"}} if not ordinary else {}))
        process.returncode = 0
        return 0

    process.wait = wait

    async def spawn(*args, **kwargs):
        return process

    def absent(pid, sig):
        assert (pid, sig) == (process.pid, 0)
        raise ProcessLookupError

    monkeypatch.setattr(asyncio, "create_subprocess_exec", spawn)
    monkeypatch.setattr(deployment.os, "kill", absent)
    deployment.DeploymentService._launch(f.service, directory, "install")
    task = f.service.tasks[directory.name]
    await asyncio.wait_for(task, 5)
    finished = deployment.read_receipt(directory)
    assert finished["status"] == "stopped" and finished["phase"] == "configure"
    assert deployment.interactive_cleanup_matches(finished)
    assert not f.service.tasks and not f.service.processes
    return directory, destination


async def test_actual_reserved_builds_keep_distinct_per_guide_owners(registry):
    f = registry
    first, other = new_guide(f), new_guide(f, "Another local application")
    assert not f.implementation.root.exists()
    for saved in (first, other):
        assert f.registry.initial_build_options(saved["id"], saved["version"]) == {"runtime_target": f.target}
    assert not f.implementation.root.exists()
    one, two = await build(f, first), await build(f, other)
    a, b = f.registry.host_for_build(first["id"], one["build_id"]), f.registry.host_for_build(other["id"], two["build_id"])
    assert a is not b and a.guide_id == first["id"] and b.guide_id == other["id"]
    assert a.console_operator_declaration()["id"] == "forge-" + first["id"]
    assert b.console_operator_declaration()["id"] == "forge-" + other["id"]
    for saved, result, host in ((first, one, a), (other, two, b)):
        directory = f.implementation._directory(saved["id"], result["build_id"])
        assert directory.joinpath("local-application-source/blueprint.json").is_file()
        project = workspace(f.implementation, result)
        for name, pin in host._protected:
            assert project.read_file(name).encode() == pin.read()
        assert not host._installation_supervisor and not host._retained_preparation_inputs
    with pytest.raises(CustodyError, match="no retained local source owner"):
        f.registry.host_for_build(first["id"], two["build_id"])
    before = source_bytes(f.implementation._directory(first["id"], one["build_id"]) / "local-application-source")
    repeated = await build(f, first)
    assert repeated["build_id"] == one["build_id"]
    assert f.registry.host_for_build(first["id"], repeated["build_id"]) is a
    assert before == source_bytes(f.implementation._directory(first["id"], one["build_id"]) / "local-application-source")


async def test_same_source_new_build_has_new_host_and_exact_original_pins(registry):
    f = registry
    saved = new_guide(f)
    original = await build(f, saved)
    await accept_source_fixture(f, original)
    first = f.registry.host_for_build(saved["id"], original["build_id"])
    original_source = source_bytes(f.implementation._directory(saved["id"], original["build_id"]) / "local-application-source")
    newer = await build(f, saved, "Improve the requested review output", previous=original)
    second = f.registry.host_for_build(saved["id"], newer["build_id"])
    assert newer["build_id"] != original["build_id"] and second is not first
    assert second._protected == first._protected
    assert second._pins == first._pins
    assert second._startup_selection != first._startup_selection
    assert not first._installation_supervisor and not second._installation_supervisor
    assert not (f.implementation._directory(saved["id"], newer["build_id"]) / "local-application-source").exists()
    assert source_bytes(f.implementation._directory(saved["id"], original["build_id"]) / "local-application-source") == original_source


async def test_original_install_keeps_old_owner_after_changed_plan_and_new_build(registry, monkeypatch):
    f = registry
    saved = new_guide(f)
    first = await build(f, saved)
    await accept_source_fixture(f, first)
    old = f.registry.host_for_build(saved["id"], first["build_id"])
    directory, destination = await install_fixture(f, saved, first, monkeypatch)
    assert f.registry.selected_host(saved["id"], directory.name) is old
    inputs = old._retained_preparation_inputs
    assert inputs is not None and inputs.supervisor is old._installation_supervisor
    before = source_bytes(destination / ".mobkit/forge-local-setup"), (directory / "package.zip").read_bytes(), (destination / ".env").read_bytes()
    updated = f.store.get(saved["id"])
    updated["proposal"]["project"]["name"] += " changed application"
    updated = f.store.save(updated, updated["version"])
    second = await build(f, updated, "Implement the revised application", previous=first)
    await accept_source_fixture(f, second)
    new = f.registry.host_for_build(saved["id"], second["build_id"])
    assert new is not old and new._blueprint != old._blueprint
    assert f.registry.host_for_build(saved["id"], first["build_id"]) is old
    assert f.registry.selected_host(saved["id"], directory.name) is old
    assert old._retained_preparation_inputs is inputs and new._retained_preparation_inputs is None
    status = await f.registry.setup_status(f.service, saved["id"], directory.name)
    assert status["available"] and status["action"] == "prepare-setup"
    assert status["build_id"] == first["build_id"] and status["guide_version"] == saved["version"]
    assert before == (source_bytes(destination / ".mobkit/forge-local-setup"), (directory / "package.zip").read_bytes(), (destination / ".env").read_bytes())
    assert not inputs.inventory_attempt_path.exists() and not inputs.startup_selection_path.exists()


@pytest.mark.parametrize("changed", ["package", "build", "policy"])
async def test_selected_installed_owner_refuses_substitution_after_valid_baseline(registry, monkeypatch, changed):
    f = registry
    saved = new_guide(f)
    result = await build(f, saved)
    await accept_source_fixture(f, result)
    host = f.registry.host_for_build(saved["id"], result["build_id"])
    directory, destination = await install_fixture(f, saved, result, monkeypatch)
    assert f.registry.selected_host(saved["id"], directory.name) is host
    assert host._retained_preparation_inputs is not None
    if changed == "package":
        path = directory / "package.zip"
        path.write_bytes(path.read_bytes() + b" ")
        expected, refusal = "saved update package changed", DraftConflict
    elif changed == "build":
        deployment.update_receipt(directory, build_id=str(uuid4()))
        expected, refusal = "retained original host", CustodyError
    else:
        pin = dict(host._protected)["application/tool-policy.json"]
        pin.path.write_bytes(pin.read() + b" ")
        expected, refusal = "selected bytes", CustodyError
    altered = (directory / "receipt.json").read_bytes(), (directory / "package.zip").read_bytes(), (destination / ".env").read_bytes()
    with pytest.raises(refusal, match=expected):
        f.registry.selected_host(saved["id"], directory.name)
    assert altered == ((directory / "receipt.json").read_bytes(), (directory / "package.zip").read_bytes(), (destination / ".env").read_bytes())


async def test_saved_product_source_cannot_reconstruct_lost_registry_owner(registry, monkeypatch):
    f = registry
    saved = new_guide(f)
    result = await build(f, saved)
    await accept_source_fixture(f, result)
    directory, destination = await install_fixture(f, saved, result, monkeypatch)
    assert f.registry.selected_host(saved["id"], directory.name) is f.registry.host_for_build(saved["id"], result["build_id"])
    before = source_bytes(f.implementation._directory(saved["id"], result["build_id"]) / "local-application-source"), source_bytes(destination / ".mobkit/forge-local-setup")
    lost = LocalApplicationRegistry(guide_store=f.store, implementation=f.implementation,
        deployment=f.service, runtime_target=f.target)
    with pytest.raises(CustodyError, match="no retained local source owner"):
        lost.host_for_build(saved["id"], result["build_id"])
    with pytest.raises(CustodyError, match="retained original host"):
        lost.selected_host(saved["id"], directory.name)
    assert before == (source_bytes(f.implementation._directory(saved["id"], result["build_id"]) / "local-application-source"), source_bytes(destination / ".mobkit/forge-local-setup"))


async def test_ordinary_build_and_installed_readback_keep_no_product_owner(registry, monkeypatch):
    f = registry
    saved = new_guide(f, ordinary=True)
    result = await build(f, saved, ordinary=True)
    await accept_source_fixture(f, result)
    assert f.registry.host_for_build(saved["id"], result["build_id"]) is None
    directory, _ = await install_fixture(f, saved, result, monkeypatch, ordinary=True)
    assert f.registry.selected_host(saved["id"], directory.name) is None
    source = f.implementation._directory(saved["id"], result["build_id"]) / "local-application-source"
    assert not source.exists()
    lost = LocalApplicationRegistry(guide_store=f.store, implementation=f.implementation,
        deployment=f.service, runtime_target=f.target)
    before = (directory / "receipt.json").read_bytes(), (directory / "package.zip").read_bytes()
    lost.validate_settings(saved["id"], {"OPENAI_API_KEY": "inert-not-a-provider-key"}, build_id=result["build_id"])
    assert lost.selected_host(saved["id"], directory.name) is None
    assert before == ((directory / "receipt.json").read_bytes(), (directory / "package.zip").read_bytes())
    assert not source.exists()


async def test_interrupted_source_publication_keeps_refused_bytes_without_selectable_owner(registry, monkeypatch):
    f = registry
    saved = new_guide(f)
    original_write = local_application_source._write_bytes
    writes = []

    def change_guide_after_write(path, content):
        pin = original_write(path, content)
        writes.append(pin)
        state = f.store.get(saved["id"])
        f.store.save(state, state["version"])
        return pin

    monkeypatch.setattr(local_application_source, "_write_bytes", change_guide_after_write)
    with pytest.raises(CustodyError, match="exact current completed version"):
        await build(f, saved)
    assert len(writes) == 1 and writes[0].path.name == "blueprint.json"
    original_directory = writes[0].path.parent.parent
    assert original_directory.parent == f.implementation.root / saved["id"]
    directory = f.implementation.root.parent / "unadmitted-builds" / saved["id"] / original_directory.name
    assert directory.is_dir() and not original_directory.exists()
    source = directory / "local-application-source"
    frozen = source_bytes(source)
    assert set(frozen) == {"blueprint.json"} and not f.implementation.tasks and not f.service.launched
    for _ in range(2):
        with pytest.raises(CustodyError, match="no retained local source owner"):
            f.registry.host_for_build(saved["id"], directory.name)
    assert len(writes) == 1 and source_bytes(source) == frozen


async def test_deliberate_install_recovers_accepted_uninstalled_build_after_restart(registry):
    f = registry
    saved = new_guide(f)
    result = await build(f, saved)
    await accept_source_fixture(f, result)
    directory = f.implementation._directory(saved["id"], result["build_id"])
    before = source_bytes(directory)
    lost = LocalApplicationRegistry(guide_store=f.store, implementation=f.implementation,
        deployment=f.service, runtime_target=f.target)
    with pytest.raises(CustodyError, match="no retained local source owner"):
        lost.host_for_build(saved["id"], result["build_id"])
    lost.validate_settings(saved["id"], {"CONSOLE_USER_ID": "forge-" + saved["id"]},
        build_id=result["build_id"])
    recovered = lost.host_for_build(saved["id"], result["build_id"])
    assert recovered is not f.registry.host_for_build(saved["id"], result["build_id"])
    assert recovered._target is f.target
    assert recovered._blueprint == saved["proposal"]
    assert before == source_bytes(directory)
    assert not lost._installations and not f.service.launched


async def test_install_recovery_follows_signed_continuation_and_refuses_old_selection(registry):
    f = registry
    saved = new_guide(f)
    original = await build(f, saved)
    await accept_source_fixture(f, original)
    newer = await build(f, saved, "Improve the review output", previous=original)
    await accept_source_fixture(f, newer)
    lost = LocalApplicationRegistry(guide_store=f.store, implementation=f.implementation,
        deployment=f.service, runtime_target=f.target)
    settings = {"CONSOLE_USER_ID": "forge-" + saved["id"]}
    with pytest.raises(CustodyError, match="current original accepted Build"):
        lost.validate_settings(saved["id"], settings, build_id=original["build_id"])
    lost.validate_settings(saved["id"], settings, build_id=newer["build_id"])
    recovered = lost.host_for_build(saved["id"], newer["build_id"])
    assert recovered._pins == f.registry.host_for_build(saved["id"], original["build_id"])._pins
    assert not (f.implementation._directory(saved["id"], newer["build_id"]) / "local-application-source").exists()
    assert not lost._installations and not f.service.launched


@pytest.mark.parametrize("changed", ["source", "protected", "acceptance", "missing-source", "guide", "viewer"])
async def test_install_recovery_refuses_changed_original_build_without_effects(registry, changed):
    f = registry
    saved = new_guide(f)
    result = await build(f, saved)
    await accept_source_fixture(f, result)
    directory = f.implementation._directory(saved["id"], result["build_id"])
    if changed == "source":
        (directory / "workspace/project/application/inert.txt").write_text("Changed after acceptance.\n")
    elif changed == "protected":
        path = directory / "local-application-source/access.toml"
        path.write_text('enabled = false\n')
    elif changed == "acceptance":
        path = directory / "acceptance/acceptance-state.json"
        value = json.loads(path.read_text())
        value["rounds"][-1]["publication"]["signature"] = "0" * 64
        path.write_text(json.dumps(value))
    elif changed == "missing-source":
        (directory / "local-application-source/source-selection.json").unlink()
    elif changed == "viewer":
        path = directory / "local-application-source/APPLICATION-SELECTION.json"
        value = json.loads(path.read_text())
        value["operator"]["subject"] = "another-console-user"
        path.write_text(json.dumps(value))
    else:
        state = f.store.get(saved["id"])
        f.store.save(state, state["version"])
    before = source_bytes(directory)
    lost = LocalApplicationRegistry(guide_store=f.store, implementation=f.implementation,
        deployment=f.service, runtime_target=f.target)
    with pytest.raises((CustodyError, DraftConflict, FileNotFoundError, ValueError)):
        lost.validate_settings(saved["id"], {"CONSOLE_USER_ID": "forge-" + saved["id"]},
            build_id=result["build_id"])
    assert before == source_bytes(directory)
    assert not lost._builds and not lost._installations and not f.service.launched


@pytest.mark.parametrize("custody", [False, True], ids=["bare-target", "model-owner"])
async def test_managed_host_product_install_retains_inputs_only_with_launcher_model_owner(
        registry, tmp_path, monkeypatch, custody):
    """A launcher's model owner reaches the managed-host product's retained inputs."""
    from launchkit.connect_application_startup import _ModelConfiguration
    from test_managed_model_factory_selection import model_source
    f = registry
    selection = model_source(tmp_path, monkeypatch)
    monkeypatch.setenv("XDG_CONFIG_HOME", selection["credential_config_root"])
    owner = _ModelConfiguration.observe(selection) if custody else None
    # The server's local runtime selection constructs this registry from the
    # launcher's owner; a bare product target supplies none.
    f.registry = LocalApplicationRegistry(guide_store=f.store, implementation=f.implementation,
        deployment=f.service, runtime_target=f.target,
        model_configuration=None if owner is None else owner.selection())
    f.implementation.prepare_workspace = f.registry.prepare_workspace
    bind_registry(f.service, f.registry)
    saved = new_guide(f)
    saved["proposal"]["agent_classes"][0]["model_auth"] = "managed_host"
    saved = f.store.save(saved, saved["version"])
    result = await build(f, saved)
    await accept_source_fixture(f, result)
    if not custody:
        # Install refuses before any worker, with guidance, instead of failing
        # its input retention after the worker installed the application.
        with pytest.raises(DraftConflict, match=MANAGED_MODELS_WITHOUT_CUSTODY):
            await install_fixture(f, saved, result, monkeypatch)
        assert not (f.root / ("installed-" + saved["id"])).exists()
        assert not f.service.tasks and not f.service.processes
        return
    directory, destination = await install_fixture(f, saved, result, monkeypatch)
    receipt = deployment.read_receipt(directory)
    host = f.registry.selected_host(saved["id"], receipt["deployment_id"])
    assert host._retained_preparation_inputs.model_configuration == selection
    async with f.service.lock:
        chosen = host._selected_preparation(f.service, saved["id"], receipt["deployment_id"])
    assert chosen["model_configuration"] == selection
    policy = json.loads((destination / ".mobkit/forge-local-setup/local-application-policy.json").read_bytes())
    assert policy["model_configuration"] == selection
