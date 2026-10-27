"""Offline adversarial scheduling journeys, with no native or model execution.

The service below records simulated sends and inert publication text. The real
coordinator and its temporary durable store own all demand/admission decisions.
Saved review authorization is a server boundary, tested separately by that owner.
"""
import asyncio
from copy import deepcopy
import hashlib
import json
import uuid

import pytest

from launchkit.projects import fingerprint
from launchkit.visualization_operations import (
    VisualizationConflict, VisualizationIntentConflict, VisualizationOperations,
)
from launchkit.visualizer import VisualizerPending
from test_visualization_operations import ContinuousService


INTENT_FIELDS = ('consumer_id', 'intent_revision', 'setup_id', 'stream_id', 'intent_token')


def body(ack):
    return {name: ack[name] for name in INTENT_FIELDS}


def consumer():
    return str(uuid.uuid4())


def parent_binding(receipt):
    return {'operation_id': receipt['id'], 'code_hash': receipt['artifact']['code_hash'],
            **{name: receipt[name] for name in ('blueprint_hash', 'model_hash', 'setup_id', 'stream_id')}}


class LeaseClock:
    """Deterministic host clock for leases, not a latency measurement."""
    def __init__(self):
        self.value = 1_800_000_000

    def __call__(self):
        return self.value

    def advance(self, seconds):
        self.value += seconds


class OfflineDemandService(ContinuousService):
    """Gated, source-bound offline sends; generated text is never executed."""
    def __init__(self, root, *, before_admission=False, after_send=False,
                 lost_response=False, early_publication=False):
        super().__init__()
        self.root = root
        self.entered = asyncio.Event()
        self.before = asyncio.Event()
        self.after = asyncio.Event()
        self.waiting = asyncio.Event()
        self.cancelled = []
        self.invocations = []
        self.reattachable = True
        self.lost_response = lost_response
        self.early_publication = early_publication
        self.lost_keys = set()
        self.artifacts = {}
        if not before_admission:
            self.before.set()
        if not after_send:
            self.after.set()

    def can_reattach(self, key):
        return self.reattachable and super().can_reattach(key)

    def ledger_path(self, key):
        return self.root / (key + '.json')

    def save_record(self, key):
        self.root.mkdir(parents=True, exist_ok=True)
        self.ledger_path(key).write_text(json.dumps(self.records[key], sort_keys=True))

    def artifact(self, key, *, complete):
        record = self.records[key]
        source = {name: record[name] for name in (
            'contract', 'blueprint_hash', 'model_hash', 'reference_hash', 'revision_binding')
                  if name in record}
        if 'feedback_hash' in record:
            source['feedback_hash'] = record['feedback_hash']
        code = 'return {duration:1,render(frame){ /* inert offline ' + key + ' */ }};'
        code_hash = hashlib.sha256(code.encode()).hexdigest()
        admission = deepcopy(record['admission'])
        native_run = {'status': 'completed' if complete else 'unknown',
                      'interaction_id': admission['interaction_id']}
        if complete:
            native_run['terminal_cursor'] = 'console:1000'
        return {**source, 'key': key, 'code': code, 'code_hash': code_hash,
                'summary': 'Offline source-bound publication', 'native_admission': admission,
                'native_run': native_run, 'publication_receipt': {
                    **source, 'request_key': key, 'code_hash': code_hash,
                    'authority': 'publish_visualization'}}

    async def _produce(self, blueprint, model, context, on_admission, on_native_admission, feedback):
        self.invocations.append(deepcopy(blueprint))
        self.entered.set()
        at_admission_fence = False
        try:
            await self.before.wait()
            at_admission_fence = True
            await on_admission()
            at_admission_fence = False
            source = self.request_binding(blueprint, model, feedback, revision_context=context)
            key = source.pop('native_request_key')
            if key not in self.records:
                number = len(self.calls) + 1
                admission = {'interaction_id': f'offline-native-{number}',
                             'session_id': 'offline-owned-session', 'cursor': f'console:{number * 10}'}
                await on_native_admission(key, admission)
                self.records[key] = {**deepcopy(source), 'key': key, 'admission': admission,
                                     'idempotency_key': 'offline-idempotency-' + key}
                self.save_record(key)
                self.calls.append({'blueprint': deepcopy(blueprint), 'model': deepcopy(model),
                                   'context': deepcopy(context), 'key': key})
                self.started.set()
            else:
                await on_native_admission(key, self.records[key]['admission'])
            if self.lost_response and key not in self.lost_keys:
                self.lost_keys.add(key)
                raise VisualizerPending({'key': key, 'admission': self.records[key]['admission']})
            if self.early_publication:
                return self.artifact(key, complete=False)
            self.waiting.set()
            await self.after.wait()
            artifact = self.artifact(key, complete=True)
            self.records[key]['native_run'] = deepcopy(artifact['native_run'])
            self.save_record(key)
            return artifact
        except asyncio.CancelledError:
            # Refusing this candidate at the host fence is not cancellation of
            # an observed native owner or an in-flight service wait.
            if not at_admission_fence:
                self.cancelled.append(deepcopy(blueprint))
            raise

    async def observe_generation(self, key):
        self.waiting.set()
        await self.after.wait()
        artifact = self.artifact(key, complete=True)
        self.records[key]['native_run'] = deepcopy(artifact['native_run'])
        self.save_record(key)
        return artifact


@pytest.fixture
async def hosts(tmp_path):
    owned = []

    def make(name='operations', **kwargs):
        host = VisualizationOperations(root=tmp_path / name, **kwargs)
        owned.append(host)
        return host

    yield make
    for host in reversed(owned):
        await host.shutdown()


async def settle(host):
    await asyncio.gather(*list(host.tasks.values()), return_exceptions=True)


async def until(predicate):
    async def wait():
        while not predicate():
            await asyncio.sleep(0)
    await asyncio.wait_for(wait(), 30)


async def declare(host, identity, revision, *, setup='setup-A', stream='overview'):
    ack = await host.declare_intent(identity, revision, setup, stream)
    assert ack['status'] == 'live' and ack['lease_seconds'] == 90
    assert str(uuid.UUID(ack['intent_token'])) == ack['intent_token']
    return ack


async def submit(host, service, ack, version=1, **kwargs):
    return await host.submit(service, {'version': version}, {}, setup_id=ack['setup_id'],
                             stream_id=ack['stream_id'], consumer_intent=body(ack), **kwargs)


@pytest.mark.asyncio
async def test_token_free_release_before_ack_fences_delayed_declaration_across_restart(hosts, tmp_path):
    identity, service = consumer(), OfflineDemandService(tmp_path / 'offline-ledger')
    first = hosts()
    retired = await first.release_intent(identity, 1, 'setup-A', 'overview')
    assert retired['status'] == 'released' and retired['intent_token'] is None
    assert await first.release_intent(identity, 1, 'setup-A', 'overview') == retired
    restored = hosts()
    with pytest.raises(VisualizationIntentConflict):
        await declare(restored, identity, 1)
    fresh = await declare(restored, identity, 2)
    with pytest.raises(VisualizationIntentConflict):
        await restored.submit(service, {'version': 1}, {}, setup_id='setup-A', stream_id='overview',
                              consumer_intent={**body(fresh), 'intent_revision': 1})
    assert service.calls == [] and restored.receipts == {} and not restored.tasks
    receipt = await submit(restored, service, fresh, 2)
    await settle(restored)
    assert receipt['status'] == 'completed' and [call['blueprint']['version'] for call in service.calls] == [2]


@pytest.mark.asyncio
async def test_delayed_old_commands_do_not_change_latest_navigation_or_exact_body_binding(hosts, tmp_path):
    host, service, identity = hosts(), OfflineDemandService(tmp_path / 'offline-ledger'), consumer()
    old = await declare(host, identity, 1)
    middle = await declare(host, identity, 2, stream='flow:survey')
    current = await declare(host, identity, 3)
    for ack in (old, middle):
        release = await host.release_intent(**body(ack))
        assert release['status'] == 'released' and release['intent_token'] is None
        with pytest.raises(VisualizationIntentConflict):
            await host.renew_intent(**body(ack))
        with pytest.raises(VisualizationIntentConflict):
            await submit(host, service, ack)
    assert await declare(host, identity, 3) == current
    with pytest.raises(VisualizationIntentConflict):
        await declare(host, identity, 3, stream='flow:survey')
    receipt = await submit(host, service, current, 3)
    assert await submit(host, service, current, 3) is receipt
    before = deepcopy(host.receipts)
    with pytest.raises(VisualizationIntentConflict):
        await submit(host, service, current, 4)
    assert host.receipts == before
    await settle(host)
    assert receipt['status'] == 'completed' and len(service.calls) == 1
    restored = hosts()
    assert await declare(restored, identity, 3) == current
    with pytest.raises(VisualizationIntentConflict):
        await submit(restored, service, middle)
    assert await submit(restored, service, current, 3) == receipt
    assert len(service.calls) == 1


@pytest.mark.asyncio
async def test_shared_request_survives_one_window_switch_and_retires_after_last_release(hosts, tmp_path):
    host, service = hosts(), OfflineDemandService(tmp_path / 'offline-ledger')
    await host.generation_lock.acquire()
    one = await declare(host, consumer(), 1)
    two = await declare(host, consumer(), 1)
    shared = await submit(host, service, one)
    assert await submit(host, service, two) is shared
    next_one = await declare(host, one['consumer_id'], 2, stream='flow:survey')
    next_receipt = await submit(host, service, next_one, 2)
    assert shared['scheduling'] == {'desired': True, 'managed_consumers': 1,
                                   'legacy_interest': False, 'status': 'wanted'}
    unrelated = await declare(host, consumer(), 1, setup='setup-B', stream='overview')
    other_receipt = await submit(host, service, unrelated, 3)
    await host.release_intent(**body(two))
    assert shared['scheduling']['desired'] is False
    assert next_receipt['scheduling']['desired'] and other_receipt['scheduling']['desired']
    host.generation_lock.release()
    await settle(host)
    assert [call['blueprint']['version'] for call in service.calls] == [2, 3]
    assert 'native_admission' not in shared and 'started_at' not in shared and 'completed_at' not in shared
    assert next_receipt['status'] == other_receipt['status'] == 'completed'


@pytest.mark.asyncio
@pytest.mark.parametrize('setup', ['setup-A', None])
async def test_managed_and_legacy_interest_share_factual_scope_and_native_identity(hosts, tmp_path, setup):
    host, service = hosts(), OfflineDemandService(tmp_path / 'offline-ledger')
    await host.generation_lock.acquire()
    ack = await declare(host, consumer(), 1, setup=setup)
    shared = await submit(host, service, ack)
    legacy = await host.submit(service, {'version': 1}, {}, setup_id=setup, stream_id='overview')
    assert legacy is shared and shared['scheduling']['legacy_interest']
    changed = await declare(host, ack['consumer_id'], 2, setup=setup, stream='flow:survey')
    next_receipt = await submit(host, service, changed, 2)
    await host.release_intent(**body(changed))
    assert shared['scheduling']['desired'] and shared['scheduling']['managed_consumers'] == 0
    assert host._desired_head((setup, 'overview')) == shared['id']
    assert host._desired_head((setup, 'flow:survey')) is None
    host.generation_lock.release()
    await settle(host)
    assert shared['status'] == 'completed' and len(service.calls) == 1
    call = service.calls[0]
    assert shared['setup_id'] == setup and shared['stream_id'] == 'overview'
    assert ack['consumer_id'] not in json.dumps(call)
    assert call['key'] == service.request_binding({'version': 1}, {},
                                                 revision_context=shared['revision_context'])['native_request_key']
    assert shared['request_fingerprint'] == fingerprint({'blueprint': {'version': 1}, 'model': {}})
    assert 'native_admission' not in next_receipt


@pytest.mark.asyncio
async def test_legacy_head_replacement_cannot_cancel_another_managed_interest(hosts, tmp_path):
    host, service = hosts(), OfflineDemandService(tmp_path / 'offline-ledger')
    await host.generation_lock.acquire()
    ack = await declare(host, consumer(), 1)
    managed = await submit(host, service, ack)
    assert await host.submit(service, {'version': 1}, {}, setup_id='setup-A', stream_id='overview') is managed
    legacy_next = await host.submit(service, {'version': 2}, {}, setup_id='setup-A', stream_id='overview')
    assert managed['scheduling']['desired'] and managed['scheduling']['managed_consumers'] == 1
    host.generation_lock.release()
    await settle(host)
    assert [call['blueprint']['version'] for call in service.calls] == [1, 2]
    assert managed['status'] == legacy_next['status'] == 'completed'


@pytest.mark.asyncio
async def test_cold_restored_get_projects_shared_legacy_demand_without_recursion_or_native_claims(hosts, tmp_path):
    first, service = hosts(), OfflineDemandService(tmp_path / 'offline-ledger')
    ack = await declare(first, consumer(), 1)
    shared = await submit(first, service, ack)
    assert await first.submit(service, {'version': 1}, {}, setup_id='setup-A', stream_id='overview') is shared
    await first.release_intent(**body(ack))
    actual = {name: deepcopy(shared[name]) for name in (
        'id', 'status', 'queued_at', 'native_request_key', 'blueprint_hash', 'model_hash', 'revision_context')}
    # This queued host coroutine has not run. The reconstructed coordinator has
    # no cached stream pointer or job; get must load the actual legacy pointer.
    task = first.tasks[shared['id']]
    task.cancel()
    await asyncio.gather(task, return_exceptions=True)
    restored = hosts()
    assert not restored.streams and not restored.tasks
    for _ in range(3):
        observed = restored.get(shared['id'])
        assert observed is not None
        assert {name: observed[name] for name in actual} == actual
        assert observed['scheduling'] == {'desired': True, 'managed_consumers': 0,
                                          'legacy_interest': True, 'status': 'wanted'}
        assert 'native_admission' not in observed and 'started_at' not in observed and 'completed_at' not in observed
    assert not service.calls
    replay = await restored.submit(service, {'version': 1}, {}, setup_id='setup-A', stream_id='overview')
    await settle(restored)
    assert replay['id'] == shared['id'] and replay['status'] == 'completed' and len(service.calls) == 1


@pytest.mark.asyncio
async def test_release_wins_final_admission_fence_without_simulated_send(hosts, tmp_path):
    host = hosts()
    service = OfflineDemandService(tmp_path / 'offline-ledger', before_admission=True)
    ack = await declare(host, consumer(), 1)
    receipt = await submit(host, service, ack)
    await asyncio.wait_for(service.entered.wait(), 30)
    await host.release_intent(**body(ack))
    assert service.cancelled == []
    service.before.set()
    await settle(host)
    assert service.calls == [] and service.records == {}
    assert not receipt['scheduling']['desired']
    assert 'native_admission' not in receipt and 'started_at' not in receipt and 'completed_at' not in receipt


@pytest.mark.asyncio
@pytest.mark.parametrize('lost_response', [False, True])
async def test_admission_wins_release_preserving_pending_identity_and_idempotency(hosts, tmp_path, lost_response):
    host = hosts(reconnect_interval=0, reconnect_max_interval=0)
    service = OfflineDemandService(tmp_path / 'offline-ledger', after_send=True, lost_response=lost_response)
    ack = await declare(host, consumer(), 1)
    receipt = await submit(host, service, ack)
    await asyncio.wait_for(service.waiting.wait(), 30)
    key = receipt['native_request_key']
    exact_pending = service.ledger_path(key).read_bytes()
    exact_owner = {name: deepcopy(receipt[name]) for name in (
        'native_request_key', 'native_admission', 'blueprint_hash', 'model_hash', 'revision_binding', 'started_at')}
    await host.release_intent(**body(ack))
    assert not receipt['scheduling']['desired'] and service.cancelled == []
    assert service.ledger_path(key).read_bytes() == exact_pending
    assert {name: receipt[name] for name in exact_owner} == exact_owner
    assert 'completed_at' not in receipt and len(service.calls) == 1
    with pytest.raises(VisualizationIntentConflict):
        await submit(host, service, ack)
    service.after.set()
    await settle(host)
    assert receipt['status'] == 'completed' and len(service.calls) == 1
    assert receipt['native_admission'] == exact_owner['native_admission'] and service.cancelled == []
    assert json.loads(exact_pending)['idempotency_key'] == service.records[key]['idempotency_key']
    fresh = await declare(host, ack['consumer_id'], 2)
    assert await submit(host, service, fresh) is receipt
    assert len(service.calls) == 1


@pytest.mark.asyncio
async def test_withdrawn_candidate_never_borrows_or_cancels_draining_prior_owner(hosts, tmp_path):
    host = hosts()
    started, release = asyncio.Event(), asyncio.Event()
    prior_path = tmp_path / 'offline-prior-owner.json'
    prior = {'key': fingerprint({'offline': 'prior source'}),
             'admission': {'interaction_id': 'offline-prior-A', 'session_id': 'offline-A', 'cursor': 'console:10'},
             'idempotency_key': 'offline-A-original-key'}
    prior_path.write_text(json.dumps(prior, sort_keys=True))
    prior_bytes = prior_path.read_bytes()

    class DrainingService(OfflineDemandService):
        async def _produce(self, blueprint, model, context, on_admission, on_native_admission, feedback):
            started.set()
            try:
                await release.wait()
            except asyncio.CancelledError:
                self.cancelled.append('offline-prior-A')
                raise
            assert prior_path.read_bytes() == prior_bytes
            return await super()._produce(blueprint, model, context, on_admission, on_native_admission, feedback)

    service = DrainingService(tmp_path / 'offline-ledger')
    ack = await declare(host, consumer(), 1)
    candidate = await submit(host, service, ack, 2)
    await asyncio.wait_for(started.wait(), 30)
    await host.release_intent(**body(ack))
    assert service.cancelled == [] and prior_path.read_bytes() == prior_bytes
    assert candidate['native_request_key'] != prior['key'] and 'native_admission' not in candidate
    release.set()
    await settle(host)
    assert service.calls == [] and service.records == {}
    assert prior_path.read_bytes() == prior_bytes and service.cancelled == []
    assert 'native_admission' not in candidate and 'completed_at' not in candidate


@pytest.mark.asyncio
async def test_actual_visualizer_drains_saved_owner_after_release_without_any_new_transport_send(
        hosts, tmp_path, monkeypatch):
    from types import SimpleNamespace
    from launchkit.blueprint import starter_blueprint
    from launchkit.visualizer import VisualizerService

    async def offline_start():
        return None

    architect = SimpleNamespace(root=tmp_path / 'offline-architect', start=offline_start,
                                runtime=SimpleNamespace(rust_http_base_url='http://offline.invalid'))
    service = VisualizerService(architect, timeout=2, poll_interval=0)
    spec = starter_blueprint().model_dump(mode='json')
    model = {'nodes': [{'id': name, 'kind': 'agent'} for name in starter_blueprint().identities()],
             'edges': [], 'phases': []}
    source = service.request_binding(spec, model)
    key = source.pop('native_request_key')
    code = 'return {duration:1,render(frame){ /* inert saved owner */ }};'
    code_hash = hashlib.sha256(code.encode()).hexdigest()
    admission = {'interaction_id': 'offline-prior-A', 'session_id': 'offline-A', 'cursor': 'console:10'}
    prior = {**source, 'key': key, 'blueprint': spec, 'model': model,
             'owner_id': 'offline-previous-host', 'idempotency_key': 'offline-A-exact-idempotency',
             'admission': admission, 'examples_read': True,
             'publication': {'code': code, 'summary': 'Inert offline prior owner'},
             'publication_receipt': {**source, 'request_key': key, 'code_hash': code_hash,
                                     'authority': 'publish_visualization'}}
    service.root.mkdir(parents=True)
    pending_path = service.root / 'pending.json'
    pending_path.write_text(json.dumps(prior, sort_keys=True))
    exact_pending = pending_path.read_bytes()
    entered, release = asyncio.Event(), asyncio.Event()
    sends, cancelled = [], []

    async def offline_transport(client, method, url, **kwargs):
        if method == 'post':
            sends.append(deepcopy(kwargs['json']))
            raise AssertionError('A withdrawn candidate cannot send any new native request.')
        assert method == 'get' and url.endswith('/console/timeline')
        assert kwargs['params']['after'] == admission['cursor']
        entered.set()
        try:
            await release.wait()
        except asyncio.CancelledError:
            cancelled.append(admission['interaction_id'])
            raise
        return SimpleNamespace(json=lambda: {'frames': [{'kind': 'interaction_complete',
            'interaction_id': admission['interaction_id'], 'cursor': 'console:11'}]})

    monkeypatch.setattr('launchkit.architect._native_request', offline_transport)
    host = hosts()
    ack = await declare(host, consumer(), 1)
    desired = deepcopy(spec)
    desired['project']['name'] = 'A different queued proposal'
    candidate = await host.submit(service, desired, model, setup_id='setup-A', stream_id='overview',
                                  consumer_intent=body(ack))
    await asyncio.wait_for(entered.wait(), 30)
    await host.release_intent(**body(ack))
    assert pending_path.read_bytes() == exact_pending and not cancelled and not sends
    assert candidate['native_request_key'] != key and 'native_admission' not in candidate
    release.set()
    await settle(host)
    assert not sends and not cancelled and service.pending is None
    prior_result = json.loads((service.root / (key + '.json')).read_text())
    assert prior_result['native_admission'] == admission
    assert prior_result['native_run'] == {'status': 'completed', 'interaction_id': 'offline-prior-A',
                                        'terminal_cursor': 'console:11'}
    assert prior_result['code_hash'] == code_hash
    assert {name: prior_result[name] for name in source} == source
    assert not (service.root / (candidate['native_request_key'] + '.json')).exists()
    assert 'native_admission' not in candidate and 'completed_at' not in candidate


@pytest.mark.asyncio
async def test_managed_requeue_keeps_original_context_before_another_window_publishes(hosts, tmp_path):
    host, service = hosts(), OfflineDemandService(tmp_path / 'offline-ledger', after_send=True)
    first_intent = await declare(host, consumer(), 1)
    first = await submit(host, service, first_intent)
    await asyncio.wait_for(service.waiting.wait(), 30)
    waiting_intent = await declare(host, consumer(), 1)
    waiting = await submit(host, service, waiting_intent, 2)
    frozen = deepcopy(waiting['revision_context'])
    assert frozen['previous_program'] is None
    await host.release_intent(**body(waiting_intent))
    other_window = await declare(host, consumer(), 1)
    assert await submit(host, service, other_window, 2) is waiting
    service.after.set()
    await settle(host)
    assert first['status'] == waiting['status'] == 'completed'
    assert waiting['revision_context'] == frozen and service.calls[1]['context'] == frozen
    assert [call['blueprint']['version'] for call in service.calls] == [1, 2]


@pytest.mark.asyncio
async def test_restored_queued_receipt_and_tombstone_cannot_resurrect_after_lost_job(hosts, tmp_path):
    first, service = hosts(), OfflineDemandService(tmp_path / 'offline-ledger')
    ack = await declare(first, consumer(), 1)
    # Cancel before this newly created coroutine has ever run, simulating a
    # missing queued host owner without claiming a native cancellation.
    receipt = await submit(first, service, ack)
    task = first.tasks[receipt['id']]
    task.cancel()
    await asyncio.gather(task, return_exceptions=True)
    assert receipt['status'] == 'queued' and 'native_admission' not in receipt and not service.calls
    restored = hosts()
    await restored.release_intent(**body(ack))
    again = hosts()
    with pytest.raises(VisualizationIntentConflict):
        await submit(again, service, ack)
    assert not again.tasks and not service.calls
    new_ack = await declare(again, ack['consumer_id'], 2)
    wanted = await submit(again, service, new_ack)
    await settle(again)
    assert wanted['status'] == 'completed' and len(service.calls) == 1
    assert wanted['revision_context']['previous_program'] is None


@pytest.mark.asyncio
@pytest.mark.parametrize('native_state', ['running', 'observing', 'published'])
async def test_orphaned_native_receipt_never_uses_missing_job_as_permission_to_send(hosts, tmp_path, native_state):
    first = hosts(reconnect_interval=100)
    service = OfflineDemandService(tmp_path / 'offline-ledger', after_send=True,
                                   lost_response=native_state == 'observing',
                                   early_publication=native_state == 'published')
    ack = await declare(first, consumer(), 1)
    receipt = await submit(first, service, ack)
    await until(lambda: receipt['status'] == native_state and 'native_admission' in receipt)
    owned = {name: deepcopy(receipt[name]) for name in (
        'native_request_key', 'native_admission', 'revision_context', 'revision_binding', 'queued_at', 'started_at')}
    if native_state == 'published':
        owned.update(artifact=deepcopy(receipt['artifact']), published_at=receipt['published_at'])
    exact_pending = service.ledger_path(receipt['native_request_key']).read_bytes()
    restored = hosts()
    assert restored.get(receipt['id']) is not None and not restored.tasks
    await restored.release_intent(**body(ack))
    service.reattachable = False
    fresh = await declare(restored, ack['consumer_id'], 2)
    retained = await submit(restored, service, fresh)
    assert retained['id'] == receipt['id'] and retained['observation_status'] == 'unavailable'
    assert {name: retained[name] for name in owned} == owned
    assert 'completed_at' not in retained and retained['status'] == native_state
    assert len(service.calls) == 1 and not restored.tasks and service.cancelled == []
    assert service.ledger_path(receipt['native_request_key']).read_bytes() == exact_pending


@pytest.mark.asyncio
async def test_managed_correction_owns_its_parent_despite_other_window_and_replays_after_restart(hosts, tmp_path):
    host, service = hosts(), OfflineDemandService(tmp_path / 'offline-ledger')
    one = await declare(host, consumer(), 1)
    first = await submit(host, service, one)
    await settle(host)
    two = await declare(host, consumer(), 1)
    other = await submit(host, service, two, 2)
    await settle(host)
    correction_intent = await declare(host, one['consumer_id'], 2)
    with pytest.raises(VisualizationConflict):
        await submit(host, service, correction_intent, 2, feedback='Improve labels.',
                     expected_parent=parent_binding(other))
    with pytest.raises(VisualizationConflict):
        await submit(host, service, correction_intent, 99, feedback='Improve labels.',
                     expected_parent=parent_binding(first))
    correction = await submit(host, service, correction_intent, feedback='Improve labels.',
                              expected_parent=parent_binding(first))
    await settle(host)
    assert correction['status'] == 'completed'
    assert correction['revision_context']['previous_program']['operation_id'] == first['id']
    assert correction['revision_context']['previous_program']['code'] == first['artifact']['code']
    assert host._desired_head(('setup-A', 'overview')) is None
    restored = hosts()
    assert await declare(restored, one['consumer_id'], 2) == correction_intent
    replay = await submit(restored, service, correction_intent, feedback='Improve labels.',
                          expected_parent=parent_binding(first))
    assert replay == correction and len(service.calls) == 3
    newer = await declare(restored, one['consumer_id'], 3)
    with pytest.raises(VisualizationIntentConflict):
        await submit(restored, service, correction_intent, feedback='Improve labels.',
                     expected_parent=parent_binding(first))
    with pytest.raises(VisualizationConflict):
        await submit(restored, service, newer, feedback='A different correction.',
                     expected_parent=parent_binding(first))
    service.reference_hash = fingerprint({'reference': 'changed after reviewed publication'})
    with pytest.raises(VisualizationConflict):
        await submit(restored, service, newer, feedback='Improve labels.',
                     expected_parent=parent_binding(first))
    assert len(service.calls) == 3 and not restored.tasks


@pytest.mark.asyncio
async def test_correction_reservation_is_durable_once_per_parent_and_shared_exactly(hosts, tmp_path):
    host, service = hosts(), OfflineDemandService(tmp_path / 'offline-ledger')
    one, two = await declare(host, consumer(), 1), await declare(host, consumer(), 1)
    parent = await submit(host, service, one)
    assert await submit(host, service, two) is parent
    await settle(host)
    one_next, two_next = await declare(host, one['consumer_id'], 2), await declare(host, two['consumer_id'], 2)
    await host.generation_lock.acquire()
    correction = await submit(host, service, one_next, feedback='Improve labels.',
                              expected_parent=parent_binding(parent))
    restored = hosts()
    with pytest.raises(VisualizationConflict):
        await submit(restored, service, two_next, feedback='A different second correction.',
                     expected_parent=parent_binding(parent))
    shared = await submit(host, service, two_next, feedback='Improve labels.',
                          expected_parent=parent_binding(parent))
    assert shared is correction
    await host.release_intent(**body(one_next))
    assert shared['scheduling']['desired'] and shared['scheduling']['managed_consumers'] == 1
    host.generation_lock.release()
    await settle(host)
    assert correction['status'] == 'completed' and len(service.calls) == 2
    assert correction['correction_parent'] == parent_binding(parent)
    # Legacy callers still need their desired head; managed ownership is not a
    # permission to accept an arbitrary old publication through the old API.
    await host.submit(service, {'version': 3}, {}, setup_id='setup-A', stream_id='overview')
    await settle(host)
    with pytest.raises(VisualizationConflict):
        await host.submit(service, {'version': 1}, {}, setup_id='setup-A', stream_id='overview',
                          feedback='Improve labels.', expected_parent=parent_binding(parent))
    assert len(service.calls) == 3


@pytest.mark.asyncio
async def test_older_spent_correction_without_reservation_preserves_budget_and_exact_replay(hosts, tmp_path):
    host, service = hosts(), OfflineDemandService(tmp_path / 'offline-ledger')
    parent = await host.submit(service, {'version': 1}, {}, setup_id='setup-A', stream_id='overview')
    await settle(host)
    correction = await host.submit(service, {'version': 1}, {}, setup_id='setup-A', stream_id='overview',
                                   feedback='Improve labels.', expected_parent=parent_binding(parent))
    await settle(host)
    await host.submit(service, {'version': 2}, {}, setup_id='setup-A', stream_id='overview')
    await settle(host)
    owner = {name: deepcopy(correction[name]) for name in (
        'id', 'native_request_key', 'native_admission', 'revision_context', 'artifact', 'published_at', 'started_at')}
    exact_source = service.ledger_path(correction['native_request_key']).read_bytes()
    # Older releases retained the actual correction receipt but no separate
    # once-per-parent reservation. Keep every receipt and native byte intact.
    (tmp_path / 'operations' / ('correction-' + parent['id'] + '.json')).unlink()
    restored = hosts()
    viewer = await declare(restored, consumer(), 1)
    assert (await submit(restored, service, viewer))['id'] == parent['id']
    correction_intent = await declare(restored, viewer['consumer_id'], 2)
    with pytest.raises(VisualizationConflict):
        await submit(restored, service, correction_intent, feedback='A second automatic correction.',
                     expected_parent=parent_binding(parent))
    assert len(service.calls) == 3 and not restored.tasks
    replay = await submit(restored, service, correction_intent, feedback='Improve labels.',
                          expected_parent=parent_binding(parent))
    assert {name: replay[name] for name in owner} == owner
    assert service.ledger_path(correction['native_request_key']).read_bytes() == exact_source
    assert len(service.calls) == 3 and not restored.tasks


@pytest.mark.asyncio
async def test_corrupt_older_spent_correction_cannot_be_silently_skipped_into_a_fresh_budget(hosts, tmp_path):
    host, service = hosts(), OfflineDemandService(tmp_path / 'offline-ledger')
    parent = await host.submit(service, {'version': 1}, {}, setup_id='setup-A', stream_id='overview')
    await settle(host)
    correction = await host.submit(service, {'version': 1}, {}, setup_id='setup-A', stream_id='overview',
                                   feedback='Improve labels.', expected_parent=parent_binding(parent))
    await settle(host)
    directory = tmp_path / 'operations'
    (directory / ('correction-' + parent['id'] + '.json')).unlink()
    path = directory / (correction['id'] + '.json')
    saved = json.loads(path.read_text())
    saved['native_request_key'] = '0' * 64
    path.write_text(json.dumps(saved))
    restored = hosts()
    viewer = await declare(restored, consumer(), 1)
    assert (await submit(restored, service, viewer))['id'] == parent['id']
    next_intent = await declare(restored, viewer['consumer_id'], 2)
    with pytest.raises(VisualizationConflict):
        await submit(restored, service, next_intent, feedback='A second automatic correction.',
                     expected_parent=parent_binding(parent))
    assert len(service.calls) == 2 and not restored.tasks


@pytest.mark.asyncio
@pytest.mark.parametrize('restart', [False, True])
async def test_withdrawn_reserved_correction_exact_reattachment_resumes_its_original_queue(hosts, tmp_path, restart):
    first, service = hosts(), OfflineDemandService(tmp_path / 'offline-ledger')
    viewer = await declare(first, consumer(), 1)
    parent = await submit(first, service, viewer)
    await settle(first)
    correction_intent = await declare(first, viewer['consumer_id'], 2)
    await first.generation_lock.acquire()
    correction = await submit(first, service, correction_intent, feedback='Improve labels.',
                              expected_parent=parent_binding(parent))
    original = {name: deepcopy(correction[name]) for name in (
        'id', 'queued_at', 'native_request_key', 'request_fingerprint', 'revision_context',
        'revision_binding', 'correction_parent')}
    old_task = first.tasks[correction['id']]
    await asyncio.sleep(0)
    await first.release_intent(**body(correction_intent))
    await asyncio.gather(old_task, return_exceptions=True)
    assert len(service.calls) == 1 and 'native_admission' not in correction and correction['status'] == 'queued'
    owner = hosts() if restart else first
    fresh = await declare(owner, viewer['consumer_id'], 3)
    resumed = await submit(owner, service, fresh, feedback='Improve labels.',
                           expected_parent=parent_binding(parent))
    assert {name: resumed[name] for name in original} == original
    first.generation_lock.release()
    await settle(owner)
    assert resumed['status'] == 'completed' and len(service.calls) == 2
    assert await submit(owner, service, fresh, feedback='Improve labels.',
                        expected_parent=parent_binding(parent)) is resumed
    assert len(service.calls) == 2


@pytest.mark.asyncio
async def test_expired_shared_lease_retires_only_its_interest_and_cannot_renew_tombstone(hosts, tmp_path):
    clock, service = LeaseClock(), OfflineDemandService(tmp_path / 'offline-ledger')
    host = hosts(consumer_clock=clock)
    await host.generation_lock.acquire()
    one, two = await declare(host, consumer(), 1), await declare(host, consumer(), 1)
    shared = await submit(host, service, one)
    assert await submit(host, service, two) is shared
    clock.advance(60)
    renewed = await host.renew_intent(**body(two))
    assert renewed['intent_token'] == two['intent_token'] and renewed['expires_at'] != two['expires_at']
    clock.advance(31)
    await host.expire_intents()
    assert shared['scheduling']['desired'] and shared['scheduling']['managed_consumers'] == 1
    with pytest.raises(VisualizationIntentConflict):
        await host.renew_intent(**body(one))
    with pytest.raises(VisualizationIntentConflict):
        await submit(host, service, one)
    host.generation_lock.release()
    await settle(host)
    assert shared['status'] == 'completed' and len(service.calls) == 1
    restored = hosts(consumer_clock=clock)
    with pytest.raises(VisualizationIntentConflict):
        await declare(restored, one['consumer_id'], 1)
    assert await declare(restored, two['consumer_id'], 1) == renewed


@pytest.mark.asyncio
@pytest.mark.parametrize('admitted', [False, True])
async def test_lease_expiry_withdraws_unadmitted_desire_but_preserves_admitted_custody(hosts, tmp_path, admitted):
    clock = LeaseClock()
    host = hosts(consumer_clock=clock)
    service = OfflineDemandService(tmp_path / 'offline-ledger', before_admission=not admitted,
                                   after_send=admitted)
    ack = await declare(host, consumer(), 1)
    receipt = await submit(host, service, ack)
    await asyncio.wait_for((service.waiting if admitted else service.entered).wait(), 30)
    owned = deepcopy(receipt.get('native_admission'))
    clock.advance(91)
    await host.expire_intents()
    assert not receipt['scheduling']['desired']
    with pytest.raises(VisualizationIntentConflict):
        await host.renew_intent(**body(ack))
    service.before.set()
    service.after.set()
    await settle(host)
    assert service.cancelled == []
    if admitted:
        assert receipt['status'] == 'completed' and receipt['native_admission'] == owned
        assert len(service.calls) == 1
    else:
        assert not service.calls and 'native_admission' not in receipt and 'completed_at' not in receipt


@pytest.mark.asyncio
@pytest.mark.parametrize('mutation', ['extra_ack_fields', 'foreign_consumer', 'wrong_token', 'changed_scope'])
async def test_body_intent_is_exact_and_cannot_impersonate_an_independent_document(hosts, tmp_path, mutation):
    host, service = hosts(), OfflineDemandService(tmp_path / 'offline-ledger')
    ack = await declare(host, consumer(), 1)
    independent = await declare(host, consumer(), 1)
    bad = body(ack)
    if mutation == 'extra_ack_fields':
        bad = deepcopy(ack)
    elif mutation == 'foreign_consumer':
        bad['consumer_id'] = independent['consumer_id']
    elif mutation == 'wrong_token':
        bad['intent_token'] = independent['intent_token']
    else:
        bad['stream_id'] = 'flow:survey'
    with pytest.raises(VisualizationIntentConflict):
        await host.submit(service, {'version': 1}, {}, setup_id='setup-A', stream_id='overview',
                          consumer_intent=bad)
    assert not host.tasks and not host.receipts and not service.calls
    assert ack['consumer_id'] != independent['consumer_id'] and ack['intent_token'] != independent['intent_token']
