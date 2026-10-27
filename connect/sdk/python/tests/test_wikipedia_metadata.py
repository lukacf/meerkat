"""Wikipedia citation metadata comes from the successful summary response."""
from copy import deepcopy
from datetime import datetime, timezone
import json
import unittest
from unittest.mock import patch
from urllib.parse import parse_qs, quote, urlsplit

from mcp import Client
import requests

from meerkat_toolkit.adapters.agno_mcp import create_server
from test_agno_http_readers import Offline, config, decoded, response


class WikipediaMetadataTests(Offline, unittest.IsolatedAsyncioTestCase):
    async def read(self, title='Alpha Beta', *, language='en', final=None):
        seen, bodies = [], []
        original = {'pageid': 42, 'title': title,
                    'fullurl': f'https://{language}.wikipedia.org/wiki/' + quote(title.replace(' ', '_'), safe='/:'),
                    'extract': 'The exact provider summary.'}
        def send(_adapter, request, **kwargs):
            seen.append(request)
            query = parse_qs(urlsplit(request.url).query, keep_blank_values=True)
            item = deepcopy(original)
            if len(seen) == 1:
                item.pop('extract')
            elif final is not None:
                item = final(item)
            result = response(request, {'query': {'pages': {'42': item}}})
            bodies.append(result.raw)
            return result
        before = datetime.now(timezone.utc)
        with patch.object(requests.adapters.HTTPAdapter, 'send', send):
            async with Client(create_server(self.profile('wikipedia', {**config('wikipedia'), 'language': language}))) as client:
                result = await client.call_tool('search_wikipedia', {'query': title})
        after = datetime.now(timezone.utc)
        self.assertEqual(len(seen), 2)
        self.assertTrue(all(body.closed for body in bodies))
        return result, seen, before, after

    async def test_summary_metadata_is_same_response_and_local_observation(self):
        source = 'https://en.wikipedia.org/wiki/Alpha%20Beta'
        def final(item):
            item['fullurl'] = source
            item['lastrevid'] = 12345
            item['touched'] = '1999-01-01T00:00:00Z'
            return item
        result, seen, before, after = await self.read(final=final)
        self.assertFalse(result.is_error, result)
        value = decoded(result)
        self.assertEqual(value['name'], 'Alpha Beta')
        self.assertEqual(value['content'], 'The exact provider summary.')
        meta = value['meta_data']
        self.assertEqual(set(meta), {'source_url', 'canonical_title', 'page_id', 'language', 'retrieved_at'})
        self.assertEqual({k: v for k, v in meta.items() if k != 'retrieved_at'}, {
            'source_url': source, 'canonical_title': 'Alpha Beta', 'page_id': 42, 'language': 'en'})
        self.assertTrue(meta['retrieved_at'].endswith('Z'))
        observed = datetime.fromisoformat(meta['retrieved_at'].replace('Z', '+00:00'))
        self.assertLessEqual(before, observed)
        self.assertLessEqual(observed, after)
        query = parse_qs(urlsplit(seen[-1].url).query, keep_blank_values=True)
        self.assertEqual(query['prop'], ['extracts|info'])
        self.assertEqual(query['inprop'], ['url'])

    async def test_unicode_subpages_namespaces_and_encoded_title_characters(self):
        for title in ('Ångström', 'Category:Stars/Visible', '100% pure + bright & clear',
                      'Literal%20Escape', 'Alpha_Beta', chr(0x1F30C) * 256):
            with self.subTest(title=title):
                result, _, _, _ = await self.read(title, language='sv')
                self.assertFalse(result.is_error, result)
                meta = decoded(result)['meta_data']
                self.assertEqual(meta['canonical_title'], title)
                self.assertEqual(meta['language'], 'sv')
                self.assertTrue(meta['source_url'].startswith('https://sv.wikipedia.org/wiki/'))

    async def test_canonical_url_preferred_after_both_fields_are_validated(self):
        canonical = 'https://en.wikipedia.org/wiki/Alpha%20Beta'
        result, _, _, _ = await self.read(final=lambda item: {**item, 'canonicalurl': canonical})
        self.assertFalse(result.is_error, result)
        self.assertEqual(decoded(result)['meta_data']['source_url'], canonical)
        def canonical_only(item):
            item.pop('fullurl')
            return {**item, 'canonicalurl': canonical}
        result, _, _, _ = await self.read(final=canonical_only)
        self.assertFalse(result.is_error, result)
        self.assertEqual(decoded(result)['meta_data']['source_url'], canonical)

    async def test_malformed_or_contradictory_source_metadata_is_refused(self):
        invalid = (
            None, '', 1, 'https://foreign.invalid/wiki/Alpha_Beta',
            'http://en.wikipedia.org/wiki/Alpha_Beta', 'https://sv.wikipedia.org/wiki/Alpha_Beta',
            'https://user@en.wikipedia.org/wiki/Alpha_Beta', 'https://en.wikipedia.org:443/wiki/Alpha_Beta',
            'https://en.wikipedia.org/wiki/Alpha_Beta?oldid=1', 'https://en.wikipedia.org/wiki/Alpha_Beta#part',
            'https://en.wikipedia.org/wiki/Other', 'https://en.wikipedia.org/w/index.php',
            'https://en.wikipedia.org/wiki/Alpha%2', 'https://en.wikipedia.org/wiki/Alpha%GG',
            'https://en.wikipedia.org/wiki/Alpha%FFBeta', 'https://en.wikipedia.org/wiki/Alpha%00Beta',
            'https://en.wikipedia.org/wiki/Alpha_Beta\n', 'https://en.wikipedia.org\\evil/wiki/Alpha_Beta',
            'https://en.wikipedia.org/wiki/' + 'A' * 4096,
        )
        for source in invalid:
            for field in ('fullurl', 'canonicalurl'):
                with self.subTest(source=source, field=field):
                    result, _, _, _ = await self.read(final=lambda item: {**item, field: source})
                    self.assertTrue(result.is_error, result)
                    self.assertNotIn('provider summary', str(result))
        def missing(item):
            item.pop('fullurl')
            return item
        result, _, _, _ = await self.read(final=missing)
        self.assertTrue(result.is_error, result)

    async def test_redirect_preserves_requested_name_and_returned_canonical_identity(self):
        seen = []
        def send(_adapter, request, **kwargs):
            seen.append(request)
            query = parse_qs(urlsplit(request.url).query, keep_blank_values=True)
            item = {'pageid': 42, 'title': 'Alpha Beta', 'fullurl': 'https://en.wikipedia.org/wiki/Alpha_Beta'}
            value = {'query': {'pages': {'42': item}}}
            if query['titles'] == ['Alias']:
                value['query']['redirects'] = [{'from': 'Alias', 'to': 'Alpha Beta'}]
            if query['prop'] == ['extracts|info']:
                item['extract'] = 'The exact provider summary.'
            return response(request, value)
        with patch.object(requests.adapters.HTTPAdapter, 'send', send):
            async with Client(create_server(self.profile('wikipedia'))) as client:
                result = await client.call_tool('search_wikipedia', {'query': 'Alias'})
        self.assertFalse(result.is_error, result)
        value = decoded(result)
        self.assertEqual(value['name'], 'Alias')
        self.assertEqual(value['meta_data']['canonical_title'], 'Alpha Beta')
        self.assertEqual(len(seen), 3)

    async def test_browser_normalized_paths_cannot_misattribute_the_title(self):
        for title in ('A/../B', './A', 'A/./B', 'A\\B'):
            with self.subTest(title=title):
                result, _, _, _ = await self.read(title)
                self.assertTrue(result.is_error, result)
        result, _, _, _ = await self.read('A/../B', final=lambda item: {
            **item, 'fullurl': 'https://en.wikipedia.org/wiki/A/%2E%2E/B'})
        self.assertTrue(result.is_error, result)


if __name__ == '__main__':
    unittest.main()
