from types import SimpleNamespace
from test_meta_guide_runtime import selected as selected_guide_runtime


import pytest
from fastapi.testclient import TestClient

from launchkit.blueprint import starter_blueprint
from launchkit.visualization_operations import VisualizationConflict


def sources():
    spec = starter_blueprint()
    return {'blueprint': spec.model_dump(mode='json'),
            'model': {'nodes': [{'id': identity, 'kind': 'agent'} for identity in spec.identities()],
                      'edges': [], 'phases': []}}


def active(monkeypatch, server):
    monkeypatch.setattr(server, 'architect', SimpleNamespace(visualizer=object()))
    monkeypatch.setattr(server, 'visual_review_runtime', SimpleNamespace(capability=lambda: {'available': True}))


def test_review_route_binds_operation_and_has_its_own_body_limit(monkeypatch):
    from launchkit import server
    active(monkeypatch, server)
    calls = []
    operation_id = 'a' * 64

    async def submit(**values):
        calls.append(values)
        return {'id': 'b' * 64, 'binding': values['binding'], 'status': 'reviewing'}

    monkeypatch.setattr(server, 'visualization_reviews', SimpleNamespace(submit=submit))
    body = {**sources(), 'binding': {'id': operation_id}, 'renderer_hash': server.REVIEW_RENDERER_HASH,
            'frames': [{'bounded_fixture': 'x' * 110_000}]}
    client = TestClient(server.app)
    response = client.post(f'/api/visualizations/{operation_id}/review', json=body)
    assert response.status_code == 202 and calls == [body]
    response = client.post(f'/api/visualizations/{"c" * 64}/review', json=body)
    assert response.status_code == 409 and len(calls) == 1
    assert client.post('/api/unrelated', content='x' * 110_000).status_code == 413
    assert client.post(f'/api/visualizations/{operation_id}/review', json={**body, 'frames': []}).status_code == 422


def test_missing_critic_profile_does_not_start_or_spawn_native_work(monkeypatch):
    from launchkit import server
    monkeypatch.setattr(server, 'visual_review_runtime', SimpleNamespace(capability=lambda: {'available': False}))
    client = TestClient(server.app)
    assert client.get('/api/visualization-review-capability').json()['available'] is False
    response = client.post('/api/visualizations/' + 'a' * 64 + '/review', json={**sources(),
        'binding': {'id': 'a' * 64}, 'renderer_hash': server.REVIEW_RENDERER_HASH, 'frames': [{}]})
    assert response.status_code == 503


def test_review_admission_limit_has_an_honest_response(monkeypatch):
    from launchkit import server
    from launchkit.visualization_review import ReviewAdmissionUnavailable
    active(monkeypatch, server)

    async def limited(**values):
        raise ReviewAdmissionUnavailable('private-capacity-context')

    monkeypatch.setattr(server, 'visualization_reviews', SimpleNamespace(submit=limited))
    response = TestClient(server.app).post('/api/visualizations/' + 'a' * 64 + '/review', json={**sources(),
        'binding': {'id': 'a' * 64}, 'renderer_hash': server.REVIEW_RENDERER_HASH, 'frames': [{}]})
    assert response.status_code == 429 and 'private-capacity-context' not in response.text


def test_review_get_is_scoped_and_never_exposes_provider_errors(monkeypatch):
    from launchkit import server
    client = TestClient(server.app)
    monkeypatch.setattr(server, 'visualization_reviews', SimpleNamespace(get=lambda _: {
        'id': 'b' * 64, 'binding': {'id': 'a' * 64}, 'status': 'inconclusive'}))
    assert client.get('/api/visualizations/' + 'a' * 64 + '/reviews/' + 'b' * 64).status_code == 200
    assert client.get('/api/visualizations/' + 'c' * 64 + '/reviews/' + 'b' * 64).status_code == 404

    def fail(_):
        raise ValueError('private-provider-data')

    monkeypatch.setattr(server, 'visualization_reviews', SimpleNamespace(get=fail))
    response = client.get('/api/visualizations/' + 'a' * 64 + '/reviews/' + 'b' * 64)
    assert response.status_code == 404 and 'private-provider-data' not in response.text


def test_correction_requires_the_saved_report_and_rechecks_atomic_head(monkeypatch):
    from launchkit import server
    active(monkeypatch, server)
    calls = []
    parent = {'operation_id': 'a' * 64, 'code_hash': 'c' * 64,
              'blueprint_hash': 'd' * 64, 'model_hash': 'e' * 64,
              'setup_id': 'setup-A', 'stream_id': 'overview'}
    review = {'id': 'b' * 64, 'status': 'reviewed', 'correction_advisory': {
        'expected_parent': parent, 'review_id': 'b' * 64, 'feedback': 'Improve sampled labels.'}}
    monkeypatch.setattr(server, 'visualization_reviews', SimpleNamespace(get=lambda _: review))

    async def submit(service, blueprint, model, **options):
        calls.append(options)
        raise VisualizationConflict('private-operation-data')

    monkeypatch.setattr(server, 'visualization_operations', SimpleNamespace(submit=submit))
    body = {**sources(), 'expected_parent': parent, 'review_id': 'b' * 64,
            'feedback': review['correction_advisory']['feedback'], 'setup_id': 'setup-A', 'stream_id': 'overview'}
    client = TestClient(server.app)
    response = client.post('/api/visualizations', json=body)
    assert response.status_code == 409 and 'private-operation-data' not in response.text
    assert calls[0]['expected_parent'] == parent
    for mutation in ({'feedback': 'Invented critique.'}, {'review_id': None}, {'expected_parent': None}):
        assert client.post('/api/visualizations', json={**body, **mutation}).status_code == 409
    assert len(calls) == 1


@pytest.mark.asyncio
async def test_critic_runtime_has_no_seeds_or_mutable_setup_tools(tmp_path, monkeypatch, selected_guide_runtime):
    import tomllib
    from unittest.mock import AsyncMock
    from launchkit import visual_review_runtime as module
    from meerkat_meta import Meta

    captured, builds = {}, []
    process = SimpleNamespace(pid=123, returncode=None)
    process.poll = lambda: process.returncode
    async def stop():
        process.returncode = 0
    runtime = SimpleNamespace(shutdown=AsyncMock(side_effect=stop), _transport=SimpleNamespace(_process=process))

    class Builder:
        def __getattr__(self, method):
            def capture(value=None, *args, **kwargs):
                captured[method] = value
                return self
            return capture

        async def build(self):
            builds.append(True)
            return runtime

    monkeypatch.setattr(Meta, 'builder', lambda: Builder())
    monkeypatch.setattr(module, 'planner_capability', lambda: {'available': True, 'provider': 'openai'})
    monkeypatch.setattr(module, 'gateway_path', lambda: tmp_path / 'fixture-gateway')
    owner = module.VisualReviewRuntime(tmp_path)
    calls = []
    owner.reviews = SimpleNamespace(register_tools=lambda draft: calls.append(draft), confirm_runtime_terminated=AsyncMock())
    assert await owner.get_runtime() is runtime
    assert await owner.get_runtime() is runtime and len(builds) == 1
    assert owner.runtime_owner_id(runtime) == owner.owner_id
    with pytest.raises(ValueError):
        owner.runtime_owner_id(object())
    mob = tomllib.loads(captured['mob_inline'])
    assert set(mob['profiles']) == {'visual-review'}
    assert mob['profiles']['visual-review']['tools'] == {'builtins': False, 'comms': True, 'mob': False, 'memory': False}
    assert mob['profiles']['visual-review']['external_addressable'] is False
    assert 'orchestrator' not in mob and 'role_wiring' not in mob
    assert captured['persistent_state'] == str(tmp_path / 'generations' / owner.owner_id / '.mobkit/state')
    assert await captured['roster'].roster(None) == []
    draft = SimpleNamespace(profile_name='visual-review')
    await captured['session_service'].build_agent(draft)
    assert calls == [draft]
    with pytest.raises(ValueError):
        await captured['session_service'].build_agent(SimpleNamespace(profile_name='architect'))
    await owner.shutdown()
    runtime.shutdown.assert_awaited_once()
    assert owner.state == 'closed' and owner.runtime is None and owner.lock_context is None
    owner.reviews.confirm_runtime_terminated.assert_awaited_once_with(owner.owner_id)
    assert (tmp_path / ('terminated-' + owner.owner_id + '.json')).is_file()
    with pytest.raises(ValueError):
        await owner.get_runtime()
    assert len(builds) == 1


@pytest.mark.asyncio
async def test_failed_critic_start_does_not_retry_automatically(tmp_path, monkeypatch, selected_guide_runtime):
    from launchkit import visual_review_runtime as module
    from meerkat_meta import Meta
    builds = []

    class Builder:
        def __getattr__(self, method):
            return lambda *args, **kwargs: self

        async def build(self):
            builds.append(True)
            raise RuntimeError('private-native-start-failure')

    monkeypatch.setattr(Meta, 'builder', lambda: Builder())
    monkeypatch.setattr(module, 'planner_capability', lambda: {'available': True, 'provider': 'openai'})
    monkeypatch.setattr(module, 'gateway_path', lambda: tmp_path / 'fixture-gateway')
    owner = module.VisualReviewRuntime(tmp_path)
    owner.reviews = object()
    with pytest.raises(RuntimeError):
        await owner.get_runtime()
    assert owner.state == 'failed' and owner.capability()['available'] is False
    assert 'private-native-start-failure' not in str(owner.capability())
    with pytest.raises(ValueError):
        await owner.get_runtime()
    assert len(builds) == 1 and owner.lock_context is None


@pytest.mark.asyncio
async def test_failed_critic_shutdown_retains_runtime_and_store_custody(tmp_path):
    from unittest.mock import AsyncMock
    from launchkit.visual_review_runtime import VisualReviewRuntime
    from launchkit.templates.storage_setup import project_lock
    owner = VisualReviewRuntime(tmp_path)
    owner.runtime = SimpleNamespace(shutdown=AsyncMock(side_effect=RuntimeError('cleanup failed')))
    owner.native_process = SimpleNamespace(pid=123, returncode=0, poll=lambda: 0)
    owner.reviews = SimpleNamespace(confirm_runtime_terminated=AsyncMock())
    owner.lock_context = project_lock(tmp_path)
    owner.lock_context.__enter__()
    with pytest.raises(RuntimeError):
        await owner.shutdown()
    assert owner.runtime is not None and owner.lock_context is not None and owner.state == 'failed'
    owner.runtime.shutdown.side_effect = None
    await owner.shutdown()
    assert owner.runtime is None and owner.lock_context is None


@pytest.mark.asyncio
async def test_shutdown_never_confirms_a_native_owner_that_remains_alive(tmp_path):
    from unittest.mock import AsyncMock
    from launchkit.visual_review_runtime import VisualReviewRuntime
    from launchkit.templates.storage_setup import project_lock
    owner = VisualReviewRuntime(tmp_path)
    owner.runtime = SimpleNamespace(shutdown=AsyncMock())
    owner.native_process = SimpleNamespace(pid=123, returncode=None, poll=lambda: None)
    owner.reviews = SimpleNamespace(confirm_runtime_terminated=AsyncMock())
    owner.lock_context = project_lock(tmp_path)
    owner.lock_context.__enter__()
    with pytest.raises(RuntimeError, match='positively terminated'):
        await owner.shutdown()
    owner.reviews.confirm_runtime_terminated.assert_not_awaited()
    assert not list(tmp_path.glob('terminated-*.json'))
    assert owner.lock_context is not None and owner.runtime is not None
    owner.native_process = SimpleNamespace(pid=123, returncode=0, poll=lambda: 0)
    await owner.shutdown()


@pytest.mark.asyncio
async def test_saved_positive_termination_reconciles_after_confirmation_write_gap(tmp_path):
    from unittest.mock import AsyncMock
    from launchkit.visual_review_runtime import VisualReviewRuntime
    from launchkit.templates.storage_setup import project_lock
    owner = VisualReviewRuntime(tmp_path)
    owner.runtime = SimpleNamespace(shutdown=AsyncMock())
    owner.native_process = SimpleNamespace(pid=123, returncode=0, poll=lambda: 0)
    owner.reviews = SimpleNamespace(confirm_runtime_terminated=AsyncMock(side_effect=OSError('write gap')))
    owner.lock_context = project_lock(tmp_path)
    owner.lock_context.__enter__()
    with pytest.raises(OSError):
        await owner.shutdown()
    assert (tmp_path / ('terminated-' + owner.owner_id + '.json')).is_file()
    restored = VisualReviewRuntime(tmp_path)
    restored.reviews = SimpleNamespace(confirm_runtime_terminated=AsyncMock())
    assert restored.owner_id != owner.owner_id
    assert await restored.reconcile_terminated() is True
    restored.reviews.confirm_runtime_terminated.assert_awaited_once_with(owner.owner_id)
    assert restored.runtime is None and restored.state == 'stopped'
    owner.reviews.confirm_runtime_terminated.side_effect = None
    await owner.shutdown()


@pytest.mark.asyncio
async def test_invalid_saved_termination_disables_critic_without_dispatch(tmp_path):
    from unittest.mock import AsyncMock
    from launchkit.visual_review_runtime import VisualReviewRuntime
    owner = VisualReviewRuntime(tmp_path)
    owner.reviews = SimpleNamespace(confirm_runtime_terminated=AsyncMock())
    (tmp_path / ('terminated-' + 'a' * 32 + '.json')).write_text('{}')
    assert await owner.reconcile_terminated() is False
    assert owner.state == 'failed' and owner.runtime is None
    owner.reviews.confirm_runtime_terminated.assert_not_awaited()
    with pytest.raises(ValueError):
        await owner.get_runtime()
