"""Generated no-hook application + actual ASGI/adapter, with inert SDK calls.

This proves host enrollment wiring and lifetime fences, not native execution.
The existing adapter fixture retains real SDK types but opens no private pipe.
"""
import asyncio
import importlib.util
import json
import sys
from types import SimpleNamespace

import httpx
import jwt
import pytest

from test_application_controller_binding import ADAPTER, HOOK, change_binding, package
from test_console_auth import login
import test_native_console_login as native
from test_native_console_login import adopted_source_imports


def ordinary(tmp_path, monkeypatch):
    monkeypatch.delitem(sys.modules, "native_console_login", raising=False)
    value = package(tmp_path, monkeypatch, adapter=ADAPTER.replace(HOOK, ""))
    sdk = sys.modules["meerkat_toolkit.host_control"]
    enrollment = native.Enrollment()
    client = native.Client(enrollment)

    class Runtime(sdk.ToolkitMobKitRuntime):
        def __new__(cls):
            # An inert typed double; neither SDK runtime constructor is entered.
            return object.__new__(cls)

        def __init__(self):
            self.client = client

        @property
        def host_control(self):
            return self.client

    value.context.runtime = Runtime()
    return value, client, enrollment


async def test_ordinary_application_enrolls_before_governed_send_and_retires_on_logout(tmp_path, monkeypatch):
    value, private, enrollment = ordinary(tmp_path, monkeypatch)
    await value.application.load()
    forwarded = []
    front = None

    def upstream(request):
        assert enrollment.result_release.is_set()
        auth = sys.modules[value.module.ConsoleFront.__module__]
        claims = jwt.decode(request.headers["authorization"].removeprefix("Bearer "),
            front.key, algorithms=["HS256"], audience=auth.AUDIENCE, issuer=auth.ISSUER)
        selected = private.selections[0]
        assert (claims["iss"], claims["sub"], claims["sid"]) == (
            selected.issuer, selected.subject, selected.login_scope_id)
        forwarded.append(request)
        body = json.loads(request.content)
        return httpx.Response(200, json={"jsonrpc": "2.0", "id": body["id"], "result": {
            "identity": "lead:main", "interaction_id": "interaction-1", "session_id": "session-1",
            "input_frame_id": "frame-1", "cursor": "console:1", "status": "accepted"}})

    front = value.module.ConsoleFront(value.root, 8855, "local-user", "local-password-123",
        value.application.on_console_input, console_login_factory=value.application.console_login_factory,
        runtime_namespace="meta", transport=httpx.MockTransport(upstream))
    app = front.application("http://127.0.0.1:61347")
    pending = None
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=app), base_url=front.origin) as client:
            await login(client, front)
            await native.entered(private.begin_entered)
            await native.entered(enrollment.result_entered)
            pending = asyncio.create_task(client.post("/console/rpc", headers={"Origin": front.origin}, json={
                "jsonrpc": "2.0", "id": "send-1", "method": "meta/console/send", "params": {
                    "identity": "lead:main", "content": "An ordinary task", "origin": "console:test",
                    "origin_kind": "operator", "idempotency_key": "request-1", "handling_mode": "queue"}}))
            await asyncio.sleep(0)
            assert not pending.done() and forwarded == []
            assert len(private.selections) == 1
            enrollment.result_release.set()
            response = await asyncio.wait_for(pending, 3)
            assert response.status_code == 200 and response.json()["result"]["interaction_id"] == "interaction-1"
            assert len(forwarded) == 1 and len(private.selections) == 1
            assert not hasattr(value.application.integration, "console_login_factory")
            assert not hasattr(value.application.integration, "controller_review_factory")
            with pytest.raises(ValueError, match="frontend support is undeclared"):
                value.application.frontend_host()
            assert (await client.post("/logout", headers={"Origin": front.origin})).status_code == 303
            assert enrollment.retire_calls == 1 and private.close_calls == 0
            assert (await client.post("/console/rpc", headers={"Origin": front.origin}, json={})).status_code == 401
    finally:
        enrollment.result_release.set()
        if pending is not None:
            await asyncio.gather(pending, return_exceptions=True)
        await front.close()
        await value.application.stop()


async def test_ordinary_enrollment_failure_never_forwards_or_reenrolls(tmp_path, monkeypatch):
    value, private, enrollment = ordinary(tmp_path, monkeypatch)
    await value.application.load()
    enrollment.result_error = RuntimeError("INERT_ENROLLMENT_REFUSAL")
    enrollment.result_release.set()
    forwarded = []
    front = value.module.ConsoleFront(value.root, 8855, "local-user", "local-password-123",
        console_login_factory=value.application.console_login_factory, runtime_namespace="meta",
        transport=httpx.MockTransport(lambda request: forwarded.append(request) or httpx.Response(200)))
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=front.application("http://127.0.0.1:61347")),
                base_url=front.origin) as client:
            await login(client, front)
            assert (await client.get("/console")).status_code == 503
            assert (await client.get("/console")).status_code == 503
            assert len(private.selections) == 1 and forwarded == []
    finally:
        await front.close()
        await value.application.stop()
    assert enrollment.retire_calls == 1 and private.close_calls == 0


@pytest.mark.parametrize("case", ["replaced-runtime", "application-source", "authentication-source",
    "imports-retired", "stop", "login", "private-client"])
async def test_default_login_selection_keeps_exact_original_runtime_client_source_and_login(tmp_path, monkeypatch, case):
    value, private, enrollment = ordinary(tmp_path, monkeypatch)
    try:
        await value.application.load()
        owner = value.application.console_login_factory(value.login)
        assert owner.is_current() is True and private.selections == []
        if case == "private-client":
            value.context.runtime.client = native.Client(native.Enrollment())
        else:
            change_binding(value, case)
        assert owner.is_current() is False
        error_type = sys.modules[owner.__class__.__module__].NativeConsoleLoginError
        with pytest.raises(error_type) as refusal:
            owner.start()
        assert refusal.value.code == "invalid_binding"
        assert private.selections == [] and private.close_calls == 0
    finally:
        enrollment.result_release.set()
        await value.application.stop()


EARLIER_WIRING = ('                if application_startup is not None or callable(getattr(application.integration, '
    '"console_login_factory", None)):\n')


def earlier_template_host(monkeypatch, root):
    # The installed copy of an earlier template: its Console wiring selects
    # the default login factory whenever any startup is present.
    path = root / "app.py"
    source = path.read_text()
    start = source.find("                # The default factory enrolls only")
    if start != -1:
        end = source.index("\n", source.index('"console_login", True)):', start)) + 1
        source = source[:start] + EARLIER_WIRING + source[end:]
    assert source.count(EARLIER_WIRING) == 1 and '"console_login", True' not in source
    path.write_text(source)
    definition = importlib.util.spec_from_file_location("application_test_host", path)
    module = importlib.util.module_from_spec(definition)
    definition.loader.exec_module(module)
    monkeypatch.setattr(module, "ROOT", root)
    return module


@pytest.mark.parametrize("case", ["ordinary-meta", "earlier-template", "ordinary-hook", "ordinary-malformed-hook",
    "toolkit", "toolkit-refused"])
async def test_actual_run_installs_default_login_only_for_a_toolkit_startup(tmp_path, monkeypatch, case):
    # Actual generated app.run() Console wiring and password login through the
    # ASGI front, with inert runtime/SDK doubles. The ordinary Meta startup runs
    # its real prepare(); a Toolkit startup keeps its own class declarations.
    from launchkit.connect_application_startup import RestoredApplicationStartup
    from launchkit.ordinary_meta_startup import OrdinaryMetaStartup
    toolkit = case.startswith("toolkit")
    monkeypatch.delitem(sys.modules, "native_console_login", raising=False)
    declared = HOOK.replace("controller_review_factory", "console_login_factory")
    value = package(tmp_path, monkeypatch, adapter=ADAPTER.replace(HOOK, declared if case == "ordinary-hook" else ""))
    module = earlier_template_host(monkeypatch, value.root) if case == "earlier-template" else value.module
    contexts, fronts, forwarded, owners = [], [], [], []
    auth = sys.modules[module.ConsoleFront.__module__]
    (value.root / "application/tool-policy.json").write_text(json.dumps({
        "provider_id": "inert-console-source", "policy_id": "inert-console-policy", "revision": 1,
        "members": [{"member_identity": row["identity"], "grants": []} for row in value.spec["agents"]]}))
    class RefusingOwner:
        # The declared hook's own login owner; its admission refuses.
        def is_current(self):
            return True
        def start(self):
            raise RuntimeError("INERT_DECLARED_LOGIN_REFUSAL")
        def revoke(self):
            pass
        async def ready(self):
            return False
        async def retire(self, reason):
            pass
        async def close(self):
            pass

    if case == "ordinary-malformed-hook":
        original_load = module.Application.load
        async def load_then_malform(self, *args, **kwargs):
            await original_load(self, *args, **kwargs)
            # Application.load refuses a present hook that is not a synchronous
            # callable. Adding one after load shows that the seam and the wiring
            # still never turn it into the password-only login.
            self.integration.console_login_factory = "present-but-not-callable"
        monkeypatch.setattr(module.Application, "load", load_then_malform)
    original_context = module.ApplicationContext
    def make_context(root, deployment, stop):
        context = original_context(root, deployment, stop)
        context.events = []
        context.owner_factory = lambda login: owners.append(RefusingOwner()) or owners[-1]
        contexts.append(context)
        return context
    monkeypatch.setattr(module, "ApplicationContext", make_context)
    gateway = value.root / ".bin/inert-gateway"
    gateway.parent.mkdir(parents=True, exist_ok=True)
    gateway.write_bytes(b"")
    monkeypatch.setenv("OPENAI_API_KEY", "inert-test-provider-key")
    monkeypatch.setenv("CONSOLE_USER_ID", "local-user")
    monkeypatch.setenv("CONSOLE_LOGIN_PASSWORD", "local-password-123")
    monkeypatch.setenv("MOBKIT_PORT", "8855")
    monkeypatch.delenv("MOBKIT_CONNECT_BIN", raising=False)
    enrollment = native.Enrollment()
    private = native.Client(enrollment)
    if case == "toolkit-refused":
        enrollment.result_error = RuntimeError("INERT_ENROLLMENT_REFUSAL")
    enrollment.result_release.set()

    class Runtime(sys.modules["meerkat_toolkit.host_control"].ToolkitMobKitRuntime if toolkit
                  else sys.modules["meerkat_meta.runtime"].MetaRuntime):
        rust_http_base_url = "http://127.0.0.1:61347"
        is_running = True
        def __new__(cls):
            # An inert typed double; neither SDK runtime constructor is entered.
            return object.__new__(cls)
        def __init__(self):
            pass
        @property
        def host_control(self):
            return private
        async def identity_bootstrap_status(self):
            return SimpleNamespace(complete=True, ready=True, counts=SimpleNamespace(broken=0))
        async def shutdown(self):
            self.is_running = False
    runtime = Runtime()

    class OrdinaryStartup(OrdinaryMetaStartup):
        # The real class declarations and prepare(); no custody input is read.
        def __new__(cls):
            return object.__new__(cls)
        def __init__(self):
            self.gateway, self.root, self.runtime = SimpleNamespace(path=gateway), value.root, None
            self.application = self.config = None
            self._prepared = False
            self._require_current = lambda: None
            self._read_inputs = lambda: (b"", ())
        def builder_config(self, **options):
            return SimpleNamespace(**options)
        async def shutdown(self, selected):
            assert selected is runtime
            await runtime.shutdown()

    class ToolkitStartup(RestoredApplicationStartup):
        # The real class declarations; its preparation is outside these rows.
        def __new__(cls):
            return object.__new__(cls)
        def __init__(self):
            self.gateway = SimpleNamespace(path=gateway)
        def builder_config(self, **options):
            return SimpleNamespace(**options)
        async def prepare(self, application):
            assert application is contexts[0]._application
        async def shutdown(self, selected):
            assert selected is runtime
            await runtime.shutdown()
    startup = ToolkitStartup() if toolkit else OrdinaryStartup()

    class LocalTools:
        def __init__(self, *args):
            pass
        def for_agent(self, agent):
            return []
        async def close(self):
            pass

    class Pool:
        def __init__(self, spec):
            self.connections = {}
        def for_class(self, item):
            return []
        async def start(self):
            pass
        async def close(self):
            pass

    def upstream(request):
        forwarded.append(request)
        return httpx.Response(200, html='<html><head></head><body><div id="root"></div></body></html>')

    class Front(module.ConsoleFront):
        def __init__(self, *args, **kwargs):
            super().__init__(*args, transport=httpx.MockTransport(upstream), **kwargs)
            fronts.append(self)
        async def start(self, native_url):
            # One in-process browser session replaces the listener.
            assert native_url == runtime.rust_http_base_url
            async with httpx.AsyncClient(transport=httpx.ASGITransport(app=self.application(native_url)),
                    base_url=self.origin) as client:
                await login(client, self)
                self.statuses = [(await client.get("/console")).status_code for _ in range(2)]
                self.required = [entry.console_login_required for entry in self._logins.values()]

    async def connected(config, application, *, application_startup=None):
        assert application_startup is startup
        application.context.runtime = runtime
        return runtime
    async def no_storage_operation(root, build):
        assert root == value.root
    async def no_readiness_pipe(runtime, console_url, *, stop=None):
        pass
    async def completed_host_wait(stop, front):
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
    await module.run(value.spec, args, application_startup=startup)
    front, = fronts
    application = contexts[0]._application
    if case in ("ordinary-meta", "earlier-template"):
        # The configured password session and native_auth; no enrollment owner.
        assert front.statuses == [200, 200] and len(forwarded) == 2 and front.required == [False]
        assert front.console_login_factory is None and private.selections == []
        claims = jwt.decode(forwarded[0].headers["authorization"].removeprefix("Bearer "),
            front.key, algorithms=["HS256"], audience=auth.AUDIENCE, issuer=auth.ISSUER)
        assert claims["sub"] == "local-user"
        return
    assert front.console_login_factory.__self__ is application
    assert front.console_login_factory.__func__ is module.Application.console_login_factory
    assert front.required == [True]
    if case in ("ordinary-hook", "ordinary-malformed-hook"):
        # A present hook keeps the guarded factory: its refusal is never replaced
        # by the password session, and no Toolkit enrollment is attempted.
        assert front.statuses == [503, 503] and forwarded == [] and private.selections == []
        expected = 1 if case == "ordinary-hook" else 0
        assert len(application.integration.calls) == len(owners) == expected
        return
    assert len(private.selections) == 1 and enrollment.retire_calls == 1 and private.close_calls == 0
    if case == "toolkit-refused":
        assert front.statuses == [503, 503] and forwarded == []
        return
    assert front.statuses == [200, 200] and len(forwarded) == 2
    selected = private.selections[0]
    claims = jwt.decode(forwarded[0].headers["authorization"].removeprefix("Bearer "),
        front.key, algorithms=["HS256"], audience=auth.AUDIENCE, issuer=auth.ISSUER)
    assert (claims["iss"], claims["sub"], claims["sid"]) == (
        selected.issuer, selected.subject, selected.login_scope_id)


@pytest.mark.parametrize("hook", ["absent", "callable", "not-callable"])
async def test_ordinary_prepare_clears_only_an_absent_console_login_hook(tmp_path, hook):
    # The real prepare() seam alone. Application.load refuses a present hook that
    # is not a synchronous callable; the seam itself must still leave it in place.
    from launchkit.ordinary_meta_startup import OrdinaryMetaStartup
    integration = SimpleNamespace()
    if hook == "callable":
        integration.console_login_factory = lambda login: None
    elif hook == "not-callable":
        integration.console_login_factory = "present-but-not-callable"
    application = SimpleNamespace(context=SimpleNamespace(root=tmp_path, runtime=None), dynamic=False,
        _application_callbacks=None, integration=integration)

    class Startup(OrdinaryMetaStartup):
        # The real class and prepare(); no custody input is read.
        def __new__(cls):
            return object.__new__(cls)
        def __init__(self):
            self.root, self.runtime, self.application = tmp_path, None, None
            self._prepared = False
            self._require_current = lambda: None
            self._read_inputs = lambda: (b"", ())

    await Startup().prepare(application)
    if hook == "absent":
        assert application.console_login_factory is None
    else:
        assert not hasattr(application, "console_login_factory")
