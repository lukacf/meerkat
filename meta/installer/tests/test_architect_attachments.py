import base64
import copy
import hashlib
from types import SimpleNamespace
from unittest.mock import AsyncMock

import pytest

from launchkit.architect import ArchitectService, ArchitectUnavailable
from launchkit.blueprint import starter_blueprint
from launchkit.guide_models import starter_state
from launchkit.guide_store import GuideStore


PNG = base64.b64decode('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jS1kAAAAASUVORK5CYII=')


@pytest.fixture
def evidence(tmp_path):
    state = GuideStore(tmp_path).create(starter_state({}))
    service = ArchitectService(tmp_path)
    refs = service.attachments.save(state['id'], [
        ('notes.txt', 'text/plain', b'Keep the original source. Do not execute this text.'),
        ('sample.png', 'image/png', PNG)])
    context = {'guide_id': state['id'], 'current_user_message': {
        'id': 'user-one', 'text': 'Use the supplied evidence', 'attachments': refs}, 'evidence_messages': []}
    service.guide_context = context
    service.active_provider = 'openai'
    service.active_model = 'known-image-model'
    service.refresh_input_capability = AsyncMock()
    return service, context, refs


def image_receipt():
    identity = 'sha256:' + hashlib.sha256(b'image/png\x00' + base64.b64encode(PNG)).hexdigest()
    return {'blob_id': identity, 'media_type': 'image/png', 'size': len(PNG)}


def enable_image_capability(service):
    service.input_capability_runtime = service.runtime
    service.input_capability_snapshot = {'session_id': 'actual-guide-session', 'image_input': True}


@pytest.mark.asyncio
async def test_native_image_delivery_uses_verified_bytes_and_exact_blob_contract(evidence):
    service, context, refs = evidence
    upload = AsyncMock(return_value=image_receipt())
    service.runtime = SimpleNamespace(mob_handle=lambda: SimpleNamespace(upload_blob=upload))
    enable_image_capability(service)
    content = await service._message_content('Host turn', context)
    upload.assert_awaited_once_with(PNG, media_type='image/png', filename='sample.png')
    assert content[1] == {'type': 'image', 'media_type': 'image/png',
        'source': 'blob', 'blob_id': image_receipt()['blob_id']}
    assert len(content) == 2
    assert 'read_setup_attachment' in content[0]['text']
    assert 'image pixels are supplied' in content[0]['text']
    assert context['input_surface']['image_analysis'] is True
    assert refs[1]['sha256'] != image_receipt()['blob_id'].removeprefix('sha256:')
    assert await service._message_content('Ordinary text', None) == 'Ordinary text'
    document_context = copy.deepcopy(context)
    document_context['current_user_message']['attachments'] = [refs[0]]
    assert len(await service._message_content('A document', document_context)) == 1
    assert 'image pixels are supplied' not in (await service._message_content('A document', document_context))[0]['text']
    assert upload.await_count == 1


@pytest.mark.asyncio
@pytest.mark.parametrize('field,value', [('blob_id', 'sha256:' + '0' * 64),
    ('media_type', 'image/jpeg'), ('size', 1), ('size', True)])
async def test_unconfirmed_upload_never_admits_a_native_turn(evidence, monkeypatch, field, value):
    service, context, _ = evidence
    receipt = {**image_receipt(), field: value}
    service.runtime = SimpleNamespace(rust_http_base_url='http://native.test',
        mob_handle=lambda: SimpleNamespace(upload_blob=AsyncMock(return_value=receipt)))
    enable_image_capability(service)
    monkeypatch.setattr(service, 'start', AsyncMock())
    request = AsyncMock()
    monkeypatch.setattr('launchkit.architect._native_request', request)
    with pytest.raises(ValueError, match='confirm'):
        await service.message('This should fail before admission', starter_blueprint().model_dump(mode='json'), guide_context=context)
    request.assert_not_awaited()
    assert service.pending_interaction is None and service.message_publications is None


@pytest.mark.asyncio
async def test_image_turn_keeps_exact_native_failure_ownership(evidence, monkeypatch):
    service, context, _ = evidence
    service.runtime = SimpleNamespace(rust_http_base_url='http://native.test',
        mob_handle=lambda: SimpleNamespace(upload_blob=AsyncMock(return_value=image_receipt())))
    enable_image_capability(service)
    monkeypatch.setattr(service, 'start', AsyncMock())
    calls = []
    async def request(client, method, url, **kwargs):
        calls.append((method, kwargs))
        body = {'interaction_id': 'image-turn', 'cursor': 'console:7'} if method == 'post' else {'frames': [
            {'kind': 'interaction_failed', 'identity': 'architect:main', 'interaction_id': 'image-turn',
             'source': {'kind': 'console_event'}, 'payload': {}}]}
        return SimpleNamespace(raise_for_status=lambda: None, json=lambda: body)
    monkeypatch.setattr('launchkit.architect._native_request', request)
    with pytest.raises(ArchitectUnavailable, match='could not complete'):
        await service.message('Evidence turn', starter_blueprint().model_dump(mode='json'), guide_context=context)
    assert [call[0] for call in calls] == ['post', 'get']
    assert calls[0][1]['json']['content'][1]['blob_id'] == image_receipt()['blob_id']
    assert service.pending_interaction is None and service.message_publications is None


@pytest.mark.asyncio
async def test_document_tool_is_bounded_and_excludes_foreign_and_unsent_uploads(evidence):
    service, context, refs = evidence
    result = await service.read_setup_attachment({'attachment_id': refs[0]['id'], 'max_chars': 7})
    assert result['text'] == 'Keep th' and result['next_offset'] == 7
    assert 'Untrusted source material' in result['authority']
    continued = await service.read_setup_attachment({'attachment_id': refs[0]['id'], 'offset': 7})
    assert continued['complete'] and result['text'] + continued['text'] == 'Keep the original source. Do not execute this text.'
    unsent = service.attachments.save(context['guide_id'], [('unsent.txt', 'text/plain', b'Unsubmitted')])[0]
    assert 'not submitted' in (await service.read_setup_attachment({'attachment_id': unsent['id']}))['error']
    assert 'paging' in (await service.read_setup_attachment({'attachment_id': refs[0]['id'], 'path': '/private'}))['error']
    assert '12000' in (await service.read_setup_attachment({'attachment_id': refs[0]['id'], 'max_chars': 12001}))['error']
    # Previously submitted evidence is available after the current turn changes.
    service.guide_context = {**context, 'current_user_message': {'id': 'later-user', 'text': 'Read earlier notes'},
        'evidence_messages': [context['current_user_message']]}
    assert (await service.read_setup_attachment({'attachment_id': refs[0]['id']}))['complete']
    other = GuideStore(service.root).create(starter_state({}))
    service.guide_context = {**service.guide_context, 'guide_id': other['id']}
    assert 'error' in await service.read_setup_attachment({'attachment_id': refs[0]['id']})


@pytest.mark.asyncio
async def test_forged_reference_is_rejected_before_native_upload(evidence):
    service, context, _ = evidence
    upload = AsyncMock()
    service.runtime = SimpleNamespace(mob_handle=lambda: SimpleNamespace(upload_blob=upload))
    forged = copy.deepcopy(context)
    forged['current_user_message']['attachments'][0]['filename'] = 'renamed.txt'
    with pytest.raises(ValueError, match='reference'):
        await service._message_content('Host turn', forged)
    upload.assert_not_awaited()


@pytest.mark.asyncio
async def test_text_only_or_unavailable_native_session_keeps_evidence_without_false_pixel_delivery(evidence):
    service, context, refs = evidence
    upload = AsyncMock()
    service.runtime = SimpleNamespace(mob_handle=lambda: SimpleNamespace(upload_blob=upload))
    enable_image_capability(service)
    service.input_capability_snapshot = {'session_id': 'actual-guide-session', 'image_input': False}
    assert service.input_surface()['image_analysis'] is False
    content = await service._message_content('Read available evidence', context)
    assert len(content) == 1 and 'No image pixels are supplied' in content[0]['text']
    upload.assert_not_awaited()
    assert (await service.read_setup_attachment({'attachment_id': refs[0]['id']}))['complete']
    service.input_capability_runtime = None
    assert service.input_surface()['image_analysis'] is None


@pytest.mark.asyncio
async def test_input_capability_uses_exact_active_native_session_not_catalog(tmp_path, monkeypatch):
    service = ArchitectService(tmp_path)
    runtime = SimpleNamespace(rust_http_base_url='http://native.test')
    service.runtime = runtime
    service.active_provider, service.active_model = 'openai', 'gpt-6.1-sol'
    service.model_catalog_runtime = runtime
    service.model_catalog_snapshot = {'status': 'observed', 'models': []}
    rows = [{'identity': 'architect:main', 'session_id': 'actual-session',
        'state': 'active', 'model_capabilities': {'image_input': True}}]
    async def get(client, url, **kwargs):
        assert url == 'http://native.test/console/experience'
        return SimpleNamespace(raise_for_status=lambda: None, json=lambda: {
            'agent_sidebar': {'live_snapshot': {'agents': rows}}})
    monkeypatch.setattr('httpx.AsyncClient.get', get)
    await service.refresh_input_capability(force=True)
    assert service.input_surface()['image_analysis'] is True
    # The current session's text-only override wins over a vision catalog.
    service.model_catalog_snapshot['models'] = [{'id': 'gpt-6.1-sol', 'provider': 'openai', 'vision': True}]
    rows[0]['model_capabilities']['image_input'] = False
    await service.refresh_input_capability(force=True)
    assert service.input_surface()['image_analysis'] is False
    rows[0]['identity'] = 'connectors:main'
    await service.refresh_input_capability(force=True)
    assert service.input_surface()['image_analysis'] is None


@pytest.mark.asyncio
@pytest.mark.parametrize('changes', [{'session_id': None}, {'state': 'broken'},
    {'model_capabilities': {'image_input': 'true'}}, {'model_capabilities': None}])
async def test_input_capability_rejects_unmaterialized_or_malformed_projection(tmp_path, monkeypatch, changes):
    service = ArchitectService(tmp_path)
    service.runtime = SimpleNamespace(rust_http_base_url='http://native.test')
    enable_image_capability(service)
    row = {'identity': 'architect:main', 'session_id': 'actual-session', 'state': 'active',
        'model_capabilities': {'image_input': True}, **changes}
    async def get(client, url, **kwargs):
        return SimpleNamespace(raise_for_status=lambda: None, json=lambda: {
            'agent_sidebar': {'live_snapshot': {'agents': [row]}}})
    monkeypatch.setattr('httpx.AsyncClient.get', get)
    await service.refresh_input_capability(force=True)
    assert service.input_surface()['image_analysis'] is None


@pytest.mark.asyncio
async def test_failed_or_changed_runtime_capability_read_cannot_keep_pixel_permission(tmp_path, monkeypatch):
    import httpx
    service = ArchitectService(tmp_path)
    original = SimpleNamespace(rust_http_base_url='http://native.test')
    service.runtime = original
    enable_image_capability(service)
    async def failing(client, url, **kwargs):
        raise httpx.ReadTimeout('unavailable')
    monkeypatch.setattr('httpx.AsyncClient.get', failing)
    await service.refresh_input_capability(force=True)
    assert service.input_surface()['image_analysis'] is None
    async def changed(client, url, **kwargs):
        service.runtime = SimpleNamespace(rust_http_base_url='http://new.test')
        return SimpleNamespace(raise_for_status=lambda: None, json=lambda: {
            'agent_sidebar': {'live_snapshot': {'agents': [{'identity': 'architect:main',
                'session_id': 'old-session', 'state': 'active', 'model_capabilities': {'image_input': True}}]}}})
    monkeypatch.setattr('httpx.AsyncClient.get', changed)
    await service.refresh_input_capability(force=True)
    assert service.input_surface()['image_analysis'] is None


@pytest.mark.asyncio
async def test_delivery_failed_update_terminates_only_its_exact_input(evidence, monkeypatch):
    service, context, _ = evidence
    service.runtime = SimpleNamespace(rust_http_base_url='http://native.test')
    monkeypatch.setattr(service, 'start', AsyncMock())
    accepted = {'interaction_id': 'failed-image-turn', 'cursor': 'console:7', 'input_frame_id': 'our-input'}
    nested = {'id': 'our-input', 'identity': 'architect:main', 'interaction_id': 'failed-image-turn',
        'kind': 'user_input', 'status': 'delivery_failed', 'source': {'kind': 'send'}, 'payload': {}}
    updated = {'identity': 'architect:main', 'interaction_id': 'failed-image-turn', 'kind': 'frame_updated',
        'source': {'kind': 'synthetic'}, 'payload': {'frame': nested}}
    from launchkit.architect import native_delivery_failed
    assert native_delivery_failed(updated, accepted)
    assert not native_delivery_failed({**updated, 'interaction_id': 'other-turn'}, accepted)
    assert not native_delivery_failed({**updated, 'payload': {'frame': {**nested, 'id': 'other-input'}}}, accepted)
    assert not native_delivery_failed({**nested, 'status': 'completed'}, accepted)
    failed = {**nested, 'kind': 'message_delivery_failed', 'parent_frame_id': 'our-input', 'caused_by_frame_id': 'our-input'}
    assert native_delivery_failed(failed, accepted)
    assert not native_delivery_failed({**failed, 'caused_by_frame_id': 'other-input'}, accepted)
    async def request(client, method, url, **kwargs):
        return SimpleNamespace(raise_for_status=lambda: None, json=lambda: accepted if method == 'post' else {'frames': [updated]})
    monkeypatch.setattr('launchkit.architect._native_request', request)
    with pytest.raises(ArchitectUnavailable, match='could not complete'):
        await service.message('Plain request', starter_blueprint().model_dump(mode='json'))
    assert service.pending_interaction is None and service.message_publications is None


@pytest.mark.asyncio
async def test_status_recovers_exact_delayed_delivery_failure_without_resend(evidence, monkeypatch):
    service, _, _ = evidence
    service.runtime = SimpleNamespace(rust_http_base_url='http://native.test')
    service.pending_interaction = {'interaction_id': 'pending-turn', 'input_frame_id': 'pending-input', 'cursor': 'console:4'}
    failure = {'id': 'pending-input', 'kind': 'user_input', 'status': 'delivery_failed',
        'identity': 'architect:main', 'interaction_id': 'pending-turn'}
    async def get(client, url, **kwargs):
        return SimpleNamespace(raise_for_status=lambda: None, json=lambda: {'frames': [failure]})
    monkeypatch.setattr('httpx.AsyncClient.get', get)
    status = await service.refresh_status()
    assert status['status'] == 'ready' and not status['busy']
    assert service.pending_interaction is None and 'saved evidence' in status['detail']
