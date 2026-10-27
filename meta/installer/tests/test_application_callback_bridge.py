"""Generated application callbacks exercise the shipping one-dict SDK bridge."""
from __future__ import annotations

import ast
import asyncio
import copy
import importlib.util
import json
from pathlib import Path

import pytest

from launchkit.blueprint import starter_blueprint
from launchkit.compiler import generate_files
from meerkat_mobkit.agent_builder import CallbackDispatcher


def load_package(tmp_path, monkeypatch, application_source):
    files = generate_files(starter_blueprint())
    for name, contents in files.items():
        destination = tmp_path / name
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text(contents)
    directory = tmp_path / "application"
    directory.mkdir()
    (directory / "__init__.py").write_text(application_source)
    monkeypatch.syspath_prepend(str(tmp_path))
    definition = importlib.util.spec_from_file_location("callback_bridge_host", tmp_path / "app.py")
    host = importlib.util.module_from_spec(definition)
    definition.loader.exec_module(host)
    context = host.ApplicationContext(tmp_path, json.loads(files["deployment.json"]), asyncio.Event())
    return host, host.Application(context), files


def fixed_session_builder(host, application):
    # Execute the actual generated registration class without starting run(),
    # storage, adapters, signals, gateways or native processes.
    tree = ast.parse(Path(host.__file__).read_text())
    run = next(node for node in tree.body if isinstance(node, ast.AsyncFunctionDef) and node.name == "run")
    builder = next(node for node in run.body if isinstance(node, ast.ClassDef) and node.name == "SessionBuilder")
    module = ast.fix_missing_locations(ast.Module(body=[copy.deepcopy(builder)], type_ignores=[]))
    namespace = {
        "application": application,
        "spec": application.deployment,
        "by_profile": {agent["profile"]: agent for agent in application.deployment["agents"]},
        "configured_tools": application.tools,
    }
    exec(compile(module, str(host.__file__), "exec"), namespace)
    return namespace["SessionBuilder"]()


APPLICATION = '''
class Integration:
    def __init__(self, context):
        self.context = context
        self.calls = []

    def tools_for_agent(self, agent):
        if agent['classId'] != 'lead':
            return []
        identity = agent['identity']
        async def read_case_work(operation_id=None):
            self.calls.append(('read', identity, operation_id))
            return {'identity': identity, 'operation_id': operation_id}
        def save_case_findings(operation_id, text):
            self.calls.append(('save', identity, operation_id, text))
            return {'saved': text, 'operation_id': operation_id}
        async def publish_worksheet(operation_id, hypotheses, uncertainty, hazards, next_steps):
            self.calls.append(('publish', identity, operation_id))
            return {'operation_id': operation_id, 'hypotheses': hypotheses,
                    'uncertainty': uncertainty, 'hazards': hazards, 'next_steps': next_steps}
        def read_mapping(arguments):
            self.calls.append(('mapping', identity, arguments))
            return arguments
        return [
            {'name': 'read_case_work', 'handler': read_case_work, 'description': 'Read only owned case work', 'schema': {'type': 'object'}},
            {'name': 'save_case_findings', 'handler': save_case_findings, 'description': 'Save findings to owned case work', 'schema': {'type': 'object'}},
            {'name': 'publish_worksheet', 'handler': publish_worksheet, 'description': 'Publish owned worksheet', 'schema': {'type': 'object'}},
            {'name': 'read_mapping', 'handler': read_mapping, 'description': 'Read a JSON dictionary', 'schema': {'type': 'object'}},
        ]

def build(context):
    return Integration(context)
'''


async def call(dispatcher, scope, name, arguments):
    # CallbackDispatcher 0.8.42 itself supplies exactly handler(arguments).
    return await dispatcher.handle_callback("callback/call_tool", {
        "scope_id": scope, "tool": name, "arguments": arguments})


async def test_generated_fixed_tools_handle_keyword_fields_and_legacy_dictionary_through_exact_sdk(tmp_path, monkeypatch):
    host, application, files = load_package(tmp_path, monkeypatch, APPLICATION)
    await application.load()
    assert application.context.runtime is None
    dispatcher = CallbackDispatcher()
    dispatcher.register_builder(fixed_session_builder(host, application))
    lead = next(agent for agent in application.deployment["agents"] if agent["classId"] == "lead")
    other = next(agent for agent in application.deployment["agents"] if agent["classId"] != "lead")
    for scope, profile in (("lead-scope", lead["profile"]), ("other-scope", other["profile"])):
        await dispatcher.handle_callback("callback/build_agent", {"options": {
            "scope_id": scope, "profile_name": profile, "resume_session_id": "saved-session"}})
    assert (await call(dispatcher, "lead-scope", "read_case_work", {}))["content"] == {
        "identity": lead["identity"], "operation_id": None}
    assert (await call(dispatcher, "lead-scope", "read_case_work", {"operation_id": "case-one"}))["content"]["operation_id"] == "case-one"
    assert (await call(dispatcher, "lead-scope", "save_case_findings", {
        "operation_id": "case-one", "text": "An observed finding"}))["content"] == {
            "operation_id": "case-one", "saved": "An observed finding"}
    worksheet = {"operation_id": "case-one", "hypotheses": ["synthetic"], "uncertainty": "high",
                 "hazards": [], "next_steps": ["inspect"]}
    assert (await call(dispatcher, "lead-scope", "publish_worksheet", worksheet))["content"] == worksheet
    mapping = {"selected": ["retained"]}
    assert (await call(dispatcher, "lead-scope", "read_mapping", mapping))["content"] == mapping
    with pytest.raises(ValueError, match="no handler"):
        await call(dispatcher, "other-scope", "read_case_work", {})
    assert len(application.integration.calls) == 5
    assert "keyword binding first" in files["README.md"]


async def test_generated_dynamic_customizer_preserves_identity_scope_on_create_and_resume(tmp_path, monkeypatch):
    source = APPLICATION.replace("    def tools_for_agent(self, agent):", '''    def desired_roster(self):
        rows = [{'identity': agent['identity'], 'classId': agent['classId']} for agent in self.context.deployment['agents']]
        return rows + [{'identity': 'lead:extra', 'classId': 'lead'}]

    def desired_edges(self, target_identities):
        return []

    def tools_for_agent(self, agent):''')
    _, application, _ = load_package(tmp_path, monkeypatch, source)
    await application.load()
    dispatcher = CallbackDispatcher()
    dispatcher.register_agent_customizer(application)
    rows = await application.roster(None)
    lead = next(row for row in rows if row.identity == "lead:main")
    extra = next(row for row in rows if row.identity == "lead:extra")
    async def bind(row, scope):
        await dispatcher.handle_callback("callback/agent_customizer/customize_build", {
            "scope_id": scope, "context": {"identity": row.identity}, "spec": row.to_dict(), "draft": {}})
    await bind(lead, "lead-created")
    await bind(extra, "extra-created")
    assert (await call(dispatcher, "lead-created", "read_case_work", {"operation_id": "case-a"}))["content"] == {"identity": lead.identity, "operation_id": "case-a"}
    assert (await call(dispatcher, "extra-created", "read_case_work", {"operation_id": "case-b"}))["content"] == {"identity": extra.identity, "operation_id": "case-b"}
    await bind(lead, "lead-resumed")
    with pytest.raises(ValueError, match="no handler"):
        await call(dispatcher, "lead-created", "read_case_work", {})
    assert (await call(dispatcher, "lead-resumed", "read_case_work", {}))["content"]["identity"] == lead.identity
    assert (await call(dispatcher, "extra-created", "read_case_work", {}))["content"]["identity"] == extra.identity


async def test_callback_exception_after_side_effect_is_never_reinvoked_in_another_form(tmp_path, monkeypatch):
    host, _, _ = load_package(tmp_path, monkeypatch, APPLICATION)
    calls = []
    def sync_failure(operation_id=None):
        calls.append(operation_id)
        raise TypeError("Callback failed after its own mutation")
    async def async_failure(operation_id=None):
        calls.append(operation_id)
        await asyncio.sleep(0)
        raise TypeError("Callback failed after its own mutation")
    for handler in (sync_failure, async_failure):
        wrapped = host.application_callback(handler)
        with pytest.raises(TypeError, match="after its own mutation"):
            await wrapped({"operation_id": "case-one"})
    assert calls == ["case-one", "case-one"]


async def test_binding_failure_does_not_invoke_handler_or_echo_argument_values(tmp_path, monkeypatch):
    host, _, _ = load_package(tmp_path, monkeypatch, APPLICATION)
    calls = []
    def requires_two(operation_id, text):
        calls.append((operation_id, text))
    wrapped = host.application_callback(requires_two)
    with pytest.raises(TypeError, match="do not match") as error:
        await wrapped({"unexpected-private-name": "private-value"})
    assert "private" not in str(error.value)
    assert calls == []
    with pytest.raises(ValueError, match="JSON argument object"):
        await wrapped(["case-one", "text"])
    assert calls == []


async def test_bridge_awaits_non_coroutine_awaitables_and_keeps_noninspectable_dictionary_convention(tmp_path, monkeypatch):
    host, _, _ = load_package(tmp_path, monkeypatch, APPLICATION)
    def awaitable_result(operation_id):
        result = asyncio.get_running_loop().create_future()
        result.set_result({"operation_id": operation_id})
        return result
    assert await host.application_callback(awaitable_result)({"operation_id": "case-future"}) == {
        "operation_id": "case-future"}
    # dict's constructor has no inspectable signature on this pinned Python;
    # its existing SDK dictionary invocation still returns the same data.
    assert await host.application_callback(dict)({"value": "same"}) == {"value": "same"}


async def test_new_package_offline_rehearsal_uses_same_argument_bridge_without_starting_a_process(tmp_path, monkeypatch):
    import inspect
    import runpy
    import sys
    from launchkit.application_trial import _HARNESS

    host, application, _ = load_package(tmp_path, monkeypatch, APPLICATION + '''

def rehearse(context, fixtures):
    return Integration(context)
''')
    # Use actual worker start() and value() code, excluding its stdio/process
    # entrypoint. Every source and state path is this synthetic package fixture.
    tree = ast.parse(_HARNESS)
    functions = [copy.deepcopy(node) for node in tree.body
                 if isinstance(node, ast.AsyncFunctionDef) and node.name in {"value", "start", "native_policy"}]
    scratch = tmp_path / "scratch"
    scratch.mkdir()
    namespace = {"asyncio": asyncio, "inspect": inspect, "runpy": runpy, "sys": sys,
                 "root": tmp_path, "scratch": scratch, "deployment": application.deployment,
                 "production": None, "integration": None, "handlers": {}, "stop_event": None}
    code = ast.fix_missing_locations(ast.Module(body=functions, type_ignores=[]))
    exec(compile(code, "application-trial-worker-start", "exec"), namespace)
    result = await namespace["start"]({})
    lead = next(agent for agent in application.deployment["agents"] if agent["classId"] == "lead")
    other = next(agent for agent in application.deployment["agents"] if agent["classId"] != "lead")
    assert result["unavailable"] is None and result["dependencies"] == []
    callbacks = namespace["handlers"]
    assert (other["identity"], "read_case_work") not in callbacks
    assert await callbacks[(lead["identity"], "read_case_work")]({"operation_id": "offline-case"}) == {
        "identity": lead["identity"], "operation_id": "offline-case"}
    assert await callbacks[(lead["identity"], "read_mapping")]({"retained": "mapping"}) == {"retained": "mapping"}
    assert namespace["production"].context.runtime is None
