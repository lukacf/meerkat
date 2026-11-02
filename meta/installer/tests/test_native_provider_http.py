"""Actual HTTP/SDK pipe composition; synthetic peer, no native/provider run."""
import asyncio
from dataclasses import replace
import hashlib
import json

import httpx
import pytest

from launchkit import server
from launchkit.connect_installer_caller import ExactFile
from launchkit.provider_setup_host import ForgeProviderSetupSession, ProviderSetupUnavailable
from test_provider_setup_host import sdk, construction, until, PEER


@pytest.fixture
async def route(construction, monkeypatch):
    pin, binding, login, current = construction
    owner = ForgeProviderSetupSession(pin, binding, login)
    context = [login]
    monkeypatch.setattr(server, 'provider_setup_route', None)
    resolver = lambda request: context[0]
    selected = server.select_provider_setup_session(owner, resolver)
    transport = httpx.ASGITransport(app=server.app)
    # The fixture's original authenticated origin is retained, no header identity.
    async with httpx.AsyncClient(transport=transport, base_url='http://testserver',
                                headers={'origin': login.origin}) as client:
        yield client, selected, owner, context, resolver
    await selected.shutdown()


async def test_http_selection_uses_original_prompt_and_never_activates_guide(route, construction, tmp_path):
    client, selected, owner, context, _ = route
    login = context[0]
    assert (await client.get('/api/provider-setup')).json()['status'] == 'not_started'
    assert not (tmp_path/'started').exists()
    assert (await client.post('/api/provider-setup/begin', json={})).status_code == 200
    await until(lambda: owner.snapshot(login)['status'] == 'prompt')
    prompt = (await client.get('/api/provider-setup')).json()
    assert prompt['prompt']['choices'][0]['choice_id'] == 'native-choice'
    for body in [
        {'prompt_id': 'foreign', 'answer': {'kind': 'choice', 'choice_id': 'native-choice'}},
        {'prompt_id': 'native-prompt', 'answer': {'kind': 'action', 'action': 'sign_in'}},
        {'prompt_id': 'native-prompt', 'answer': {'kind': 'choice', 'choice_id': 'forged'}}]:
        assert (await client.post('/api/provider-setup/answer', json=body)).status_code == 409
    assert not (tmp_path/'commands').exists()
    body = {'prompt_id': 'native-prompt', 'answer': {'kind': 'choice', 'choice_id': 'native-choice'}}
    assert (await client.post('/api/provider-setup/answer', json=body)).status_code == 200
    assert (await client.post('/api/provider-setup/answer', json=body)).status_code == 409
    await until(lambda: owner.snapshot(login)['status'] == 'selected')
    result = await client.get('/api/provider-setup')
    assert result.json()['joined'] is True
    assert result.json()['selection']['model'] == 'native-model'
    assert 'auth_binding' not in result.text
    assert result.headers['cache-control'] == 'no-store'
    assert json.loads((tmp_path/'commands').read_text())['id'] == 'answer-1'
    # Guide activation has no HTTP action on this surface.
    assert (await client.post('/api/provider-setup/activate', json={})).status_code == 404


async def test_http_identity_origin_and_private_body_refusals_before_launch(route, construction, tmp_path):
    client, _, owner, context, _ = route
    original = context[0]
    for other in [None, replace(original), replace(original, login_scope_id='other')]:
        context[0] = other
        assert (await client.get('/api/provider-setup')).status_code == 403
        assert (await client.post('/api/provider-setup/begin', json={})).status_code == 403
    context[0] = original
    assert (await client.post('/api/provider-setup/begin', json={}, headers={'origin':'https://foreign.invalid'})).status_code == 403
    for payload in ['{"private":"secret-canary"}', '{"x":1,"x":2}', '[]', 'not json']:
        result = await client.post('/api/provider-setup/begin', content=payload, headers={'content-type':'application/json'})
        assert result.status_code == 400
        assert 'secret-canary' not in result.text
    assert (await client.post('/api/provider-setup/begin?login_scope_id=foreign', json={})).status_code == 400
    assert (await client.post('/api/provider-setup/begin', content='x'*65537,
                             headers={'content-type':'application/json'})).status_code == 413
    assert not (tmp_path/'started').exists()
    construction[3][0] = False
    assert (await client.get('/api/provider-setup')).status_code == 403


async def test_cancel_timeout_keeps_original_child_and_readable_native_terminal(route, tmp_path):
    client, _, owner, context, _ = route
    login = context[0]
    (tmp_path/'hold_exit').touch()
    try:
        await client.post('/api/provider-setup/begin', json={})
        await until(lambda:owner.snapshot(login)['status']=='prompt')
        child = owner._client._process
        with pytest.raises(TimeoutError):
            await asyncio.wait_for(client.post('/api/provider-setup/cancel', json={}), .08)
        assert child.poll() is None
        pending = (await client.get('/api/provider-setup')).json()
        assert pending['status'] in ('closing', 'joining') and pending['joined'] is False
        assert (await client.post('/api/provider-setup/begin', json={})).json()['joined'] is False
        (tmp_path/'hold_exit').unlink()
        await until(lambda:owner.snapshot(login)['status']=='cancelled')
        result = (await client.get('/api/provider-setup')).json()
        assert result['joined'] is True and result['terminal']['kind']=='cancelled'
        assert owner._client._process is child and child.returncode==0
        assert json.loads((tmp_path/'commands').read_text())['method']=='cancel'
    finally:
        (tmp_path/'hold_exit').unlink(missing_ok=True)


async def test_separate_original_logins_retire_independently_and_cannot_reenroll(route, construction):
    client, selected, first, context, resolver = route
    pin, binding, login, _ = construction
    second_login = replace(login, login_scope_id='second-login')
    second = ForgeProviderSetupSession(pin, replace(binding, login_scope_id='second-login'), second_login)
    assert server.select_provider_setup_session(second, resolver) is selected
    context[0] = second_login
    assert (await client.get('/api/provider-setup')).json()['status']=='not_started'
    first.revoke(); await first.close()
    assert (await client.get('/api/provider-setup')).status_code==200
    context[0] = login
    assert (await client.get('/api/provider-setup')).status_code==403
    with pytest.raises(ProviderSetupUnavailable):
        server.select_provider_setup_session(ForgeProviderSetupSession(pin, binding, login), resolver)
    with pytest.raises(ProviderSetupUnavailable):
        server.select_provider_setup_session(second, lambda request: second_login)


async def test_native_failure_reason_and_notices_survive_actual_pipe_join(construction, tmp_path):
    pin, binding, login, _ = construction
    script = PEER.replace("emit({'kind':'prompt',", "emit({'kind':'notice','interaction_id':'native-interaction','notice':{'kind':'target_saved','native':'retained'}})\nemit({'kind':'prompt',")
    script = script.replace("emit({'kind':'selected','selection':{'interaction_id':'native-interaction','model':'native-model','provider':'openai','auth_binding':{'realm':'native-realm','binding':'native-binding','origin':'configured'},'credential':'ready','live_entitlement':'unobserved'}})",
        "emit({'kind':'failed','interaction_id':'native-interaction','reason':'native_refusal_or_unavailable','native_effects':'reobserve','joined':True,'join_scope':'provider_interaction'})")
    pin.path.write_text(pin.path.read_text().split('\n',1)[0]+'\n'+script+'\nsys.exit(1)\n')
    owner = ForgeProviderSetupSession(ExactFile(pin.path, hashlib.sha256(pin.path.read_bytes()).hexdigest()), binding, login)
    try:
        await owner.begin(login)
        await until(lambda:owner.snapshot(login)['status']=='prompt')
        assert owner.snapshot(login)['notices'][0]['kind']=='target_saved'
        await owner.answer(login, 'native-prompt', {'kind':'choice','choice_id':'native-choice'})
        await until(lambda:owner.snapshot(login)['status']=='failed')
        result = owner.snapshot(login)
        assert result['joined'] is True
        assert result['terminal']=={'kind':'failed','reason':'native_refusal_or_unavailable','native_effects':'reobserve','join_scope':'provider_interaction'}
        assert result['notices'][0]['native']=='retained'
        assert owner._client._process.returncode==1
        await until(lambda:owner.snapshot(login)['restart_available'])
        first=owner._client
        await owner.restart(login,0)
        assert owner._client is not first
        assert owner._previous_attempts[0][0] is first
        assert owner._previous_attempts[0][1]['kind']=='failed'
        assert owner._previous_attempts[0][2] is True
    finally: await owner.close()


async def test_transport_failure_is_readable_while_same_child_cleanup_is_pending(construction, tmp_path, monkeypatch):
    pin, binding, login, _ = construction
    script = PEER.split("request=json.loads(sys.stdin.readline())")[0] + """
while not (root/'fail_transport').exists():time.sleep(.01)
print('invalid-json',flush=True)
while (root/'hold_exit').exists():time.sleep(.01)
sys.exit(1)
"""
    pin.path.write_text(pin.path.read_text().split('\n', 1)[0] + '\n' + script)
    owner = ForgeProviderSetupSession(ExactFile(pin.path, hashlib.sha256(pin.path.read_bytes()).hexdigest()), binding, login)
    monkeypatch.setattr(server, 'provider_setup_route', None)
    route = server.select_provider_setup_session(owner, lambda request: login)
    (tmp_path/'hold_exit').touch()
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=server.app), base_url='http://testserver',
                                     headers={'origin': login.origin}) as client:
            await client.post('/api/provider-setup/begin', json={})
            await until(lambda: owner.snapshot(login)['status'] == 'prompt')
            child = owner._client._process
            (tmp_path/'fail_transport').touch()
            await until(lambda: owner.snapshot(login)['status'] == 'failed')
            failed = (await client.get('/api/provider-setup')).json()
            assert failed['joined'] is False and child.poll() is None
            assert 'prompt' not in failed and 'selection' not in failed
            with pytest.raises(ProviderSetupUnavailable):
                owner.selection(login)
            (tmp_path/'hold_exit').unlink()
            await until(lambda: owner.snapshot(login)['joined'])
            closed = (await client.get('/api/provider-setup')).json()
            assert closed['status'] == 'failed' and closed['joined'] is True
            assert owner._client._process is child and child.returncode == 1
            assert closed['restart_available'] is False
            with pytest.raises(ProviderSetupUnavailable):await owner.restart(login,0)
    finally:
        (tmp_path/'fail_transport').touch()
        (tmp_path/'hold_exit').unlink(missing_ok=True)
        await route.shutdown()


async def test_retired_surface_refuses_and_native_mode_refuses_legacy_save(route):
    client, selected, _, _, _ = route
    assert (await client.get('/api/provider-settings', headers={'origin':'http://testserver'})).json()['native_provider_setup'] is True
    result = await client.put('/api/provider-settings', json={'provider':'openai','api_key':'secret-canary'}, headers={'origin':'http://testserver'})
    assert result.status_code==409 and 'secret-canary' not in result.text
    await selected.shutdown()
    assert (await client.get('/api/provider-setup')).status_code==403


async def test_http_restart_rejects_stale_answers_and_cancels_even_with_reused_prompt_id(route, tmp_path):
    client,_,owner,context,_=route
    login=context[0]
    await client.post('/api/provider-setup/begin',json={'attempt':0})
    await until(lambda:owner.snapshot(login)['status']=='prompt')
    await client.post('/api/provider-setup/cancel',json={'attempt':0})
    await until(lambda:owner.snapshot(login)['restart_available'])
    assert (await client.post('/api/provider-setup/restart',json={})).status_code==400
    assert (await client.post('/api/provider-setup/restart',json={'attempt':0})).status_code==200
    await until(lambda:owner.snapshot(login)['status']=='prompt')
    (tmp_path/'commands').unlink()
    for action,body in [('begin',{}),('cancel',{}),('cancel',{'attempt':0}),
                        ('restart',{'attempt':0}),('restart',{'attempt':True}),
                        ('answer',{'attempt':0,'prompt_id':'native-prompt',
                                   'answer':{'kind':'choice','choice_id':'native-choice'}})]:
        assert (await client.post('/api/provider-setup/'+action,json=body)).status_code==409
    assert not (tmp_path/'commands').exists()
    body={'attempt':1,'prompt_id':'native-prompt','answer':{'kind':'choice','choice_id':'native-choice'}}
    assert (await client.post('/api/provider-setup/answer',json=body)).status_code==200
    await until(lambda:owner.snapshot(login)['status']=='selected')
    assert len(owner._previous_attempts)==1


async def test_uncomposed_native_surface_has_no_fallback(monkeypatch):
    monkeypatch.setattr(server, 'provider_setup_route', None)
    async with httpx.AsyncClient(transport=httpx.ASGITransport(app=server.app), base_url='http://testserver') as client:
        for path in ('/api/provider-setup', '/api/provider-setup/begin'):
            result = await client.get(path) if path.endswith('setup') else await client.post(path, json={})
            assert result.status_code==503


async def test_logout_during_body_read_refuses_before_native_launch(route, construction, tmp_path):
    client, _, _, _, _ = route
    async def body():
        yield b'{'
        construction[3][0] = False
        yield b'}'
    result = await client.post('/api/provider-setup/begin', content=body(), headers={'content-type':'application/json'})
    assert result.status_code==409
    assert not (tmp_path/'started').exists()


async def test_shutdown_fences_all_logins_and_joins_child_after_waiter_cancel(route, construction, tmp_path):
    client, selected, first, context, resolver = route
    pin, binding, login, _ = construction
    other_login = replace(login, login_scope_id='another-login')
    second = ForgeProviderSetupSession(pin, replace(binding, login_scope_id='another-login'), other_login)
    server.select_provider_setup_session(second, resolver)
    (tmp_path/'hold_exit').touch()
    try:
        await client.post('/api/provider-setup/begin', json={})
        await until(lambda:first.snapshot(login)['status']=='prompt')
        closing = asyncio.create_task(selected.shutdown())
        await until(lambda:first._closed)
        closing.cancel(); await asyncio.sleep(.03)
        assert not closing.done()
        for original in (login, other_login):
            context[0] = original
            assert (await client.get('/api/provider-setup')).status_code==403
        assert not selected.retired
        (tmp_path/'hold_exit').unlink()
        with pytest.raises(asyncio.CancelledError): await asyncio.wait_for(closing, 5)
        assert selected.retired and first._client._process.returncode==0
    finally: (tmp_path/'hold_exit').unlink(missing_ok=True)
