"""Offline viewer ownership and durable queue recovery, with no native runtime."""
import asyncio
from copy import deepcopy
import json
import uuid

import pytest
import pytest_asyncio

from launchkit.projects import fingerprint
from launchkit.visualization_consumers import INTENT_FIELDS, MAX_INTENT_REVISION, checked_intent
from launchkit.visualization_operations import VisualizationOperations, VisualizationIntentConflict, VisualizationConflict
from test_visualization_operations import ContinuousService, finish_operations


def body(ack):
    return {name: ack[name] for name in INTENT_FIELDS}


@pytest_asyncio.fixture
async def coordinators(tmp_path):
    values = []
    def create(**kwargs):
        value = VisualizationOperations(root=kwargs.pop('root', tmp_path), **kwargs)
        values.append(value)
        return value
    yield create
    for value in values:
        await value.shutdown()


@pytest.mark.asyncio
@pytest.mark.parametrize('revision', [True, False, 0, -1, 1.0, '1', MAX_INTENT_REVISION + 1])
async def test_revision_is_a_strict_positive_js_safe_integer(coordinators, revision):
    operations = coordinators()
    with pytest.raises(VisualizationIntentConflict):
        await operations.declare_intent(str(uuid.uuid4()), revision, 'setup', 'overview')
    assert not operations.consumers.records and not operations.tasks


@pytest.mark.asyncio
@pytest.mark.parametrize('consumer', ['not-a-uuid', str(uuid.uuid4()).upper(), '00000000000000000000000000000000', None, 1])
async def test_consumer_uuid_is_canonical_and_lowercase(coordinators, consumer):
    operations = coordinators()
    with pytest.raises(VisualizationIntentConflict):
        await operations.declare_intent(consumer, 1, 'setup', 'overview')


@pytest.mark.asyncio
async def test_declaration_retry_renewal_and_body_bindings_are_exact(coordinators):
    now = [1000.0]
    operations = coordinators(consumer_clock=lambda: now[0])
    consumer = str(uuid.uuid4())
    first = await operations.declare_intent(consumer, 1, 'setup', 'overview')
    now[0] += 20
    assert await operations.declare_intent(consumer, 1, 'setup', 'overview') == first
    renewed = await operations.renew_intent(**body(first))
    assert renewed['intent_token'] == first['intent_token']
    assert renewed['expires_at'] != first['expires_at'] and renewed['lease_seconds'] == 90
    assert checked_intent(body(renewed)) == body(first)
    for changed in [{**body(first), 'intent_token': str(uuid.uuid4())},
                    {**body(first), 'stream_id': 'other'},
                    {**body(first), 'extra': True}, {name: value for name, value in body(first).items() if name != 'setup_id'}]:
        with pytest.raises(VisualizationIntentConflict):
            await operations.submit(ContinuousService(), {}, {}, setup_id='setup', stream_id='overview', consumer_intent=changed)
    assert not operations.tasks and not operations.receipts


@pytest.mark.asyncio
async def test_release_before_declare_is_durable_and_old_release_cannot_replace_newer_intent(coordinators, tmp_path):
    consumer = str(uuid.uuid4())
    operations = coordinators()
    tombstone = await operations.release_intent(consumer, 2, 'setup', 'flow')
    assert tombstone['status'] == 'released' and tombstone['intent_token'] is None
    assert (await operations.release_intent(consumer, 2, 'setup', 'flow'))['status'] == 'released'
    restored = coordinators()
    for revision in [1, 2]:
        with pytest.raises(VisualizationIntentConflict):
            await restored.declare_intent(consumer, revision, 'setup', 'flow')
    current = await restored.declare_intent(consumer, 3, 'setup', 'overview')
    before = (tmp_path/'consumer-intents'/f'{consumer}.json').read_bytes()
    await restored.release_intent(consumer, 1, 'setup', 'flow')
    assert (tmp_path/'consumer-intents'/f'{consumer}.json').read_bytes() == before
    assert await restored.declare_intent(consumer, 3, 'setup', 'overview') == current
    assert not restored.tasks


@pytest.mark.asyncio
async def test_nullable_preview_scope_and_exact_same_intent_body_recovery(coordinators):
    operations, service = coordinators(), ContinuousService()
    consumer = str(uuid.uuid4())
    await operations.generation_lock.acquire()
    intent = body(await operations.declare_intent(consumer, 1, None, 'preview:overview'))
    receipt = await operations.submit(service, {'version': 1}, {}, setup_id=None, stream_id='preview:overview', consumer_intent=intent)
    assert receipt['setup_id'] is None and receipt['scheduling']['desired']
    assert await operations.submit(service, {'version': 1}, {}, setup_id=None, stream_id='preview:overview', consumer_intent=intent) is receipt
    for options in [{'version': 2}, {'version': 1, 'changed': True}]:
        with pytest.raises(VisualizationIntentConflict):
            await operations.submit(service, options, {}, setup_id=None, stream_id='preview:overview', consumer_intent=intent)
    operations.generation_lock.release()
    await finish_operations(operations)
    assert len(service.calls) == 1 and receipt['status'] == 'completed'
    frozen = deepcopy({name: receipt[name] for name in ('native_request_key', 'request_fingerprint', 'revision_context', 'native_admission', 'queued_at', 'started_at', 'published_at', 'completed_at')})
    again = body(await operations.declare_intent(consumer, 2, None, 'preview:overview'))
    assert await operations.submit(service, {'version': 1}, {}, setup_id=None, stream_id='preview:overview', consumer_intent=again) is receipt
    assert {name: receipt[name] for name in frozen} == frozen and len(service.calls) == 1
    assert receipt['request_fingerprint'] == fingerprint({'blueprint': {'version': 1}, 'model': {}})


@pytest.mark.asyncio
async def test_released_saved_receipt_projects_current_demand_even_when_not_loaded(coordinators, tmp_path):
    operations, service = coordinators(), ContinuousService()
    consumer = str(uuid.uuid4())
    intent = body(await operations.declare_intent(consumer, 1, 'setup', 'overview'))
    receipt = await operations.submit(service, {'version': 1}, {}, setup_id='setup', stream_id='overview', consumer_intent=intent)
    await finish_operations(operations)
    frozen = deepcopy({key: value for key, value in receipt.items() if key != 'scheduling'})
    await operations.shutdown()
    restored = coordinators()
    assert not restored.receipts
    await restored.release_intent(**intent)
    assert not restored.receipts, 'Release must not require loading every historical native receipt'
    read = restored.get(receipt['id'])
    assert read['scheduling'] == {'desired': False, 'managed_consumers': 0, 'legacy_interest': False, 'status': 'withdrawn'}
    assert {key: value for key, value in read.items() if key != 'scheduling'} == frozen
    assert len(service.calls) == 1


@pytest.mark.asyncio
async def test_shutdown_preserves_known_unadmitted_queue_for_exact_restart(coordinators):
    operations, service = coordinators(), ContinuousService()
    consumer = str(uuid.uuid4())
    await operations.generation_lock.acquire()
    intent = body(await operations.declare_intent(consumer, 1, 'setup', 'overview'))
    receipt = await operations.submit(service, {'version': 1}, {}, setup_id='setup', stream_id='overview', consumer_intent=intent)
    await asyncio.sleep(0)
    await operations.shutdown()
    assert receipt['status'] == 'queued' and 'started_at' not in receipt and not service.calls
    original = deepcopy(receipt)
    operations.generation_lock.release()
    restored = coordinators()
    resumed = await restored.submit(service, {'version': 1}, {}, setup_id='setup', stream_id='overview', consumer_intent=intent)
    assert resumed['id'] == original['id'] and resumed['queued_at'] == original['queued_at']
    assert resumed['revision_context'] == original['revision_context']
    await finish_operations(restored)
    assert resumed['status'] == 'completed' and len(service.calls) == 1


@pytest.mark.asyncio
async def test_cold_shared_legacy_head_does_not_recurse_or_lose_its_interest(coordinators):
    operations, service = coordinators(), ContinuousService()
    receipt = await operations.submit(service, {'version': 1}, {}, setup_id='setup', stream_id='overview')
    await finish_operations(operations)
    intent = body(await operations.declare_intent(str(uuid.uuid4()), 1, 'setup', 'overview'))
    assert await operations.submit(service, {'version': 1}, {}, setup_id='setup', stream_id='overview', consumer_intent=intent) is receipt
    await operations.shutdown()
    restored = coordinators()
    assert not restored.streams and not restored.receipts
    loaded = restored.get(receipt['id'])
    assert loaded['scheduling']['managed_consumers'] == 1 and loaded['scheduling']['legacy_interest']
    await restored.release_intent(**intent)
    assert loaded['scheduling'] == {'desired': True, 'managed_consumers': 0, 'legacy_interest': True, 'status': 'wanted'}
    assert loaded['status'] == 'completed' and not restored.tasks and len(service.calls) == 1


@pytest.mark.asyncio
async def test_canceled_unadmitted_coroutine_cannot_interrupt_its_immediate_reattachment(coordinators):
    operations, service = coordinators(), ContinuousService()
    await operations.generation_lock.acquire()
    consumer = str(uuid.uuid4())
    first = body(await operations.declare_intent(consumer, 1, 'setup', 'overview'))
    receipt = await operations.submit(service, {'version': 1}, {}, setup_id='setup', stream_id='overview', consumer_intent=first)
    old_task = operations.tasks[receipt['id']]
    await asyncio.sleep(0)
    await operations.release_intent(**first)
    second = body(await operations.declare_intent(consumer, 2, 'setup', 'overview'))
    same = await operations.submit(service, {'version': 1}, {}, setup_id='setup', stream_id='overview', consumer_intent=second)
    assert same is receipt and operations.tasks[receipt['id']] is not old_task
    await asyncio.gather(old_task, return_exceptions=True)
    assert receipt['status'] == 'queued' and receipt['scheduling']['desired'] and not service.calls
    operations.generation_lock.release()
    await finish_operations(operations)
    assert receipt['status'] == 'completed' and len(service.calls) == 1


@pytest.mark.asyncio
@pytest.mark.parametrize('limit', ['MAX_CORRECTION_MIGRATION_RECEIPTS', 'MAX_CORRECTION_MIGRATION_BYTES'])
async def test_legacy_correction_migration_has_a_fail_closed_inventory_budget(coordinators, monkeypatch, limit):
    operations, service = coordinators(), ContinuousService()
    parent = await operations.submit(service, {'version': 1}, {}, setup_id='setup', stream_id='overview')
    await finish_operations(operations)
    expected = {name: parent[name] for name in ('blueprint_hash', 'model_hash', 'setup_id', 'stream_id')}
    expected.update(operation_id=parent['id'], code_hash=parent['artifact']['code_hash'])
    monkeypatch.setattr(f'launchkit.visualization_operations.{limit}', 0)
    with pytest.raises(VisualizationConflict, match='bounded migration budget'):
        await operations.submit(service, {'version': 1}, {}, feedback='Checked visual defect.', setup_id='setup', stream_id='overview', expected_parent=expected)
    assert len(service.calls) == 1 and not operations.tasks


def correction_parent(receipt):
    return {'operation_id': receipt['id'], 'code_hash': receipt['artifact']['code_hash'],
            **{name: receipt[name] for name in ('blueprint_hash', 'model_hash', 'setup_id', 'stream_id')}}


@pytest.mark.asyncio
@pytest.mark.parametrize('reconstruct', [False, True])
async def test_withdrawn_unadmitted_correction_exact_replay_recovers_the_reserved_task(coordinators, reconstruct):
    operations, service = coordinators(), ContinuousService()
    consumer = str(uuid.uuid4())
    initial = body(await operations.declare_intent(consumer, 1, 'setup', 'overview'))
    parent = await operations.submit(service, {'version': 1}, {}, setup_id='setup', stream_id='overview', consumer_intent=initial)
    await finish_operations(operations)
    expected = correction_parent(parent)
    await operations.generation_lock.acquire()
    second = body(await operations.declare_intent(consumer, 2, 'setup', 'overview'))
    correction = await operations.submit(service, {'version': 1}, {}, feedback='Checked visual defect.',
        setup_id='setup', stream_id='overview', expected_parent=expected, consumer_intent=second)
    abandoned = operations.tasks[correction['id']]
    await asyncio.sleep(0)
    await operations.release_intent(**second)
    await asyncio.gather(abandoned, return_exceptions=True)
    assert correction['status'] == 'queued' and not operations.tasks and len(service.calls) == 1
    frozen = deepcopy({name: correction[name] for name in (
        'id', 'queued_at', 'native_request_key', 'revision_context', 'correction_parent')})
    if reconstruct:
        await operations.shutdown()
        operations.generation_lock.release()
        operations = coordinators()
        await operations.generation_lock.acquire()
    third = body(await operations.declare_intent(consumer, 3, 'setup', 'overview'))
    replay = await operations.submit(service, {'version': 1}, {}, feedback='Checked visual defect.',
        setup_id='setup', stream_id='overview', expected_parent=expected, consumer_intent=third)
    assert replay['id'] == correction['id'] and replay['id'] in operations.tasks
    assert {name: replay[name] for name in frozen} == frozen
    assert 'native_admission' not in replay and 'started_at' not in replay
    operations.generation_lock.release()
    await finish_operations(operations)
    assert replay['status'] == 'completed' and len(service.calls) == 2
    assert {name: replay[name] for name in frozen} == frozen
    assert await operations.submit(service, {'version': 1}, {}, feedback='Checked visual defect.',
        setup_id='setup', stream_id='overview', expected_parent=expected, consumer_intent=third) is replay
    assert len(service.calls) == 2


@pytest.mark.asyncio
@pytest.mark.parametrize('native_status', ['queued', 'running'])
async def test_reserved_correction_with_retained_native_custody_never_uses_replay_to_send_again(coordinators, native_status):
    operations, service = coordinators(), ContinuousService()
    consumer = str(uuid.uuid4())
    initial = body(await operations.declare_intent(consumer, 1, 'setup', 'overview'))
    parent = await operations.submit(service, {'version': 1}, {}, setup_id='setup', stream_id='overview', consumer_intent=initial)
    await finish_operations(operations)
    await operations.generation_lock.acquire()
    second = body(await operations.declare_intent(consumer, 2, 'setup', 'overview'))
    correction = await operations.submit(service, {'version': 1}, {}, feedback='Checked visual defect.',
        setup_id='setup', stream_id='overview', expected_parent=correction_parent(parent), consumer_intent=second)
    abandoned = operations.tasks[correction['id']]
    await asyncio.sleep(0)
    await operations.release_intent(**second)
    await asyncio.gather(abandoned, return_exceptions=True)
    # A retained offline receipt is custody evidence, not an executable owner.
    correction.update(status=native_status, started_at='2026-10-03T00:00:00+00:00',
        native_admission={'interaction_id': 'offline-correction', 'session_id': 'offline-session', 'cursor': 'console:10'})
    operations._persist(correction)
    frozen = deepcopy({name: correction[name] for name in (
        'id', 'queued_at', 'started_at', 'native_request_key', 'native_admission', 'revision_context', 'correction_parent')})
    await operations.shutdown()
    operations.generation_lock.release()
    restored = coordinators()
    third = body(await restored.declare_intent(consumer, 3, 'setup', 'overview'))
    replay = await restored.submit(service, {'version': 1}, {}, feedback='Checked visual defect.',
        setup_id='setup', stream_id='overview', expected_parent=correction_parent(parent), consumer_intent=third)
    assert replay['observation_status'] == 'unavailable' and not restored.tasks
    assert {name: replay[name] for name in frozen} == frozen
    assert len(service.calls) == 1


@pytest.mark.asyncio
async def test_automatic_lease_expiry_retires_waiting_work_without_native_terminality(coordinators):
    # The lease clock stands still until the scene is submitted, so a busy host
    # cannot let the lease lapse first. The host's own lease task then expires
    # the intent once the clock passes its lease.
    now = [0.0]
    operations = coordinators(consumer_lease_seconds=.025, consumer_clock=lambda: now[0])
    service = ContinuousService()
    await operations.generation_lock.acquire()
    intent = body(await operations.declare_intent(str(uuid.uuid4()), 1, 'setup', 'overview'))
    receipt = await operations.submit(service, {'version': 1}, {}, setup_id='setup', stream_id='overview', consumer_intent=intent)
    now[0] = 1.0
    async def expired():
        while operations.consumers.records[intent['consumer_id']]['status'] != 'expired':
            await asyncio.sleep(.005)
    await asyncio.wait_for(expired(), 5)
    assert receipt['status'] == 'queued' and receipt['scheduling']['status'] == 'withdrawn'
    assert not operations.tasks and not operations.jobs and 'started_at' not in receipt
    operations.generation_lock.release()
    await asyncio.sleep(0)
    assert not service.calls
    for call in [operations.declare_intent(intent['consumer_id'], 1, 'setup', 'overview'),
                 operations.renew_intent(**intent), operations.submit(service, {'version': 1}, {}, setup_id='setup', stream_id='overview', consumer_intent=intent)]:
        with pytest.raises(VisualizationIntentConflict):
            await call


@pytest.mark.asyncio
async def test_declaring_new_view_preserves_last_accepted_receipt_per_scope(coordinators):
    operations, service = coordinators(), ContinuousService()
    consumer = str(uuid.uuid4())
    first = body(await operations.declare_intent(consumer, 1, 'setup', 'overview'))
    receipt = await operations.submit(service, {'version': 1}, {}, setup_id='setup', stream_id='overview', consumer_intent=first)
    await finish_operations(operations)
    other = body(await operations.declare_intent(consumer, 2, 'setup', 'flow'))
    await operations.submit(service, {'version': 2}, {}, setup_id='setup', stream_id='flow', consumer_intent=other)
    await finish_operations(operations)
    await operations.release_intent(**other)
    await operations.shutdown()
    restored = coordinators()
    await restored.declare_intent(consumer, 3, 'setup', 'overview')
    assert restored.consumers.records[consumer]['previous_receipt_id'] == receipt['id']
    assert not restored.tasks


@pytest.mark.asyncio
@pytest.mark.parametrize('mutate', [lambda value: {**value, 'intent_revision': True},
                                   lambda value: {**value, 'request_fingerprint': 'not-a-hash'},
                                   lambda value: {**value, 'history': []},
                                   lambda value: {**value, 'status': 'live', 'intent_token': None}])
async def test_malformed_durable_consumer_record_cannot_enable_native_admission(coordinators, tmp_path, mutate):
    operations = coordinators()
    consumer = str(uuid.uuid4())
    ack = await operations.declare_intent(consumer, 1, 'setup', 'overview')
    await operations.shutdown()
    path = tmp_path/'consumer-intents'/f'{consumer}.json'
    path.write_text(json.dumps(mutate(json.loads(path.read_text()))))
    restored, service = coordinators(), ContinuousService()
    with pytest.raises(VisualizationIntentConflict):
        await restored.submit(service, {}, {}, setup_id='setup', stream_id='overview', consumer_intent=body(ack))
    assert not service.calls and not restored.tasks
