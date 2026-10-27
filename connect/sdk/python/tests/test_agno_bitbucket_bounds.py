"""Actual Requests/urllib3 loopback reads through Agno's public MCP adapter."""
from __future__ import annotations

import asyncio
import base64
from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
import threading
import time
import unittest
from unittest.mock import patch
from urllib.parse import parse_qs, urlsplit

from mcp import Client
import requests

from meerkat_toolkit.adapters.agno_mcp import create_server
from meerkat_toolkit.adapters.agno_profiles import build_profile
from meerkat_toolkit.adapters.agno_transports import build_bitbucket


LIMIT = 1_048_576
KEY = 'bitbucket-fixture-secret-never-report'
CANARY = 'provider-diagnostic-never-report'
OPERATIONS = ('list_repositories', 'get_repository_details',
              'list_all_pull_requests', 'get_pull_request_details')


class TrackedRaw:
    def __init__(self, raw, observed):
        self.raw, self.observed = raw, observed

    def __getattr__(self, key):
        return getattr(self.raw, key)

    def stream(self, *args, **kwargs):
        for data in self.raw.stream(*args, **kwargs):
            self.observed['body_bytes'] += len(data)
            yield data

    def read1(self, *args, **kwargs):
        data = self.raw.read1(*args, **kwargs)
        self.observed['body_bytes'] += len(data)
        return data


@contextmanager
def fixture(*, body=b'{"name":"fixture"}', status=200, length=True,
            encoding=None, slow=False, chunked=False, idle=False, content_type='application/json'):
    """Only the test adapter rewrites the fixed public origin to loopback."""
    observed = {'body_bytes': 0, 'requests': [], 'response_closes': 0,
                'started': threading.Event(), 'finished': threading.Event()}
    stop = threading.Event()

    class Handler(BaseHTTPRequestHandler):
        protocol_version = 'HTTP/1.1'

        def log_message(self, *_args):
            pass

        def do_GET(self):
            try:
                self.send_response(status)
                self.send_header('Content-Type', content_type)
                self.send_header('Connection', 'close')
                if encoding is not None:
                    self.send_header('Content-Encoding', encoding)
                if chunked:
                    self.send_header('Transfer-Encoding', 'chunked')
                elif length:
                    self.send_header('Content-Length', str(len(body)))
                if status == 302:
                    self.send_header('Location', 'https://foreign.invalid/credential-target')
                if status == 429:
                    self.send_header('Retry-After', '300')
                self.end_headers()
                observed['started'].set()
                if idle:
                    stop.wait(3)
                    return
                if chunked:
                    # One entire declared chunk, not one chunk per byte. This
                    # detects a reader that waits to fill a large chunk.
                    self.wfile.write(f'{len(body):x}\r\n'.encode())
                    self.wfile.flush()
                if slow:
                    for byte in body:
                        self.wfile.write(bytes([byte]))
                        self.wfile.flush()
                        if stop.wait(0.05):
                            break
                else:
                    self.wfile.write(body)
                if chunked and not stop.is_set():
                    self.wfile.write(b'\r\n0\r\n\r\n')
                self.wfile.flush()
            except (BrokenPipeError, ConnectionResetError):
                pass
            finally:
                observed['finished'].set()
                self.close_connection = True

    server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    server.daemon_threads = False
    thread = threading.Thread(target=server.serve_forever, kwargs={'poll_interval': 0.01})
    thread.start()
    original_send = requests.adapters.HTTPAdapter.send

    def send(adapter, request, **kwargs):
        parsed = urlsplit(request.url)
        if parsed.scheme != 'https' or parsed.netloc != 'api.bitbucket.org':
            raise AssertionError('Unexpected network target')
        observed['requests'].append(request.copy())
        local = request.copy()
        local.url = f'http://127.0.0.1:{server.server_port}{parsed.path}'
        if parsed.query:
            local.url += '?' + parsed.query
        response = original_send(adapter, local, **kwargs)
        response.raw = TrackedRaw(response.raw, observed)
        original_close = response.close

        def close():
            observed['response_closes'] += 1
            original_close()

        response.close = close
        return response

    try:
        with patch.object(requests.adapters.HTTPAdapter, 'send', send):
            yield observed
    finally:
        stop.set()
        server.shutdown()
        thread.join(4)
        server.server_close()
        assert not thread.is_alive()


class BitbucketBoundsTests(unittest.IsolatedAsyncioTestCase):
    def profile(self):
        with patch.dict(os.environ, {'BITBUCKET_BOUNDS_TEST_KEY': KEY}, clear=True):
            value = build_profile('bitbucket', operations=OPERATIONS, config={
                'api_key_env': 'BITBUCKET_BOUNDS_TEST_KEY', 'timeout': 1,
                'account_email': 'fixture@example.invalid', 'workspace': 'fixture', 'repo_slug': 'project'})
        self.addCleanup(value.close)
        return value

    def failure(self, result):
        self.assertTrue(result.is_error, result.model_dump_json()[:300])
        self.assertEqual(result.structured_content['error']['kind'], 'upstream_operation_error')
        self.assertEqual(result.structured_content['error']['write_outcome'], 'not_applicable')
        self.assertNotIn(KEY, result.model_dump_json())
        self.assertNotIn(CANARY, result.model_dump_json())

    async def test_declared_oversize_rejected_before_consumption(self):
        body = b'{"description":"' + b'x' * (LIMIT * 2) + b'"}'
        with fixture(body=body) as seen:
            async with Client(create_server(self.profile())) as client:
                result = await client.call_tool('get_repository_details', {})
            print(json.dumps({'case': 'declared_oversize', 'body_bytes': seen['body_bytes']}))
            self.assertEqual(seen['body_bytes'], 0)
            self.failure(result)
            self.assertEqual(len(seen['requests']), 1)
            self.assertEqual(seen['response_closes'], 1)

    async def test_undeclared_and_chunked_oversize_are_bounded(self):
        for chunked in (False, True):
            with self.subTest(chunked=chunked), fixture(
                    body=b'{"description":"' + b'x' * (LIMIT * 2) + b'"}',
                    length=False, chunked=chunked) as seen:
                async with Client(create_server(self.profile())) as client:
                    result = await client.call_tool('get_repository_details', {})
                self.assertLessEqual(seen['body_bytes'], LIMIT + 1)
                self.failure(result)
                self.assertEqual(len(seen['requests']), 1)
                self.assertEqual(seen['response_closes'], 1)

    async def test_continuous_body_deadline_joins_repeated_cancellation(self):
        for chunked in (False, True):
            body = b'{"description":"' + b'x' * 40 + b'"}'
            with self.subTest(chunked=chunked), fixture(body=body, slow=True, chunked=chunked) as seen:
                async with Client(create_server(self.profile())) as client:
                    started = time.monotonic()
                    active = asyncio.create_task(client.call_tool('get_repository_details', {}))
                    while not seen['started'].is_set():
                        await asyncio.sleep(0.005)
                    await asyncio.sleep(0.1)
                    active.cancel()
                    await asyncio.sleep(0.05)
                    active.cancel()
                    with self.assertRaises(asyncio.CancelledError):
                        await active
                    elapsed = time.monotonic() - started
                    print(json.dumps({'case': 'continuous_cancel', 'chunked': chunked,
                                      'elapsed_seconds': round(elapsed, 3), 'body_bytes': seen['body_bytes']}))
                    self.assertLess(elapsed, 1.8)
                    self.assertEqual(seen['response_closes'], 1)
                    self.assertEqual(len(seen['requests']), 1)

    async def test_error_and_redirect_bodies_are_not_read_or_retried(self):
        for status in (302, 401, 429, 500):
            with self.subTest(status=status), fixture(
                    body=CANARY.encode() * (LIMIT // len(CANARY) + 1), status=status) as seen:
                async with Client(create_server(self.profile())) as client:
                    self.failure(await client.call_tool('get_repository_details', {}))
                self.assertEqual(seen['body_bytes'], 0)
                self.assertEqual(seen['response_closes'], 1)
                self.assertEqual(len(seen['requests']), 1)

    async def test_deadline_failure_then_success_reuses_selected_profile(self):
        profile = self.profile()
        async with Client(create_server(profile)) as client:
            with fixture(body=b'{"description":"' + b'x' * 40 + b'"}', slow=True, chunked=True) as seen:
                started = time.monotonic()
                result = await client.call_tool('get_repository_details', {})
                elapsed = time.monotonic() - started
                self.failure(result)
                self.assertLess(elapsed, 1.8)
                self.assertEqual(seen['response_closes'], 1)
                self.assertEqual(len(seen['requests']), 1)
            with fixture(body=b'{"name":"still-selected"}') as seen:
                result = await client.call_tool('get_repository_details', {})
                self.assertFalse(result.is_error)
                self.assertEqual(json.loads(result.structured_content['value']), {'name': 'still-selected'})
                self.assertEqual(len(seen['requests']), 1)

    async def test_success_preserves_four_donor_contracts_and_account(self):
        body = {'id': 2, 'values': [{'description': None, 'nested': {'name': 'fixture'}}],
                'next': 'https://foreign.invalid/next', 'unknown': ['preserved', False]}
        with fixture(body=json.dumps(body).encode()) as seen:
            async with Client(create_server(self.profile())) as client:
                tools = await client.list_tools()
                self.assertEqual({t.name for t in tools.tools}, set(OPERATIONS))
                for operation in OPERATIONS:
                    args = {'pull_request_id': 2} if operation == 'get_pull_request_details' else {}
                    if operation == 'list_all_pull_requests':
                        args = {'state': 'MERGED'}
                    result = await client.call_tool(operation, args)
                    self.assertFalse(result.is_error, result.model_dump_json())
                    self.assertEqual(json.loads(result.structured_content['value']), body)
            self.assertEqual(len(seen['requests']), 4)
            self.assertEqual(seen['response_closes'], 4)
            authorization = 'Basic ' + base64.b64encode(f'fixture@example.invalid:{KEY}'.encode()).decode()
            for request in seen['requests']:
                self.assertEqual(request.headers['Authorization'], authorization)
                self.assertEqual(request.headers['Accept-Encoding'], 'identity')
            self.assertEqual(parse_qs(urlsplit(seen['requests'][2].url).query),
                             {'state': ['MERGED'], 'page': ['1'], 'pagelen': ['20']})

    async def test_idle_timeout_invalid_json_and_encoding_are_safe(self):
        cases = ({'idle': True}, {'body': b'{invalid ' + CANARY.encode()},
                 {'body': b'{"bad":NaN}'}, {'body': b'\xff'},
                 {'body': b'not-read', 'encoding': 'gzip'},
                 {'body': b'not-read', 'content_type': 'text/html'})
        for case in cases:
            with self.subTest(case=list(case)), fixture(**case) as seen:
                async with Client(create_server(self.profile())) as client:
                    self.failure(await client.call_tool('get_repository_details', {}))
                self.assertEqual(len(seen['requests']), 1)
                self.assertEqual(seen['response_closes'], 1)
                if 'encoding' in case or 'content_type' in case:
                    self.assertEqual(seen['body_bytes'], 0)

    def test_preloaded_response_and_borrowed_session_compatibility(self):
        session = requests.Session()
        with patch.object(session, 'close', wraps=session.close) as close:
            toolkit = build_bitbucket(KEY, 1, session=session, account_email='fixture@example.invalid',
                                      workspace='fixture', repo_slug='project')
            response = requests.Response()
            response.status_code = 200
            response._content = b'{"unknown":null,"values":[]}'
            response.headers['Content-Type'] = 'application/json'
            with patch.object(session, 'request', return_value=response):
                self.assertEqual(json.loads(toolkit.get_repository_details()), {'unknown': None, 'values': []})
            close.assert_not_called()
        session.close()

    def test_preloaded_exact_byte_limit_and_oversize(self):
        with requests.Session() as session:
            toolkit = build_bitbucket(KEY, 1, session=session, account_email='fixture@example.invalid',
                                      workspace='fixture', repo_slug='project')
            for size in (LIMIT, LIMIT + 1):
                with self.subTest(size=size):
                    response = requests.Response()
                    response.status_code = 200
                    response.headers['Content-Type'] = 'application/json; charset=utf-8'
                    response._content = b'{"value":"' + b'x' * (size - 12) + b'"}'
                    self.assertEqual(len(response._content), size)
                    with patch.object(session, 'request', return_value=response):
                        result = json.loads(toolkit.get_repository_details())
                    if size == LIMIT:
                        self.assertEqual(len(result['value']), size - 12)
                    else:
                        self.assertEqual(result, {'error': 'Bitbucket request failed'})

    async def test_borrowed_plain_session_keeps_hooks_and_rejects_redirect_before_body(self):
        with requests.Session() as session:
            session.trust_env = False
            observed = []
            hook = lambda response, **_kwargs: observed.append(response.status_code)
            session.hooks['response'].append(hook)
            toolkit = build_bitbucket(KEY, 1, session=session, account_email='fixture@example.invalid',
                                      workspace='fixture', repo_slug='project')
            with fixture(status=302, body=CANARY.encode() * 100_000) as seen:
                self.assertEqual(json.loads(await asyncio.to_thread(toolkit.get_repository_details)),
                                 {'error': 'Bitbucket request failed'})
                self.assertEqual(seen['body_bytes'], 0)
                self.assertEqual(seen['response_closes'], 1)
                self.assertEqual(len(seen['requests']), 1)
                self.assertEqual(observed, [])
            with fixture() as seen:
                self.assertEqual(json.loads(await asyncio.to_thread(toolkit.get_repository_details)),
                                 {'name': 'fixture'})
                self.assertEqual(observed, [200])
                self.assertEqual(session.hooks['response'], [hook])
                self.assertEqual(seen['response_closes'], 1)

    async def test_borrowed_hook_failure_keeps_response_custody(self):
        with requests.Session() as session:
            session.trust_env = False
            def fail(_response, **_kwargs):
                raise ValueError(CANARY)
            session.hooks['response'].append(fail)
            toolkit = build_bitbucket(KEY, 1, session=session, account_email='fixture@example.invalid',
                                      workspace='fixture', repo_slug='project')
            with fixture() as seen:
                result = json.loads(await asyncio.to_thread(toolkit.get_repository_details))
                self.assertEqual(result, {'error': 'Bitbucket request failed'})
                self.assertEqual(seen['body_bytes'], 0)
                self.assertEqual(seen['response_closes'], 1)
                self.assertEqual(session.hooks['response'], [fail])

    async def test_borrowed_hook_replacement_and_mutation_keep_custody(self):
        for replacement_status in (200, 302, None):
            with self.subTest(status=replacement_status), requests.Session() as session:
                session.trust_env = False
                replacement = requests.Response()
                replacement.status_code = replacement_status or 200
                replacement._content = b'{"name":"replacement"}'
                replacement.headers['Content-Type'] = 'application/json'
                def hook(response, **_kwargs):
                    if replacement_status is None:
                        response.status_code = 302
                        response.headers['Location'] = 'https://foreign.invalid/'
                        return None
                    return replacement
                session.hooks['response'].append(hook)
                toolkit = build_bitbucket(KEY, 1, session=session, account_email='fixture@example.invalid',
                                          workspace='fixture', repo_slug='project')
                with patch.object(replacement, 'close', wraps=replacement.close) as close, fixture() as seen:
                    result = json.loads(await asyncio.to_thread(toolkit.get_repository_details))
                    self.assertEqual(result, {'name': 'replacement'} if replacement_status == 200
                                     else {'error': 'Bitbucket request failed'})
                    self.assertEqual(seen['body_bytes'], 0)
                    self.assertEqual(seen['response_closes'], 1)
                    self.assertEqual(close.call_count, 0 if replacement_status is None else 1)
                    self.assertEqual(len(seen['requests']), 1)
                    self.assertEqual(session.hooks['response'], [hook])

    def test_response_close_failure_remains_safe_failure(self):
        with requests.Session() as session:
            toolkit = build_bitbucket(KEY, 1, session=session, account_email='fixture@example.invalid',
                                      workspace='fixture', repo_slug='project')
            response = requests.Response()
            response.status_code = 200
            response._content = b'{"name":"fixture"}'
            response.headers['Content-Type'] = 'application/json'
            with patch.object(session, 'request', return_value=response), patch.object(
                    response, 'close', side_effect=ValueError(CANARY)) as close:
                self.assertEqual(json.loads(toolkit.get_repository_details()), {'error': 'Bitbucket request failed'})
                close.assert_called_once()


if __name__ == '__main__':
    unittest.main()
