"""Native-required source composition with real MCP and controlled HTTP owners.

These fixtures exercise fields-v1 correlation, not native admission or grants.
"""
import asyncio
from contextlib import ExitStack
import io
import importlib.util
import json
import logging
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
from types import SimpleNamespace
import unittest
from unittest.mock import patch

import anyio
import httpx
from mcp import Client
import requests

from meerkat_toolkit.adapters import services_config, services_mcp, agno_mcp, agno_profiles
from meerkat_toolkit.adapters._connector_call import (
    ConnectorBinding, ConnectorCall, CredentialRefused, BINDING_ENV, KEY,
    private_diagnostics, protect_server,
)
from meerkat_toolkit.adapters.agno_transports import ServiceSession

PRIVATE = 'private-native-credential-canary'
ORIGIN = 'https://selected.my.salesforce.com'
RECORD = '001000000000001'
ACCOUNT = {'issuer': 'fixture-issuer', 'strategy_id': 'fixture-verification', 'subject': 'fixture-subject'}


def startup():
    return {'version': 1, 'operations': {name: {'account': dict(ACCOUNT), 'destination_origin': origin}
        for name, origin in [('slack_auth_test', 'https://slack.com'),
                             ('gmail_read_text_part', 'https://gmail.googleapis.com'),
                             ('get_record', ORIGIN), ('probe', ORIGIN)]}}


def meta(origin, token=PRIVATE):
    return {KEY: {'authorization': 'Bearer ' + token, 'destination_origin': origin,
                  'account': dict(ACCOUNT), 'scopes': [PRIVATE], 'expires_at': '2000-01-01T00:00:00+00:00'}}


def service(profile='slack', operations=None):
    config = {'token_env': 'DO_NOT_RESOLVE', 'timeout_seconds': 3, 'max_response_bytes': 65536}
    if profile == 'gmail':
        config.update(user_id='me', max_message_bytes=4096)
    return services_config.validate_configuration({'profile': profile,
        'operations': operations or (['auth_test'] if profile == 'slack' else ['get_profile']), 'config': config})


def salesforce():
    return agno_profiles.validate_configuration('salesforce', operations=['get_record'],
        config={'api_key_env': 'DO_NOT_RESOLVE', 'instance_url': ORIGIN, 'timeout': 3})


class Raw(io.BytesIO):
    def release_conn(self): self.close()


def response(request):
    value = requests.Response(); value.status_code = 200; value.request = request; value.url = request.url
    value.raw = Raw(json.dumps({'Id': RECORD, 'Name': 'Selected record'}).encode())
    return value


class NativeTests(unittest.IsolatedAsyncioTestCase):
    def setUp(self):
        self.guards = ExitStack(); self.addCleanup(self.guards.close)
        self.guards.enter_context(patch.dict(os.environ, {'DO_NOT_RESOLVE': PRIVATE,
            BINDING_ENV: json.dumps(startup()),
            'HTTPS_PROXY': 'http://ambient.invalid', 'NETRC': '/ambient.invalid',
            'SSL_CERT_FILE': '/ambient.invalid', 'SSL_CERT_DIR': '/ambient.invalid'}, clear=True))
        for target in ('socket.socket.connect', 'socket.socket.connect_ex', 'socket.getaddrinfo',
                       'requests.sessions.get_netrc_auth'):
            self.guards.enter_context(patch(target, side_effect=AssertionError('unowned I/O forbidden')))
        self.guards.enter_context(patch.object(services_mcp, 'selected_token', side_effect=AssertionError('no startup token')))
        self.guards.enter_context(patch.object(agno_profiles, '_explicit_key', side_effect=AssertionError('no startup token')))

    async def test_startup_binding_is_closed_and_refuses_before_client_construction(self):
        malformed = [None, PRIVATE, 'null', '{}', '{"version":1,"version":1,"operations":{}}',
                     '{"version":1,"operations":{},"private":"' + PRIVATE + '"}']
        for version in (True, 0, 2, '1'):
            value = startup(); value['version'] = version
            malformed.append(json.dumps(value))
        for path, replacement in ((('operations',), []),
                                  (('operations', 'get_record'), None),
                                  (('operations', 'get_record', 'account'), {'subject': PRIVATE}),
                                  (('operations', 'get_record', 'account', 'issuer'), 1),
                                  (('operations', 'get_record', 'destination_origin'), ORIGIN + '/record'),
                                  (('operations', 'get_record', 'destination_origin'), ORIGIN + ':0')):
            value = startup(); item = value
            for key in path[:-1]: item = item[key]
            item[path[-1]] = replacement
            malformed.append(json.dumps(value))
        value = startup(); value['operations']['get_record']['account']['unknown'] = PRIVATE
        malformed.append(json.dumps(value))
        value = json.dumps(startup())
        malformed.append(value.replace('"subject": "fixture-subject"',
            '"subject":"fixture-subject","subject":"' + PRIVATE + '"', 1))
        with patch.object(ConnectorCall, 'httpx_client') as http_client, \
             patch.object(ConnectorCall, 'requests_session') as requests_client:
            for encoded in malformed:
                with self.subTest(case=malformed.index(encoded)):
                    if encoded is None: os.environ.pop(BINDING_ENV, None)
                    else: os.environ[BINDING_ENV] = encoded
                    for factory, configuration in ((services_mcp.create_native_server, service()),
                                                    (agno_mcp.create_native_server, salesforce())):
                        with self.assertRaises(CredentialRefused) as caught:
                            factory(configuration)
                        self.assertEqual(str(caught.exception), 'Connector credential refused')
                        self.assertIsNone(caught.exception.__context__)
            http_client.assert_not_called(); requests_client.assert_not_called()

    async def test_actual_handlers_refuse_mismatched_closed_call_binding_before_clients(self):
        for kind, factory, configuration, name, arguments, origin, error_field, error_kind in (
            ('services', services_mcp.create_native_server, service(), 'slack_auth_test', {},
             'https://slack.com', 'operation_invoked', 'permission'),
            ('agno', agno_mcp.create_native_server, salesforce(), 'get_record',
             {'sobject': 'Account', 'record_id': RECORD}, ORIGIN, 'entered', 'invalid_arguments'),
        ):
            cases = []
            for field in ACCOUNT:
                value = meta(origin); value[KEY]['account'][field] = PRIVATE
                cases.append(value)
                value = meta(origin); value[KEY]['account'][field] = False
                cases.append(value)
            for field in ('authorization', 'destination_origin', 'account', 'scopes'):
                value = meta(origin); del value[KEY][field]
                cases.append(value)
            for field, value in (('scopes', PRIVATE), ('scopes', [False]), ('expires_at', 1),
                                 ('destination_origin', origin + ':443'),
                                 ('destination_origin', origin + '/')):
                supplied = meta(origin); supplied[KEY][field] = value
                cases.append(supplied)
            value = meta(origin); value[KEY]['account']['unexpected'] = PRIVATE
            cases.append(value)
            value = meta(origin); value[KEY]['unexpected'] = PRIVATE
            cases.append(value)
            with patch.object(ConnectorCall, 'httpx_client') as http_client, \
                 patch.object(ConnectorCall, 'requests_session') as requests_client:
                async with Client(factory(configuration)) as client:
                    for index, supplied in enumerate(cases):
                        with self.subTest(kind=kind, case=index):
                            result = await client.call_tool(name, arguments, meta=supplied)
                            self.assertTrue(result.is_error)
                            self.assertEqual(result.structured_content['error']['kind'], error_kind)
                            self.assertFalse(result.structured_content['error'][error_field])
                            self.assertNotIn(PRIVATE, str(result))
                http_client.assert_not_called(); requests_client.assert_not_called()

    async def test_actual_raw_operation_and_selected_profile_cannot_be_substituted(self):
        value = startup()
        value['operations']['auth_test'] = value['operations'].pop('slack_auth_test')
        os.environ[BINDING_ENV] = json.dumps(value)
        with patch.object(ConnectorCall, 'httpx_client') as http_client, \
             patch.object(ConnectorCall, 'requests_session') as requests_client:
            async with Client(services_mcp.create_native_server(service())) as client:
                # The offered raw name does not inherit a binding for its bare alias.
                result = await client.call_tool('slack_auth_test', {}, meta=meta('https://slack.com'))
                self.assertEqual(result.structured_content['error']['kind'], 'permission')
                self.assertFalse(result.structured_content['error']['operation_invoked'])
                for name, origin in (('auth_test', 'https://slack.com'), ('get_record', ORIGIN)):
                    result = await client.call_tool(name, {}, meta=meta(origin))
                    self.assertEqual(result.structured_content['error']['kind'], 'unknown_function')
                    self.assertFalse(result.structured_content['error']['operation_invoked'])
            value = startup(); value['operations'].pop('get_record')
            os.environ[BINDING_ENV] = json.dumps(value)
            async with Client(agno_mcp.create_native_server(salesforce())) as client:
                result = await client.call_tool('get_record', {'sobject': 'Account', 'record_id': RECORD}, meta=meta(ORIGIN))
                self.assertEqual(result.structured_content['error']['kind'], 'invalid_arguments')
                self.assertFalse(result.structured_content['error']['entered'])
                result = await client.call_tool('slack_auth_test', {}, meta=meta('https://slack.com'))
                self.assertEqual(result.structured_content['error']['kind'], 'unknown_function')
                self.assertFalse(result.structured_content['error']['entered'])
            http_client.assert_not_called(); requests_client.assert_not_called()

    async def test_retained_startup_and_selected_configuration_ignore_later_mutation(self):
        http_calls, request_calls = [], []
        async def handle(_transport, request):
            http_calls.append(request)
            return httpx.Response(200, json={'ok': True, 'team_id': 'T1', 'user_id': 'U1'}, request=request)
        def send(_adapter, request, **_kwargs): request_calls.append(request); return response(request)
        selected = salesforce()
        service_server = services_mcp.create_native_server(service())
        agno_server = agno_mcp.create_native_server(selected)
        # This dict belongs to the caller. The server retained a validated copy.
        selected.config['instance_url'] = 'https://foreign.invalid'
        selected.config['timeout'] = 120
        replacement = startup()
        for expected in replacement['operations'].values():
            expected['account']['subject'] = 'replacement-subject'
        os.environ[BINDING_ENV] = json.dumps(replacement)
        with patch.object(httpx.AsyncHTTPTransport, 'handle_async_request', handle), \
             patch.object(requests.adapters.HTTPAdapter, 'send', send):
            for server, name, arguments, origin, field in (
                (service_server, 'slack_auth_test', {}, 'https://slack.com', 'operation_invoked'),
                (agno_server, 'get_record', {'sobject': 'Account', 'record_id': RECORD}, ORIGIN, 'entered'),
            ):
                async with Client(server) as client:
                    supplied = meta(origin); supplied[KEY]['account']['subject'] = 'replacement-subject'
                    result = await client.call_tool(name, arguments, meta=supplied)
                    self.assertTrue(result.is_error); self.assertFalse(result.structured_content['error'][field])
                    result = await client.call_tool(name, arguments, meta=meta(origin))
                    self.assertFalse(result.is_error)
        self.assertEqual(len(http_calls), 1); self.assertEqual(len(request_calls), 1)
        self.assertEqual(http_calls[0].url.scheme, 'https')
        self.assertEqual(http_calls[0].url.host, 'slack.com')
        self.assertIsNone(http_calls[0].url.port)
        self.assertEqual(http_calls[0].url.path, '/api/auth.test')
        self.assertEqual(http_calls[0].url.query, b'')
        self.assertTrue(request_calls[0].url.startswith(ORIGIN + '/services/data/'))

    async def test_native_context_types_are_validated_without_scope_or_freshness_decisions(self):
        encoded = json.dumps(startup())
        binding = ConnectorBinding(encoded)
        self.assertNotIn('fixture-subject', repr(binding))
        for expiry in ('omitted', None, '2000-01-01T00:00:00+00:00'):
            supplied = meta(ORIGIN); supplied[KEY]['scopes'] = []
            if expiry == 'omitted': del supplied[KEY]['expires_at']
            else: supplied[KEY]['expires_at'] = expiry
            call = ConnectorCall(SimpleNamespace(meta=supplied), binding, 'probe', ORIGIN)
            self.assertFalse(call.request_entered)
            self.assertNotIn(PRIVATE, repr(call))
            supplied[KEY]['authorization'] = 'Bearer changed-after-construction'
            supplied[KEY]['account']['subject'] = 'changed-after-construction'
            self.assertEqual(call._authorization, 'Bearer ' + PRIVATE)

    async def test_services_catalog_missing_binding_rotation_and_client_close(self):
        calls, closes = [], []
        original = httpx.AsyncClient.aclose
        async def close(client):
            closes.append(client); await original(client)
        async def handle(_transport, request):
            calls.append(request)
            return httpx.Response(200, json={'ok': True, 'team_id': 'T1', 'user_id': 'U1'}, request=request)
        with patch.object(httpx.AsyncHTTPTransport, 'handle_async_request', handle), patch.object(httpx.AsyncClient, 'aclose', close):
            server = services_mcp.create_native_server(service())
            async with Client(server) as client:
                self.assertEqual([t.name for t in (await client.list_tools()).tools], ['slack_auth_test'])
                for value in (None, {}, meta('https://foreign.invalid'), {KEY: {'authorization': 'Bearer x'}}):
                    result = await client.call_tool('slack_auth_test', {}, meta=value)
                    self.assertTrue(result.is_error); self.assertFalse(result.structured_content['error']['operation_invoked'])
                self.assertEqual(calls, [])
                for token in ('native-first', 'native-second'):
                    result = await client.call_tool('slack_auth_test', {}, meta=meta('https://slack.com', token))
                    self.assertFalse(result.is_error); self.assertNotIn(PRIVATE, str(result))
                refused = await client.call_tool('slack_auth_test', {'Authorization': PRIVATE}, meta=meta('https://slack.com'))
                self.assertTrue(refused.is_error)
        self.assertEqual([r.headers['Authorization'] for r in calls], ['Bearer native-first', 'Bearer native-second'])
        self.assertEqual(len(closes), 3); self.assertEqual(len({id(c) for c in closes}), 3)
        self.assertTrue(all(c.is_closed and 'Authorization' not in c.headers for c in closes))

    async def test_salesforce_catalog_missing_binding_rotation_and_worker_close(self):
        calls, closes = [], []
        original = ServiceSession.close
        def close(session): closes.append((session, threading.get_ident())); return original(session)
        def send(_adapter, request, **_kwargs): calls.append(request); return response(request)
        with patch.object(requests.adapters.HTTPAdapter, 'send', send), patch.object(ServiceSession, 'close', close):
            server = agno_mcp.create_native_server(salesforce())
            closes.clear()  # The inert catalog's credential-free session already closed.
            async with Client(server) as client:
                self.assertEqual([t.name for t in (await client.list_tools()).tools], ['get_record'])
                for value in (None, meta('https://foreign.invalid')):
                    result = await client.call_tool('get_record', {'sobject': 'Account', 'record_id': RECORD}, meta=value)
                    self.assertTrue(result.is_error); self.assertFalse(result.structured_content['error']['entered'])
                self.assertEqual(calls, [])
                for token in ('native-first', 'native-second'):
                    result = await client.call_tool('get_record', {'sobject': 'Account', 'record_id': RECORD}, meta=meta(ORIGIN, token))
                    self.assertFalse(result.is_error); self.assertNotIn(PRIVATE, str(result))
        self.assertEqual([r.headers['Authorization'] for r in calls], ['Bearer native-first', 'Bearer native-second'])
        self.assertEqual(len(closes), 2); self.assertIsNot(closes[0][0], closes[1][0])
        self.assertTrue(all(thread != threading.get_ident() for _, thread in closes))
        other = agno_profiles.validate_configuration('hackernews', operations=['get_user_details'], config={'timeout': 3})
        with self.assertRaises(agno_profiles.ProfileConfigError): agno_mcp.create_native_server(other)

    async def test_actual_request_owner_origin_refusal_and_monotonic_entry(self):
        http_calls = []; request_calls = []
        binding = ConnectorBinding.from_environment()
        with self.assertRaises(CredentialRefused):
            ConnectorCall(SimpleNamespace(meta=meta(ORIGIN + ':0')), binding, 'probe', ORIGIN)
        async def handle(_transport, request):
            http_calls.append(request); return httpx.Response(200, json={}, request=request)
        def send(_adapter, request, **_kwargs): request_calls.append(request); return response(request)
        for kind in ('httpx', 'requests'):
            call = ConnectorCall(SimpleNamespace(meta=meta(ORIGIN)), binding, 'probe', ORIGIN)
            with self.assertRaises(CredentialRefused): call.check_url(ORIGIN + ':0/')
            with patch.object(httpx.AsyncHTTPTransport, 'handle_async_request', handle), patch.object(requests.adapters.HTTPAdapter, 'send', send):
                if kind == 'httpx':
                    async with call.httpx_client(ORIGIN) as client:
                        with self.assertRaises(CredentialRefused): await client.get('https://foreign.invalid/')
                        with self.assertRaises(CredentialRefused): await client.get(ORIGIN + ':0/')
                        self.assertFalse(call.request_entered)
                        await client.get('/selected')
                        self.assertTrue(call.request_entered)
                        with self.assertRaises(CredentialRefused): await client.get('https://foreign.invalid/')
                        self.assertTrue(call.request_entered)
                else:
                    with call.requests_session() as client:
                        with self.assertRaises(CredentialRefused): client.get('https://foreign.invalid/')
                        self.assertFalse(call.request_entered)
                        client.get(ORIGIN + '/selected').close()
                        self.assertTrue(call.request_entered)
                        with self.assertRaises(CredentialRefused): client.get('https://foreign.invalid/')
                        self.assertTrue(call.request_entered)
        self.assertEqual(len(http_calls), 1); self.assertEqual(len(request_calls), 1)

    async def test_gmail_bounded_sequence_reuses_call_auth_and_preserves_later_failure(self):
        calls = []
        async def handle(_transport, request):
            calls.append(request)
            if request.url.path.endswith('/messages/selected'):
                value = {'id': 'selected', 'threadId': 'thread', 'payload': {'partId': '', 'mimeType': 'text/plain',
                    'headers': [], 'body': {'size': 4, 'attachmentId': 'body'}}}
                return httpx.Response(200, json=value, request=request)
            return httpx.Response(503, json={}, request=request)
        with patch.object(httpx.AsyncHTTPTransport, 'handle_async_request', handle):
            async with Client(services_mcp.create_native_server(service('gmail', ['read_text_part']))) as client:
                result = await client.call_tool('gmail_read_text_part', {'message_id': 'selected', 'part_id': ''},
                    meta=meta('https://gmail.googleapis.com'))
                self.assertTrue(result.is_error); self.assertTrue(result.structured_content['error']['operation_invoked'])
        self.assertEqual(len(calls), 2)
        self.assertEqual([r.url.path for r in calls], ['/gmail/v1/users/me/messages/selected', '/gmail/v1/users/me/messages/selected/attachments/body'])
        self.assertTrue(all(r.headers['Authorization'] == 'Bearer ' + PRIVATE for r in calls))

    async def test_salesforce_cancel_joins_worker_before_per_call_client_close(self):
        entered = threading.Event(); release = threading.Event(); exited = threading.Event(); closed = []
        original = ServiceSession.close
        def send(_adapter, request, **_kwargs):
            entered.set()
            if not release.wait(3): raise AssertionError('missing fixture release')
            exited.set(); return response(request)
        def close(session): closed.append(exited.is_set()); return original(session)
        server = agno_mcp.create_native_server(salesforce())
        try:
            with patch.object(requests.adapters.HTTPAdapter, 'send', send), patch.object(ServiceSession, 'close', close):
                async with Client(server) as client:
                    task = asyncio.create_task(client.call_tool('get_record', {'sobject': 'Account', 'record_id': RECORD}, meta=meta(ORIGIN)))
                    self.assertTrue(await asyncio.to_thread(entered.wait, 2))
                    task.cancel(); await asyncio.sleep(0)
                    self.assertEqual(closed, []); self.assertFalse(exited.is_set())
                    release.set()
                    try: await task
                    except asyncio.CancelledError: pass
                self.assertEqual(closed, [True])
        finally: release.set()

    async def test_services_cancel_retains_response_client_and_serial_admission(self):
        closing, release = asyncio.Event(), asyncio.Event()
        events, clients, requests_seen, gates = [], [], [], []
        original_close, original_gate = httpx.AsyncClient.aclose, services_mcp.CallAdmission

        def gate(lock):
            value = original_gate(lock); gates.append(value); return value

        async def close(client):
            clients.append(client); events.append('client_closed')
            await original_close(client)

        async def handle(_transport, request):
            index = len(requests_seen); requests_seen.append(request)
            events.append('request_' + str(index))

            class Stream(httpx.AsyncByteStream):
                async def __aiter__(self):
                    yield b'{"ok":true,"team_id":"T1","user_id":"U1"}'

                async def aclose(self):
                    if index == 0:
                        closing.set()
                        await release.wait()
                    events.append('response_closed_' + str(index))

            return httpx.Response(200, headers={'Content-Type': 'application/json'},
                                  stream=Stream(), request=request)

        with patch.object(services_mcp, 'CallAdmission', side_effect=gate), \
             patch.object(httpx.AsyncClient, 'aclose', close), \
             patch.object(httpx.AsyncHTTPTransport, 'handle_async_request', handle):
            server = services_mcp.create_native_server(service())
            async with Client(server) as client:
                first = asyncio.create_task(client.call_tool('slack_auth_test', {},
                    meta=meta('https://slack.com', 'first')))
                queued = None
                try:
                    await asyncio.wait_for(closing.wait(), 3)
                    first.cancel()
                    queued = asyncio.create_task(client.call_tool('slack_auth_test', {},
                        meta=meta('https://slack.com', 'second')))
                    async with asyncio.timeout(3):
                        while gates[0]._admitted != 2:
                            await asyncio.sleep(0)
                    self.assertEqual(events, ['request_0'])
                    self.assertEqual(clients, [])
                    self.assertFalse(queued.done())
                finally:
                    release.set()
                    await asyncio.wait_for(asyncio.gather(first, *([queued] if queued else []),
                        return_exceptions=True), 5)
                self.assertTrue(first.cancelled())
                self.assertFalse(queued.result().is_error)
        self.assertEqual(events, ['request_0', 'response_closed_0', 'client_closed',
                                  'request_1', 'response_closed_1', 'client_closed'])
        self.assertEqual([request.headers['Authorization'] for request in requests_seen],
                         ['Bearer first', 'Bearer second'])
        self.assertEqual(len({id(client) for client in clients}), 2)
        self.assertTrue(all(client.is_closed for client in clients))

    async def test_cli_mode_is_explicit_and_unrelated_agno_refuses(self):
        with patch.object(services_mcp, 'load_configuration', return_value=service()), patch('anyio.run') as run:
            self.assertEqual(services_mcp.main(['serve-native', '--config', 'selected.json']), 0)
            self.assertIs(run.call_args.args[0], services_mcp.serve_native_stdio)
            self.assertEqual(services_mcp.main(['serve', '--config', 'selected.json']), 0)
            self.assertIs(run.call_args.args[0], services_mcp.serve_stdio)
        with patch.object(agno_mcp, 'load_configuration', return_value=salesforce()), patch('anyio.run') as run:
            self.assertEqual(agno_mcp.main(['serve-native', '--config', 'selected.json']), 0)
            self.assertIs(run.call_args.args[0], agno_mcp.serve_native_stdio)
            self.assertEqual(agno_mcp.main(['serve', '--config', 'selected.json']), 0)
            self.assertIs(run.call_args.args[0], agno_mcp.serve_stdio)

    async def test_independent_stage_namespaces_admit_credentials_and_refuse_binding(self):
        root = Path(__file__).resolve().parents[3]
        helper = root / 'sdk/python/src/meerkat_toolkit/adapters/_connector_call.py'
        script = '''
import asyncio, importlib, io, json, os, sys
from contextlib import ExitStack
from pathlib import Path
from unittest.mock import patch
class BlockCombined:
    def find_spec(self, fullname, path=None, target=None):
        if fullname == 'meerkat_toolkit' or fullname.startswith('meerkat_toolkit.'):
            raise AssertionError('combined SDK fallback forbidden')
sys.meta_path.insert(0, BlockCombined())
from mcp import Client
import httpx, requests
namespace = sys.argv[1]
stage_root = Path(sys.argv[2]).resolve()
private = 'private-native-credential-canary'
account = {'issuer': 'fixture-issuer', 'strategy_id': 'fixture-verification', 'subject': 'fixture-subject'}
requests_seen, bodies, created, closed = [], [], [], []
if namespace == 'meerkat_toolkit_services':
    module = importlib.import_module(namespace + '.adapters.services_mcp')
    configuration = importlib.import_module(namespace + '.adapters.services_config').validate_configuration({
        'profile': 'slack', 'operations': ['auth_test'], 'config': {
            'token_env': 'DO_NOT_RESOLVE', 'timeout_seconds': 3, 'max_response_bytes': 65536}})
    tool, arguments = 'slack_auth_test', {}
    helper = importlib.import_module(namespace + '.adapters._connector_call')
    origin, error_field, error_kind = 'https://slack.com', 'operation_invoked', 'permission'
    credential_owner, credential_function = module, 'selected_token'
    client_type = httpx.AsyncClient
else:
    module = importlib.import_module(namespace + '.agno_mcp')
    configuration = importlib.import_module(namespace + '.agno_profiles').validate_configuration(
        'salesforce', operations=['get_record'], config={
            'api_key_env': 'DO_NOT_RESOLVE', 'timeout': 3, 'instance_url': 'https://selected.my.salesforce.com'})
    tool, arguments = 'get_record', {'sobject': 'Account', 'record_id': '001000000000001'}
    helper = importlib.import_module(namespace + '._connector_call')
    origin, error_field, error_kind = 'https://selected.my.salesforce.com', 'entered', 'invalid_arguments'
    credential_owner = importlib.import_module(namespace + '.agno_profiles')
    credential_function = '_explicit_key'
    client_type = importlib.import_module(namespace + '.agno_transports').ServiceSession

def metadata(token=private):
    return {helper.KEY: {'authorization': 'Bearer ' + token, 'destination_origin': origin,
        'account': dict(account), 'scopes': [], 'expires_at': None}}

original_init = client_type.__init__
def initialize(client, *args, **kwargs):
    original_init(client, *args, **kwargs)
    created.append(client)

class Stream(httpx.AsyncByteStream):
    def __init__(self): self.closed = False; bodies.append(self)
    async def __aiter__(self): yield b'{"ok":true,"team_id":"T1","user_id":"U1"}'
    async def aclose(self): self.closed = True

async def http_send(_transport, request):
    requests_seen.append(request)
    assert request.method == 'POST'
    assert (request.url.scheme, request.url.host, request.url.path, request.url.query) == (
        'https', 'slack.com', '/api/auth.test', b'')
    return httpx.Response(200, headers={'Content-Type': 'application/json'},
        stream=Stream(), request=request)

class Raw(io.BytesIO):
    def release_conn(self): self.close()

def requests_send(_adapter, request, **_kwargs):
    requests_seen.append(request)
    assert request.method == 'GET'
    assert request.url.startswith(origin + '/services/data/')
    assert request.url.endswith('/sobjects/Account/001000000000001')
    assert '?' not in request.url
    value = requests.Response(); value.status_code = 200
    value.request = request; value.url = request.url
    value.raw = Raw(b'{"Id":"001000000000001","Name":"Selected record"}')
    bodies.append(value.raw)
    return value

if namespace == 'meerkat_toolkit_services':
    original_close = client_type.aclose
    async def close(client):
        await original_close(client); closed.append(client)
    close_name = 'aclose'
else:
    original_close = client_type.close
    def close(client):
        original_close(client); closed.append(client)
    close_name = 'close'

async def main():
    with ExitStack() as guards:
        for target in ('socket.socket.connect', 'socket.socket.connect_ex', 'socket.getaddrinfo',
                       'requests.sessions.get_netrc_auth'):
            guards.enter_context(patch(target, side_effect=AssertionError('unowned I/O forbidden')))
        guards.enter_context(patch.object(credential_owner, credential_function,
            side_effect=AssertionError('legacy credential resolution forbidden')))
        guards.enter_context(patch.object(client_type, '__init__', initialize))
        guards.enter_context(patch.object(client_type, close_name, close))
        guards.enter_context(patch.object(httpx.AsyncHTTPTransport, 'handle_async_request', http_send))
        guards.enter_context(patch.object(requests.adapters.HTTPAdapter, 'send', requests_send))
        expected = os.environ.pop(helper.BINDING_ENV)
        try:
            try: module.create_native_server(configuration)
            except helper.CredentialRefused as error:
                assert str(error) == 'Connector credential refused'
                assert error.__context__ is None
            else: raise AssertionError('missing startup binding accepted')
            assert not requests_seen and not created and not closed
        finally: os.environ[helper.BINDING_ENV] = expected
        server = module.create_native_server(configuration)
        # Agno's inert, credential-free catalog session has already closed.
        assert len(created) == len(closed)
        created.clear(); closed.clear()
        async with Client(server) as client:
            assert [item.name for item in (await client.list_tools()).tools] == [tool]
            wrong_account = metadata(); wrong_account[helper.KEY]['account']['subject'] = 'another-account'
            wrong_origin = metadata(); wrong_origin[helper.KEY]['destination_origin'] = origin + ':443'
            for supplied in (None, wrong_account, wrong_origin):
                result = await client.call_tool(tool, arguments, meta=supplied)
                assert result.is_error
                assert result.structured_content['error']['kind'] == error_kind
                assert result.structured_content['error'][error_field] is False
                assert private not in str(result)
            assert not requests_seen and not created and not closed
            for token in (private + '-first', private + '-second'):
                result = await client.call_tool(tool, arguments, meta=metadata(token))
                assert not result.is_error
                assert private not in str(result)
        assert [request.headers['Authorization'] for request in requests_seen] == [
            'Bearer ' + private + '-first', 'Bearer ' + private + '-second']
        assert len(created) == len(closed) == len(bodies) == 2
        assert {id(client) for client in created} == {id(client) for client in closed}
        assert len({id(client) for client in created}) == 2
        assert all(body.closed for body in bodies)
        if namespace == 'meerkat_toolkit_services': assert all(client.is_closed for client in closed)
        for name, imported in list(sys.modules.items()):
            if name == namespace or name.startswith(namespace + '.'):
                assert Path(imported.__file__).resolve().is_relative_to(stage_root)
asyncio.run(main())
assert 'meerkat_toolkit' not in sys.modules
print('staged credential calls and binding refusals passed')
'''
        with tempfile.TemporaryDirectory() as directory:
            for family in ('services', 'agno'):
                spec = importlib.util.spec_from_file_location('stage_' + family, root / 'sdk' / family / 'stage.py')
                stage = importlib.util.module_from_spec(spec); spec.loader.exec_module(stage)
                destination = Path(directory) / family
                stage.stage(destination)
                namespace = 'meerkat_toolkit_' + family
                staged = destination / 'src' / namespace
                staged_helper = staged / ('adapters' if family == 'services' else '') / '_connector_call.py'
                self.assertEqual(staged_helper.read_bytes(), helper.read_bytes())
                environment = dict(os.environ, PYTHONPATH=str(destination / 'src'), PYTHONDONTWRITEBYTECODE='1')
                result = await asyncio.to_thread(subprocess.run, [sys.executable, '-c', script, namespace, str(destination / 'src')],
                    env=environment, capture_output=True, text=True, timeout=20, cwd=directory)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(result.stdout.strip(), 'staged credential calls and binding refusals passed')
                self.assertNotIn(PRIVATE, result.stderr)

    async def test_private_parser_errors_logs_preinit_frames_and_otel(self):
        from mcp.server import Server
        from mcp.server.stdio import stdio_server
        from mcp.server._otel import OpenTelemetryMiddleware
        async def fail(_context, _params): raise ValueError(PRIVATE)
        server = protect_server(Server('private-fixture', on_call_tool=fail))
        self.assertFalse(any(isinstance(m, OpenTelemetryMiddleware) for m in server.middleware))
        records = io.StringIO(); handler = logging.StreamHandler(records)
        loggers = [logging.getLogger(n) for n in ('mcp.server.runner', 'mcp.shared.jsonrpc_dispatcher')]
        original = [(log, log.level) for log in loggers]
        for log in loggers: log.addHandler(handler); log.setLevel(logging.DEBUG)
        try:
            with private_diagnostics(), patch.object(OpenTelemetryMiddleware, '__call__', side_effect=AssertionError('private tracing forbidden')):
                async with Client(server) as client:
                    with self.assertRaises(Exception) as caught:
                        await client.call_tool('probe', {}, meta=meta(ORIGIN))
                    self.assertNotIn(PRIVATE, str(caught.exception))
                frames = [json.dumps({'jsonrpc': '2.0', 'method': 'notifications/probe', 'params': {'_meta': meta(ORIGIN)}}) for _ in range(10)]
                frames.insert(0, '{' + PRIVATE)
                frames.append(json.dumps({'jsonrpc': '2.0', 'id': 'init', 'method': 'initialize', 'params': {
                    'protocolVersion': '2025-11-25', 'capabilities': {}, 'clientInfo': {'name': 'fixture', 'version': '1'}}}))
                output = io.StringIO()
                async with stdio_server(stdin=anyio.wrap_file(io.StringIO('\n'.join(frames)+'\n')),
                                        stdout=anyio.wrap_file(output)) as (read, write):
                    await server.run(read, write, server.create_initialization_options())
                self.assertNotIn(PRIVATE, output.getvalue())
            self.assertIn('Private connector MCP diagnostic', records.getvalue())
            self.assertNotIn(PRIVATE, records.getvalue())
        finally:
            for log, level in original: log.removeHandler(handler); log.setLevel(level)


    async def test_native_cleanup_failure_preserves_exact_success_and_error_results(self):
        binding = startup()
        binding['operations']['gmail_get_profile'] = {
            'account': dict(ACCOUNT), 'destination_origin': 'https://gmail.googleapis.com'}
        with patch.dict(os.environ, {BINDING_ENV: json.dumps(binding)}):
            for family, factory, configuration, name, arguments, origin, payload in (
                ('services', services_mcp.create_native_server, service(), 'slack_auth_test', {},
                 'https://slack.com', {'ok': True, 'team_id': 'T1', 'user_id': 'U1'}),
                ('services', services_mcp.create_native_server, service('gmail'), 'gmail_get_profile', {},
                 'https://gmail.googleapis.com', {'emailAddress': 'selected@example.invalid',
                  'messagesTotal': 1, 'threadsTotal': 1, 'historyId': '7'}),
                ('agno', agno_mcp.create_native_server, salesforce(), 'get_record',
                 {'sobject': 'Account', 'record_id': RECORD}, ORIGIN,
                 {'Id': RECORD, 'Name': 'Selected record'}),
            ):
                # Construct the inert catalog outside the per-call close controls.
                server = factory(configuration)
                for failed_operation in (False, True):
                    baseline = None
                    for failed_cleanup in (None, 'profile', 'client'):
                        with self.subTest(name=name, failed_operation=failed_operation,
                                          failed_cleanup=failed_cleanup):
                            events, responses, clients = [], [], []
                            async def handle(_transport, request):
                                events.append('request')
                                value = httpx.Response(503 if failed_operation else 200,
                                    json={} if failed_operation else payload, request=request)
                                responses.append(value); return value
                            def send(_adapter, request, **_kwargs):
                                events.append('request')
                                value = response(request)
                                value.status_code = 503 if failed_operation else 200
                                responses.append(value); return value
                            original_http_close = httpx.AsyncClient.aclose
                            original_service_close = services_mcp.ServiceProfile.aclose
                            original_session_close = ServiceSession.close
                            original_agno_close = agno_profiles.AgnoMcpProfile.close
                            async def service_close(profile):
                                await original_service_close(profile); events.append('profile_closed')
                                if failed_cleanup == 'profile': raise RuntimeError(PRIVATE)
                            async def http_close(client):
                                clients.append(client)
                                await original_http_close(client); events.append('client_closed')
                                if failed_cleanup == 'client': raise RuntimeError(PRIVATE)
                            def agno_close(profile):
                                original_agno_close(profile); events.append('profile_closed')
                                if failed_cleanup == 'profile': raise RuntimeError(PRIVATE)
                            def session_close(client):
                                clients.append(client)
                                original_session_close(client); events.append('client_closed')
                                if failed_cleanup == 'client': raise RuntimeError(PRIVATE)
                            with patch.object(httpx.AsyncHTTPTransport, 'handle_async_request', handle), \
                                 patch.object(requests.adapters.HTTPAdapter, 'send', send), \
                                 patch.object(services_mcp.ServiceProfile, 'aclose', service_close), \
                                 patch.object(httpx.AsyncClient, 'aclose', http_close), \
                                 patch.object(agno_profiles.AgnoMcpProfile, 'close', agno_close), \
                                 patch.object(ServiceSession, 'close', session_close):
                                async with Client(server, mode='legacy') as client:
                                    result = await client.call_tool(name, arguments, meta=meta(origin))
                            wire = result.model_dump(by_alias=True, mode='json', exclude_none=True)
                            self.assertEqual(result.is_error, failed_operation)
                            self.assertNotIn(PRIVATE, json.dumps(wire))
                            self.assertEqual(events, ['request', 'profile_closed', 'client_closed'])
                            self.assertEqual(len(clients), 1)
                            if family == 'services':
                                self.assertTrue(responses[0].is_closed); self.assertTrue(clients[0].is_closed)
                            else:
                                self.assertTrue(responses[0].raw.closed)
                            if failed_cleanup is None:
                                self.assertIsNone(result.meta)
                                if not failed_operation:
                                    actual = result.structured_content['value']
                                    self.assertEqual(json.loads(actual) if type(actual) is str else actual, payload)
                                baseline = wire
                            else:
                                self.assertEqual(wire.pop('_meta'), {
                                    'meerkat/connector_cleanup': {'status': 'failed'}})
                                self.assertEqual(wire, baseline)

    async def test_native_no_result_with_cleanup_failure_keeps_fixed_refusal(self):
        for family, factory, configuration, name, arguments, origin, field, kind in (
            ('services', services_mcp.create_native_server, service(), 'slack_auth_test', {},
             'https://slack.com', 'operation_invoked', 'operation_error'),
            ('agno', agno_mcp.create_native_server, salesforce(), 'get_record',
             {'sobject': 'Account', 'record_id': RECORD}, ORIGIN, 'entered', 'entrypoint_error'),
        ):
            with self.subTest(family=family):
                server = factory(configuration)
                closed = []
                original_http_close, original_session_close = httpx.AsyncClient.aclose, ServiceSession.close
                async def http_close(client):
                    await original_http_close(client); closed.append('client_closed'); raise RuntimeError(PRIVATE)
                def session_close(client):
                    original_session_close(client); closed.append('client_closed'); raise RuntimeError(PRIVATE)
                with patch.object(services_mcp, 'create_profile_from_client', side_effect=ValueError(PRIVATE)), \
                     patch.object(agno_profiles, 'build_salesforce_profile_from_client', side_effect=ValueError(PRIVATE)), \
                     patch.object(httpx.AsyncClient, 'aclose', http_close), \
                     patch.object(ServiceSession, 'close', session_close), \
                     patch.object(httpx.AsyncHTTPTransport, 'handle_async_request') as http_entry, \
                     patch.object(requests.adapters.HTTPAdapter, 'send') as request_entry:
                    async with Client(server, mode='legacy') as client:
                        result = await client.call_tool(name, arguments, meta=meta(origin))
                    http_entry.assert_not_called(); request_entry.assert_not_called()
                self.assertTrue(result.is_error); self.assertIsNone(result.meta)
                self.assertEqual(result.structured_content['error']['kind'], kind)
                self.assertFalse(result.structured_content['error'][field])
                self.assertEqual(closed, ['client_closed'])
                self.assertNotIn(PRIVATE, result.model_dump_json(by_alias=True))

    async def test_services_original_cancel_survives_client_close_failure(self):
        closing, release = asyncio.Event(), asyncio.Event()
        events, clients, requests_seen, gates = [], [], [], []
        original_close, original_gate = httpx.AsyncClient.aclose, services_mcp.CallAdmission
        def gate(lock):
            value = original_gate(lock); gates.append(value); return value
        async def close(client):
            clients.append(client)
            await original_close(client); events.append('client_closed')
            raise RuntimeError(PRIVATE)
        async def handle(_transport, request):
            index = len(requests_seen); requests_seen.append(request)
            events.append('request_' + str(index))
            class Stream(httpx.AsyncByteStream):
                async def __aiter__(self):
                    yield b'{"ok":true,"team_id":"T1","user_id":"U1"}'
                async def aclose(self):
                    if index == 0:
                        closing.set(); await release.wait()
                    events.append('response_closed_' + str(index))
            return httpx.Response(200, headers={'Content-Type': 'application/json'},
                                  stream=Stream(), request=request)
        with patch.object(services_mcp, 'CallAdmission', side_effect=gate), \
             patch.object(httpx.AsyncClient, 'aclose', close), \
             patch.object(httpx.AsyncHTTPTransport, 'handle_async_request', handle):
            server = services_mcp.create_native_server(service())
            async with Client(server, mode='legacy') as client:
                first = asyncio.create_task(client.call_tool('slack_auth_test', {},
                    meta=meta('https://slack.com', 'first')))
                queued = None
                try:
                    await asyncio.wait_for(closing.wait(), 3)
                    first.cancel()
                    queued = asyncio.create_task(client.call_tool('slack_auth_test', {},
                        meta=meta('https://slack.com', 'second')))
                    async with asyncio.timeout(3):
                        while gates[0]._admitted != 2: await asyncio.sleep(0)
                    self.assertEqual(events, ['request_0']); self.assertEqual(clients, [])
                    self.assertFalse(queued.done())
                finally:
                    release.set()
                    await asyncio.wait_for(asyncio.gather(first, *([queued] if queued else []),
                        return_exceptions=True), 5)
                self.assertTrue(first.cancelled())
                result = queued.result()
                self.assertFalse(result.is_error)
                self.assertEqual(result.structured_content['value'], {'ok': True, 'team_id': 'T1', 'user_id': 'U1'})
                self.assertEqual(result.meta, {'meerkat/connector_cleanup': {'status': 'failed'}})
                self.assertNotIn(PRIVATE, result.model_dump_json(by_alias=True))
        self.assertEqual(events, ['request_0', 'response_closed_0', 'client_closed',
                                  'request_1', 'response_closed_1', 'client_closed'])
        self.assertEqual(len(clients), 2); self.assertTrue(all(client.is_closed for client in clients))

    async def test_salesforce_cancel_joins_worker_despite_client_close_failure(self):
        entered, release, exited = threading.Event(), threading.Event(), threading.Event()
        closed = []
        original = ServiceSession.close
        def send(_adapter, request, **_kwargs):
            entered.set()
            if not release.wait(3): raise AssertionError('missing fixture release')
            exited.set(); return response(request)
        def close(session):
            closed.append(exited.is_set()); original(session); raise RuntimeError(PRIVATE)
        server = agno_mcp.create_native_server(salesforce())
        try:
            with patch.object(requests.adapters.HTTPAdapter, 'send', send), patch.object(ServiceSession, 'close', close):
                async with Client(server, mode='legacy') as client:
                    task = asyncio.create_task(client.call_tool('get_record',
                        {'sobject': 'Account', 'record_id': RECORD}, meta=meta(ORIGIN)))
                    self.assertTrue(await asyncio.to_thread(entered.wait, 2))
                    task.cancel(); await asyncio.sleep(0)
                    self.assertEqual(closed, []); self.assertFalse(exited.is_set())
                    release.set()
                    with self.assertRaises(asyncio.CancelledError): await task
                self.assertEqual(closed, [True])
        finally: release.set()


if __name__ == '__main__': unittest.main()
