from types import SimpleNamespace
from unittest.mock import AsyncMock
import asyncio
import json
import os
import tomllib

import pytest
from fastapi.testclient import TestClient

from launchkit import server
from launchkit import architect as architect_module
from launchkit.architect import ArchitectService, ArchitectUnavailable, setup_runtime_root
from launchkit.provider_settings import ProviderSettings
from launchkit.templates import storage_setup
from test_provider_settings import native_spec
from test_architect_startup_recovery import FakeRuntime, startup

from test_meta_guide_runtime import selected as selected_guide_runtime


@pytest.fixture
def api(tmp_path, monkeypatch):
    root = tmp_path / 'private-state'
    root.mkdir(mode=0o700)
    settings = ProviderSettings(root, environ={})
    architect = ArchitectService(root)
    architect.provider_settings = settings
    architect.visualizer = SimpleNamespace(lock=asyncio.Lock(), pending=None)
    monkeypatch.setattr(server, 'STUDIO_ROOT', root)
    monkeypatch.setattr(server, 'provider_settings', settings)
    monkeypatch.setattr(server, 'architect', architect)
    for name in ('guide_operations', 'visualization_operations', 'visualization_reviews',
                 'implementation', 'rehearsal', 'deployment', 'remote_deployment', 'gcp_provisioning'):
        monkeypatch.setattr(server, name, SimpleNamespace(tasks={}, processes={}))
    monkeypatch.setattr(server, 'visual_review_runtime', SimpleNamespace(runtime=None, lock=asyncio.Lock()))
    monkeypatch.setattr(server, 'toolkit_saved_requests', SimpleNamespace(tasks={}, setups={}))
    monkeypatch.setattr(server, 'planner_capability', lambda: {'provider': settings.status()['effective_provider']})
    return TestClient(server.app), settings, architect, root


def test_private_save_public_presence_and_no_native_start(api):
    client, settings, architect, root = api
    secret = 'synthetic-private-provider-key'
    response = client.put('/api/provider-settings', json={'provider': 'openai', 'api_key': secret})
    assert response.status_code == 200
    assert secret not in response.text
    assert response.json()['effective_provider'] == 'openai'
    assert response.json()['runtime_active'] is False
    assert response.json()['runtime_active'] is False
    assert architect.runtime is None and architect.state == 'stopped'
    assert root.joinpath('provider-settings.json').stat().st_mode & 0o777 == 0o600
    restored = ProviderSettings(root, environ={}).restore()
    assert restored['providers'][0]['source'] == 'saved'
    assert secret not in json.dumps(restored)


@pytest.mark.parametrize('body', [
    {'provider': 'synthetic-private-field', 'api_key': 'synthetic-private-key'},
    {'provider': 'openai', 'api_key': ['synthetic-private-key']},
    {'provider': 'openai', 'api_key': 'synthetic-private-key', 'synthetic-private-field': 'synthetic-private-key'},
    {'provider': 'openai', 'api_key': 'synthetic-private-key\n'},
])
def test_invalid_private_fields_never_echo(api, body):
    client, _, _, root = api
    response = client.put('/api/provider-settings', json=body)
    assert response.status_code == 422
    assert 'synthetic-private' not in response.text
    assert not root.joinpath('provider-settings.json').exists()


@pytest.mark.parametrize('owner', ['guide_operations', 'visualization_operations', 'visualization_reviews',
    'implementation', 'rehearsal', 'deployment', 'remote_deployment', 'gcp_provisioning',
    'toolkit_saved_requests'])
def test_owned_work_refuses_save_and_stop(api, owner):
    client, _, architect, root = api
    getattr(server, owner).tasks['current'] = object()
    assert client.put('/api/provider-settings', json={'provider': 'openai', 'api_key': 'fixture'}).status_code == 409
    assert client.post('/api/architect/stop').status_code == 409
    assert architect.runtime is None
    assert not root.joinpath('provider-settings.json').exists()


def test_retained_toolkit_worker_blocks_provider_changes_until_settled(api):
    client, settings, architect, root = api
    settings.save('openai', 'synthetic-original-key')
    saved = root / 'provider-settings.json'
    before = saved.read_bytes()
    architect.stop_for_provider = AsyncMock()
    server.toolkit_saved_requests.setups['retained'] = object()
    assert client.get('/api/provider-settings').json()['work_active'] is True
    assert client.put('/api/provider-settings', json={
        'provider': 'openai', 'api_key': 'synthetic-replacement-key'}).status_code == 409
    assert client.post('/api/architect/stop').status_code == 409
    assert saved.read_bytes() == before
    architect.stop_for_provider.assert_not_awaited()
    server.toolkit_saved_requests.setups.clear()
    assert client.get('/api/provider-settings').json()['work_active'] is False
    assert client.put('/api/provider-settings', json={
        'provider': 'openai', 'api_key': 'synthetic-replacement-key'}).status_code == 200


def test_active_runtime_and_startup_refuse_save(api):
    client, _, architect, _ = api
    architect.runtime = object()
    assert client.put('/api/provider-settings', json={'provider': 'openai', 'api_key': 'fixture'}).status_code == 409
    architect.runtime = None
    asyncio.run(architect.start_lock.acquire())
    try:
        assert client.put('/api/provider-settings', json={'provider': 'openai', 'api_key': 'fixture'}).status_code == 409
        assert client.post('/api/architect/stop').status_code == 409
    finally:
        architect.start_lock.release()


def test_failed_stop_retains_owner_lock_and_refuses_followup(api):
    client, _, architect, _ = api
    runtime = SimpleNamespace(shutdown=AsyncMock(side_effect=RuntimeError('private-fixture-do-not-echo')))
    lock = SimpleNamespace(__exit__=lambda *_: pytest.fail('Failed stop must retain lock'))
    architect.runtime, architect.lock_context = runtime, lock
    response = client.post('/api/architect/stop')
    assert response.status_code == 503 and 'private-fixture' not in response.text
    assert architect.runtime is runtime and architect.lock_context is lock
    assert architect.provider_stop_failed is True
    assert client.get('/api/provider-settings').json()['work_active'] is True
    assert client.put('/api/provider-settings', json={'provider': 'openai', 'api_key': 'fixture'}).status_code == 409
    with pytest.raises(ArchitectUnavailable, match='cleanup is unconfirmed'):
        asyncio.run(architect.start(retry_failed=True))


def test_successful_idle_stop_releases_after_confirmation(api):
    client, _, architect, _ = api
    runtime = SimpleNamespace(shutdown=AsyncMock())
    released = []
    architect.runtime = runtime
    architect.lock_context = SimpleNamespace(__exit__=lambda *_: released.append(True))
    response = client.post('/api/architect/stop')
    assert response.status_code == 200
    assert architect.runtime is None and architect.state == 'stopped'
    assert released == [True]
    assert architect.provider_paused is True
    with pytest.raises(ArchitectUnavailable, match='stopped for provider settings'):
        asyncio.run(architect.start())
    assert architect.runtime is None
    assert client.get('/api/provider-settings').json()['runtime_active'] is False
    saved = client.put('/api/provider-settings', json={'provider': 'openai', 'api_key': 'fictional-corrected-key'})
    assert saved.status_code == 200 and saved.json()['runtime_active'] is False
    assert architect.provider_paused is True and architect.runtime is None
    assert architect.provider_paused is True
    with pytest.raises(ArchitectUnavailable, match='stopped for provider settings'):
        asyncio.run(architect.start())
    assert architect.runtime is None and architect.provider_paused is True


@pytest.mark.asyncio
async def test_explicit_start_reopens_a_deliberate_provider_pause(startup):
    architect, controller = startup
    controller.outcomes = [FakeRuntime()]
    architect.provider_paused = True
    with pytest.raises(ArchitectUnavailable, match='stopped for provider settings'):
        await architect.start()
    assert controller.builds == 0
    ready = await architect.start(retry_failed=True)
    assert ready['status'] == 'ready' and controller.builds == 1
    assert architect.provider_paused is False
    await architect.shutdown()


def test_cross_site_provider_write_is_refused(api):
    client, _, _, root = api
    response = client.put('/api/provider-settings', json={'provider': 'openai', 'api_key': 'fixture'},
                          headers={'Origin': 'https://other.example'})
    assert response.status_code == 403
    assert not root.joinpath('provider-settings.json').exists()


def test_saved_canonical_provider_wins_public_selection_over_environment_priority(api):
    client, settings, architect, root = api
    settings.environ.update(OPENAI_API_KEY='fictional-openai-key', ANTHROPIC_API_KEY='fictional-anthropic-key')
    database = native_spec(setup_runtime_root(root), provider='anthropic')
    before = database.read_bytes()
    architect.start = AsyncMock()
    assert settings.status()['effective_provider'] == 'openai'
    response = client.get('/api/provider-settings')
    assert response.status_code == 200
    public = response.json()
    assert public['locked_provider'] == public['effective_provider'] == 'anthropic'
    assert public['provider_binding_error'] is None
    assert 'fictional' not in response.text
    rejected = client.put('/api/provider-settings', json={'provider': 'openai', 'api_key': 'fictional-new-key'})
    assert rejected.status_code == 409
    assert not root.joinpath('provider-settings.json').exists()
    assert database.read_bytes() == before
    architect.start.assert_not_awaited()


def test_missing_canonical_key_is_not_claimed_configured_or_started(api, monkeypatch, selected_guide_runtime):
    from dataclasses import replace
    from launchkit import bootstrap
    monkeypatch.setattr(bootstrap, "_selected_guide_runtime", replace(selected_guide_runtime, _model_route=None))
    client, settings, architect, root = api
    settings.environ['OPENAI_API_KEY'] = 'fictional-other-provider-key'
    native_spec(setup_runtime_root(root), provider='anthropic')
    for name in ('OPENAI_API_KEY', 'ANTHROPIC_API_KEY', 'GEMINI_API_KEY', 'GOOGLE_API_KEY',
                 'RKAT_OPENAI_API_KEY', 'RKAT_ANTHROPIC_API_KEY', 'RKAT_GEMINI_API_KEY', 'RKAT_GOOGLE_API_KEY'):
        monkeypatch.delenv(name, raising=False)
    monkeypatch.setattr(architect_module, 'planner_capability', lambda: {
        'available': True, 'provider': 'openai', 'reason': 'Fictional other-provider availability'})
    response = client.get('/api/provider-settings')
    public = response.json()
    assert public['locked_provider'] == 'anthropic' and public['effective_provider'] is None
    assert next(row for row in public['providers'] if row['provider'] == 'anthropic')['configured'] is False
    started = client.post('/api/architect/start')
    assert started.status_code == 503 and 'fictional' not in started.text
    assert architect.runtime is None and architect.state == 'stopped'


def test_unverifiable_canonical_store_blocks_overwrite_and_start(api, monkeypatch):
    client, settings, architect, root = api
    settings.save('openai', 'fictional-original-key')
    private_file = root / 'provider-settings.json'
    private_before = private_file.read_bytes()
    database = native_spec(setup_runtime_root(root), mutate=lambda value:
        value['definition']['profiles']['visualizer'].update(provider='openai'))
    database_before = database.read_bytes()
    response = client.get('/api/provider-settings')
    assert response.json()['provider_binding_error']
    rejected = client.put('/api/provider-settings', json={'provider': 'openai', 'api_key': 'fictional-replacement-key'})
    assert rejected.status_code == 409 and 'fictional' not in rejected.text
    started = client.post('/api/architect/start')
    assert started.status_code == 503 and 'fictional' not in started.text
    assert private_file.read_bytes() == private_before and database.read_bytes() == database_before
    assert architect.runtime is None and architect.state == 'stopped'


@pytest.mark.asyncio
async def test_explicit_retry_restores_durable_key_before_native_build(startup, monkeypatch):
    service, controller = startup
    for name in ('OPENAI_API_KEY', 'ANTHROPIC_API_KEY', 'GEMINI_API_KEY', 'GOOGLE_API_KEY',
                 'RKAT_OPENAI_API_KEY', 'RKAT_ANTHROPIC_API_KEY', 'RKAT_GEMINI_API_KEY', 'RKAT_GOOGLE_API_KEY'):
        monkeypatch.delenv(name, raising=False)
    ProviderSettings(service.root, environ={}).save('openai', 'fictional-durable-key')
    service.provider_settings = ProviderSettings(service.root)
    service.state = 'failed'
    assert not os.environ.get('OPENAI_API_KEY')
    observations = []
    def capability():
        observations.append(os.environ.get('OPENAI_API_KEY'))
        return {'available': bool(os.environ.get('OPENAI_API_KEY')), 'provider': 'openai',
                'reason': 'Fictional provider availability'}
    monkeypatch.setattr(architect_module, 'planner_capability', capability)
    async def observe_build(builder):
        assert os.environ.get('OPENAI_API_KEY') == 'fictional-durable-key'
    controller.callback_runner = observe_build
    controller.outcomes = [FakeRuntime()]
    status = await service.start(retry_failed=True)
    assert status['status'] == 'ready' and controller.builds == 1
    assert observations and all(value == 'fictional-durable-key' for value in observations)
    assert service.provider_settings.status()['providers'][0]['source'] == 'saved'
    assert all(profile['provider'] == 'openai'
               for profile in tomllib.loads(controller.mob_configs[0])['profiles'].values())
    assert 'fictional-durable-key' not in json.dumps(status)
    await service.shutdown()


@pytest.mark.asyncio
async def test_native_build_uses_canonical_provider_despite_other_available_key(startup, monkeypatch):
    service, controller = startup
    for name in ('RKAT_OPENAI_API_KEY', 'RKAT_ANTHROPIC_API_KEY', 'RKAT_GEMINI_API_KEY', 'RKAT_GOOGLE_API_KEY'):
        monkeypatch.delenv(name, raising=False)
    monkeypatch.setenv('OPENAI_API_KEY', 'fictional-first-provider-key')
    monkeypatch.setenv('ANTHROPIC_API_KEY', 'fictional-saved-provider-key')
    runtime_root = setup_runtime_root(service.root)
    database = native_spec(runtime_root, provider='anthropic')
    for directory in (service.root, runtime_root, runtime_root / '.mobkit', database.parent):
        directory.chmod(0o700)
    storage_setup.write_private(runtime_root / 'deployment.json', {'slug': 'launch-architect', 'mobkitVersion': '0.8.47'})
    storage_setup.write_private(runtime_root / '.mobkit/launch-storage.json',
        {'format': 1, 'project': 'launch-architect', 'version': '0.8.47', 'phase': 'ready'})
    before, inode = database.read_bytes(), database.stat().st_ino
    controller.outcomes = [FakeRuntime()]
    status = await service.start()
    assert status['provider'] == 'anthropic' and controller.builds == 1
    assert all(profile['provider'] == 'anthropic'
               for profile in tomllib.loads(controller.mob_configs[0])['profiles'].values())
    assert 'fictional' not in json.dumps(status)
    assert database.read_bytes() == before and database.stat().st_ino == inode
    await service.shutdown()


@pytest.mark.asyncio
async def test_provider_pause_blocks_automatic_start_until_explicit_retry(startup, monkeypatch):
    service, controller = startup
    monkeypatch.setenv('OPENAI_API_KEY', 'fictional-existing-key')
    service.provider_paused = True
    with pytest.raises(ArchitectUnavailable, match='stopped for provider settings'):
        await service.start()
    assert controller.builders == controller.builds == 0
    assert service.provider_paused is True and service.runtime is None
    controller.outcomes = [FakeRuntime()]
    status = await service.start(retry_failed=True)
    assert status['status'] == 'ready' and service.provider_paused is False
    assert controller.builders == controller.builds == 1
    await service.shutdown()
