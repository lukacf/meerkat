"""Trusted app/login composition without any child or provider operation."""
from pathlib import Path
from types import SimpleNamespace
import ast
import asyncio
import copy
import os
import shutil
import sys
import time

import pytest

from launchkit.templates_v2.console_binding import (application_console_options,
    console_connect_options, console_service_options)
from launchkit.templates_v2.console_connect import ConnectError
from launchkit.templates_v2 import console_binding, console_presentation


TEMPLATES = Path(__file__).resolve().parents[1] / "launchkit/templates_v2"


@pytest.fixture
def installed(tmp_path):
    root = tmp_path.resolve() / "app"
    root.mkdir()
    for name in ("console_shell.mjs", "console_shell.css", "console_setup.mjs", "console_setup.css"):
        shutil.copyfile(TEMPLATES / name, root / name)
    shutil.copytree(TEMPLATES / "toolkit-ui", root / "toolkit-ui")
    return root


def login(**changes):
    values = dict(principal="fixture-reader", login_scope_id="login-one",
                  expires_at_ms=int(time.time() * 1000) + 60000,
                  origin="http://127.0.0.1:4510", is_current=lambda: True)
    return SimpleNamespace(**(values | changes))


def options(root, **changes):
    values = dict(is_current=lambda: True, provider_key="selected-private-fixture")
    return console_connect_options(root, "reading-with-telegram",
        root / "native/bin/meerkat-connect", **(values | changes))


def test_actual_login_and_explicit_realm_are_bound_without_starting_any_child(installed):
    selected = options(installed)
    owner = login()
    bridge = selected["connect_factory"](owner)
    assert bridge.bootstrap["viewer"] == {"principal_id": owner.principal,
        "login_scope_id": owner.login_scope_id, "expires_at_ms": owner.expires_at_ms}
    assert bridge.bootstrap["application"]["origin"] == owner.origin
    assert bridge.bootstrap["application"]["application_id"] == "reading-with-telegram"
    assert bridge.bootstrap["realm"] == {"realm_id": "forge-reading-with-telegram",
        "state_root": str(installed / ".mobkit/connect/state"), "context_root": str(installed),
        "user_config_root": str(installed / ".mobkit/connect/user")}
    assert "model" not in bridge.bootstrap
    assert "auth_binding" not in bridge.bootstrap
    assert bridge._process is None and bridge._start_task is None
    assert not (installed / ".mobkit").exists()


def test_same_app_lifetime_shares_instance_but_distinct_actual_logins_do_not_share_bridges(installed):
    factory = options(installed)["connect_factory"]
    first = factory(login())
    second = factory(login(principal="other-reader", login_scope_id="login-two"))
    assert first is not second
    assert first.bootstrap["application"] == second.bootstrap["application"]
    assert first.bootstrap["viewer"] != second.bootstrap["viewer"]
    assert first._requests is not second._requests


def test_restart_changes_instance_preserving_the_same_explicit_realm_paths(installed):
    first = options(installed)["connect_factory"](login())
    successor = options(installed)["connect_factory"](login())
    assert first.bootstrap["application"]["instance_id"] != successor.bootstrap["application"]["instance_id"]
    assert first.bootstrap["realm"] == successor.bootstrap["realm"]
    assert "model" not in first.bootstrap and "model" not in successor.bootstrap


@pytest.mark.parametrize("service", [False, True])
@pytest.mark.parametrize("model", [None, "gpt-6-luna", "owner-selected-model"])
def test_native_model_selection_is_unchanged_unless_owner_supplies_override(installed, service, model):
    if service:
        selected = console_service_options(installed, "reading-with-telegram",
            installed / "native/bin/meerkat-connect", resource_manifest_sha256="ab" * 32,
            is_current=lambda: True, model=model)
    else:
        selected = options(installed, model=model)
    bridge = selected["connect_factory"](login())
    if model is None:
        assert "model" not in bridge.bootstrap
    else:
        assert bridge.bootstrap["model"] == model
    assert bridge._process is None and not (installed / ".mobkit").exists()


@pytest.mark.parametrize("service", [False, True])
@pytest.mark.parametrize("model", ["", False, 1, {}])
def test_invalid_explicit_model_is_refused_instead_of_becoming_native_default(installed, service, model):
    with pytest.raises(ValueError):
        if service:
            console_service_options(installed, "reading-with-telegram",
                installed / "native/bin/meerkat-connect", resource_manifest_sha256="ab" * 32,
                is_current=lambda: True, model=model)
        else:
            options(installed, model=model)


def test_different_apps_have_distinct_realms_and_context(installed):
    other = installed.parent / "other"
    shutil.copytree(installed, other)
    first = options(installed)["connect_factory"](login())
    second = console_connect_options(other, "other-app", other / "native/bin/meerkat-connect",
        is_current=lambda: True)["connect_factory"](login())
    assert first.bootstrap["realm"] != second.bootstrap["realm"]
    assert first.bootstrap["application"]["application_id"] != second.bootstrap["application"]["application_id"]


def test_selected_provider_only_no_ambient_credentials_and_each_bridge_has_its_own_copy(installed, monkeypatch):
    for name in ("HOME", "PATH", "ANTHROPIC_API_KEY", "AZURE_OPENAI_API_KEY", "HTTP_PROXY", "RKAT_CONFIG"):
        monkeypatch.setenv(name, "ambient-value-must-not-be-inherited")
    factory = options(installed)["connect_factory"]
    first, second = factory(login()), factory(login(login_scope_id="login-two"))
    assert first.environment == second.environment == {"HOME": str(installed / ".mobkit/connect/user"),
        "PATH": "", "OPENAI_API_KEY": "selected-private-fixture"}
    first.environment["OPENAI_API_KEY"] = "changed-fixture"
    assert second.environment["OPENAI_API_KEY"] == "selected-private-fixture"


@pytest.mark.parametrize("key", [None, ""])
def test_missing_key_does_not_fall_back_to_ambient_or_other_auth(installed, monkeypatch, key):
    monkeypatch.setenv("OPENAI_API_KEY", "ambient-key-must-not-be-inherited")
    bridge = options(installed, provider_key=key)["connect_factory"](login())
    assert bridge.environment == {"HOME": str(installed / ".mobkit/connect/user"), "PATH": ""}
    assert "auth_binding" not in bridge.bootstrap


@pytest.mark.parametrize("which", ["login", "application"])
def test_both_owners_fence_a_previously_created_bridge(installed, which):
    active = {"login": True, "application": True}
    factory = options(installed, is_current=lambda: active["application"])["connect_factory"]
    owner = login(is_current=lambda: active["login"])
    bridge = factory(owner)
    assert bridge._authorized()
    active[which] = False
    assert not bridge._authorized()
    with pytest.raises(ConnectError) as error:
        factory(owner)
    assert error.value.code == "invalid_binding"
    assert bridge._process is None


def test_owner_failure_refuses_with_fixed_error_without_exposing_source(installed):
    def changed_source():
        raise RuntimeError("private fixture detail")
    factory = options(installed, is_current=changed_source)["connect_factory"]
    with pytest.raises(ConnectError) as error:
        factory(login())
    assert error.value.code == "invalid_binding"
    assert "private fixture detail" not in str(error.value)


def test_expired_actual_login_is_not_authorized(installed):
    bridge = options(installed)["connect_factory"](login(expires_at_ms=1))
    assert not bridge._authorized()


@pytest.mark.parametrize("origin", ["http://localhost:4510", "https://127.0.0.1:4510", "http://127.0.0.1:4510/", "http://example.invalid"])
def test_browser_or_noncanonical_origin_cannot_become_bootstrap(installed, origin):
    with pytest.raises(ValueError):
        options(installed)["connect_factory"](login(origin=origin))


def test_public_presentation_is_frozen_and_no_private_bootstrap_is_embedded(installed):
    selected = options(installed)
    with pytest.raises(TypeError):
        selected["console_assets"]["other"] = b"bad"
    original = selected["console_assets"]["/console/forge/assets/console_shell.mjs"][0]
    (installed / "console_shell.mjs").write_text("different public file")
    assert selected["console_assets"]["/console/forge/assets/console_shell.mjs"][0] == original
    native = (TEMPLATES.parents[1] / "tests/fixtures/console_shell/native-console.html").read_bytes()
    transformed = selected["document_transform"](native)
    assert b"selected-private-fixture" not in transformed and b"fixture-reader" not in transformed
    assert set(selected) == {"connect_factory", "console_assets", "document_transform"}


def test_flat_or_relative_component_is_refused(installed):
    for binary in (installed / "meerkat-connect", Path("bin/meerkat-connect")):
        with pytest.raises(ValueError):
            console_connect_options(installed, "reading", binary, is_current=lambda: True)


def application(root):
    host = {"protocol": 1, "host_id": "forge:fixture", "application_id": "reading-with-telegram",
            "source_digest": "fixture-public-source"}
    context = SimpleNamespace(root=root, deployment={"slug": "reading-with-telegram"},
                              runtime=object(), stop_event=asyncio.Event())
    return SimpleNamespace(context=context, integration=SimpleNamespace(), frontend_host=lambda: dict(host), host=host)


@pytest.mark.parametrize("setup", [False, True])
async def test_actual_normal_startup_always_composes_base_but_only_selected_setup_checks_frontend(installed, monkeypatch, setup):
    # Execute the actual Console startup branch, excluding gateways, storage,
    # signals and application lifecycle. This is not a full run() trial.
    tree = ast.parse((TEMPLATES / "app.py").read_text())
    branch = next(node for node in ast.walk(tree) if isinstance(node, ast.If)
        and isinstance(node.test, ast.Name) and node.test.id == "console_front"
        and any(isinstance(child, ast.ImportFrom) and child.module == "console_presentation"
                for child in node.body))
    function = ast.AsyncFunctionDef(name="startup", args=ast.arguments(
        posonlyargs=[], args=[ast.arg(arg="console_front"), ast.arg(arg="application")],
        kwonlyargs=[], kw_defaults=[], defaults=[]), body=[copy.deepcopy(branch)], decorator_list=[])
    source = ast.fix_missing_locations(ast.Module(body=[function], type_ignores=[]))
    namespace = {"ROOT": installed, "os": os, "application_startup": None}
    exec(compile(source, str(TEMPLATES / "app.py"), "exec"), namespace)
    monkeypatch.setitem(sys.modules, "console_presentation", console_presentation)
    monkeypatch.setitem(sys.modules, "console_binding", console_binding)
    monkeypatch.setenv("OPENAI_API_KEY", "selected-private-fixture")
    if setup:
        monkeypatch.setenv("MOBKIT_CONNECT_BIN", str(installed / "native/bin/meerkat-connect"))
    else:
        monkeypatch.delenv("MOBKIT_CONNECT_BIN", raising=False)
    owner = application(installed)
    owner.context.runtime = SimpleNamespace(rust_http_base_url="http://127.0.0.1:61347")
    runtime = owner.context.runtime
    namespace["runtime"] = runtime
    host_calls = []
    original_host = owner.frontend_host
    def frontend_host():
        host_calls.append(True)
        if not setup:
            raise ValueError("No frontend support was declared.")
        return original_host()
    owner.frontend_host = frontend_host
    starts = []
    async def start(url):
        assert front.console_assets and callable(front.document_transform)
        starts.append(url)
    front = SimpleNamespace(connect_factory=None, console_assets=None, document_transform=None, start=start)
    await namespace["startup"](front, owner)
    assert starts == [runtime.rust_http_base_url]
    native = b'<html><head></head><body><div id="root"></div></body></html>'
    result = front.document_transform(native)
    assert b'console_shell.mjs' in result and b'console_shell.css' in result
    if setup:
        assert host_calls == [True] and callable(front.connect_factory)
        assert "model" not in front.connect_factory(login()).bootstrap
        assert b'loadSetup: async' in result and b'toolkit-ui/index.js' in result
    else:
        assert host_calls == [] and front.connect_factory is None
        assert len(front.console_assets) == 2
        assert b'console_setup' not in result and b'toolkit-ui' not in result
    assert not (installed / ".mobkit").exists()


@pytest.mark.parametrize("change", ["source", "runtime", "stop"])
def test_actual_app_owner_and_exact_runtime_fence_created_bridge(installed, change):
    owner = application(installed)
    selected = application_console_options(owner, installed / "native/bin/meerkat-connect",
                                          provider_key="selected-private-fixture")
    bridge = selected["connect_factory"](login())
    assert bridge._authorized()
    assert "model" not in bridge.bootstrap
    if change == "source":
        owner.host["source_digest"] = "successor-source"
    elif change == "runtime":
        owner.context.runtime = object()
    else:
        owner.context.stop_event.set()
    assert not bridge._authorized()
    with pytest.raises(ConnectError):
        selected["connect_factory"](login())


@pytest.mark.parametrize("kind", ["not-started", "stopping", "wrong-app", "undeclared"])
def test_component_selection_does_not_invent_loaded_frontend_support(installed, kind):
    owner = application(installed)
    if kind == "not-started":
        owner.context.runtime = None
    elif kind == "stopping":
        owner.context.stop_event.set()
    elif kind == "wrong-app":
        owner.host["application_id"] = "other-app"
    else:
        def undeclared():
            raise ValueError("Frontend support undeclared by application owner.")
        owner.frontend_host = undeclared
    with pytest.raises(ValueError):
        application_console_options(owner, installed / "native/bin/meerkat-connect")


def test_service_selection_is_separate_host_input_and_never_starts_or_falls_back(installed):
    auth = {"realm": "selected-provider-realm", "binding": "selected-native-binding"}
    selected = console_service_options(installed, "reading-with-telegram",
        installed / "service-native/bin/meerkat-connect",
        resource_manifest_sha256="ab" * 32, is_current=lambda: True, auth_binding=auth)
    auth["binding"] = "later-caller-mutation"
    first = selected["connect_factory"](login())
    second = selected["connect_factory"](login(login_scope_id="login-two"))
    assert first.bootstrap["protocol_version"] == 3
    assert first.bootstrap["adapter"] == "forge_services"
    assert first.bootstrap["services"] == {"resource_manifest_sha256": "ab" * 32,
                                           "preparation": "permitted"}
    assert first.bootstrap["auth_binding"]["binding"] == "selected-native-binding"
    assert "model" not in first.bootstrap
    assert first.bootstrap["viewer"] != second.bootstrap["viewer"]
    assert first.bootstrap["application"] == second.bootstrap["application"]
    assert first.environment == {"HOME": str(installed / ".mobkit/connect/user"), "PATH": ""}
    assert first._process is None and first._start_task is None
    assert not (installed / ".mobkit").exists()
    legacy = options(installed)["connect_factory"](login())
    assert legacy.bootstrap["protocol_version"] == 1
    assert "adapter" not in legacy.bootstrap and "services" not in legacy.bootstrap


@pytest.mark.parametrize("digest", [None, "", "ab" * 31, "AB" * 32,
                                  "../../resource", "https://example.invalid/resource", True])
def test_service_selection_refuses_untrusted_resource_shape_before_composition(installed, digest):
    with pytest.raises(ValueError):
        console_service_options(installed, "reading", installed / "native/bin/meerkat-connect",
            resource_manifest_sha256=digest, is_current=lambda: True)


@pytest.mark.parametrize("auth", [{"realm": "owner"}, {"realm": "owner", "binding": ""},
    {"realm": "owner", "binding": "native", "key": "not-a-bootstrap-secret"},
    {"realm": "owner", "binding": "native", "profile": None}])
def test_service_selection_does_not_accept_a_secret_or_invent_a_native_binding(installed, auth):
    with pytest.raises(ValueError):
        console_service_options(installed, "reading", installed / "native/bin/meerkat-connect",
            resource_manifest_sha256="ab" * 32, is_current=lambda: True, auth_binding=auth)


def test_service_effect_selection_retains_both_application_and_login_fences(installed):
    active = {"application": True, "login": True}
    selected = console_service_options(installed, "reading", installed / "native/bin/meerkat-connect",
        resource_manifest_sha256="ab" * 32, is_current=lambda: active["application"])
    owner = login(is_current=lambda: active["login"])
    bridge = selected["connect_factory"](owner)
    active["application"] = False
    assert not bridge._authorized()
    with pytest.raises(ConnectError):
        selected["connect_factory"](owner)
    active["application"] = True
    active["login"] = False
    assert not bridge._authorized()
    with pytest.raises(ConnectError):
        selected["connect_factory"](owner)
