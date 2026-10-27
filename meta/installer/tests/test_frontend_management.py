"""Offline actual installed-front boundary; no server, native or provider starts."""
from __future__ import annotations

from contextlib import asynccontextmanager
from copy import deepcopy
from dataclasses import FrozenInstanceError
import json
import marshal
import os
from pathlib import Path
import py_compile
import sys

import httpx
import pytest

from launchkit.templates_v2 import console_auth as console_module
from launchkit.templates_v2.console_auth import ConsoleFront
from test_application_callback_bridge import load_package
from test_console_auth import FIELDS, login, metadata

PATH = "/console/forge/frontends"
HOST = {"protocol": 1, "host_id": "forge:" + "1" * 64,
        "application_id": "synthetic-app", "source_digest": "2" * 64}
CONFIG = {"component": "synthetic-chat", "routing": {"participant": "lead:main"},
          "options": {"caption": "A precise \u03bb choice", "features": ["chat", "status"]}}


class Owner:
    def __init__(self):
        self.calls = []
        self.fail_submit = False

    async def __call__(self, request):
        self.calls.append(request)
        if request.operation == "prepare":
            return {"proposal": {"change_id": "change:one", "configuration": request.payload["configuration"],
                                 "expected_revision": request.payload["expected_revision"], "owner_option": "specific"}}
        if request.operation == "submit" and self.fail_submit:
            raise TypeError("Synthetic side effect happened before a private owner exception.")
        return {"status": "unknown", "operation": request.operation,
                **({"change_id": request.payload["change_id"]} if request.operation == "status" else {})}


@asynccontextmanager
async def boundary(tmp_path, owner=None, host=None):
    metadata(tmp_path)
    native = []
    front = ConsoleFront(tmp_path, 8855, "owner", "local-password-123",
        transport=httpx.MockTransport(lambda request: native.append(request) or httpx.Response(200, json={"native": True})))
    if owner is not None:
        front.frontend_management = owner
        front.frontend_host = lambda: deepcopy(HOST if host is None else host)
        front.frontend_private_fields = ("CONSOLE_LOGIN_PASSWORD", "SYNTHETIC_CHAT_TOKEN")
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=front.application("http://127.0.0.1:61347")),
                                     base_url=front.origin) as client:
            yield client, front, native
    finally:
        await front.close()


async def prepare(client, front, configuration=None):
    response = await client.post(PATH + "/prepare", json={"configuration": CONFIG if configuration is None else configuration,
        "expected_revision": {"component": 7}}, headers={"Origin": front.origin})
    assert response.status_code == 200, response.text
    return response.json()


def submission(prepared):
    return {"proposal": deepcopy(prepared["proposal"]),
            "approval": {"approved": True, **prepared["approval"]}}


async def test_undeclared_frontend_owner_never_forwards_management_to_native(tmp_path):
    async with boundary(tmp_path) as (client, front, native):
        await login(client, front)
        response = await client.get(PATH)
        assert response.status_code == 501
        assert "has not declared" in response.text
        assert native == []


async def test_authenticated_gets_delegate_only_read_operations_and_exact_host_principal(tmp_path):
    owner = Owner()
    async with boundary(tmp_path, owner) as (client, front, native):
        await login(client, front)
        inventory = await client.get(PATH)
        status = await client.get(PATH + "/changes/change:one")
        assert inventory.status_code == status.status_code == 200
        assert inventory.json() == {"protocol": 1, "host": HOST,
                                    "observation": {"status": "unknown", "operation": "inventory"}}
        assert status.json()["observation"] == {"status": "unknown", "operation": "status", "change_id": "change:one"}
        assert inventory.headers["cache-control"] == "no-store"
        assert [request.operation for request in owner.calls] == ["inventory", "status"]
        assert all(request.principal == "owner" and request.host == HOST and not request.approved for request in owner.calls)
        with pytest.raises(FrozenInstanceError):
            owner.calls[0].principal = "forged"
        assert native == []


@pytest.mark.parametrize("credential", ["absent", "health", "native"])
async def test_frontend_management_requires_human_session_even_with_host_bearer(tmp_path, credential):
    owner = Owner()
    async with boundary(tmp_path, owner) as (client, front, native):
        headers = {} if credential == "absent" else {"Authorization": "Bearer " + console_module._token(
            front.key, "owner", scope="host-health" if credential == "health" else "native-console")}
        assert (await client.get(PATH, headers=headers)).status_code == 401
        headers["Origin"] = front.origin
        assert (await client.post(PATH + "/prepare", json={"configuration": CONFIG, "expected_revision": 7},
                                  headers=headers)).status_code == 401
        assert owner.calls == native == []


@pytest.mark.parametrize("origin", [None, "http://other.invalid"])
async def test_frontend_mutations_require_authenticated_same_origin(tmp_path, origin):
    owner = Owner()
    async with boundary(tmp_path, owner) as (client, front, native):
        await login(client, front)
        headers = {} if origin is None else {"Origin": origin}
        response = await client.post(PATH + "/prepare", json={"configuration": CONFIG, "expected_revision": 7}, headers=headers)
        assert response.status_code == 403
        assert owner.calls == native == []


async def test_exact_protected_proposal_is_not_approval_until_human_submit(tmp_path):
    owner = Owner()
    async with boundary(tmp_path, owner) as (client, front, native):
        await login(client, front)
        prepared = await prepare(client, front)
        assert prepared["host"] == HOST and prepared["proposal"]["configuration"] == CONFIG
        assert len(owner.calls) == 1 and owner.calls[0].operation == "prepare" and not owner.calls[0].approved
        body = submission(prepared)
        response = await client.post(PATH + "/submit", json=body, headers={"Origin": front.origin})
        assert response.status_code == 200 and response.json()["observation"]["status"] == "unknown"
        assert len(owner.calls) == 2 and owner.calls[1].operation == "submit" and owner.calls[1].approved is True
        assert owner.calls[1].payload == {"proposal": prepared["proposal"],
            "approval": {"principal": "owner", "expires_at": prepared["approval"]["expires_at"]}}
        assert native == []


@pytest.mark.parametrize("alteration", ["configuration", "revision", "change", "unapproved", "expiry", "proof", "source", "principal"])
async def test_changed_or_unapproved_proposal_never_reaches_submit_owner(tmp_path, alteration):
    owner, host = Owner(), deepcopy(HOST)
    async with boundary(tmp_path, owner, host) as (client, front, native):
        await login(client, front)
        body = submission(await prepare(client, front))
        if alteration == "configuration": body["proposal"]["configuration"]["routing"]["participant"] = "other:main"
        if alteration == "revision": body["proposal"]["expected_revision"] = 8
        if alteration == "change": body["proposal"]["change_id"] = "another"
        if alteration == "unapproved": body["approval"]["approved"] = False
        if alteration == "expiry": body["approval"]["expires_at"] = 0
        if alteration == "proof": body["approval"]["proof"] = "0" * 64
        if alteration == "source": host["source_digest"] = "3" * 64
        if alteration == "principal": front.principal = "different-configured-user"
        response = await client.post(PATH + "/submit", json=body, headers={"Origin": front.origin})
        assert response.status_code == 403
        assert [request.operation for request in owner.calls] == ["prepare"]
        assert native == []


async def test_failed_submit_is_unknown_once_and_get_status_does_not_retry(tmp_path):
    owner = Owner()
    async with boundary(tmp_path, owner) as (client, front, native):
        await login(client, front)
        body = submission(await prepare(client, front))
        owner.fail_submit = True
        failed = await client.post(PATH + "/submit", json=body, headers={"Origin": front.origin})
        assert failed.status_code == 503 and failed.json()["status"] == "unknown"
        assert failed.json()["change_id"] == "change:one"
        assert "private owner exception" not in failed.text
        status = await client.get(PATH + "/changes/change:one")
        assert status.status_code == 200 and status.json()["observation"]["status"] == "unknown"
        assert [request.operation for request in owner.calls] == ["prepare", "submit", "status"]
        assert native == []


async def test_declared_private_fields_allow_references_and_refuse_secret_literals_without_echo(tmp_path):
    owner = Owner()
    async with boundary(tmp_path, owner) as (client, front, native):
        await login(client, front)
        secret = "synthetic-private-secret-canary"
        result = await client.post(PATH + "/prepare", json={"configuration": {"options": {"SYNTHETIC_CHAT_TOKEN": secret}},
            "expected_revision": 7}, headers={"Origin": front.origin})
        assert result.status_code == 400 and secret not in result.text
        assert owner.calls == []
        reference = {"SYNTHETIC_CHAT_TOKEN": {"private_field": "SYNTHETIC_CHAT_TOKEN"}}
        assert (await prepare(client, front, reference))["proposal"]["configuration"] == reference
        assert native == []


@pytest.mark.parametrize("raw", [
    pytest.param(b'{"configuration":{},"expected_revision":1,"expected_revision":2}', id="duplicate-key"),
    pytest.param(b'{"configuration":{"bad":NaN},"expected_revision":1}', id="non-finite-number"),
    pytest.param(b'{"configuration":{},"expected_revision":1,"principal":"forged"}', id="forged-principal"),
    pytest.param(b'{"configuration":{"secret":"' + b'x' * (256 * 1024) + b'"},"expected_revision":1}', id="oversize-256KiB")])
async def test_invalid_or_oversize_commands_do_not_invoke_owner(tmp_path, raw):
    owner = Owner()
    async with boundary(tmp_path, owner) as (client, front, native):
        await login(client, front)
        response = await client.post(PATH + "/prepare", content=raw,
            headers={"Origin": front.origin, "Content-Type": "application/json"})
        assert response.status_code == 400
        assert owner.calls == native == []


async def test_deep_valid_json_copy_failure_is_a_controlled_refusal_before_owner(tmp_path):
    # Ordinary JSON validation uses less stack than deepcopy. Reproduce that
    # distinct boundary without depending on pytest's interpreter stack limit.
    nested = "synthetic-nested-value"
    for _ in range(sys.getrecursionlimit() // 2 + 32):
        nested = {"next": nested}
    payload = {"configuration": {"nested": nested}, "expected_revision": 7}
    encoded = console_module._frontend_json(payload)
    assert len(encoded) < console_module.MAX_FRONTEND_BODY
    with pytest.raises(RecursionError):
        deepcopy(payload)
    owner = Owner()
    async with boundary(tmp_path, owner) as (client, front, native):
        await login(client, front)
        response = await client.post(PATH + "/prepare", content=encoded,
            headers={"Origin": front.origin, "Content-Type": "application/json"})
        assert response.status_code == 400
        assert "synthetic-nested-value" not in response.text
        assert owner.calls == native == []


async def test_read_routes_reject_mutations_queries_and_get_bodies_without_owner_invocation(tmp_path):
    owner = Owner()
    async with boundary(tmp_path, owner) as (client, front, native):
        await login(client, front)
        assert (await client.post(PATH, json={}, headers={"Origin": front.origin})).status_code == 405
        assert (await client.get(PATH + "?principal=forged")).status_code == 400
        assert (await client.request("GET", PATH, content=b'{}')).status_code == 400
        assert (await client.get(PATH + "/arbitrary-forwarding")).status_code == 404
        assert owner.calls == native == []


APPLICATION = '''class Integration:
    def __init__(self, context):
        self.context = context
        self.commands = []
    def frontend_management(self, request):
        self.commands.append(request)
        return {"status":"unknown"}
def build(context):
    return Integration(context)
'''


async def test_application_frontend_hook_requires_existing_declared_console_auth(tmp_path, monkeypatch):
    _, application, _ = load_package(tmp_path, monkeypatch, APPLICATION)
    with pytest.raises(ValueError, match="frontend_management requires local-password"):
        await application.load()
    assert application.context.runtime is None


async def test_actual_loaded_application_source_binding_refuses_drift_before_callback(tmp_path, monkeypatch):
    host, application, _ = load_package(tmp_path, monkeypatch, APPLICATION)
    metadata(tmp_path)
    await application.load()
    selected = application.frontend_host()
    assert selected["application_id"] == application.context.deployment["slug"]
    assert str(tmp_path) not in json.dumps(selected)
    # Neither private settings nor native/application state contribute to source identity.
    (tmp_path / ".env").write_text("SYNTHETIC_CHAT_TOKEN=synthetic-private-value\n")
    (tmp_path / "data").mkdir(exist_ok=True)
    (tmp_path / "data/state.json").write_text('{"revision":123}')
    assert application.frontend_host() == selected
    command = console_module.FrontendManagementRequest("inventory", "owner", selected, {})
    assert await application.frontend_management(command) == {"status": "unknown"}
    assert len(application.integration.commands) == 1
    source = tmp_path / "application/__init__.py"
    source.write_text(source.read_text() + "\n# a later application source\n")
    with pytest.raises(ValueError, match="source changed"):
        await application.frontend_management(command)
    assert len(application.integration.commands) == 1
    assert application.context.runtime is None


async def test_asgi_refuses_source_drift_without_callback_or_native_forwarding(tmp_path):
    owner = Owner()
    async with boundary(tmp_path, owner) as (client, front, native):
        await login(client, front)
        def drifted():
            raise ValueError("Synthetic loaded source drift")
        front.frontend_host = drifted
        response = await client.get(PATH)
        assert response.status_code == 409
        assert owner.calls == native == []


@pytest.mark.parametrize("child", [False, True], ids=["entry", "relative-child"])
async def test_managed_owner_refuses_stale_timestamp_bytecode_without_factory_reexecution(tmp_path, monkeypatch, child):
    integration = '''class Integration:
    def __init__(self, context):
        self.marker = "old"
    def frontend_management(self, request):
        return {"marker": self.marker}
'''
    factory = '''def build(context):
    context.factory_calls.append("called")
    return Integration(context)
'''
    source = "from .owner import Integration\n" + factory if child else integration + factory
    _, application, _ = load_package(tmp_path, monkeypatch, source)
    metadata(tmp_path)
    path = tmp_path / ("application/owner.py" if child else "application/__init__.py")
    if child:
        path.write_text(integration)
    original = path.read_bytes()
    stat = path.stat()
    cache = py_compile.compile(str(path), doraise=True)
    cached = Path(cache).read_bytes()
    successor = original.replace(b'"old"', b'"new"')
    assert len(successor) == len(original) and successor != original
    path.write_bytes(successor)
    os.utime(path, ns=(stat.st_atime_ns, stat.st_mtime_ns))
    assert path.stat().st_mtime_ns == stat.st_mtime_ns
    application.context.factory_calls = []
    with pytest.raises(ValueError, match="does not match captured source"):
        await application.load()
    assert application.context.factory_calls == ["called"]
    assert application.integration.marker == "old"
    assert application._frontend_sources is None
    assert Path(cache).read_bytes() == cached
    assert application._frontend_imports not in sys.meta_path
    assert application.context.runtime is None


@pytest.mark.parametrize("operation", ["frontend", "start"])
async def test_managed_lazy_relative_import_refuses_stale_code_before_execution_and_releases_guard(tmp_path, monkeypatch, operation):
    source = '''class Integration:
    def __init__(self, context):
        self.context = context
    def frontend_management(self, request):
        from .lazy import observation
        return observation
    def start(self):
        from .lazy import observation
        self.observation = observation
    def stop(self):
        self.context.events.append("stopped")
def build(context):
    context.events.append("built")
    return Integration(context)
'''
    _, application, _ = load_package(tmp_path, monkeypatch, source)
    metadata(tmp_path)
    child = tmp_path / "application/lazy.py"
    child.write_text('observation = {"marker":"old"}\nif observation["marker"] != "new":\n    raise AssertionError("stale code must not execute")\n')
    stat = child.stat()
    cache = py_compile.compile(str(child), doraise=True)
    cached = Path(cache).read_bytes()
    child.write_bytes(child.read_bytes().replace(b'"old"', b'"new"'))
    os.utime(child, ns=(stat.st_atime_ns, stat.st_mtime_ns))
    assert marshal.loads(cached[16:]) != compile(child.read_bytes(), str(child), "exec", dont_inherit=True)
    application.context.events = []
    unrelated_importers = list(sys.meta_path)
    await application.load()
    command = console_module.FrontendManagementRequest("inventory", "owner", application.frontend_host(), {})
    assert application.context.events == ["built"]
    assert application._frontend_imports in sys.meta_path
    assert application._frontend_imports.find_spec("unrelated.module", []) is None
    with pytest.raises(ValueError, match="does not match captured source"):
        if operation == "start":
            await application.start(None)
        else:
            await application.frontend_management(command)
    assert Path(cache).read_bytes() == cached
    with pytest.raises(ValueError, match="source changed"):
        application.frontend_host()
    await application.stop()
    assert application.context.events == ["built", "stopped"]
    assert sys.meta_path == unrelated_importers
    assert application.context.runtime is None


async def test_ordinary_relative_import_factory_and_lifecycle_remain_once_without_management_guard(tmp_path, monkeypatch):
    source = '''from .ordinary import Integration
def build(context):
    context.events.append("built")
    return Integration(context)
'''
    _, application, _ = load_package(tmp_path, monkeypatch, source)
    (tmp_path / "application/ordinary.py").write_text('''class Integration:
    def __init__(self, context):
        self.context = context
    def start(self):
        self.context.events.append("started")
    def stop(self):
        self.context.events.append("stopped")
''')
    application.context.events = []
    unrelated_importers = list(sys.meta_path)
    await application.load()
    assert sys.meta_path == unrelated_importers
    await application.start(None)
    await application.stop()
    assert application.context.events == ["built", "started", "stopped"]
    assert application._frontend_sources is None
    assert application.context.runtime is None


@pytest.mark.parametrize("stop_failure", [False, True])
async def test_managed_current_relative_source_and_stop_cleanup_preserve_single_factory(tmp_path, monkeypatch, stop_failure):
    source = '''class Integration:
    def __init__(self, context):
        self.context = context
    def frontend_management(self, request):
        from .nested import observation
        return observation
    def stop(self):
        self.context.events.append("stopped")
        if self.context.stop_failure:
            raise ValueError("synthetic stop failure")
def build(context):
    context.events.append("built")
    return Integration(context)
'''
    _, application, _ = load_package(tmp_path, monkeypatch, source)
    metadata(tmp_path)
    directory = tmp_path / "application/nested"
    directory.mkdir()
    (directory / "__init__.py").write_text("from .value import observation\n")
    (directory / "value.py").write_text('observation = {"status":"inactive","revision":7}\n')
    application.context.events = []
    application.context.stop_failure = stop_failure
    unrelated_importers = list(sys.meta_path)
    await application.load()
    selected = application.frontend_host()
    command = console_module.FrontendManagementRequest("inventory", "owner", selected, {})
    assert await application.frontend_management(command) == {"status": "inactive", "revision": 7}
    assert application.frontend_host() == selected
    assert application.context.events == ["built"]
    if stop_failure:
        with pytest.raises(ValueError, match="synthetic stop failure"):
            await application.stop()
    else:
        await application.stop()
    assert application.context.events == ["built", "stopped"]
    assert sys.meta_path == unrelated_importers
    with pytest.raises(ValueError, match="source changed"):
        application.frontend_host()
    assert application.context.runtime is None
