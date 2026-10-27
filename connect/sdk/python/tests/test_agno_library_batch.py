"""Eight real donor/SDK read integrations; only local controlled provider bodies."""
from __future__ import annotations

import asyncio
from contextlib import ExitStack, redirect_stderr, redirect_stdout
import hashlib
import importlib
import inspect
import io
import json
import os
from pathlib import Path
import threading
import unittest
from unittest.mock import patch
from urllib.parse import parse_qs, urlsplit

from jsonschema import Draft202012Validator
from mcp import Client
import requests
from meerkat_toolkit.adapters.agno_mcp import create_server
from meerkat_toolkit.adapters.agno_profiles import build_profile, validate_configuration, ProfileConfigError

REFERENCE = 'LIBRARY_FIXTURE_KEY'
TOKEN_REF = 'LIBRARY_FIXTURE_TOKEN'
KEY = 'selected-library-key'
TOKEN = 'selected-trello-token'
CANARY = 'ambient-or-provider-secret'
ACCOUNT = 'AC' + '1' * 32
CALL = 'CA' + '2' * 32
AUTH = 'MA' + '3' * 18
UUID = '12345678-1234-1234-1234-123456789abc'
RECORD = '001' + '4' * 12
OPERATIONS = {'exa': ['search_exa'], 'firecrawl': ['scrape_website', 'map_website'],
              'trello': ['list_boards'], 'salesforce': ['get_record'], 'twilio': ['get_call_details'],
              'webex': ['list_rooms'], 'plivo': ['get_call_details'], 'arxiv': ['search_arxiv_and_return_articles']}
DONOR_CLASSES = {'exa': 'ExaTools', 'firecrawl': 'FirecrawlTools', 'trello': 'TrelloTools',
                 'salesforce': 'SalesforceTools', 'twilio': 'TwilioTools', 'webex': 'WebexTools',
                 'plivo': 'PlivoTools', 'arxiv': 'ArxivTools'}
DONOR_HASHES = {'exa': '593fc82a232ea3aa2815750ee6191c8c35de46ca43a51324563117e49aaf3df1', 'firecrawl': '0361a79c24e488be3fda2c5824afc62a15b1061d820b6e999bba74b4f1e8f59a', 'trello': 'a8bddaaf8a824c24348daf7941588b406062b725c8e7f4baf71d7ec0a69b5cd4', 'salesforce': 'fefb9c5c4997a4d10725f49e8c898b73ce043c2b55c1100eb47e027ac6730371', 'twilio': '7b00c4b451968a114864e74b1b0accbeb3355f72cfe635c66f3cfe2b4e66655d', 'webex': 'e4d2467f52181d7fcb98fa97660afc02bdcb82d2c30629378bccecd3063b4daf', 'plivo': '6e157636f32a7dc04858d1e13f629bc0b661d2167f286eb99690271df19f0705', 'arxiv': 'b667b57fdfaec93c2a91d09c9d4e46451fc7d0b105b3b6c3da3c087b3af04c5d'}

HOSTILE = {REFERENCE: KEY, TOKEN_REF: TOKEN, **{key: CANARY for key in (
    'EXA_API_KEY', 'FIRECRAWL_API_KEY', 'TRELLO_API_KEY', 'TRELLO_API_SECRET', 'TRELLO_TOKEN',
    'SALESFORCE_USERNAME', 'SALESFORCE_PASSWORD', 'SALESFORCE_SECURITY_TOKEN',
    'TWILIO_ACCOUNT_SID', 'TWILIO_AUTH_TOKEN', 'TWILIO_API_KEY', 'TWILIO_API_SECRET', 'TWILIO_REGION', 'TWILIO_EDGE',
    'WEBEX_ACCESS_TOKEN', 'BE_GEO_ID', 'WEBEX_PYTHON_SDK_CALLER', 'PLIVO_AUTH_ID', 'PLIVO_AUTH_TOKEN',
)}, 'HTTPS_PROXY': 'http://ambient.invalid', 'NETRC': '/nonexistent/ambient'}


def config(name):
    return {'timeout': 3, **({} if name == 'arxiv' else {'api_key_env': REFERENCE}), **{
        'trello': {'token_env': TOKEN_REF}, 'salesforce': {'instance_url': 'https://selected.salesforce.com'},
        'twilio': {'account_sid': ACCOUNT}, 'plivo': {'auth_id': AUTH},
    }.get(name, {})}


def arguments(name, op=None):
    return {'exa': {'query': 'Useful caf\u00e9 search', 'num_results': 2},
            'firecrawl': {'url': 'https://target.example.invalid/article'}, 'trello': {},
            'salesforce': {'sobject': 'Account', 'record_id': RECORD}, 'twilio': {'call_sid': CALL},
            'plivo': {'call_uuid': UUID}, 'webex': {},
            'arxiv': {'query': 'quantum tests', 'num_articles': 2}}[name]


def atom(empty=False):
    entry = '' if empty else '''<entry><id>http://arxiv.org/abs/2601.00001v1</id>
      <updated>2026-01-01T00:00:00Z</updated><published>2026-01-01T00:00:00Z</published>
      <title>Useful quantum paper</title><summary>Original research summary</summary>
      <author><name>Ada Scientist</name></author><arxiv:primary_category term="quant-ph"/>
      <category term="quant-ph"/><link href="https://arxiv.org/pdf/2601.00001v1" title="pdf"/></entry>'''
    return (f'''<feed xmlns="http://www.w3.org/2005/Atom" xmlns:arxiv="http://arxiv.org/schemas/atom"
      xmlns:o="http://a9.com/-/spec/opensearch/1.1/"><o:totalResults>{0 if empty else 1}</o:totalResults>
      <o:itemsPerPage>{0 if empty else 1}</o:itemsPerPage><o:startIndex>0</o:startIndex>{entry}</feed>''').encode()


def provider_value(request):
    host, path = urlsplit(request.url).hostname, urlsplit(request.url).path
    if host == 'api.exa.ai':
        return {'results': [{'id': 'source-1', 'url': 'https://source.example.invalid/', 'title': 'Useful source', 'text': 'Original source text'}]}
    if host == 'api.firecrawl.dev':
        return ({'success': True, 'data': {'markdown': '# Useful page\nOriginal page'}} if path.endswith('scrape')
                else {'success': True, 'links': [{'url': 'https://target.example.invalid/one', 'title': 'Useful link'}]})
    if host == 'api.trello.com':
        return [{'id': 'board-1', 'name': 'Useful board', 'desc': 'Original description', 'url': 'https://trello.com/b/example', 'closed': False, 'starred': False}]
    if host and host.endswith('.salesforce.com'):
        return {'Id': RECORD, 'Name': 'Useful account', 'Active__c': False, 'Count__c': 0}
    if host == 'api.twilio.com':
        return {'sid': CALL, 'account_sid': path.split('/')[3], 'to': '+15550000001', 'from': '+15550000002', 'status': 'completed',
                'duration': '42', 'direction': 'outbound-api', 'price': '-0.01', 'start_time': None, 'end_time': None}
    if host == 'webexapis.com':
        return {'items': [{'id': 'room-1', 'title': 'Useful room', 'type': 'group', 'isPublic': False, 'isReadOnly': False}]}
    if host == 'api.plivo.com':
        return {'api_id': UUID, 'call_uuid': UUID, 'to_number': '+15550000001', 'from_number': '+15550000002',
                'call_state': 'ANSWER', 'call_duration': 42, 'call_direction': 'outbound', 'initiation_time': None, 'answer_time': None, 'total_amount': '0.01'}
    raise AssertionError('Unexpected provider: ' + request.url)


class Raw(io.BytesIO):
    def release_conn(self):
        self.close()


def response(request, value=None, *, status=200, raw=None, headers=None):
    result = requests.Response()
    result.status_code, result.url, result.request = status, request.url, request
    result.headers.update({'Content-Type': 'application/json', **(headers or {})})
    if raw is None:
        raw = atom() if urlsplit(request.url).hostname == 'export.arxiv.org' else json.dumps(provider_value(request) if value is None else value).encode()
    result.raw = Raw(raw)
    return result


def decoded(result):
    value = json.loads(result.content[0].text)
    return json.loads(value) if type(value) is str else value


class OfflineMixin:
    def setUp(self):
        super().setUp()
        self.guards = ExitStack()
        self.addCleanup(self.guards.close)
        self.guards.enter_context(patch.dict(os.environ, HOSTILE, clear=True))
        for target in ('socket.socket.connect', 'socket.socket.connect_ex', 'socket.getaddrinfo',
                       'requests.sessions.get_netrc_auth', 'requests.get', 'requests.post',
                       'httpx.HTTPTransport.handle_request', 'httpx.get', 'httpx.post'):
            self.guards.enter_context(patch(target, side_effect=AssertionError('unowned I/O forbidden')))

    def profile(self, name, settings=None):
        value = build_profile(name, operations=OPERATIONS[name], config=config(name) if settings is None else settings)
        self.addCleanup(value.close)
        return value

    def owner(self, profile):
        return inspect.unwrap(next(iter(profile.adapter._bindings.values())).entrypoint).__self__


class ConfigurationTests(OfflineMixin, unittest.TestCase):
    def test_original_donor_operation_bodies_and_real_sdk_classes(self):
        for name, cls in DONOR_CLASSES.items():
            with self.subTest(name=name):
                donor = getattr(importlib.import_module('agno.tools.' + name), cls)
                self.assertEqual(hashlib.sha256(Path(inspect.getfile(donor)).read_bytes()).hexdigest(), DONOR_HASHES[name])
                owner = self.owner(self.profile(name))
                self.assertIsInstance(owner, donor)
                for op in OPERATIONS[name]:
                    if name in ('trello', 'twilio'):
                        self.assertIsNot(getattr(type(owner), op), getattr(donor, op))
                    else:
                        self.assertIs(getattr(type(owner), op), getattr(donor, op))
                client = getattr(owner, {'exa': 'exa', 'firecrawl': 'app', 'salesforce': 'sf'}.get(name, 'client'))
                roots = {'exa': 'exa_py', 'firecrawl': 'firecrawl', 'trello': 'trello', 'salesforce': 'simple_salesforce',
                         'twilio': 'twilio', 'webex': 'webexpythonsdk', 'plivo': 'plivo', 'arxiv': 'arxiv'}
                self.assertTrue(any(base.__module__.split('.')[0] == roots[name] for base in type(client).__mro__))

    def test_configuration_is_explicit_and_describe_is_inert(self):
        for name in OPERATIONS:
            for delta in ({'timeout': True}, {'timeout': 61}, {'api_key': 'raw'}, {'base_url': 'https://other.invalid'}):
                with self.subTest(name=name, delta=delta), self.assertRaises(ProfileConfigError):
                    validate_configuration(name, operations=OPERATIONS[name], config={**config(name), **delta})
            if name != 'arxiv':
                with patch.dict(os.environ, {REFERENCE: ''}), self.assertRaises(ProfileConfigError): self.profile(name)
            profile = self.profile(name)
            self.assertTrue(all(d['effect'] == 'read' for d in profile.descriptions))
            self.assertNotIn(KEY, json.dumps(profile.descriptions))
        for account in ('bad', AUTH + '\n', 'MA' + '../'*6):
            with self.assertRaises(ProfileConfigError): self.profile('plivo', {**config('plivo'), 'auth_id': account})
        for url in ('http://selected.salesforce.com', 'https://u:p@selected.salesforce.com', 'https://selected.salesforce.com/path'):
            with self.assertRaises(ProfileConfigError): self.profile('salesforce', {**config('salesforce'), 'instance_url': url})

    def test_all_constructed_sessions_close_on_normal_and_partial_construction(self):
        from agno.tools import Toolkit
        original_init, original_close = requests.Session.__init__, requests.Session.close
        for fail in (False, True):
            for name in OPERATIONS:
                sessions, closed = [], []
                def initialize(session, *args, **kwargs):
                    original_init(session, *args, **kwargs); sessions.append(session)
                def close(session):
                    closed.append(session); original_close(session)
                with self.subTest(name=name, fail=fail), patch.object(requests.Session, '__init__', initialize), patch.object(requests.Session, 'close', close):
                    if fail:
                        with patch.object(Toolkit, '__init__', side_effect=ValueError(CANARY)), self.assertRaises(ProfileConfigError): self.profile(name)
                    else:
                        profile = self.profile(name); profile.close(); profile.close()
                    self.assertTrue(sessions)
                    self.assertEqual({id(s) for s in sessions}, {id(s) for s in closed})
                    self.assertEqual(len(sessions), len(closed))


class McpTests(OfflineMixin, unittest.IsolatedAsyncioTestCase):
    async def test_all_nine_operations_use_real_sdk_outputs_in_both_modes(self):
        for mode in ('legacy', 'auto'):
            seen = []
            def send(_, request, **kwargs):
                seen.append(request)
                self.assertEqual(kwargs['timeout'], 3)
                self.assertTrue(kwargs['stream'])
                self.assertFalse(kwargs.get('proxies'))
                self.assertNotIn(CANARY, request.url + str(request.headers) + str(request.body))
                return response(request)
            with patch.object(requests.adapters.HTTPAdapter, 'send', send):
                for name in OPERATIONS:
                    with self.subTest(mode=mode, name=name):
                        profile = self.profile(name)
                        async with Client(create_server(profile), mode=mode) as client:
                            for op in OPERATIONS[name]:
                                result = await client.call_tool(op, arguments(name, op))
                                self.assertFalse(result.is_error, (name, result))
                                value = decoded(result)
                                text = json.dumps(value)
                                expected = {'exa': 'Original source text', 'firecrawl': 'Useful', 'trello': 'Useful board',
                                            'salesforce': 'Useful account', 'twilio': 'completed', 'webex': 'Useful room',
                                            'plivo': 'ANSWER', 'arxiv': 'Original research summary'}[name]
                                self.assertIn(expected, text)
            self.assertEqual(len(seen), 9)
            trello = next(r for r in seen if urlsplit(r.url).hostname == 'api.trello.com')
            self.assertEqual(parse_qs(urlsplit(trello.url).query), {'filter': ['all'], 'key': [KEY], 'token': [TOKEN]})
            self.assertIn('max=100', next(r.url for r in seen if urlsplit(r.url).hostname == 'webexapis.com'))

    async def test_optional_absent_null_empty_and_extra_arguments_match_published_schema(self):
        cases = [('exa', 'search_exa', {'category': None}, False), ('exa', 'search_exa', {'category': ''}, False),
                 ('exa', 'search_exa', {'category': 'news'}, True), ('salesforce', 'get_record', {'fields': ''}, True),
                 ('salesforce', 'get_record', {'fields': None}, False), ('salesforce', 'get_record', {'fields': 'Name'}, False),
                 ('trello', 'list_boards', {'board_filter': None}, False), ('trello', 'list_boards', {'board_filter': ''}, False),
                 ('webex', 'list_rooms', {'room_id': 'mutation-only'}, False)]
        for name, op, delta, valid in cases:
            profile = self.profile(name)
            args = {**arguments(name), **delta}
            schema = next(d['parameters'] for d in profile.descriptions if d['name'] == op)
            self.assertEqual(Draft202012Validator(schema).is_valid(args), valid, (name, delta))
            calls = []
            with patch.object(requests.adapters.HTTPAdapter, 'send', lambda _, req, **kw: (calls.append(req), response(req))[1]):
                async with Client(create_server(profile)) as client:
                    result = await client.call_tool(op, args)
                    self.assertEqual(result.is_error, not valid, (name, delta, result))
            self.assertEqual(len(calls), int(valid))

    async def test_route_selectors_and_trailing_newlines_fail_before_provider(self):
        invalid = {'salesforce': [('sobject', '__class__'), ('record_id', RECORD + '\n'), ('record_id', '../' + RECORD)],
                   'twilio': [('call_sid', CALL + '\n'), ('call_sid', CALL + '/foreign')],
                   'plivo': [('call_uuid', UUID + '\n'), ('call_uuid', '-' * 36)],
                   'firecrawl': [('url', 'https://u:p@target.invalid'), ('url', 'file:///tmp/private')],
                   'exa': [('num_results', 21), ('num_results', True)], 'arxiv': [('num_articles', 21)]}
        for name, values in invalid.items():
            async with Client(create_server(self.profile(name))) as client:
                for key, value in values:
                    with self.subTest(name=name, key=key, value=value), patch.object(requests.adapters.HTTPAdapter, 'send', side_effect=AssertionError('must not reach provider')):
                        self.assertTrue((await client.call_tool(OPERATIONS[name][0], {**arguments(name), key: value})).is_error)

    async def test_malformed_shapes_do_not_become_empty_success_and_valid_empties_survive(self):
        bad = {'exa': [{}, {'results': {}}, {'results': [{}]}], 'firecrawl': [{}, {'success': True}, {'success': True, 'data': {}}],
               'trello': [{}, [{}]], 'salesforce': [{}, {'Id': 'foreign'}], 'twilio': [{}, {'sid': CALL, 'account_sid': 'foreign'}],
               'webex': [{}, {'items': {}}, {'items': [{}]}], 'plivo': [{}, {'call_uuid': UUID}]}
        for name, values in bad.items():
            async with Client(create_server(self.profile(name))) as client:
                for value in values:
                    with self.subTest(name=name, value=value), patch.object(requests.adapters.HTTPAdapter, 'send', lambda _, req, value=value, **kw: response(req, value)):
                        self.assertTrue((await client.call_tool(OPERATIONS[name][0], arguments(name))).is_error)
        for name, value in [('exa', {'results': []}), ('trello', []), ('webex', {'items': []})]:
            with patch.object(requests.adapters.HTTPAdapter, 'send', lambda _, req, value=value, **kw: response(req, value)):
                async with Client(create_server(self.profile(name))) as client:
                    self.assertFalse((await client.call_tool(OPERATIONS[name][0], arguments(name))).is_error)
        with patch.object(requests.adapters.HTTPAdapter, 'send', lambda _, req, **kw: response(req, raw=atom(True))):
            async with Client(create_server(self.profile('arxiv'))) as client:
                self.assertEqual(decoded(await client.call_tool(OPERATIONS['arxiv'][0], arguments('arxiv'))), [])

    async def test_webex_pagination_fails_explicitly_without_following_or_partial_result(self):
        for link in ('<https://webexapis.com/v1/rooms?before=next>; rel="next"', '<https://foreign.invalid/>; rel="next"'):
            seen = []
            with patch.object(requests.adapters.HTTPAdapter, 'send', lambda _, req, **kw: (seen.append(req), response(req, headers={'Link': link}))[1]):
                async with Client(create_server(self.profile('webex'))) as client:
                    result = await client.call_tool('list_rooms', {})
                    self.assertTrue(result.is_error)
                    self.assertNotIn('Useful room', str(result))
            self.assertEqual(len(seen), 1)

    async def test_arxiv_malformed_feed_and_skipped_entries_are_errors(self):
        payloads = [b'<not-a-feed/>', b'', b'<feed', atom().replace(b'<title>Useful quantum paper</title>', b''),
                    atom().replace(b'<published>2026-01-01T00:00:00Z</published>', b''),
                    atom().replace(b'<o:totalResults>1</o:totalResults>', b'<o:totalResults>bad</o:totalResults>'),
                    atom().replace(b'<o:totalResults>1</o:totalResults>', b'')]
        async with Client(create_server(self.profile('arxiv'))) as client:
            for payload in payloads:
                with self.subTest(payload=payload[:40]), patch.object(requests.adapters.HTTPAdapter, 'send', lambda _, req, payload=payload, **kw: response(req, raw=payload)):
                    self.assertTrue((await client.call_tool('search_arxiv_and_return_articles', arguments('arxiv'))).is_error)

    async def test_failures_do_not_retry_redirect_or_leak_secrets_and_recovery_works(self):
        for name in OPERATIONS:
            async with Client(create_server(self.profile(name))) as client:
                for status, raw in ((302, b'redirect'), (401, CANARY.encode()), (429, CANARY.encode()), (500, CANARY.encode()),
                                    (200, b'{invalid'), (200, b'x' * 1_048_577)):
                    seen, responses = [], []
                    def send(_, req, **kw):
                        seen.append(req); result = response(req, status=status, raw=raw, headers={'Location': 'https://foreign.invalid', 'Retry-After': '100'})
                        responses.append(result); return result
                    with self.subTest(name=name, status=status, size=len(raw)), patch.object(requests.adapters.HTTPAdapter, 'send', send):
                        result = await client.call_tool(OPERATIONS[name][0], arguments(name))
                        self.assertTrue(result.is_error)
                        self.assertNotIn(KEY, str(result)); self.assertNotIn(TOKEN, str(result)); self.assertNotIn(CANARY, str(result))
                        self.assertEqual(len(seen), 1)
                        self.assertTrue(responses[0].raw.closed)
                with patch.object(requests.adapters.HTTPAdapter, 'send', lambda _, req, **kw: response(req)):
                    self.assertFalse((await client.call_tool(OPERATIONS[name][0], arguments(name))).is_error, name)

    async def test_transport_exception_is_sanitized_in_mcp_and_stderr(self):
        for name in OPERATIONS:
            output = io.StringIO()
            with patch.object(requests.adapters.HTTPAdapter, 'send', side_effect=requests.ConnectionError(KEY + TOKEN + CANARY)), redirect_stdout(output), redirect_stderr(output):
                async with Client(create_server(self.profile(name))) as client:
                    result = await client.call_tool(OPERATIONS[name][0], arguments(name))
                    self.assertTrue(result.is_error)
                    self.assertNotIn(KEY, str(result)); self.assertNotIn(TOKEN, str(result)); self.assertNotIn(CANARY, str(result))
            for secret in (KEY, TOKEN, CANARY): self.assertNotIn(secret, output.getvalue())

    async def test_concurrent_accounts_keep_credentials_and_resource_retirement_separate(self):
        for name in OPERATIONS:
            first = self.profile(name)
            settings = {**config(name), **{'twilio': {'account_sid': 'AC' + '9' * 32},
                                          'plivo': {'auth_id': 'MA' + '9' * 18},
                                          'salesforce': {'instance_url': 'https://second.salesforce.com'}}.get(name, {})}
            with patch.dict(os.environ, {REFERENCE: 'second-key', TOKEN_REF: 'second-token'}): second = self.profile(name, settings)
            seen = []
            with patch.object(requests.adapters.HTTPAdapter, 'send', lambda _, req, **kw: (seen.append(req), response(req))[1]):
                async with Client(create_server(first)) as a, Client(create_server(second)) as b:
                    results = await asyncio.gather(a.call_tool(OPERATIONS[name][0], arguments(name)), b.call_tool(OPERATIONS[name][0], arguments(name)))
                    self.assertTrue(all(not result.is_error for result in results), name)
                first.close()
                async with Client(create_server(second)) as b:
                    self.assertFalse((await b.call_tool(OPERATIONS[name][0], arguments(name))).is_error)
            if name != 'arxiv':
                self.assertNotEqual((seen[0].url, seen[0].headers), (seen[1].url, seen[1].headers), name)
                self.assertIn((seen[2].url, seen[2].headers), [(req.url, req.headers) for req in seen[:2]], name)
                if name == 'trello':
                    self.assertEqual(parse_qs(urlsplit(seen[2].url).query)['token'], ['second-token'])
                    for req in seen:
                        query = parse_qs(urlsplit(req.url).query)
                        self.assertIn((query['key'][0], query['token'][0]), ((KEY, TOKEN), ('second-key', 'second-token')))
                elif name == 'exa':
                    self.assertEqual(seen[2].headers['x-api-key'], 'second-key')
                elif name not in ('twilio', 'plivo'):
                    self.assertEqual(seen[2].headers['Authorization'], 'Bearer second-key')
                else:
                    import base64
                    self.assertTrue(base64.b64decode(seen[2].headers['Authorization'].split()[1]).decode().endswith(':second-key'))

    async def test_salesforce_fifteen_character_id_accepts_its_eighteen_character_form(self):
        for returned, expected in ((RECORD, True), (RECORD + 'AAA', True), ('002' + RECORD[3:] + 'AAA', False)):
            with patch.object(requests.adapters.HTTPAdapter, 'send', lambda _, req, **kw: response(req, {'Id': returned, 'Name': 'Useful account'})):
                async with Client(create_server(self.profile('salesforce'))) as client:
                    result = await client.call_tool('get_record', arguments('salesforce'))
                    self.assertEqual(result.is_error, not expected)
                    if expected: self.assertEqual(decoded(result)['Id'], returned)

    async def test_trello_optional_metadata_preserves_true_false_null_and_absence(self):
        from trello.board import Board
        base = {'id': 'board-1', 'name': 'Useful board', 'url': 'https://trello.com/b/example', 'closed': False}
        variants = [
            ({'desc': 'Details', 'starred': True, 'idOrganization': 'selected-org'},
             {'description': 'Details', 'starred': True, 'organization': 'selected-org'}),
            ({'desc': '', 'starred': False, 'idOrganization': ''},
             {'description': '', 'starred': False, 'organization': ''}),
            ({'desc': None, 'starred': None, 'idOrganization': None},
             {'description': None, 'starred': None, 'organization': None}),
            ({}, {}),
        ]
        profile = self.profile('trello')
        owner = self.owner(profile)
        with patch.object(owner.client, 'fetch_json', wraps=owner.client.fetch_json) as fetch:
            with patch.object(Board, 'from_json', wraps=Board.from_json) as parse:
                async with Client(create_server(profile)) as client:
                    for supplied, optional in variants:
                        with self.subTest(supplied=supplied), patch.object(requests.adapters.HTTPAdapter, 'send',
                                lambda _, req, **kw: response(req, [{**base, **supplied}])):
                            result = await client.call_tool('list_boards', {'board_filter': 'starred'})
                            self.assertFalse(result.is_error, result)
                            self.assertEqual(decoded(result), {'filter_used': 'starred', 'total_boards': 1,
                                                              'boards': [{**base, **optional}]})
                self.assertEqual(fetch.call_count, 4)
                self.assertEqual(parse.call_count, 4)

    async def test_twilio_valid_utc_and_missing_null_times_keep_real_sdk_projection(self):
        missing = object()
        cases = [(missing, 'None'), (None, 'None'),
                 ('Fri, 02 Oct 2026 10:15:00 +0000', '2026-10-02 10:15:00+00:00'),
                 ('Fri, 02 Oct 2026 10:15:00 GMT', '2026-10-02 10:15:00+00:00'),
                 ('Thu, 29 Feb 2024 23:59:59 +0000', '2024-02-29 23:59:59+00:00'),
                 ('Fri, 01 Jan 0100 00:00:00 +0000', '0100-01-01 00:00:00+00:00')]
        async with Client(create_server(self.profile('twilio'))) as client:
            for key in ('start_time', 'end_time'):
                for supplied, expected in cases:
                    def send(_, req, **kw):
                        value = provider_value(req)
                        if supplied is missing: value.pop(key)
                        else: value[key] = supplied
                        return response(req, value)
                    with self.subTest(key=key, supplied=supplied), patch.object(requests.adapters.HTTPAdapter, 'send', send):
                        result = await client.call_tool('get_call_details', {'call_sid': CALL})
                        self.assertFalse(result.is_error, result)
                        self.assertEqual(decoded(result)[key], expected)
                        self.assertEqual(decoded(result)['from'], '+15550000002')

    async def test_twilio_present_invalid_times_fail_before_sdk_loss(self):
        valid = 'Fri, 02 Oct 2026 10:15:00 +0000'
        invalid = ['', 'not a timestamp', ' ', 'Fri, 31 Feb 2026 10:15:00 +0000',
                   'Fri, 02 Oct 2026 10:15:00', 'Fri, 02 Oct 2026 10:15:00 +0100',
                   'Fri, 02 Oct 2026 10:15:00 -0000', 'Fri, 02 Oct 2026 10:15:00 UTC',
                   'Mon, 01 Jan 0001 00:00:00 +0000', 'Sun, 01 Jan 0068 00:00:00 +0000',
                   'Tue, 01 Jan 0069 00:00:00 +0000', 'Thu, 01 Jan 0099 00:00:00 +0000',
                   'Thu, 02 Oct 2026 10:15:00 +0000', 'Fri, 02 Oct 2026 24:00:00 +0000',
                   'Fri, 02 Oct 2026 10:15:60 +0000', 'Fri, 2 Oct 2026 10:15:00 +0000',
                   'Fri, 02 oct 2026 10:15:00 +0000', ' ' + valid, '(date) ' + valid,
                   valid.replace(', ', ',\t'), valid.replace(', ', ',  '),
                   *[valid + suffix for suffix in ('\nprovider-canary', '\n', '\r\n', ' provider-canary',
                                                 ' ', '\t', ' (UTC)', '\x00', '\u00a0', '\u2028')],
                   False, 0, [], {}]
        async with Client(create_server(self.profile('twilio'))) as client:
            for key in ('start_time', 'end_time'):
                for value in invalid:
                    seen = []
                    def send(_, req, **kw):
                        seen.append(req)
                        return response(req, {**provider_value(req), key: value})
                    with self.subTest(key=key, value=value), patch.object(requests.adapters.HTTPAdapter, 'send', send):
                        result = await client.call_tool('get_call_details', {'call_sid': CALL})
                        self.assertTrue(result.is_error, result)
                        self.assertEqual(len(seen), 1)
                        self.assertNotIn('not a timestamp', str(result))

    async def test_provider_collection_caps_are_explicit_errors(self):
        cases = [('exa', 'search_exa', {'results': [{'id': 'id', 'url': 'https://source.invalid'}] * 21}),
                 ('firecrawl', 'map_website', {'success': True, 'links': [{'url': 'https://source.invalid'}] * 101}),
                 ('trello', 'list_boards', [{'id': 'id', 'name': 'board', 'url': 'https://source.invalid', 'closed': False}] * 101),
                 ('webex', 'list_rooms', {'items': [{'id': 'id', 'title': 'room', 'type': 'group'}] * 101})]
        for name, op, value in cases:
            with patch.object(requests.adapters.HTTPAdapter, 'send', lambda _, req, **kw: response(req, value)):
                async with Client(create_server(self.profile(name))) as client:
                    self.assertTrue((await client.call_tool(op, arguments(name))).is_error)

    async def test_cancellation_joins_held_body_before_retirement(self):
        for name in OPERATIONS:
            entered, release, closed = threading.Event(), threading.Event(), threading.Event()
            class Held(Raw):
                def read(self, *args):
                    entered.set(); release.wait(5); return super().read(*args)
                def close(self):
                    closed.set(); return super().close()
            def send(_, req, **kw):
                value = response(req); value.raw = Held(value.raw.getvalue()); return value
            profile = self.profile(name)
            try:
                with patch.object(requests.adapters.HTTPAdapter, 'send', send):
                    async with Client(create_server(profile)) as client:
                        task = asyncio.create_task(client.call_tool(OPERATIONS[name][0], arguments(name)))
                        self.assertTrue(await asyncio.to_thread(entered.wait, 2), name)
                        task.cancel()
                        await asyncio.sleep(.03)
                        self.assertFalse(closed.is_set())
                        release.set()
                        with self.assertRaises(asyncio.CancelledError): await task
                        for _ in range(100):
                            if closed.is_set(): break
                            await asyncio.sleep(.01)
                        self.assertTrue(closed.is_set(), name)
            finally: release.set()
            profile.close()

    async def test_pre_cancelled_calls_never_enter_transport(self):
        for name in OPERATIONS:
            with patch.object(requests.adapters.HTTPAdapter, 'send', side_effect=AssertionError('unexpected provider call')):
                async with Client(create_server(self.profile(name))) as client:
                    task = asyncio.create_task(client.call_tool(OPERATIONS[name][0], arguments(name)))
                    task.cancel()
                    with self.assertRaises(asyncio.CancelledError): await task


if __name__ == '__main__':
    unittest.main()
