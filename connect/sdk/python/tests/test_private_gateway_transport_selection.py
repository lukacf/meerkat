"""Private-bootstrap transport selection only; no child or adoption acceptance."""
import asyncio
import hashlib
from types import SimpleNamespace

import pytest

from meerkat_meta._transport import PersistentTransport
from meerkat_meta.builder import MetaBuilderConfig
from meerkat_toolkit import host_control
from meerkat_toolkit.application_runtime import ToolkitApplicationRuntime
from meerkat_toolkit.application_update import ToolkitApplicationUpdateRuntime
from meerkat_toolkit.host_control import (
    ApplicationPreparation, ApplicationUpdateStartup, HostBootstrapSelection,
    HostControlError, SelectedHostArtifact, ToolkitMobKitRuntime,
    ToolkitPersistentTransport,
)


KINDS = ("restore", "prepare", "update")
HOST_SELECTIONS = ("transport", "factory", "falsey_factory", "false")


def _selection(kind, tmp_path):
    path = tmp_path / "selected.json"
    raw = b'{"fixture":"private transport selection only"}\n'
    path.write_bytes(raw)
    artifact = SelectedHostArtifact(str(path), hashlib.sha256(raw).hexdigest())
    if kind == "prepare":
        return ApplicationPreparation(artifact, timeout=1.0)
    if kind == "update":
        return ApplicationUpdateStartup(artifact, artifact, str(tmp_path / "associations"))
    return HostBootstrapSelection(str(tmp_path), artifact, artifact, artifact)


def _runtime(kind, config, selection):
    options = {"hosted_mcp_redirect_base": "https://host.example/application"}
    if kind == "prepare":
        async def finalize(_artifact):
            pytest.fail("transport selection must not enter the application finalizer")
        return ToolkitApplicationRuntime(config, selection, finalize, **options)
    if kind == "update":
        return ToolkitApplicationUpdateRuntime(config, selection, **options)
    # This construction-only control checks that refusal does not consume the
    # existing one-shot flag. It makes no claim that native accepts the flag.
    return ToolkitMobKitRuntime(config, selection, initialize_native_owner=True, **options)


def _host_selection(kind, monkeypatch):
    calls = []

    def forbidden(action):
        def called(*_args, **_kwargs):
            calls.append(action)
            pytest.fail(f"private bootstrap touched the unclaimed host selection: {action}")
        return called

    if kind == "transport":
        # A genuine, never-started Meta transport. Constructing it spawns no
        # child. Connect must neither claim it nor dispose of the host's owner.
        selected = PersistentTransport("/inert/not-spawned")
        monkeypatch.setattr(selected, "start", forbidden("start"))
        monkeypatch.setattr(selected, "stop", forbidden("stop"))
        monkeypatch.setattr(selected, "_is_fresh", forbidden("inspect_freshness"))
        return selected, calls
    if kind == "factory":
        return forbidden("factory"), calls
    if kind == "falsey_factory":
        class FalseyFactory:
            def __bool__(self):
                calls.append("bool")
                return False

            def __call__(self):
                return forbidden("factory")()

        return FalseyFactory(), calls
    assert kind == "false"
    return False, calls


def _refuse_private_construction(monkeypatch):
    calls = []

    def forbidden(*_args, **_kwargs):
        calls.append("ToolkitPersistentTransport")
        pytest.fail("refused selection reached private transport construction")

    # The three modules import this same class. Intercept its constructor,
    # leaving the real runtime factory and Meta connect dispatch in place.
    monkeypatch.setattr(host_control.ToolkitPersistentTransport, "__init__", forbidden)
    return calls


def _attempt_state(runtime):
    names = ("_initialize_native_owner", "_application_started", "_update_started",
             "_application_closing", "preparation_attempt", "_transport",
             "_last_gateway_transport", "_shutdown_task", "_stop_pending", "_running")
    return {name: getattr(runtime, name) for name in names if hasattr(runtime, name)}


@pytest.mark.parametrize("kind", KINDS)
@pytest.mark.parametrize("host_kind", HOST_SELECTIONS)
def test_constructor_rejects_host_transport_without_claiming_it(kind, host_kind, tmp_path, monkeypatch):
    config = MetaBuilderConfig(gateway_bin="/inert/not-spawned")
    selected, host_calls = _host_selection(host_kind, monkeypatch)
    config.gateway_transport = selected
    selection = _selection(kind, tmp_path)
    private_calls = _refuse_private_construction(monkeypatch)

    with pytest.raises(HostControlError, match="does not accept gateway_transport"):
        _runtime(kind, config, selection)

    assert config.gateway_transport is selected
    assert host_calls == private_calls == []


@pytest.mark.parametrize("kind", KINDS)
@pytest.mark.parametrize("host_kind", HOST_SELECTIONS)
def test_factory_rechecks_mutated_config_before_one_shot_consumption(kind, host_kind, tmp_path, monkeypatch):
    config = MetaBuilderConfig(gateway_bin="/inert/not-spawned")
    runtime = _runtime(kind, config, _selection(kind, tmp_path))
    before = _attempt_state(runtime)
    selected, host_calls = _host_selection(host_kind, monkeypatch)
    config.gateway_transport = selected
    private_calls = _refuse_private_construction(monkeypatch)

    with pytest.raises(HostControlError, match="does not accept gateway_transport"):
        runtime._make_gateway_transport()

    assert _attempt_state(runtime) == before
    assert config.gateway_transport is selected
    assert host_calls == private_calls == []


@pytest.mark.parametrize("kind", KINDS)
@pytest.mark.parametrize("host_kind", HOST_SELECTIONS)
def test_connect_rejects_mutated_config_before_handoff_or_spawn(kind, host_kind, tmp_path, monkeypatch):
    config = MetaBuilderConfig(gateway_bin="/inert/not-spawned")
    runtime = _runtime(kind, config, _selection(kind, tmp_path))
    before = _attempt_state(runtime)
    selected, host_calls = _host_selection(host_kind, monkeypatch)
    config.gateway_transport = selected
    private_calls = _refuse_private_construction(monkeypatch)

    with pytest.raises(HostControlError, match="does not accept gateway_transport"):
        asyncio.run(runtime.connect())

    assert _attempt_state(runtime) == before
    assert config.gateway_transport is selected
    assert host_calls == private_calls == []


@pytest.mark.parametrize("kind", KINDS)
@pytest.mark.parametrize("present", [False, True])
def test_absent_or_none_selection_preserves_gateway_only_factory(kind, present, tmp_path):
    # The absent-field variant retains compatibility with earlier configs.
    config = SimpleNamespace(gateway_bin="/inert/not-spawned", gateway_init_timeout=17.0)
    if present:
        config.gateway_transport = None
    original_config = vars(config).copy()
    selection = _selection(kind, tmp_path)
    runtime = _runtime(kind, config, selection)
    transport = runtime._make_gateway_transport()

    assert isinstance(transport, ToolkitPersistentTransport)
    assert transport.gateway_bin == config.gateway_bin
    assert transport._selection is selection
    assert transport._process is None and transport.host_control is None
    assert transport._extra_args == transport._inherited_fds == ()
    assert transport.request_timeout == 60.0
    assert transport._bootstrap_timeout == (None if kind == "prepare" else 17.0)
    assert transport._hosted_mcp_redirect_base == "https://host.example/application"
    assert vars(config) == original_config
    if kind == "restore":
        assert transport._initialize_native_owner is True
        assert runtime._initialize_native_owner is False
        restored = runtime._make_gateway_transport()
        assert restored._initialize_native_owner is False and restored._process is None
    else:
        with pytest.raises(HostControlError, match="already started"):
            runtime._make_gateway_transport()
