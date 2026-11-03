"""Optional host integration absence is distinct from a broken installed module."""
import builtins
from types import SimpleNamespace
from unittest.mock import Mock

import pytest

from launchkit.bootstrap import GatewayInstallError
from launchkit.remote_protected_worker import RemoteProtectedWorker
from launchkit.role_runtime import PreparedRuntime


@pytest.mark.asyncio
@pytest.mark.parametrize("kind,module", [("worker", "workspace_worker_client"),
    ("runtime", "workspace_runtime_admission_client")])
@pytest.mark.parametrize("missing", ["selected-module", "internal-dependency"])
async def test_missing_optional_host_module_refuses_before_owner_invocation(monkeypatch, kind, module, missing):
    original_import = builtins.__import__
    absent = module if missing == "selected-module" else "unrelated_internal_dependency"
    def importing(name, *args, **kwargs):
        if name == module:
            raise ModuleNotFoundError("inert missing module", name=absent)
        return original_import(name, *args, **kwargs)
    monkeypatch.setattr(builtins, "__import__", importing)
    owner = Mock()
    error = GatewayInstallError if missing == "selected-module" else ModuleNotFoundError
    with pytest.raises(error) as caught:
        if kind == "worker":
            RemoteProtectedWorker(owner)
        else:
            value = PreparedRuntime(SimpleNamespace(preparer=owner), "architect", None, None)
            await value._prepare()
    owner.assert_not_called()
    if missing == "internal-dependency":
        assert caught.value.name == absent
    else:
        assert "host-supplied" in str(caught.value)
        assert isinstance(caught.value.__cause__, ModuleNotFoundError)


def test_ordinary_guide_pin_does_not_select_a_borrowed_channel():
    from launchkit import guide_sdk
    from launchkit.guide_transport import SelectedGuideTransport
    from launchkit.protected_startup import SelectedProtectedRuntime
    factory = Mock()
    fields = dict(version=guide_sdk.SELECTED_VERSION, core_version=guide_sdk.declared_core_version(),
        meta_source="a" * 40, core_source="b" * 40,
        _model_route={"provider": "openai", "model": "gpt-6.1-sol"})
    assert guide_sdk.SELECTED_VERSION == "0.9.3"
    with pytest.raises(GatewayInstallError):
        SelectedGuideTransport(factory=factory, **fields)
    with pytest.raises(GatewayInstallError):
        SelectedProtectedRuntime(**fields)
    factory.assert_not_called()


def test_selected_receiver_refuses_an_sdk_without_the_public_transport_api():
    from meerkat_meta import Meta
    from launchkit import receiver_sdk
    from launchkit.guide_transport import SelectedGuideTransport
    builder = Meta.builder()
    # This negative case targets the ordinary Guide SDK. A transport-capable
    # SDK is exercised by its separate owner qualification, not this refusal.
    if callable(getattr(builder, "gateway_transport", None)):
        builder = object()
    factory = Mock()
    selected = SelectedGuideTransport(factory, receiver_sdk.SELECTED_VERSION,
        receiver_sdk.declared_core_version(), "a" * 40, "b" * 40,
        {"provider": "openai", "model": "gpt-6.1-sol"})
    with pytest.raises(GatewayInstallError, match="does not support borrowed"):
        selected.configure_builder(builder)
    assert selected._bound
    factory.assert_not_called()
