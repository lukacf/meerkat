import asyncio
import pytest
from launchkit.visualization_operations import VisualizationOperations
from launchkit.visualizer import VisualizerPending
from test_meta_guide_runtime import selected as selected_guide_runtime



@pytest.fixture
def publication_clock(monkeypatch):
    """Deterministic host timestamps for offline coordinator observations."""
    from datetime import datetime, timedelta, timezone

    class Clock:
        value = datetime(2026, 10, 3, 12, tzinfo=timezone.utc)

        @classmethod
        def now(cls, zone):
            assert zone is timezone.utc
            return cls.value

        @classmethod
        def advance(cls, seconds):
            cls.value += timedelta(seconds=seconds)

    monkeypatch.setattr("launchkit.visualization_operations.datetime", Clock)
    return Clock


@pytest.mark.asyncio
async def test_pending_observation_reconnect_backoff_is_capped_and_does_not_claim_completion(monkeypatch):
    delays, states = [], []
    original_sleep = asyncio.sleep
    async def sleep(delay):
        delays.append(delay)
        states.append(dict(receipt))
        await original_sleep(0)
    monkeypatch.setattr("launchkit.visualization_operations.asyncio.sleep", sleep)
    class Service:
        def __init__(self):
            self.calls = 0
        async def generate(self, blueprint, model):
            self.calls += 1
            if self.calls <= 4:
                raise VisualizerPending({"key": "exact-native-request", "admission": {"interaction_id": "real-interaction"}})
            return {"key": "exact-native-request", "code": "published", "native_run": {"status": "completed"}}
    service = Service()
    operations = VisualizationOperations(reconnect_interval=2, reconnect_max_interval=5)
    receipt = await operations.submit(service, {}, {})
    await asyncio.gather(*operations.tasks.values())
    assert delays == [2, 4, 5, 5]
    assert all(item["status"] == "observing" and item["native_run_status"] == "unknown"
               and item["native_interaction_id"] == "real-interaction" and "artifact" not in item
               and "completed_at" not in item for item in states)
    assert receipt["status"] == "completed" and not operations.tasks

@pytest.mark.asyncio
async def test_reconnect_deduplicates_and_revisions_queue_without_blocking_submit():
    started = asyncio.Event()
    release = asyncio.Event()
    calls = []
    class Service:
        async def generate(self, blueprint, model):
            calls.append(blueprint)
            started.set()
            await release.wait()
            return {'code': 'return {};', 'summary': str(blueprint)}
    ops = VisualizationOperations()
    service = Service()
    first = await ops.submit(service, {'version': 1}, {})
    await started.wait()
    same = await ops.submit(service, {'version': 1}, {})
    second = await asyncio.wait_for(ops.submit(service, {'version': 2}, {}), .1)
    assert first is same and second['id'] != first['id']
    assert len(calls) == 1
    release.set()
    await asyncio.gather(*ops.tasks.values())
    assert len(calls) == 2
    assert ops.get(first['id'])['status'] == 'completed'
    assert (await ops.submit(service, {'version': 1}, {}))['artifact']['code']
    assert len(calls) == 2


@pytest.mark.asyncio
async def test_feedback_revisions_deduplicate_and_deliver_exact_diagnostic():
    started = asyncio.Event()
    release = asyncio.Event()
    calls = []
    blueprint, model = {'version': 1}, {'nodes': []}

    class Service:
        async def generate(self, given_blueprint, given_model, *, feedback=""):
            calls.append((given_blueprint, given_model, feedback))
            if feedback:
                started.set()
                await release.wait()
            return {'code': 'return {};', 'summary': feedback or 'Initial layout'}

    operations = VisualizationOperations()
    service = Service()
    original = await operations.submit(service, blueprint, model)
    await asyncio.gather(*operations.tasks.values())
    first_feedback = 'Program first failed validation: missing configured identity.'
    second_feedback = 'Program second failed validation: missing configured identity.'
    revised = await operations.submit(service, blueprint, model, feedback=first_feedback)
    await started.wait()
    same = await operations.submit(service, blueprint, model, feedback=first_feedback)
    queued = await asyncio.wait_for(operations.submit(service, blueprint, model, feedback=second_feedback), .1)
    assert same is revised
    assert len({original['id'], revised['id'], queued['id']}) == 3
    assert calls == [(blueprint, model, ''), (blueprint, model, first_feedback)]
    release.set()
    await asyncio.gather(*operations.tasks.values())
    assert calls == [(blueprint, model, ''), (blueprint, model, first_feedback), (blueprint, model, second_feedback)]
    assert revised['status'] == queued['status'] == 'completed'
    assert revised['artifact']['summary'] == first_feedback
    assert queued['artifact']['summary'] == second_feedback
    assert await operations.submit(service, blueprint, model, feedback=first_feedback) is revised
    assert len(calls) == 3


def test_visualization_api_passes_bounded_feedback_to_operations(monkeypatch):
    from types import SimpleNamespace
    from fastapi.testclient import TestClient
    from launchkit import server
    from launchkit.blueprint import starter_blueprint

    calls = []
    visualizer = object()

    class Operations:
        async def submit(self, service, blueprint, model, *, feedback="", stream_id=None):
            calls.append((service, blueprint, model, feedback, stream_id))
            return {'id': 'revision-request', 'status': 'running'}

    monkeypatch.setattr(server, 'architect', SimpleNamespace(visualizer=visualizer))
    monkeypatch.setattr(server, 'visualization_operations', Operations())
    spec = starter_blueprint()
    blueprint = spec.model_dump(mode='json')
    model = {'nodes': [{'id': identity, 'kind': 'agent'} for identity in spec.identities()],
             'edges': [], 'phases': []}
    payload = {'blueprint': blueprint, 'model': model, 'feedback': 'Program first failed validation: missing identity.'}
    client = TestClient(server.app)
    response = client.post('/api/visualizations', json=payload)
    assert response.status_code == 202
    assert calls == [(visualizer, blueprint, model, payload['feedback'], None)]
    assert response.json() == {'id': 'revision-request', 'status': 'running'}
    assert client.post('/api/visualizations', json={**payload, 'feedback': 'x' * 1001}).status_code == 422
    assert len(calls) == 1
    response = client.post('/api/visualizations', json={**payload, 'stream_id': 'guide:abc:overview'})
    assert response.status_code == 202
    assert calls[-1] == (visualizer, blueprint, model, payload['feedback'], 'guide:abc:overview')
    for invalid in ['', '   ', 'x' * 257, 12, ['guide']]:
        assert client.post('/api/visualizations', json={**payload, 'stream_id': invalid}).status_code == 422
    assert len(calls) == 2


@pytest.mark.asyncio
async def test_failure_and_shutdown_are_visible_and_retryable():
    class Service:
        async def generate(self, blueprint, model):
            raise RuntimeError('private details')
    ops = VisualizationOperations()
    receipt = await ops.submit(Service(), {}, {})
    await asyncio.gather(*ops.tasks.values())
    assert receipt['status'] == 'failed'
    assert 'private' not in receipt['detail']
    waiting = asyncio.Event()
    class Slow:
        async def generate(self, blueprint, model):
            waiting.set()
            await asyncio.Event().wait()
    receipt = await ops.submit(Slow(), {}, {})
    await waiting.wait()
    await ops.shutdown()
    assert receipt['status'] == 'interrupted'


class GatedService:
    def __init__(self):
        self.started = asyncio.Event()
        self.release = asyncio.Event()
        self.calls = []
        self.cancellations = 0

    async def generate(self, blueprint, model):
        self.calls.append(blueprint['version'])
        if len(self.calls) == 1:
            self.started.set()
            try:
                await self.release.wait()
            except asyncio.CancelledError:
                self.cancellations += 1
                raise
        return {'code': f"return {blueprint['version']};"}


@pytest.mark.asyncio
async def test_unscoped_queue_wait_is_not_recorded_as_actual_admission(publication_clock):
    operations, service = VisualizationOperations(), GatedService()
    await operations.generation_lock.acquire()
    try:
        receipt = await operations.submit(service, {'version': 1}, {})
        await asyncio.sleep(0)
        assert receipt['queued_at'] == '2026-10-03T12:00:00+00:00'
        assert 'started_at' not in receipt and service.calls == []
        publication_clock.advance(5)
        operations.generation_lock.release()
        await service.started.wait()
        assert receipt['started_at'] == '2026-10-03T12:00:05+00:00'
        assert 'published_at' not in receipt and 'completed_at' not in receipt
        publication_clock.advance(3)
        service.release.set()
        await asyncio.gather(*operations.tasks.values())
        assert receipt['status'] == 'completed'
        assert receipt['published_at'] == receipt['completed_at'] == '2026-10-03T12:00:08+00:00'
    finally:
        if operations.generation_lock.locked():
            operations.generation_lock.release()
        await operations.shutdown()


@pytest.mark.asyncio
async def test_stream_coalesces_three_queued_changes_without_cancelling_admitted_generation():
    operations, service = VisualizationOperations(), GatedService()
    active = await operations.submit(service, {'version': 1}, {}, stream_id='guide:one')
    await service.started.wait()
    queued = []
    for version in (2, 3, 4):
        queued.append(await asyncio.wait_for(operations.submit(service, {'version': version}, {}, stream_id='guide:one'), .1))
    assert active['status'] == 'running'
    assert service.calls == [1] and service.cancellations == 0
    assert [receipt['status'] for receipt in queued] == ['superseded', 'superseded', 'queued']
    assert queued[0]['replacement_id'] == queued[1]['id']
    assert queued[1]['replacement_id'] == queued[2]['id']
    assert set(operations.tasks) == {active['id'], queued[-1]['id']}
    assert await operations.submit(service, {'version': 4}, {}, stream_id='guide:one') is queued[-1]
    service.release.set()
    await asyncio.gather(*operations.tasks.values())
    assert service.calls == [1, 4] and service.cancellations == 0
    assert active['status'] == queued[-1]['status'] == 'completed'


@pytest.mark.asyncio
@pytest.mark.parametrize('same_owner,window', [
    (False, 'none'), (False, 'before'), (False, 'after'), (True, 'none'),
])
async def test_restored_native_owner_drains_before_latest_stream_revision_is_admitted(
        tmp_path, monkeypatch, same_owner, window, selected_guide_runtime):
    import copy
    import hashlib
    import json
    from types import SimpleNamespace
    from launchkit.blueprint import starter_blueprint
    from launchkit.projects import fingerprint
    from launchkit.visualizer import CONTRACT_VERSION, REFERENCE_ROOT, VisualizerService, visualization_examples
    from launchkit.visualization_review import bundled_review_reference_context
    from meerkat_meta import ToolResultContent

    async def start():
        pass
    from test_visualizer import NativeMembers
    members = NativeMembers()
    members.session_override = "new-C"
    architect = SimpleNamespace(root=tmp_path, start=start,
                                runtime=SimpleNamespace(rust_http_base_url='http://native.invalid',
                                                        mob_handle=lambda: members))
    service = VisualizerService(architect, timeout=.02 if window != 'none' else 1, poll_interval=.001)
    # This regression exercises an earlier host's unscoped native protocol.
    # Continuous requests have separate scoped-key coverage below.
    monkeypatch.setattr(service, 'generate_continuous_with_native_admission', None)
    code = 'return {duration:1,render(frame){}};'
    code_hash = hashlib.sha256(code.encode()).hexdigest()
    blueprint = starter_blueprint().model_dump(mode='json')
    model = {'nodes': [{'id': identity, 'kind': 'agent'} for identity in starter_blueprint().identities()],
             'edges': [], 'phases': []}
    current_reference_hash = service.request_binding(blueprint, model)['reference_hash']
    # Load real checked pixels outside the 20ms prior-owner observation window.
    # The current job still receives and binds them through actual inspection.
    reference_context = bundled_review_reference_context(REFERENCE_ROOT)
    async def cached_references(job):
        if service.pending is job and job['reference_hash'] == reference_context['source_reference_hash']:
            return copy.deepcopy(reference_context)
        return None
    monkeypatch.setattr(service, '_author_reference_context', cached_references)
    # The same-owner branch restores the current reference identity. The other
    # branches retain the earlier text-only identity and must still drain it.
    reference_hash = current_reference_hash if same_owner else fingerprint(visualization_examples())
    assert (reference_hash == current_reference_hash) is same_owner
    def request_identity(spec):
        return {'contract': CONTRACT_VERSION, 'blueprint_hash': fingerprint(spec),
                'model_hash': fingerprint(model), 'reference_hash': reference_hash}
    identity = request_identity(blueprint)
    original_key = fingerprint(identity)
    original_admission = {'interaction_id': 'native-A', 'session_id': 'saved-A', 'cursor': 'console:10'}
    original = {**identity, 'key': original_key, 'blueprint': blueprint, 'model': model,
                'owner_id': 'previous-host', 'idempotency_key': 'original-native-idempotency',
                'admission': original_admission, 'examples_read': True,
                'publication': {'code': code, 'summary': 'Saved owner A'},
                'publication_receipt': {**identity, 'request_key': original_key,
                    'code_hash': code_hash, 'authority': 'publish_visualization'}}
    service.root.mkdir()
    (service.root / 'pending.json').write_text(json.dumps(original))
    first_read, replacement_read, release = asyncio.Event(), asyncio.Event(), asyncio.Event()
    reads, sends, observation_cancellations, author_reads = [], [], [], []
    async def request(client, method, url, **kwargs):
        if method == 'post':
            assert url.endswith('/console/send')
            sends.append(copy.deepcopy(kwargs['json']))
            return SimpleNamespace(json=lambda: {'interaction_id': 'native-C',
                                                'identity': service.pending['author_identity'],
                                                'session_id': 'new-C', 'cursor': 'console:20'})
        assert method == 'get' and url.endswith('/console/timeline')
        if service.pending['admission']['interaction_id'] == 'native-A':
            reads.append(kwargs['params']['after'])
            first_read.set()
            if len(reads) == 2:
                replacement_read.set()
            try:
                await release.wait()
            except asyncio.CancelledError:
                observation_cancellations.append('native-A')
                raise
            return SimpleNamespace(json=lambda: {'frames': [{'kind': 'interaction_complete',
                'interaction_id': 'native-A', 'cursor': 'console:11'}]})
        request_key = service.pending['key']
        inspected = await service.inspect({'view': 'overview', 'request_key': request_key,
                                           'include_author_references': True})
        assert isinstance(inspected, ToolResultContent)
        assert [block['type'] for block in inspected.blocks] == ['text', 'image', 'image']
        author_reads.append(request_key)
        published = await service.publish({'request_key': request_key, 'code': code, 'summary': 'Latest C'})
        assert published['valid'] is True
        return SimpleNamespace(json=lambda: {'frames': [{'kind': 'interaction_complete',
            'interaction_id': 'native-C', 'cursor': 'console:21'}]})
    monkeypatch.setattr('launchkit.architect._native_request', request)
    operations = VisualizationOperations(reconnect_interval=100)
    desired = copy.deepcopy(blueprint)
    if not same_owner:
        desired['project']['name'] = 'Queued B'
    first = await operations.submit(service, desired, model, stream_id='guide:one')
    first_task = operations.tasks[first['id']]
    await asyncio.wait_for(first_read.wait(), 30)
    if window == 'before':
        async def observing():
            while not first.get('reconnect_attempts'):
                await asyncio.sleep(0)
        await asyncio.wait_for(observing(), 30)
        assert first['status'] == ('observing' if same_owner else 'queued')
        assert first['native_run_status'] == 'unknown' and 'completed_at' not in first
    latest_spec = copy.deepcopy(blueprint)
    latest_spec['project']['name'] = 'Latest C'
    latest = await operations.submit(service, latest_spec, model, stream_id='guide:one')
    assert await operations.submit(service, latest_spec, model, stream_id='guide:one') is latest
    if same_owner:
        assert first['status'] == 'published' and operations.jobs[first['id']]['admitted']
        assert first['native_run_status'] == 'unknown' and 'completed_at' not in first
        assert latest['status'] == 'queued'
    else:
        assert first['status'] == 'superseded' and first['replacement_id'] == latest['id']
        assert not operations.jobs[latest['id']]['admitted']
        if window != 'before':
            assert not first_task.done() and operations.jobs[first['id']]['draining']
    if window == 'after':
        # Supersession does not cancel the old owner's observation. Its own
        # finite window exits without changing B or reconnecting it forever.
        await asyncio.wait_for(first_task, .5)
        assert first['status'] == 'superseded' and 'observation_status' not in first
        assert first['id'] not in operations.tasks and first['id'] not in operations.jobs
        await asyncio.wait_for(replacement_read.wait(), 30)
    assert sends == []
    saved = json.loads((service.root / 'pending.json').read_text())
    assert saved['key'] == original_key and saved['admission'] == original_admission
    assert saved['idempotency_key'] == 'original-native-idempotency'
    release.set()
    # A settle bound only: the assertions below check state, not elapsed time.
    await asyncio.wait_for(asyncio.gather(first_task, *operations.tasks.values(), return_exceptions=True), 30)
    assert len(sends) == 1
    assert latest['status'] == 'completed', latest.get('error', latest)
    latest_key = service.request_binding(latest_spec, model)['native_request_key']
    assert author_reads == [latest_key]
    assert f'request {latest_key}.' in sends[0]['content']
    assert sends[0]['identity'].startswith('visualizer-request-')
    assert members.ensures[0][0] == sends[0]['identity']
    assert members.retirements == [sends[0]['identity']]
    assert latest['status'] == 'completed' and latest['artifact']['key'] == latest_key
    assert latest['artifact']['native_run']['interaction_id'] == 'native-C'
    assert reads == ['console:10'] * (2 if window != 'none' else 1)
    # Only the explicit bounded observation timeout may interrupt A's GET.
    assert observation_cancellations == (['native-A'] if window != 'none' else [])
    prior = json.loads((service.root / f'{original_key}.json').read_text())
    assert prior['native_run'] == {'status': 'completed', 'interaction_id': 'native-A',
                                  'terminal_cursor': 'console:11'}
    assert prior['publication_receipt']['request_key'] == original_key and prior['code_hash'] == code_hash
    assert prior['reference_hash'] == reference_hash
    if not same_owner:
        assert not (service.root / f'{fingerprint(request_identity(desired))}.json').exists()
        assert not (service.root / f'{service.request_binding(desired, model)["native_request_key"]}.json').exists()
    assert service.pending is None and not (service.root / 'pending.json').exists()
    assert not operations.tasks and not operations.jobs


@pytest.mark.asyncio
@pytest.mark.parametrize('shutdown', [False, True])
async def test_superseded_prior_owner_observation_stays_tracked_and_receipt_stable(shutdown):
    observing, release = asyncio.Event(), asyncio.Event()
    cancelled, sends = [], []
    class Service:
        async def generate_with_admission(self, blueprint, model, *, on_admission):
            if blueprint['version'] == 1:
                observing.set()
                try:
                    await release.wait()
                except asyncio.CancelledError:
                    cancelled.append('prior-owner-observation')
                    raise
                raise RuntimeError('prior-owner private error')
            await on_admission()
            sends.append(blueprint['version'])
            return {'code': 'published C', 'native_run': {'status': 'completed'}}
    operations = VisualizationOperations()
    first = await operations.submit(Service(), {'version': 1}, {}, stream_id='guide')
    await observing.wait()
    latest = await operations.submit(Service(), {'version': 2}, {}, stream_id='guide')
    assert first['status'] == 'superseded'
    assert set(operations.tasks) == set(operations.jobs) == {first['id'], latest['id']}
    assert cancelled == [] and sends == []
    if shutdown:
        await operations.shutdown()
        assert latest['status'] == 'interrupted' and cancelled == ['prior-owner-observation']
        assert sends == []
    else:
        release.set()
        await asyncio.wait_for(asyncio.gather(*operations.tasks.values()), .5)
        assert latest['status'] == 'completed' and sends == [2] and cancelled == []
    assert first['status'] == 'superseded' and first['replacement_id'] == latest['id']
    assert 'private' not in first.get('detail', '')
    assert not operations.tasks and not operations.jobs


@pytest.mark.asyncio
async def test_streams_coalesce_independently_and_reuse_completed_content_across_streams():
    operations, service = VisualizationOperations(), GatedService()
    active = await operations.submit(service, {'version': 1}, {}, stream_id='guide:one')
    await service.started.wait()
    obsolete = await operations.submit(service, {'version': 2}, {}, stream_id='guide:one')
    other = await operations.submit(service, {'version': 2}, {}, stream_id='guide:two')
    newest = await operations.submit(service, {'version': 3}, {}, stream_id='guide:one')
    assert obsolete['status'] == 'superseded' and obsolete['replacement_id'] == newest['id']
    assert other['status'] == 'queued' and other['id'] != obsolete['id']
    assert operations.get(other['id']) is other
    service.release.set()
    await asyncio.gather(*operations.tasks.values())
    assert service.calls == [1, 2, 3]
    assert active['status'] == other['status'] == newest['status'] == 'completed'
    cached = await operations.submit(service, {'version': 2}, {}, stream_id='guide:three')
    assert cached['status'] == 'completed' and cached['artifact'] == {**other['artifact'], 'cached': True}
    assert not operations.tasks and service.calls == [1, 2, 3]


@pytest.mark.asyncio
async def test_revisiting_superseded_content_keeps_reconnect_chain_acyclic():
    operations, service = VisualizationOperations(), GatedService()
    await operations.submit(service, {'version': 1}, {}, stream_id='guide')
    await service.started.wait()
    old = await operations.submit(service, {'version': 2}, {}, stream_id='guide')
    middle = await operations.submit(service, {'version': 3}, {}, stream_id='guide')
    latest = await operations.submit(service, {'version': 2}, {}, stream_id='guide')
    assert old['id'] != latest['id']
    assert operations.get(old['replacement_id']) is middle
    assert operations.get(middle['replacement_id']) is latest
    assert await operations.submit(service, {'version': 2}, {}, stream_id='guide') is latest
    service.release.set()
    await asyncio.gather(*operations.tasks.values())
    assert service.calls == [1, 2]
    assert old['status'] == middle['status'] == 'superseded' and latest['status'] == 'completed'


@pytest.mark.asyncio
async def test_shutdown_marks_queued_and_admitted_requests_without_losing_supersession():
    operations, service = VisualizationOperations(), GatedService()
    active = await operations.submit(service, {'version': 1}, {}, stream_id='guide')
    await service.started.wait()
    obsolete = await operations.submit(service, {'version': 2}, {}, stream_id='guide')
    latest = await operations.submit(service, {'version': 3}, {}, stream_id='guide')
    # latest has not had an opportunity to start its coroutine yet.
    await operations.shutdown()
    assert service.cancellations == 1 and service.calls == [1]
    assert active['status'] == latest['status'] == 'interrupted'
    assert obsolete['status'] == 'superseded' and obsolete['replacement_id'] == latest['id']
    assert not operations.tasks and not operations.jobs
    retry = await operations.submit(service, {'version': 3}, {}, stream_id='guide')
    await asyncio.gather(*operations.tasks.values())
    assert retry['status'] == 'completed' and retry['id'] != latest['id']


@pytest.mark.asyncio
async def test_queued_input_is_frozen_until_admission():
    operations, service = VisualizationOperations(), GatedService()
    await operations.submit(service, {'version': 1}, {}, stream_id='guide')
    await service.started.wait()
    draft = {'version': 2}
    latest = await operations.submit(service, draft, {}, stream_id='guide')
    draft['version'] = 99
    service.release.set()
    await asyncio.gather(*operations.tasks.values())
    assert service.calls == [1, 2] and latest['artifact']['code'] == 'return 2;'


@pytest.mark.asyncio
async def test_published_output_is_delivered_while_native_tail_drains_and_next_spec_waits(publication_clock):
    observing, release = asyncio.Event(), asyncio.Event()
    calls = []
    class Service:
        async def generate(self, blueprint, model):
            calls.append(blueprint['version'])
            publication_clock.advance(3)
            return {'key': str(blueprint['version']), 'code': 'real published code', 'native_run': {'status': 'unknown'}}

        async def observe_generation(self, key):
            if key == '1':
                observing.set()
                await release.wait()
            return {'key': key, 'code': 'real published code', 'native_run': {'status': 'completed'}}
    service, operations = Service(), VisualizationOperations()
    first = await operations.submit(service, {'version': 1}, {}, stream_id='guide')
    await observing.wait()
    assert first['status'] == 'published' and first['native_run_status'] == 'unknown'
    assert first['artifact']['code'] == 'real published code' and 'completed_at' not in first
    first_published_at = first['published_at']
    assert first['queued_at'] == first['started_at'] == '2026-10-03T12:00:00+00:00'
    assert first_published_at == '2026-10-03T12:00:03+00:00'
    assert await operations.submit(service, {'version': 1}, {}, stream_id='guide') is first
    second = await operations.submit(service, {'version': 2}, {}, stream_id='guide')
    assert second['status'] == 'queued' and calls == [1]
    publication_clock.advance(60)
    release.set()
    await asyncio.gather(*operations.tasks.values())
    assert calls == [1, 2] and first['status'] == second['status'] == 'completed'
    assert first['queued_at'] == first['started_at'] == '2026-10-03T12:00:00+00:00'
    assert first['published_at'] == first_published_at
    assert first['completed_at'] == '2026-10-03T12:01:03+00:00'
    assert second['queued_at'] == '2026-10-03T12:00:03+00:00'
    assert second['started_at'] == '2026-10-03T12:01:03+00:00'
    assert second['published_at'] == second['completed_at'] == '2026-10-03T12:01:06+00:00'
    assert second['published_at'] != first_published_at


@pytest.mark.asyncio
async def test_stopping_published_observation_reaps_task_without_erasing_output_or_inventing_terminal():
    observing = asyncio.Event()
    stopped = []
    class Service:
        async def generate(self, blueprint, model):
            return {'key': 'accepted', 'code': 'real published code', 'native_run': {'status': 'unknown'}}

        async def observe_generation(self, key):
            observing.set()
            try:
                await asyncio.Event().wait()
            finally:
                stopped.append(key)
    operations = VisualizationOperations()
    receipt = await operations.submit(Service(), {}, {})
    await observing.wait()
    await operations.shutdown()
    assert stopped == ['accepted'] and not operations.tasks and not operations.jobs
    assert receipt['status'] == 'published' and receipt['artifact']['code'] == 'real published code'
    assert receipt['native_run_status'] == 'unknown' and receipt['observation_status'] == 'interrupted'
    assert 'completed_at' not in receipt


@pytest.mark.asyncio
async def test_interrupted_native_run_with_valid_publication_is_never_reported_completed():
    class Service:
        async def generate(self, blueprint, model):
            return {'key': 'accepted', 'code': 'real published code', 'native_run': {'status': 'interrupted'}}
    operations = VisualizationOperations()
    receipt = await operations.submit(Service(), {}, {})
    await asyncio.gather(*operations.tasks.values())
    assert receipt['status'] == 'published' and receipt['native_run_status'] == 'interrupted'
    assert 'completed_at' not in receipt


class ContinuousService:
    """Owned native receipts with no actual runtime or model calls."""
    def __init__(self):
        from launchkit.projects import fingerprint
        self.reference_hash = fingerprint({'reference': 'current'})
        self.records = {}
        self.calls = []
        self.started = asyncio.Event()
        self.release = None
        self.borrowed_result = None

    def request_binding(self, blueprint, model, feedback='', *, revision_context=None):
        import hashlib
        from launchkit.projects import fingerprint
        from launchkit.visualizer import CONTRACT_VERSION, revision_context_binding
        binding = {'contract': CONTRACT_VERSION, 'blueprint_hash': fingerprint(blueprint),
                   'model_hash': fingerprint(model), 'reference_hash': self.reference_hash}
        if feedback:
            binding['feedback_hash'] = hashlib.sha256(feedback.encode()).hexdigest()
        if revision_context is not None:
            binding['revision_binding'] = revision_context_binding(revision_context)
        return {**binding, 'native_request_key': fingerprint(binding)}

    def saved_request(self, key, *, interaction_id=None):
        from copy import deepcopy
        from launchkit.visualization_timeline import checked_request_record
        value = self.records.get(key)
        if value is None or interaction_id is not None and value['admission']['interaction_id'] != interaction_id:
            return None
        return checked_request_record(deepcopy(value))

    def can_reattach(self, key):
        return key in self.records

    async def generate_with_native_admission(self, blueprint, model, *, on_admission, on_native_admission, feedback=''):
        return await self._produce(blueprint, model, None, on_admission, on_native_admission, feedback)

    async def generate_continuous_with_native_admission(self, blueprint, model, *, revision_context,
                                                        on_admission, on_native_admission, feedback=''):
        return await self._produce(blueprint, model, revision_context, on_admission, on_native_admission, feedback)

    async def _produce(self, blueprint, model, context, on_admission, on_native_admission, feedback):
        import hashlib
        from copy import deepcopy
        await on_admission()
        source = self.request_binding(blueprint, model, feedback, revision_context=context)
        key = source.pop('native_request_key')
        number = len(self.calls) + 1
        admission = {'interaction_id': f'native-{number}', 'session_id': 'owned-session', 'cursor': f'console:{number * 10}'}
        await on_native_admission(key, admission)
        self.calls.append({'blueprint': deepcopy(blueprint), 'context': deepcopy(context), 'key': key})
        self.started.set()
        if self.release is not None and number == 1:
            await self.release.wait()
        code = f'return {{duration:1,render(frame){{ /* scene {number} */ }}}};'
        code_hash = hashlib.sha256(code.encode()).hexdigest()
        native_run = {'status': 'completed', 'interaction_id': admission['interaction_id'],
                      'terminal_cursor': f'console:{number * 10 + 1}'}
        self.records[key] = {**deepcopy(source), 'key': key, 'admission': admission, 'native_run': native_run}
        artifact = {**source, 'key': key, 'code': code, 'code_hash': code_hash,
                    'summary': f'Published scene {number}', 'native_admission': admission, 'native_run': native_run,
                    'publication_receipt': {**deepcopy(source), 'request_key': key, 'code_hash': code_hash,
                                            'authority': 'publish_visualization'}}
        return deepcopy(self.borrowed_result) if self.borrowed_result is not None else artifact


async def finish_operations(operations):
    await asyncio.gather(*list(operations.tasks.values()))


@pytest.mark.asyncio
async def test_latest_saved_adoption_keeps_exact_native_source_and_other_viewers(tmp_path, monkeypatch, publication_clock):
    from copy import deepcopy
    from launchkit.projects import fingerprint
    from launchkit.visualization_consumers import VisualizationIntentConflict
    service = ContinuousService()
    monkeypatch.setattr('launchkit.visualizer.visualization_reference_hash', lambda: service.reference_hash)
    ops = VisualizationOperations(root=tmp_path)
    blueprint, model = {'version': 1}, {'nodes': []}
    first_intent = await ops.declare_intent('18017cba-330e-4282-9e1d-1f331d82cd19', 1, 'setup', 'setup:overview')
    intent = {name: first_intent[name] for name in ('consumer_id', 'intent_revision', 'setup_id', 'stream_id', 'intent_token')}
    first = await ops.submit(service, blueprint, model, setup_id='setup', stream_id='setup:overview', consumer_intent=intent)
    await finish_operations(ops)
    publication_clock.advance(1)
    second = await ops.submit(service, blueprint, model, feedback='Preserve the approved composition.', setup_id='setup', stream_id='setup:overview')
    await finish_operations(ops)
    before = deepcopy(ops.receipts)
    source_files = {receipt['id']: (tmp_path / (receipt['id'] + '.json')).read_bytes() for receipt in (first, second)}
    scope = dict(setup_id='setup', stream_id='setup:overview', blueprint_hash=fingerprint(blueprint), model_hash=fingerprint(model))
    assert ops.latest_saved(service, **scope)['id'] == second['id']
    # A retained older completion cannot move the durable saved pointer back.
    ops._remember_publication(first, service)
    restored = VisualizationOperations(root=tmp_path)
    assert restored.latest_saved(service, **scope)['id'] == second['id']
    declared = await ops.declare_intent('87e420ed-c806-4db4-9f4f-e6039b3e08d4', 1, 'setup', 'setup:overview')
    fresh = {name: declared[name] for name in intent}
    parent = {**scope, 'operation_id': second['id'], 'code_hash': second['artifact']['code_hash']}
    attached = await ops.adopt_saved(service, publication=parent, consumer_intent=fresh)
    assert attached == before[second['id']]
    assert ops.consumers.current(intent)['receipt_id'] == first['id']
    assert ops.consumers.current(fresh)['receipt_id'] == second['id']
    assert len(service.calls) == 2 and not ops.tasks
    assert all((tmp_path / (identity + '.json')).read_bytes() == raw for identity, raw in source_files.items())
    # A response-loss replay remains the exact selection after a newer publication.
    publication_clock.advance(1)
    await ops.submit(service, blueprint, model, feedback='A further ordinary revision.', setup_id='setup', stream_id='setup:overview')
    await finish_operations(ops)
    assert (await ops.adopt_saved(service, publication=parent, consumer_intent=fresh))['id'] == second['id']
    # Reload restores the previously explicit source choice, even if another
    # publication exists, under a new live viewer rather than old feedback.
    restored_intent = await ops.declare_intent('cd297393-4d0d-4124-884d-b7971259b5c1', 1, 'setup', 'setup:overview')
    restored_binding = {name: restored_intent[name] for name in intent}
    selected = await ops.adopt_saved(service, publication=parent, consumer_intent=restored_binding, require_latest=False)
    assert selected['id'] == second['id'] and len(service.calls) == 3
    with pytest.raises(VisualizationIntentConflict):
        await ops.adopt_saved(service, publication=parent, consumer_intent=intent)
    await ops.shutdown()
    await restored.shutdown()


@pytest.mark.asyncio
async def test_saved_selection_refuses_wrong_scope_changed_source_and_stale_choice(monkeypatch, publication_clock):
    from launchkit.projects import fingerprint
    from launchkit.visualization_operations import VisualizationConflict
    service, ops = ContinuousService(), VisualizationOperations()
    monkeypatch.setattr('launchkit.visualizer.visualization_reference_hash', lambda: service.reference_hash)
    first = await ops.submit(service, {'version': 1}, {}, setup_id='setup', stream_id='setup:overview')
    await finish_operations(ops)
    scope = dict(setup_id='setup', stream_id='setup:overview', blueprint_hash=fingerprint({'version': 1}), model_hash=fingerprint({}))
    for change in ({'setup_id': 'other'}, {'stream_id': 'other:view'}, {'blueprint_hash': 'b' * 64}, {'model_hash': 'c' * 64}):
        assert ops.latest_saved(service, **{**scope, **change}) is None
    declared = await ops.declare_intent('18017cba-330e-4282-9e1d-1f331d82cd19', 1, 'setup', 'setup:overview')
    intent = {name: declared[name] for name in ('consumer_id', 'intent_revision', 'setup_id', 'stream_id', 'intent_token')}
    parent = {**scope, 'operation_id': first['id'], 'code_hash': first['artifact']['code_hash']}
    first['artifact']['code'] += ' /* changed after publication */'
    assert ops.latest_saved(service, **scope) is None
    with pytest.raises(VisualizationConflict):
        await ops.adopt_saved(service, publication=parent, consumer_intent=intent)
    with pytest.raises(VisualizationConflict):
        await ops.adopt_saved(service, publication={**parent, 'code_hash': 'd' * 64},
                              consumer_intent=intent, require_latest=False)
    first['artifact']['code'] = first['artifact']['code'].removesuffix(' /* changed after publication */')
    publication_clock.advance(1)
    await ops.submit(service, {'version': 1}, {}, feedback='New saved drawing.', setup_id='setup', stream_id='setup:overview')
    await finish_operations(ops)
    with pytest.raises(VisualizationConflict):
        await ops.adopt_saved(service, publication=parent, consumer_intent=intent)
    assert ops.consumers.current(intent)['receipt_id'] is None and len(service.calls) == 2
    await ops.shutdown()


@pytest.mark.asyncio
@pytest.mark.parametrize('cached,legacy_missing_start', [(True, False), (False, False), (False, True)])
async def test_durable_published_receipt_reattachment_retains_first_boundaries(
        tmp_path, publication_clock, cached, legacy_missing_start):
    from copy import deepcopy

    class TailService(ContinuousService):
        def __init__(self):
            super().__init__()
            self.observing = asyncio.Event()
            self.tail = asyncio.Event()
            self.reattach = False
            self.reattachments = 0
            self.published = None
            self.terminal = None

        async def _produce(self, blueprint, model, context, on_admission, on_native_admission, feedback):
            if self.reattach:
                self.reattachments += 1
                artifact = deepcopy(self.published)
                if cached:
                    artifact['cached'] = True
                else:
                    await on_admission()
                    await on_native_admission(artifact['key'], artifact['native_admission'])
                artifact['native_run'] = deepcopy(self.terminal)
                self.records[artifact['key']]['native_run'] = deepcopy(artifact['native_run'])
                return artifact

            async def first_admission():
                await on_admission()
                publication_clock.advance(3)

            artifact = await super()._produce(blueprint, model, context, first_admission, on_native_admission, feedback)
            self.terminal = deepcopy(artifact['native_run'])
            artifact['native_run'] = {'status': 'unknown',
                                      'interaction_id': artifact['native_admission']['interaction_id']}
            self.records[artifact['key']].pop('native_run')
            self.published = deepcopy(artifact)
            return artifact

        async def observe_generation(self, key):
            self.observing.set()
            await self.tail.wait()

    service = TailService()
    operations = VisualizationOperations(root=tmp_path)
    original = await operations.submit(service, {'version': 1}, {}, setup_id='setup', stream_id='overview')
    await service.observing.wait()
    assert original['status'] == 'published' and 'completed_at' not in original
    first_published_at = original['published_at']
    assert original['queued_at'] == original['started_at'] == '2026-10-03T12:00:00+00:00'
    original_admission = deepcopy(original['native_admission'])
    await operations.shutdown()
    assert original['status'] == 'published' and original['observation_status'] == 'interrupted'
    assert not operations.tasks and not operations.jobs
    if legacy_missing_start:
        # A missing legacy interval stays unknown until a new actual admission.
        original.pop('started_at')
        operations._persist(original)
    publication_clock.advance(60)
    service.reattach = True
    restored = VisualizationOperations(root=tmp_path)
    replay = await restored.submit(service, {'version': 1}, {}, setup_id='setup', stream_id='overview')
    assert replay['queued_at'] == original['queued_at']
    assert replay['published_at'] == first_published_at and 'completed_at' not in replay
    if legacy_missing_start:
        assert 'started_at' not in replay
    else:
        assert replay['started_at'] == original['started_at']
    await finish_operations(restored)
    assert replay['id'] == original['id'] and replay['native_admission'] == original_admission
    assert replay['status'] == 'completed' and replay['artifact'].get('cached', False) is cached
    assert replay['queued_at'] == '2026-10-03T12:00:00+00:00'
    if legacy_missing_start:
        assert replay['started_at'] == '2026-10-03T12:01:03+00:00'
        assert replay['started_at'] > replay['published_at']
    else:
        assert replay['started_at'] == '2026-10-03T12:00:00+00:00'
    assert replay['published_at'] == first_published_at == '2026-10-03T12:00:03+00:00'
    assert replay['completed_at'] == '2026-10-03T12:01:03+00:00'
    assert service.reattachments == 1 and len(service.calls) == 1
    saved = VisualizationOperations(root=tmp_path).get(original['id'])
    assert all(saved[name] == replay[name] for name in ('queued_at', 'started_at', 'published_at', 'completed_at'))


@pytest.mark.asyncio
async def test_continuous_revision_freezes_exact_prior_source_and_publishes_new_result(tmp_path):
    from copy import deepcopy
    from launchkit.projects import fingerprint
    from launchkit.visualization_timeline import publication
    operations, service = VisualizationOperations(root=tmp_path), ContinuousService()
    first = await operations.submit(service, {'version': 1}, {}, setup_id='setup-A', stream_id='overview')
    await finish_operations(operations)
    assert first['revision_context']['previous_program'] is None
    original = deepcopy(first)
    second = await operations.submit(service, {'version': 2}, {}, setup_id='setup-A', stream_id='overview')
    prior = second['revision_context']['previous_program']
    assert prior == {name: first['artifact'][name] for name in (
        'contract', 'blueprint_hash', 'model_hash', 'reference_hash', 'code_hash', 'code', 'summary')} | {
            'operation_id': first['id'], 'request_key': first['native_request_key'], 'matches_current': False}
    assert second['request_fingerprint'] == fingerprint({'blueprint': {'version': 2}, 'model': {}})
    await finish_operations(operations)
    assert service.calls[-1]['context'] == second['revision_context']
    assert second['artifact']['code'] != prior['code'] and second['artifact']['key'] != prior['request_key']
    assert second['artifact']['blueprint_hash'] == fingerprint({'version': 2})
    assert publication(second)['available'] and first == original
    assert operations._published_pointer('setup-A', 'overview').is_file()


@pytest.mark.asyncio
async def test_continuous_submission_snapshots_inputs_before_waiting_for_its_lock():
    from copy import deepcopy
    from launchkit.projects import fingerprint
    operations, service = VisualizationOperations(), ContinuousService()
    blueprint = {'project': {'version': 1}}
    model = {'nodes': [{'name': 'Original context owner'}]}
    original_blueprint, original_model = deepcopy(blueprint), deepcopy(model)
    await operations.lock.acquire()
    try:
        task = asyncio.create_task(operations.submit(service, blueprint, model,
            setup_id='setup-A', stream_id='overview'))
        await asyncio.sleep(0)
        assert not task.done()
        blueprint['project']['version'] = 2
        model['nodes'][0]['name'] = 'Later unrelated mutation'
    finally:
        operations.lock.release()
    receipt = await task
    await finish_operations(operations)
    assert receipt['status'] == 'completed'
    assert service.calls[0]['blueprint'] == original_blueprint
    assert receipt['request_fingerprint'] == fingerprint({'blueprint': original_blueprint, 'model': original_model})
    assert receipt['blueprint_hash'] == fingerprint(original_blueprint)
    assert receipt['model_hash'] == receipt['artifact']['model_hash'] == fingerprint(original_model)


@pytest.mark.asyncio
async def test_continuous_setup_and_view_keys_isolate_context_and_native_cache(tmp_path):
    operations, service = VisualizationOperations(root=tmp_path), ContinuousService()
    receipts = []
    for setup, view in [('setup-A', 'overview'), ('setup-B', 'overview'), ('setup-A', 'flow:one')]:
        receipt = await operations.submit(service, {'version': 1}, {}, setup_id=setup, stream_id=view)
        await finish_operations(operations)
        receipts.append(receipt)
    assert len({receipt['native_request_key'] for receipt in receipts}) == 3
    assert len({receipt['request_fingerprint'] for receipt in receipts}) == 1
    assert len(service.calls) == 3 and all(call['context']['previous_program'] is None for call in service.calls)
    for setup, view, prior in [('setup-A', 'overview', receipts[0]), ('setup-B', 'overview', receipts[1])]:
        receipt = await operations.submit(service, {'version': 2}, {}, setup_id=setup, stream_id=view)
        assert receipt['revision_context']['previous_program']['operation_id'] == prior['id']
        await finish_operations(operations)
    assert len(service.calls) == 5


@pytest.mark.asyncio
async def test_continuous_queued_snapshot_does_not_adopt_late_prior_and_superseded_work_never_sends():
    from copy import deepcopy
    operations, service = VisualizationOperations(), ContinuousService()
    service.release = asyncio.Event()
    first = await operations.submit(service, {'version': 1}, {}, setup_id='setup', stream_id='overview')
    await service.started.wait()
    queued = await operations.submit(service, {'version': 2}, {}, setup_id='setup', stream_id='overview')
    latest = await operations.submit(service, {'version': 3}, {}, setup_id='setup', stream_id='overview')
    frozen = deepcopy(latest['revision_context'])
    assert queued['status'] == 'superseded' and frozen['previous_program'] is None
    service.release.set()
    await finish_operations(operations)
    assert [call['blueprint']['version'] for call in service.calls] == [1, 3]
    assert latest['revision_context'] == frozen == service.calls[-1]['context']
    assert first['status'] == latest['status'] == 'completed' and 'native_admission' not in queued


@pytest.mark.asyncio
async def test_continuous_exact_replay_retains_prior_across_new_publication_and_restart(tmp_path, publication_clock):
    from copy import deepcopy
    operations, service = VisualizationOperations(root=tmp_path), ContinuousService()
    receipts = []
    for version in (1, 2, 3):
        publication_clock.advance(5)
        receipt = await operations.submit(service, {'version': version}, {}, setup_id='setup', stream_id='overview')
        await finish_operations(operations)
        receipts.append(receipt)
    frozen = deepcopy(receipts[1])
    saved_bytes = (tmp_path / (frozen['id'] + '.json')).read_bytes()
    assert len({receipt['published_at'] for receipt in receipts}) == 3
    publication_clock.advance(90)
    restored = VisualizationOperations(root=tmp_path)
    replay = await restored.submit(service, {'version': 2}, {}, setup_id='setup', stream_id='overview')
    assert replay == frozen and len(service.calls) == 3 and not restored.tasks
    assert (tmp_path / (frozen['id'] + '.json')).read_bytes() == saved_bytes
    assert replay['revision_context']['previous_program']['operation_id'] == receipts[0]['id']
    next_receipt = await restored.submit(service, {'version': 4}, {}, setup_id='setup', stream_id='overview')
    assert next_receipt['revision_context']['previous_program']['operation_id'] == receipts[2]['id']
    await finish_operations(restored)


@pytest.mark.asyncio
@pytest.mark.parametrize('damage', ['context', 'binding', 'scope', 'native-key', 'missing-context', 'missing-binding', 'missing-both'])
async def test_continuous_get_refuses_tampered_context_binding_and_scope(tmp_path, damage):
    import json
    from copy import deepcopy
    operations, service = VisualizationOperations(root=tmp_path), ContinuousService()
    receipt = await operations.submit(service, {'version': 1}, {}, setup_id='setup', stream_id='overview')
    await finish_operations(operations)
    changed = deepcopy(receipt)
    if damage == 'context':
        changed['revision_context']['extra'] = 'not authored authority'
    elif damage == 'binding':
        changed['revision_binding']['context_hash'] = '0' * 64
    elif damage == 'scope':
        changed['setup_id'] = 'other-setup'
    elif damage.startswith('missing-'):
        if damage in {'missing-context', 'missing-both'}:
            changed.pop('revision_context')
        if damage in {'missing-binding', 'missing-both'}:
            changed.pop('revision_binding')
    else:
        changed['native_request_key'] = '0' * 64
    operations.receipts[receipt['id']] = changed
    assert operations.get(receipt['id']) is None
    (tmp_path / (receipt['id'] + '.json')).write_text(json.dumps(changed))
    assert VisualizationOperations(root=tmp_path).get(receipt['id']) is None


@pytest.mark.asyncio
@pytest.mark.parametrize('damage', ['code', 'invalid-unicode', 'summary', 'missing-admission', 'old-contract', 'new-reference', 'legacy-unverified'])
async def test_continuous_invalid_prior_is_optional_and_cannot_become_new_source(tmp_path, damage):
    from launchkit.projects import fingerprint
    operations, service = VisualizationOperations(root=tmp_path), ContinuousService()
    first = await operations.submit(service, {'version': 1}, {}, setup_id='setup', stream_id='overview')
    await finish_operations(operations)
    if damage == 'code':
        first['artifact']['code'] += ' altered'
    elif damage == 'invalid-unicode':
        first['artifact']['code'] += '\ud800'
    elif damage == 'summary':
        first['artifact']['summary'] = 'x' * 2001
    elif damage == 'missing-admission':
        service.records.clear()
    elif damage == 'old-contract':
        first['artifact']['contract'] = 'visual-layout-v2'
    elif damage == 'new-reference':
        service.reference_hash = fingerprint({'reference': 'new'})
    else:
        first['artifact'].pop('publication_receipt')
        first.pop('revision_context')
        first.pop('revision_binding')
    next_receipt = await operations.submit(service, {'version': 2}, {}, setup_id='setup', stream_id='overview')
    assert next_receipt['revision_context']['previous_program'] is None
    await finish_operations(operations)
    assert next_receipt['status'] == 'completed' and len(service.calls) == 2


@pytest.mark.asyncio
@pytest.mark.parametrize('damage', ['wrong-scope', 'foreign-id', 'non-object', 'symlink'])
async def test_continuous_restart_ignores_invalid_or_foreign_publication_pointer(tmp_path, damage):
    import json
    operations, service = VisualizationOperations(root=tmp_path), ContinuousService()
    foreign = await operations.submit(service, {'version': 1}, {}, setup_id='other', stream_id='overview')
    await finish_operations(operations)
    pointer = operations._published_pointer('setup', 'overview')
    value = {'setup_id': 'setup', 'stream_id': 'overview', 'id': foreign['id']}
    if damage == 'wrong-scope':
        value['setup_id'] = 'other'
    if damage == 'symlink':
        pointer.symlink_to(operations._published_pointer('other', 'overview'))
    else:
        pointer.write_text(json.dumps([] if damage == 'non-object' else value))
    restored = VisualizationOperations(root=tmp_path)
    current = await restored.submit(service, {'version': 2}, {}, setup_id='setup', stream_id='overview')
    assert current['revision_context']['previous_program'] is None
    await finish_operations(restored)
    assert current['status'] == 'completed' and len(service.calls) == 2


@pytest.mark.asyncio
async def test_continuous_result_must_bind_current_request_not_advisory_prior(tmp_path):
    from copy import deepcopy
    operations, service = VisualizationOperations(root=tmp_path), ContinuousService()
    first = await operations.submit(service, {'version': 1}, {}, setup_id='setup', stream_id='overview')
    await finish_operations(operations)
    service.borrowed_result = deepcopy(first['artifact'])
    second = await operations.submit(service, {'version': 2}, {}, setup_id='setup', stream_id='overview')
    await finish_operations(operations)
    assert second['revision_context']['previous_program']['operation_id'] == first['id']
    assert second['status'] == 'failed' and 'artifact' not in second
    assert ('native', second['native_request_key']) not in operations.completed


@pytest.mark.asyncio
async def test_continuous_can_use_loaded_verified_legacy_publication_without_directory_scan(tmp_path, monkeypatch):
    operations, service = VisualizationOperations(root=tmp_path), ContinuousService()
    method = service.generate_continuous_with_native_admission
    service.generate_continuous_with_native_admission = None
    first = await operations.submit(service, {'version': 1}, {}, setup_id='setup', stream_id='overview')
    await finish_operations(operations)
    assert 'revision_context' not in first
    service.generate_continuous_with_native_admission = method
    restored = VisualizationOperations(root=tmp_path)
    assert restored.get(first['id']) == first
    def no_scan(*args, **kwargs):
        raise AssertionError('No broad directory scan is allowed.')
    monkeypatch.setattr(type(tmp_path), 'iterdir', no_scan)
    next_receipt = await restored.submit(service, {'version': 2}, {}, setup_id='setup', stream_id='overview')
    assert next_receipt['revision_context']['previous_program']['operation_id'] == first['id']
    await finish_operations(restored)
