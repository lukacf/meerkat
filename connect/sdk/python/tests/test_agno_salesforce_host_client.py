"""Actual Requests/Agno/MCP composition over inert, owned test transports."""
from __future__ import annotations

import asyncio
from contextlib import ExitStack, redirect_stderr, redirect_stdout
import inspect
from email.message import Message
from types import SimpleNamespace
import io
import json
import os
import threading
import unittest
from unittest.mock import patch

from mcp import Client
import requests
from urllib3.util.retry import Retry

from meerkat_toolkit.adapters.agno_mcp import create_server
from meerkat_toolkit.adapters.agno_profiles import (
    ProfileConfigError, build_profile, build_salesforce_profile_from_client,
    validate_configuration,
)
from meerkat_toolkit.adapters.agno_transports import ServiceSession

ORIGIN = 'https://selected.salesforce.com'
RECORD = '001' + '4' * 12
CANARY = 'private-host-auth-test-value'


class Auth(requests.auth.AuthBase):
    def __init__(self, token='first-host-value'):
        self.token, self.calls, self.denied = token, [], False

    def __call__(self, request):
        self.calls.append(request.url)
        if self.denied:
            raise ValueError(CANARY)
        request.headers['Authorization'] = 'Bearer ' + self.token
        return request


class Raw(io.BytesIO):
    reads = 0
    closes = 0

    def read(self, *args, **kwargs):
        self.reads += 1
        return super().read(*args, **kwargs)

    def close(self):
        self.closes += 1
        return super().close()

    def release_conn(self):
        self.close()


def response(request, *, status=200, value=None, raw=None, length=None):
    result = requests.Response()
    result.status_code, result.url, result.request = status, request.url if request is not None else ORIGIN, request
    result.headers.update({'Content-Type': 'application/json', 'Location': 'https://foreign.invalid/'})
    if length is not None:
        result.headers['Content-Length'] = str(length)
    result.raw = Raw(json.dumps({'Id': RECORD, 'Name': 'Selected record'} if value is None else value).encode() if raw is None else raw)
    return result


class HostClientTests(unittest.IsolatedAsyncioTestCase):
    def setUp(self):
        self.guards = ExitStack()
        self.addCleanup(self.guards.close)
        self.guards.enter_context(patch.dict(os.environ, {
            'UNREAD_TOKEN_REF': CANARY, 'SALESFORCE_USERNAME': CANARY,
            'SALESFORCE_PASSWORD': CANARY, 'SALESFORCE_SECURITY_TOKEN': CANARY,
            'HTTPS_PROXY': 'http://ambient.invalid', 'NETRC': '/ambient.invalid',
        }, clear=True))
        for target in ('socket.socket.connect', 'socket.socket.connect_ex', 'socket.getaddrinfo',
                       'requests.sessions.get_netrc_auth', 'requests.get', 'requests.post'):
            self.guards.enter_context(patch(target, side_effect=AssertionError('unowned I/O forbidden')))
        self.configuration = validate_configuration('salesforce', operations=['get_record'], config={
            'api_key_env': 'UNREAD_TOKEN_REF', 'timeout': 2, 'instance_url': ORIGIN})
        self.session = ServiceSession()
        self.session.trust_env = False
        self.session.max_redirects = 0
        self.session.auth = Auth()
        self.addCleanup(self.session.close)
        self.calls, self.responses = [], []

    def profile(self, session=None):
        profile = build_salesforce_profile_from_client(self.configuration, self.session if session is None else session)
        self.addCleanup(profile.close)
        return profile

    def send(self, _adapter, request, **kwargs):
        self.calls.append((request, kwargs))
        result = response(request)
        message = Message(); message['Set-Cookie'] = 'selected-cookie=from-provider; Path=/; Secure'
        result.raw._original_response = SimpleNamespace(msg=message)
        self.responses.append(result)
        return result

    async def get(self, client, **kwargs):
        return await client.call_tool('get_record', {'sobject': 'Account', 'record_id': RECORD, **kwargs})

    async def test_actual_donor_rotating_auth_and_borrowed_resource_ownership(self):
        original = (self.session.auth, dict(self.session.adapters), self.session.hooks.copy())
        with patch('meerkat_toolkit.adapters.agno_profiles._explicit_key', side_effect=AssertionError('no token read')):
            profile = self.profile()
        from agno.tools.salesforce import SalesforceTools
        owner = inspect.unwrap(profile.adapter._bindings['get_record'].entrypoint).__self__
        self.assertIs(type(owner).get_record, SalesforceTools.get_record)
        self.assertEqual(owner.sf.session_id, '')
        with patch.object(requests.adapters.HTTPAdapter, 'send', lambda a,r,**kw: self.send(a,r,**kw)), patch.object(self.session, 'close', wraps=self.session.close) as closed:
            async with Client(create_server(profile)) as client:
                first = await self.get(client)
                self.session.auth.token = 'second-host-value'
                second = await self.get(client)
                self.assertFalse(first.is_error); self.assertFalse(second.is_error)
                self.assertIn('Selected record', first.content[0].text)
            profile.close()
            closed.assert_not_called()
        self.assertEqual([r.headers['Authorization'] for r, _ in self.calls], ['Bearer first-host-value', 'Bearer second-host-value'])
        self.assertEqual(len(self.session.auth.calls), 2)
        self.assertEqual(self.calls[1][0].headers['Cookie'], 'selected-cookie=from-provider')
        for request, kwargs in self.calls:
            self.assertEqual(request.method, 'GET')
            self.assertEqual(request.url, ORIGIN + '/services/data/v59.0/sobjects/Account/' + RECORD)
            self.assertIsNone(request.body)
            self.assertFalse(kwargs.get('proxies'))
        self.assertEqual(original, (self.session.auth, dict(self.session.adapters), self.session.hooks.copy()))
        self.assertTrue(all(r.raw.closed for r in self.responses))

    def test_invalid_preparation_refuses_without_mutation_or_environment_read(self):
        changes = [lambda s: setattr(s, 'trust_env', True), lambda s: setattr(s, 'verify', False),
            lambda s: s.params.update({'fields': 'Name'}), lambda s: s.proxies.update({'https': 'http://proxy.invalid'}),
            lambda s: setattr(s, 'resolve_redirects', lambda *a, **kw: iter(())), lambda s: setattr(s, 'auth', None),
            lambda s: s.headers.update({'Authorization': 'Bearer stale'}),
            lambda s: setattr(s.get_adapter(ORIGIN), 'max_retries', Retry(total=1))]
        for change in changes:
            with self.subTest(change=change):
                session = ServiceSession(); session.trust_env = False; session.max_redirects = 0; session.auth = Auth()
                try:
                    change(session)
                    before = repr((session.auth, session.verify, session.trust_env, session.params, session.proxies, session.cookies, session.headers, session.adapters))
                    with self.assertRaises(ProfileConfigError): self.profile(session)
                    self.assertEqual(before, repr((session.auth, session.verify, session.trust_env, session.params, session.proxies, session.cookies, session.headers, session.adapters)))
                finally: session.close()
        with requests.Session() as plain, self.assertRaises(ProfileConfigError): self.profile(plain)
        with self.assertRaises(ProfileConfigError): build_salesforce_profile_from_client(None, self.session)
        other = validate_configuration('trello', operations=['list_boards'], config={'api_key_env':'UNREAD_TOKEN_REF','token_env':'OTHER','timeout':2})
        with self.assertRaises(ProfileConfigError): build_salesforce_profile_from_client(other, self.session)

    async def test_runtime_mutation_and_auth_failure_never_use_a_fallback(self):
        profile = self.profile()
        with patch.object(requests.adapters.HTTPAdapter, 'send', lambda a,r,**kw: self.send(a,r,**kw)), redirect_stdout(io.StringIO()), redirect_stderr(io.StringIO()):
            async with Client(create_server(profile)) as client:
                self.session.params['bad'] = 'query'
                self.assertTrue((await self.get(client)).is_error)
                self.assertEqual(self.session.auth.calls, [])
                self.session.params.clear()
                self.session.auth.denied = True
                result = await self.get(client)
                self.assertTrue(result.is_error)
                self.assertTrue(result.structured_content['error']['entered'])
                self.assertNotIn(CANARY, str(result))
                self.assertEqual(self.calls, [])
                self.session.auth.denied = False
                self.assertFalse((await self.get(client)).is_error)
        self.assertEqual(len(self.calls), 1)

    async def test_route_schema_and_record_identity_are_preserved(self):
        profile = self.profile()
        with patch.object(requests.adapters.HTTPAdapter, 'send', lambda a,r,**kw: self.send(a,r,**kw)):
            async with Client(create_server(profile)) as client:
                for bad in ({'fields':'Name'}, {'record_id': RECORD+'\n'}, {'sobject':'../Account'}):
                    self.assertTrue((await self.get(client, **bad)).is_error)
                self.assertEqual(self.calls, [])
                owner = inspect.unwrap(profile.adapter._bindings['get_record'].entrypoint).__self__
                for method, url in [('GET','https://foreign.invalid/a'), ('POST',ORIGIN+'/services/data/v59.0/sobjects/Account/'+RECORD), ('GET',ORIGIN+'/services/data/v59.0/sobjects/Account/'+RECORD+'?fields=Name')]:
                    with self.assertRaises(ValueError): owner.sf.session.request(method,url)
                self.assertEqual(self.calls, [])
                for value, success in [({'Id':RECORD+'AAA'}, True), ({'Id':'002'+RECORD[3:]+'AAA'}, False), ({}, False)]:
                    with patch.object(requests.adapters.HTTPAdapter, 'send', lambda _s,r,**_k: response(r,value=value)):
                        self.assertEqual(not (await self.get(client)).is_error,success)

    async def test_response_guards_preserve_hook_order_and_all_cleanup(self):
        profile = self.profile()
        seen=[]
        def first(r, **kwargs): seen.append(('first', r.status_code))
        def second(r, **kwargs): seen.append(('second', r.status_code))
        hooks=[first,second]; self.session.hooks['response'][:]=hooks
        async with Client(create_server(profile)) as client:
            with patch.object(requests.adapters.HTTPAdapter, 'send', lambda a,r,**kw: self.send(a,r,**kw)):
                self.assertFalse((await self.get(client)).is_error)
            self.assertEqual(seen,[('first',200),('second',200)])
            seen.clear()
            for status, raw, length in [(307,b'x'*2_000_000,None), (401,CANARY.encode(),None), (200,b'{}',1_048_577)]:
                captured=[]
                def send(_s,r,**_k):
                    value=response(r,status=status,raw=raw,length=length); captured.append(value); return value
                with patch.object(requests.adapters.HTTPAdapter, 'send', send):
                    self.assertTrue((await self.get(client)).is_error)
                self.assertEqual(len(captured),1); self.assertEqual(captured[0].raw.reads,0)
                self.assertTrue(captured[0].raw.closed); self.assertEqual(seen,[])
            for raw in (b'x' * 1_048_577, b'{invalid', b'\xff'):
                captured=[]
                def body_failure(_s,r,**_k):
                    value=response(r,raw=raw); captured.append(value); return value
                with patch.object(requests.adapters.HTTPAdapter, 'send', body_failure):
                    self.assertTrue((await self.get(client)).is_error)
                self.assertEqual(len(captured),1); self.assertTrue(captured[0].raw.closed)
            def fail(r, **kwargs): raise ValueError(CANARY)
            self.session.hooks['response'][:]=[fail]
            with patch.object(requests.adapters.HTTPAdapter, 'send', lambda a,r,**kw: self.send(a,r,**kw)):
                self.assertTrue((await self.get(client)).is_error)
                self.assertTrue(self.responses[-1].raw.closed)
            replacement=response(None,value={'Id':RECORD,'Name':'Host replacement'})
            self.session.hooks['response'][:]=[lambda r,**kwargs: replacement]
            with patch.object(requests.adapters.HTTPAdapter, 'send', lambda a,r,**kw: self.send(a,r,**kw)):
                self.assertFalse((await self.get(client)).is_error)
            self.assertTrue(self.responses[-1].raw.closed); self.assertTrue(replacement.raw.closed)
            self.session.hooks['response'][:]=[]
            captured=[]
            def close_failure(_s,r,**_k):
                value=response(r); value.close=lambda: (_ for _ in ()).throw(ValueError(CANARY)); captured.append(value); return value
            with patch.object(requests.adapters.HTTPAdapter, 'send', close_failure):
                result=await self.get(client); self.assertTrue(result.is_error); self.assertNotIn(CANARY,str(result))
            captured[0].raw.close()
        self.session.hooks['response'][:]=hooks
        self.assertEqual(self.session.hooks['response'],hooks)

    async def test_cancel_joins_worker_and_host_closes_afterward(self):
        profile=self.profile(); entered=threading.Event(); release=threading.Event(); exited=threading.Event()
        def send(_s,r,**_k):
            entered.set()
            if not release.wait(3): raise AssertionError('test release missing')
            exited.set(); return response(r)
        try:
            with patch.object(requests.adapters.HTTPAdapter,'send',send), patch.object(self.session,'close',wraps=self.session.close) as closed:
                async with Client(create_server(profile)) as client:
                    task=asyncio.create_task(self.get(client))
                    self.assertTrue(await asyncio.to_thread(entered.wait,2))
                    task.cancel()
                    await asyncio.sleep(0)
                    closed.assert_not_called(); self.assertFalse(exited.is_set())
                    release.set()
                    try: await task
                    except asyncio.CancelledError: pass
                self.assertTrue(exited.is_set()); profile.close(); closed.assert_not_called()
                self.session.close(); closed.assert_called_once()
        finally: release.set()

    def test_owned_path_still_captures_selected_token_and_closes_owned_session(self):
        with patch.object(ServiceSession,'close',autospec=True) as close:
            profile=build_profile('salesforce',operations=['get_record'],config=self.configuration.config)
            owner=inspect.unwrap(profile.adapter._bindings['get_record'].entrypoint).__self__
            self.assertEqual(owner.sf.session_id,CANARY)
            profile.close(); close.assert_called_once()


if __name__ == '__main__': unittest.main()
