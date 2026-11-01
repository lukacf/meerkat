"""Original transport selection boundaries; no native gateway is launched."""
import asyncio
from dataclasses import replace
import pickle
from unittest.mock import Mock

import pytest

from launchkit import bootstrap, guide_runtime, receiver_sdk, guide_transport, planner
from launchkit.guide_transport import SelectedGuideTransport


@pytest.fixture
def selected(monkeypatch):
    value = SelectedGuideTransport(
        Mock(name="original_factory"), receiver_sdk.SELECTED_VERSION,
        receiver_sdk.declared_core_version(), "a" * 40, "b" * 40,
        {"provider": "openai", "model": "gpt-6.1-sol"},
    )
    monkeypatch.setattr(bootstrap, "_selected_guide_runtime", None)
    bootstrap.configure_selected_guide_runtime(value)
    return value


class Builder:
    """Only the host transport interface is available, no local gateway path."""
    def __init__(self):
        self.factories = []

    def gateway_transport(self, factory):
        self.factories.append(factory)
        return self


def test_original_factory_is_lazy_and_bound_to_one_builder(selected):
    builder = Builder()
    assert guide_runtime.configure_builder(builder, selected) is builder
    assert builder.factories == [selected.factory]
    selected.factory.assert_not_called()
    assert bootstrap.selected_guide_gateway() is None
    for other in (builder, Builder()):
        with pytest.raises(bootstrap.GatewayInstallError, match="one builder"):
            guide_runtime.configure_builder(other, selected)
    assert builder.factories == [selected.factory]


@pytest.mark.parametrize("missing_api", [False, True])
def test_configuration_failure_cannot_retry_or_fall_back(selected, missing_api):
    class RefusedBuilder(Builder):
        def gateway_transport(self, factory):
            raise RuntimeError("SDK refused fixture transport")
    first = object() if missing_api else RefusedBuilder()
    with pytest.raises(bootstrap.GatewayInstallError if missing_api else RuntimeError):
        guide_runtime.configure_builder(first, selected)
    with pytest.raises(bootstrap.GatewayInstallError, match="one builder"):
        guide_runtime.configure_builder(Builder(), selected)
    selected.factory.assert_not_called()


def test_receiver_change_refuses_before_touching_inherited_mutex(selected, monkeypatch):
    with selected._binding_lock:
        monkeypatch.setattr(guide_transport.os, "getpid", lambda: selected._pid + 1)
        with pytest.raises(bootstrap.GatewayInstallError, match="receiver processes"):
            guide_runtime.configure_builder(Builder(), selected)
    selected.factory.assert_not_called()


@pytest.mark.parametrize("change", ["factory", "route"])
def test_changed_owner_or_route_refuses_before_builder_admission(selected, change):
    if change == "factory":
        object.__setattr__(selected, "factory", Mock(name="replacement"))
    else:
        selected._model_route["model"] = "changed"
    builder = Builder()
    with pytest.raises(bootstrap.GatewayInstallError):
        guide_runtime.configure_builder(builder, selected)
    assert builder.factories == []
    selected.factory.assert_not_called()


def test_selection_cannot_be_replaced_by_another_factory_or_saved_record(selected):
    assert bootstrap.configure_selected_guide_runtime(selected) is selected
    with pytest.raises(bootstrap.GatewayInstallError, match="cannot be replaced"):
        bootstrap.configure_selected_guide_runtime(replace(selected, factory=Mock()))
    with pytest.raises(bootstrap.GatewayInstallError, match="saved worker record"):
        bootstrap.restore_selected_guide_context({})
    assert bootstrap.selected_guide_runtime() is selected


def test_worker_capture_and_pickle_cannot_serialize_the_owner(selected, tmp_path):
    with pytest.raises(TypeError, match="cannot be serialized"):
        pickle.dumps(selected)
    with pytest.raises(bootstrap.GatewayInstallError, match="own trusted original gateway channel"):
        guide_runtime.capture_worker_runtime(tmp_path)
    assert not list(tmp_path.iterdir())
    selected.factory.assert_not_called()


@pytest.mark.parametrize("extra", [
    {"auth_binding": "fixture"}, {"meerkat_config_path": "/never/read"},
    {"provider": ""}, {"model": "\x00"},
])
def test_only_public_model_route_enters_restricted_selection(selected, extra):
    with pytest.raises(bootstrap.GatewayInstallError, match="public provider and model route"):
        replace(selected, _model_route={**selected.model_configuration, **extra})
    assert bootstrap.selected_guide_runtime() is selected


@pytest.mark.parametrize("changes", [
    {"factory": None}, {"version": "0.0.0"}, {"core_version": "0.0.0"},
    {"meta_source": "not-a-commit"}, {"core_source": "not-a-commit"},
])
def test_invalid_sdk_tuple_or_factory_refuses(selected, changes):
    with pytest.raises(bootstrap.GatewayInstallError, match="selected application SDK"):
        replace(selected, **changes)


def test_route_reads_are_copies_and_declaration_is_checked(selected, monkeypatch):
    route = guide_runtime.selected_model_configuration()
    route["model"] = "changed"
    assert selected.model_configuration["model"] == "gpt-6.1-sol"
    monkeypatch.setattr(receiver_sdk, "SELECTED_VERSION", "0.9.6")
    monkeypatch.setattr(receiver_sdk, "CORE_REQUIREMENT", "=0.9.7")
    assert replace(selected, version="0.9.6", core_version="0.9.7").core_version == "0.9.7"
    with pytest.raises(bootstrap.GatewayInstallError):
        replace(selected, version="0.9.6", core_version="0.9.6")


def test_planner_requires_role_admission_before_consuming_startup_factory(selected, monkeypatch):
    from meerkat_meta import Meta
    from launchkit import role_runtime
    monkeypatch.setattr(role_runtime, "_preparations", None)
    builder = Mock()
    monkeypatch.setattr(Meta, "builder", builder)
    for name in planner.ENV_KEYS.values():
        monkeypatch.delenv(name, raising=False)
    with pytest.raises(RuntimeError, match="per-role preparer"):
        asyncio.run(planner.design_team(planner.DesignRequest(
            description="Help organize household notes and make practical project plans.")))
    builder.assert_not_called()
    selected.factory.assert_not_called()


def test_visual_critic_refuses_before_consuming_factory_without_native_generation_proof(selected, tmp_path):
    from launchkit.visual_review_runtime import VisualReviewRuntime
    critic = VisualReviewRuntime(tmp_path / "critic")
    critic.reviews = object()
    assert critic.capability()["available"] is False
    with pytest.raises(ValueError, match="unavailable"):
        asyncio.run(critic.get_runtime())
    assert not critic.root.exists()
    builder = Builder()
    guide_runtime.configure_builder(builder, selected)
    assert builder.factories == [selected.factory], "refusing critic never consumes the original factory"
    selected.factory.assert_not_called()
