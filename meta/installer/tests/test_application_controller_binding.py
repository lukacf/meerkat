"""Generated host binding controls only, with inert owners and lifecycle doubles.

These cases do not enroll a native Human, create Control/ApprovalPrompt, prove
native J1, invoke an SDK owner, start a gateway, bind a server or open a browser.
Use PYTHONPATH=installer:installer/tests for the existing application helpers.
"""

import asyncio
import inspect
import json
import sys
from types import SimpleNamespace

import pytest

from launchkit.blueprint import starter_blueprint
from launchkit.compiler import generate_files
from launchkit.templates_v2.console_configuration import CONSOLE_FIELDS
from test_application_extension import load_host, write_application


HOOK = '''    def controller_review_factory(self, login):
        self.calls.append(login)
        self.observed.append((login, self.context.runtime))
        return self.context.owner_factory(login)
'''

ADAPTER = '''class Integration:
    def __init__(self, context):
        self.context = context
        self.calls = []
        self.observed = []
''' + HOOK + '''
    async def start(self):
        self.context.events.append("application_start")

    async def stop(self):
        self.context.events.append("application_stop")
'''

FRONTEND_HOOK = '''
    def frontend_management(self, command):
        return {"status": "unknown"}
'''


def package(tmp_path, monkeypatch, *, adapter=ADAPTER, authentication=True, frontend=False):
    root = tmp_path.resolve()
    files = generate_files(starter_blueprint())
    for name, content in files.items():
        if name.startswith("application/"):
            continue
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content)
    write_application(root, adapter + (FRONTEND_HOOK if frontend else ""))
    if authentication:
        (root / "application/console.json").write_text('{"authentication":"local-password"}')
        (root / "application/configuration.json").write_text(json.dumps({"fields": list(CONSOLE_FIELDS)}))
    module = load_host(monkeypatch, root)
    spec = json.loads(files["deployment.json"])
    context = module.ApplicationContext(root, spec, asyncio.Event())
    context.events = []
    owner_live = {"current": True}
    owner = SimpleNamespace(is_current=lambda: owner_live["current"])
    context.owner_factory = lambda login: owner
    application = module.Application(context)
    login_live = {"current": True}
    login_type = sys.modules[module.ConsoleFront.__module__].ConsoleConnectLogin
    login = login_type("owner@example.test", "exact-login-scope", 2_000_000_000_000,
        "http://127.0.0.1:4110", lambda: login_live["current"])
    return SimpleNamespace(root=root, files=files, spec=spec, module=module,
        application=application, context=context, owner=owner, owner_live=owner_live,
        login=login, login_live=login_live)


def change_binding(value, case):
    if case == "missing-runtime":
        value.context.runtime = None
    elif case == "replaced-runtime":
        value.context.runtime = SimpleNamespace(is_running=True)
    elif case == "application-source":
        path = value.root / "application/adapters.py"
        path.write_text(path.read_text() + "\n# Different public application source.\n")
    elif case == "deployment-source":
        path = value.root / "deployment.json"
        path.write_text(path.read_text() + "\n")
    elif case == "authentication-source":
        path = value.root / "application/console.json"
        path.write_text(path.read_text() + "\n")
    elif case == "imports-retired":
        value.application._frontend_imports.close()
    elif case == "import-mismatch":
        value.application._frontend_imports.mismatch = True
    elif case == "stop":
        value.context.stop_event.set()
    elif case == "login":
        value.login_live["current"] = False
    elif case == "nonboolean-login":
        value.login_live["current"] = 1
    else:
        raise AssertionError(f"Unknown binding mutation: {case}")


async def test_controller_hook_calls_trusted_integration_with_exact_login_and_keeps_frontend_support_undeclared(tmp_path, monkeypatch):
    value = package(tmp_path, monkeypatch)
    try:
        await value.application.load()
        assert value.context.runtime is None
        assert value.application._frontend_imports.managed is True
        assert value.application._frontend_sources["deployment.json"] == (value.root / "deployment.json").read_bytes()
        with pytest.raises(ValueError, match="current application runtime"):
            value.application.controller_review_factory(value.login)
        assert value.application.integration.calls == []
        runtime = value.context.runtime = SimpleNamespace(is_running=True)
        returned = value.application.controller_review_factory(value.login)
        assert returned is value.owner
        assert value.application.integration.calls == [value.login]
        assert value.application.integration.calls[0] is value.login
        assert value.application.integration.observed == [(value.login, runtime)]
        assert returned.is_current() is True
        value.owner_live["current"] = False
        assert returned.is_current() is False
        value.owner_live["current"] = True
        assert returned.is_current() is True
        with pytest.raises(ValueError, match="frontend support is undeclared"):
            value.application.frontend_host()
        assert all(tool["name"] != "controller_review_factory"
            for tools in value.application.tools.values() for tool in tools)
        assert value.application.runnables == {}
        # Private configuration and mutable app state are not public source.
        (value.root / ".env").write_text("INERT_PRIVATE_SETTING=not-public-source\n")
        (value.root / "data").mkdir(exist_ok=True)
        (value.root / "data/state.json").write_text('{"changed":true}')
        assert returned.is_current() is True
        assert value.application.integration.calls == [value.login]
    finally:
        await value.application.stop()


@pytest.mark.parametrize("case", ["missing-runtime", "application-source", "deployment-source",
    "authentication-source", "imports-retired", "import-mismatch", "stop", "login", "nonboolean-login"])
async def test_stale_binding_refuses_before_the_trusted_factory_is_called(tmp_path, monkeypatch, case):
    value = package(tmp_path, monkeypatch)
    try:
        await value.application.load()
        value.context.runtime = SimpleNamespace(is_running=True)
        change_binding(value, case)
        with pytest.raises(ValueError):
            value.application.controller_review_factory(value.login)
        assert value.application.integration.calls == []
        assert value.application.integration.observed == []
    finally:
        await value.application.stop()


@pytest.mark.parametrize("case", ["replaced-runtime", "application-source", "imports-retired", "stop", "login"])
async def test_returned_owner_currentness_is_narrowed_to_its_exact_host_lifetime(tmp_path, monkeypatch, case):
    value = package(tmp_path, monkeypatch)
    try:
        await value.application.load()
        value.context.runtime = SimpleNamespace(is_running=True)
        returned = value.application.controller_review_factory(value.login)
        assert returned is value.owner and returned.is_current() is True
        change_binding(value, case)
        assert returned.is_current() is False
        assert value.application.integration.calls == [value.login]
    finally:
        await value.application.stop()


@pytest.mark.parametrize("case", ["replaced-runtime", "application-source", "stop", "login"])
async def test_binding_changed_during_factory_refuses_instead_of_returning_the_selected_owner(tmp_path, monkeypatch, case):
    value = package(tmp_path, monkeypatch)
    try:
        await value.application.load()
        runtime = value.context.runtime = SimpleNamespace(is_running=True)
        def select(login):
            assert login is value.login and value.context.runtime is runtime
            change_binding(value, case)
            return value.owner
        value.context.owner_factory = select
        with pytest.raises(ValueError, match="changed during review selection"):
            value.application.controller_review_factory(value.login)
        assert value.application.integration.calls == [value.login]
        assert value.application.integration.observed == [(value.login, runtime)]
    finally:
        await value.application.stop()


@pytest.mark.parametrize("kind", ["noncallable", "async-function", "async-call-object"])
async def test_invalid_or_async_factory_declarations_refuse_during_load_without_invocation(tmp_path, monkeypatch, kind):
    if kind == "noncallable":
        adapter = ADAPTER.replace(HOOK, "    controller_review_factory = False\n")
    elif kind == "async-function":
        adapter = ADAPTER.replace(HOOK, '''    async def controller_review_factory(self, login):
        raise AssertionError("The asynchronous factory must not be invoked.")
''')
    else:
        adapter = '''class AsyncFactory:
    async def __call__(self, login):
        raise AssertionError("The asynchronous callable must not be invoked.")
''' + ADAPTER.replace(HOOK, "    controller_review_factory = AsyncFactory()\n")
    value = package(tmp_path, monkeypatch, adapter=adapter)
    try:
        with pytest.raises(ValueError, match="callable|synchronous"):
            await value.application.load()
        assert value.context.runtime is None
        assert value.application.integration.calls == []
    finally:
        await value.application.stop()


async def test_factory_requires_real_local_password_source_metadata_and_private_field_declarations(tmp_path, monkeypatch):
    value = package(tmp_path, monkeypatch, authentication=False)
    try:
        with pytest.raises(ValueError, match="requires local-password"):
            await value.application.load()
        assert value.application.integration.calls == [] and value.context.runtime is None
        (value.root / "application/console.json").write_text('{"authentication":"local-password"}')
        (value.root / "application/configuration.json").write_text('{"fields":[]}')
        with pytest.raises(ValueError, match="CONSOLE_USER_ID"):
            await value.application.load()
        assert value.application.integration.calls == [] and value.context.runtime is None
    finally:
        await value.application.stop()


async def test_sync_hook_returning_coroutine_closes_it_without_running_its_body(tmp_path, monkeypatch):
    value = package(tmp_path, monkeypatch)
    bodies = []
    async def unexpected_async_owner():
        bodies.append("executed")
        return value.owner
    pending = unexpected_async_owner()
    try:
        await value.application.load()
        value.context.runtime = SimpleNamespace(is_running=True)
        value.context.owner_factory = lambda login: pending
        with pytest.raises(ValueError, match="synchronously"):
            value.application.controller_review_factory(value.login)
        assert inspect.getcoroutinestate(pending) == inspect.CORO_CLOSED
        assert bodies == [] and value.application.integration.calls == [value.login]
    finally:
        pending.close()
        await value.application.stop()


async def test_none_selection_passes_through_without_inventing_an_owner(tmp_path, monkeypatch):
    value = package(tmp_path, monkeypatch)
    try:
        await value.application.load()
        value.context.runtime = SimpleNamespace(is_running=True)
        value.context.owner_factory = lambda login: None
        assert value.application.controller_review_factory(value.login) is None
        assert value.application.integration.calls == [value.login]
    finally:
        await value.application.stop()


async def test_controller_hook_does_not_implicitly_declare_optional_connect_frontend_support(tmp_path, monkeypatch):
    value = package(tmp_path, monkeypatch)
    try:
        await value.application.load()
        value.context.runtime = SimpleNamespace(is_running=True)
        from console_binding import application_console_options
        binary = value.root / ".mobkit/connect/bin/connect"
        binary.parent.mkdir(parents=True)
        binary.write_bytes(b"")
        with pytest.raises(ValueError, match="frontend support is undeclared"):
            application_console_options(value.application, binary)
        assert value.application.integration.calls == []
    finally:
        await value.application.stop()


@pytest.mark.parametrize("declared", [False, True], ids=["no-review-hook", "review-hook"])
@pytest.mark.parametrize("connect", [False, True], ids=["base-console", "optional-connect"])
@pytest.mark.parametrize("native_startup", [False, True], ids=["legacy-runtime", "native-runtime"])
async def test_actual_run_assigns_declared_hook_before_front_start_and_preserves_its_presentation_with_connect(tmp_path, monkeypatch, declared, connect, native_startup):
    # Actual generated app.run/presentation/Connect composition, with no native
    # execution or HTTP listener. The front below deliberately does not claim
    # NativeControllerReview type validation, reservation, enrollment or start.
    adapter = ADAPTER if declared else ADAPTER.replace(HOOK, "")
    value = package(tmp_path, monkeypatch, adapter=adapter, frontend=connect)
    if native_startup:
        # Inert source intent for the mocked runtime, granting no tools.
        (value.root / "application/tool-policy.json").write_text(json.dumps({
            "provider_id": "inert-console-source", "policy_id": "inert-console-policy", "revision": 1,
            "members": [{"member_identity": row["identity"], "grants": []}
                for row in value.spec["agents"]]}))
    module, contexts, fronts, lifecycle = value.module, [], [], []
    original_context = module.ApplicationContext
    def make_context(root, deployment, stop):
        context = original_context(root, deployment, stop)
        context.events = lifecycle
        context.owner_factory = lambda login: value.owner
        contexts.append(context)
        return context
    monkeypatch.setattr(module, "ApplicationContext", make_context)
    gateway = value.root / ".bin/inert-gateway"
    gateway.parent.mkdir(parents=True, exist_ok=True)
    gateway.write_bytes(b"")
    monkeypatch.setattr(module, "gateway_path", lambda: gateway)
    monkeypatch.setenv("OPENAI_API_KEY", "inert-test-provider-key")
    monkeypatch.setenv("CONSOLE_USER_ID", "owner@example.test")
    monkeypatch.setenv("CONSOLE_LOGIN_PASSWORD", "inert-test-password-123")
    monkeypatch.setenv("MOBKIT_PORT", "4110")
    if connect:
        binary = value.root / ".mobkit/connect/bin/connect"
        binary.parent.mkdir(parents=True)
        binary.write_bytes(b"")
        monkeypatch.setenv("MOBKIT_CONNECT_BIN", str(binary))
    else:
        monkeypatch.delenv("MOBKIT_CONNECT_BIN", raising=False)

    class LocalTools:
        def __init__(self, *args):
            pass
        def for_agent(self, agent):
            return []
        async def close(self):
            lifecycle.append("tools_close")

    class Pool:
        def __init__(self, spec):
            self.connections = {}
        def for_class(self, item):
            return []
        async def start(self):
            lifecycle.append("pool_start")
        async def close(self):
            lifecycle.append("pool_close")

    class Runtime:
        rust_http_base_url = "http://127.0.0.1:0"
        is_running = True
        async def identity_bootstrap_status(self):
            return SimpleNamespace(complete=True, ready=True, counts=SimpleNamespace(broken=0))
        async def shutdown(self):
            self.is_running = False
            lifecycle.append("runtime_shutdown")
    runtime = Runtime()

    class Front:
        native_auth = {"mode": "inert-host-wiring"}
        def __init__(self, *args, **kwargs):
            self.on_input = args[4]
            self.console_login_factory = kwargs.get("console_login_factory")
            self.controller_review_factory = kwargs.get("controller_review_factory")
            self.connect_factory = None
            fronts.append(self)
        async def start(self, native_url):
            assert native_url == runtime.rust_http_base_url
            assert lifecycle[-1] == "application_start"
            application = contexts[0]._application
            assert contexts[0].runtime is runtime and not contexts[0].stop_event.is_set()
            if native_startup:
                assert self.console_login_factory.__self__ is application
                assert self.console_login_factory.__func__ is module.Application.console_login_factory
            else:
                assert self.console_login_factory is None
            if declared:
                assert self.controller_review_factory.__self__ is application
                assert self.controller_review_factory.__func__ is module.Application.controller_review_factory
                assert self.controller_review_factory(value.login) is value.owner
                assert value.owner.is_current() is True
            else:
                assert self.controller_review_factory is None
            assert callable(self.connect_factory) is connect
            review_files = {"controller_review.mjs", "controller_review.css"}
            for name in review_files:
                url = "/console/forge/assets/" + name
                assert (url in self.console_assets) is declared
                if declared:
                    assert self.console_assets[url][0] == (value.root / name).read_bytes()
            assert ("/console/forge/assets/console_setup.mjs" in self.console_assets) is connect
            self.document = self.document_transform(b'<html><head></head><body><div id="root"></div></body></html>')
            assert (b"/console/forge/assets/controller_review.mjs" in self.document) is declared
            assert (b"/console/forge/assets/controller_review.css" in self.document) is declared
            assert (b"/console/forge/assets/console_setup.mjs" in self.document) is connect
            assert b'<div id="root"></div>' in self.document
            lifecycle.append("front_start")
        async def close(self):
            lifecycle.append("front_close")

    async def connected(config, application, *, application_startup=None):
        assert application_startup is startup
        assert config.roster_provider is application
        assert config.console_require_app_auth is True
        application.context.runtime = runtime
        lifecycle.append("runtime_connected")
        return runtime
    async def no_storage_operation(root, build):
        assert root == value.root
    async def no_readiness_pipe(runtime, console_url, *, stop=None):
        assert console_url == "http://127.0.0.1:4110/console"
    async def completed_host_wait(stop, front):
        assert front is fronts[0] and lifecycle[-1] == "front_start"
        stop.set()

    monkeypatch.setattr(module, "ConnectorTools", LocalTools)
    monkeypatch.setattr(module, "MCPPool", Pool)
    monkeypatch.setattr(module, "ConsoleFront", Front)
    monkeypatch.setattr(module, "connect_application_runtime", connected)
    monkeypatch.setattr(module, "report_host_readiness", no_readiness_pipe)
    monkeypatch.setattr(module, "await_service_shutdown", completed_host_wait)
    monkeypatch.setitem(sys.modules, "storage_setup", SimpleNamespace(prepare_storage=no_storage_operation))
    monkeypatch.setattr(asyncio.get_running_loop(), "add_signal_handler", lambda *args: None)
    args = SimpleNamespace(connections_check=False, activate_revision=None, definition_revision=False,
        list_flows=False, run_flow=None, smoke=False)
    async def prepared(application):
        assert application is contexts[0]._application
    async def shutdown_selected(selected):
        assert selected is runtime
        await runtime.shutdown()
    startup = (SimpleNamespace(gateway=SimpleNamespace(path=gateway),
        builder_config=lambda **kwargs: SimpleNamespace(**kwargs), prepare=prepared, shutdown=shutdown_selected)
        if native_startup else None)
    await module.run(value.spec, args, application_startup=startup)
    assert len(contexts) == len(fronts) == 1
    assert len(contexts[0]._application.integration.calls) == int(declared)
    assert lifecycle == ["pool_start", "runtime_connected", "application_start", "front_start",
        "front_close", "application_stop", "runtime_shutdown", "pool_close", "tools_close"]
    if declared:
        assert value.owner.is_current() is False
