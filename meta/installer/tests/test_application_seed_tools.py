"""Shipping SDK callbacks for fixed seeds before the identity customizer exists."""
from __future__ import annotations

import ast
import asyncio
import copy
import importlib.metadata
import importlib.util
import json
from pathlib import Path

import pytest

from launchkit.blueprint import starter_blueprint
from launchkit.compiler import generate_files
from meerkat_mobkit.agent_builder import CallbackDispatcher


APPLICATION = '''
import copy

class Integration:
    def __init__(self, context):
        self.context = context
        self.rows = [{'identity': row['identity'], 'classId': row['classId'],
                      'context': {'record': row['identity'], 'version': 1}}
                     for row in context.deployment['agents']]
        self.name = 'read_case_work'
        self.schema = {'type': 'object', 'properties': {'operation_id': {'type': 'string'}}}
        self.duplicate = False
        self.started = False

    def desired_roster(self):
        return self.rows

    def desired_edges(self, identities):
        return []

    def tools_for_agent(self, agent):
        owned = copy.deepcopy(agent)
        async def read(operation_id=None):
            return {'identity': owned['identity'], 'context': owned.get('context'),
                    'classId': owned['classId'], 'tools': owned['tools'],
                    'operation_id': operation_id}
        rows = [{'name': self.name, 'handler': read, 'description': 'Read owned case work',
                 'schema': copy.deepcopy(self.schema)}]
        if self.duplicate:
            rows += copy.copy(rows)
        return rows

def build(context):
    return Integration(context)
'''


async def package(tmp_path, monkeypatch, *, application_source=APPLICATION):
    # The compiler's actual exported app.py owns registration; no stub replaces it.
    files = generate_files(starter_blueprint())
    for name, contents in files.items():
        destination = tmp_path / name
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text(contents)
    directory = tmp_path / "application"
    directory.mkdir()
    (directory / "__init__.py").write_text(application_source)
    monkeypatch.syspath_prepend(str(tmp_path))
    definition = importlib.util.spec_from_file_location("seed_callback_host", tmp_path / "app.py")
    host = importlib.util.module_from_spec(definition)
    definition.loader.exec_module(host)
    context = host.ApplicationContext(tmp_path, json.loads(files["deployment.json"]), asyncio.Event())
    application = host.Application(context)
    await application.load()
    connector = host.ConnectorTools(tmp_path, {"connectors": [{"id": "files"}]})
    application._native_tools = lambda row: host.local_tools(connector, row)
    await application._prepare_snapshot()
    tree = ast.parse(Path(host.__file__).read_text())
    run = next(node for node in tree.body if isinstance(node, ast.AsyncFunctionDef) and node.name == "run")
    builder = next(node for node in run.body if isinstance(node, ast.ClassDef) and node.name == "SessionBuilder")
    namespace = {"application": application, "spec": application.deployment,
                 "HOST_TOOL_POLICY": host.HOST_TOOL_POLICY,
                 "by_profile": {row["profile"]: row for row in application.deployment["agents"]},
                 "configured_tools": {row["profile"]: host.local_tools(connector, row)
                                      + application.tools[row["profile"]]
                                      for row in application.deployment["agents"]}}
    module = ast.fix_missing_locations(ast.Module(body=[copy.deepcopy(builder)], type_ignores=[]))
    exec(compile(module, str(host.__file__), "exec"), namespace)
    dispatcher = CallbackDispatcher()
    dispatcher.register_builder(namespace["SessionBuilder"]())
    return host, application, dispatcher


async def build(dispatcher, profile, scope, *, resume=None):
    return await dispatcher.handle_callback("callback/build_agent", {"options": {
        "scope_id": scope, "profile_name": profile, "resume_session_id": resume}})


async def call(dispatcher, scope, name, arguments=None):
    return await dispatcher.handle_callback("callback/call_tool", {
        "scope_id": scope, "tool": name, "arguments": arguments or {}})


async def test_dynamic_fixed_seeds_have_exact_tools_before_any_customizer_callback(tmp_path, monkeypatch):
    assert importlib.metadata.version("meerkat-mobkit") == "0.8.42"
    _, application, dispatcher = await package(tmp_path, monkeypatch)
    assert application.dynamic and application.context.runtime is None
    # No agent_customizer is registered or invoked on this bootstrap path.
    for index, row in enumerate(application.deployment["agents"]):
        scope = f"bootstrap-{index}"
        rendered = await build(dispatcher, row["profile"], scope)
        names = [tool["name"] for tool in rendered.get("tools", [])]
        expected = (["files_list", "files_read"] if row["tools"]["workspace"] else [])
        expected += ["notes_search"]
        if row["tools"]["notes"] == "write":
            expected.insert(-1, "notes_save")
        expected += ["read_case_work"]
        assert names == expected and len(names) == len(set(names))
        declared = next(tool for tool in rendered["tools"] if tool["name"] == "read_case_work")
        assert declared["input_schema"] == application.integration.schema
        result = (await call(dispatcher, scope, "read_case_work", {"operation_id": "job-one"}))["content"]
        assert result == {"identity": row["identity"], "context": {"record": row["identity"], "version": 1},
                          "classId": row["classId"], "tools": row["tools"], "operation_id": "job-one"}
        if row["tools"]["notes"] == "read":
            with pytest.raises(ValueError, match="no handler"):
                await call(dispatcher, scope, "notes_save", {"content": "not granted"})
        if not row["tools"]["workspace"]:
            with pytest.raises(ValueError, match="no handler"):
                await call(dispatcher, scope, "files_read", {"path": "not granted"})
    # Two seeds with the same class have separate unique profile-bound closures.
    first = (await call(dispatcher, "bootstrap-1", "read_case_work"))["content"]
    second = (await call(dispatcher, "bootstrap-2", "read_case_work"))["content"]
    assert first["identity"] == "research:1" and second["identity"] == "research:2"


async def test_dynamic_seed_resume_uses_refreshed_validated_context_and_original_grants(tmp_path, monkeypatch):
    _, application, dispatcher = await package(tmp_path, monkeypatch)
    seed = application.deployment["agents"][0]
    await build(dispatcher, seed["profile"], "seed-original")
    row = next(row for row in application.integration.rows if row["identity"] == seed["identity"])
    row["context"] = {"record": "changed-in-this-process", "version": 2}
    await application._prepare_snapshot()
    await build(dispatcher, seed["profile"], "seed-resume", resume="durable-session")
    result = (await call(dispatcher, "seed-resume", "read_case_work"))["content"]
    assert result["identity"] == seed["identity"]
    assert result["context"] == {"record": "changed-in-this-process", "version": 2}
    assert result["tools"] == seed["tools"]
    assert (await call(dispatcher, "seed-original", "read_case_work"))["content"]["context"]["version"] == 1


async def test_seed_customizer_and_following_session_builder_agree_on_current_identity_callbacks(tmp_path, monkeypatch):
    _, application, dispatcher = await package(tmp_path, monkeypatch)
    seed = application.deployment["agents"][1]
    row = next(row for row in application.integration.rows if row["identity"] == seed["identity"])
    row["context"] = {"record": "adopted-current-context", "version": 2}
    await application._prepare_snapshot()
    dispatcher.register_agent_customizer(application)
    adopted = next(row for row in await application.roster(None) if row.identity == seed["identity"])
    customized = await dispatcher.handle_callback("callback/agent_customizer/customize_build", {
        "scope_id": "customize-seed", "context": {"identity": adopted.identity},
        "spec": adopted.to_dict(), "draft": {}})
    # The published gateway uses distinct scopes and SessionBuilder wins tool
    # name collisions. Both stages must therefore advertise the same catalog
    # and retain the same current seed context, not a cached initial closure.
    session = await build(dispatcher, seed["profile"], "session-seed", resume="durable-session")
    assert customized["external_tools"] == session["tools"]
    current = (await call(dispatcher, "customize-seed", "read_case_work"))["content"]
    assert current["identity"] == seed["identity"]
    assert current["context"] == {"record": "adopted-current-context", "version": 2}
    assert (await call(dispatcher, "session-seed", "read_case_work"))["content"] == current


async def test_reusable_profiles_wait_for_actual_identity_and_customizer_restores_exact_scope(tmp_path, monkeypatch):
    _, application, dispatcher = await package(tmp_path, monkeypatch)
    for index, profile in enumerate(application.deployment["applicationProfiles"].values()):
        scope = f"unbound-reusable-{index}"
        assert (await build(dispatcher, profile, scope)).get("tools", []) == []
        with pytest.raises(ValueError, match="no handler"):
            await call(dispatcher, scope, "read_case_work")
    with pytest.raises(ValueError, match="unknown profile"):
        await build(dispatcher, "invented-profile", "unknown")
    application.integration.rows += [{"identity": f"lead:{key}", "classId": "lead",
                                      "context": {"record": key}} for key in ("one", "two")]
    await application._prepare_snapshot()
    dispatcher.register_agent_customizer(application)
    async def customize(identity, scope):
        spec = next(row for row in await application.roster(None) if row.identity == identity)
        rendered = await dispatcher.handle_callback("callback/agent_customizer/customize_build", {
            "scope_id": scope, "context": {"identity": identity}, "spec": spec.to_dict(), "draft": {}})
        names = [tool["name"] for tool in rendered["external_tools"]]
        assert names == ["notes_save", "notes_search", "read_case_work"]
        assert len(names) == len(set(names))
    await customize("lead:one", "customize-one")
    await customize("lead:two", "customize-two")
    one = (await call(dispatcher, "customize-one", "read_case_work"))["content"]
    two = (await call(dispatcher, "customize-two", "read_case_work"))["content"]
    assert one["identity"] == "lead:one" and one["context"] == {"record": "one"}
    assert two["identity"] == "lead:two" and two["context"] == {"record": "two"}
    await customize("lead:one", "customize-one-restored")
    with pytest.raises(ValueError, match="no handler"):
        await call(dispatcher, "customize-one", "read_case_work")
    assert (await call(dispatcher, "customize-one-restored", "read_case_work"))["content"] == one
    assert (await call(dispatcher, "customize-two", "read_case_work"))["content"] == two


@pytest.mark.parametrize("fault", ["schema", "name", "duplicate", "collision"])
async def test_dynamic_seed_registration_rechecks_catalog_and_collisions(tmp_path, monkeypatch, fault):
    _, application, dispatcher = await package(tmp_path, monkeypatch)
    seed = application.deployment["agents"][0]
    if fault == "schema":
        application.integration.schema["properties"]["new_field"] = {"type": "string"}
    elif fault == "name":
        application.integration.name = "unadmitted_tool"
    elif fault == "duplicate":
        application.integration.duplicate = True
    else:
        application.integration.name = "notes_save"
        # A colliding name is already in the frozen class catalog for this test;
        # registration must still refuse to shadow a granted local callback.
        application._tool_catalog[seed["classId"]]["notes_save"] = application._json_signature(
            application.integration.schema, "Application callback schema")
    with pytest.raises(ValueError, match="catalog|Duplicate application tool|collides"):
        await build(dispatcher, seed["profile"], "rejected-seed")
    with pytest.raises(ValueError, match="no handler"):
        await call(dispatcher, "rejected-seed", "read_case_work")


async def test_dynamic_seed_registration_rejects_snapshot_changed_during_async_hook(tmp_path, monkeypatch):
    _, application, dispatcher = await package(tmp_path, monkeypatch)
    seed = application.deployment["agents"][0]
    entered, release = asyncio.Event(), asyncio.Event()
    original = application.integration.tools_for_agent
    async def delayed(row):
        entered.set()
        await release.wait()
        return original(row)
    application.integration.tools_for_agent = delayed
    pending = asyncio.create_task(build(dispatcher, seed["profile"], "stale-seed"))
    await asyncio.wait_for(entered.wait(), 30)
    # A new validated snapshot was published by another operation during the hook.
    application._snapshot = copy.deepcopy(application._snapshot)
    release.set()
    with pytest.raises(ValueError, match="roster changed"):
        await pending
    with pytest.raises(ValueError, match="no handler"):
        await call(dispatcher, "stale-seed", "read_case_work")


async def test_host_selection_omits_global_callbacks_before_fixed_seed_customization(tmp_path, monkeypatch):
    _, application, dispatcher = await package(tmp_path, monkeypatch)
    selections = []
    def select(agent, available_names):
        selections.append((agent["identity"], tuple(available_names), copy.deepcopy(agent.get("context"))))
        return []
    application.integration.select_host_tools = select
    await application._prepare_snapshot()
    for index, row in enumerate(application.deployment["agents"]):
        scope = f"selected-seed-{index}"
        rendered = await build(dispatcher, row["profile"], scope)
        assert [tool["name"] for tool in rendered.get("tools", [])] == ["read_case_work"]
        for name in ("notes_save", "notes_search", "files_list", "files_read"):
            with pytest.raises(ValueError, match="no handler"):
                await call(dispatcher, scope, name)
    first = (await call(dispatcher, "selected-seed-1", "read_case_work"))["content"]
    second = (await call(dispatcher, "selected-seed-2", "read_case_work"))["content"]
    assert first["identity"] == "research:1" and first["context"]["record"] == "research:1"
    assert second["identity"] == "research:2" and second["context"]["record"] == "research:2"
    assert any(identity == "research:1" and "files_read" in names for identity, names, _ in selections)


async def test_host_selection_two_contexts_keep_scoped_callbacks_on_create_and_restore(tmp_path, monkeypatch):
    _, application, dispatcher = await package(tmp_path, monkeypatch)
    application.integration.select_host_tools = lambda agent, names: []
    application.integration.rows += [{"identity": f"lead:{key}", "classId": "lead",
                                      "context": {"record": key}} for key in ("one", "two")]
    await application._prepare_snapshot()
    dispatcher.register_agent_customizer(application)
    async def customize(identity, scope):
        spec = next(row for row in await application.roster(None) if row.identity == identity)
        rendered = await dispatcher.handle_callback("callback/agent_customizer/customize_build", {
            "scope_id": scope, "context": {"identity": identity}, "spec": spec.to_dict(), "draft": {}})
        assert [tool["name"] for tool in rendered["external_tools"]] == ["read_case_work"]
        for name in ("notes_save", "notes_search", "files_list", "files_read"):
            with pytest.raises(ValueError, match="no handler"):
                await call(dispatcher, scope, name)
        return (await call(dispatcher, scope, "read_case_work"))["content"]
    one = await customize("lead:one", "selected-one")
    two = await customize("lead:two", "selected-two")
    assert one["context"] == {"record": "one"} and two["context"] == {"record": "two"}
    assert await customize("lead:one", "selected-one-restored") == one
    with pytest.raises(ValueError, match="no handler"):
        await call(dispatcher, "selected-one", "read_case_work")
    assert (await call(dispatcher, "selected-two", "read_case_work"))["content"] == two


@pytest.mark.parametrize("selection", [None, "notes_search", {}, [1], ["notes_search", "notes_search"], ["ungranted_tool"]])
async def test_host_selection_rejects_malformed_or_expanding_before_adoption(tmp_path, monkeypatch, selection):
    _, application, dispatcher = await package(tmp_path, monkeypatch)
    original_snapshot = application._snapshot
    original_catalog = copy.deepcopy(application._tool_catalog)
    application.integration.select_host_tools = lambda agent, names: selection
    with pytest.raises(ValueError, match="select_host_tools"):
        await application._prepare_snapshot()
    assert application._snapshot is original_snapshot and application._tool_catalog == original_catalog
    with pytest.raises(ValueError, match="select_host_tools"):
        await build(dispatcher, application.deployment["agents"][0]["profile"], "invalid-selection")
    with pytest.raises(ValueError, match="no handler"):
        await call(dispatcher, "invalid-selection", "read_case_work")


async def test_host_selection_cannot_add_writer_to_read_only_class(tmp_path, monkeypatch):
    _, application, dispatcher = await package(tmp_path, monkeypatch)
    application.integration.select_host_tools = lambda agent, names: ["notes_save"]
    seed = application.deployment["agents"][1]
    assert seed["tools"]["notes"] == "read"
    with pytest.raises(ValueError, match="select_host_tools"):
        await build(dispatcher, seed["profile"], "not-a-writer")
    with pytest.raises(ValueError, match="no handler"):
        await call(dispatcher, "not-a-writer", "notes_save", {"content": "synthetic private canary"})


async def test_host_selection_error_does_not_fall_back_or_publish(tmp_path, monkeypatch):
    _, application, dispatcher = await package(tmp_path, monkeypatch)
    before = application._snapshot
    calls = []
    def select(agent, names):
        calls.append(agent["identity"])
        raise RuntimeError("synthetic selection unavailable")
    application.integration.select_host_tools = select
    with pytest.raises(RuntimeError, match="synthetic selection unavailable"):
        await application._prepare_snapshot()
    assert application._snapshot is before and calls == [application.deployment["agents"][0]["identity"]]
    with pytest.raises(RuntimeError, match="synthetic selection unavailable"):
        await build(dispatcher, application.deployment["agents"][0]["profile"], "selection-error")
    with pytest.raises(ValueError, match="no handler"):
        await call(dispatcher, "selection-error", "notes_search")


async def test_host_selection_async_snapshot_change_never_registers_stale_callbacks(tmp_path, monkeypatch):
    _, application, dispatcher = await package(tmp_path, monkeypatch)
    entered, release = asyncio.Event(), asyncio.Event()
    async def select(agent, names):
        entered.set()
        await release.wait()
        return []
    application.integration.select_host_tools = select
    pending = asyncio.create_task(build(dispatcher, application.deployment["agents"][0]["profile"], "stale-selection"))
    try:
        await asyncio.wait_for(entered.wait(), 30)
        application._snapshot = copy.deepcopy(application._snapshot)
        release.set()
        with pytest.raises(ValueError, match="roster changed"):
            await pending
    finally:
        release.set()
        await asyncio.gather(pending, return_exceptions=True)
    with pytest.raises(ValueError, match="no handler"):
        await call(dispatcher, "stale-selection", "read_case_work")


async def test_host_selection_partial_subset_preserves_grants_and_native_order(tmp_path, monkeypatch):
    _, application, dispatcher = await package(tmp_path, monkeypatch)
    before = copy.deepcopy(application.deployment)
    def select(agent, names):
        agent["tools"]["workspace"] = True
        agent["tools"]["notes"] = "write"
        return ["notes_search"] if "notes_search" in names else []
    application.integration.select_host_tools = select
    await application._prepare_snapshot()
    seed = application.deployment["agents"][1]
    rendered = await build(dispatcher, seed["profile"], "selected-reader")
    assert [tool["name"] for tool in rendered["tools"]] == ["notes_search", "read_case_work"]
    assert application.deployment == before
    assert (await call(dispatcher, "selected-reader", "notes_search"))["content"] == {"notes": []}
    with pytest.raises(ValueError, match="no handler"):
        await call(dispatcher, "selected-reader", "notes_save", {"content": "not admitted"})


async def test_host_selection_deferred_until_actual_factory_is_bound(tmp_path, monkeypatch):
    source = APPLICATION.replace("    def tools_for_agent(self, agent):", '''    def select_host_tools(self, agent, available_names):
        if not available_names:
            raise RuntimeError("Host catalog was not ready")
        self.context.selected_identities = getattr(self.context, "selected_identities", []) + [agent["identity"]]
        return []

    def tools_for_agent(self, agent):''')
    _, application, dispatcher = await package(tmp_path, monkeypatch, application_source=source)
    assert set(application.context.selected_identities) == {row["identity"] for row in application.deployment["agents"]}
    rendered = await build(dispatcher, application.deployment["agents"][0]["profile"], "deferred-selection")
    assert [tool["name"] for tool in rendered["tools"]] == ["read_case_work"]


async def test_host_selection_malformed_hook_is_rejected_during_load(tmp_path, monkeypatch):
    source = APPLICATION.replace("class Integration:\n", "class Integration:\n    select_host_tools = 7\n")
    with pytest.raises(ValueError, match="select_host_tools must be callable"):
        await package(tmp_path, monkeypatch, application_source=source)
