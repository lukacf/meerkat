"""Model presentation copies cannot weaken the canonical Wiki MCP contract."""

from __future__ import annotations

from copy import deepcopy
import json
import unittest
from unittest.mock import patch
from urllib.parse import parse_qs, quote, urlsplit

from jsonschema import Draft202012Validator
from mcp import Client
import requests

from meerkat_toolkit.adapters.agno_mcp import create_server
from test_agno_http_readers import Offline, response


class WikiSchemaBoundaryTests(Offline, unittest.IsolatedAsyncioTestCase):
    async def test_presentation_copy_does_not_weaken_canonical_mcp_validation(self):
        invalid = (
            ("missing", {}),
            ("empty", {"query": ""}),
            ("overlimit", {"query": "a" * 257}),
            ("pipe", {"query": "Sun|Moon"}),
            ("nul", {"query": "Sun\x00Moon"}),
            ("del", {"query": "Sun\x7fMoon"}),
            ("final-newline", {"query": "Sun\n"}),
            ("number", {"query": 1}),
            ("null", {"query": None}),
            ("extra-key", {"query": "Sun", "extra": "unselected"}),
        )
        valid = ("Sun", "a", "\u00c5ngstr\u00f6m", "\U0001f30c" * 256)
        for mode in ("auto", "legacy"):
            with self.subTest(mode=mode):
                profile = self.profile("wikipedia")
                original = deepcopy(profile.descriptions)
                requests_seen, bodies = [], []

                def send(_adapter, request, **kwargs):
                    requests_seen.append(request)
                    parsed = urlsplit(request.url)
                    self.assertEqual(parsed.scheme, "https")
                    self.assertEqual(parsed.netloc, "en.wikipedia.org")
                    self.assertEqual(parsed.path, "/w/api.php")
                    self.assertEqual(request.method, "GET")
                    self.assertEqual(kwargs["timeout"], 3)
                    self.assertTrue(kwargs["stream"])
                    query = parse_qs(parsed.query, keep_blank_values=True)
                    title = query["titles"][0]
                    self.assertIn(title, valid)
                    prop = query["prop"][0]
                    self.assertIn(prop, ("info|pageprops", "extracts|info"))
                    page = {"pageid": 42, "title": title}
                    if prop == "extracts|info":
                        page["extract"] = "Controlled summary for " + title
                        page["fullurl"] = "https://en.wikipedia.org/wiki/" + quote(title, safe="")
                    result = response(request, {"query": {"pages": {"42": page}}})
                    bodies.append(result.raw)
                    return result

                with patch.object(requests.adapters.HTTPAdapter, "send", send), patch.object(
                    profile.adapter, "invoke", wraps=profile.adapter.invoke,
                ) as invoke:
                    async with Client(create_server(profile), mode=mode) as client:
                        listing = await client.list_tools()
                        self.assertEqual([tool.name for tool in listing.tools], ["search_wikipedia"])
                        canonical = deepcopy(listing.tools[0].input_schema)
                        model_copy = deepcopy(canonical)
                        # This models an independently presented declaration,
                        # not a generic projection implementation or acceptance
                        # claim for any model provider.
                        model_copy["properties"]["query"].pop("pattern")
                        canonical_validator = Draft202012Validator(canonical)
                        model_validator = Draft202012Validator(model_copy)
                        for label, arguments in invalid:
                            with self.subTest(mode=mode, invalid=label):
                                self.assertFalse(canonical_validator.is_valid(arguments))
                                if label in ("pipe", "nul", "del", "final-newline"):
                                    self.assertTrue(model_validator.is_valid(arguments))
                                result = await client.call_tool("search_wikipedia", arguments)
                                self.assertTrue(result.is_error)
                                self.assertEqual(result.structured_content["error"]["kind"],
                                                 "invalid_arguments")
                                self.assertFalse(result.structured_content["error"]["entered"])
                                self.assertEqual(invoke.call_count, 0)
                                self.assertEqual(requests_seen, [])

                        for title in valid:
                            with self.subTest(mode=mode, valid_length=len(title)):
                                self.assertTrue(canonical_validator.is_valid({"query": title}))
                                result = await client.call_tool("search_wikipedia", {"query": title})
                                self.assertFalse(result.is_error, result)
                                value = json.loads(result.structured_content["value"])
                                self.assertEqual(value["name"], title)
                                self.assertEqual(value["content"], "Controlled summary for " + title)

                        self.assertEqual(invoke.call_count, len(valid))
                        self.assertEqual(len(requests_seen), 2 * len(valid))
                        again = await client.list_tools()
                        self.assertEqual(again.tools[0].input_schema, canonical)
                        self.assertEqual(profile.descriptions, original)
                        self.assertNotIn("pattern", model_copy["properties"]["query"])
                self.assertTrue(all(body.closed for body in bodies))


if __name__ == "__main__":
    unittest.main()
