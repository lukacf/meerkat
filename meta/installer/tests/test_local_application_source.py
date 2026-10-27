"""Real initial source/factory/workspace joins with inert product artifacts.

The selected wheels, archive and producer records are synthetic byte fixtures.
No SDK import, gateway, provider, install or native setup is executed here;
source declarations are not authentication, qualification or useful-job proof.
"""
from copy import deepcopy
import json
from pathlib import Path
from types import SimpleNamespace

import pytest

from launchkit import local_application_source as module
from launchkit import compiler, runtime_selection as selection
from launchkit.blueprint import AcquisitionSetup, Blueprint, starter_blueprint
from launchkit.connect_installer_caller import CustodyError, ExactFile
from launchkit.forge_application_host import ForgeApplicationHost
from launchkit.guide_store import GuideStore
from launchkit.implementation_workspace import ImplementationWorkspace, WorkspaceError
from launchkit.local_application_factory import LocalApplicationFactory
from test_meta_toolkit_initial_package import setup_spec
from test_meta_toolkit_runtime_selection import product, no_product_execution
from test_meta_toolkit_runtime_update import catalog_target
import test_meta_toolkit_runtime_selection as product_fixtures
from test_candidate_implementation import bundle, target as signed_target
from test_local_application_registry import registry, new_guide, build, workspace


CHECKED_SETUP_OPERATIONS = set("""setup_state setup_jobs setup_job setup_catalog
setup_catalog_entry setup_catalog_route setup_prepare setup_retry_prepare setup_help
setup_check setup_stop setup_reconcile setup_recovery_manifest setup_guide
setup_inspect_package setup_inspect_schema setup_acquire_python setup_search_node setup_acquire_node
setup_acquire_node_prebuilt
setup_inspect_acquired setup_recipes setup_recipe setup_remember_remote
setup_export_recipe setup_install setup_selected_install setup_selected_check""".split())


@pytest.fixture
def initial(tmp_path, product):
    target = catalog_target(product)
    spec = setup_spec()
    spec["project"]["name"] = "Neighborhood route review"
    spec["project"]["slug"] = "neighborhood-route-review"
    spec["agent_classes"][0]["id"] = "planner"
    spec["agent_classes"][1]["id"] = "connections"
    spec["topology"]["entrypoint"] = "planner:main"
    spec["acquisition"]["helper_identity"] = "connections:main"
    spec = Blueprint.model_validate(spec).model_dump(mode="json")
    store = GuideStore(tmp_path / "original-guide-store")
    saved = store.create({"proposal": spec, "status": "accepted", "stage": "review", "messages": []})
    parent = tmp_path / "private-build-source"
    parent.mkdir(mode=0o700)
    return SimpleNamespace(store=store, saved=saved, target=target,
        output=parent / "source", project=tmp_path / "implementation")


def create(f, **overrides):
    return module.create_local_application_source(**{
        "guide_store": f.store, "guide_id": f.saved["id"], "guide_version": f.saved["version"],
        "runtime_target": f.target, "output_directory": f.output, **overrides})


def contents(directory):
    return {path.relative_to(directory).as_posix(): path.read_bytes()
        for path in directory.rglob("*") if path.is_file() and not path.is_symlink()}


def test_public_research_default_preserves_existing_desired_source(initial):
    prior = initial.saved["proposal"]
    assert Blueprint.model_validate(prior).model_dump(mode="json") == prior
    declared = {**prior["acquisition"], "public_research": False}
    assert AcquisitionSetup.model_validate(declared).model_dump(mode="json") == prior["acquisition"]
    for value in ("true", 1, None):
        with pytest.raises(ValueError):
            AcquisitionSetup.model_validate({**prior["acquisition"], "public_research": value})


def test_public_research_adds_selected_helper_mcp_grants_without_widening_categories(initial):
    spec = Blueprint.model_validate(initial.saved["proposal"])
    inventory = json.loads(compiler.application_inventory_input(spec))
    prior = json.loads(module._initial_policy(spec, inventory, initial.saved["id"]))
    changed = spec.model_dump(mode="json")
    changed["acquisition"]["public_research"] = True
    current = Blueprint.model_validate(changed)
    selected = json.loads(module._initial_policy(current, inventory, initial.saved["id"]))
    assert current.model_dump(mode="json") == changed
    assert compiler.mob_toml(current) == compiler.mob_toml(spec)
    for old, row in zip(prior["members"], selected["members"], strict=True):
        if row["member_identity"] == inventory["helper_identity"]:
            assert row["member_identity"] == "connections:main"
            assert len(old["grants"]) == 33
            assert row["grants"][:-2] == old["grants"]
            assert row["grants"][-2:] == [
                {"tool_name": "setup_search_github", "action": "invoke", "consequence": "r1"},
                {"tool_name": "setup_read_github", "action": "invoke", "consequence": "r1"}]
            assert len(row["grants"]) == 35
        else:
            assert row == old


@pytest.mark.parametrize("value", ["false", 0, None, [], {}])
def test_account_request_selection_requires_a_boolean(initial, value):
    with pytest.raises(ValueError):
        AcquisitionSetup.model_validate({**initial.saved["proposal"]["acquisition"],
            "account_requests": value})


def test_account_request_default_preserves_existing_source_binding(initial):
    prior = initial.saved["proposal"]
    explicit = deepcopy(prior)
    explicit["acquisition"]["account_requests"] = True
    assert Blueprint.model_validate(explicit).model_dump(mode="json") == prior


@pytest.mark.parametrize("public_research", [False, True])
def test_no_account_requests_omits_only_selected_helper_request_authority(initial, public_research):
    desired = deepcopy(initial.saved["proposal"])
    desired["acquisition"]["public_research"] = public_research
    prior = Blueprint.model_validate(desired)
    inventory = json.loads(compiler.application_inventory_input(prior))
    old = json.loads(module._initial_policy(prior, inventory, initial.saved["id"]))
    desired["acquisition"]["account_requests"] = False
    current = Blueprint.model_validate(desired)
    assert current.model_dump(mode="json")["acquisition"]["account_requests"] is False
    assert compiler.application_inventory_input(current) == compiler.application_inventory_input(prior)
    assert compiler.mob_toml(current) == compiler.mob_toml(prior)
    selected = json.loads(module._initial_policy(current, inventory, initial.saved["id"]))
    for before, after in zip(old["members"], selected["members"], strict=True):
        if before["member_identity"] != "connections:main":
            assert after == before
        else:
            assert after["grants"] == [grant for grant in before["grants"]
                if grant["tool_name"] != "toolkit_request_service_account"]
            assert len(after["grants"]) == (34 if public_research else 32)
            assert any(grant["tool_name"] == "toolkit_request_public_service" for grant in after["grants"])
    # The normal protected source path must carry the same narrowed policy,
    # not only the pure policy projection above.
    saved = initial.store.save({**initial.saved,
        "proposal": current.model_dump(mode="json")}, initial.saved["version"])
    initial.saved = saved
    host = create(initial)()
    assert json.loads(dict(host._protected)["application/tool-policy.json"].read()) == selected


@pytest.mark.parametrize("operation", ["tool_catalog_search", "tool_catalog_load",
    "setup_search_node", "setup_acquire_node_prebuilt", "setup_install", "setup_selected_check"])
def test_real_protected_helper_policy_declares_discovery_and_selected_setup_operations(initial, operation):
    host = create(initial)()
    policy = json.loads(dict(host._protected)["application/tool-policy.json"].read())
    helper = host._inventory["helper_identity"]
    rows = {row["member_identity"]: row["grants"] for row in policy["members"]}
    matches = [row for row in rows[helper] if row["tool_name"] == operation]
    assert len(matches) == 1 and matches[0]["action"] == "invoke"
    assert matches[0]["consequence"] in {"r0", "r1", "r2", "r3"}
    # The arbitrary logical helper is selected by its original inventory,
    # never by a hardcoded lowered ID or by granting the original catalog access.
    assert helper == "connections:main"
    assert rows["planner:main"] == [
        {"tool_name": "find_and_install", "action": "invoke", "consequence": "r1"}]
    assert not any(row["tool_name"].startswith("mk--") or "*" in row["tool_name"]
        for grants in rows.values() for row in grants)
    by_name = {row["tool_name"]: row for row in rows[helper]}
    assert len(by_name) == len(rows[helper])
    assert set(by_name) == CHECKED_SETUP_OPERATIONS | {"tool_catalog_search", "tool_catalog_load",
        "setup_report", "toolkit_request_public_service", "toolkit_request_service_account"}
    assert by_name["tool_catalog_search"]["consequence"] == "r0"
    assert by_name["tool_catalog_load"]["consequence"] == "r0"
    assert by_name["setup_install"]["consequence"] == "r2"
    assert by_name["setup_selected_install"]["consequence"] == "r2"


async def test_generated_schema_inspection_grant_is_helper_only_and_reaches_protected_workspace(registry):
    # Exercise normal initial Build admission and its protected source owner.
    # Existing accepted policies remain immutable; this guide has no predecessor.
    f = registry
    saved = new_guide(f)
    changed = deepcopy(saved)
    spec = changed["proposal"]
    spec["agent_classes"][0]["id"] = "planner"
    spec["agent_classes"][1]["id"] = "connections"
    spec["topology"]["entrypoint"] = "planner:main"
    spec["acquisition"].update(helper_identity="connections:main",
        public_research=True, account_requests=False)
    analyst = deepcopy(spec["agent_classes"][0])
    analyst["id"] = "analyst"
    analyst["tools"]["rust_bundles"] = []
    spec["agent_classes"].append(analyst)
    changed["proposal"] = Blueprint.model_validate(spec).model_dump(mode="json")
    saved = f.store.save(changed, changed["version"])
    admitted = await build(f, saved)
    host = f.registry.host_for_build(saved["id"], admitted["build_id"])
    raw = dict(host._protected)["application/tool-policy.json"].read()
    policy = json.loads(raw)
    rows = {row["member_identity"]: row["grants"] for row in policy["members"]}
    expected = {"tool_name": "setup_inspect_schema", "action": "invoke", "consequence": "r0"}
    assert [row for row in rows["connections:main"]
        if row["tool_name"] == "setup_inspect_schema"] == [expected]
    r1 = set("""setup_prepare setup_retry_prepare setup_check setup_stop setup_reconcile
        setup_acquire_python setup_search_node setup_acquire_node setup_acquire_node_prebuilt
        setup_remember_remote setup_export_recipe setup_selected_check""".split())
    r2 = {"setup_install", "setup_selected_install"}
    ordinary = {"tool_catalog_search", "tool_catalog_load"} | CHECKED_SETUP_OPERATIONS
    helper = [{"tool_name": name, "action": "invoke", "consequence":
        "r2" if name in r2 else "r1" if name in r1 else "r0"} for name in sorted(ordinary)]
    helper += [{"tool_name": name, "action": "invoke", "consequence": "r1"}
        for name in ("setup_report", "toolkit_request_public_service",
            "setup_search_github", "setup_read_github")]
    expected_members = [
        {"member_identity": "planner:main", "grants": [
            {"tool_name": "find_and_install", "action": "invoke", "consequence": "r1"}]},
        {"member_identity": "connections:main", "grants": helper},
        {"member_identity": "analyst:main", "grants": []}]
    def ordered(members):
        return sorted(({**row, "grants": sorted(row["grants"], key=lambda item: item["tool_name"])}
            for row in members), key=lambda row: row["member_identity"])
    assert {**policy, "members": ordered(policy["members"])} == {
        "provider_id": "meta-forge:" + saved["id"],
        "policy_id": "application-tools:" + saved["id"], "revision": 1,
        "members": ordered(expected_members)}
    project = workspace(f.implementation, admitted)
    assert project.read_file("application/tool-policy.json").encode() == raw
    with pytest.raises(WorkspaceError, match="read-only"):
        project.write_file("application/tool-policy.json", "{}\n")
    host.require_current()



def test_current_arbitrary_guide_reaches_original_factory_and_protected_workspace(initial):
    f = initial
    factory = create(f)
    assert type(factory) is LocalApplicationFactory
    host = factory()
    assert type(host) is ForgeApplicationHost and host is factory.host
    assert host.guide_id == f.saved["id"]
    acquisition = json.loads(factory.acquisition_selection.read())
    blueprint = ExactFile(Path(acquisition["blueprint"]["path"]), acquisition["blueprint"]["sha256"])
    assert json.loads(blueprint.read()) == f.saved["proposal"]
    viewer = json.loads(factory.viewer_selection.read())
    assert viewer["base_source_selection"] == factory.acquisition_selection.pin()
    subject = "forge-" + f.saved["id"]
    assert viewer["operator"] == {"subject": subject, "private_configuration_key": "CONSOLE_USER_ID"}
    protected = dict(host._protected)
    assert set(protected) == {"application/tool-policy.json", "application/configuration.json",
        "application/console.json", "config/access.toml"}
    policy = json.loads(protected["application/tool-policy.json"].read())
    helper_identity = host._inventory["helper_identity"]
    for row in policy["members"]:
        if row["member_identity"] == helper_identity:
            assert {grant["tool_name"] for grant in row["grants"]} == CHECKED_SETUP_OPERATIONS | {
                "tool_catalog_search", "tool_catalog_load", "setup_report",
                "toolkit_request_public_service", "toolkit_request_service_account"}
            assert all(grant["action"] == "invoke" for grant in row["grants"])
        else:
            assert row["grants"] == [{"tool_name": "find_and_install", "action": "invoke", "consequence": "r1"}]
    fields = json.loads(protected["application/configuration.json"].read())["fields"]
    assert {row["key"] for row in fields} == {"CONSOLE_USER_ID", "CONSOLE_LOGIN_PASSWORD"}
    assert all("default" not in row for row in fields)
    assert host.console_operator_declaration()["id"] == subject

    before = contents(f.output)
    for _ in range(2):
        assert factory() is host
        host.require_current()
        assert host.initial_build_options(f.saved["id"]) == {"runtime_target": f.target}
        factory.acquisition_selection.read()
        factory.viewer_selection.read()
    assert contents(f.output) == before
    assert host._installation_supervisor is None and factory.preparation_owner._inputs is None

    workspace = ImplementationWorkspace(f.project, f.saved["proposal"])
    workspace.seed(runtime_target=f.target)
    original_definition = workspace.read_file("config/mob.toml")
    host.prepare_workspace(f.saved["id"], "initial-owned-build", workspace)
    assert workspace.read_file("config/mob.toml") == original_definition
    for name, pin in protected.items():
        assert workspace.read_file(name).encode() == pin.read()
        with pytest.raises(WorkspaceError, match="read-only"):
            workspace.write_file(name, "unselected source override\n")
    for wheel in (f.target.meta_wheel, f.target.toolkit_wheel, *f.target.dependency_wheels):
        assert (workspace.project / "application/vendor" / wheel.path.name).read_bytes() == wheel.path.read_bytes()
    workspace.require_preparation_current()
    assert contents(f.output) == before


@pytest.mark.parametrize("change", ["version", "missing-guide", "unsupported-helper"])
def test_unadmitted_guide_or_setup_choice_refuses_before_output(initial, monkeypatch, change):
    f = initial
    arguments = {}
    expected = "exact current completed version"
    if change == "version":
        newer = deepcopy(f.saved)
        newer["proposal"]["project"]["name"] += " revised"
        f.store.save(newer, newer["version"])
    elif change == "missing-guide":
        arguments["guide_id"] = "00000000-0000-4000-8000-000000000001"
        assert f.store.get(arguments["guide_id"]) is None
    else:
        newer = deepcopy(f.saved)
        newer["proposal"]["acquisition"]["helper_tools"] = "preparation"
        Blueprint.model_validate(newer["proposal"])
        f.saved = f.store.save(newer, newer["version"])
        expected = "This plan selects a connection setup helper"
    def no_artifact_read(*args, **kwargs):
        pytest.fail("Guide/setup refusal must precede product artifact admission.")
    monkeypatch.setattr(module, "selected_runtime_artifacts", no_artifact_read)
    with pytest.raises(CustodyError, match=expected):
        create(f, **arguments)
    assert not f.output.exists() and list(f.output.parent.iterdir()) == []


def test_missing_original_store_is_not_recreated_by_source_selection(initial):
    f = initial
    database = f.store.root / "guided-setups.sqlite3"
    database.unlink()
    with pytest.raises(CustodyError, match="guide storage is missing or changed"):
        create(f)
    assert not database.exists() and not f.output.exists()
    assert list(f.output.parent.iterdir()) == []


def test_changed_original_source_refuses_cached_host_and_workspace_admission(initial):
    f = initial
    factory = create(f)
    host = factory()
    policy = dict(host._protected)["application/tool-policy.json"]
    policy.path.write_bytes(policy.read() + b" ")
    changed = contents(f.output)
    with pytest.raises(CustodyError, match="selected bytes"):
        factory()
    with pytest.raises(CustodyError, match="selected bytes"):
        host.initial_build_options(f.saved["id"])
    workspace = ImplementationWorkspace(f.project, f.saved["proposal"])
    workspace.seed(runtime_target=f.target)
    before = contents(workspace.project)
    with pytest.raises(CustodyError, match="selected bytes"):
        host.prepare_workspace(f.saved["id"], "initial-owned-build", workspace)
    assert contents(workspace.project) == before and contents(f.output) == changed


def test_guide_change_during_publication_retains_partial_source_without_factory(initial, monkeypatch):
    f = initial
    original_write = module._write_bytes
    writes = []
    def changed_after_write(path, content):
        pin = original_write(path, content)
        writes.append(pin)
        newer = deepcopy(f.saved)
        newer["proposal"]["project"]["name"] += " changed during Build"
        f.store.save(newer, newer["version"])
        return pin
    monkeypatch.setattr(module, "_write_bytes", changed_after_write)
    with pytest.raises(CustodyError, match="exact current completed version"):
        create(f)
    assert len(writes) == 1 and writes[0].path == f.output / "blueprint.json"
    partial = contents(f.output)
    assert set(partial) == {"blueprint.json"}
    assert json.loads(partial["blueprint.json"]) == f.saved["proposal"]
    with pytest.raises(CustodyError, match="exact current completed version"):
        create(f)
    assert contents(f.output) == partial and len(writes) == 1


@pytest.fixture
def linux_product_source(monkeypatch):
    """Construct the complete inert producer closure for Linux before selection."""
    target = "linux-x86_64"
    prefix = "meerkat-connect-" + target
    monkeypatch.setattr(product_fixtures, "TARGET", target)
    monkeypatch.setattr(product_fixtures, "PREFIX", prefix)
    monkeypatch.setattr(product_fixtures, "RESOURCE", prefix + "/libexec/meerkat-connect")


def cloud_plan(value):
    value = deepcopy(value)
    value["target"] = "gcp"
    value["gcp"].update(project="fixture-cloud-project", zone="europe-west1-b",
        machine_type="e2-standard-4", disk_gb=64)
    return Blueprint.model_validate(value).model_dump(mode="json")


@pytest.fixture
def cloud_initial(linux_product_source, request):
    f = request.getfixturevalue("initial")
    saved = deepcopy(f.saved)
    saved["proposal"] = cloud_plan(saved["proposal"])
    f.saved = f.store.save(saved, saved["version"])
    return f


@pytest.fixture
def cloud_registry(linux_product_source, request):
    return request.getfixturevalue("registry")


def test_trusted_linux_product_preserves_original_cloud_plan_in_protected_source(cloud_initial):
    f = cloud_initial
    before = f.store.get(f.saved["id"])
    original = f.saved["proposal"]
    original_artifacts = {pin.path: pin.path.read_bytes() for pin in (
        f.target.assembly, f.target.meta_wheel, f.target.toolkit_wheel,
        f.target.model_catalog, f.target.dependency_requirements, *f.target.dependency_wheels)}
    with pytest.raises(ValueError, match="does not match this deployment platform"):
        selection.candidate_selection(f.target, deployment_target="gcp")
    factory = create(f)
    host = factory()
    project = ImplementationWorkspace(f.project, original)
    project.seed(runtime_target=f.target)
    host.prepare_workspace(f.saved["id"], "cloud-source-build", project)
    assert json.loads(project.read_file("blueprint.json")) == original
    assert json.loads(project.read_file("deployment.json"))["target"] == "gcp"
    assert json.loads(project.read_file("blueprint.json"))["gcp"] == original["gcp"]
    deploy = project.read_file("deploy/deploy-gcp.sh")
    assert "PROJECT=fixture-cloud-project\n" in deploy and "ZONE=europe-west1-b\n" in deploy
    assert "MACHINE=e2-standard-4\n" in deploy and "--boot-disk-size 64GB" in deploy
    assert "INSTANCE=" + original["project"]["slug"] + "\n" in deploy
    assert project.framework_provenance()["runtime_candidate"] == selection.candidate_selection(f.target)
    assert set(project.framework_provenance()["protected_application_paths"]) == set(dict(host._protected))
    for name, pin in host._protected:
        assert project.read_file(name).encode() == pin.read()
        with pytest.raises(WorkspaceError, match="read-only"):
            project.write_file(name, "unselected cloud override\n")
    for wheel in (f.target.meta_wheel, f.target.toolkit_wheel, *f.target.dependency_wheels):
        assert (project.project / "application/vendor" / wheel.path.name).read_bytes() == wheel.path.read_bytes()
    project.require_preparation_current()
    assert f.store.get(f.saved["id"]) == before
    assert all(pin.read_bytes() == content for pin, content in original_artifacts.items())
    assert host._installation_supervisor is None and factory.preparation_owner._inputs is None


def test_cloud_helper_source_preserves_zero_initial_class_and_original_guide(cloud_initial):
    f = cloud_initial
    value = deepcopy(f.saved["proposal"])
    value["agent_classes"].append({**deepcopy(value["agent_classes"][1]),
        "id": "project", "instances": 0, "members": [], "tools": {}})
    f.saved = f.store.save({**f.saved,
        "proposal": Blueprint.model_validate(value).model_dump(mode="json")}, f.saved["version"])
    before = f.store.get(f.saved["id"])
    factory = create(f)
    host = factory()
    assert host._inventory["helper_identity"] == "connections:main"
    assert [row["identity"] for row in host._inventory["roster"]] == ["planner:main", "connections:main"]
    assert host._inventory["member_classes"] == []
    assert host._accepted_inventory_path == "application/native-inventory.json"
    project = ImplementationWorkspace(f.project, f.saved["proposal"])
    project.seed(runtime_target=f.target)
    host.prepare_workspace(f.saved["id"], "cloud-classes-source-build", project)
    example = json.loads(project.read_file("native-inventory.example.json"))
    assert [row["id"] for row in example["member_classes"]] == ["project"]
    assert all(row.get("initial_message") is None for row in example["roster"])
    assert f.store.get(f.saved["id"]) == before
    assert host._installation_supervisor is None and factory.preparation_owner._inputs is None


@pytest.mark.asyncio
async def test_actual_reserved_cloud_build_uses_original_registry_source_owner(cloud_registry):
    f = cloud_registry
    saved = new_guide(f, "Original cloud route review")
    saved["proposal"] = cloud_plan(saved["proposal"])
    saved = f.store.save(saved, saved["version"])
    before = f.store.get(saved["id"])
    result = await build(f, saved)
    project = workspace(f.implementation, result)
    host = f.registry.host_for_build(saved["id"], result["build_id"])
    directory = f.implementation._directory(saved["id"], result["build_id"])
    frozen = json.loads((directory / "input.json").read_bytes())
    assert frozen["blueprint"] == saved["proposal"]
    assert frozen["runtime_candidate"] == selection.candidate_selection(f.target)
    assert json.loads(project.read_file("deployment.json"))["target"] == "gcp"
    assert json.loads(project.read_file("blueprint.json"))["gcp"] == saved["proposal"]["gcp"]
    assert project.framework_provenance()["protected_application_paths"] == sorted(dict(host._protected))
    project.require_preparation_current()
    assert f.store.get(saved["id"]) == before and not f.service.launched
    assert not result["ready_to_package"] and project.verification() is None
    assert host._installation_supervisor is None and host._retained_preparation_inputs is None


@pytest.mark.parametrize("defect", ["helper-mode", "no-caller-bundle", "foreign-bundle", "zero-initial-caller"])
def test_cloud_product_source_keeps_actual_helper_bundle_and_count_guards(cloud_initial, monkeypatch, defect):
    f = cloud_initial
    saved = deepcopy(f.saved)
    value = saved["proposal"]
    if defect == "helper-mode": value["acquisition"]["helper_tools"] = "preparation"
    elif defect == "no-caller-bundle": value["agent_classes"][0]["tools"]["rust_bundles"] = []
    elif defect == "foreign-bundle": value["agent_classes"][0]["tools"]["rust_bundles"] = ["foreign.setup"]
    else:
        value["agent_classes"].append({**deepcopy(value["agent_classes"][0]),
            "id": "future_caller", "instances": 0, "members": []})
        value["agent_classes"][0]["tools"]["rust_bundles"] = []
    saved["proposal"] = Blueprint.model_validate(value).model_dump(mode="json")
    f.saved = f.store.save(saved, saved["version"])
    monkeypatch.setattr(module, "selected_runtime_artifacts",
        lambda *args, **kwargs: pytest.fail("Unsupported source strategy must refuse before artifacts or files."))
    with pytest.raises(CustodyError, match="This plan selects a connection setup helper"):
        create(f)
    assert not f.output.exists() and list(f.output.parent.iterdir()) == []


def test_cloud_product_source_refuses_mac_selected_product_before_publication(initial):
    f = initial
    saved = deepcopy(f.saved)
    saved["proposal"] = cloud_plan(saved["proposal"])
    f.saved = f.store.save(saved, saved["version"])
    with pytest.raises(ValueError, match="trusted Linux x86_64"):
        selection.implementation_candidate_selection(f.target, deployment_target="gcp")
    with pytest.raises(CustodyError, match="This plan selects a connection setup helper"):
        create(f)
    assert not f.output.exists() and list(f.output.parent.iterdir()) == []


def test_cloud_product_source_refuses_signed_legacy_target_and_keeps_ordinary_default(bundle):
    target = signed_target(bundle)
    with pytest.raises(ValueError, match="trusted Linux x86_64"):
        selection.implementation_candidate_selection(target, deployment_target="gcp")
    setup = cloud_plan(setup_spec())
    assert compiler.package_availability(setup)["available"] is False
    assert compiler.package_availability(setup, runtime_target=target)["available"] is False
    ordinary = cloud_plan(starter_blueprint().model_dump(mode="json"))
    files = {item.path: item for item in compiler.generate_package(ordinary)}
    assert json.loads(files["blueprint.json"].content) == ordinary
    runtime = json.loads(files["deployment.json"].content)
    assert runtime["target"] == "gcp" and "runtimeCandidate" not in runtime
    assert "runtime-candidate.json" not in files and "meerkat-mobkit==" in files["requirements.txt"].content


def test_cloud_product_source_rechecks_complete_original_linux_closure_before_output(cloud_initial):
    f = cloud_initial
    original = f.target.meta_wheel.path.read_bytes()
    f.target.meta_wheel.path.write_bytes(original + b"unselected wheel drift")
    with pytest.raises(ValueError):
        create(f)
    assert not f.output.exists() and list(f.output.parent.iterdir()) == []


@pytest.mark.parametrize("case", ["cloud", "native-bundle"])
def test_plan_without_a_setup_helper_names_its_actual_missing_input(initial, monkeypatch, case):
    # Only a plan that selects the helper is pending connection setup support.
    f = initial
    saved = deepcopy(f.saved)
    value = saved["proposal"]
    value["acquisition"] = None
    for index, row in enumerate(value["agent_classes"]):
        row["tools"]["rust_bundles"] = ["foreign.native"] if case == "native-bundle" and index == 0 else []
    if case == "cloud":
        value = cloud_plan(value)
    saved["proposal"] = Blueprint.model_validate(value).model_dump(mode="json")
    f.saved = f.store.save(saved, saved["version"])
    monkeypatch.setattr(module, "selected_runtime_artifacts",
        lambda *args, **kwargs: pytest.fail("An unsupported source must refuse before artifacts or files."))
    expected = {"cloud": module.ORDINARY_TARGET_UNSUPPORTED,
        "native-bundle": compiler.NATIVE_BUNDLE_PENDING}[case]
    with pytest.raises(CustodyError) as refused:
        create(f)
    assert str(refused.value) == expected
    assert "selects a connection setup helper" not in str(refused.value)
    assert not f.output.exists() and list(f.output.parent.iterdir()) == []


# Original configuration documents are field declarations, never private values.
def configuration_document(f, mutate=None):
    from launchkit.templates_v2.console_configuration import CONSOLE_FIELDS
    from hashlib import sha256
    value = {"fields": [*(dict(row) for row in CONSOLE_FIELDS),
        {"key": "DOMAIN_API_TOKEN", "label": "Domain API token", "description": "Supplied at Install.",
         "secret": True, "required": True}]}
    if mutate is not None:
        mutate(value)
    content = (json.dumps(value, indent=4) + "\n").encode()
    path = f.output.parent / "original-configuration.json"
    path.write_bytes(content)
    path.chmod(0o600)
    return ExactFile(path, sha256(content).hexdigest()), content


def test_original_complete_configuration_reaches_protected_install_source(initial):
    f = initial
    pin, content = configuration_document(f)
    factory = create(f, configuration_source=pin)
    viewer = json.loads(factory.viewer_selection.read())
    assert viewer["source_inputs"]["application/configuration.json"] == pin.pin()
    assert not (f.output / "configuration.json").exists()
    host = factory()
    assert dict(host._protected)["application/configuration.json"].read() == content
    project = ImplementationWorkspace(f.project, f.saved["proposal"])
    project.seed(runtime_target=f.target)
    host.prepare_workspace(f.saved["id"], "original-configuration-build", project)
    assert project.read_file("application/configuration.json").encode() == content
    from launchkit.templates_v2.application_configuration import application_fields
    assert [row["key"] for row in application_fields(project.project)] == [
        "CONSOLE_USER_ID", "CONSOLE_LOGIN_PASSWORD", "DOMAIN_API_TOKEN"]
    with pytest.raises(WorkspaceError, match="read-only"):
        project.write_file("application/configuration.json", '{"fields":[]}')
    assert project.verification() is None
    project.require_preparation_current()
    assert pin.read() == content
    pin.path.unlink()
    with pytest.raises((CustodyError, OSError)):
        project.require_preparation_current()


@pytest.mark.parametrize("defect", ["missing-console", "console-default", "secret-default", "host-key", "duplicate"])
def test_original_configuration_keeps_existing_console_and_private_field_rules(initial, defect):
    def mutate(value):
        rows = value["fields"]
        if defect == "missing-console":
            rows.pop(0)
        elif defect == "console-default":
            rows[0]["default"] = "wrong-source-user"
        elif defect == "secret-default":
            rows[-1]["default"] = "synthetic-forbidden-default"
        elif defect == "host-key":
            rows[-1]["key"] = "PYTHONPATH"
        else:
            rows.append(dict(rows[0]))
    pin, _ = configuration_document(initial, mutate)
    with pytest.raises(ValueError):
        create(initial, configuration_source=pin)
    assert not initial.output.exists()


def test_original_configuration_change_during_source_publication_refuses(initial, monkeypatch):
    pin, _ = configuration_document(initial)
    original = module._write_bytes
    def changed(path, content):
        result = original(path, content)
        pin.path.write_text('{"fields":[]}\n')
        return result
    monkeypatch.setattr(module, "_write_bytes", changed)
    with pytest.raises(CustodyError):
        create(initial, configuration_source=pin)
    assert not (initial.output / "APPLICATION-SELECTION.json").exists()


@pytest.mark.parametrize("encoding", ["utf-16", "utf-32"])
def test_original_configuration_requires_utf8_before_source_publication(initial, encoding):
    from hashlib import sha256
    pin, content = configuration_document(initial)
    encoded = content.decode("utf-8").encode(encoding)
    pin.path.write_bytes(encoded)
    selected = ExactFile(pin.path, sha256(encoded).hexdigest())
    with pytest.raises(UnicodeDecodeError):
        create(initial, configuration_source=selected)
    assert not initial.output.exists()
    assert selected.read() == encoded


@pytest.mark.parametrize("wrong", ["path-only", "dictionary"])
def test_automatic_configuration_requires_original_exact_file(initial, wrong):
    pin, _ = configuration_document(initial)
    value = str(pin.path) if wrong == "path-only" else pin.pin()
    with pytest.raises(CustodyError, match="original exact declaration file"):
        create(initial, configuration_source=value)
    assert not initial.output.exists()


@pytest.mark.parametrize("helperless", [False, True])
async def test_trusted_launcher_forwards_exact_guide_configuration_supplier(registry, monkeypatch, helperless):
    from launchkit import server
    from launchkit.implementation import ImplementationService
    from test_local_application_registry import shipping_callbacks
    from hashlib import sha256
    f = registry
    saved = new_guide(f, ordinary=helperless)
    if helperless:
        # The starter model can differ from this inert product's exact catalog.
        model = json.loads(f.target.model_catalog.path.read_bytes())["models"][0]
        proposal = deepcopy(saved["proposal"])
        for row in (proposal, *proposal["agent_classes"]):
            row.update(provider=model["provider"], model=model["id"])
        saved = f.store.save({**saved,
            "proposal": Blueprint.model_validate(proposal).model_dump(mode="json")}, saved["version"])
    value = {"fields": [
        {"key": "CONSOLE_USER_ID", "label": "Console user", "secret": False, "required": True},
        {"key": "CONSOLE_LOGIN_PASSWORD", "label": "Console password", "secret": True, "required": True},
        {"key": "DOMAIN_API_TOKEN", "label": "Domain API token", "secret": True, "required": True}]}
    path = f.root / "original-field-declarations.json"
    content = (json.dumps(value, indent=2) + "\n").encode()
    path.write_bytes(content)
    path.chmod(0o600)
    pin = ExactFile(path, sha256(content).hexdigest())
    selection = [pin]
    calls = []
    def selected(guide_id, guide_version):
        calls.append((guide_id, guide_version))
        assert (guide_id, guide_version) == (saved["id"], saved["version"])
        return selection[0]
    monkeypatch.setattr(server, "application_host", None)
    monkeypatch.setattr(server, "guide", f.guide)
    monkeypatch.setattr(server, "implementation", f.implementation)
    monkeypatch.setattr(server, "deployment", f.service)
    monkeypatch.setattr(f.implementation, "prepare_workspace",
        ImplementationService.prepare_workspace.__get__(f.implementation))
    for name, _ in shipping_callbacks():
        monkeypatch.setattr(f.service, name, None)
    owner = server.select_local_application_runtime(f.target, application_configuration_selection=selected)
    f.registry = owner
    assert server.select_local_application_runtime(f.target, application_configuration_selection=selected) is owner
    result = await build(f, saved)
    assert calls == [(saved["id"], saved["version"])]
    project = workspace(f.implementation, result)
    assert project.read_file("application/configuration.json").encode() == content
    assert not result["ready_to_package"]
    with pytest.raises(WorkspaceError, match="read-only"):
        project.write_file("application/configuration.json", '{"fields":[]}')
    from launchkit.projects import DraftConflict
    with pytest.raises(DraftConflict, match="original selected application runtime owner"):
        server.select_local_application_runtime(f.target, application_configuration_selection=lambda *_: pin)
    owner.application_configuration_selection = lambda *_: pin
    with pytest.raises(CustodyError, match="original services and source selections"):
        owner._require_registry()
    owner.application_configuration_selection = selected
    if not helperless:
        from test_local_application_registry import accept_source_fixture
        await accept_source_fixture(f, result)
        selection[0] = None
        continued = await build(f, saved, "Refine the application", previous=result)
        assert calls == [(saved["id"], saved["version"])]
        assert workspace(f.implementation, continued).read_file("application/configuration.json").encode() == content


def test_builder_contract_distinguishes_protected_and_authorable_configuration():
    from launchkit.implementation import APPLICATION_CONTRACT
    text = " ".join(APPLICATION_CONTRACT.split())
    assert "When the original host selected this file, the complete document is protected" in text
    assert "automatic local product routes both with and without a setup helper" in text
    assert "report missing declarations to that source owner instead of editing or replacing the file" in text
    assert "Only where this file is authorable, declare additional private wizard fields" in text
