"""Explicit Zendesk authentication modes through the real Agno/MCP profile.

All credentials and provider responses are synthetic. Network is also denied
by the execution harness; HTTPAdapter.send is the only provider seam used here.
"""
from __future__ import annotations

import asyncio
import base64
from contextlib import ExitStack, redirect_stderr, redirect_stdout
import inspect
import io
import json
import os
from pathlib import Path
import tempfile
import threading
import unittest
from unittest.mock import patch
from urllib.parse import parse_qs, urlsplit

from agno.tools.zendesk import ZendeskTools
from mcp import Client
import requests

from meerkat_toolkit.adapters.agno_mcp import create_server, main
from meerkat_toolkit.adapters.agno_profiles import (
    ProfileConfigError, build_profile, configuration_help, list_profiles, validate_configuration,
)


REFERENCE = "SELECTED_ZENDESK_CREDENTIAL"
TOKEN = "synthetic-zendesk-oauth-token"
OTHER = "synthetic-other-account-token"
CANARY = "unselected-provider-or-environment-canary"
OPERATION = "search_zendesk"


def config(*, oauth=True, subdomain="selected-site", reference=REFERENCE):
    result = {"timeout": 3, "api_key_env": reference, "subdomain": subdomain}
    if oauth:
        result["auth_mode"] = "oauth_access_token"
    else:
        result["account_email"] = "reader@example.invalid"
    return result


def response(request, value=None, *, status=200, raw=None, headers=None):
    result = requests.Response()
    result.status_code = status
    result.url = request.url
    result.request = request
    result.headers.update({"Content-Type": "application/json", **(headers or {})})
    result.raw = io.BytesIO(raw if raw is not None else json.dumps(value).encode())
    return result


class OfflineMixin:
    def setUp(self):
        super().setUp()
        self.guards = ExitStack()
        self.addCleanup(self.guards.close)
        self.guards.enter_context(patch.dict(os.environ, {
            REFERENCE: TOKEN, "SECOND_CREDENTIAL": OTHER,
            "ZENDESK_USERNAME": CANARY, "ZENDESK_PASSWORD": CANARY,
            "ZENDESK_COMPANY_NAME": CANARY, "HTTPS_PROXY": "http://ambient.invalid",
            "NETRC": "/nonexistent/ambient-netrc", "REQUESTS_CA_BUNDLE": CANARY,
        }, clear=True))
        for target in ("socket.socket.connect", "socket.socket.connect_ex", "socket.getaddrinfo",
                       "requests.sessions.get_netrc_auth", "requests.get", "httpx.get", "httpx.post",
                       "httpx.Client.request", "agno.tools.zendesk.getenv"):
            self.guards.enter_context(patch(target, side_effect=AssertionError("unowned access forbidden")))

    def profile(self, settings=None):
        value = build_profile("zendesk", operations=[OPERATION], config=config() if settings is None else settings)
        self.addCleanup(value.close)
        return value


class ConfigurationTests(OfflineMixin, unittest.TestCase):
    def test_legacy_configuration_is_unchanged_and_modes_are_explicit(self):
        legacy = config(oauth=False)
        selected = validate_configuration("zendesk", operations=[OPERATION], config=legacy)
        self.assertEqual(selected.config, legacy)
        self.assertNotIn("auth_mode", selected.config)
        for value in (legacy, {**legacy, "auth_mode": "api_token"}, config()):
            self.assertEqual(validate_configuration("zendesk", operations=[OPERATION], config=value).config, value)
        profiles = list_profiles()["profiles"]
        self.assertEqual([p for p in profiles if p["profile"] == "zendesk"],
                         [{"profile": "zendesk", "operations": [OPERATION]}])
        self.assertEqual(len(profiles), 36)

    def test_rejects_ambiguous_authentication_and_does_not_infer_from_fields(self):
        invalid = [
            {**config(), "account_email": value} for value in (None, "", "reader@example.invalid")
        ]
        invalid += [{**config(), "auth_mode": value} for value in (None, True, 1, [], {}, "oauth", "OAuth", "", CANARY)]
        invalid += [
            {k: v for k, v in config().items() if k != "auth_mode"},
            {**config(), "auth_mode": "api_token"},
            {**config(), "access_token": CANARY}, {**config(), "access_token_env": "SECOND_CREDENTIAL"},
            {**config(), "api_key_env": TOKEN + "\n"},
            {**config(), "subdomain": "selected-site.zendesk.com"},
            {**config(), "subdomain": "selected-site/elsewhere"},
            {**config(), "subdomain": "https://selected-site"},
        ]
        for value in invalid:
            with self.subTest(value=value), self.assertRaises(ProfileConfigError) as caught:
                validate_configuration("zendesk", operations=[OPERATION], config=value)
            self.assertEqual(str(caught.exception), "Invalid Agno profile configuration")

    def test_help_explains_conditional_fields_and_separate_credential_ownership(self):
        help_ = configuration_help("zendesk")
        fields = {row["name"]: row for row in help_["config_fields"]}
        self.assertEqual(set(fields), {"timeout", "api_key_env", "subdomain", "auth_mode", "account_email"})
        self.assertFalse(fields["auth_mode"]["required"])
        self.assertFalse(fields["account_email"]["required"])
        self.assertTrue(fields["api_key_env"]["required"])
        self.assertTrue(fields["api_key_env"]["credential_reference"])
        self.assertFalse(fields["auth_mode"]["credential_reference"])
        text = " ".join(row["instruction"] for row in help_["prerequisites"])
        for required in ("auth_mode=oauth_access_token", "Omitted auth_mode", "requires account_email",
                         "never guesses", "does not acquire or refresh", "confirm the least required scope",
                         "client creator", "global OAuth", "October 27, 2026", "April 30, 2027"):
            self.assertIn(required, text)
        self.assertNotIn("hc:articles:search", text)
        self.assertNotIn(TOKEN, json.dumps(help_))

    def test_cli_check_preserves_selected_reference_without_reading_it(self):
        environment_get = os.environ.get
        reads = []

        def observed(name, default=None):
            reads.append(name)
            if name == REFERENCE:
                raise AssertionError("configuration read a credential")
            return environment_get(name, default)

        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "configuration.json"
            for settings in (config(), config(oauth=False)):
                path.write_text(json.dumps({"profile": "zendesk", "operations": [OPERATION], "config": settings}))
                out, err = io.StringIO(), io.StringIO()
                with patch.object(os.environ, "get", observed), redirect_stdout(out), redirect_stderr(err):
                    code = main(["check-config", "--config", str(path)])
                self.assertEqual(code, 0, err.getvalue())
                self.assertEqual(err.getvalue(), "")
                self.assertEqual(json.loads(out.getvalue()), {
                    "profile": "zendesk", "valid": True, "scope": "configuration_only",
                    "credential_environment_references": [{
                        "field_path": ["config", "api_key_env"], "environment_variable": REFERENCE}],
                })
        self.assertNotIn(REFERENCE, reads)

    def test_construction_never_reads_donor_environment_and_only_resolves_selected_secret(self):
        environment_get = os.environ.get
        reads = []

        def observed(name, default=None):
            reads.append(name)
            return environment_get(name, default)

        with patch.object(os.environ, "get", observed):
            profile = self.profile()
        self.assertEqual(reads.count(REFERENCE), 1)
        self.assertNotIn("SECOND_CREDENTIAL", reads)
        self.assertFalse(any(name.startswith("ZENDESK_") for name in reads))
        owner = inspect.unwrap(profile.adapter._bindings[OPERATION].entrypoint).__self__
        self.assertIsInstance(owner, ZendeskTools)
        self.assertEqual(owner.username, "toolkit-oauth-unused")
        self.assertEqual(owner.password, "toolkit-oauth-unused")
        self.assertNotIn(TOKEN, json.dumps(profile.descriptions))
        self.assertEqual(profile.descriptions, self.profile(config(oauth=False)).descriptions)

    def test_missing_or_invalid_selected_secret_refuses_before_provider_or_donor_fallback(self):
        for value in (None, "", "invalid token", TOKEN + "\n", "\u00e9", "x" * 4097):
            with self.subTest(value=value), patch.dict(os.environ, {}, clear=False):
                if value is None:
                    os.environ.pop(REFERENCE)
                else:
                    os.environ[REFERENCE] = value
                with patch.object(requests.adapters.HTTPAdapter, "send") as send, self.assertRaises(ProfileConfigError):
                    self.profile()
                send.assert_not_called()


class ProtocolTests(OfflineMixin, unittest.IsolatedAsyncioTestCase):
    async def test_bearer_request_and_result_use_existing_protocol_and_exact_first_page(self):
        for mode in ("auto", "legacy"):
            calls, closed = [], []

            def send(adapter, request, **kwargs):
                calls.append(request)
                self.assertEqual(adapter.max_retries.total, 0)
                self.assertEqual(kwargs["timeout"], 3)
                self.assertTrue(kwargs["stream"])
                self.assertTrue(kwargs["verify"])
                self.assertFalse(kwargs["proxies"])
                value = response(request, {"results": [{"body": "<h1>Title</h1><p>A &amp; B</p>"}],
                                           "next_page": "https://other-account.invalid/" + CANARY, "count": 100})
                original = value.close
                value.close = lambda: (closed.append(value), original())[-1]
                return value

            profile = self.profile()
            with patch.object(requests.adapters.HTTPAdapter, "send", send):
                async with Client(create_server(profile), mode=mode) as client:
                    self.assertEqual([tool.name for tool in (await client.list_tools()).tools], [OPERATION])
                    result = await client.call_tool(OPERATION, {"search_string": "caf\u00e9 &page=2"})
            self.assertFalse(result.is_error, result.model_dump_json())
            self.assertEqual(json.loads(result.structured_content["value"]), ["TitleA &amp; B"])
            self.assertEqual(len(calls), 1)
            self.assertEqual(len(closed), 1)
            request = calls[0]
            parsed = urlsplit(request.url)
            self.assertEqual((request.method, parsed.scheme, parsed.netloc, parsed.path),
                             ("GET", "https", "selected-site.zendesk.com", "/api/v2/help_center/articles/search.json"))
            self.assertEqual(parse_qs(parsed.query), {"query": ["caf\u00e9 &page=2"]})
            self.assertEqual(request.headers["Authorization"], "Bearer " + TOKEN)
            self.assertNotIn("toolkit-oauth-unused", str(request.headers) + request.url)
            self.assertNotIn(CANARY, str(request.headers) + request.url + result.model_dump_json())

    async def test_modes_and_accounts_keep_their_captured_credentials_after_environment_change(self):
        first = self.profile()
        second = self.profile(config(subdomain="second-site", reference="SECOND_CREDENTIAL"))
        legacy = self.profile(config(oauth=False, subdomain="legacy-site"))
        os.environ[REFERENCE] = CANARY
        os.environ["SECOND_CREDENTIAL"] = CANARY
        calls = []

        def send(adapter, request, **kwargs):
            calls.append(request)
            return response(request, {"results": []})

        async def call(profile):
            async with Client(create_server(profile)) as client:
                result = await client.call_tool(OPERATION, {"search_string": "article"})
                self.assertFalse(result.is_error)
                self.assertEqual(json.loads(result.structured_content["value"]), [])

        with patch.object(requests.adapters.HTTPAdapter, "send", send):
            await asyncio.gather(*(call(profile) for profile in (first, second, legacy)))
        self.assertEqual({urlsplit(r.url).hostname: r.headers["Authorization"] for r in calls}, {
            "selected-site.zendesk.com": "Bearer " + TOKEN,
            "second-site.zendesk.com": "Bearer " + OTHER,
            "legacy-site.zendesk.com": "Basic " + base64.b64encode(
                ("reader@example.invalid/token:" + TOKEN).encode()).decode(),
        })
        self.assertEqual(len(calls), 3)

    async def test_errors_never_retry_refresh_fallback_or_disclose_credentials(self):
        profile = self.profile()
        cases = [(401, None), (403, None), (429, None), (503, None), (302, None),
                 (200, b"not-json-" + CANARY.encode()), (200, b'{"results":{}}'),
                 (200, b'{"results":[{"body":null}]}'), (200, b'{"results":[{}]}'),
                 (200, b'{"results":NaN}'), (200, b"x" * 1_048_577), (None, None)]
        for status, raw in cases:
            calls, closed = [], []

            def send(adapter, request, **kwargs):
                calls.append(request)
                if status is None:
                    raise requests.Timeout(TOKEN + CANARY, request=request)
                value = response(request, {"error": TOKEN + CANARY}, status=status, raw=raw,
                                 headers={"Location": "https://other-account.invalid/" + CANARY})
                original = value.close
                value.close = lambda: (closed.append(value), original())[-1]
                return value

            with self.subTest(status=status, raw_size=None if raw is None else len(raw)), patch.object(
                    requests.adapters.HTTPAdapter, "send", send):
                async with Client(create_server(profile)) as client:
                    result = await client.call_tool(OPERATION, {"search_string": "article"})
                self.assertTrue(result.is_error)
                self.assertEqual(len(calls), 1)
                self.assertEqual(calls[0].headers["Authorization"], "Bearer " + TOKEN)
                self.assertEqual(len(closed), 0 if status is None else 1)
                for secret in (TOKEN, OTHER, CANARY):
                    self.assertNotIn(secret, result.model_dump_json())

    async def test_invalid_search_and_unselected_operations_refuse_before_provider(self):
        profile = self.profile()
        with patch.object(requests.adapters.HTTPAdapter, "send", side_effect=AssertionError("must not enter")) as send:
            async with Client(create_server(profile)) as client:
                for arguments in ({"search_string": ""}, {"search_string": "x" * 1025},
                                  {"search_string": "ok", "account_email": "other@example.invalid"},
                                  {"search_string": "ok", "auth_mode": "api_token"}):
                    result = await client.call_tool(OPERATION, arguments)
                    self.assertTrue(result.is_error)
                    self.assertFalse(result.structured_content["error"]["entered"])
                self.assertTrue((await client.call_tool("create_ticket", {})).is_error)
            send.assert_not_called()

    async def test_cancellation_retains_entered_request_until_joined_without_auth_retry(self):
        profile = self.profile()
        entered, release = threading.Event(), threading.Event()
        calls, closed = [], []

        def send(adapter, request, **kwargs):
            calls.append(request)
            entered.set()
            if not release.wait(5):
                raise AssertionError("test release missing")
            value = response(request, {"results": []})
            original = value.close
            value.close = lambda: (closed.append(value), original())[-1]
            return value

        with patch.object(requests.adapters.HTTPAdapter, "send", send):
            try:
                async with Client(create_server(profile)) as client:
                    task = asyncio.create_task(client.call_tool(OPERATION, {"search_string": "article"}))
                    self.assertTrue(await asyncio.to_thread(entered.wait, 3))
                    task.cancel()
                    await asyncio.sleep(0)
                    self.assertEqual(len(calls), 1)
                    release.set()
                    with self.assertRaises(asyncio.CancelledError):
                        await task
            finally:
                release.set()
        self.assertEqual(len(calls), 1)
        self.assertEqual(len(closed), 1)
        self.assertEqual(calls[0].headers["Authorization"], "Bearer " + TOKEN)


if __name__ == "__main__":
    unittest.main()
