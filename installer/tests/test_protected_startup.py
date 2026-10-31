"""Protected startup metadata cannot stand in for an admitted native runtime."""
from dataclasses import replace
import pickle
from unittest.mock import Mock

import pytest

from launchkit import bootstrap, receiver_sdk, guide_runtime, planner, role_runtime
from launchkit.protected_startup import SelectedProtectedRuntime


def selection(**changes):
    fields = dict(version=receiver_sdk.SELECTED_VERSION, core_version=receiver_sdk.declared_core_version(),
        meta_source='a' * 40, core_source='b' * 40,
        _model_route={'provider': 'openai', 'model': 'gpt-6.1-sol'})
    fields.update(changes)
    return SelectedProtectedRuntime(**fields)


@pytest.fixture
def selected(monkeypatch):
    monkeypatch.setattr(bootstrap, '_selected_guide_runtime', None)
    monkeypatch.setattr(role_runtime, '_preparations', None)
    monkeypatch.setattr(role_runtime, '_sealed', False)
    value = selection()
    bootstrap.configure_selected_guide_runtime(value)
    return value


def test_original_startup_has_only_metadata_and_preserves_catalog_and_model_route(selected):
    from launchkit.native_models import model_registry_lines
    assert bootstrap.selected_guide_runtime() is selected
    assert bootstrap.selected_guide_gateway() is None
    assert guide_runtime.selected_model_configuration() == {'provider': 'openai', 'model': 'gpt-6.1-sol'}
    assert planner.planner_capability()['available'] is True
    assert model_registry_lines([('openai', 'gpt-6.1-sol')], guide_runtime=selected) == []
    assert not any(hasattr(selected, name) for name in ('gateway', 'factory', 'persistent_state', 'close'))
    assert role_runtime._preparations is None


@pytest.mark.parametrize('operation', ['builder', 'state', 'export', 'restore'])
def test_metadata_cannot_bypass_role_admission_or_worker_custody(selected, operation):
    builder = Mock()
    with pytest.raises(bootstrap.GatewayInstallError):
        if operation == 'builder': guide_runtime.configure_builder(builder, selected)
        elif operation == 'state': guide_runtime.worker_persistent_state(selected, '/absent/state')
        elif operation == 'export': bootstrap.export_selected_guide_context()
        else: bootstrap.restore_selected_guide_context({})
    assert builder.mock_calls == []
    assert bootstrap.selected_guide_runtime() is selected


@pytest.mark.asyncio
async def test_absent_hook_refuses_before_builder_or_any_gateway_allocation(selected, monkeypatch):
    from meerkat_meta import Meta
    builder = Mock()
    monkeypatch.setattr(Meta, 'builder', builder)
    with pytest.raises(RuntimeError, match='per-role preparer'):
        await planner.design_team(planner.DesignRequest(description='Organize household project notes.'))
    builder.assert_not_called()


@pytest.mark.asyncio
async def test_metadata_keeps_critic_unavailable_before_admission(selected, tmp_path):
    from launchkit.visual_review_runtime import VisualReviewRuntime
    prepare = Mock()
    role_runtime.select_runtime_preparer(prepare)
    critic = VisualReviewRuntime(tmp_path / 'critic')
    critic.reviews = object()
    assert critic.capability()['available'] is False
    with pytest.raises(ValueError, match='unavailable'): await critic.get_runtime()
    prepare.assert_not_called()
    assert not critic.root.exists()


@pytest.mark.parametrize('changes', [
    {'version': '0.0.0'}, {'core_version': '0.0.0'}, {'meta_source': 'unknown'},
    {'core_source': 'g' * 40}, {'meta_source': None},
    {'_model_route': {'provider': 'openai', 'model': ''}},
    {'_model_route': {'provider': 'openai', 'model': 'x', 'auth_binding': 'forbidden'}},
    {'_model_route': {'provider': 'openai', 'model': 'x', 'meerkat_config_path': '/private'}},
    {'_model_route': {'provider': False, 'model': 'x'}},
])
def test_startup_keeps_exact_versions_sources_and_public_route(changes):
    with pytest.raises(bootstrap.GatewayInstallError): selection(**changes)


@pytest.mark.parametrize('name', ['version', 'core_version', 'meta_source', 'core_source', '_model_route'])
def test_selected_tuple_tamper_refuses(selected, name):
    object.__setattr__(selected, name,
        {'provider': 'openai', 'model': 'changed'} if name == '_model_route' else 'changed')
    with pytest.raises(bootstrap.GatewayInstallError, match='tuple changed'):
        selected.recheck()


def test_input_and_returned_routes_are_copies_and_selection_cannot_be_replaced(selected):
    route = {'provider': 'openai', 'model': 'gpt-6.1-sol'}
    value = selection(_model_route=route)
    route['model'] = 'changed'
    read = value.model_configuration; read['provider'] = 'changed'
    assert value.model_configuration == selected.model_configuration
    with pytest.raises(bootstrap.GatewayInstallError, match='cannot be replaced'):
        bootstrap.configure_selected_guide_runtime(value)
    with pytest.raises(TypeError): pickle.dumps(selected)
    with pytest.raises(TypeError): selection(factory=Mock())


def test_foreign_receiver_refuses_without_inventing_a_gateway(selected, monkeypatch):
    from launchkit import protected_startup
    monkeypatch.setattr(protected_startup.os, 'getpid', lambda: selected._pid + 1)
    with pytest.raises(bootstrap.GatewayInstallError, match='original receiver process'):
        selected.recheck()
