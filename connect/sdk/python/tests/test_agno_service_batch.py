"""Five pinned service donors over actual, offline Requests protocol seams."""

from __future__ import annotations

import asyncio
from contextlib import ExitStack
import hashlib
import inspect
import io
import json
import os
from pathlib import Path
import threading
import traceback
import unittest
from unittest.mock import patch
from urllib.parse import parse_qs, urlsplit

from agno.tools.clickup import ClickUpTools
from agno.tools.discord import DiscordTools
from agno.tools.shopify import ShopifyTools
from agno.tools.spotify import SpotifyTools
from agno.tools.unsplash import UnsplashTools
from jsonschema import Draft202012Validator
from mcp import Client
import requests

from meerkat_toolkit.adapters.agno_mcp import _invoke, _joined_worker, create_server
from meerkat_toolkit.adapters.agno_profiles import build_profile, validate_configuration, ProfileConfigError


REFERENCE = "TOOLKIT_SERVICE_FIXTURE_KEY"
KEY = "fixture-selected-service-key"
CANARY = "fixture-ambient-or-error-value"
HOSTILE = {REFERENCE: KEY, **{name: CANARY for name in (
    "CLICKUP_API_KEY", "MASTER_SPACE_ID", "DISCORD_BOT_TOKEN", "SHOPIFY_SHOP_NAME",
    "SHOPIFY_ACCESS_TOKEN", "UNSPLASH_ACCESS_KEY", "SPOTIFY_ACCESS_TOKEN", "REQUESTS_CA_BUNDLE",
)}, "HTTPS_PROXY": "http://ambient.invalid", "NETRC": "/nonexistent/ambient-netrc"}
DONORS = {"clickup": ClickUpTools, "discord": DiscordTools, "shopify": ShopifyTools,
          "spotify": SpotifyTools, "unsplash": UnsplashTools}
OPERATIONS = {
    "clickup": ["get_task", "list_spaces"],
    "discord": ["get_channel_info", "get_channel_messages", "list_channels"],
    "shopify": ["get_shop_info", "get_products"],
    "spotify": ["get_current_user", "get_my_top_tracks", "get_user_playlists", "search_tracks"],
    "unsplash": ["search_photos", "get_photo"],
}
DONOR_HASHES = {
    "clickup": "91ae7bafe1791ab00e9d1bd6998f4140f9db250cb12bba73e83055965ac3fc07",
    "discord": "9fa239fb2e0c3b318b214c33a53b60c5876952b2668855fad6d3f27bcc022c9b",
    "shopify": "a5a7eefd2d9fb8923decf9c6eb52bc4d96773a1b5f4b4acf8820097e9e6623d0",
    "spotify": "9b75e3293ef63a450ae34288ce0561e99389edee6e0593c59e70d7a3d44e2fe8",
    "unsplash": "e4a89182cbaaa55f512002b047a8fe82e31393f30ac7d46fb2c807b08bc6f1c9",
}


def configuration(name):
    extra = {"clickup": {"workspace_id": "123"},
             "shopify": {"shop_name": "fixture-shop", "api_version": "2025-10"},
             "spotify": {"market": "SE"}}.get(name, {})
    return {"timeout": 7, "api_key_env": REFERENCE, **extra}


def arguments(operation):
    return {
        "get_task": {"task_id": "task_1"}, "get_channel_info": {"channel_id": "123"},
        "get_channel_messages": {"channel_id": "123", "limit": 3},
        "list_channels": {"guild_id": "456"}, "get_products": {"max_results": 3, "status": "ACTIVE"},
        "get_my_top_tracks": {"time_range": "short_term", "limit": 3},
        "get_user_playlists": {"max_results": 3}, "search_tracks": {"query": "fixture", "max_results": 3},
        "search_photos": {"query": "fixture", "per_page": 3, "page": 2, "orientation": "landscape"},
        "get_photo": {"photo_id": "photo_1"},
    }.get(operation, {})


def provider_value(request):
    path = urlsplit(request.url).path
    track = {"id": "track1", "name": "Fixture Song", "artists": [{"name": "Artist"}],
             "album": {"name": "Album"}, "uri": "spotify:track:track1", "popularity": 42}
    photo = {"id": "photo_1", "alt_description": "Fixture Photo", "user": {
        "name": "Photographer", "username": "fixture", "links": {"html": "https://example.invalid/author"}},
        "urls": {"regular": "https://example.invalid/image"}, "tags": [{"title": "fixture"}]}
    if "clickup.com" in request.url:
        return {"spaces": [{"id": "456", "name": "Fixture Space"}]} if path.endswith("/space") else {"id": "task_1", "name": "Fixture Task"}
    if "discord.com" in request.url:
        return [{"id": "123", "content": "Fixture Message"}] if path.endswith(("/messages", "/channels")) else {"id": "123", "name": "Fixture Channel"}
    if "myshopify.com" in request.url:
        query = json.loads(request.body)["query"]
        if "products(" not in query:
            return {"data": {"shop": {"name": "Fixture Shop", "currencyCode": "SEK"}}}
        return {"data": {"products": {"edges": [{"node": {"id": "product1", "title": "Fixture Product",
            "status": "ACTIVE", "variants": {"edges": [{"node": {"id": "variant1", "title": "Blue", "price": "10"}}]}}}]}}}
    if "spotify.com" in request.url:
        if path.endswith("/search"):
            return {"tracks": {"items": [track]}}
        if path.endswith("/top/tracks"):
            return {"items": [track]}
        if path.endswith("/playlists"):
            return {"items": [{"id": "playlist1", "name": "Fixture Playlist", "owner": {"display_name": "Owner"},
                "tracks": {"total": 2}, "external_urls": {"spotify": "https://example.invalid/list"}, "uri": "spotify:playlist:playlist1"}],
                "next": "https://other-account.invalid/next"}
        return {"id": "user1", "display_name": "Fixture User"}
    if path == "/search/photos":
        return {"total": 1, "total_pages": 1, "results": [photo]}
    return {**photo, "views": 10, "downloads": 2}


def response(request, value=None, *, status=200, raw=None, headers=None):
    result = requests.Response()
    result.status_code = status
    result.url = request.url
    result.request = request
    result.raw = io.BytesIO(raw if raw is not None else json.dumps(value).encode())
    result.headers.update({"Content-Type": "application/json", **(headers or {})})
    return result


class OfflineMixin:
    def setUp(self):
        super().setUp()
        self.guards = ExitStack()
        self.addCleanup(self.guards.close)
        self.guards.enter_context(patch.dict(os.environ, HOSTILE, clear=True))
        for target in ("socket.socket.connect", "socket.socket.connect_ex", "socket.getaddrinfo"):
            self.guards.enter_context(patch(target, side_effect=AssertionError("network forbidden")))
        self.guards.enter_context(patch("requests.sessions.get_netrc_auth", side_effect=AssertionError("netrc forbidden")))

    def profile(self, name, operations=None):
        profile = build_profile(name, operations=operations or OPERATIONS[name], config=configuration(name))
        self.addCleanup(profile.close)
        return profile


class ConfigurationTests(OfflineMixin, unittest.TestCase):
    def test_exact_pinned_methods_are_inherited_and_resources_close_on_failure(self):
        closed = []
        close = requests.Session.close
        def tracked(session):
            closed.append(session)
            close(session)
        with patch.object(requests.Session, "close", tracked):
            for name, donor in DONORS.items():
                self.assertEqual(hashlib.sha256(Path(inspect.getfile(donor)).read_bytes()).hexdigest(), DONOR_HASHES[name])
                profile = self.profile(name)
                for operation in OPERATIONS[name]:
                    bound = inspect.unwrap(profile.adapter._bindings[operation].entrypoint)
                    self.assertIs(bound.__func__, getattr(donor, operation))
                    self.assertIs(getattr(type(bound.__self__), operation), getattr(donor, operation))
                before = len(closed)
                profile.close(); profile.close()
                self.assertEqual(len(closed), before + 1)
                original = donor.__init__
                def partial(instance, *args, **kwargs):
                    original(instance, *args, **kwargs)
                    raise ValueError(KEY)
                for boundary in (patch.object(donor, "__init__", partial),
                                 patch("meerkat_toolkit.adapters.agno.AgnoToolkitAdapter", side_effect=ValueError(KEY))):
                    before = len(closed)
                    with boundary, self.assertRaises(ProfileConfigError) as caught:
                        self.profile(name)
                    self.assertEqual(len(closed), before + 1)
                    self.assertNotIn(KEY, "".join(traceback.format_exception(caught.exception)))
                    self.assertIsNone(caught.exception.__context__)

    def test_configuration_has_no_ambient_fallback_or_dynamic_factory(self):
        invalid = {"clickup": [{"workspace_id": "../other"}, {"workspace_id": "123\n"}],
                   "discord": [{"bot_token": KEY}],
                   "shopify": [{"shop_name": "other.invalid/x"}, {"shop_name": "shop\n"}, {"api_version": "unstable"}],
                   "spotify": [{"market": "SE\n"}, {"market": "se"}], "unsplash": [{"access_key": KEY}]}
        for name in OPERATIONS:
            for delta in invalid[name]:
                with self.assertRaises(ProfileConfigError):
                    validate_configuration(name, operations=OPERATIONS[name], config={**configuration(name), **delta})
            with self.assertRaises(ProfileConfigError):
                validate_configuration(name, operations=["write_unknown"], config=configuration(name))
            for missing in (None, "", KEY + "\n"):
                with patch.dict(os.environ, HOSTILE, clear=True):
                    if missing is None:
                        os.environ.pop(REFERENCE)
                    else:
                        os.environ[REFERENCE] = missing
                    with self.assertRaises(ProfileConfigError):
                        self.profile(name)


class ProtocolTests(OfflineMixin, unittest.IsolatedAsyncioTestCase):
    async def test_all_thirteen_operations_use_real_donor_bodies_and_selected_transport(self):
        calls = []
        closed = []
        original_close = requests.Response.close
        def close(value):
            closed.append(value)
            original_close(value)
        def send(adapter, request, **kwargs):
            calls.append(request)
            self.assertEqual(adapter.max_retries.total, 0)
            self.assertEqual(kwargs["proxies"], {})
            self.assertIs(kwargs["verify"], True)
            self.assertTrue(kwargs["stream"])
            self.assertEqual(kwargs["timeout"], 7)
            self.assertNotIn(CANARY, request.url + str(request.headers))
            auth = request.headers.get("Authorization")
            expected = {"clickup": KEY, "discord": f"Bot {KEY}", "spotify": f"Bearer {KEY}", "unsplash": f"Client-ID {KEY}"}
            if current == "shopify":
                self.assertEqual(request.headers["X-Shopify-Access-Token"], KEY)
                self.assertIsNone(auth)
                self.assertEqual(request.method, "POST")
                self.assertEqual(request.url, "https://fixture-shop.myshopify.com/admin/api/2025-10/graphql.json")
            else:
                self.assertEqual(auth, expected[current])
                self.assertEqual(request.method, "GET")
            if operation == "search_tracks":
                self.assertEqual(parse_qs(urlsplit(request.url).query), {"q": ["fixture"], "type": ["track"], "limit": ["3"], "market": ["SE"]})
            return response(request, provider_value(request))
        with patch.object(requests.adapters.HTTPAdapter, "send", send), patch.object(requests.Response, "close", close):
            for current in OPERATIONS:
                os.environ[REFERENCE] = KEY
                profile = self.profile(current)
                os.environ[REFERENCE] = CANARY
                async with Client(create_server(profile)) as client:
                    listed = await client.list_tools()
                    self.assertEqual({tool.name for tool in listed.tools}, set(OPERATIONS[current]))
                    for operation in OPERATIONS[current]:
                        result = await client.call_tool(operation, arguments(operation))
                        self.assertFalse(result.is_error, result.model_dump_json())
                        value = json.loads(result.structured_content["value"])
                        self.assertTrue(value)
                        if operation == "get_products":
                            self.assertEqual(value[0]["variants"][0]["price"], "10")
                        if operation == "search_photos":
                            self.assertEqual(value["photos"][0]["author"]["name"], "Photographer")
                profile.close()
        self.assertEqual(len(calls), 13)
        self.assertEqual(len(closed), 13)

    async def test_schemas_reject_bad_inputs_before_dispatch_and_writes_are_not_exposed(self):
        invalid = {
            "clickup": ("get_task", [{"task_id": "../other"}, {"task_id": "valid\n"}, {"task_id": "a\\b"}]),
            "discord": ("get_channel_messages", [{"channel_id": "123\n"}, {"channel_id": "123", "limit": 101}]),
            "shopify": ("get_products", [{"status": 'ACTIVE"} mutation {'}, {"max_results": 51}, {"max_results": True}]),
            "spotify": ("search_tracks", [{"query": "x", "market": "SE\n"}, {"query": "x", "max_results": 51}, {"query": ""}]),
            "unsplash": ("search_photos", [{"query": "x", "orientation": "invalid"}, {"query": "x", "per_page": 31}, {"query": "x", "page": 10_001}]),
        }
        with patch.object(requests.adapters.HTTPAdapter, "send", side_effect=AssertionError("must not dispatch")) as send:
            for name, (op, bad_values) in invalid.items():
                profile = self.profile(name)
                schemas = {item["name"]: Draft202012Validator(item["parameters"]) for item in profile.descriptions}
                self.assertTrue(schemas[op].is_valid(arguments(op)))
                async with Client(create_server(profile)) as client:
                    for value in bad_values:
                        self.assertFalse(schemas[op].is_valid(value), (name, value))
                        result = await client.call_tool(op, value)
                        self.assertTrue(result.is_error)
                        self.assertFalse(result.structured_content["error"]["entered"])
                    result = await client.call_tool("send_message", {"channel_id": "123", "message": "x"})
                    self.assertTrue(result.is_error)
            send.assert_not_called()

    async def test_omitted_options_use_bounded_donor_defaults(self):
        seen = []
        def send(adapter, request, **kwargs):
            seen.append(request)
            return response(request, provider_value(request))
        cases = [("discord", "get_channel_messages", {"channel_id": "123"}, "limit", "100"),
                 ("shopify", "get_products", {}, None, None),
                 ("spotify", "get_my_top_tracks", {}, "limit", "20"),
                 ("spotify", "get_user_playlists", {}, "limit", "20"),
                 ("spotify", "search_tracks", {"query": "fixture"}, "limit", "10"),
                 ("unsplash", "search_photos", {"query": "fixture"}, "per_page", "10")]
        with patch.object(requests.adapters.HTTPAdapter, "send", send):
            for name, op, args, field, expected in cases:
                profile = self.profile(name, [op])
                async with Client(create_server(profile)) as client:
                    self.assertFalse((await client.call_tool(op, args)).is_error)
                if field:
                    self.assertEqual(parse_qs(urlsplit(seen[-1].url).query)[field], [expected])
                else:
                    self.assertIn("products(first: 50)", json.loads(seen[-1].body)["query"])

    async def test_spotify_search_matches_current_ten_result_limit(self):
        seen = []
        def send(adapter, request, **kwargs):
            seen.append(request)
            return response(request, provider_value(request))
        profile = self.profile("spotify")
        with patch.object(requests.adapters.HTTPAdapter, "send", send):
            async with Client(create_server(profile)) as client:
                listed = await client.list_tools()
                schemas = {tool.name: tool.input_schema for tool in listed.tools}
                self.assertEqual(schemas["search_tracks"]["properties"]["max_results"]["maximum"], 10)
                self.assertEqual(schemas["search_tracks"]["properties"]["max_results"]["description"],
                                 "Maximum number of tracks to return (1-10, default 10).")
                self.assertEqual(schemas["get_user_playlists"]["properties"]["max_results"]["maximum"], 50)
                self.assertEqual(schemas["get_my_top_tracks"]["properties"]["limit"]["maximum"], 50)
                for limit in (0, 11, 50, True, "10"):
                    result = await client.call_tool("search_tracks", {"query": "fixture", "max_results": limit})
                    self.assertTrue(result.is_error)
                    self.assertFalse(result.structured_content["error"]["entered"])
                self.assertEqual(seen, [])
                for limit in (1, 10):
                    result = await client.call_tool("search_tracks", {"query": "fixture", "max_results": limit})
                    self.assertFalse(result.is_error)
                    self.assertEqual(parse_qs(urlsplit(seen[-1].url).query)["limit"], [str(limit)])
                self.assertEqual(len(seen), 2)

    async def test_spotify_playlist_counts_accept_current_legacy_and_absent_shapes(self):
        cases = [({"items": {"total": 7}}, 7), ({"tracks": {"total": 2}}, 2),
                 ({}, None), ({"items": {}}, None), ({"items": {"total": 0}}, 0),
                 ({"items": {"total": None}}, None),
                 ({"items": {"total": 2, "href": "new"},
                   "tracks": {"total": 2, "href": "old"}}, 2)]
        for fields, count in cases:
            with self.subTest(fields=fields):
                seen = []
                def send(adapter, request, **kwargs):
                    seen.append(request)
                    value = provider_value(request)
                    value["items"][0].pop("tracks")
                    value["items"][0].update(fields)
                    value["items"].append(None)
                    return response(request, value)
                profile = self.profile("spotify", ["get_user_playlists"])
                with patch.object(requests.adapters.HTTPAdapter, "send", send):
                    async with Client(create_server(profile)) as client:
                        result = await client.call_tool("get_user_playlists", {})
                self.assertFalse(result.is_error)
                rows = json.loads(result.structured_content["value"])
                self.assertEqual(len(rows), 1)
                self.assertEqual(rows[0]["track_count"], count)
                self.assertEqual(rows[0]["name"], "Fixture Playlist")
                self.assertEqual(len(seen), 1)

    def test_spotify_playlist_normalization_does_not_mutate_provider_input(self):
        value = {"items": [{"items": {"total": 3}}]}
        before = json.dumps(value)
        profile = self.profile("spotify", ["get_user_playlists"])
        from meerkat_toolkit.adapters import agno_transports
        toolkit = inspect.unwrap(profile.adapter._bindings["get_user_playlists"].entrypoint).__self__
        with patch.object(agno_transports, "_service_json", return_value=value):
            normalized = toolkit._make_request("me/playlists")
        self.assertEqual(json.dumps(value), before)
        self.assertEqual(normalized["items"][0]["tracks"], {"total": 3})

    async def test_spotify_playlist_invalid_or_conflicting_counts_are_refused(self):
        cases = [{"items": {"total": -1}}, {"items": {"total": True}},
                 {"items": {"total": "7"}}, {"items": []},
                 {"items": {"total": 7}, "tracks": {"total": 2}}]
        for fields in cases:
            with self.subTest(fields=fields):
                seen = []
                def send(adapter, request, **kwargs):
                    seen.append(request)
                    value = provider_value(request)
                    value["items"][0].pop("tracks")
                    value["items"][0].update(fields)
                    return response(request, value)
                profile = self.profile("spotify", ["get_user_playlists"])
                with patch.object(requests.adapters.HTTPAdapter, "send", send):
                    async with Client(create_server(profile)) as client:
                        result = await client.call_tool("get_user_playlists", {})
                self.assertTrue(result.is_error)
                self.assertEqual(len(seen), 1)

    async def test_selected_accounts_remain_isolated_after_environment_changes(self):
        for name in OPERATIONS:
            seen = []
            profiles = []
            def send(adapter, request, **kwargs):
                seen.append(request.headers.get("Authorization", request.headers.get("X-Shopify-Access-Token")))
                return response(request, provider_value(request))
            with patch.object(requests.adapters.HTTPAdapter, "send", send):
                for key in ("first-account", "second-account"):
                    os.environ[REFERENCE] = key
                    profiles.append(self.profile(name))
                os.environ[REFERENCE] = CANARY
                for profile in profiles:
                    op = OPERATIONS[name][0]
                    async with Client(create_server(profile)) as client:
                        self.assertFalse((await client.call_tool(op, arguments(op))).is_error)
            self.assertIn("first-account", seen[0])
            self.assertIn("second-account", seen[1])
            self.assertNotIn("first-account", seen[1])

    async def test_failures_are_redacted_do_not_redirect_retry_or_leave_response_open(self):
        for name in OPERATIONS:
            profile = self.profile(name)
            op = OPERATIONS[name][0]
            for mode in ("status", "redirect", "transport", "json", "large", "declared_large", "nonfinite", "raised_response"):
                calls = []
                closed = []
                logs = []
                def send(adapter, request, **kwargs):
                    calls.append(request)
                    if mode == "transport":
                        raise requests.ConnectionError(KEY + CANARY)
                    options = {"status": 503, "raw": (KEY + CANARY).encode()} if mode == "status" else \
                        {"status": 302, "headers": {"Location": "https://other-account.invalid/?" + KEY}} if mode == "redirect" else \
                        {"raw": b"[" + b" " * 1_048_576 + b"]"} if mode == "large" else \
                        {"headers": {"Content-Length": "1048577"}} if mode == "declared_large" else \
                        {"raw": b'{"x":NaN}'} if mode == "nonfinite" else {"raw": KEY.encode()}
                    result = response(request, {}, **options)
                    original = result.close
                    def close():
                        closed.append(result)
                        original()
                    result.close = close
                    if mode in ("status", "redirect", "declared_large"):
                        result.raw.read = lambda *args: self.fail("rejected body must not be read")
                    if mode == "raised_response":
                        raise requests.TooManyRedirects(KEY + CANARY, response=result)
                    return result
                with patch.object(requests.adapters.HTTPAdapter, "send", send), \
                     patch("agno.tools.discord.logger.exception", side_effect=lambda *a, **k: logs.append(traceback.format_exc())):
                    async with Client(create_server(profile)) as client:
                        result = await client.call_tool(op, arguments(op))
                self.assertTrue(result.is_error, (name, mode, result.model_dump_json()))
                self.assertEqual(len(calls), 1)
                self.assertEqual(len(closed), 0 if mode == "transport" else 1, (name, mode))
                self.assertNotIn(KEY, result.model_dump_json() + str(logs))
                self.assertNotIn(CANARY, result.model_dump_json() + str(logs))

    async def test_graphql_errors_remain_errors_even_with_http_success(self):
        profile = self.profile("shopify")
        with patch.object(requests.adapters.HTTPAdapter, "send", side_effect=lambda a, r, **kw: response(r, {"data": {"shop": {}}, "errors": [{"message": KEY}]}), autospec=True):
            async with Client(create_server(profile)) as client:
                result = await client.call_tool("get_shop_info", {})
        self.assertTrue(result.is_error)
        self.assertNotIn(KEY, result.model_dump_json())

    async def test_cancelled_owner_joins_actual_request_before_profile_cleanup(self):
        profile = self.profile("clickup", ["get_task"])
        entered, release = threading.Event(), threading.Event()
        def send(adapter, request, **kwargs):
            entered.set()
            if not release.wait(3):
                raise AssertionError("fixture worker did not finish")
            return response(request, provider_value(request))
        with patch.object(requests.adapters.HTTPAdapter, "send", send):
            owner = asyncio.create_task(_joined_worker(lambda: _invoke(profile, "get_task", arguments("get_task"))))
            try:
                async with asyncio.timeout(2):
                    while not entered.is_set():
                        await asyncio.sleep(0.01)
                owner.cancel()
                await asyncio.sleep(0.03)
                self.assertFalse(owner.done())
            finally:
                release.set()
            with self.assertRaises(asyncio.CancelledError):
                await owner
        profile.close()


if __name__ == "__main__":
    unittest.main()
