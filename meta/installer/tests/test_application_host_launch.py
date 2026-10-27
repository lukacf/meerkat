"""Inert launcher controls with safe suppliers and no production server import."""
import hashlib
from pathlib import Path
import inspect
import shlex
from types import SimpleNamespace

import pytest

from launchkit import application_host_launch as launch
from launchkit import connect_application_startup as startup


class SelectedHost:
    pass


@pytest.fixture
def supplier(monkeypatch):
    calls = []
    selected = SelectedHost()
    selected.original_source = object()
    selected.startup_selection = lambda guide, deployment: (guide, deployment, selected.original_source)
    app = object()
    owners = {name: object() for name in ("implementation", "deployment", "critic", "reading")}

    def factory():
        calls.append(("factory", selected))
        return selected

    def select(value):
        calls.append(("select", value))

    modules = {
        "trusted_operator.selection": SimpleNamespace(make_host=factory),
        "launchkit.forge_application_host": SimpleNamespace(ForgeApplicationHost=SelectedHost),
        "launchkit.runtime_selection": SimpleNamespace(MetaToolkitRuntimeTarget=type("SelectedTarget", (), {})),
        "launchkit.server": SimpleNamespace(app=app, select_application_host=select, **owners),
    }

    def load(name):
        calls.append(("import", name))
        if name not in modules:
            raise AssertionError("Unexpected production import: " + name)
        return modules[name]

    monkeypatch.setenv(launch.FACTORY_ENV, "trusted_operator.selection:make_host")
    monkeypatch.setattr(launch.importlib, "import_module", load)
    return SimpleNamespace(calls=calls, selected=selected, app=app, owners=owners, modules=modules)


def test_selects_original_host_once_before_returning_original_app(supplier):
    original_callback = supplier.selected.startup_selection
    assert launch.create_app() is supplier.app
    assert supplier.calls == [
        ("import", "trusted_operator.selection"),
        ("import", "launchkit.forge_application_host"),
        ("import", "launchkit.runtime_selection"),
        ("factory", supplier.selected),
        ("import", "launchkit.server"),
        ("select", supplier.selected),
    ]
    assert supplier.selected.startup_selection is original_callback
    assert original_callback("guide", "deployment") == ("guide", "deployment", supplier.selected.original_source)
    for name, owner in supplier.owners.items():
        assert getattr(supplier.modules["launchkit.server"], name) is owner


def test_ordinary_meta_selection_forwards_original_gateway_and_sdk_only(supplier, tmp_path):
    from launchkit.connect_installer_caller import ExactFile
    from launchkit.runtime_candidates import MetaToolkitRuntimeTarget
    # Constructor type composition only; no artifact reader/native operation
    # runs in this launcher test. Registry controls verify actual inert bytes.
    target = object.__new__(MetaToolkitRuntimeTarget)
    gateway = ExactFile(tmp_path / "ordinary-rpc", "a" * 64)
    selected = launch.OrdinaryMetaApplicationSelection(target, gateway)
    supplier.modules["trusted_operator.selection"].make_host = lambda: selected
    forwarded = []
    supplier.modules["launchkit.server"].select_local_application_runtime = (
        lambda value, **options: forwarded.append((value, options)))
    assert launch.create_app() is supplier.app
    assert len(forwarded) == 1 and forwarded[0][0] is target
    assert forwarded[0][1] == {"ordinary_meta_gateway": gateway}
    assert forwarded[0][1]["ordinary_meta_gateway"] is gateway


@pytest.mark.parametrize("gateway", [None, "/tmp/unselected-gateway", {"path": "/tmp/gateway", "sha256": "a" * 64}])
def test_ordinary_selection_rejects_untyped_runtime_or_gateway(gateway):
    from launchkit.runtime_candidates import MetaToolkitRuntimeTarget
    target = object.__new__(MetaToolkitRuntimeTarget)
    with pytest.raises(TypeError, match="exact ordinary Meta"):
        launch.OrdinaryMetaApplicationSelection(target, gateway)


def test_ordinary_application_retains_explicit_model_owner_without_selecting_guide(supplier, tmp_path, monkeypatch):
    from launchkit.connect_installer_caller import ExactFile
    from launchkit.runtime_candidates import MetaToolkitRuntimeTarget
    owner = model_owner(tmp_path, monkeypatch)
    target = object.__new__(MetaToolkitRuntimeTarget)
    gateway = ExactFile(tmp_path / "ordinary-rpc", "a" * 64)
    chosen = launch.OrdinaryMetaApplicationSelection(target, gateway, owner)
    supplier.modules["trusted_operator.selection"].make_host = lambda: chosen
    calls = []
    supplier.modules["launchkit.server"].select_local_application_runtime = (
        lambda value, **options: calls.append((value, options)))
    assert launch.create_app() is supplier.app
    assert calls == [(target, {"ordinary_meta_gateway": gateway, "model_configuration": owner})]
    assert calls[0][1]["model_configuration"] is owner
    assert not any("guide_runtime" in str(call) for call in supplier.calls)


def test_changed_application_model_source_refuses_before_server_selection(supplier, tmp_path, monkeypatch):
    from launchkit.connect_installer_caller import ExactFile, CustodyError
    from launchkit.runtime_candidates import MetaToolkitRuntimeTarget
    owner = model_owner(tmp_path, monkeypatch)
    target = object.__new__(MetaToolkitRuntimeTarget)
    gateway = ExactFile(tmp_path / "ordinary-rpc", "a" * 64)
    chosen = launch.OrdinaryMetaApplicationSelection(target, gateway, owner)
    supplier.modules["trusted_operator.selection"].make_host = lambda: chosen
    supplier.modules["launchkit.server"].select_local_application_runtime = (
        lambda *args, **kwargs: pytest.fail("Changed source cannot reach server selection"))
    owner.host_config.path.write_text("Changed inert nonsecret model source.\n")
    with pytest.raises(CustodyError):
        launch.create_app()


@pytest.mark.parametrize("value", [{}, "/tmp/model-config", object()])
def test_application_model_configuration_needs_actual_custody_owner(tmp_path, value):
    from launchkit.connect_installer_caller import ExactFile
    from launchkit.runtime_candidates import MetaToolkitRuntimeTarget
    target = object.__new__(MetaToolkitRuntimeTarget)
    with pytest.raises(TypeError, match="original application model configuration owner"):
        launch.OrdinaryMetaApplicationSelection(target, ExactFile(tmp_path / "rpc", "a" * 64), value)


def model_owner(tmp_path, monkeypatch):
    """An inert private model source with Linux custody for this feature only.

    The launcher supplier replaces import_module, so build the owner without
    first imports of Forge services or other test modules.
    """
    directory = tmp_path / "model-owner"
    directory.mkdir(mode=0o700)
    config = directory / "host.toml"
    content = b'[model]\nselected = "inert-custody-specimen"\n'
    config.write_bytes(content)
    config.chmod(0o600)
    root = directory / "xdg"
    root.mkdir(mode=0o700)
    monkeypatch.setattr(startup, "sys", SimpleNamespace(platform="linux"))
    monkeypatch.setenv("XDG_CONFIG_HOME", str(root))
    return startup._ModelConfiguration.observe({"host_config": {"path": str(config),
        "sha256": hashlib.sha256(content).hexdigest()}, "credential_config_root": str(root)})


def test_toolkit_application_forwards_explicit_model_owner_to_local_runtime(supplier, tmp_path, monkeypatch):
    from launchkit.runtime_candidates import MetaToolkitRuntimeTarget
    owner = model_owner(tmp_path, monkeypatch)
    target = object.__new__(MetaToolkitRuntimeTarget)
    chosen = launch.ToolkitApplicationSelection(target, owner)
    supplier.modules["trusted_operator.selection"].make_host = lambda: chosen
    calls = []
    supplier.modules["launchkit.server"].select_local_application_runtime = (
        lambda value, **options: calls.append((value, options)))
    assert launch.create_app() is supplier.app
    assert calls == [(target, {"model_configuration": owner})]
    assert calls[0][0] is target and calls[0][1]["model_configuration"] is owner
    assert not any("guide_runtime" in str(call) for call in supplier.calls)


def test_bare_toolkit_target_still_selects_no_application_model_owner(supplier):
    target_type = supplier.modules["launchkit.runtime_selection"].MetaToolkitRuntimeTarget
    target = target_type()
    supplier.modules["trusted_operator.selection"].make_host = lambda: target
    calls = []
    supplier.modules["launchkit.server"].select_local_application_runtime = (
        lambda value, **options: calls.append((value, options)))
    assert launch.create_app() is supplier.app
    assert calls == [(target, {})] and calls[0][0] is target


def test_changed_toolkit_model_source_refuses_before_server_selection(supplier, tmp_path, monkeypatch):
    from launchkit.connect_installer_caller import CustodyError
    from launchkit.runtime_candidates import MetaToolkitRuntimeTarget
    owner = model_owner(tmp_path, monkeypatch)
    chosen = launch.ToolkitApplicationSelection(object.__new__(MetaToolkitRuntimeTarget), owner)
    supplier.modules["trusted_operator.selection"].make_host = lambda: chosen
    supplier.modules["launchkit.server"].select_local_application_runtime = (
        lambda *args, **kwargs: pytest.fail("Changed source cannot reach server selection"))
    owner.host_config.path.write_text("Changed inert nonsecret model source.\n")
    with pytest.raises(CustodyError):
        launch.create_app()


@pytest.mark.parametrize("defect", ["target-none", "target-lookalike", "owner-none", "owner-selection",
    "owner-path", "owner-object"])
def test_toolkit_application_needs_exact_product_and_model_owner(tmp_path, monkeypatch, defect):
    from launchkit.runtime_candidates import MetaToolkitRuntimeTarget
    owner = model_owner(tmp_path, monkeypatch)
    target = object.__new__(MetaToolkitRuntimeTarget)
    target, value = {"target-none": (None, owner),
        "target-lookalike": (SimpleNamespace(target="linux-x86_64"), owner),
        "owner-none": (target, None), "owner-selection": (target, owner.selection()),
        "owner-path": (target, str(owner.host_config.path)), "owner-object": (target, object())}[defect]
    with pytest.raises(TypeError, match="exact Meta Toolkit product and its original application model configuration owner"):
        launch.ToolkitApplicationSelection(target, value)


def test_forged_toolkit_application_refuses_before_server_selection(supplier, tmp_path, monkeypatch):
    from launchkit.runtime_candidates import MetaToolkitRuntimeTarget
    owner = model_owner(tmp_path, monkeypatch)
    chosen = launch.ToolkitApplicationSelection(object.__new__(MetaToolkitRuntimeTarget), owner)
    # A frozen value can still be forged after construction; selection rechecks it.
    object.__setattr__(chosen, "model_configuration", owner.selection())
    supplier.modules["trusted_operator.selection"].make_host = lambda: chosen
    supplier.modules["launchkit.server"].select_local_application_runtime = (
        lambda *args, **kwargs: pytest.fail("A forged selection cannot reach server selection"))
    with pytest.raises(TypeError, match="original application model configuration owner"):
        launch.create_app()


def configured_custody(monkeypatch, owner):
    """A launcher configures application model custody in this process before serving."""
    from launchkit import packaged_runtime
    for name in ("_selected_target", "_application_model_custody"):
        monkeypatch.setattr(packaged_runtime, name, None)
    packaged_runtime.configure_application_model_custody(owner)
    return owner


def other_model_owner(tmp_path, owner):
    """Another inert model source over the same credential root."""
    config = tmp_path / "other-host.toml"
    config.write_bytes(b'[model]\nselected = "another-inert-custody-specimen"\n')
    config.chmod(0o600)
    return startup._ModelConfiguration.observe({"host_config": {"path": str(config),
        "sha256": hashlib.sha256(config.read_bytes()).hexdigest()},
        "credential_config_root": owner.selection()["credential_config_root"]})


def test_bare_toolkit_target_takes_the_application_model_custody_configured_for_this_forge(
        supplier, tmp_path, monkeypatch):
    from launchkit.runtime_candidates import MetaToolkitRuntimeTarget
    owner = configured_custody(monkeypatch, model_owner(tmp_path, monkeypatch))
    supplier.modules["launchkit.runtime_selection"] = SimpleNamespace(MetaToolkitRuntimeTarget=MetaToolkitRuntimeTarget)
    target = object.__new__(MetaToolkitRuntimeTarget)
    supplier.modules["trusted_operator.selection"].make_host = lambda: target
    calls = []
    supplier.modules["launchkit.server"].select_local_application_runtime = (
        lambda value, **options: calls.append((value, options)))
    assert launch.create_app() is supplier.app
    assert calls == [(target, {"model_configuration": owner})]
    assert calls[0][0] is target and calls[0][1]["model_configuration"] is owner


def test_ordinary_selection_without_an_owner_takes_the_configured_custody(supplier, tmp_path, monkeypatch):
    from launchkit.connect_installer_caller import ExactFile
    from launchkit.runtime_candidates import MetaToolkitRuntimeTarget
    owner = configured_custody(monkeypatch, model_owner(tmp_path, monkeypatch))
    target = object.__new__(MetaToolkitRuntimeTarget)
    gateway = ExactFile(tmp_path / "ordinary-rpc", "a" * 64)
    supplier.modules["trusted_operator.selection"].make_host = (
        lambda: launch.OrdinaryMetaApplicationSelection(target, gateway))
    calls = []
    supplier.modules["launchkit.server"].select_local_application_runtime = (
        lambda value, **options: calls.append((value, options)))
    assert launch.create_app() is supplier.app
    assert calls == [(target, {"ordinary_meta_gateway": gateway, "model_configuration": owner})]
    assert calls[0][1]["model_configuration"] is owner


@pytest.mark.parametrize("same", ["same-owner", "same-selection"])
def test_selection_with_the_configured_owner_keeps_its_own(supplier, tmp_path, monkeypatch, same):
    from launchkit.runtime_candidates import MetaToolkitRuntimeTarget
    owner = configured_custody(monkeypatch, model_owner(tmp_path, monkeypatch))
    own = owner if same == "same-owner" else startup._ModelConfiguration.observe(owner.selection())
    target = object.__new__(MetaToolkitRuntimeTarget)
    supplier.modules["trusted_operator.selection"].make_host = lambda: launch.ToolkitApplicationSelection(target, own)
    calls = []
    supplier.modules["launchkit.server"].select_local_application_runtime = (
        lambda value, **options: calls.append((value, options)))
    assert launch.create_app() is supplier.app
    assert calls == [(target, {"model_configuration": own})] and calls[0][1]["model_configuration"] is own


@pytest.mark.parametrize("kind", ["toolkit", "ordinary"])
def test_selection_with_another_model_owner_refuses_the_configured_custody(supplier, tmp_path, monkeypatch, kind):
    from launchkit.connect_installer_caller import CustodyError, ExactFile
    from launchkit.runtime_candidates import MetaToolkitRuntimeTarget
    owner = configured_custody(monkeypatch, model_owner(tmp_path, monkeypatch))
    other = other_model_owner(tmp_path, owner)
    target = object.__new__(MetaToolkitRuntimeTarget)
    chosen = (launch.ToolkitApplicationSelection(target, other) if kind == "toolkit" else
        launch.OrdinaryMetaApplicationSelection(target, ExactFile(tmp_path / "ordinary-rpc", "a" * 64), other))
    supplier.modules["trusted_operator.selection"].make_host = lambda: chosen
    supplier.modules["launchkit.server"].select_local_application_runtime = (
        lambda *args, **kwargs: pytest.fail("Two application model owners cannot reach server selection"))
    with pytest.raises(CustodyError, match="differs from the model owner"):
        launch.create_app()


@pytest.mark.parametrize("kind", ["application-host", "application-host-with-matching-attribute",
    "local-and-linux-control"])
def test_selection_that_cannot_carry_the_configured_custody_refuses_it(supplier, tmp_path, monkeypatch, kind):
    from launchkit.connect_installer_caller import CustodyError
    from launchkit.remote_product_control import ProductControlSelection
    from launchkit.runtime_candidates import MetaToolkitRuntimeTarget
    owner = configured_custody(monkeypatch, model_owner(tmp_path, monkeypatch))
    if kind.startswith("application-host"):
        chosen = supplier.selected
        if kind == "application-host-with-matching-attribute":
            # An ad hoc attribute is no checked model owner.
            chosen.model_configuration = owner.selection()
    else:
        local = object.__new__(MetaToolkitRuntimeTarget)
        object.__setattr__(local, "target", "linux-x86_64")
        chosen = launch.ApplicationRuntimeSelection(local, object.__new__(ProductControlSelection))
    supplier.modules["trusted_operator.selection"].make_host = lambda: chosen
    server = supplier.modules["launchkit.server"]
    server.select_application_host = server.select_product_application_runtime = (
        lambda *args, **kwargs: pytest.fail("Configured custody cannot be dropped at server selection"))
    with pytest.raises(CustodyError, match="cannot reach the selection"):
        launch.create_app()


@pytest.mark.parametrize("value", [None, "", "module", ":factory", "module:",
    " module:factory", "module:factory ", "module:factory:other", "../owner.py:factory",
    ".owner:factory", "owner..selection:factory", "owner:object.factory", "owner:class",
    "owner:fact\u00f6ry", "owner:factory\x00"])
def test_invalid_reference_refuses_before_any_import(monkeypatch, supplier, value):
    monkeypatch.setattr(launch.os, "environ", {launch.FACTORY_ENV: value})
    with pytest.raises(ValueError, match="trusted Python module:factory"):
        launch.create_app()
    assert supplier.calls == []


def test_missing_trusted_module_does_not_import_server(monkeypatch, supplier):
    monkeypatch.setenv(launch.FACTORY_ENV, "missing_owner:make_host")
    failure = ImportError("Retained trusted owner is unavailable")

    def missing(name):
        assert name == "missing_owner"
        raise failure

    monkeypatch.setattr(launch.importlib, "import_module", missing)
    with pytest.raises(ImportError) as caught:
        launch.create_app()
    assert caught.value is failure
    assert supplier.calls == []


def test_missing_factory_does_not_import_server(supplier):
    del supplier.modules["trusted_operator.selection"].make_host
    with pytest.raises(AttributeError):
        launch.create_app()
    assert supplier.calls == [("import", "trusted_operator.selection")]


@pytest.mark.parametrize("kind", ["not_callable", "requires_argument", "async", "async_callable",
    "async_generator", "async_generator_callable"])
def test_invalid_callable_refuses_without_invocation_or_server_import(supplier, kind):
    def requires_argument(selection):
        raise AssertionError("An invalid factory must not run")

    async def asynchronous():
        raise AssertionError("An invalid factory must not run")

    class AsyncCallable:
        async def __call__(self):
            raise AssertionError("An invalid factory must not run")

    async def asynchronous_generator():
        raise AssertionError("An invalid factory must not run")
        yield

    class AsyncGeneratorCallable:
        async def __call__(self):
            raise AssertionError("An invalid factory must not run")
            yield

    supplier.modules["trusted_operator.selection"].make_host = {
        "not_callable": supplier.selected, "requires_argument": requires_argument,
        "async": asynchronous, "async_callable": AsyncCallable(),
        "async_generator": asynchronous_generator,
        "async_generator_callable": AsyncGeneratorCallable(),
    }[kind]
    with pytest.raises(TypeError, match="synchronous zero-argument factory"):
        launch.create_app()
    assert supplier.calls == [("import", "trusted_operator.selection")]


def test_sync_factory_coroutine_is_closed_without_execution_or_server_import(supplier):
    entered = []

    async def incorrectly_returned():
        entered.append("executed")
        return supplier.selected

    coroutine = incorrectly_returned()
    supplier.modules["trusted_operator.selection"].make_host = lambda: coroutine
    assert inspect.getcoroutinestate(coroutine) == inspect.CORO_CREATED
    with pytest.raises(TypeError, match="exact ForgeApplicationHost"):
        launch.create_app()
    assert inspect.getcoroutinestate(coroutine) == inspect.CORO_CLOSED
    assert entered == []
    assert ("import", "launchkit.server") not in supplier.calls


def test_factory_failure_stops_before_server_import(supplier):
    failure = RuntimeError("Original selection is no longer current")

    def stale():
        raise failure

    supplier.modules["trusted_operator.selection"].make_host = stale
    with pytest.raises(RuntimeError) as caught:
        launch.create_app()
    assert caught.value is failure
    assert ("import", "launchkit.server") not in supplier.calls


@pytest.mark.parametrize("kind", ["missing", "lookalike", "subclass"])
def test_wrong_host_type_refuses_before_server_import(supplier, kind):
    class Subclass(SelectedHost):
        pass

    value = {"missing": None, "lookalike": SimpleNamespace(require_current=lambda: None),
        "subclass": Subclass()}[kind]
    supplier.modules["trusted_operator.selection"].make_host = lambda: value
    with pytest.raises(TypeError, match="exact ForgeApplicationHost"):
        launch.create_app()
    assert ("import", "launchkit.server") not in supplier.calls


def test_existing_selector_failure_never_returns_an_unselected_app(supplier):
    failure = RuntimeError("Preserve the original selected application host")

    def retained_owner_refuses(selected):
        assert selected is supplier.selected
        raise failure

    supplier.modules["launchkit.server"].select_application_host = retained_owner_refuses
    with pytest.raises(RuntimeError) as caught:
        launch.create_app()
    assert caught.value is failure
    assert supplier.calls.count(("factory", supplier.selected)) == 1


def test_actual_start_declarations_keep_default_and_select_factory_only_when_configured():
    script = (Path(__file__).parents[1] / "start.sh").read_text()
    tail = script[script.index('echo "Keep this terminal open. Press Ctrl+C to stop Meta Forge."'):]
    lines = tail.splitlines()
    assert lines[1] == 'if [ -n "${META_FORGE_APPLICATION_HOST_FACTORY:-}" ]; then'
    assert lines[3] == "fi"
    configured = shlex.split(lines[2])
    assert lines[4] == 'if [ "$standalone_runtime" = meta-toolkit ]; then'
    packaged = shlex.split(lines[5])
    assert lines[6] == "fi"
    default = shlex.split(lines[7])
    # Requests still in flight get 20 s after a stop signal, not an unbounded wait.
    graceful = ["--timeout-graceful-shutdown", "20"]
    assert configured == ["exec", ".venv/bin/python", "-m", "uvicorn",
        "launchkit.application_host_launch:create_app", "--factory", "--host", "127.0.0.1", "--port", "$launch_port",
        *graceful]
    assert default == ["exec", ".venv/bin/python", "-m", "uvicorn", "launchkit.server:app",
        "--host", "127.0.0.1", "--port", "$launch_port", *graceful]
    assert packaged == ["exec", ".venv/bin/python", "-m", "uvicorn",
        "launchkit.packaged_runtime:create_app", "--factory", "--host", "127.0.0.1", "--port", "$launch_port",
        *graceful]
    assert len(lines) == 8
    recovery = next(line for line in script.splitlines() if line.startswith("  for setting in "))
    assert launch.FACTORY_ENV in [word.rstrip(";") for word in recovery.split()]
