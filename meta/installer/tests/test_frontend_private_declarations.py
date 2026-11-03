"""Actual app-owner declaration custody, without runtime or server startup."""
from __future__ import annotations

import ast
import asyncio
import copy
import importlib.util
import json
import os
import sys
from pathlib import Path
from types import SimpleNamespace

import pytest

from launchkit.blueprint import starter_blueprint
from launchkit.compiler import TEMPLATES_V2, generate_files

# Qualification may select an isolated exact app.py. Maintained runs use the
# ordinary product template; no artifact-local dependency is required.
APP_SOURCE = Path(os.environ.get("FORGE_PRIVATE_DECLARATIONS_APP", TEMPLATES_V2 / "app.py"))
APPLICATION = '''class Integration:
    def frontend_management(self, request):
        return {"status": "unknown"}
def build(context):
    return Integration()
'''
SETUP_NAMES = ("application/configuration.json", "application/console.json")


def _configuration(token):
    return {"fields": [
        {"key": "CONSOLE_USER_ID", "label": "Console user", "secret": False, "required": True},
        {"key": "CONSOLE_LOGIN_PASSWORD", "label": "Console password", "secret": True, "required": True},
        {"key": token, "label": "Inert credential declaration", "secret": True, "required": False},
        {"key": "PUBLIC_CAPTION", "label": "Caption", "secret": False, "required": False,
         "default": "public-default-is-not-a-private-field-name"},
    ]}


def _package(tmp_path, monkeypatch):
    # Pure source generation copies ordinary templates. No application build,
    # native factory, provider, process, transport or installed action is run.
    files = generate_files(starter_blueprint())
    for name, contents in files.items():
        target = tmp_path / name
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(contents)
    (tmp_path / "app.py").write_bytes(APP_SOURCE.read_bytes())
    directory = tmp_path / "application"
    directory.mkdir()
    (directory / "__init__.py").write_text(APPLICATION)
    (directory / "configuration.json").write_text(json.dumps(_configuration("EARLY_TOKEN")))
    (directory / "console.json").write_text('{"authentication":"local-password"}')
    monkeypatch.syspath_prepend(str(tmp_path))
    for name in ("connectors", "mcp_connections", "application_configuration", "console_configuration",
                 "console_auth", "native_definition_continuity"):
        if name not in sys.modules:
            monkeypatch.setitem(sys.modules, name, None)
        monkeypatch.delitem(sys.modules, name)
    definition = importlib.util.spec_from_file_location("private_declarations_host", tmp_path / "app.py")
    host = importlib.util.module_from_spec(definition)
    definition.loader.exec_module(host)
    context = host.ApplicationContext(tmp_path, json.loads(files["deployment.json"]), asyncio.Event())
    application = host.Application(context)
    fields = [*context.deployment["environment"], *host.application_fields(tmp_path,
        (field["key"] for field in context.deployment["environment"]))]
    return host, application, fields


def _run_nodes(host):
    tree = ast.parse(Path(host.__file__).read_text())
    return next(node for node in tree.body if isinstance(node, ast.AsyncFunctionDef) and node.name == "run")


def _wire(host, application, fields):
    # Execute the actual installed run's declaration wiring only. This source
    # slice has no runtime connection, startup, signal, HTTP or process call.
    node = next(node for node in ast.walk(_run_nodes(host)) if isinstance(node, ast.If)
        and isinstance(node.test, ast.Name) and node.test.id == "console_front"
        and any(isinstance(child, ast.Assign) and any(isinstance(target, ast.Attribute)
            and target.attr == "frontend_private_fields" for target in child.targets) for child in ast.walk(node)))
    front = SimpleNamespace(frontend_private_fields=())
    namespace = {"application": application, "console_front": front, "fields": fields}
    exec(compile(ast.fix_missing_locations(ast.Module(body=[copy.deepcopy(node)], type_ignores=[])),
                 str(host.__file__), "exec"), namespace)
    return front


async def _load_from_run(host, application, sources):
    # Keep the actual shipping load call, including any source-currentness
    # argument. The predecessor has none, which exposes its real admission bug.
    node = next(node for node in ast.walk(_run_nodes(host)) if isinstance(node, ast.Expr)
        and isinstance(node.value, ast.Await) and isinstance(node.value.value, ast.Call)
        and isinstance(node.value.value.func, ast.Attribute) and node.value.value.func.attr == "load"
        and isinstance(node.value.value.func.value, ast.Name) and node.value.value.func.value.id == "application")
    function = ast.AsyncFunctionDef(name="load_from_run", args=ast.arguments(posonlyargs=[], args=[],
        kwonlyargs=[], kw_defaults=[], defaults=[]), body=[copy.deepcopy(node)], decorator_list=[])
    namespace = {"application": application, "console_setup_sources": sources}
    exec(compile(ast.fix_missing_locations(ast.Module(body=[function], type_ignores=[])),
                 str(host.__file__), "exec"), namespace)
    await namespace["load_from_run"]()


async def test_actual_wiring_uses_accepted_owner_names_not_an_earlier_field_read(tmp_path, monkeypatch):
    host, application, earlier_fields = _package(tmp_path, monkeypatch)
    (tmp_path / "application/configuration.json").write_text(json.dumps(_configuration("ACCEPTED_TOKEN")))
    try:
        # This direct owner load has no earlier Console authentication owner.
        # The next control covers the stricter actual run admission boundary.
        await application.load()
        front = _wire(host, application, earlier_fields)
        assert "ACCEPTED_TOKEN" in front.frontend_private_fields
        assert "EARLY_TOKEN" not in front.frontend_private_fields
        assert "CONSOLE_LOGIN_PASSWORD" in front.frontend_private_fields
        assert isinstance(front.frontend_private_fields, tuple)
        assert front.frontend_private_fields == application.frontend_private_fields()
        assert "PUBLIC_CAPTION" not in front.frontend_private_fields
        assert "public-default-is-not-a-private-field-name" not in front.frontend_private_fields
        (tmp_path / ".env").write_text("ACCEPTED_TOKEN=inert-private-value-not-a-declaration\n")
        assert application.frontend_private_fields() == front.frontend_private_fields
        assert application.context.runtime is None
    finally:
        await application.stop()


@pytest.mark.parametrize("source", SETUP_NAMES, ids=["configuration", "authentication"])
async def test_actual_run_refuses_frontend_snapshot_changed_since_console_composition(tmp_path, monkeypatch, source):
    host, application, _ = _package(tmp_path, monkeypatch)
    initial = {name: (tmp_path / name).read_bytes() for name in SETUP_NAMES}
    if source.endswith("configuration.json"):
        (tmp_path / source).write_text(json.dumps(_configuration("ACCEPTED_TOKEN")))
    else:
        # The authentication kind is still valid. Its exact public source has
        # changed, so the earlier Console composition is no longer the owner.
        (tmp_path / source).write_text('{\n "authentication": "local-password"\n}\n')
    try:
        with pytest.raises(ValueError, match="Console setup source changed"):
            await _load_from_run(host, application, initial)
        with pytest.raises(ValueError, match="source changed|undeclared"):
            application.frontend_host()
        assert application.context.runtime is None
    finally:
        await application.stop()


async def test_actual_wiring_refuses_source_changed_after_owner_load(tmp_path, monkeypatch):
    host, application, earlier_fields = _package(tmp_path, monkeypatch)
    try:
        await application.load()
        (tmp_path / "application/configuration.json").write_text(json.dumps(_configuration("LATER_TOKEN")))
        with pytest.raises(ValueError, match="source changed"):
            _wire(host, application, earlier_fields)
        assert application.context.runtime is None
    finally:
        await application.stop()


async def test_actual_early_auth_composition_refuses_change_during_metadata_read(tmp_path, monkeypatch):
    host, application, _ = _package(tmp_path, monkeypatch)
    run = _run_nodes(host)
    nodes = []
    for node in run.body:
        if isinstance(node, ast.Assign) and any(isinstance(target, ast.Name)
                and target.id in {"console_setup_sources", "fields", "authenticated_console"} for target in node.targets):
            nodes.append(copy.deepcopy(node))
        elif isinstance(node, ast.If) and isinstance(node.test, ast.BoolOp) and any(isinstance(child, ast.Call)
                and isinstance(child.func, ast.Name) and child.func.id == "console_setup_source_bytes"
                for child in ast.walk(node.test)):
            nodes.append(copy.deepcopy(node))
    original = host.application_fields
    def changed_metadata(root, existing_keys=()):
        fields = original(root, existing_keys)
        (root / "application/console.json").write_text('{\n "authentication": "local-password"\n}\n')
        return fields
    namespace = {**vars(host), "spec": application.context.deployment, "application_fields": changed_metadata}
    try:
        with pytest.raises(ValueError, match="Console setup source changed during authentication composition"):
            exec(compile(ast.fix_missing_locations(ast.Module(body=nodes, type_ignores=[])),
                         str(host.__file__), "exec"), namespace)
        assert application.context.runtime is None
    finally:
        await application.stop()


def test_package_helper_restores_initially_absent_and_existing_modules(tmp_path, monkeypatch):
    names = ("connectors", "mcp_connections", "application_configuration", "console_configuration",
             "console_auth", "native_definition_continuity")
    # This outer context preserves actual collection state. The inner fixture
    # must restore one existing synthetic module and five initially absent ones.
    for name in names:
        monkeypatch.setitem(sys.modules, name, None)
        monkeypatch.delitem(sys.modules, name)
    existing = SimpleNamespace(inert_preexisting_module=True)
    monkeypatch.setitem(sys.modules, "connectors", existing)
    with pytest.MonkeyPatch.context() as fixture_control:
        host, application, _ = _package(tmp_path, fixture_control)
        assert host is not None and application.context.runtime is None
        assert all(name in sys.modules for name in names)
    assert sys.modules["connectors"] is existing
    assert all(name not in sys.modules for name in names if name != "connectors")
