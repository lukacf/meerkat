"""Six read profiles: real donor/SDK execution with local mocked provider I/O."""
from __future__ import annotations

import asyncio
from contextlib import ExitStack, redirect_stderr, redirect_stdout
from copy import deepcopy
import hashlib
import importlib
import inspect
import io
import json
import logging
import os
from pathlib import Path
import threading
import unittest
from unittest.mock import patch
from urllib.parse import parse_qs, urlsplit

import requests
from jsonschema import Draft202012Validator
from mcp import Client
from meerkat_toolkit.adapters.agno_profiles import build_profile, validate_configuration, ProfileConfigError
from meerkat_toolkit.adapters.agno_mcp import create_server

KEY = 'selected-service-key'
CANARY = 'foreign-account-provider-canary'
KEY_REF = 'HTTP_READER_SELECTED_KEY'
ACCESS_REF = 'HTTP_READER_ACCESS'
SESSION_REF = 'HTTP_READER_SESSION'
ACCESS = 'AKIA' + 'A' * 16
TOKEN = 'selected-session-token'
OPS = {'reddit': ['get_top_posts', 'get_subreddit_stats', 'get_user_info'],
       'x': ['get_user_info', 'search_posts'], 'youtube': ['get_youtube_video_data'],
       'oxylabs': ['search_google', 'get_amazon_product'], 'wikipedia': ['search_wikipedia'],
       'aws-lambda': ['list_functions']}
HASHES = {'reddit': '60ddb69ea8cb18c09f8a8633430ee3495482c5b2df24f82a20d604dd9c4880ea',
          'x': '8a5ee602606c31197162a1b07ec7f536150946b3d8fa78de8b603f7420551c1e',
          'youtube': '292519954dcb91d5c6b2459e6f1cde55401cc1f0bbaaf07af946706049dcef09',
          'oxylabs': 'ff721fd8703a63352935625bf9675222403f4fd101da89f7cf46ca01ebdca3a0',
          'wikipedia': 'f243f9022e6325bc42218b765657eaab3a6ae228b45805464dc958f9178b9a0e',
          'aws_lambda': '6710f0bd41683e439e5945499fb785f2da33100877615d4230b9f6faaeeac3db'}
HOSTILE = {KEY_REF: KEY, ACCESS_REF: ACCESS, SESSION_REF: TOKEN,
           **{k: CANARY for k in ('REDDIT_CLIENT_ID', 'REDDIT_CLIENT_SECRET', 'X_BEARER_TOKEN',
              'X_CONSUMER_KEY', 'X_CONSUMER_SECRET', 'X_ACCESS_TOKEN', 'X_ACCESS_TOKEN_SECRET',
              'OXYLABS_USERNAME', 'OXYLABS_PASSWORD', 'AWS_ACCESS_KEY_ID', 'AWS_SECRET_ACCESS_KEY',
              'AWS_SESSION_TOKEN', 'AWS_PROFILE', 'AWS_REGION', 'AWS_DATA_PATH', 'praw_site',
              'praw_oauth_url', 'praw_reddit_url', 'praw_username', 'praw_password')},
           'HTTPS_PROXY': 'http://foreign.invalid', 'NETRC': '/nonexistent/foreign',
           'AWS_ENDPOINT_URL': 'https://foreign.invalid', 'AWS_ENDPOINT_URL_LAMBDA': 'https://foreign.invalid',
           'AWS_SHARED_CREDENTIALS_FILE': '/nonexistent/foreign', 'AWS_CONFIG_FILE': '/nonexistent/foreign'}


def config(name):
    return {'timeout': 3, **({} if name in ('youtube', 'wikipedia') else {'api_key_env': KEY_REF}), **{
        'reddit': {'user_agent': 'Meerkat selected test contact'}, 'x': {'include_post_metrics': True},
        'oxylabs': {'username': 'selected-user'}, 'wikipedia': {'language': 'en'},
        'aws-lambda': {'access_key_id_env': ACCESS_REF, 'session_token_env': SESSION_REF, 'region': 'us-east-1'},
    }.get(name, {})}


def arguments(name, op=None):
    if name == 'reddit': return {'username': 'ada'} if op == 'get_user_info' else {'subreddit': 'science'}
    if name == 'x': return {'username': 'ada'} if op == 'get_user_info' else {'query': 'useful tests', 'max_results': 10}
    return {'youtube': {'url': 'https://www.youtube.com/watch?v=abcdefghijk'},
            'oxylabs': {'asin': 'B07FZ8S74R'} if op == 'get_amazon_product' else {'query': 'useful research'},
            'wikipedia': {'query': 'Ada Lovelace'}, 'aws-lambda': {}}[name]


def post():
    return {'id': 'abc', 'title': 'Useful research', 'score': 17, 'url': 'https://example.invalid/paper',
            'selftext': 'Original paper', 'author': 'ada', 'permalink': '/r/science/comments/abc',
            'created_utc': 1760000000.0, 'subreddit': 'science', 'subreddit_name_prefixed': 'r/science'}


def provider(request):
    parsed = urlsplit(request.url)
    query = parse_qs(parsed.query, keep_blank_values=True)
    if parsed.hostname == 'oauth.reddit.com':
        if parsed.path.endswith('/top'):
            return {'kind': 'Listing', 'data': {'children': [{'kind': 't3', 'data': post()}], 'after': None, 'before': None}}
        if parsed.path.startswith('/r/'):
            return {'kind': 't5', 'data': {'display_name': 'science', 'subscribers': 42, 'active_user_count': 3,
                    'description': 'Useful science', 'public_description': 'Research forum', 'created_utc': 1760000000, 'over18': False}}
        return {'kind': 't2', 'data': {'name': 'ada', 'comment_karma': 42, 'link_karma': 7, 'is_mod': False,
                                     'is_gold': True, 'is_employee': False, 'created_utc': 1760000000}}
    if parsed.hostname == 'api.twitter.com':
        user = {'id': '123', 'name': 'Ada', 'username': 'ada', 'verified': False, 'description': 'Useful research',
                'public_metrics': {'followers_count': 42, 'following_count': 3, 'tweet_count': 7}}
        if parsed.path.startswith('/2/users'): return {'data': user}
        return {'data': [{'id': '456', 'text': 'Useful paper', 'author_id': '123', 'edit_history_tweet_ids': ['456'], 'created_at': '2026-01-02T03:04:05.000Z',
                          'public_metrics': {'retweet_count': 0, 'reply_count': 1, 'like_count': 2, 'quote_count': 0}}],
                'meta': {'result_count': 1}, 'includes': {'users': [user]}}
    if parsed.hostname == 'www.youtube.com':
        return {'title': 'Useful video', 'author_name': 'Ada', 'author_url': 'https://www.youtube.com/@ada', 'type': 'video',
                'height': 150, 'width': 200, 'version': '1.0', 'provider_name': 'YouTube', 'provider_url': 'https://www.youtube.com/',
                'thumbnail_url': 'https://i.ytimg.com/vi/abcdefghijk/hqdefault.jpg'}
    if parsed.hostname == 'realtime.oxylabs.io':
        body = json.loads(request.body)
        content = ({'results': {'organic': [{'title': 'Useful result', 'url': 'https://example.invalid/research', 'desc': 'Original research', 'pos': 1}]}}
                   if body['source'] == 'google_search' else
                   {'title': 'Useful product', 'price': 12.5, 'currency': 'USD', 'rating': 4.5, 'reviews_count': 42,
                    'url': 'https://amazon.com/dp/B07FZ8S74R', 'description': 'Original description', 'stock': 'In stock',
                    'brand': 'Example', 'images': ['image'], 'bullet_points': ['feature']})
        return {'results': [{'status_code': 200, 'content': content}]}
    if parsed.hostname and parsed.hostname.endswith('.wikipedia.org'):
        item = {'pageid': 42, 'title': 'Ada Lovelace', 'fullurl': f'https://{parsed.hostname}/wiki/Ada_Lovelace'}
        if query.get('prop') == ['extracts|info']: item['extract'] = 'Ada was a mathematician. Useful original summary.'
        return {'query': {'pages': {'42': item}}}
    if parsed.hostname and parsed.hostname.startswith('lambda.'):
        return {'Functions': [{'FunctionName': 'UsefulFunction'}]}
    raise AssertionError('Unexpected selected route')


class Raw(io.BytesIO):
    def release_conn(self): self.close()


def response(request, value=None, *, raw=None, status=200, headers=None):
    result = requests.Response()
    result.request, result.url, result.status_code = request, request.url, status
    result.headers.update({'Content-Type': 'application/json', **(headers or {})})
    result.raw = Raw(raw if raw is not None else json.dumps(provider(request) if value is None else value).encode())
    return result


def decoded(result):
    try: value = json.loads(result.content[0].text)
    except json.JSONDecodeError: return result.content[0].text
    return json.loads(value) if type(value) is str and value[:1] in ('{', '[') else value


class Offline:
    def setUp(self):
        super().setUp()
        self.stack = ExitStack()
        self.addCleanup(self.stack.close)
        self.stack.enter_context(patch.dict(os.environ, HOSTILE, clear=True))
        for target in ('socket.socket.connect', 'socket.socket.connect_ex', 'socket.getaddrinfo',
                       'requests.sessions.get_netrc_auth', 'requests.get', 'requests.post', 'urllib.request.urlopen',
                       'httpx.HTTPTransport.handle_request'):
            self.stack.enter_context(patch(target, side_effect=AssertionError('Unowned I/O forbidden')))

    def profile(self, name, settings=None):
        result = build_profile(name, operations=OPS[name], config=config(name) if settings is None else settings)
        self.addCleanup(result.close)
        return result

    def owner(self, profile):
        return inspect.unwrap(next(iter(profile.adapter._bindings.values())).entrypoint).__self__


class ConfigurationTests(Offline, unittest.TestCase):
    def test_exact_donor_bytes_real_sdk_types_and_inherited_operations(self):
        tools = Path(importlib.import_module('agno.tools').__file__).parent
        for module, digest in HASHES.items():
            self.assertEqual(hashlib.sha256((tools / (module + '.py')).read_bytes()).hexdigest(), digest)
        names = {'reddit': ('reddit', 'RedditTools', 'reddit', 'praw'), 'x': ('x', 'XTools', 'client', 'tweepy'),
                 'oxylabs': ('oxylabs', 'OxylabsTools', 'client', 'oxylabs'),
                 'aws-lambda': ('aws_lambda', 'AWSLambdaTools', 'client', 'botocore')}
        for name, (module, klass, field, sdk) in names.items():
            owner = self.owner(self.profile(name))
            donor = getattr(importlib.import_module('agno.tools.' + module), klass)
            self.assertIsInstance(owner, donor)
            self.assertTrue(any(base.__module__.split('.')[0] == sdk for base in type(getattr(owner, field)).__mro__))
            for op in OPS[name]:
                if name == 'x' and op == 'search_posts': self.assertIsNot(getattr(type(owner), op), getattr(donor, op))
                else: self.assertIs(getattr(type(owner), op), getattr(donor, op))

    def test_explicit_closed_configuration_and_inert_descriptions(self):
        for name in OPS:
            for delta in ({'timeout': True}, {'timeout': 61}, {'base_url': 'https://foreign.invalid'}, {'api_key': KEY}):
                with self.subTest(name=name, delta=delta), self.assertRaises(ProfileConfigError):
                    validate_configuration(name, operations=OPS[name], config={**config(name), **delta})
            p = self.profile(name)
            self.assertTrue(all(d['effect'] == 'read' for d in p.descriptions))
            self.assertNotIn(KEY, json.dumps(p.descriptions))
        for name, key, bad in [('reddit', 'user_agent', 'short'), ('wikipedia', 'language', 'en.evil'),
                               ('oxylabs', 'username', 'bad:password'), ('aws-lambda', 'region', 'us-east-1\n')]:
            with self.assertRaises(ProfileConfigError): self.profile(name, {**config(name), key: bad})

    def test_every_requests_session_closes_once_on_partial_or_normal_construction(self):
        from agno.tools import Toolkit
        original_init, original_close = requests.Session.__init__, requests.Session.close
        for name in OPS:
            for failure in (False, True):
                made, closed = [], []
                def initialize(s, *a, **kw): original_init(s, *a, **kw); made.append(s)
                def close(s): closed.append(s); original_close(s)
                with self.subTest(name=name, failure=failure), patch.object(requests.Session, '__init__', initialize), patch.object(requests.Session, 'close', close):
                    if failure:
                        with patch.object(Toolkit, '__init__', side_effect=ValueError(CANARY)), self.assertRaises(ProfileConfigError): self.profile(name)
                    else:
                        p = self.profile(name); p.close(); p.close()
                    # Tweepy import creates a legacy process-global API session.
                    # It is never borrowed/modified by this profile, and has no I/O.
                    import tweepy
                    owned = [s for s in made if s is not tweepy.api.session]
                    self.assertEqual({id(s) for s in owned}, {id(s) for s in closed})
                    self.assertEqual(len(owned), len(closed))

    def test_oauthlib_successor_real_oauth1_request_serialization(self):
        import oauthlib
        from requests_oauthlib import OAuth1
        self.assertEqual(oauthlib.__version__, '3.3.1')
        auth = OAuth1('key', 'secret', 'token', 'token-secret', nonce='fixednonce', timestamp='1234567890')
        request = requests.Request('GET', 'https://api.trello.com/1/members/me/boards', params={'filter': 'all'}, auth=auth).prepare()
        header = request.headers['Authorization'].decode()
        self.assertIn('oauth_signature_method="HMAC-SHA1"', header)
        self.assertIn('oauth_consumer_key="key"', header)
        self.assertNotIn('token-secret', header)
        from oauthlib.oauth1 import Client as OAuthClient
        _, expected, _ = OAuthClient('key', client_secret='secret', resource_owner_key='token', resource_owner_secret='token-secret', nonce='fixednonce', timestamp='1234567890').sign(request.url)
        self.assertEqual(header, expected['Authorization'])
        self.assertEqual(header, 'OAuth oauth_nonce="fixednonce", oauth_timestamp="1234567890", oauth_version="1.0", oauth_signature_method="HMAC-SHA1", oauth_consumer_key="key", oauth_token="token", oauth_signature="CawQeflQ390IG4kFdHxGvtat7SE%3D"')

    def test_real_sdk_models_and_serializers_execute(self):
        import praw, tweepy
        from oxylabs.sources.response import Response
        with patch.object(requests.adapters.HTTPAdapter, 'send', lambda _, req, **kw: response(req)):
            reddit = self.owner(self.profile('reddit')).reddit
            self.assertIsInstance(next(reddit.subreddit('science').top(limit=1)), praw.models.Submission)
            user = self.owner(self.profile('x')).client.get_user(username='ada', user_fields=['description', 'public_metrics']).data
            self.assertIsInstance(user, tweepy.User)
            oxy = self.owner(self.profile('oxylabs')).client.google.scrape_search(query='useful', domain='com', parse=True)
            self.assertIsInstance(oxy, Response)
            owner = self.owner(self.profile('aws-lambda'))
            serializer = owner.client._serializer
            with patch.object(serializer, 'serialize_to_request', wraps=serializer.serialize_to_request) as serialized:
                self.assertIn('UsefulFunction', owner.list_functions())
                self.assertEqual(serialized.call_count, 1)
                self.assertEqual(serialized.call_args.args[0], {'MaxItems': 50})

    def test_reddit_and_aws_ignore_sdk_home_configuration_and_credentials(self):
        import praw.config, botocore.configloader, botocore.session
        original = botocore.configloader.raw_config_parse
        seen = []
        def read(path, *args, **kwargs):
            seen.append(path)
            self.assertEqual(path, '/dev/null')
            return original(path, *args, **kwargs)
        with patch.object(praw.config.Config, '_load_config', side_effect=AssertionError('No user config')),             patch.object(botocore.configloader, 'raw_config_parse', read),             patch.object(botocore.session.Session, 'get_credentials', side_effect=AssertionError('No credential discovery')):
            self.profile('reddit')
            self.profile('aws-lambda')
        self.assertTrue(seen)

    def test_lambda_wire_signature_matches_real_botocore_sigv4(self):
        from datetime import datetime
        from botocore.auth import SigV4Auth
        from botocore.awsrequest import AWSRequest
        from botocore.credentials import Credentials
        seen = []
        def send(_, req, **kw): seen.append(req); return response(req)
        with patch.object(requests.adapters.HTTPAdapter, 'send', send):
            self.assertIn('UsefulFunction', self.owner(self.profile('aws-lambda')).list_functions())
        request = seen[0]
        names = request.headers['Authorization'].split('SignedHeaders=')[1].split(',')[0].split(';')
        unsigned = {name: request.headers[name] for name in names if name != 'host'}
        expected = AWSRequest(method='GET', url=request.url, headers=unsigned)
        instant = datetime.strptime(request.headers['X-Amz-Date'], '%Y%m%dT%H%M%SZ')
        with patch('botocore.auth.get_current_datetime', return_value=instant):
            SigV4Auth(Credentials(ACCESS, KEY, TOKEN), 'lambda', 'us-east-1').add_auth(expected)
        self.assertEqual(request.headers['Authorization'], expected.headers['Authorization'])


class McpTests(Offline, unittest.IsolatedAsyncioTestCase):
    async def test_all_ten_operations_useful_output_and_exact_owned_io_both_modes(self):
        for mode in ('legacy', 'auto'):
            seen, raws = [], []
            def send(_, request, **kw):
                self.assertEqual(kw['timeout'], 3)
                self.assertTrue(kw['stream'])
                self.assertFalse(kw.get('proxies'))
                self.assertNotIn(CANARY, request.url + str(request.headers) + str(request.body))
                seen.append(request)
                result = response(request); raws.append(result.raw); return result
            with patch.object(requests.adapters.HTTPAdapter, 'send', send):
                for name in OPS:
                    async with Client(create_server(self.profile(name)), mode=mode) as client:
                        for op in OPS[name]:
                            with self.subTest(name=name, op=op, mode=mode):
                                result = await client.call_tool(op, arguments(name, op))
                                self.assertFalse(result.is_error, result)
                                value = decoded(result)
                                self.assertTrue(value)
                                if op == 'search_posts': self.assertEqual(value['posts'][0]['url'], 'https://x.com/ada/status/456')
                                if op == 'get_user_info': self.assertIn('ada', json.dumps(value))
                                if op == 'get_youtube_video_data': self.assertEqual(value['title'], 'Useful video')
                                if op == 'list_functions': self.assertEqual(value, 'Available Lambda functions: UsefulFunction')
            self.assertEqual(len(seen), 11)
            self.assertTrue(all(raw.closed for raw in raws))
            for req in seen:
                host = urlsplit(req.url).hostname
                if host == 'oauth.reddit.com': self.assertEqual(req.headers['Authorization'], 'bearer ' + KEY)
                if host == 'api.twitter.com': self.assertEqual(req.headers['Authorization'], 'Bearer ' + KEY)
                if host == 'realtime.oxylabs.io': self.assertEqual(req.headers['Authorization'], requests.auth._basic_auth_str('selected-user', KEY))
                if host and host.startswith('lambda.'):
                    self.assertIn('Credential=' + ACCESS + '/', req.headers['Authorization'])
                    self.assertIn('/us-east-1/lambda/aws4_request', req.headers['Authorization'])
                    self.assertEqual(req.headers['X-Amz-Security-Token'], TOKEN)

    async def test_malformed_provider_envelopes_never_become_empty_success(self):
        cases = {'reddit': [{}, {'kind': 'Listing', 'data': {'children': {}}}, {'kind': 'Listing', 'data': {'children': [{}]}}],
                 'x': [{}, {'data': None}, {'data': {'id': '1'}}], 'youtube': [{}, {'title': 'x'}],
                 'oxylabs': [{}, {'results': None}, {'results': [{}]},
                             {'results': [], 'job': {'status': 'faulted'}},
                             {'results': [], 'job': {'status': 'pending'}},
                             {'results': [], 'job': {'status': None}},
                             {'results': [], 'job': None}],
                 'wikipedia': [{}, {'query': {'pages': {}}}, {'query': {'pages': {'1': {'missing': ''}}}}],
                 'aws-lambda': [{}, {'Functions': {}}, {'Functions': [{}]}, {'Functions': [], 'NextMarker': 'more'}]}
        for name, values in cases.items():
            async with Client(create_server(self.profile(name))) as client:
                for value in values:
                    with self.subTest(name=name, value=value), patch.object(requests.adapters.HTTPAdapter, 'send', lambda _, req, v=value, **kw: response(req, v)):
                        result = await client.call_tool(OPS[name][0], arguments(name, OPS[name][0]))
                        self.assertTrue(result.is_error, result)

    async def test_valid_empty_collections_and_deleted_reddit_author_are_preserved(self):
        cases = [('reddit', 'get_top_posts', {'kind': 'Listing', 'data': {'children': [], 'after': None, 'before': None}}),
                 ('x', 'search_posts', {'meta': {'result_count': 0}}), ('oxylabs', 'search_google', {'results': []}),
                 ('oxylabs', 'search_google', {'results': [], 'job': {'status': 'done'}}),
                 ('aws-lambda', 'list_functions', {'Functions': []})]
        for name, op, value in cases:
            with self.subTest(name=name), patch.object(requests.adapters.HTTPAdapter, 'send', lambda _, req, **kw: response(req, value)):
                async with Client(create_server(self.profile(name))) as client:
                    self.assertFalse((await client.call_tool(op, arguments(name, op))).is_error)
        value = {'kind': 'Listing', 'data': {'children': [{'kind': 't3', 'data': {**post(), 'author': '[deleted]'}}], 'after': None, 'before': None}}
        with patch.object(requests.adapters.HTTPAdapter, 'send', lambda _, req, **kw: response(req, value)):
            async with Client(create_server(self.profile('reddit'))) as client:
                result = await client.call_tool('get_top_posts', {'subreddit': 'science'})
                self.assertEqual(decoded(result)['top_posts'][0]['author'], 'None')

    async def test_input_schema_absent_null_extra_and_newline(self):
        invalid = [('reddit', 'get_top_posts', {'limit': True}), ('reddit', 'get_top_posts', {'limit': 26}),
                   ('reddit', 'get_top_posts', {'subreddit': 'science\n'}), ('reddit', 'get_top_posts', {'time_filter': None}),
                   ('x', 'search_posts', {'max_results': 9}), ('x', 'search_posts', {'query': ''}),
                   ('x', 'get_user_info', {'username': 'ada\n'}), ('youtube', 'get_youtube_video_data', {'url': None}),
                   ('oxylabs', 'get_amazon_product', {'asin': 'B07FZ8S74R\n'}), ('oxylabs', 'search_google', {'domain_code': 'evil/'}),
                   ('wikipedia', 'search_wikipedia', {'query': 'One|Two'}), ('aws-lambda', 'list_functions', {'function_name': 'write-only'})]
        for name, op, delta in invalid:
            p = self.profile(name); args = {**arguments(name, op), **delta}
            schema = next(d['parameters'] for d in p.descriptions if d['name'] == op)
            self.assertFalse(Draft202012Validator(schema).is_valid(args), (name, args))
            with patch.object(requests.adapters.HTTPAdapter, 'send', side_effect=AssertionError('No provider call')):
                async with Client(create_server(p)) as client: self.assertTrue((await client.call_tool(op, args)).is_error)

    async def test_youtube_full_authority_path_query_selection(self):
        invalid = ['http://youtube.com/watch?v=abcdefghijk', 'https://youtube.com.evil/watch?v=abcdefghijk',
                   'https://user@youtube.com/watch?v=abcdefghijk', 'https://youtube.com:443/watch?v=abcdefghijk',
                   'https://youtu.be/abcdefghijk/extra', 'https://youtube.com/embed/abcdefghijk/extra',
                   'https://youtube.com/watch?v=abcdefghijk&v=lmnopqrstuv', 'https://youtube.com/watch?v=abcdefghijk#fragment',
                   'https://youtube.com/watch?v=%61bcdefghijk', 'https://youtube.com/redirect?url=elsewhere']
        async with Client(create_server(self.profile('youtube'))) as client:
            for url in invalid:
                with self.subTest(url=url), patch.object(requests.adapters.HTTPAdapter, 'send', side_effect=AssertionError('No provider call')):
                    self.assertTrue((await client.call_tool('get_youtube_video_data', {'url': url})).is_error)
            for url in ['https://youtu.be/abcdefghijk', 'https://youtube.com/embed/abcdefghijk', 'https://www.youtube.com/v/abcdefghijk']:
                with patch.object(requests.adapters.HTTPAdapter, 'send', lambda _, req, **kw: response(req)):
                    self.assertFalse((await client.call_tool('get_youtube_video_data', {'url': url})).is_error)

    async def test_provider_errors_redirects_and_oversized_bodies_close_without_retry(self):
        for name in OPS:
            for kind in ('redirect', 'error', 'too-large', 'invalid-json', 'transport'):
                seen, raws = [], []
                def send(_, req, **kw):
                    seen.append(req)
                    if kind == 'transport': raise requests.ConnectionError(CANARY)
                    result = response(req, raw=(b'x' * 1_048_577 if kind == 'too-large' else CANARY.encode()),
                                      status=302 if kind == 'redirect' else 500 if kind == 'error' else 200,
                                      headers={'Location': 'https://foreign.invalid/' + CANARY})
                    raws.append(result.raw); return result
                with self.subTest(name=name, kind=kind), patch.object(requests.adapters.HTTPAdapter, 'send', send):
                    async with Client(create_server(self.profile(name))) as client:
                        result = await client.call_tool(OPS[name][0], arguments(name, OPS[name][0]))
                        self.assertTrue(result.is_error, result)
                        self.assertNotIn(CANARY, str(result))
                self.assertEqual(len(seen), 1)
                self.assertTrue(all(raw.closed for raw in raws))

    async def test_wikipedia_disambiguation_and_redirect_bounds(self):
        seen = []
        def disambiguation(_, req, **kw):
            seen.append(req)
            prop = parse_qs(urlsplit(req.url).query)['prop'][0]
            item = ({'title': 'Mercury', 'pageprops': {'disambiguation': ''}} if prop == 'info|pageprops' else
                    {'title': 'Mercury', 'revisions': [{'*': '<ul><li><a>Mercury planet</a></li><li><a>Mercury element</a></li><li class="tocsection-1"><a>Skip</a></li></ul>'}]})
            return response(req, {'query': {'pages': {'1': item}}})
        with patch.object(requests.adapters.HTTPAdapter, 'send', disambiguation):
            async with Client(create_server(self.profile('wikipedia'))) as client:
                result = await client.call_tool('search_wikipedia', {'query': 'Mercury'})
                self.assertFalse(result.is_error, result)
                self.assertEqual(decoded(result)['options'], ['Mercury planet', 'Mercury element'])
        self.assertEqual(len(seen), 2)
        seen.clear()
        def redirect(_, req, **kw):
            seen.append(req)
            return response(req, {'query': {'pages': {'1': {'title': 'Target'}}, 'redirects': [{'from': 'Original', 'to': 'Target'}]}})
        with patch.object(requests.adapters.HTTPAdapter, 'send', redirect):
            async with Client(create_server(self.profile('wikipedia'))) as client:
                self.assertTrue((await client.call_tool('search_wikipedia', {'query': 'Original'})).is_error)
        self.assertEqual(len(seen), 4)

    async def test_x_authors_and_times_cannot_silently_be_fabricated(self):
        for change in ('author', 'time', 'count', 'metrics'):
            def send(_, req, **kw):
                value = provider(req)
                if change == 'author': value['includes']['users'] = []
                if change == 'time': value['data'][0]['created_at'] += '\n' + CANARY
                if change == 'count': value['meta']['result_count'] = 7
                if change == 'metrics': value['data'][0]['public_metrics']['like_count'] = False
                return response(req, value)
            with self.subTest(change=change), patch.object(requests.adapters.HTTPAdapter, 'send', send):
                async with Client(create_server(self.profile('x'))) as client:
                    self.assertTrue((await client.call_tool('search_posts', {'query': 'research'})).is_error)

    async def test_x_consumed_identifiers_are_canonical_before_sdk_integer_conversion(self):
        cases = [('get_user_info', 'user'), ('search_posts', 'expanded-user'),
                 ('search_posts', 'tweet'), ('search_posts', 'author')]
        for op, field in cases:
            def send(_, req, **kw):
                value = provider(req)
                if field == 'user': value['data']['id'] = '0123'
                elif field == 'expanded-user':
                    value['includes']['users'][0]['id'] = '0123'
                    value['data'][0]['author_id'] = '0123'
                elif field == 'tweet': value['data'][0]['id'] = '0456'
                else: value['data'][0]['author_id'] = '0123'
                return response(req, value)
            with self.subTest(field=field), patch.object(requests.adapters.HTTPAdapter, 'send', send):
                async with Client(create_server(self.profile('x'))) as client:
                    self.assertTrue((await client.call_tool(op, arguments('x', op))).is_error)

    async def test_oxylabs_consumed_empty_parsed_fallback_is_validated(self):
        for fallback, expected_error, expected_count in (([], False, 0), ([{}], True, None), ('fifty', False, 50), ('fifty-one', True, None)):
            def send(_, req, **kw):
                value = provider(req); row = value['results'][0]
                original = row['content']['results']['organic'][0]
                items = [deepcopy(original) for _ in range(50 if fallback == 'fifty' else 51)] if type(fallback) is str else fallback
                row['content_parsed'] = {'results': {'organic': []}}
                row['content']['results']['organic'] = items
                return response(req, value)
            with self.subTest(fallback=fallback), patch.object(requests.adapters.HTTPAdapter, 'send', send):
                async with Client(create_server(self.profile('oxylabs'))) as client:
                    result = await client.call_tool('search_google', {'query': 'research'})
                    self.assertEqual(result.is_error, expected_error, result)
                    if not expected_error: self.assertEqual(len(decoded(result)['results']), expected_count)

    async def test_wikipedia_secondary_identity_and_continuation_are_checked(self):
        for change in ('page-key', 'page-id', 'title', 'continuation', 'revision-page', 'revision-continuation'):
            seen = []
            def send(_, req, **kw):
                seen.append(req); value = provider(req); item = value['query']['pages']['42']
                prop = parse_qs(urlsplit(req.url).query)['prop'][0]
                if change.startswith('revision'):
                    if prop == 'info|pageprops': item['pageprops'] = {'disambiguation': ''}
                    else: item['revisions'] = [{'*': '<li><a>Ada Lovelace</a></li>'}]
                if prop != 'info|pageprops':
                    if change == 'page-key': value['query']['pages'] = {'99': {**item, 'pageid': 99}}
                    elif change == 'page-id': item['pageid'] = 99
                    elif change in ('title', 'revision-page'): item['title'] = 'Different page'
                    elif change in ('continuation', 'revision-continuation'): value['continue'] = {'continue': '||'}
                return response(req, value)
            with self.subTest(change=change), patch.object(requests.adapters.HTTPAdapter, 'send', send):
                async with Client(create_server(self.profile('wikipedia'))) as client:
                    self.assertTrue((await client.call_tool('search_wikipedia', {'query': 'Ada Lovelace'})).is_error)
            self.assertEqual(len(seen), 2)

    async def test_reddit_short_nonterminal_page_fails_before_second_request(self):
        seen = []
        def send(_, req, **kw):
            seen.append(req); value = provider(req); value['data']['after'] = 'next'; return response(req, value)
        with patch.object(requests.adapters.HTTPAdapter, 'send', send):
            async with Client(create_server(self.profile('reddit'))) as client:
                self.assertTrue((await client.call_tool('get_top_posts', {'subreddit': 'science', 'limit': 10})).is_error)
        self.assertEqual(len(seen), 1)

    async def test_concurrent_selected_accounts_do_not_exchange_credentials(self):
        for name in ('reddit', 'x', 'oxylabs', 'aws-lambda'):
            seen = []
            def send(_, req, **kw): seen.append(req); return response(req)
            first = self.profile(name)
            with patch.dict(os.environ, {KEY_REF: 'second-service-key', ACCESS_REF: 'AKIA' + 'B' * 16, SESSION_REF: 'second-session-token'}):
                second = self.profile(name, {**config(name), **({'username': 'second-user'} if name == 'oxylabs' else {})})
            with patch.object(requests.adapters.HTTPAdapter, 'send', send):
                async with Client(create_server(first)) as a, Client(create_server(second)) as b:
                    results = await asyncio.gather(*(c.call_tool(OPS[name][0], arguments(name, OPS[name][0])) for c in (a, b)))
                    self.assertTrue(all(not r.is_error for r in results), results)
            headers = [r.headers['Authorization'] for r in seen]
            self.assertEqual(len(set(headers)), 2, name)
            self.assertNotIn(CANARY, str(headers))

    async def test_provider_shape_boundaries_and_selected_identity(self):
        cases = [('reddit', 'get_top_posts', lambda v: v['data']['children'][0]['data'].update(author=None)),
                 ('reddit', 'get_subreddit_stats', lambda v: v['data'].update(display_name='foreign')),
                 ('reddit', 'get_user_info', lambda v: v['data'].update(name='foreign')),
                 ('x', 'get_user_info', lambda v: v['data'].update(username='foreign')),
                 ('x', 'search_posts', lambda v: v['data'][0].update(created_at='2026-02-30T00:00:00Z')),
                 ('oxylabs', 'get_amazon_product', lambda v: v['results'][0]['content'].update(price=False)),
                 ('oxylabs', 'search_google', lambda v: v['results'][0].update(status_code=403)),
                 ('aws-lambda', 'list_functions', lambda v: v.update(NextMarker=False)),
                 ('youtube', 'get_youtube_video_data', lambda v: v.update(width=True))]
        for name, op, change in cases:
            def send(_, req, **kw):
                value = provider(req); change(value); return response(req, value)
            with self.subTest(name=name, op=op), patch.object(requests.adapters.HTTPAdapter, 'send', send):
                async with Client(create_server(self.profile(name))) as client:
                    self.assertTrue((await client.call_tool(op, arguments(name, op))).is_error)

    async def test_wikipedia_selected_language_redirect_success_and_incomplete_options(self):
        seen = []
        def send(_, req, **kw):
            seen.append(req)
            self.assertEqual(urlsplit(req.url).hostname, 'sv.wikipedia.org')
            params = parse_qs(urlsplit(req.url).query)
            value = provider(req)
            if params['titles'] == ['Original']:
                value['query']['redirects'] = [{'from': 'Original', 'to': 'Ada Lovelace'}]
            return response(req, value)
        with patch.object(requests.adapters.HTTPAdapter, 'send', send):
            async with Client(create_server(self.profile('wikipedia', {**config('wikipedia'), 'language': 'sv'}))) as client:
                result = await client.call_tool('search_wikipedia', {'query': 'Original'})
                self.assertFalse(result.is_error, result)
                self.assertEqual(decoded(result)['name'], 'Original')
        self.assertEqual(len(seen), 3)
        seen.clear()
        def disambiguate(_, req, **kw):
            seen.append(req)
            item = ({'title': 'Topic', 'pageprops': {'disambiguation': ''}} if len(seen) == 1 else
                    {'title': 'Topic', 'revisions': [{'*': '<li><a>Option</a></li>' * 51}]})
            return response(req, {'query': {'pages': {'1': item}}})
        with patch.object(requests.adapters.HTTPAdapter, 'send', disambiguate):
            async with Client(create_server(self.profile('wikipedia'))) as client:
                self.assertTrue((await client.call_tool('search_wikipedia', {'query': 'Topic'})).is_error)
        self.assertEqual(len(seen), 2)

    async def test_sdk_errors_and_debug_projection_exclude_credentials_and_provider_diagnostics(self):
        stream = io.StringIO()
        handler = logging.StreamHandler(stream)
        root = logging.getLogger()
        previous = root.level
        root.addHandler(handler); root.setLevel(logging.DEBUG)
        try:
            for name in OPS:
                seen = []
                def send(_, req, **kw):
                    seen.append(req)
                    return response(req, raw=(CANARY + KEY + TOKEN).encode(), status=503)
                with self.subTest(name=name), patch.object(requests.adapters.HTTPAdapter, 'send', send):
                    async with Client(create_server(self.profile(name))) as client:
                        result = await client.call_tool(OPS[name][0], arguments(name, OPS[name][0]))
                        self.assertTrue(result.is_error)
                        self.assertNotIn(CANARY, str(result))
                self.assertEqual(len(seen), 1)
        finally:
            root.removeHandler(handler); root.setLevel(previous)
        for secret in (CANARY, KEY, TOKEN, ACCESS): self.assertNotIn(secret, stream.getvalue())

    async def test_cancellation_waits_for_body_and_retires_owned_resources(self):
        for name in OPS:
            entered, release = threading.Event(), threading.Event()
            bodies = []
            class HeldRaw(Raw):
                def read(self, size=-1):
                    entered.set(); release.wait(5); return super().read(size)
            def send(_, req, **kw):
                result = response(req); result.raw = HeldRaw(result.raw.getvalue()); bodies.append(result.raw); return result
            profile = self.profile(name)
            with patch.object(requests.adapters.HTTPAdapter, 'send', send):
                async with Client(create_server(profile)) as client:
                    call = asyncio.create_task(client.call_tool(OPS[name][0], arguments(name, OPS[name][0])))
                    self.assertTrue(await asyncio.to_thread(entered.wait, 3), name)
                    call.cancel()
                    await asyncio.sleep(0.03)
                    self.assertTrue(any(not b.closed for b in bodies))
                    release.set()
                    try: await call
                    except asyncio.CancelledError: pass
                profile.close()
                self.assertTrue(all(b.closed for b in bodies))


if __name__ == '__main__': unittest.main()
