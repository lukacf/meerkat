"""Local SQLite/ASGI/HTTP transport controls; no provider or native runtime call."""
import asyncio
import copy
import json
from types import SimpleNamespace

import httpx
import pytest

from launchkit.architect import ArchitectService
from launchkit.console_command import send_command
from launchkit.guide import GuideService
from launchkit.guide_commands import COMMAND_ORIGIN, GuideCommand, pending_command
from launchkit.guide_models import starter_state
from launchkit.guide_operations import GuideOperations
from launchkit.guide_store import GuideStore
from launchkit.projects import DraftConflict
from launchkit.server import GuideResponse

KEY = 'a' * 64
ACCEPTED = {'identity': 'architect:main', 'interaction_id': 'turn-original',
            'input_frame_id': 'frame-original', 'cursor': 'console:1',
            'session_id': 'session-original', 'status': 'delivered'}
REQUEST = {'identity': 'architect:main', 'origin': COMMAND_ORIGIN, 'idempotency_key': KEY,
           'handling_mode': 'queue', 'content': 'Exact frozen content'}

EXPERIENCE = {'send_dedupe': {'durable': True}, 'agent_sidebar': {'live_snapshot': {'agents': [
    {'identity': 'architect:main', 'session_id': 'session-original', 'state': 'active'}]}}}


def command_fixture(tmp_path):
    store = GuideStore(tmp_path)
    state = store.create(starter_state({}))
    value = GuideResponse(version=1, action='chat', text='A single original request')
    command = GuideCommand(store, KEY, state['id'], value.model_dump(mode='json'))
    command.prepare({'context': {'original_message_id': 'user-original'}})
    return store, state, value, command


def test_frozen_request_survives_reopen_and_conflicting_reuse(tmp_path):
    store, state, value, command = command_fixture(tmp_path)
    reopened = GuideCommand(GuideStore(tmp_path), KEY, state['id'], value.model_dump(mode='json'))
    assert reopened.prepare({'context': {'original_message_id': 'replacement'}}) == command.load()['prepared']
    for guide_id, body in [(store.create(starter_state({}))['id'], value.model_dump(mode='json')),
                           (state['id'], {**value.model_dump(mode='json'), 'text': 'Changed'})]:
        with pytest.raises(DraftConflict, match='original setup or request'):
            GuideCommand(store, KEY, guide_id, body).load()
    second = GuideCommand(store, 'b' * 64, state['id'], value.model_dump(mode='json'))
    with pytest.raises(DraftConflict, match='previous guide command'):
        second.prepare({})
    assert pending_command(store) == KEY
    assert (tmp_path / 'guided-setups.sqlite3').stat().st_mode & 0o777 == 0o600


def test_guide_revision_and_command_completion_are_one_transaction(tmp_path):
    store, state, value, command = command_fixture(tmp_path)
    with pytest.raises(DraftConflict, match='previous guide command'):
        store.save(state, 1)
    with pytest.raises(DraftConflict, match='cannot complete'):
        store.save(state, 1, command=command)
    assert store.get(state['id']) == state and command.load()['applied'] is None
    command.result({'message': 'Validated original result'})
    saved = store.save(state, 1, command=command)
    assert GuideStore(tmp_path).get(state['id']) == saved == command.load()['applied']
    assert pending_command(store) is None
    with pytest.raises(DraftConflict):
        store.save(state, 1, command=command)
    assert store.get(state['id'])['version'] == 2


@pytest.mark.asyncio
async def test_lost_ack_recovers_without_sending_again(tmp_path):
    store, state, value, command = command_fixture(tmp_path)
    calls = []
    def transport(request):
        calls.append((request.url.path, json.loads(request.content) if request.content else None))
        if request.url.path == '/console/experience':
            return httpx.Response(200, json=EXPERIENCE)
        if request.url.path == '/console/send':
            raise httpx.ReadTimeout('Lost response after native admission')
        return httpx.Response(200, json={**ACCEPTED, 'native': {'state': 'admitted'}})
    async with httpx.AsyncClient(transport=httpx.MockTransport(transport)) as client:
        with pytest.raises(httpx.ReadTimeout):
            await send_command(client, 'http://native.test', command, REQUEST, 'original-runtime')
        reopened = GuideCommand(GuideStore(tmp_path), KEY, state['id'], value.model_dump(mode='json'))
        result = await send_command(client, 'http://native.test', reopened, REQUEST, 'original-runtime')
    assert result['interaction_id'] == ACCEPTED['interaction_id']
    assert [path for path, _ in calls].count('/console/send') == 1
    assert calls[-1] == ('/console/send/recover', {k: REQUEST[k] for k in ('identity', 'origin', 'idempotency_key')})
    assert command.load()['acceptance']['input_frame_id'] == 'frame-original'
    from launchkit.guide_commands import command_response
    projection = command_response(store, KEY, state['id'])
    assert projection['status'] == 'pending' and projection['response'] is None
    assert projection['native_receipt']['interaction_id'] == 'turn-original'
    assert projection['native_receipt']['conversation_id'] is None


@pytest.mark.asyncio
@pytest.mark.parametrize('case', ['nondurable', 'unknown', 'notfound', 'failed', 'moved', 'different_request', 'missing_session', 'replaced_receipt'])
async def test_uncertain_or_conflicting_recovery_never_creates_a_new_send(tmp_path, case):
    store, state, value, command = command_fixture(tmp_path)
    command.native_request(REQUEST, 'original-runtime')
    command.begin_dispatch()
    command.acceptance(ACCEPTED)
    posts = []
    def transport(request):
        if request.method == 'GET':
            return httpx.Response(200, json={'send_dedupe': {'durable': case != 'nondurable'}})
        posts.append(request.url.path)
        assert request.url.path == '/console/send/recover'
        if case == 'notfound':
            return httpx.Response(404, json={'error': {'kind': 'send_not_found'}})
        result = {**ACCEPTED, 'status': 'accepted', 'native': {'state': 'unknown'}}
        if case == 'failed': result['status'] = 'delivery_failed'
        if case == 'missing_session': result.pop('session_id')
        if case == 'replaced_receipt': result['interaction_id'] = 'different-interaction'
        return httpx.Response(200, json=result)
    async with httpx.AsyncClient(transport=httpx.MockTransport(transport)) as client:
        with pytest.raises(DraftConflict):
            await send_command(client, 'http://native.test', command,
                               {**REQUEST, 'content': 'changed'} if case == 'different_request' else REQUEST,
                               'moved-runtime' if case == 'moved' else 'original-runtime')
    assert '/console/send' not in posts
    assert command.load()['failure'] == ('native_delivery_failed' if case == 'failed' else None)


@pytest.mark.asyncio
async def test_native_absent_frame_replays_identical_content_and_original_key(tmp_path):
    store, state, value, command = command_fixture(tmp_path)
    command.native_request(REQUEST, 'original-runtime')
    command.begin_dispatch()
    posts = []
    def transport(request):
        if request.method == 'GET':
            return httpx.Response(200, json=EXPERIENCE)
        posts.append((request.url.path, json.loads(request.content)))
        return httpx.Response(200, json={**ACCEPTED, 'status': 'accepted', 'native': {'state': 'absent'}})
    async with httpx.AsyncClient(transport=httpx.MockTransport(transport)) as client:
        await send_command(client, 'http://native.test', command, REQUEST, 'original-runtime')
    assert [path for path, _ in posts] == ['/console/send/recover', '/console/send']
    assert posts[-1][1] == REQUEST


@pytest.mark.asyncio
@pytest.mark.parametrize('interruption', ['after_publication', 'before_guide_save'])
async def test_actual_guide_reopens_original_publication_and_applies_once(tmp_path, monkeypatch, interruption):
    from test_guided_edit_publication import initial_edits, initial_question, edit_args
    monkeypatch.setattr('launchkit.guide.planner_capability', lambda: {'available': True, 'provider': 'openai'})
    real_client = httpx.AsyncClient
    author = ArchitectService(tmp_path)
    service = GuideService(GuideStore(tmp_path), author)
    initial = await service.create()
    value = GuideResponse(version=1, action='chat', text='Compare the documents I provide and cite differences.')
    sends, contexts, acknowledgements = [], [], []
    interrupted = False
    current_author = author

    async def transport(request):
        nonlocal interrupted
        if request.url.path == '/console/experience':
            return httpx.Response(200, json=EXPERIENCE)
        if request.url.path == '/console/send':
            sends.append(json.loads(request.content))
            context = current_author.guide_context
            contexts.append(copy.deepcopy(context))
            ack = await current_author.publish_guide_edit(edit_args(current_author,
                initial_edits(context['current_user_message']['id']), publication_mode='advance', question=initial_question(),
                message='A first comparison team. Who should it serve?', summary='Original published response.'))
            assert ack['valid'], ack
            acknowledgements.append(ack)
            return httpx.Response(200, json=ACCEPTED)
        if request.url.path == '/console/send/recover':
            return httpx.Response(200, json={**ACCEPTED, 'native': {'state': 'admitted'}})
        if interruption == 'after_publication' and not interrupted:
            interrupted = True
            raise asyncio.CancelledError()
        def frame(kind, payload, interaction='turn-original'):
            return {'kind': kind, 'identity': 'architect:main', 'interaction_id': interaction,
                    'source': {'kind': 'console_event'}, 'payload': payload}
        return httpx.Response(200, json={'frames': [
            frame('interaction_complete', {'result': 'Unrelated later output'}, 'other-turn'),
            frame('tool_result_received', {'name': 'publish_guide_edit', 'is_error': False,
                                          'result': json.dumps(acknowledgements[0])}),
            frame('interaction_complete', {'result': 'Original completed output'})]})

    monkeypatch.setattr(httpx, 'AsyncClient', lambda **kwargs: real_client(transport=httpx.MockTransport(transport), **kwargs))
    def bind(instance):
        instance.runtime = SimpleNamespace(rust_http_base_url='http://native.test')
        instance.reference_read = True
        async def start():
            assert instance.guide_edit_turn['token'] == instance.guide_context['edit_base_token']
            if instance is not author:
                assert instance.guide_context['current_user_message'] == contexts[0]['current_user_message']
                assert instance.proposal is not None and instance.guide_result is not None
        monkeypatch.setattr(instance, 'start', start)
    bind(author)
    if interruption == 'before_guide_save':
        async def fail_save(*args, **kwargs): raise OSError('Simulated stop before guide transaction')
        monkeypatch.setattr(service, '_save', fail_save)
    with pytest.raises(asyncio.CancelledError if interruption == 'after_publication' else OSError):
        await service.respond(initial['id'], value, command_id=KEY)
    command = GuideCommand(service.store, KEY, initial['id'], value.model_dump(mode='json'))
    retained = command.load()
    assert retained['publications'] and retained['applied'] is None
    assert service.store.get(initial['id'])['version'] == 1
    other = await service.create()
    with pytest.raises(DraftConflict, match='previous guide command'):
        await service.respond(other['id'], value)
    with pytest.raises(DraftConflict, match='previous guide command'):
        await service.apply(initial['id'], 1)
    current_author = ArchitectService(tmp_path)
    reopened = GuideService(GuideStore(tmp_path), current_author)
    bind(current_author)
    operations = GuideOperations(wait_seconds=2)
    saved, pending = await operations.submit(reopened, initial['id'], value, command_id=KEY)
    assert not pending and saved['version'] == 2
    assert len(sends) == 1
    assert sends[0]['idempotency_key'] == KEY and sends[0]['origin'] == COMMAND_ORIGIN
    assert saved['messages'][-2]['id'] == contexts[0]['current_user_message']['id']
    assert saved['messages'][-1]['text'] == 'A first comparison team. Who should it serve?'
    assert command.load()['result']['message'] == 'Original completed output'
    assert command.load()['applied']['version'] == 2
    later = reopened.store.save(reopened.store.get(initial['id']), 2)
    replay, pending = await operations.submit(reopened, initial['id'], value, command_id=KEY)
    assert not pending and replay['version'] == 2
    assert reopened.store.get(initial['id']) == later and later['version'] == 3
    assert len(sends) == 1


@pytest.mark.asyncio
async def test_command_route_is_bounded_and_forwards_only_original_command(tmp_path, monkeypatch):
    from launchkit import server
    store = GuideStore(tmp_path)
    state = store.create(starter_state({}))
    calls = []
    async def submit(service, identity, value, *, command_id):
        calls.append((identity, command_id, value.model_dump(mode='json')))
        return {'id': identity, 'version': 2}, False
    monkeypatch.setattr(server, 'guide_operations', SimpleNamespace(submit=submit))
    async with httpx.AsyncClient(transport=httpx.ASGITransport(app=server.app), base_url='http://testserver') as client:
        path = f'/api/guide/{state["id"]}/commands/{KEY}'
        for body in [{'version': 1, 'text': 'hello', 'workspace': 'foreign'},
                     {'version': 1, 'text': 'x' * 6001}, {'version': True, 'text': 'hello'}]:
            assert (await client.post(path, json=body)).status_code == 422
        assert (await client.post(path[:-1] + 'z', json={'version': 1, 'text': 'hello'})).status_code == 422
        response = await client.post(path, json={'version': 1, 'text': 'hello'})
    assert response.status_code == 200 and 'no-store' in response.headers['cache-control']
    assert calls == [(state['id'], KEY, GuideResponse(version=1, action='chat', text='hello').model_dump(mode='json'))]


@pytest.mark.asyncio
async def test_retained_command_read_is_exact_and_never_resumes_work(tmp_path, monkeypatch):
    from launchkit import server
    store, state, value, command = command_fixture(tmp_path)
    monkeypatch.setattr(server, 'guide', SimpleNamespace(store=store))
    async with httpx.AsyncClient(transport=httpx.ASGITransport(app=server.app), base_url='http://testserver') as client:
        path = f'/api/guide/{state["id"]}/commands/{KEY}'
        before = command.load()
        pending = await client.get(path)
        assert pending.status_code == 200 and pending.json()['status'] == 'pending'
        assert command.load() == before and not command.load().get('dispatch_attempted')
        other = store.create(starter_state({}))
        assert (await client.get(path.replace(state['id'], other['id']))).status_code == 404
        command.result({'message': 'Completed original response'})
        saved = store.save(state, 1, command=command)
        later = store.save(saved, 2)
        result = await client.get(path)
        assert result.json()['status'] == 'completed' and result.json()['response'] == saved
        assert store.get(state['id']) == later


@pytest.mark.asyncio
async def test_pending_command_guards_context_credentials_and_console_actions(tmp_path, monkeypatch):
    from launchkit import server
    from launchkit.blueprint import starter_blueprint
    store, state, value, command = command_fixture(tmp_path)
    monkeypatch.setattr(server, 'guide', SimpleNamespace(store=store))
    author = ArchitectService(tmp_path)
    monkeypatch.setattr(server, 'architect', author)
    monkeypatch.setattr(server, 'resolve_guide_owner', lambda *args: object())
    assert server.provider_owned_work() is True
    async with httpx.AsyncClient(transport=httpx.ASGITransport(app=server.app), base_url='http://testserver') as client:
        for path, body in [('/api/architect/context', {'blueprint': starter_blueprint().model_dump(mode='json')}),
                           ('/api/architect/start', {'blueprint': starter_blueprint().model_dump(mode='json')}),
                           ('/api/owned-console/guide/test-generation/console/send', {})]:
            result = await client.post(path, json=body)
            assert result.status_code == 409, result.text
    assert command.load()['native'] == {} and author.runtime is None


@pytest.mark.asyncio
async def test_cold_identity_is_not_dispatched_without_a_recoverable_session(tmp_path):
    store, state, value, command = command_fixture(tmp_path)
    requests = []
    def transport(request):
        requests.append(request.method)
        return httpx.Response(200, json={'send_dedupe': {'durable': True}})
    async with httpx.AsyncClient(transport=httpx.MockTransport(transport)) as client:
        with pytest.raises(DraftConflict, match='session is not ready'):
            await send_command(client, 'http://native.test', command, REQUEST, 'original-runtime')
    assert requests == ['GET'] and not command.load().get('dispatch_attempted')
