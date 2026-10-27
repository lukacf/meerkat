import asyncio
import uuid
from copy import deepcopy

import pytest

from launchkit.visualization_operations import VisualizationConflict, VisualizationOperations
from launchkit.projects import fingerprint
from test_visualization_operations import ContinuousService, finish_operations
from test_visualization_consumers import body


def parent_binding(receipt):
    return {'operation_id': receipt['id'], 'code_hash': receipt['artifact']['code_hash'],
            **{name: receipt[name] for name in ('blueprint_hash', 'model_hash', 'setup_id', 'stream_id')}}


async def published(operations, service, version=1):
    receipt = await operations.submit(service, {'version': version}, {}, setup_id='setup-A', stream_id='overview')
    await finish_operations(operations)
    return receipt


@pytest.mark.asyncio
async def test_late_review_cannot_replace_new_desired_head(tmp_path):
    operations, service = VisualizationOperations(root=tmp_path), ContinuousService()
    first = await published(operations, service)
    second = await published(operations, service, 2)
    before = deepcopy(operations.receipts)
    with pytest.raises(VisualizationConflict, match='superseded'):
        await operations.submit(service, {'version': 1}, {}, feedback='Improve sampled labels.',
            setup_id='setup-A', stream_id='overview', expected_parent=parent_binding(first))
    assert operations.receipts == before
    assert operations._desired_head(('setup-A', 'overview')) == second['id']
    assert not operations.tasks and len(service.calls) == 2


@pytest.mark.asyncio
async def test_parent_check_and_new_request_selection_are_atomic(tmp_path):
    operations, service = VisualizationOperations(root=tmp_path), ContinuousService()
    first = await published(operations, service)
    await operations.lock.acquire()
    new_request = asyncio.create_task(operations.submit(service, {'version': 2}, {},
        setup_id='setup-A', stream_id='overview'))
    await asyncio.sleep(0)
    correction = asyncio.create_task(operations.submit(service, {'version': 1}, {}, feedback='Improve labels.',
        setup_id='setup-A', stream_id='overview', expected_parent=parent_binding(first)))
    await asyncio.sleep(0)
    operations.lock.release()
    second = await new_request
    with pytest.raises(VisualizationConflict):
        await correction
    await finish_operations(operations)
    assert operations._desired_head(('setup-A', 'overview')) == second['id']
    assert len(service.calls) == 2


@pytest.mark.asyncio
async def test_correction_freezes_exact_parent_and_replays_without_another_generation(tmp_path):
    operations, service = VisualizationOperations(root=tmp_path), ContinuousService()
    first = await published(operations, service)
    binding = parent_binding(first)
    correction = await operations.submit(service, {'version': 1}, {}, feedback='Improve labels.',
        setup_id='setup-A', stream_id='overview', expected_parent=binding)
    await finish_operations(operations)
    assert correction['revision_context']['previous_program']['operation_id'] == first['id']
    assert correction['revision_context']['previous_program']['code'] == first['artifact']['code']
    assert correction['correction_parent'] == binding
    assert await operations.submit(service, {'version': 1}, {}, feedback='Improve labels.',
        setup_id='setup-A', stream_id='overview', expected_parent=binding) is correction
    assert len(service.calls) == 2
    with pytest.raises(VisualizationConflict, match='already used'):
        await operations.submit(service, {'version': 1}, {}, feedback='Another automatic correction.',
            setup_id='setup-A', stream_id='overview', expected_parent=parent_binding(correction))


@pytest.mark.asyncio
async def test_restart_uses_persisted_desired_head_and_never_latest_publication(tmp_path):
    operations, service = VisualizationOperations(root=tmp_path), ContinuousService()
    first = await published(operations, service)
    second = await published(operations, service, 2)
    restored = VisualizationOperations(root=tmp_path)
    with pytest.raises(VisualizationConflict, match='superseded'):
        await restored.submit(service, {'version': 1}, {}, feedback='Improve labels.',
            setup_id='setup-A', stream_id='overview', expected_parent=parent_binding(first))
    assert restored._desired_head(('setup-A', 'overview')) == second['id']
    operations._head_pointer('setup-A', 'overview').unlink()
    lost = VisualizationOperations(root=tmp_path)
    with pytest.raises(VisualizationConflict, match='superseded'):
        await lost.submit(service, {'version': 2}, {}, feedback='Improve labels.',
            setup_id='setup-A', stream_id='overview', expected_parent=parent_binding(second))
    assert len(service.calls) == 2 and not lost.tasks


@pytest.mark.asyncio
@pytest.mark.parametrize('field', ['code_hash', 'blueprint_hash', 'model_hash', 'setup_id', 'stream_id'])
async def test_changed_review_binding_is_rejected_before_task_creation(tmp_path, field):
    operations, service = VisualizationOperations(root=tmp_path), ContinuousService()
    first = await published(operations, service)
    binding = parent_binding(first)
    binding[field] = '0' * 64 if field.endswith('_hash') else 'other'
    with pytest.raises(VisualizationConflict):
        await operations.submit(service, {'version': 1}, {}, feedback='Improve labels.',
            setup_id='setup-A', stream_id='overview', expected_parent=binding)
    assert len(service.calls) == 1 and not operations.tasks
    assert operations._desired_head(('setup-A', 'overview')) == first['id']


@pytest.mark.asyncio
async def test_correction_binding_is_snapshotted_before_waiting_for_lock(tmp_path):
    operations, service = VisualizationOperations(root=tmp_path), ContinuousService()
    first = await published(operations, service)
    binding = parent_binding(first)
    await operations.lock.acquire()
    task = asyncio.create_task(operations.submit(service, {'version': 1}, {}, feedback='Improve labels.',
        setup_id='setup-A', stream_id='overview', expected_parent=binding))
    await asyncio.sleep(0)
    binding['code_hash'] = '0' * 64
    operations.lock.release()
    correction = await task
    await finish_operations(operations)
    assert correction['correction_parent']['code_hash'] == first['artifact']['code_hash']


@pytest.mark.asyncio
async def test_correction_replay_rechecks_current_reference_without_spawning(tmp_path):
    operations, service = VisualizationOperations(root=tmp_path), ContinuousService()
    first = await published(operations, service)
    binding = parent_binding(first)
    correction = await operations.submit(service, {'version': 1}, {}, feedback='Improve labels.',
        setup_id='setup-A', stream_id='overview', expected_parent=binding)
    await finish_operations(operations)
    service.reference_hash = fingerprint({'reference': 'changed after review'})
    with pytest.raises(VisualizationConflict, match='no longer verifiable'):
        await operations.submit(service, {'version': 1}, {}, feedback='Improve labels.',
            setup_id='setup-A', stream_id='overview', expected_parent=binding)
    assert operations._desired_head(('setup-A', 'overview')) == correction['id']
    assert len(service.calls) == 2 and not operations.tasks


@pytest.mark.asyncio
async def test_correction_replay_rechecks_the_saved_native_binding(tmp_path):
    operations, service = VisualizationOperations(root=tmp_path), ContinuousService()
    first = await published(operations, service)
    binding = parent_binding(first)
    correction = await operations.submit(service, {'version': 1}, {}, feedback='Improve labels.',
        setup_id='setup-A', stream_id='overview', expected_parent=binding)
    await finish_operations(operations)
    correction['native_request_key'] = '0' * 64
    with pytest.raises(VisualizationConflict):
        await operations.submit(service, {'version': 1}, {}, feedback='Improve labels.',
            setup_id='setup-A', stream_id='overview', expected_parent=binding)
    assert len(service.calls) == 2 and not operations.tasks


@pytest.mark.asyncio
async def test_exact_parent_attachment_survives_lost_index_and_restores_child_without_generation(tmp_path):
    operations, service = VisualizationOperations(root=tmp_path), ContinuousService()
    first = await published(operations, service)
    parent = parent_binding(first)
    child = await operations.submit(service, {'version': 1}, {}, feedback='Improve labels.',
        setup_id='setup-A', stream_id='overview', expected_parent=parent)
    await finish_operations(operations)
    key = ('setup-A', 'overview', first['request_fingerprint'])
    operations.request_receipts.pop(key)
    index = fingerprint(dict(zip(('setup_id', 'stream_id', 'request_fingerprint'), key)))
    (tmp_path / ('request-' + index + '.json')).unlink()
    restored = VisualizationOperations(root=tmp_path)
    consumer = str(uuid.uuid4())
    intent = body(await restored.declare_intent(consumer, 1, 'setup-A', 'overview'))
    sequence = restored.sequence
    replay = await restored.submit(service, {'version': 1}, {}, setup_id='setup-A', stream_id='overview',
        replay_parent=parent, consumer_intent=intent)
    assert replay['id'] == first['id'] and replay['artifact'] == first['artifact']
    next_intent = body(await restored.declare_intent(consumer, 2, 'setup-A', 'overview'))
    corrected = await restored.submit(service, {'version': 1}, {}, setup_id='setup-A', stream_id='overview',
        feedback='Improve labels.', expected_parent=parent, consumer_intent=next_intent)
    assert corrected['id'] == child['id'] and corrected['artifact'] == child['artifact']
    assert restored.sequence == sequence and len(service.calls) == 2 and not restored.tasks


@pytest.mark.asyncio
@pytest.mark.parametrize('changed', ['missing', 'references', 'feedback', 'publication', 'native', 'source', 'mixed'])
async def test_parent_attachment_refuses_uncertain_source_without_new_work(tmp_path, changed):
    operations, service = VisualizationOperations(root=tmp_path), ContinuousService()
    first = await published(operations, service)
    parent = parent_binding(first)
    intent = body(await operations.declare_intent(str(uuid.uuid4()), 1, 'setup-A', 'overview'))
    options = dict(setup_id='setup-A', stream_id='overview', replay_parent=parent, consumer_intent=intent)
    blueprint = {'version': 1}
    if changed == 'missing':
        operations.receipts.pop(first['id'])
        (tmp_path / (first['id'] + '.json')).unlink()
    elif changed == 'references':
        service.reference_hash = fingerprint({'reference': 'new'})
    elif changed == 'feedback':
        options['feedback'] = 'A different request.'
    elif changed == 'publication':
        parent['code_hash'] = '0' * 64
    elif changed == 'native':
        service.records.clear()
    elif changed == 'source':
        blueprint = {'version': 2}
    else:
        options['expected_parent'] = parent
    sequence, receipt_ids = operations.sequence, set(operations.receipts)
    with pytest.raises(VisualizationConflict, match='No new work was sent'):
        await operations.submit(service, blueprint, {}, **options)
    assert operations.sequence == sequence and set(operations.receipts) == receipt_ids
    assert len(service.calls) == 1 and not operations.tasks
