"""Generated application enrollment selection, not SDK or native execution."""
import hashlib
import inspect
import sys
from pathlib import Path
from types import SimpleNamespace

import pytest

from launchkit.blueprint import starter_blueprint
from launchkit.compiler import generate_files
from test_application_controller_binding import ADAPTER, HOOK, change_binding, package as controller_package


LOGIN_HOOK = HOOK.replace("controller_review_factory", "console_login_factory")
LOGIN_ADAPTER = ADAPTER.replace(HOOK, LOGIN_HOOK)


def package(tmp_path, monkeypatch, *, adapter=LOGIN_ADAPTER, authentication=True):
    return controller_package(tmp_path, monkeypatch, adapter=adapter, authentication=authentication)


async def test_login_factory_passes_exact_labels_in_a_host_fenced_projection_without_claiming_frontend_support(tmp_path, monkeypatch):
    value = package(tmp_path, monkeypatch)
    try:
        await value.application.load()
        with pytest.raises(ValueError, match="current application runtime"):
            value.application.console_login_factory(value.login)
        assert value.application.integration.calls == []
        runtime = value.context.runtime = SimpleNamespace(is_running=True)
        original_owner_check = value.owner.is_current
        returned = value.application.console_login_factory(value.login)
        assert returned is value.owner and returned.is_current is original_owner_check
        selected = value.application.integration.calls[0]
        assert selected is not value.login
        assert type(selected) is type(value.login)
        for field in ("principal", "login_scope_id", "expires_at_ms", "origin"):
            assert getattr(selected, field) == getattr(value.login, field)
        assert selected.is_current() is True and value.login.is_current() is True
        assert value.application.integration.observed == [(selected, runtime)]
        assert all(tool["name"] != "console_login_factory"
            for tools in value.application.tools.values() for tool in tools)
        assert value.application.runnables == {}
        with pytest.raises(ValueError, match="frontend support is undeclared"):
            value.application.frontend_host()
        (value.root / ".env").write_text("INERT_PRIVATE_SETTING=not-public-source\n")
        assert selected.is_current() is True
        assert value.application.integration.calls == [selected]
    finally:
        await value.application.stop()


@pytest.mark.parametrize("case", ["missing-runtime", "application-source", "imports-retired", "stop", "login"])
async def test_stale_application_login_binding_never_enters_trusted_selection(tmp_path, monkeypatch, case):
    value = package(tmp_path, monkeypatch)
    try:
        await value.application.load()
        value.context.runtime = SimpleNamespace(is_running=True)
        change_binding(value, case)
        with pytest.raises(ValueError):
            value.application.console_login_factory(value.login)
        assert value.application.integration.calls == []
    finally:
        await value.application.stop()


@pytest.mark.parametrize("case", ["replaced-runtime", "application-source", "imports-retired", "stop", "login"])
async def test_selected_login_projection_remains_bound_to_original_application_and_login(tmp_path, monkeypatch, case):
    value = package(tmp_path, monkeypatch)
    try:
        await value.application.load()
        value.context.runtime = SimpleNamespace(is_running=True)
        assert value.application.console_login_factory(value.login) is value.owner
        selected = value.application.integration.calls[0]
        assert selected.is_current() is True
        change_binding(value, case)
        assert selected.is_current() is False
        assert value.application.integration.calls == [selected]
    finally:
        await value.application.stop()


async def test_selection_that_invalidates_source_still_returns_dormant_owner_for_console_cleanup(tmp_path, monkeypatch):
    value = package(tmp_path, monkeypatch)
    try:
        await value.application.load()
        value.context.runtime = SimpleNamespace(is_running=True)
        def select(selected):
            assert selected.is_current() is True
            change_binding(value, "application-source")
            return value.owner
        value.context.owner_factory = select
        # ConsoleFront must acquire this returned owner before validating it.
        # The application does not discard an owner after source changes.
        assert value.application.console_login_factory(value.login) is value.owner
        assert len(value.application.integration.calls) == 1
        assert value.application.integration.calls[0].is_current() is False
        assert value.login.is_current() is True
    finally:
        await value.application.stop()


@pytest.mark.parametrize("kind", ["noncallable", "async-function", "async-call-object"])
async def test_login_factory_declaration_requires_a_synchronous_callable(tmp_path, monkeypatch, kind):
    if kind == "noncallable":
        adapter = LOGIN_ADAPTER.replace(LOGIN_HOOK, "    console_login_factory = False\n")
    elif kind == "async-function":
        adapter = LOGIN_ADAPTER.replace(LOGIN_HOOK, '''    async def console_login_factory(self, login):
        raise AssertionError("An asynchronous login factory must not be entered.")
''')
    else:
        adapter = '''class AsyncFactory:
    async def __call__(self, login):
        raise AssertionError("An asynchronous login callable must not be entered.")
''' + LOGIN_ADAPTER.replace(LOGIN_HOOK, "    console_login_factory = AsyncFactory()\n")
    value = package(tmp_path, monkeypatch, adapter=adapter)
    try:
        with pytest.raises(ValueError, match="callable|synchronous"):
            await value.application.load()
        assert value.context.runtime is None and value.application.integration.calls == []
    finally:
        await value.application.stop()


async def test_login_factory_requires_actual_local_password_metadata(tmp_path, monkeypatch):
    value = package(tmp_path, monkeypatch, authentication=False)
    try:
        with pytest.raises(ValueError, match="console_login_factory requires local-password"):
            await value.application.load()
        assert value.context.runtime is None and value.application.integration.calls == []
    finally:
        await value.application.stop()


async def test_login_factory_hands_raw_selection_to_console_without_entering_async_owner(tmp_path, monkeypatch):
    value = package(tmp_path, monkeypatch)
    bodies = []
    async def unexpected_owner():
        bodies.append("executed")
        return value.owner
    pending = unexpected_owner()
    try:
        await value.application.load()
        value.context.runtime = SimpleNamespace(is_running=True)
        value.context.owner_factory = lambda selected: pending
        assert value.application.console_login_factory(value.login) is pending
        assert inspect.getcoroutinestate(pending) == inspect.CORO_CREATED
        assert bodies == [] and len(value.application.integration.calls) == 1
    finally:
        pending.close()
        await value.application.stop()


async def test_normal_generated_package_retains_login_adapter_bytes_and_defaults_do_not_import_toolkit(tmp_path, monkeypatch):
    toolkit_before = sys.modules.get("meerkat_toolkit")
    value = package(tmp_path, monkeypatch, adapter=ADAPTER.replace(HOOK, ""))
    try:
        await value.application.load()
        assert getattr(value.application.integration, "console_login_factory", None) is None
        assert sys.modules.get("meerkat_toolkit") is toolkit_before
        files = generate_files(starter_blueprint())
        source = Path(__file__).parents[1] / "launchkit/templates_v2/native_console_login.py"
        assert files["native_console_login.py"].encode() == source.read_bytes()
        digest = hashlib.sha256(source.read_bytes()).hexdigest()
        assert f"{digest}  native_console_login.py\n" in files["SHA256SUMS"]
        assert (value.root / "native_console_login.py").read_bytes() == source.read_bytes()
    finally:
        await value.application.stop()
