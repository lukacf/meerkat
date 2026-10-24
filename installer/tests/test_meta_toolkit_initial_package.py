"""Inert initial package/source controls, never SDK or native qualification."""
import asyncio
from copy import deepcopy
from dataclasses import replace
import io
import hashlib
import json
import os
from pathlib import Path
import tomllib
import zipfile

import pytest

from launchkit import compiler, deployment, deployment_updates as updates
from launchkit.blueprint import Blueprint, starter_blueprint
from launchkit.implementation_workspace import (ImplementationWorkspace, WorkspaceError,
    PROTECTED_APPLICATION_PATHS)
from launchkit.projects import DraftConflict, fingerprint
from launchkit.runtime_selection import candidate_selection
from test_candidate_implementation import (service, bundle, target as legacy_target,
    accepted_fixture, frozen)
from test_implementation import workspace
from test_meta_toolkit_runtime_selection import product, no_product_execution
from test_meta_toolkit_runtime_update import catalog_target
from test_guide import guide_client, start, answer


@pytest.fixture(autouse=True)
def inert_isolation_probe(monkeypatch):
    """A guide turn reads the host's isolation probe, which starts a process.

    This file forbids processes, so the probe passed only after an earlier
    file had cached it. Select an inert sandbox path here, as other fixtures do.
    """
    monkeypatch.setattr("launchkit.implementation_workspace._sandbox_executable", lambda: "/inert/sandbox")


def setup_spec():
    value = starter_blueprint().model_dump(mode="json")
    value["model"] = "gpt-6-astra"
    value["agent_classes"] = value["agent_classes"][:2]
    value["agent_classes"][0]["id"] = "assistant"
    value["agent_classes"][0]["tools"]["rust_bundles"] = ["meerkat_toolkit.setup"]
    value["agent_classes"][1]["id"] = "setup"
    for item in value["agent_classes"]:
        item["model"] = "gpt-6-astra"
        item["instances"] = 1
    value["topology"]["entrypoint"] = "assistant:main"
    value["flows"] = []
    value["acquisition"] = {"helper_identity": "setup:main", "helper_profile": "dedicated",
        "helper_tools": "checked_installation"}
    return Blueprint.model_validate(value).model_dump(mode="json")


def test_scoped_connect_profiles_omit_unsupported_builtin_targets_without_changing_other_products(product):
    target = catalog_target(product)
    value = setup_spec()
    for row in value["agent_classes"]:
        row["tools"].update(memory=True, read_only=True, shell=True, workgraph=True, schedule=True)
    scoped = tomllib.loads(compiler.mob_toml(value, runtime_target=target))
    ordinary_value = deepcopy(value)
    ordinary_value["acquisition"] = None
    ordinary = tomllib.loads(compiler.mob_toml(ordinary_value, runtime_target=target))
    legacy = tomllib.loads(compiler.mob_toml(value))
    assert set(scoped["profiles"]) == set(ordinary["profiles"]) == set(legacy["profiles"])
    for name, selected in scoped["profiles"].items():
        tools = selected["tools"]
        assert tools["comms"] is True and tools["memory"] is False
        assert tools["deny"] == ["peers", "send_message", "reply_to_peer", "send_request", "send_response"]
        expected = deepcopy(ordinary["profiles"][name]["tools"])
        assert expected["memory"] is True and "deny" not in expected
        expected.update(memory=False, deny=tools["deny"])
        assert tools == expected
        assert legacy["profiles"][name]["tools"] == ordinary["profiles"][name]["tools"]
        assert {key: item for key, item in selected.items() if key != "tools"} == {
            key: item for key, item in ordinary["profiles"][name].items() if key != "tools"}


def test_selected_connect_role_guidance_does_not_promise_denied_peer_tools(product):
    from launchkit.design_knowledge import COMMS_PROTOCOL
    from launchkit.guide import compact_guidance_context, guide_package_availability

    target = catalog_target(product)
    value = setup_spec()
    scoped = tomllib.loads(compiler.mob_toml(value, runtime_target=target))
    ordinary = tomllib.loads(compiler.mob_toml(value))
    assert ordinary["skills"]["launch_comms"]["content"] == COMMS_PROTOCOL
    assert "does not expose ordinary peer messaging tools" in scoped["skills"]["launch_comms"]["content"]
    assert "Native Recorder memory is unavailable" in scoped["skills"]["launch_comms"]["content"]
    for role in ("assistant_role", "setup_role"):
        assert "Use your connected peers when useful" in ordinary["skills"][role]["content"]
        assert "Use your connected peers when useful" not in scoped["skills"][role]["content"]
        assert "Ordinary peer messaging is unavailable" in scoped["skills"][role]["content"]
        assert "Native Recorder memory is also unavailable" in scoped["skills"][role]["content"]
    state = {"id": "guide", "version": 1, "proposal": value}
    availability = guide_package_availability(state, runtime_target=target)
    assert availability["available"] is True
    assert len(availability["design_constraints"]) == 1
    assert "denies ordinary peer messaging tools" in availability["design_constraints"][0]
    assert "disables native Recorder memory" in availability["design_constraints"][0]
    context = {"package_availability": availability}
    assert compact_guidance_context(context)["package_availability"] == availability
    assert "design_constraints" not in guide_package_availability(state)
    without_setup = deepcopy(state)
    without_setup["proposal"]["acquisition"] = None
    assert "design_constraints" not in guide_package_availability(without_setup, runtime_target=target)


@pytest.mark.parametrize("requested_memory", [True, False])
def test_helperless_guidance_reports_the_same_native_limits_as_compilation(product, requested_memory):
    from launchkit.guide import compact_guidance_context, guide_package_availability

    target = catalog_target(product)
    value = setup_spec()
    value["acquisition"] = None
    for row in value["agent_classes"]:
        row["tools"].update(rust_bundles=[], memory=requested_memory)
    assert compiler.connect_route(value, target) is True
    compiled = tomllib.loads(compiler.mob_toml(value, runtime_target=target))
    ordinary = tomllib.loads(compiler.mob_toml(value, runtime_target=target, legacy_ordinary=True))
    for name, profile in compiled["profiles"].items():
        assert profile["tools"]["memory"] is False
        assert "send_message" in profile["tools"]["deny"]
        assert ordinary["profiles"][name]["tools"]["memory"] is requested_memory
        assert "deny" not in ordinary["profiles"][name]["tools"]
    state = {"id": "helperless", "version": 1, "proposal": value}
    availability = guide_package_availability(state, runtime_target=target)
    assert availability["available"] is True
    constraints = availability["design_constraints"]
    assert len(constraints) == 1
    assert "denies ordinary peer messaging tools" in constraints[0]
    assert "disables native Recorder memory" in constraints[0]
    assert compact_guidance_context({"package_availability": availability})["package_availability"] == availability
    assert "design_constraints" not in guide_package_availability(state)
    assert all(row["tools"]["memory"] is requested_memory for row in value["agent_classes"])


@pytest.mark.parametrize("add_setup", [True, False])
def test_manual_plan_rebinds_selected_runtime_guidance_before_agent_turn(product, guide_client, add_setup):
    from launchkit.guide import compact_guidance_context
    from launchkit.projects import fingerprint

    client, agent, service = guide_client
    target = catalog_target(product)
    service.package_runtime_provider = lambda state: target
    initial = setup_spec()
    changed = deepcopy(initial)
    if add_setup:
        initial["acquisition"] = None
    else:
        changed["acquisition"] = None
    state = start(client)
    state["blueprint"] = initial
    state = service.store.save(state, state["version"])
    answer(client, state, action="manual", blueprint=changed,
        text="Review this setup capability change while preserving the rest of the work.")
    context = agent.calls[-1]["guide_context"]
    assert agent.calls[-1]["blueprint"] == Blueprint.model_validate(changed).model_dump(mode="json")
    availability = compact_guidance_context(context)["package_availability"]
    assert availability["blueprint_hash"] == fingerprint(Blueprint.model_validate(changed).model_dump(mode="json"))
    assert ("design_constraints" in availability) is add_setup


@pytest.fixture
def inputs(tmp_path):
    root = tmp_path / "original host selected application inputs"
    root.mkdir(mode=0o700)
    values = {
        "application/tool-policy.json": {"provider_id": "application", "policy_id": "inert-selected",
            "revision": 1, "members": [
                {"member_identity": "assistant:main", "grants": [{"tool_name": "find_and_install",
                    "action": "invoke", "consequence": "r1"}]},
                {"member_identity": "setup:main", "grants": [
                    {"tool_name": "toolkit_request_public_service", "action": "invoke", "consequence": "r1"},
                    {"tool_name": "setup_report", "action": "invoke", "consequence": "r1"}]}]},
        "application/configuration.json": {"fields": [
            {"key": "CONSOLE_USER_ID", "label": "User ID", "required": True, "secret": False},
            {"key": "CONSOLE_LOGIN_PASSWORD", "label": "Password", "required": True, "secret": True}]},
        "application/console.json": {"authentication": "local-password"},
        "config/access.toml": '[access]\nmode = "local_only"\n',
    }
    sources = {}
    for name, value in values.items():
        path = root / name
        path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        path.write_text(value if isinstance(value, str) else json.dumps(value) + "\n")
        path.chmod(0o644)
        sources[name] = path
    return sources


def prepare(service, inputs, monkeypatch, *, after=None):
    original = dict(inputs)
    selected = {name: (hashlib.sha256(path.read_bytes()).hexdigest(), path.stat().st_mode & 0o777)
        for name, path in original.items()}
    def require_current():
        if inputs != original or any(selected[name] != (
                hashlib.sha256(path.read_bytes()).hexdigest(), path.stat().st_mode & 0o777)
                for name, path in original.items()):
            raise WorkspaceError("The originally selected application source changed.")
    def hook(guide_id, build_id, project):
        assert guide_id == service.guide.identity
        project.adopt_application_inputs(inputs, require_current=require_current)
        if after is not None:
            after(project)
    monkeypatch.setattr(service, "prepare_workspace", hook)


def report_files(project):
    files = {item.path: (item.as_bytes(), item.mode) for item in project.export_files(require_verified=False)}
    files["implementation-report.json"] = (json.dumps({"framework": project.framework_provenance()}).encode(), 0o644)
    return files


def test_exact_product_can_compile_original_fixed_setup_source_but_public_preview_stays_pending(product):
    target = catalog_target(product)
    spec = setup_spec()
    assert compiler.package_availability(spec)["available"] is False
    assert compiler.package_preview(spec)["files"] == []
    assert compiler.package_availability(spec, runtime_target=target)["available"] is True
    files = {item.path: item for item in compiler.generate_package(spec, runtime_target=target)}
    assert json.loads(files["blueprint.json"].content) == spec
    assert 'rust_bundles = ["meerkat_toolkit.setup"]' in files["config/mob.toml"].content
    assert json.loads(files["runtime-candidate.json"].content) == candidate_selection(target)
    assert f"meerkat-meta==0.9.0 --hash=sha256:{target.meta.sha256}" in files["requirements.txt"].content.splitlines()
    assert f"meerkat-toolkit==0.9.0 --hash=sha256:{target.toolkit.sha256}" in files["requirements.txt"].content.splitlines()
    assert target.meta.version == target.toolkit.version == "0.9.0"
    assert not any(name in files for name in ("application/tool-policy.json", "activation.json", "plan.json"))


def test_connect_route_console_opens_the_toolkit_setup_panel(product):
    target = catalog_target(product)
    files = {item.path: item for item in compiler.generate_package(setup_spec(), runtime_target=target)}
    console = tomllib.loads(files["config/console.toml"].content)
    assert console["sidebar"] == {"buttons": [{"id": "connect-setup", "label": "Connect setup",
        "panel": "/host/toolkit/setup"}]}
    # The native Console docks only a same-origin panel path without a
    # control or link, and drops an invalid button silently.
    panel = console["sidebar"]["buttons"][0]["panel"]
    assert panel.startswith("/") and not panel.startswith("//") and len(panel.encode()) <= 2048
    assert not any(character.isspace() or not character.isprintable() or character in "\\\ufeff"
        for character in panel)
    assert not {".", ".."} & set(panel.split("?", 1)[0].split("#", 1)[0].lower().replace("%2e", ".").split("/"))
    ordinary = deepcopy(setup_spec())
    ordinary["acquisition"] = None
    del ordinary["agent_classes"][0]["tools"]["rust_bundles"]
    without = {item.path: item for item in compiler.generate_package(ordinary, runtime_target=target)}
    assert console == tomllib.loads(without["config/console.toml"].content)


@pytest.mark.parametrize("kind", ["helperless-product", "public-package"])
def test_console_setup_panel_follows_selected_product_route(product, kind):
    spec = setup_spec()
    spec["acquisition"] = None
    del spec["agent_classes"][0]["tools"]["rust_bundles"]
    options = {"runtime_target": catalog_target(product)} if kind == "helperless-product" else {}
    files = {item.path: item for item in compiler.generate_package(spec, **options)}
    console = tomllib.loads(files["config/console.toml"].content)
    assert ("sidebar" in console) == (kind == "helperless-product")
    assert ("/host/toolkit/setup" in files["config/console.toml"].content) == (kind == "helperless-product")


def setup_with_classes_spec():
    value = setup_spec()
    discovery = deepcopy(value["agent_classes"][0])
    discovery.update(id="discovery", tools={**discovery["tools"], "rust_bundles": []})
    value["agent_classes"].extend([discovery, {**deepcopy(discovery),
        "id": "project", "instances": 0, "members": []}])
    return Blueprint.model_validate(value).model_dump(mode="json")


def test_selected_setup_preserves_real_seeds_and_zero_initial_class(product):
    value = setup_with_classes_spec()
    original = deepcopy(value)
    files = {row.path: row.content for row in compiler.generate_package(value, runtime_target=catalog_target(product))}
    deployment = json.loads(files["deployment.json"])
    inventory = json.loads(files["native-inventory.example.json"])
    assert deployment["applicationInventory"] == "application/native-inventory.json"
    assert inventory["helper_identity"] == "setup:main"
    assert [row["identity"] for row in inventory["roster"]] == ["assistant:main", "setup:main", "discovery:main"]
    assert all(row.get("initial_message") is None for row in inventory["roster"])
    assert inventory["member_classes"] == [{"id": "project",
        "profile": deployment["applicationProfiles"]["project"], "factory": "application:project",
        "callbacks": [], "grants": []}]
    assert "application/native-inventory.json" not in files
    assert value == original


@pytest.mark.parametrize("with_helper", [False, True])
def test_selected_class_package_readme_includes_native_contract(product, with_helper):
    from launchkit.application_inventory_contract import HELPERLESS_APPLICATION_CONTRACT

    value = setup_with_classes_spec()
    if not with_helper:
        value["acquisition"] = None
        value["agent_classes"] = [row for row in value["agent_classes"] if row["id"] != "setup"]
        value["agent_classes"][0]["tools"]["rust_bundles"] = []
    files = {row.path: row.content for row in compiler.generate_package(value, runtime_target=catalog_target(product))}
    assert json.loads(files["deployment.json"])["applicationInventory"] == "application/native-inventory.json"
    assert HELPERLESS_APPLICATION_CONTRACT in files["README.md"]


@pytest.mark.parametrize("defect", ["bundle", "no-caller-bundle", "helper-mode", "helper-profile", "zero-initial-caller", "cloud"])
def test_target_does_not_enable_arbitrary_acquisition_or_dynamic_source(product, defect):
    target = catalog_target(product)
    spec = setup_spec()
    if defect == "bundle": spec["agent_classes"][0]["tools"]["rust_bundles"] = ["unknown.native.bundle"]
    elif defect == "no-caller-bundle": del spec["agent_classes"][0]["tools"]["rust_bundles"]
    elif defect == "helper-mode": spec["acquisition"]["helper_tools"] = "preparation"
    elif defect == "helper-profile": spec["acquisition"]["helper_profile"] = "exclusive_application_role"
    elif defect == "zero-initial-caller":
        spec["agent_classes"].append({**deepcopy(spec["agent_classes"][0]),
            "id": "future_caller", "instances": 0, "members": []})
        spec["agent_classes"][0]["tools"]["rust_bundles"] = []
    else: spec["target"] = "gcp"
    with pytest.raises(ValueError):
        compiler.generate_package(spec, runtime_target=target)


async def test_new_protected_product_package_joins_original_policy_viewer_and_export(service, product, inputs, monkeypatch):
    target = catalog_target(product)
    service.guide.state["proposal"] = setup_spec()
    prepare(service, inputs, monkeypatch)
    try:
        status = await service.submit(service.guide.identity, 3, runtime_target=target,
            expected_selected_build_id=None)
        project = workspace(service, status)
        selected = frozen(service, status)
        assert selected["runtime_candidate"] == candidate_selection(target)
        provenance = project.framework_provenance()
        assert provenance["protected_application_paths"] == sorted(inputs)
        assert provenance["baseline_digest"] != provenance["current_compiler_baseline_digest"]
        assert provenance["current_compiler_baseline_digest"] == fingerprint(
            ImplementationWorkspace._baseline(compiler.generate_package(setup_spec(), runtime_target=target)))
        assert updates.current_host_framework(report_files(project), runtime_target=target)
        assert not status["ready_to_package"] and project.verification() is None
        await accepted_fixture(service, status)
        payload, _ = await service.package(service.guide.identity, status["build_id"])
        with zipfile.ZipFile(io.BytesIO(payload)) as archive:
            for name, path in inputs.items():
                assert archive.read(name) == path.read_bytes()
        # The retained fixture acceptance is synthetic; neither code nor SDKs ran.
    finally:
        await service.shutdown()


async def test_missing_original_policy_refuses_before_build_publication(service, product):
    service.guide.state["proposal"] = setup_spec()
    with pytest.raises(DraftConflict, match="original application policy"):
        await service.submit(service.guide.identity, 3, runtime_target=catalog_target(product))
    assert not service.tasks
    assert not (service.root / service.guide.identity / "latest.json").exists()


async def test_actual_private_source_modes_project_to_canonical_archive_and_current_intake(service, product, inputs, monkeypatch):
    for path in inputs.values(): path.chmod(0o600)
    service.guide.state["proposal"] = setup_spec()
    target = catalog_target(product)
    prepare(service, inputs, monkeypatch)
    try:
        status = await service.submit(service.guide.identity, 3, runtime_target=target)
        await accepted_fixture(service, status)
        payload, _ = await service.package(service.guide.identity, status["build_id"])
        files = deployment.package_files(payload)
        assert updates.current_host_framework(files, runtime_target=target)
        project = workspace(service, status)
        for name, source in inputs.items():
            assert source.stat().st_mode & 0o777 == 0o600
            assert files[name] == (source.read_bytes(), 0o644)
            assert project._state()["baseline"][name]["mode"] == 0o644
        # Source mode identity remains distinct from the public package mode.
        inputs["application/tool-policy.json"].chmod(0o644)
        with pytest.raises(WorkspaceError, match="originally selected"):
            await service.submit(service.guide.identity, 3, "Retain the same selected inputs.")
    finally:
        await service.shutdown()


@pytest.mark.parametrize("change", ["original", "copied"])
async def test_protected_inputs_are_rechecked_after_final_await_before_publication(service, product, inputs, monkeypatch, change):
    service.guide.state["proposal"] = setup_spec()
    target = catalog_target(product)
    holder = {}
    prepare(service, inputs, monkeypatch, after=lambda project: holder.update(project=project))
    original = service._guide
    async def guide(identity):
        state = await original(identity)
        if holder:
            path = (inputs["application/tool-policy.json"] if change == "original" else
                holder["project"].project / "application/tool-policy.json")
            path.write_bytes(path.read_bytes() + b"\n")
        return state
    monkeypatch.setattr(service, "_guide", guide)
    with pytest.raises(WorkspaceError):
        await service.submit(service.guide.identity, 3, runtime_target=target)
    assert not service.tasks
    assert not (service.root / service.guide.identity / "latest.json").exists()


@pytest.mark.parametrize("defect", ["source-bytes", "source-mode", "selection", "target", "workspace-bytes", "workspace-mode", "source-parent-link"])
async def test_original_source_or_target_change_during_preparation_refuses_before_publication(service, product, inputs, monkeypatch, defect):
    target = catalog_target(product)
    service.guide.state["proposal"] = setup_spec()
    def change(project):
        path = inputs["application/tool-policy.json"]
        if defect == "source-bytes": path.write_text("changed selected source\n")
        elif defect == "source-mode": path.chmod(0o600)
        elif defect == "selection": inputs.pop("application/console.json")
        elif defect == "target": target.assembly.path.write_bytes(b"changed exact artifact")
        elif defect == "workspace-bytes": (project.project / "application/tool-policy.json").write_text("changed protected source\n")
        elif defect == "source-parent-link":
            held = path.parent.with_name("original-application")
            path.parent.rename(held)
            path.parent.symlink_to(held.name)
        else: (project.project / "application/tool-policy.json").chmod(0o600)
    prepare(service, inputs, monkeypatch, after=change)
    with pytest.raises((ValueError, AssertionError)):
        await service.submit(service.guide.identity, 3, runtime_target=target)
    assert not service.tasks
    assert not (service.root / service.guide.identity / "latest.json").exists()


def test_adopted_inputs_use_existing_protected_tools_and_exact_modes(tmp_path, product, inputs):
    project = ImplementationWorkspace(tmp_path / "protected source", setup_spec())
    project.seed(runtime_target=catalog_target(product))
    project.adopt_application_inputs(inputs, require_current=lambda: None)
    before = project.digest()
    for name in inputs:
        with pytest.raises(WorkspaceError, match="read-only"):
            project.write_file(name, "builder override\n")
        with pytest.raises(WorkspaceError, match="read-only"):
            project.apply_batch([{"kind": "write", "path": name, "content": "builder override\n",
                "expected_sha256": "0" * 64}])
    project.write_file("application/custom.py", "# Ordinary application source remains editable.\n")
    assert project.digest() != before
    assert not hasattr(__import__("launchkit.implementation_worker", fromlist=["BuildTools"]).BuildTools,
        "adopt_application_inputs")


@pytest.mark.parametrize("defect", ["alias", "compiler", "unowned", "writable", "symlink", "hardlink"])
def test_host_adoption_rejects_untrusted_paths_and_source_modes(tmp_path, product, inputs, defect):
    project = ImplementationWorkspace(tmp_path / "protected source", setup_spec())
    project.seed(runtime_target=catalog_target(product))
    before = project.digest()
    sources = dict(inputs)
    path = sources["application/tool-policy.json"]
    if defect == "alias": sources["application/./tool-policy.json"] = sources.pop("application/tool-policy.json")
    elif defect == "compiler": sources["app.py"] = path
    elif defect == "unowned":
        # No chown: source ownership is checked against the real current UID.
        from unittest.mock import patch
        with patch("launchkit.implementation_workspace.os.getuid", return_value=os.getuid() + 1):
            with pytest.raises(WorkspaceError): project.adopt_application_inputs(sources, require_current=lambda: None)
        assert project.digest() == before
        return
    elif defect == "writable": path.chmod(0o666)
    elif defect == "symlink":
        link = path.parent / "linked.json"
        link.symlink_to(path.name)
        sources["application/tool-policy.json"] = link
    else: os.link(path, path.parent / "linked.json")
    with pytest.raises(WorkspaceError): project.adopt_application_inputs(sources, require_current=lambda: None)
    assert project.digest() == before


@pytest.mark.parametrize("defect", ["bytes", "mode", "paths", "compiler-digest"])
def test_admitted_framework_intake_refuses_changed_protected_package_input(tmp_path, product, inputs, defect):
    target = catalog_target(product)
    project = ImplementationWorkspace(tmp_path / "protected source", setup_spec())
    project.seed(runtime_target=target)
    project.adopt_application_inputs(inputs, require_current=lambda: None)
    files = report_files(project)
    assert updates.current_host_framework(files, runtime_target=target)
    name = "application/tool-policy.json"
    if defect == "bytes": files[name] = (b"changed protected policy\n", files[name][1])
    elif defect == "mode": files[name] = (files[name][0], 0o600)
    else:
        value = json.loads(files["implementation-report.json"][0])
        if defect == "paths": value["framework"]["protected_application_paths"] += ["app.py"]
        else: value["framework"]["current_compiler_baseline_digest"] = value["framework"]["baseline_digest"]
        files["implementation-report.json"] = (json.dumps(value).encode(), 0o644)
    assert not updates.current_host_framework(files, runtime_target=target)


async def test_accepted_continuation_retains_protected_inputs_and_original_compiler_digest(service, product, inputs, monkeypatch):
    target = catalog_target(product)
    service.guide.state["proposal"] = setup_spec()
    prepare(service, inputs, monkeypatch)
    try:
        first = await service.submit(service.guide.identity, 3, runtime_target=target)
        original = await accepted_fixture(service, first)
        # Source preservation must carry the authenticated baseline itself,
        # without requiring a second adoption by the original source hook.
        monkeypatch.setattr(service, "prepare_workspace", lambda *args: None)
        second = await service.submit(service.guide.identity, 3, "Improve the custom application response.")
        project = workspace(service, second)
        provenance = project.framework_provenance()
        assert provenance["basis"] == "accepted-baseline"
        assert provenance["retained_template_paths"] == []
        assert provenance["protected_application_paths"] == sorted(inputs)
        assert provenance["current_compiler_baseline_digest"] == original.framework_provenance()["current_compiler_baseline_digest"]
        assert updates.current_host_framework(report_files(project), runtime_target=target)
        for name, source in inputs.items(): assert project.read_file(name) == source.read_text()
        project.write_file("application/new.py", "# Later authored source remains editable.\n")
        assert not second["ready_to_package"] and project.verification() is None
    finally:
        await service.shutdown()


async def test_initial_product_cannot_replace_existing_unaccepted_workspace_and_legacy_stays_refresh_only(service, product, bundle):
    ordinary = await service.submit(service.guide.identity, 3)
    try:
        with pytest.raises(DraftConflict, match="new workspace"):
            await service.submit(service.guide.identity, 3, runtime_target=catalog_target(product))
        with pytest.raises(ValueError, match="preserve-design"):
            await service.submit(service.guide.identity, 3, runtime_target=legacy_target(bundle))
        assert (await service.status(service.guide.identity))["build_id"] == ordinary["build_id"]
    finally:
        await service.shutdown()


def test_ordinary_no_extra_baseline_contract_stays_current(tmp_path):
    spec = starter_blueprint().model_dump(mode="json")
    project = ImplementationWorkspace(tmp_path / "ordinary source", spec)
    project.seed()
    provenance = project.framework_provenance()
    assert "protected_application_paths" not in provenance
    assert provenance["baseline_digest"] == provenance["current_compiler_baseline_digest"]
    assert updates.current_host_framework(report_files(project))


def test_selected_crlf_policy_and_viewer_bytes_are_exact_and_raw_mutation_refuses(tmp_path, product, inputs):
    for path in inputs.values():
        path.write_bytes(path.read_bytes().replace(b"\n", b"\r\n"))
    project = ImplementationWorkspace(tmp_path / "exact protected source", setup_spec())
    target = catalog_target(product)
    project.seed(runtime_target=target)
    project.adopt_application_inputs(inputs, require_current=lambda: None)
    for name, source in inputs.items():
        assert (project.project / name).read_bytes() == source.read_bytes()
        assert next(item for item in project.export_files(require_verified=False) if item.path == name).as_bytes() == source.read_bytes()
    assert updates.current_host_framework(report_files(project), runtime_target=target)
    changed = inputs["application/tool-policy.json"]
    changed.write_bytes(changed.read_bytes().replace(b"\r\n", b"\n"))
    with pytest.raises(WorkspaceError, match="originally selected"):
        project.require_preparation_current()
    copied = project.project / "application/console.json"
    copied.write_bytes(copied.read_bytes().replace(b"\r\n", b"\n"))
    with pytest.raises(WorkspaceError, match="changed outside"):
        project.digest()
