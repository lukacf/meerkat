import asyncio
from copy import deepcopy
import hashlib
import json
from types import SimpleNamespace

import httpx
import pytest

from launchkit.projects import fingerprint
from launchkit.visualization_operations import VisualizationOperations
from launchkit.visualization_timeline import MAX_NATIVE_PAGE_BYTES, checked_request_record, public_frame, read_activity
from launchkit.visualizer import VisualizerService, VisualizerPending
from test_visualizer import inputs, CODE, NativeMembers, native_admission, revision_context


@pytest.fixture
def owned(tmp_path):
    async def forbidden_start():
        raise AssertionError('Activity reads must never start native work.')
    members = NativeMembers()
    architect = SimpleNamespace(root=tmp_path, runtime=SimpleNamespace(rust_http_base_url='http://owned.invalid', mob_handle=lambda: members), start=forbidden_start)
    service = VisualizerService(architect)
    service.native_members = members
    blueprint, model = inputs()
    binding = service.request_binding(blueprint, model)
    receipt = {'id': fingerprint({'operation': 1}), 'status': 'running', 'setup_id': 'setup-A', 'stream_id': 'overview',
               'source_generation': 1, 'request_fingerprint': fingerprint({'blueprint': blueprint, 'model': model}),
               **{key: binding[key] for key in ('blueprint_hash', 'model_hash', 'native_request_key')}}
    job = {**{key: value for key, value in binding.items() if key != 'native_request_key'}, 'key': binding['native_request_key'],
           'admission': {'interaction_id': 'actual-interaction', 'session_id': 'actual-session', 'cursor': 'console:10'},
           'blueprint': blueprint, 'model': model, 'private_prompt': 'PRIVATE-SCAFFOLD-CANARY'}
    receipt['native_admission'] = deepcopy(job['admission'])
    service._archive_request(job)
    ops = VisualizationOperations(root=tmp_path / 'operations')
    ops.receipts[receipt['id']] = receipt
    ops._persist(receipt)
    expected = {name: receipt[name] for name in ('setup_id', 'stream_id', 'request_fingerprint', 'blueprint_hash', 'model_hash', 'native_request_key', 'source_generation')}
    return SimpleNamespace(service=service, ops=ops, receipt=receipt, job=job, expected=expected, blueprint=blueprint, model=model)


def transport(monkeypatch, handler):
    real = httpx.AsyncClient
    monkeypatch.setattr(httpx, 'AsyncClient', lambda **kwargs: real(transport=httpx.MockTransport(handler), **kwargs))


def frame(cursor, kind, payload=None, **extra):
    return {'id': f'frame-{cursor}', 'cursor': f'console:{cursor}', 'kind': kind,
            'interaction_id': 'actual-interaction', 'session_id': 'actual-session',
            'timestamp_ms': 1000 + cursor, 'payload': payload or {}, **extra}


@pytest.mark.asyncio
async def test_request_owned_author_activity_uses_exact_saved_identity_session_and_interaction(owned, monkeypatch):
    identity = 'visualizer-request-' + 'a' * 32
    owned.job.update(author_identity=identity, author_session_id='actual-session')
    owned.job['admission']['identity'] = identity
    owned.receipt['native_admission'] = deepcopy(owned.job['admission'])
    owned.service._archive_request(owned.job)
    before = deepcopy(owned.receipt)
    frames = [frame(11, 'run_started', identity=identity),
              frame(12, 'text_delta', {'delta': 'Other request'}, identity='visualizer:main'),
              frame(13, 'interaction_complete', identity=identity, session_id='foreign-session'),
              frame(14, 'interaction_complete', identity=identity, interaction_id='foreign-interaction'),
              frame(15, 'tool_call_requested', {'name': 'inspect_visualization'}, identity=identity),
              frame(16, 'interaction_complete', identity=identity)]
    def respond(request):
        assert request.method == 'GET' and request.url.params['identity'] == identity
        return httpx.Response(200, json={'frames': frames, 'latest_cursor': 'console:16'})
    transport(monkeypatch, respond)
    result = await read_activity(owned.ops, owned.service, owned.receipt['id'], owned.expected)
    assert [item['cursor'] for item in result['frames']] == ['console:11', 'console:15', 'console:16']
    assert result['native']['status'] == 'completed' and result['native']['session_id'] == 'actual-session'
    assert result['native']['identity'] == identity
    assert all(item['identity'] == identity for item in result['frames'])
    assert owned.receipt == before


@pytest.mark.parametrize('field,value', [('author_identity', 'visualizer:main'),
                                        ('author_session_id', 'foreign-session')])
def test_saved_request_owned_author_cannot_rebind_native_admission(owned, field, value):
    identity = 'visualizer-request-' + 'a' * 32
    owned.job.update(author_identity=identity, author_session_id='actual-session')
    owned.job['admission']['identity'] = identity
    assert checked_request_record(owned.job)['author_identity'] == identity
    owned.job[field] = value
    with pytest.raises(ValueError):
        checked_request_record(owned.job)


def test_compaction_markers_preserve_safe_native_counters_without_private_payloads(owned):
    started = frame(51271, 'compaction_started', {'input_tokens': 0, 'estimated_history_tokens': 827619,
                    'message_count': 44, 'messages': ['PRIVATE-HISTORY-CANARY'], 'reasoning': 'PRIVATE-REASONING-CANARY'})
    completed = frame(53180, 'compaction_completed', {'summary_tokens': 1912, 'messages_before': 44,
                      'messages_after': 4, 'summary': 'PRIVATE-UNDECLARED-CANARY'})
    failed = frame(53181, 'compaction_failed', {'error': 'PRIVATE-ERROR-CANARY', 'summary_tokens': -1,
                   'input_tokens': True, 'message_count': float('inf')})
    assert public_frame(started, owned.job['admission'])['payload'] == {
        'input_tokens': 0, 'estimated_history_tokens': 827619, 'message_count': 44}
    assert public_frame(completed, owned.job['admission'])['payload'] == {
        'summary_tokens': 1912, 'messages_before': 44, 'messages_after': 4}
    assert public_frame(failed, owned.job['admission'])['payload'] == {}
    projected = public_frame(frame(53182, 'frame_updated', {'frame': completed}), owned.job['admission'])
    assert projected['payload']['frame']['kind'] == 'compaction_completed'
    assert 'CANARY' not in json.dumps(projected)


@pytest.mark.asyncio
@pytest.mark.parametrize('compaction_end', ['compaction_completed', 'compaction_failed'])
async def test_compaction_lifecycle_is_public_activity_but_not_native_interaction_terminal(owned, monkeypatch, compaction_end):
    before = deepcopy(owned.receipt)
    frames = [frame(11, 'compaction_started', {'message_count': 44}),
              frame(12, 'text_delta', {'delta': 'Public context preservation summary.'}),
              frame(13, compaction_end, {'messages_after': 4}),
              frame(14, 'text_complete', {'text': 'Ordinary author answer after preservation.'})]
    transport(monkeypatch, lambda request: httpx.Response(200, json={'frames': frames, 'latest_cursor': 'console:14'}))
    result = await read_activity(owned.ops, owned.service, owned.receipt['id'], owned.expected)
    assert [item['kind'] for item in result['frames']] == [item['kind'] for item in frames]
    assert result['native']['status'] == 'unknown' and result['native']['terminal_cursor'] is None
    assert result['next_cursor'] == 'console:14' and result['observation']['status'] == 'available'
    assert owned.receipt == before


@pytest.mark.asyncio
async def test_actual_admission_public_page_is_read_only(owned, monkeypatch):
    before = {path: path.read_bytes() for path in owned.service.architect.root.rglob('*.json')}
    frames = [frame(11, 'user_input', {'content': 'PRIVATE-SCAFFOLD-CANARY'}),
              frame(12, 'reasoning_delta', {'delta': 'RAW-REASONING-CANARY'}),
              frame(13, 'reasoning_summary_delta', {'summary': 'Arrange the actual owners and their handoffs.'}),
              frame(14, 'tool_call_requested', {'name': 'inspect_visualization', 'arguments': {'secret': 'PRIVATE-ARG-CANARY'}, 'id': 'tool-1'}),
              frame(15, 'tool_execution_completed', {'name': 'inspect_visualization', 'result': {'blueprint': 'PRIVATE-RESULT-CANARY'}, 'id': 'tool-1'}),
              frame(16, 'text_delta', {'delta': 'The scene is taking shape.'}),
              frame(17, 'interaction_complete', {'result': 'PRIVATE-TERMINAL-CANARY'}),
              frame(18, 'text_delta', {'delta': 'FOREIGN-CANARY'}, interaction_id='another-interaction'),
              frame(19, 'text_delta', {'delta': 'FOREIGN-SESSION-CANARY'}, session_id='another-session')]
    calls = []
    def respond(request):
        calls.append(request)
        assert request.method == 'GET' and request.url.path == '/console/timeline'
        assert request.url.params == httpx.QueryParams({'identity': 'visualizer:main', 'after': 'console:10', 'limit': '100'})
        return httpx.Response(200, json={'frames': frames, 'latest_cursor': 'console:19'})
    transport(monkeypatch, respond)
    result = await read_activity(owned.ops, owned.service, owned.receipt['id'], owned.expected)
    assert result['native'] == {'admission_confirmed': True, 'identity': 'visualizer:main', 'interaction_id': 'actual-interaction', 'session_id': 'actual-session', 'status': 'completed', 'terminal_cursor': 'console:17'}
    assert [item['cursor'] for item in result['frames']] == ['console:13', 'console:14', 'console:15', 'console:16', 'console:17']
    assert result['frames'][0]['payload']['summary'] == frames[2]['payload']['summary']
    assert result['frames'][1]['payload']['id'] == 'tool-1'
    assert result['next_cursor'] == 'console:19' and result['exhausted'] is True
    assert result['publication'] == {'available': False} and result['observation']['status'] == 'available'
    assert 'CANARY' not in json.dumps(result) and len(calls) == 1
    assert before == {path: path.read_bytes() for path in before}
    assert owned.receipt['status'] == 'running'


@pytest.mark.asyncio
async def test_reference_image_tool_frames_do_not_hide_later_public_activity(owned, monkeypatch):
    before = deepcopy(owned.receipt)
    frames = [frame(11, 'tool_execution_completed', {'name': 'read_visualization_examples', 'id': 'references',
                  'result': {'images': [{'data': 'PRIVATE-REFERENCE-CANARY' + 'x' * 2_200_000}]}}),
              frame(12, 'tool_result_received', {'name': 'read_visualization_examples', 'id': 'references',
                  'content': [{'type': 'image', 'data': 'PRIVATE-REFERENCE-CANARY' + 'y' * 1_920_000}]}),
              frame(13, 'reasoning_delta', {'delta': 'RAW-REASONING-CANARY'}),
              frame(14, 'text_delta', {'delta': 'The five-owner animation is ready.'}),
              frame(15, 'interaction_complete', {'result': 'PRIVATE-TERMINAL-CANARY'})]
    reply = httpx.Response(200, json={'frames': frames, 'latest_cursor': 'console:15'})
    assert 4_000_000 < len(reply.content) < 8_000_000
    calls = []
    def respond(request):
        calls.append(request)
        return reply
    transport(monkeypatch, respond)
    result = await read_activity(owned.ops, owned.service, owned.receipt['id'], owned.expected)
    assert result['observation']['status'] == 'available'
    assert [item['cursor'] for item in result['frames']] == ['console:11', 'console:12', 'console:14', 'console:15']
    assert result['native']['status'] == 'completed' and result['native']['terminal_cursor'] == 'console:15'
    assert result['next_cursor'] == 'console:15' and result['exhausted'] is True
    assert len(json.dumps(result).encode()) < 64_000 and 'CANARY' not in json.dumps(result)
    assert owned.receipt == before and len(calls) == 1


@pytest.mark.asyncio
@pytest.mark.parametrize('field', ['setup_id', 'stream_id', 'request_fingerprint', 'blueprint_hash', 'model_hash', 'native_request_key', 'source_generation'])
async def test_exact_scope_refusal_precedes_native_read(owned, monkeypatch, field):
    transport(monkeypatch, lambda request: (_ for _ in ()).throw(AssertionError('Stale source native read')))
    stale = {**owned.expected, field: 2 if field == 'source_generation' else 'another' if field.endswith('_id') else 'f' * 64}
    with pytest.raises(ValueError, match='does not match'):
        await read_activity(owned.ops, owned.service, owned.receipt['id'], stale)


@pytest.mark.asyncio
async def test_transient_read_failure_retains_admission_without_failed_status(owned, monkeypatch):
    before = deepcopy(owned.receipt)
    saved = owned.service.saved_request(owned.expected['native_request_key'])
    transport(monkeypatch, lambda request: httpx.Response(503, text='PRIVATE-NATIVE-ERROR-CANARY'))
    result = await read_activity(owned.ops, owned.service, owned.receipt['id'], owned.expected)
    assert result['observation']['status'] == 'unavailable' and result['native']['status'] == 'unknown'
    assert result['native']['admission_confirmed'] is True and result['frames'] == []
    assert 'CANARY' not in json.dumps(result)
    assert owned.receipt == before and owned.service.saved_request(owned.expected['native_request_key']) == saved


@pytest.mark.asyncio
async def test_unadmitted_and_disconnected_reads_do_not_start_runtime(owned, monkeypatch):
    transport(monkeypatch, lambda request: (_ for _ in ()).throw(AssertionError('Unexpected native read')))
    admission = owned.receipt.pop('native_admission')
    owned.receipt['status'] = 'queued'
    result = await read_activity(owned.ops, owned.service, owned.receipt['id'], owned.expected)
    assert result['frames'] == [] and result['native']['admission_confirmed'] is False
    owned.receipt['native_admission'] = admission
    owned.service.architect.runtime = None
    result = await read_activity(owned.ops, owned.service, owned.receipt['id'], owned.expected)
    assert result['native']['admission_confirmed'] is True and result['frames'] == []
    assert result['native']['status'] == 'unknown' and result['observation']['status'] == 'unavailable'


@pytest.mark.asyncio
async def test_publication_and_retained_cache_are_not_native_terminal(owned, monkeypatch):
    evidence = {name: owned.job[name] for name in ('blueprint_hash', 'model_hash', 'reference_hash')}
    evidence.update(request_key=owned.job['key'], code_hash=hashlib.sha256(CODE.encode()).hexdigest(), authority='publish_visualization')
    owned.receipt.update(status='published', artifact={**owned.job, 'publication_receipt': evidence, 'code': CODE, 'code_hash': evidence['code_hash'], 'cached': True})
    calls = []
    def respond(request):
        calls.append(request)
        return httpx.Response(200, json={'frames': [], 'latest_cursor': 'console:10'})
    transport(monkeypatch, respond)
    result = await read_activity(owned.ops, owned.service, owned.receipt['id'], owned.expected)
    assert result['publication']['available'] is True and result['native']['status'] == 'unknown'
    assert result['activity_origin'] == 'retained' and result['frames'] == [] and 'code' not in result['publication']
    owned.receipt['artifact']['code'] += 'tampered'
    invalid = await read_activity(owned.ops, owned.service, owned.receipt['id'], owned.expected)
    assert invalid['publication']['available'] is False
    assert invalid['native']['admission_confirmed'] is False and invalid['frames'] == []
    assert len(calls) == 1


@pytest.mark.asyncio
@pytest.mark.parametrize('change', ['runtime', 'receipt', 'admission'])
async def test_read_to_publication_boundary_rechecks_exact_native_and_host_identity(owned, monkeypatch, change):
    def changed(request):
        if change == 'runtime':
            owned.service.architect.runtime = SimpleNamespace(rust_http_base_url='http://replacement.invalid')
        elif change == 'receipt':
            owned.receipt['source_generation'] = 2
        else:
            owned.service._archive_request({**owned.job, 'admission': {**owned.job['admission'], 'session_id': 'replacement'}})
        return httpx.Response(200, json={'frames': [frame(11, 'interaction_complete')], 'latest_cursor': 'console:11'})
    transport(monkeypatch, changed)
    result = await read_activity(owned.ops, owned.service, owned.receipt['id'], owned.expected)
    assert result['frames'] == [] and result['native']['status'] == 'unknown'
    assert result['observation']['status'] == 'unavailable'


@pytest.mark.asyncio
async def test_byte_budget_preserves_next_cursor_and_unicode_bound(owned, monkeypatch):
    frames = [frame(n, 'text_delta', {'delta': '🍂' * 2000}) for n in range(11, 31)]
    transport(monkeypatch, lambda request: httpx.Response(200, json={'frames': frames, 'latest_cursor': 'console:30'}))
    result = await read_activity(owned.ops, owned.service, owned.receipt['id'], owned.expected, limit=20)
    assert 0 < len(result['frames']) < 20 and result['exhausted'] is False
    assert result['next_cursor'] == result['frames'][-1]['cursor']
    assert all(item['projection_truncated'] and len(item['payload']['delta'].encode()) <= 6000 for item in result['frames'])
    assert len(json.dumps(result, ensure_ascii=False).encode()) < 66_000


@pytest.mark.parametrize('mutation', [lambda job: job.update(key='f' * 64), lambda job: job.update(model_hash='f' * 64),
                                    lambda job: job.update(admission={'interaction_id': 'actual', 'cursor': -1}),
                                    lambda job: job.update(admission={'interaction_id': '', 'cursor': 10})])
def test_saved_admission_source_validation_and_private_archive(owned, mutation):
    malformed = deepcopy(owned.job)
    mutation(malformed)
    with pytest.raises(ValueError):
        checked_request_record(malformed)
    with pytest.raises(ValueError):
        owned.service.saved_request('../../pending')
    assert 'PRIVATE' not in (owned.service.root / 'requests' / (owned.job['key'] + '.json')).read_text()


def test_wrapped_frames_and_raw_reasoning_are_not_private_scaffolding(owned):
    admission = owned.job['admission']
    assert public_frame(frame(12, 'reasoning_complete', {'reasoning': 'private'}), admission) is None
    assert public_frame(frame(12, 'frame_updated', {'frame': frame(11, 'user_input', {'content': 'private'})}), admission) is None
    actual = frame(13, 'reasoning_complete', {'summary': 'Public summary', 'reasoning': 'private'})
    projected = public_frame(frame(14, 'frame_updated', {'frame': actual}), admission)
    assert projected['payload']['frame']['payload'] == {'text': 'Public summary', 'summary': 'Public summary'}


@pytest.mark.asyncio
async def test_operation_restart_is_read_only_and_reattaches_actual_admission(owned, monkeypatch):
    restored = VisualizationOperations(root=owned.ops.root)
    assert restored.get(owned.receipt['id']) == owned.receipt and restored.tasks == {}
    transport(monkeypatch, lambda request: httpx.Response(200, json={'frames': [frame(11, 'interaction_complete')], 'latest_cursor': 'console:11'}))
    result = await read_activity(restored, owned.service, owned.receipt['id'], owned.expected)
    assert result['native']['interaction_id'] == owned.job['admission']['interaction_id']
    assert result['native']['status'] == 'completed' and restored.tasks == {}


@pytest.mark.asyncio
async def test_setup_reuse_and_draining_owner_cannot_join_foreign_activity(owned):
    entered, release = asyncio.Event(), asyncio.Event()
    class Service:
        request_binding = owned.service.request_binding
        async def generate_with_admission(self, blueprint, model, *, on_admission):
            entered.set()
            await release.wait()
            raise VisualizerPending({'key': 'a' * 64, 'admission': {'interaction_id': 'prior-A'}})
    operations = VisualizationOperations(reconnect_interval=.01)
    first = await operations.submit(Service(), owned.blueprint, owned.model, setup_id='setup-A', stream_id='same-stream')
    await entered.wait()
    second = await operations.submit(Service(), owned.blueprint, owned.model, setup_id='setup-B', stream_id='same-stream')
    assert first['id'] != second['id'] and first['status'] != 'superseded'
    release.set()
    for _ in range(30):
        if first.get('reconnect_attempts'):
            break
        await asyncio.sleep(.001)
    assert first['status'] == 'queued' and first['native_request_key'] == owned.job['key']
    assert 'native_interaction_id' not in first
    await operations.shutdown()


def test_timeline_api_source_guards_and_input_echo_suppression(owned, monkeypatch):
    from fastapi.testclient import TestClient
    from launchkit import server
    owned.service.architect.visualizer = owned.service
    monkeypatch.setattr(server, 'architect', owned.service.architect)
    monkeypatch.setattr(server, 'visualization_operations', owned.ops)
    owned.service.architect.runtime = None
    client = TestClient(server.app)
    url = '/api/visualizations/' + owned.receipt['id'] + '/timeline'
    result = client.get(url, params=owned.expected)
    assert result.status_code == 200 and result.json()['native']['admission_confirmed'] is True
    for changed in ({'setup_id': 'wrong'}, {'native_request_key': 'f' * 64}):
        assert client.get(url, params={**owned.expected, **changed}).status_code == 409
    result = client.get(url, params={**owned.expected, 'after': 'PRIVATE-CANARY'})
    assert result.status_code == 422 and 'PRIVATE-CANARY' not in result.text
    assert client.get(url, params={**owned.expected, 'limit': 201}).status_code == 422
    result = client.post('/api/visualizations', json={'blueprint': {}, 'model': {}, 'feedback': 'PRIVATE-CANARY' * 100})
    assert result.status_code == 422 and 'PRIVATE-CANARY' not in result.text


@pytest.mark.asyncio
async def test_restored_running_http_receipt_without_native_owner_never_dispatches(owned):
    restored = VisualizationOperations(root=owned.ops.root)
    receipt = await restored.submit(owned.service, owned.blueprint, owned.model, setup_id='setup-A', stream_id='overview')
    assert receipt['id'] == owned.receipt['id'] and restored.tasks == {}
    assert receipt['observation_status'] == 'unavailable'
    assert owned.service.architect.runtime.rust_http_base_url == 'http://owned.invalid'


@pytest.mark.asyncio
async def test_restored_pending_owner_reattaches_same_operation_without_replacing_admission(owned, monkeypatch):
    from launchkit.templates.storage_setup import write_private
    pending = {**owned.job, 'idempotency_key': 'same-stable-key'}
    write_private(owned.service.root / 'pending.json', pending)
    starts, releases = [], asyncio.Event()
    async def same_request(blueprint, model, *, on_admission, on_native_admission):
        assert owned.service.can_reattach(owned.job['key']) is True
        starts.append(owned.service.saved_request(owned.job['key'])['admission'])
        await on_admission()
        await on_native_admission(owned.job['key'], owned.job['admission'])
        await releases.wait()
        return {'native_run': {'status': 'completed'}}
    monkeypatch.setattr(owned.service, 'generate_with_native_admission', same_request)
    restored = VisualizationOperations(root=owned.ops.root)
    receipt = await restored.submit(owned.service, owned.blueprint, owned.model, setup_id='setup-A', stream_id='overview')
    assert receipt['id'] == owned.receipt['id']
    assert await restored.submit(owned.service, owned.blueprint, owned.model, setup_id='setup-A', stream_id='overview') is receipt
    await asyncio.sleep(0)
    assert starts == [owned.job['admission']]
    releases.set()
    await asyncio.gather(*restored.tasks.values())
    assert receipt['status'] == 'completed'


@pytest.mark.asyncio
async def test_real_service_publication_archives_only_admission_and_survives_http_restart(owned, monkeypatch):
    from test_visualizer import native_completion
    async def allowed_start():
        pass
    owned.service.architect.start = allowed_start
    calls = native_completion(owned.service, monkeypatch)
    (owned.service.root / 'requests' / (owned.job['key'] + '.json')).unlink()
    operations = VisualizationOperations(root=owned.ops.root / 'actual-service')
    receipt = await operations.submit(owned.service, owned.blueprint, owned.model, setup_id='setup-A', stream_id='real-source')
    await asyncio.gather(*operations.tasks.values())
    assert receipt['status'] == 'completed' and receipt['artifact']['code'] == CODE
    record = owned.service.saved_request(receipt['native_request_key'])
    assert record['admission'] == receipt['native_admission']
    assert record['admission']['identity'] == record['author_identity']
    assert record['admission']['session_id'] == record['author_session_id']
    assert record['native_run'] == {'status': 'completed', 'interaction_id': 'native-visualization', 'terminal_cursor': 3}
    assert not set(record).intersection({'blueprint', 'model', 'publication', 'publication_receipt', 'revision_feedback', 'idempotency_key'})
    assert owned.service.pending is None and not (owned.service.root / 'pending.json').exists()
    restored = VisualizationOperations(root=operations.root)
    result = await restored.submit(owned.service, owned.blueprint, owned.model, setup_id='setup-A', stream_id='real-source')
    assert result == receipt and restored.tasks == {}
    assert sum(method == 'post' for method, *_ in calls) == 1


@pytest.mark.asyncio
async def test_malformed_and_oversized_native_pages_do_not_advance_receipt_or_native_status(owned, monkeypatch):
    for reply in [httpx.Response(200, json={'frames': [frame(11, 'interaction_complete'), frame(11, 'text_delta', {'text': 'bad duplicate'})]}),
                  httpx.Response(200, json={'frames': [frame(11, 'tool_execution_completed',
                      {'name': 'read_visualization_examples', 'result': {'image': 'x' * MAX_NATIVE_PAGE_BYTES}})]}),
                  httpx.Response(200, json={'frames': [], 'latest_cursor': 'invalid'})]:
        transport(monkeypatch, lambda request: reply)
        result = await read_activity(owned.ops, owned.service, owned.receipt['id'], owned.expected)
        assert result['frames'] == [] and result['native']['status'] == 'unknown'
        assert result['next_cursor'] == 'console:10' and result['observation']['status'] == 'unavailable'
        monkeypatch.undo()


def test_actual_frame_version_and_source_ids_survive_without_private_fields(owned):
    actual = frame(12, 'tool_execution_started', {'name': 'publish_visualization', 'arguments': {'code': 'PRIVATE-CODE'}},
                   status='dispatching', frame_version=2, updated_at_ms=1042, source_event_id='actual-event',
                   source={'kind': 'console_event', 'source_cursor': 'actual-source'}, runtime_key='/private/runtime')
    projected = public_frame(actual, owned.job['admission'])
    assert projected['frame_version'] == 2 and projected['updated_at_ms'] == 1042
    assert projected['status'] == 'dispatching' and projected['source_event_id'] == 'actual-event'
    assert projected['source'] == actual['source']
    assert 'PRIVATE' not in json.dumps(projected) and 'runtime_key' not in projected
    assert public_frame({**actual, 'identity': 'architect:main'}, owned.job['admission']) is None
    terminal = public_frame(frame(13, 'interaction_complete', {'text': 'A published scene.', 'result': {'private': True}}), owned.job['admission'])
    assert terminal['payload'] == {'text': 'A published scene.'}


@pytest.mark.asyncio
async def test_exact_unknown_archived_admission_cannot_be_blindly_redispatched(owned):
    from launchkit.visualizer import VisualizerUnavailable
    with pytest.raises(VisualizerUnavailable, match='terminal status is unknown'):
        await owned.service.generate(owned.blueprint, owned.model)
    assert owned.service.pending is None


@pytest.mark.asyncio
async def test_restored_known_terminal_without_publication_fails_host_request_only(owned):
    terminal = {'status': 'completed', 'interaction_id': 'actual-interaction', 'terminal_cursor': 'console:11'}
    owned.service._archive_request({**owned.job, 'native_run': terminal})
    restored = VisualizationOperations(root=owned.ops.root)
    receipt = await restored.submit(owned.service, owned.blueprint, owned.model, setup_id='setup-A', stream_id='overview')
    assert receipt['id'] == owned.receipt['id'] and receipt['status'] == 'failed' and restored.tasks == {}
    assert owned.service.saved_request(owned.job['key'])['native_run'] == terminal


@pytest.mark.asyncio
async def test_receipt_write_failure_does_not_lose_owned_task_cleanup_or_send_native_work(owned, monkeypatch):
    writes, sends = [], []
    operations = VisualizationOperations()
    def persist(receipt):
        writes.append(receipt['status'])
        if len(writes) > 1:
            raise OSError('private-disk-path')
    class Service:
        async def generate(self, blueprint, model):
            sends.append('unexpected')
    monkeypatch.setattr(operations, '_persist', persist)
    receipt = await operations.submit(Service(), owned.blueprint, owned.model, stream_id='owned')
    results = await asyncio.gather(*operations.tasks.values(), return_exceptions=True)
    assert any(isinstance(result, OSError) for result in results)
    assert not operations.tasks and not operations.jobs and sends == []
    assert receipt['status'] == 'failed' and 'private' not in receipt['detail']


@pytest.mark.asyncio
@pytest.mark.parametrize('status', ['queued', 'running', 'observing'])
async def test_equal_content_without_operation_native_admission_cannot_borrow_prior_attempt(owned, monkeypatch, status):
    owned.service._archive_request({**owned.job, 'native_run': {
        'interaction_id': 'actual-interaction', 'status': 'failed', 'terminal_cursor': 'console:17'}})
    owned.receipt.pop('native_admission')
    owned.receipt.update(status=status, native_interaction_id='actual-interaction')
    transport(monkeypatch, lambda request: (_ for _ in ()).throw(AssertionError('Unadmitted operation read another attempt')))
    result = await read_activity(owned.ops, owned.service, owned.receipt['id'], owned.expected)
    assert result['native']['admission_confirmed'] is False and result['native']['interaction_id'] is None
    assert result['native']['status'] == 'unknown' and result['frames'] == []
    assert result['activity_origin'] == 'request-bound'


@pytest.mark.asyncio
async def test_equal_content_in_another_setup_keeps_its_own_native_admission(owned, monkeypatch):
    async def allowed_start():
        pass
    owned.service.architect.start = allowed_start
    (owned.service.root / 'requests' / (owned.job['key'] + '.json')).unlink()
    release = asyncio.Event()
    sends = []
    async def native_request(client, method, url, **kwargs):
        if method == 'post':
            sends.append(kwargs['json'])
            return SimpleNamespace(json=lambda: native_admission(owned.service, 'native-'+str(len(sends))))
        await release.wait()
        await owned.service.inspect({"view": "overview", "request_key": owned.service.pending["key"], "include_author_references": True})
        result = await owned.service.publish({'request_key': owned.service.pending['key'], 'code': CODE,
                                              'summary': 'This setup has its own published program'})
        assert result['valid']
        interaction = owned.service.pending['admission']['interaction_id']
        return SimpleNamespace(json=lambda: {'frames': [{'kind': 'interaction_complete', 'cursor': 3,
                                                         'interaction_id': interaction}]})
    monkeypatch.setattr('launchkit.architect._native_request', native_request)
    operations = VisualizationOperations(root=owned.ops.root / 'equal-content')
    first = await operations.submit(owned.service, owned.blueprint, owned.model, setup_id='setup-A', stream_id='same-stream')
    async with asyncio.timeout(30):
        while not first.get('native_admission'):
            await asyncio.sleep(.001)
    second = await operations.submit(owned.service, owned.blueprint, owned.model, setup_id='setup-B', stream_id='same-stream')
    expected = {name: second[name] for name in owned.expected}
    assert first['native_request_key'] != second['native_request_key'] and first['id'] != second['id']
    assert second['status'] == 'queued' and 'native_admission' not in second
    with monkeypatch.context() as reads:
        transport(reads, lambda request: (_ for _ in ()).throw(AssertionError('Queued B borrowed A activity')))
        before = await read_activity(operations, owned.service, second['id'], expected)
    assert before['native']['admission_confirmed'] is False and before['frames'] == []
    release.set()
    await asyncio.wait_for(asyncio.gather(*operations.tasks.values()), 1)
    assert second['artifact']['cached'] is False and len(sends) == 2
    assert second['native_admission']['interaction_id'] != first['native_admission']['interaction_id']
    with monkeypatch.context() as reads:
        transport(reads, lambda request: httpx.Response(200, json={'frames': [], 'latest_cursor': 'console:3'}))
        current = await read_activity(operations, owned.service, second['id'], expected)
    assert current['activity_origin'] == 'request-bound' and current['publication']['available'] is True
    assert current['observation']['status'] == 'available'
    assert current['native']['interaction_id'] == second['native_admission']['interaction_id']


@pytest.mark.asyncio
async def test_versioned_revision_archive_retains_current_scope_and_public_activity(owned, monkeypatch):
    context = revision_context(setup=owned.receipt['setup_id'], stream=owned.receipt['stream_id'])
    binding = owned.service.request_binding(owned.blueprint, owned.model, revision_context=context)
    owned.job.update(key=binding['native_request_key'], revision_binding=binding['revision_binding'],
                     revision_context=context, private_previous_program='PRIVATE-OLD-PROGRAM-CANARY')
    owned.receipt.update(native_request_key=owned.job['key'], revision_binding=binding['revision_binding'],
                         revision_context=context, contract=binding['contract'],
                         reference_hash=binding['reference_hash'])
    owned.expected['native_request_key'] = owned.job['key']
    owned.service._archive_request(owned.job)
    record = owned.service.saved_request(owned.job['key'])
    assert record['revision_binding'] == binding['revision_binding']
    assert record['admission'] == owned.job['admission']
    archive = (owned.service.root / 'requests' / (owned.job['key']+'.json')).read_text()
    assert 'private_previous_program' not in archive and 'revision_context' not in json.loads(archive)
    assert 'PRIVATE-OLD-PROGRAM-CANARY' not in archive
    transport(monkeypatch, lambda request: httpx.Response(200, json={
        'frames': [frame(11, 'text_delta', {'delta': 'Revising the current proposal.'})], 'latest_cursor': 'console:11'}))
    result = await read_activity(owned.ops, owned.service, owned.receipt['id'], owned.expected)
    assert result['native']['admission_confirmed'] is True and result['frames'][0]['payload']['delta']=='Revising the current proposal.'
    assert result['native_request_key']==binding['native_request_key']


@pytest.mark.parametrize('field,value',[('version','unknown'),('setup_id','another-setup'),
                                        ('stream_id','another-view'),('context_hash','f'*64)])
def test_revision_archive_cannot_rebind_scope_or_context_without_changing_request_key(owned,field,value):
    context = revision_context(setup=owned.receipt['setup_id'],stream=owned.receipt['stream_id'])
    binding = owned.service.request_binding(owned.blueprint,owned.model,revision_context=context)
    job = {**owned.job,'key':binding['native_request_key'],'revision_binding':deepcopy(binding['revision_binding'])}
    assert checked_request_record(job)['revision_binding']==binding['revision_binding']
    job['revision_binding'][field]=value
    with pytest.raises(ValueError):checked_request_record(job)


@pytest.mark.asyncio
async def test_same_content_retry_keeps_original_attempt_across_native_admission_and_restart(owned, monkeypatch):
    async def allowed_start():
        pass
    owned.service.architect.start = allowed_start
    owned.service.native_members.session_override = 'retry-session'
    owned.receipt['status'] = 'failed'
    owned.ops._persist(owned.receipt)
    original = {**owned.job, 'native_run': {'interaction_id': 'actual-interaction', 'status': 'completed', 'terminal_cursor': 'console:17'}}
    owned.service._archive_request(original)
    entered, release = asyncio.Event(), asyncio.Event()
    posts = []
    async def request(client, method, url, **kwargs):
        if method == 'post':
            posts.append(kwargs['json']['idempotency_key'])
            entered.set()
            await release.wait()
            return SimpleNamespace(json=lambda: native_admission(owned.service, 'retry-interaction', 'console:30'))
        return SimpleNamespace(json=lambda: {'frames': [frame(31, 'interaction_complete', interaction_id='retry-interaction', session_id='retry-session')]})
    monkeypatch.setattr('launchkit.architect._native_request', request)
    retried = await owned.ops.submit(owned.service, owned.blueprint, owned.model, setup_id='setup-A', stream_id='overview')
    await asyncio.wait_for(entered.wait(), 30)
    expected = {name: retried[name] for name in owned.expected}
    assert retried['id'] != owned.receipt['id'] and retried['status'] == 'running'
    assert 'native_admission' not in retried
    transport(monkeypatch, lambda request: httpx.Response(200, json={'frames': [frame(17, 'interaction_complete')], 'latest_cursor': 'console:17'}))
    pending = await read_activity(owned.ops, owned.service, retried['id'], expected)
    assert pending['native']['admission_confirmed'] is False and pending['frames'] == []
    prior = await read_activity(owned.ops, owned.service, owned.receipt['id'], owned.expected)
    assert prior['native']['interaction_id'] == 'actual-interaction' and prior['native']['terminal_cursor'] == 'console:17'
    release.set()
    await asyncio.wait_for(asyncio.gather(*owned.ops.tasks.values()), 1)
    assert retried['native_admission']['interaction_id'] == 'retry-interaction' and len(posts) == 1
    assert retried['native_request_key'] != original['key']
    assert owned.service.saved_request(original['key'])['admission']['interaction_id'] == 'actual-interaction'
    assert owned.service.saved_request(retried['native_request_key'])['admission']['interaction_id'] == 'retry-interaction'
    restored = VisualizationOperations(root=owned.ops.root)
    prior = await read_activity(restored, owned.service, owned.receipt['id'], owned.expected)
    assert prior['native']['interaction_id'] == 'actual-interaction'
    assert prior['native']['status'] == 'completed' and prior['native']['terminal_cursor'] == 'console:17'
    assert all(item['interaction_id'] == 'actual-interaction' for item in prior['frames'])
    assert restored.tasks == {} and owned.receipt['native_admission'] == original['admission']
