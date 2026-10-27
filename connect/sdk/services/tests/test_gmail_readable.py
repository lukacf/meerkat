"""Original public Gmail content contract and real MCP with controlled HTTP."""
from __future__ import annotations

import asyncio
import base64
from contextlib import asynccontextmanager
from copy import deepcopy
import importlib
import json
import os
import unittest

import httpx
from mcp import Client

NAMESPACE = os.environ.get('TOOLKIT_SERVICES_TEST_NAMESPACE', 'meerkat_toolkit')
content = importlib.import_module(NAMESPACE + '.connectors.gmail_content')
adapter = importlib.import_module(NAMESPACE + '.adapters.services_mcp')
config = importlib.import_module(NAMESPACE + '.adapters.services_config')
prepare = importlib.import_module(NAMESPACE + '.httpx_client').prepare_httpx_client


def body(value):
    return {'size': len(value), 'data': base64.urlsafe_b64encode(value).decode().rstrip('=')}


def leaf(value=b'Hello', *, part_id='', mime='text/plain', charset='utf-8', filename=''):
    return {'partId': part_id, 'mimeType': mime, 'filename': filename,
            'headers': [{'name': 'Content-Type', 'value': mime + '; charset=' + charset}],
            'body': body(value)}


def message(payload=None):
    result = {'id': 'message-1', 'threadId': 'thread-1', 'labelIds': ['INBOX'],
              'historyId': '945', 'internalDate': '1791136800000', 'sizeEstimate': 231,
              'snippet': 'A provider snippet, not completeness', 'futureMetadata': {'keep': True},
              'payload': deepcopy(payload if payload is not None else leaf())}
    result['payload'].setdefault('headers', []).append({'name': 'From', 'value': 'outer@example.invalid'})
    return result


def multipart(*parts):
    return {'partId': '', 'mimeType': 'multipart/mixed', 'filename': '',
            'body': {'size': 0}, 'parts': list(parts)}


def settings(operations=('read_message', 'read_text_part'), user='me', maximum=65536, timeout=3):
    return config.validate_configuration({'profile': 'gmail', 'operations': list(operations), 'config': {
        'token_env': 'SYNTHETIC_REFERENCE', 'timeout_seconds': timeout, 'max_response_bytes': maximum,
        'max_message_bytes': 4096, 'user_id': user}})


class ContentTests(unittest.TestCase):
    def test_plain_html_and_exact_outer_metadata(self):
        supplied = message(multipart(leaf('Räksmörgås'.encode(), part_id='0'),
                           leaf(b'<a href="https://do-not-fetch.invalid/">HTML</a>', part_id='1', mime='text/html')))
        before = deepcopy(supplied)
        result = content.readable_message(supplied, max_bytes=65536)
        self.assertEqual(result['message'], {k: v for k, v in supplied.items() if k != 'payload'})
        self.assertEqual(result['headers'], [{'name': 'From', 'value': 'outer@example.invalid'}])
        self.assertEqual([p['text'] for p in result['parts'][1:]], ['Räksmörgås', '<a href="https://do-not-fetch.invalid/">HTML</a>'])
        self.assertTrue(result['body_complete']);self.assertEqual(supplied, before)

    def test_attached_message_never_supplies_outer_content(self):
        nested = {'partId': '1', 'mimeType': 'message/rfc822', 'filename': 'forwarded.eml',
                  'headers': [{'name': 'From', 'value': 'forged@example.invalid'}],
                  'parts': [leaf(b'Nested misleading content', part_id='1.0')], 'body': {'size': 0}}
        supplied = message(multipart(leaf(b'Outer', part_id='0'), nested))
        result = content.readable_message(supplied, max_bytes=65536)
        self.assertNotIn('forged@example.invalid', json.dumps(result))
        self.assertNotIn('Nested misleading content', json.dumps(result))
        self.assertTrue(result['parts'][3]['embedded_message'])
        with self.assertRaises(content.GmailContentError) as caught:
            content.select_text_part(supplied, '1.0', max_bytes=65536)
        self.assertEqual(caught.exception.code, 'unsupported_part')

    def test_empty_root_is_selectable_but_missing_body_is_incomplete(self):
        supplied = message(leaf(b''))
        chosen = content.select_text_part(supplied, '', max_bytes=65536)
        self.assertEqual(content.readable_text_part(chosen, max_bytes=65536)['part']['text'], '')
        supplied['payload']['body'] = {}
        result = content.readable_message(supplied, max_bytes=65536)
        self.assertFalse(result['body_complete'])
        self.assertEqual(result['parts'][0]['omitted_reason'], 'missing_body')

    def test_text_attachment_is_only_read_when_explicitly_selected(self):
        supplied = message(multipart(leaf(b'Body', part_id='0'), leaf(b'Private note', part_id='1', filename='note.txt')))
        result = content.readable_message(supplied, max_bytes=65536)
        self.assertIsNone(result['parts'][2]['text'])
        selected = content.select_text_part(supplied, '1', max_bytes=65536)
        result = content.readable_text_part(selected, max_bytes=65536)
        self.assertEqual(result['part']['text'], 'Private note')
        self.assertTrue(result['part']['is_attachment']);self.assertFalse(result['part']['is_outer_body'])

    def test_external_part_is_omitted_then_bound_to_expected_size(self):
        supplied = message(leaf(part_id='external'))
        supplied['payload']['body'] = {'size': 4, 'attachmentId': 'attached-1'}
        result = content.readable_message(supplied, max_bytes=65536)
        self.assertFalse(result['body_complete']);self.assertEqual(result['parts'][0]['omitted_reason'], 'external_body')
        chosen = content.select_text_part(supplied, 'external', max_bytes=65536)
        self.assertEqual(content.readable_text_part(chosen, external_body=body(b'Text'), max_bytes=65536)['part']['text'], 'Text')
        with self.assertRaises(content.GmailContentError):
            content.readable_text_part(chosen, external_body=body(b'Changed length'), max_bytes=65536)

    def test_charset_is_explicit_with_no_replacement_or_double_transfer_decode(self):
        for charset, data, expected in [('utf-8', 'Ω'.encode(), 'Ω'), ('iso-8859-1', b'caf\xe9', 'café'),
                                        ('windows-1252', b'\x80', '€'), ('us-ascii', b'=3D', '=3D')]:
            supplied = message(leaf(data, charset=charset))
            supplied['payload']['headers'].append({'name': 'Content-Transfer-Encoding', 'value': 'quoted-printable'})
            self.assertEqual(content.readable_message(supplied, max_bytes=65536)['parts'][0]['text'], expected)
        for charset, data, reason in [('x-unknown', b'abc', 'unsupported_charset'), ('utf-8', b'\xff', 'invalid_text')]:
            supplied = message(leaf(data, charset=charset))
            result = content.readable_message(supplied, max_bytes=65536)
            self.assertFalse(result['body_complete']);self.assertEqual(result['parts'][0]['omitted_reason'], reason)
            with self.assertRaises(content.GmailContentError) as caught:
                content.select_text_part(supplied, '', max_bytes=65536)
            self.assertEqual(caught.exception.code, reason)
            self.assertIsNone(caught.exception.__context__)

    def test_duplicate_headers_ids_and_malformed_bodies(self):
        mutations = [lambda p: p['headers'].append({'name': 'content-type', 'value': 'text/html'}),
                     lambda p: p['body'].update(data='SGVsbG9='),  # noncanonical pad bits
                     lambda p: p['body'].update(data='%%%'),
                     lambda p: p['body'].update(data=None),
                     lambda p: p['body'].update(size=None),
                     lambda p: p['body'].update(attachmentId=None),
                     lambda p: p['body'].update(size=True),
                     lambda p: p['body'].update(attachmentId='hidden'),
                     lambda p: p['headers'][0].update(value='text/plain; charset=utf-8; charset=ascii')]
        for change in mutations:
            supplied = message();change(supplied['payload'])
            with self.subTest(change=change), self.assertRaises(content.GmailContentError):
                content.readable_message(supplied, max_bytes=65536)
        supplied = message(multipart(leaf(part_id='same'), leaf(part_id='same')))
        with self.assertRaises(content.GmailContentError) as caught:
            content.select_text_part(supplied, 'same', max_bytes=65536)
        self.assertEqual(caught.exception.code, 'ambiguous_part')

    def test_unspecified_charset_is_ascii_and_empty_is_not_missing(self):
        supplied = message(leaf(b'\xff'))
        supplied['payload']['headers'] = []
        result = content.readable_message(supplied, max_bytes=65536)
        self.assertEqual(result['parts'][0]['charset'], 'us-ascii')
        self.assertEqual(result['parts'][0]['omitted_reason'], 'invalid_text')
        supplied['payload']['body'] = {'size': 0}
        result = content.readable_message(supplied, max_bytes=65536)
        self.assertEqual(result['parts'][0]['text'], '')
        self.assertTrue(result['body_complete'])

    def test_limits_before_large_decode_and_depth(self):
        supplied = message(leaf(b'x' * 1000))
        with self.assertRaises(content.GmailContentError) as caught:
            content.readable_message(supplied, max_bytes=1000)
        self.assertEqual(caught.exception.code, 'content_limit')
        node = leaf()
        for _ in range(34):node = multipart(node)
        with self.assertRaises(content.GmailContentError) as caught:
            content.readable_message(message(node), max_bytes=65536)
        self.assertEqual(caught.exception.code, 'content_limit')


class Provider:
    def __init__(self, supplied=None, attachment=None):
        self.supplied = supplied or message();self.attachment = attachment or body(b'Text')
        self.calls = [];self.closed = [];self.started = asyncio.Event();self.release = None;self.delay = 0

    async def handle(self, request):
        self.calls.append((request.method, str(request.url), request.headers.get('authorization')))
        self.started.set()
        if self.delay:await asyncio.sleep(self.delay)
        value = self.attachment if '/attachments/' in request.url.path else self.supplied
        raw = json.dumps(value).encode();owner = self
        class Stream(httpx.AsyncByteStream):
            async def __aiter__(self):
                if owner.release is not None:await owner.release.wait()
                yield raw
            async def aclose(self):owner.closed.append(str(request.url))
        return httpx.Response(200, headers={'content-type':'application/json'}, stream=Stream(), request=request)


@asynccontextmanager
async def connection(provider, selection=None):
    async with httpx.AsyncClient(base_url='https://gmail.googleapis.com/',
             headers={'Authorization':'Bearer test-only'}, transport=httpx.MockTransport(provider.handle),
             trust_env=False) as http:
        prepare(http)
        profile = adapter.create_profile_from_client(selection or settings(), http)
        try:
            async with Client(adapter.create_server(profile)) as client:yield client, profile
        finally:await profile.aclose()


class McpTests(unittest.IsolatedAsyncioTestCase):
    async def test_read_message_is_one_get_with_raw_operation_unchanged(self):
        provider = Provider(message(multipart(leaf('Full body Ω'.encode(), part_id='0'), leaf(b'<b>Alternative</b>', part_id='1', mime='text/html'))))
        async with connection(provider, settings(('read_message','get_message'))) as (client, _):
            result = await client.call_tool('gmail_read_message', {'message_id':'message-1'})
            self.assertFalse(result.is_error)
            self.assertEqual(result.structured_content['value']['parts'][1]['text'], 'Full body Ω')
            self.assertEqual(len(provider.calls),1)
            raw_result = await client.call_tool('gmail_get_message', {'message_id':'message-1'})
            self.assertEqual(raw_result.structured_content['value'], provider.supplied)
        self.assertTrue(all('format=full' in call[1] and call[0]=='GET' for call in provider.calls))
        self.assertEqual(len(provider.closed),2)

    async def test_external_selection_uses_exact_current_message_part(self):
        supplied = message(leaf(part_id='0'));supplied['payload']['body']={'attachmentId':'body-9','size':4}
        provider = Provider(supplied)
        async with connection(provider) as (client, _):
            result = await client.call_tool('gmail_read_text_part', {'message_id':'message-1','part_id':'0'})
        self.assertFalse(result.is_error)
        self.assertEqual(result.structured_content['value']['part']['text'],'Text')
        self.assertEqual(len(provider.calls),2)
        self.assertTrue(provider.calls[1][1].endswith('/messages/message-1/attachments/body-9'))
        self.assertEqual(len(provider.closed),2)

    async def test_duplicate_or_unknown_part_never_fetches_body(self):
        for part_id, supplied, code in [('nope',message(),'part_not_found'),
            ('same',message(multipart(leaf(part_id='same'),leaf(part_id='same'))),'ambiguous_part')]:
            provider=Provider(supplied)
            async with connection(provider) as (client,_):
                result=await client.call_tool('gmail_read_text_part',{'message_id':'message-1','part_id':part_id})
            self.assertTrue(result.is_error);self.assertEqual(result.structured_content['error']['content_code'],code)
            self.assertEqual(len(provider.calls),1)

    async def test_configuration_and_schema_refuse_before_transport(self):
        provider=Provider()
        async with connection(provider,settings(('read_message',))) as (client,_):
            denied=await client.call_tool('gmail_read_text_part',{'message_id':'message-1','part_id':''})
            invalid=await client.call_tool('gmail_read_message',{'message_id':'message-1','user_id':'other'})
        self.assertTrue(denied.is_error);self.assertTrue(invalid.is_error);self.assertEqual(provider.calls,[])

    async def test_content_refusal_is_distinct_and_does_not_echo_input(self):
        supplied=message();supplied['payload']['body']['data']='secret-canary%'
        provider=Provider(supplied)
        async with connection(provider) as (client,_):
            result=await client.call_tool('gmail_read_message',{'message_id':'message-1'})
        self.assertEqual(result.structured_content['error']['kind'],'content_error')
        self.assertNotIn('secret-canary',result.model_dump_json())

    async def test_part_id_trailing_newline_refuses_before_transport(self):
        provider=Provider()
        async with connection(provider) as (client,_):
            result=await client.call_tool('gmail_read_text_part',{'message_id':'message-1','part_id':'0\n'})
        self.assertTrue(result.is_error)
        self.assertEqual(result.structured_content['error']['kind'],'invalid_arguments')
        self.assertFalse(result.structured_content['error']['operation_invoked'])
        self.assertEqual(provider.calls,[])

    async def test_account_routing_is_fixed_and_profiles_independent(self):
        one,two=Provider(message(leaf(b'First'))),Provider(message(leaf(b'Second')))
        async with connection(one,settings(user='first@example.invalid')) as (a,_), connection(two,settings(user='second@example.invalid')) as (b,_):
            left,right=await asyncio.gather(a.call_tool('gmail_read_message',{'message_id':'message-1'}),b.call_tool('gmail_read_message',{'message_id':'message-1'}))
        self.assertEqual(left.structured_content['value']['parts'][0]['text'],'First')
        self.assertEqual(right.structured_content['value']['parts'][0]['text'],'Second')
        self.assertIn('first%40example.invalid',one.calls[0][1]);self.assertIn('second%40example.invalid',two.calls[0][1])

    async def test_whole_operation_deadline_does_not_restart_for_attachment(self):
        supplied=message();supplied['payload']['body']={'attachmentId':'external','size':4}
        provider=Provider(supplied);provider.delay=.6
        async with connection(provider,settings(timeout=1)) as (client,_):
            start=asyncio.get_running_loop().time()
            result=await client.call_tool('gmail_read_text_part',{'message_id':'message-1','part_id':''})
            elapsed=asyncio.get_running_loop().time()-start
        self.assertTrue(result.is_error);self.assertEqual(result.structured_content['error']['kind'],'timeout')
        self.assertLess(elapsed,1.2);self.assertEqual(len(provider.calls),2)

    async def test_cancelled_read_joins_response_close(self):
        provider=Provider();provider.release=asyncio.Event()
        async with connection(provider) as (client,_):
            task=asyncio.create_task(client.call_tool('gmail_read_message',{'message_id':'message-1'}))
            await asyncio.wait_for(provider.started.wait(),1)
            task.cancel()
            with self.assertRaises(asyncio.CancelledError):await task
            for _ in range(100):
                if provider.closed:break
                await asyncio.sleep(.01)
            self.assertEqual(len(provider.closed),1)


if __name__=='__main__':unittest.main(verbosity=2)
