"""Bound complete real MCP results and preserve exact resumed Slack occurrences."""
from copy import deepcopy
import importlib
import json
import os
import unittest

import httpx
from mcp import Client, types

NAMESPACE = os.environ.get('TOOLKIT_SERVICES_TEST_NAMESPACE', 'meerkat_toolkit')
adapter = importlib.import_module(NAMESPACE + '.adapters.services_mcp')
config = importlib.import_module(NAMESPACE + '.adapters.services_config')
prepare = importlib.import_module(NAMESPACE + '.httpx_client').prepare_httpx_client


def selected(maximum=None, operations=None):
    values = {'token_env': 'UNUSED_REFERENCE', 'timeout_seconds': 3,
              'max_response_bytes': 65536, 'max_interval_requests': 32,
              'max_interval_messages': 100, 'allowed_channel_ids': ['C1']}
    if maximum is not None:
        values['max_interval_result_bytes'] = maximum
    return {'profile': 'slack', 'operations': operations or ['read_interval'], 'config': values}


def wire(result):
    return result.model_dump_json(by_alias=True, exclude_none=True).encode('utf-8')


class ResultEnvelopeTests(unittest.TestCase):
    def test_full_text_and_structured_content_with_escaping(self):
        empty = types.CallToolResult(content=[types.TextContent(type='text', text='')],
                                     structured_content={'value': None})
        for value in [None, False, 0, -1.25e-50, {'text': '\u4e2d\U0001f600\x00\x7f"\\' * 200},
                      {'numbers': [1e-300, 1e300, -0.0], 'empty': [None, False, '']}]:
            with self.subTest(value_type=type(value).__name__):
                size = len(json.dumps(value, ensure_ascii=True, allow_nan=False,
                                      separators=(',', ':')).encode('ascii'))
                result = adapter._interval_tool_result(value, size)
                self.assertFalse(result.is_error)
                self.assertEqual(json.loads(result.content[0].text), value)
                self.assertEqual(result.structured_content['value'], value)
                self.assertLessEqual(len(wire(result)), 3 * size + len(wire(empty)))
                refused = adapter._interval_tool_result(value, size - 1)
                self.assertTrue(refused.is_error)
                self.assertEqual(refused.structured_content['error']['kind'], 'result_too_large')
                self.assertNotIn('value', refused.structured_content)

    def test_config_default_bounds_and_conditional_field(self):
        current = config.validate_configuration(selected())
        tool = config.tool_descriptions(current)[0]
        bound = tool['inputSchema']['properties']['max_result_bytes']
        self.assertEqual(bound['default'], 262144)
        self.assertEqual(bound['maximum'], 262144)
        for maximum in [4096, 65536, 16777216]:
            current = config.validate_configuration(selected(maximum))
            bound = config.tool_descriptions(current)[0]['inputSchema']['properties']['max_result_bytes']
            self.assertEqual(bound['maximum'], maximum)
        for maximum in [True, 4095, 16777217, 8192.0, '8192']:
            with self.assertRaises(config.ConfigurationError):
                config.validate_configuration(selected(maximum))
        value = selected(8192, ['auth_test'])
        del value['config']['max_interval_requests'], value['config']['max_interval_messages']
        with self.assertRaises(config.ConfigurationError):
            config.validate_configuration(value)


class RealMcpBudgetTests(unittest.IsolatedAsyncioTestCase):
    async def trial(self, text, count, cap):
        calls = []
        originals = [{'ts': f'11.{i:06}', 'thread_ts': '1', 'text': text} for i in range(count)]

        async def handle(request):
            cursor = request.url.params.get('cursor')
            calls.append((request.url.path, cursor))
            if request.url.path.endswith('replies'):
                return httpx.Response(200, json={'ok': True, 'messages': []})
            index = int(cursor.removeprefix('page-')) if cursor else 0
            payload = {'ok': True, 'messages': [originals[index]], 'has_more': index + 1 < count}
            if index + 1 < count:
                payload['response_metadata'] = {'next_cursor': f'page-{index+1}'}
            return httpx.Response(200, json=payload)

        async with prepare(httpx.AsyncClient(base_url='https://slack.com/',
                    transport=httpx.MockTransport(handle), trust_env=False)) as transport:
            profile = adapter.create_profile_from_client(config.validate_configuration(selected(cap)), transport)
            try:
                async with Client(adapter.create_server(profile)) as client:
                    args = {'channel': 'C1', 'start': '10', 'end': '20', 'root_oldest': '0',
                            'max_requests': 32, 'max_messages': 100, 'resumable': True}
                    bad = await client.call_tool('slack_read_interval', {**args, 'max_result_bytes': cap + 1})
                    self.assertTrue(bad.is_error)
                    self.assertEqual(calls, [])
                    observed, parts, saved = [], [], None
                    for _ in range(count + 3):
                        before = deepcopy(args.get('continuation'))
                        result = await client.call_tool('slack_read_interval', args)
                        self.assertFalse(result.is_error)
                        value = result.structured_content['value']
                        self.assertEqual(json.loads(result.content[0].text), value)
                        self.assertLessEqual(len(result.content[0].text.encode('ascii')), cap)
                        self.assertLessEqual(len(wire(result)), 3 * cap + 256)
                        self.assertEqual(value['limits']['max_result_bytes'], cap)
                        self.assertEqual(before, args.get('continuation'))
                        observed.extend(row['message'] for row in value['messages'])
                        parts.append(value)
                        if value['continuation'] is None:
                            break
                        self.assertEqual(value['termination'], 'result_budget')
                        self.assertTrue(value['messages'])
                        saved = deepcopy(value['continuation'])
                        args['continuation'] = saved
                    else:
                        self.fail('resumption failed to finish')
                    self.assertEqual(observed, originals)
                    self.assertGreater(len(parts), 1)
                    self.assertTrue(any(part['termination'] == 'result_budget' for part in parts))
                    self.assertEqual(parts[-1]['termination'], 'finished')
            finally:
                await profile.aclose()
        return parts, calls

    async def test_oversized_first_page_stays_pending_until_explicit_larger_budget(self):
        calls = []
        original = {"ts": "11", "thread_ts": "1", "text": "X" * 12000}

        async def handle(request):
            calls.append((request.url.path, request.url.params.get("cursor")))
            return httpx.Response(200, json={"ok": True, "messages":
                [] if request.url.path.endswith("replies") else [original]})

        async with prepare(httpx.AsyncClient(base_url="https://slack.com/",
                    transport=httpx.MockTransport(handle))) as transport:
            profile = adapter.create_profile_from_client(config.validate_configuration(selected(65536)), transport)
            try:
                async with Client(adapter.create_server(profile)) as client:
                    args = {"channel": "C1", "start": "10", "end": "20", "root_oldest": "0",
                            "max_requests": 32, "max_messages": 100, "resumable": True,
                            "max_result_bytes": 4096}
                    first = (await client.call_tool("slack_read_interval", args)).structured_content["value"]
                    self.assertEqual(first["termination"], "result_budget")
                    self.assertEqual(first["messages"], [])
                    self.assertEqual(first["requests_made"], 1)
                    self.assertEqual(first["history"]["pages"], 0)
                    self.assertEqual(first["continuation"]["discovered_roots"], [])
                    self.assertEqual(len(calls), 1)
                    checkpoint = deepcopy(first["continuation"])
                    wrong = await client.call_tool("slack_read_interval", {
                        **args, "continuation": checkpoint, "channel": "C2"})
                    self.assertTrue(wrong.is_error)
                    self.assertEqual(len(calls), 1)
                    second = await client.call_tool("slack_read_interval", {
                        **args, "continuation": checkpoint, "max_result_bytes": 65536})
                    self.assertFalse(second.is_error)
                    value = second.structured_content["value"]
                    self.assertEqual([row["message"] for row in value["messages"]], [original])
                    self.assertIsNone(value["continuation"])
                    self.assertEqual(value["termination"], "finished")
                    self.assertEqual(checkpoint, first["continuation"])
                    self.assertLessEqual(len(wire(second)), 3 * 65536 + 256)
            finally:
                await profile.aclose()

    async def test_many_pages_return_bounded_parts_without_message_loss(self):
        await self.trial('A' * 1800, 16, 8192)

    async def test_unicode_and_escaped_text_use_actual_byte_budget(self):
        await self.trial('\u4e2d\U0001f600\x00"\\' * 150, 8, 16384)

    async def test_default_profile_bounds_previously_unbounded_week(self):
        await self.trial('A' * 49152, 16, 262144)


if __name__ == '__main__':
    unittest.main()
