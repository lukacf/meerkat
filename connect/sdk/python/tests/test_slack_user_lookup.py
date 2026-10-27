"""Explicit Slack user observations through the existing HTTP and MCP owners."""
from __future__ import annotations

import asyncio
from contextlib import asynccontextmanager
import json
import unittest

import httpx
from mcp import Client
from meerkat_toolkit.adapters import services_config, services_mcp
from meerkat_toolkit.connectors.slack import FailureKind, SlackError, SlackOperations
from meerkat_toolkit.httpx_client import prepare_httpx_client
from test_home_assistant import Server, client as loopback_client, respond


PRIVATE = 'fixture-private-auth-and-provider-message'


def configuration(operations=('get_user',), channels=None):
    config = {'token_env': 'UNUSED_FIXTURE_REFERENCE', 'timeout_seconds': 2,
              'max_response_bytes': 65536}
    if channels is not None:
        config['allowed_channel_ids'] = channels
    return services_config.validate_configuration({'profile': 'slack',
        'operations': list(operations), 'config': config})


class SlackUserLookupTests(unittest.IsolatedAsyncioTestCase):
    async def test_explicit_user_lookup_preserves_observation(self):
        value = {'ok': True, 'user': {'id': 'W012A3CDE', 'deleted': True,
            'profile': {'display_name': '', 'real_name': None},
            'is_stranger': True, 'future': [None, {'nested': False}]}, 'future': None}
        requests = []
        def handle(request):
            requests.append(request)
            return httpx.Response(200, json=value)
        async with httpx.AsyncClient(base_url='https://slack.com/selected/proxy/',
                params={'user': 'WRONG', 'token': PRIVATE},
                headers={'Authorization': 'Bearer '+PRIVATE},
                transport=httpx.MockTransport(handle)) as client:
            prepare_httpx_client(client)
            operations = SlackOperations(client, allowed_channel_ids=[])
            result = await operations.get_user('W012A3CDE')
            self.assertEqual(result, value)
            self.assertFalse(client.is_closed)
        self.assertEqual(len(requests), 1)
        self.assertEqual(requests[0].method, 'GET')
        self.assertEqual(requests[0].url.path, '/selected/proxy/api/users.info')
        self.assertEqual(dict(requests[0].url.params), {'user': 'W012A3CDE', 'include_locale': 'false'})
        self.assertEqual(requests[0].content, b'')

    async def test_explicit_mcp_selection_and_channel_ceiling(self):
        requests = []
        def handle(request):
            requests.append(request)
            return httpx.Response(200, json={'ok': True, 'user': {'id': 'U123', 'profile': None}})
        async with httpx.AsyncClient(base_url='https://slack.com/',
                transport=httpx.MockTransport(handle)) as client:
            prepare_httpx_client(client)
            profile = services_mcp.create_profile_from_client(configuration(channels=[]), client)
            try:
                async with Client(services_mcp.create_server(profile)) as mcp:
                    listed = await mcp.list_tools()
                    self.assertEqual([t.name for t in listed.tools], ['slack_get_user'])
                    result = await mcp.call_tool('slack_get_user', {'user': 'U123', 'include_locale': True})
                    self.assertFalse(result.is_error)
                    self.assertEqual(result.structured_content['value']['user'], {'id': 'U123', 'profile': None})
            finally:
                await profile.aclose()
        self.assertEqual(len(requests), 1)
        self.assertEqual(dict(requests[0].url.params), {'user': 'U123', 'include_locale': 'true'})

    async def test_loopback_optional_fields_and_identifier_boundary(self):
        user = 'U' * 128
        for fields in ({}, {'profile': None}, {'profile': {'display_name': '', 'real_name': None}},
                       {'profile': {'email': 'observed@example.invalid'}, 'is_bot': True}):
            value = {'ok': True, 'user': {'id': user, **fields}}
            async def handle(_request, writer):
                await respond(writer, value)
            async with Server(handle) as server:
                async with loopback_client(server) as transport:
                    self.assertEqual(await SlackOperations(transport).get_user(user), value)
                self.assertEqual(len(server.requests), 1)
                self.assertEqual(server.requests[0].method, 'GET')
            self.assertTrue(server.joined)

    async def test_mcp_safe_missing_user_and_unselected_operation(self):
        async with user_client({'ok': False, 'error': 'user_not_found', 'detail': PRIVATE}) as (ops, client, requests, stream):
            for operations in (('get_user',), ('auth_test',)):
                profile = services_mcp.create_profile_from_client(configuration(operations), client)
                try:
                    async with Client(services_mcp.create_server(profile)) as mcp:
                        result = await mcp.call_tool('slack_get_user', {'user': 'U1'})
                        error = result.structured_content['error']
                        self.assertEqual(error['kind'], 'not_found' if operations == ('get_user',) else 'unknown_function')
                        if operations == ('get_user',):
                            self.assertEqual(error['provider_code'], 'user_not_found')
                            self.assertEqual(error['write_outcome'], 'not_applicable')
                        self.assertNotIn(PRIVATE, result.model_dump_json())
                finally:
                    await profile.aclose()
            self.assertEqual(len(requests), 1)

    async def test_invalid_arguments_refuse_before_transport(self):
        async with user_client({'ok': True, 'user': {'id': 'U1'}}) as (ops, client, requests, stream):
            for user in (None, True, 12, '', 'U1\n', '../U1', 'U'*129, 'ü'):
                with self.subTest(user=user), self.assertRaises(ValueError):
                    await ops.get_user(user)
            for flag in (None, 0, 1, 'true', [], {}):
                with self.subTest(flag=flag), self.assertRaises(ValueError):
                    await ops.get_user('U1', include_locale=flag)
            self.assertEqual(requests, [])
            self.assertEqual(stream.closed, 0)

    async def test_malformed_identity_and_envelope_fail_without_reflecting_body(self):
        values = [None, [], {}, {'ok': 1}, {'ok': True},
                  {'ok': True, 'user': None}, {'ok': True, 'user': []},
                  {'ok': True, 'user': {}}, {'ok': True, 'user': {'id': 'UOTHER'}}]
        values += [{'ok': True, 'user': {'id': value}} for value in (None, 1, True, '', 'U1\n')]
        for value in values:
            with self.subTest(value=value):
                async with user_client(value) as (ops, client, requests, stream):
                    with self.assertRaises(SlackError) as caught:
                        await ops.get_user('U1')
                    self.assertEqual(caught.exception.kind, FailureKind.INVALID_RESPONSE)
                    self.assertEqual(len(requests), 1)
                    self.assertEqual(stream.closed, 1)
        for raw in (b'{"ok":true,"user":{"id":"U1","id":"U1"}}',
                    b'{"ok":true,"user":{"id":"U1"}} trailing', b'\xff'):
            async with user_client(raw=raw) as (ops, client, requests, stream):
                with self.assertRaises(SlackError) as caught:
                    await ops.get_user('U1')
                self.assertEqual(caught.exception.kind, FailureKind.INVALID_RESPONSE)
                self.assertEqual(stream.closed, 1)

    async def test_provider_and_http_errors_are_safe_and_single_attempt(self):
        for code, kind in [('user_not_found', FailureKind.NOT_FOUND),
                           ('missing_scope', FailureKind.PERMISSION),
                           ('ratelimited', FailureKind.RATE_LIMITED),
                           (PRIVATE, FailureKind.PROVIDER)]:
            async with user_client({'ok': False, 'error': code, 'detail': PRIVATE}) as (ops, client, requests, stream):
                with self.assertRaises(SlackError) as caught:
                    await ops.get_user('U1')
                error = caught.exception
                self.assertEqual(error.kind, kind)
                self.assertEqual(error.provider_code, None if code == PRIVATE else code)
                self.assertEqual(error.retry_after_seconds, 17)
                self.assertNotIn(PRIVATE, repr(vars(error)))
                self.assertEqual(len(requests), 1)
                self.assertEqual(stream.closed, 1)
        for status, kind in [(401, FailureKind.AUTHENTICATION), (403, FailureKind.PERMISSION),
                             (404, FailureKind.NOT_FOUND), (429, FailureKind.RATE_LIMITED),
                             (302, FailureKind.REDIRECT)]:
            async with user_client(raw=PRIVATE.encode(), status=status) as (ops, client, requests, stream):
                with self.assertRaises(SlackError) as caught:
                    await ops.get_user('U1')
                self.assertEqual(caught.exception.kind, kind)
                self.assertEqual(len(requests), 1)
                self.assertEqual(stream.closed, 1)
                self.assertEqual(stream.reads, 0)

    async def test_body_limits_encoding_and_transport_cleanup(self):
        raw = b'{"ok":true,"user":{"id":"U1"}}'
        for maximum, extra in [(len(raw), 0), (len(raw), 1)]:
            async with user_client(raw=raw+b' '*extra, maximum=maximum) as (ops, client, requests, stream):
                if extra:
                    with self.assertRaises(SlackError) as caught:
                        await ops.get_user('U1')
                    self.assertEqual(caught.exception.kind, FailureKind.RESPONSE_TOO_LARGE)
                else:
                    self.assertEqual((await ops.get_user('U1'))['user']['id'], 'U1')
                self.assertEqual(stream.closed, 1)
        for headers, kind in [({'Content-Encoding': 'gzip'}, FailureKind.UNSUPPORTED_ENCODING),
                              ({'Content-Type': 'text/html'}, FailureKind.INVALID_RESPONSE),
                              ({'Content-Length': '999999'}, FailureKind.RESPONSE_TOO_LARGE),
                              ({'Content-Length': 'wrong'}, FailureKind.INVALID_RESPONSE)]:
            async with user_client(raw=raw, headers=headers) as (ops, client, requests, stream):
                with self.assertRaises(SlackError) as caught:
                    await ops.get_user('U1')
                self.assertEqual(caught.exception.kind, kind)
                self.assertEqual(stream.closed, 1)
                self.assertEqual(stream.reads, 0)
        stream = UserStream([b'{"ok":', httpx.ReadError(PRIVATE)])
        async with user_client(stream=stream) as (ops, client, requests, stream):
            with self.assertRaises(SlackError) as caught:
                await ops.get_user('U1')
            self.assertEqual(caught.exception.kind, FailureKind.TRANSPORT)
            self.assertNotIn(PRIVATE, repr(vars(caught.exception)))
            self.assertEqual(stream.closed, 1)

    async def test_cancel_and_deadline_join_response_close(self):
        for cancel in (False, True):
            stream = UserStream([b'{"ok":true,'], block_read=True, block_close=True)
            async with user_client(stream=stream, timeout=2 if cancel else .02) as (ops, client, requests, stream):
                task = asyncio.create_task(ops.get_user('U1'))
                try:
                    await asyncio.wait_for(stream.reading.wait(), 1)
                    if cancel:
                        task.cancel(PRIVATE)
                    await asyncio.wait_for(stream.closing.wait(), 1)
                    if cancel:
                        task.cancel(PRIVATE)
                    await asyncio.sleep(0)
                    self.assertFalse(task.done())
                    stream.close_release.set()
                    if cancel:
                        with self.assertRaises(asyncio.CancelledError):
                            await asyncio.wait_for(task, 1)
                    else:
                        with self.assertRaises(SlackError) as caught:
                            await asyncio.wait_for(task, 1)
                        self.assertEqual(caught.exception.kind, FailureKind.TIMEOUT)
                    self.assertEqual(stream.closed, 1)
                    self.assertFalse(client.is_closed)
                    self.assertEqual(len(requests), 1)
                finally:
                    stream.close_release.set()
                    stream.read_release.set()
                    if not task.done():
                        task.cancel()
                    await asyncio.gather(task, return_exceptions=True)

    async def test_separate_account_clients_and_no_user_cache(self):
        for account in ('first', 'second'):
            value = {'ok': True, 'user': {'id': 'U1', 'profile': {'display_name': account}}}
            async with user_client(value, token=account) as (ops, client, requests, stream):
                self.assertEqual((await ops.get_user('U1'))['user']['profile']['display_name'], account)
                self.assertEqual((await ops.get_user('U1'))['user']['profile']['display_name'], account)
                self.assertEqual(len(requests), 2)
                self.assertTrue(all(request.headers['Authorization'] == 'Bearer '+account for request in requests))

    async def test_mcp_refuses_unselected_invalid_and_outside_channel_calls(self):
        async with user_client({'ok': True, 'user': {'id': 'U1'}}) as (ops, client, requests, stream):
            profile = services_mcp.create_profile_from_client(configuration(('get_user', 'get_conversation'), ['C1']), client)
            try:
                async with Client(services_mcp.create_server(profile)) as mcp:
                    for arguments in ({}, {'user': 1}, {'user': 'U1', 'include_locale': 'true'}, {'user': 'U1', 'unknown': PRIVATE}):
                        result = await mcp.call_tool('slack_get_user', arguments)
                        self.assertTrue(result.is_error)
                        self.assertEqual(result.structured_content['error']['kind'], 'invalid_arguments')
                    result = await mcp.call_tool('slack_get_conversation', {'channel': 'C2'})
                    self.assertEqual(result.structured_content['error']['kind'], 'permission')
                    result = await mcp.call_tool('slack_history_page', {'channel': 'C1'})
                    self.assertEqual(result.structured_content['error']['kind'], 'unknown_function')
                    self.assertEqual(requests, [])
                    result = await mcp.call_tool('slack_get_user', {'user': 'U1'})
                    self.assertFalse(result.is_error)
                    self.assertEqual(len(requests), 1)
            finally:
                await profile.aclose()

    def test_help_explains_independent_scope_without_mutating_other_operations(self):
        help = services_config.configuration_help('slack')
        text = json.dumps(help)
        self.assertIn('users:read', text)
        self.assertIn('users:read.email', text)
        self.assertIn('independent workspace metadata', text)
        self.assertIn('does not request or widen scopes', text)
        selected = services_config.tool_descriptions(configuration(('auth_test',)))
        self.assertEqual([item['name'] for item in selected], ['slack_auth_test'])
        self.assertEqual(selected[0]['inputSchema']['properties'], {})


class UserStream(httpx.AsyncByteStream):
    def __init__(self, chunks, *, block_read=False, block_close=False):
        self.chunks = chunks
        self.reads = self.closed = 0
        self.reading, self.closing = asyncio.Event(), asyncio.Event()
        self.read_release, self.close_release = asyncio.Event(), asyncio.Event()
        if not block_read:
            self.read_release.set()
        if not block_close:
            self.close_release.set()

    async def __aiter__(self):
        self.reading.set()
        for chunk in self.chunks:
            self.reads += 1
            if isinstance(chunk, Exception):
                raise chunk
            yield chunk
        await self.read_release.wait()

    async def aclose(self):
        self.closing.set()
        await self.close_release.wait()
        self.closed += 1


@asynccontextmanager
async def user_client(value=None, *, raw=None, status=200, headers=None, maximum=65536,
                      stream=None, timeout=2, token=PRIVATE):
    if raw is None:
        raw = json.dumps(value).encode()
    if stream is None:
        stream = UserStream([raw])
    requests = []
    def handle(request):
        requests.append(request)
        return httpx.Response(status, headers={'Content-Type': 'application/json', 'Retry-After': '17', **(headers or {})}, stream=stream)
    async with httpx.AsyncClient(base_url='https://slack.com/', headers={'Authorization': 'Bearer '+token},
            transport=httpx.MockTransport(handle), trust_env=False) as client:
        prepare_httpx_client(client)
        yield SlackOperations(client, timeout_seconds=timeout, max_response_bytes=maximum), client, requests, stream
