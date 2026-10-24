"""Pinned class composition, with no native construction or admission claims."""
from copy import deepcopy
import hashlib
import json
from types import SimpleNamespace

import pytest

from launchkit.connect_application_callbacks import SelectedApplicationCallbacks
from launchkit.connect_application_startup import DYNAMIC_ROSTER_UNSUPPORTED
from launchkit.connect_installer_caller import CustodyError, ExactFile
from test_application_callbacks import Fixture


class ClassFixture(Fixture):
    def __init__(self, root):
        super().__init__(root, native=True)
        self.declaration = {"id": "reader", "profile": "reusable-reader", "factory": "application:reader",
            "callbacks": [{"name": "read_record", "description": "Read this record",
                "input_schema": {"type": "object", "properties": {}, "additionalProperties": False}}],
            "grants": [{"tool_name": "read_record", "action": "invoke", "consequence": "r0"}]}
        self.definition["profiles"]["reusable-reader"] = deepcopy(self.definition["profiles"]["reader-source"])
        self.value["application"]["definition"]["definition"] = deepcopy(self.definition)
        self.value["application"]["member_classes"] = [self.declaration]
        self.observed["profiles"]["reusable-reader"] = deepcopy(self.definition["profiles"]["reusable-reader"])
        self.deployment = {"classes": [{"id": "helper"}, {"id": "reader"}],
            "applicationProfiles": {"helper": "reusable-helper", "reader": "reusable-reader"},
            "agents": [{"identity": "helper:setup", "profile": "helper-source", "classId": "helper"},
                       {"identity": "reader:main", "profile": "reader-source", "classId": "reader"}]}
        self.select()

    def select(self):
        self.path.write_bytes(json.dumps(self.value).encode())
        self.pin = ExactFile(self.path, hashlib.sha256(self.path.read_bytes()).hexdigest())
        self.callbacks = SelectedApplicationCallbacks(self.pin)


def test_original_class_declaration_supplies_catalog_without_a_placeholder(tmp_path):
    f = ClassFixture(tmp_path)
    catalog = f.callbacks.dynamic_catalog(f.deployment)
    assert catalog == {"reader": {"read_record": f.declaration["callbacks"][0]["input_schema"]}}
    assert set(f.callbacks.member_classes) == {"reader"}
    assert "reader:new" not in f.callbacks._fixed
    catalog["reader"].clear()
    assert f.callbacks.dynamic_catalog(f.deployment)["reader"]


@pytest.mark.parametrize("case", ["unknown-class", "wrong-profile", "fixed-profile", "duplicate",
    "missing-callbacks", "callback-duplicate", "shared-workflow", "changed-plan"])
def test_class_cannot_change_profile_callback_selection_or_original_pin(tmp_path, case):
    f = ClassFixture(tmp_path)
    if case == "unknown-class": f.declaration["id"] = "foreign"
    elif case == "wrong-profile": f.declaration["profile"] = "missing"
    elif case == "fixed-profile": f.declaration["profile"] = "reader-source"
    elif case == "duplicate": f.value["application"]["member_classes"].append(deepcopy(f.declaration))
    elif case == "missing-callbacks": del f.declaration["callbacks"]
    elif case == "callback-duplicate": f.declaration["callbacks"].append(deepcopy(f.declaration["callbacks"][0]))
    elif case == "shared-workflow": f.declaration["purpose"] = "shared_workflow"
    else:
        f.path.write_bytes(f.path.read_bytes()+b" ")
    with pytest.raises((CustodyError, ValueError)):
        if case != "changed-plan": f.select()
        f.callbacks.dynamic_catalog(f.deployment)


def test_reusable_profile_waits_for_exact_identity_customizer(tmp_path):
    f = ClassFixture(tmp_path)
    draft = f.draft("opaque-native-new-member", "reusable-reader")
    with pytest.raises(CustodyError): f.callbacks.helper_agent(draft, f.by_profile)
    f.observe()
    assert f.callbacks.helper_agent(draft, f.by_profile) is None
    for row in f.members:
        draft.labels["agent_identity"] = draft.labels["meerkat_id"] = row["member"]["member"]
        with pytest.raises(CustodyError): f.callbacks.helper_agent(draft, f.by_profile)


@pytest.mark.parametrize("ordinary", [
    {"identity": "reader:new", "profile": "reusable-reader"},
    {"identity": "reader:main", "profile": "reader-source"},
])
def test_identity_projection_changes_only_the_exact_observed_helper_profile(tmp_path, ordinary):
    f = ClassFixture(tmp_path)
    f.observe()
    original = {"identity": "helper:setup", "profile": "helper-source", "labels": {"role": "helper"}}
    projected = f.callbacks.project_identity_spec(original)
    assert projected == {**original, "profile": f.profile}
    assert original["profile"] == "helper-source"
    assert f.callbacks.project_identity_spec(ordinary) == ordinary
    with pytest.raises(CustodyError):
        f.callbacks.project_identity_spec({"identity": "reader:new", "profile": "helper-source"})


def test_actual_callback_surface_must_equal_declared_class(tmp_path):
    f = ClassFixture(tmp_path)
    tool = {"name": "read_record", "description": "Read this record",
        "schema": deepcopy(f.declaration["callbacks"][0]["input_schema"]), "handler": lambda x: x}
    agent = {"identity": "reader:new", "classId": "reader", "profile": "reusable-reader"}
    f.callbacks.require_member_callbacks(agent, [tool])
    for changed in [[], [tool, tool], [{**tool, "description": "Different"}], [{**tool, "schema": {}}]]:
        with pytest.raises(CustodyError): f.callbacks.require_member_callbacks(agent, changed)
    with pytest.raises(CustodyError):
        f.callbacks.require_member_callbacks({**agent, "classId": "helper"}, [tool])


def startup_fixture(tmp_path, monkeypatch, mode, *, declarations=None, helperless=False):
    from test_application_startup import Fixture as Fresh, pin
    from test_application_restart import RestartFixture
    from launchkit.connect_application_startup import FreshApplicationStartup
    f = (Fresh if mode == "fresh" else RestartFixture)(tmp_path, monkeypatch)
    value = json.loads(f.plan.read())
    value["application"]["definition"]["definition"]["profiles"]["reusable-reader"] = {
        "model": "selected-model", "tools": {"comms": True}}
    value["application"]["member_classes"] = declarations if declarations is not None else [{"id": "reader", "profile": "reusable-reader",
        "factory": "application:reader", "callbacks": [], "grants": []}]
    if helperless:
        del value["application"]["helper"]
        del value["application"]["helper_profile"]
    f.plan = pin(f.plan.path, json.dumps(value).encode())
    if mode == "restored":
        setup = json.loads(f.setup.read())
        setup["native_owner"] = {"native_host": {
            "member_classes": deepcopy(value["application"]["member_classes"])}}
        f.setup = pin(f.setup.path, json.dumps(setup).encode())
        manifest = json.loads(f.manifest.read())
        for artifact in manifest["artifacts"]:
            if artifact["path"] == f.setup.path.name:
                artifact["sha256"] = f.setup.sha256
        f.manifest = pin(f.manifest.path, json.dumps(manifest).encode())
        f.selection["policy_manifest"] = f.manifest.pin()
        f.selection_file = pin(f.selection_file.path, json.dumps(f.selection).encode())
        f.owner = f.make_owner()
    else:
        f.owner = FreshApplicationStartup(project_lease=f.lease, accepted_source=f.source,
            gateway=f.gateway, preparation_plan=f.plan, source_policy=f.policy,
            activation_template=f.template, command=lambda *a, **kw: pytest.fail("unexpected command"),
            command_timeout=5, startup_timeout=60, provision_attempt_path=f.root / "provision.json",
            working_directory=f.workspace, bundle_path=f.root / "bundle", activation_path=f.root / "activation.json")
    f.app.dynamic = True
    f.app.deployment = {"classes": [{"id": "reader"}], "applicationProfiles": {"reader": "reusable-reader"}}
    return f


@pytest.mark.parametrize("mode", ["fresh", "restored"])
@pytest.mark.asyncio
async def test_shared_workflow_only_declarations_keep_the_typed_refusal(tmp_path, monkeypatch, mode):
    declarations = [{"id": "workflow", "purpose": "shared_workflow", "profile": "native-workflow-profile",
        "factory": "native-workflow-factory", "callbacks": [], "grants": []}]
    f = startup_fixture(tmp_path, monkeypatch, mode, declarations=declarations)
    with pytest.raises(CustodyError) as refused:
        await f.owner.prepare(f.app)
    assert str(refused.value) == DYNAMIC_ROSTER_UNSUPPORTED
    assert f.events == []


@pytest.mark.parametrize("mode", ["fresh", "restored"])
@pytest.mark.asyncio
async def test_selected_startup_retains_actual_dynamic_providers_and_customizer(tmp_path, monkeypatch, mode):
    f = startup_fixture(tmp_path, monkeypatch, mode)
    await f.owner.prepare(f.app)
    assert getattr(f.app.context, "_begin_member_class_add", None) is None
    config = f.owner.builder_config(roster_provider=f.app, topology_provider=f.app,
        agent_customizer=f.app, persistent_state=str(f.root / ".mobkit/state"))
    runtime = await f.owner.connect(config, f.app)
    assert runtime is f.owner.runtime is f.app.context.runtime
    assert config.roster_provider is config.topology_provider is config.agent_customizer is f.app
    assert f.app._application_callbacks is f.owner.callbacks
    assert set(f.owner.callbacks.member_classes) == {"reader"}
    assert callable(f.app.context._begin_member_class_add)
    await f.owner.shutdown(runtime)
    with pytest.raises(CustodyError):
        await f.app.context._begin_member_class_add(admission_id="after-shutdown", class_id="reader",
            spec=object(), retain_request=lambda value: None, publish=lambda: None)


@pytest.mark.parametrize("mode", ["fresh", "restored"])
@pytest.mark.asyncio
async def test_selected_startup_refuses_replacement_customizer_before_native_construction(tmp_path, monkeypatch, mode):
    f = startup_fixture(tmp_path, monkeypatch, mode)
    await f.owner.prepare(f.app)
    config = f.owner.builder_config(roster_provider=f.app, topology_provider=f.app,
        agent_customizer=object(), persistent_state=str(f.root / ".mobkit/state"))
    with pytest.raises(CustodyError): await f.owner.connect(config, f.app)
    assert f.events == []


@pytest.mark.parametrize("mode", ["fresh", "restored"])
@pytest.mark.asyncio
async def test_selected_startup_refuses_foreign_application_class_profile(tmp_path, monkeypatch, mode):
    f = startup_fixture(tmp_path, monkeypatch, mode)
    f.app.deployment["applicationProfiles"]["reader"] = "reader-source"
    with pytest.raises(CustodyError): await f.owner.prepare(f.app)
    assert f.events == []


async def generated_dynamic(tmp_path, monkeypatch, *, durable=False, helperless=False):
    pytest.importorskip("meerkat_meta.agent_builder")
    from test_profile_only_classes import application, profile_plan
    from launchkit.blueprint import Blueprint
    from launchkit.compiler import generate_files
    files = generate_files(Blueprint.model_validate(profile_plan()))
    if durable:
        from pathlib import Path
        from test_application_extension import load_host, write_application
        host = load_host(monkeypatch, tmp_path)
        deployment = json.loads(files["deployment.json"])
        if not (tmp_path / "application").exists():
            write_application(tmp_path, (Path(__file__).parent / "fixtures/dynamic_member_application.py").read_text())
        app = host.Application(host.ApplicationContext(tmp_path, deployment, None))
        await app.load()
    else:
        host, app, deployment = await application(tmp_path, monkeypatch, callbacks=True)
    seed = app._snapshot.specs[0]
    from launchkit.connect_application_callbacks import _source_definition
    source = {"format": "toml", "text": files["config/mob.toml"]}
    definition = _source_definition(source)
    binding = {"mob_id": definition["id"], "role": seed.profile, "member": "opaque-seed-member"}
    declaration = {"id": "resident", "profile": deployment["applicationProfiles"]["resident"],
        "factory": "application:resident", "callback_account": {"kind": "declared_none"},
        "callbacks": [{"name": "read_resident", "description": "Read the actual owner's record.",
            "input_schema": {"type": "object", "properties": {}}}],
        "grants": [{"tool_name": "read_resident", "action": "invoke", "consequence": "r0"}]}
    value = {"application": {"schema": 4, "definition": source,
        "roster": [seed.to_dict()], "members": [{"member": binding}],
        "helper": {"member": binding}, "helper_profile": "dedicated", "member_classes": [declaration]}}
    if helperless:
        del value["application"]["helper"]
        del value["application"]["helper_profile"]
    path = tmp_path / "selected-plan.json"
    path.write_bytes(json.dumps(value).encode()); path.chmod(0o600)
    selected = SelectedApplicationCallbacks(ExactFile(path, hashlib.sha256(path.read_bytes()).hexdigest()))
    observed = deepcopy(definition)
    projected = deepcopy(definition["profiles"][seed.profile])
    projected["tools"].setdefault("mcp_servers", []).append({"name": "toolkit-setup", "command": "inert"})
    projected["tools"].setdefault("rust_bundles", []).append("meerkat_toolkit.setup_results")
    if not helperless:
        observed["profiles"]["observed-native-helper"] = projected
    context = SimpleNamespace(mob_definition=observed)
    app._application_callbacks = selected
    app._native_identity_models = True
    if not durable:
        app.integration.pending = []
        app.integration.admission_roster = lambda: {"listed": app.integration.rows, "admitting": app.integration.pending}
    await app._prepare_snapshot()
    await app.roster(context)
    return host, app, context


@pytest.mark.parametrize("helperless", [False, True])
@pytest.mark.asyncio
async def test_generated_zero_class_builds_only_exact_admitting_identity_with_declared_callback(tmp_path, monkeypatch, helperless):
    CallbackDispatcher = pytest.importorskip("meerkat_meta.agent_builder").CallbackDispatcher
    _, app, context = await generated_dynamic(tmp_path, monkeypatch, helperless=helperless)
    assert set(app._tool_catalog["resident"]) == {"read_resident"}
    assert [row.identity for row in await app.roster(context)] == ["lead:main"]
    app.integration.pending.append({"identity": "resident:alice", "classId": "resident", "context": {"owner": "alice"}})
    await app._prepare_snapshot()
    assert [row.identity for row in await app.roster(context)] == ["lead:main"]
    adopted = next(row for row in app._snapshot.buildable_specs if row.identity == "resident:alice")
    dispatcher = CallbackDispatcher()
    dispatcher.register_agent_customizer(app)
    result = await dispatcher.handle_callback("callback/agent_customizer/customize_build", {
        "scope_id": "held-alice", "context": {"identity": adopted.identity}, "spec": adopted.to_dict(), "draft": {}})
    assert [row["name"] for row in result["external_tools"]] == ["read_resident"]
    read = await dispatcher.handle_callback("callback/call_tool", {"scope_id": "held-alice",
        "tool": "read_resident", "arguments": {}})
    assert read["content"]["identity"] == "resident:alice"
    assert read["content"]["context"] == {"owner": "alice"}
    assert app.context.runtime is None


@pytest.mark.asyncio
async def test_generated_helper_accepts_only_native_observed_profile_projection(tmp_path, monkeypatch):
    CallbackDispatcher = pytest.importorskip("meerkat_meta.agent_builder").CallbackDispatcher
    _, app, _ = await generated_dynamic(tmp_path, monkeypatch)
    selected = app._snapshot.specs[0].to_dict()
    selected["profile"] = "observed-native-helper"
    dispatcher = CallbackDispatcher()
    dispatcher.register_agent_customizer(app)
    result = await dispatcher.handle_callback("callback/agent_customizer/customize_build", {
        "scope_id": "helper", "context": {"identity": selected["identity"]}, "spec": selected, "draft": {}})
    assert result.get("external_tools", []) == []
    selected["display_name"] = "Foreign helper metadata"
    with pytest.raises(ValueError, match="validated desired roster"):
        await dispatcher.handle_callback("callback/agent_customizer/customize_build", {
            "scope_id": "foreign-helper", "context": {"identity": selected["identity"]}, "spec": selected, "draft": {}})


@pytest.mark.asyncio
async def test_generated_catalog_does_not_widen_or_publish_wrong_class_implementation(tmp_path, monkeypatch):
    _, app, _ = await generated_dynamic(tmp_path, monkeypatch)
    app.integration.pending.append({"identity": "resident:alice", "classId": "resident"})
    original = app.integration.tools_for_agent
    def changed(agent):
        return [{**row, "description": "Different operation"} for row in original(agent)]
    app.integration.tools_for_agent = changed
    before = app._snapshot
    with pytest.raises(CustodyError, match="declaration"):
        await app._prepare_snapshot()
    assert app._snapshot is before
    assert "resident:alice" not in app._identity_classes


@pytest.mark.parametrize("mode", ["fresh", "restored"])
@pytest.mark.asyncio
async def test_dynamic_mode_cannot_change_after_preparation_to_bypass_customizer_check(tmp_path, monkeypatch, mode):
    f = startup_fixture(tmp_path, monkeypatch, mode)
    await f.owner.prepare(f.app)
    f.app.dynamic = False
    config = f.owner.builder_config(roster_provider=f.app, topology_provider=f.app,
        agent_customizer=None, persistent_state=str(f.root / ".mobkit/state"))
    with pytest.raises(CustodyError): await f.owner.connect(config, f.app)
    assert f.events == []


def test_shared_workflow_declaration_stays_with_its_native_owner(tmp_path):
    f = ClassFixture(tmp_path)
    f.value["application"]["member_classes"].append({"id": "workflow", "purpose": "shared_workflow",
        "profile": "native-workflow-profile", "factory": "native-workflow-factory", "callbacks": [], "grants": []})
    f.select()
    assert set(f.callbacks.dynamic_catalog(f.deployment)) == {"reader"}
    f.observe()
    with pytest.raises(CustodyError):
        f.callbacks.helper_agent(f.draft("opaque-workflow-member", "native-workflow-profile"), f.by_profile)
