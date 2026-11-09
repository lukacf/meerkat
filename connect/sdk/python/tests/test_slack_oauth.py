"""Local transport fixtures for the Slack known-account protocol helper."""

import asyncio
import base64
from contextlib import asynccontextmanager, suppress
import hashlib
import json
import unittest
from unittest.mock import patch
from urllib.parse import parse_qs, urlsplit

import anyio
import httpx

from meerkat_toolkit.auth import slack_oauth as oauth
from meerkat_toolkit.httpx_client import prepare_httpx_client


SECRET = "fixture-private-value"
ACCESS = "opaque-user-access-value"
VERIFIER = "V" * 43
SCOPES = frozenset({"channels:history", "channels:read"})


def spec(**changes):
    values = dict(client_id="123.456", app_id="ATEST", redirect_uri="http://localhost:41234/callback",
                  expected_team_id="TTEST", expected_user_id="WTEST", requested_scopes=SCOPES)
    values.update(changes)
    return oauth.SlackReconnectSpec(**values)


def document(value):
    return json.dumps(value, separators=(",", ":")).encode()


class Fixture:
    def __init__(self):
        self.token = {"ok": True, "app_id": "ATEST", "team": {"id": "TTEST"},
                      "enterprise": None,
                      "authed_user": {"id": "WTEST", "access_token": ACCESS,
                                      "token_type": "user", "scope": ",".join(sorted(SCOPES))}}
        self.probe = {"ok": True, "team_id": "TTEST", "user_id": "WTEST", "user": SECRET,
                      "url": "https://unselected.invalid/private"}
        self.calls, self.closed = [], []
        self.raw, self.status, self.headers, self.fail, self.before = {}, {}, {}, {}, {}
        self.gate_url = None
        self.entered, self.release = asyncio.Event(), asyncio.Event()

    async def handle(self, request):
        self.calls.append(request)
        url = str(request.url)
        if url in self.before:
            self.before[url]()
        if url in self.fail:
            raise self.fail[url]
        values = {oauth.TOKEN_ENDPOINT: self.token, oauth.IDENTITY_ENDPOINT: self.probe}
        if url not in values:
            raise AssertionError("Unexpected fixture URL")
        raw = self.raw.get(url, document(values[url]))
        owner = self

        class Stream(httpx.AsyncByteStream):
            async def __aiter__(self):
                yield raw

            async def aclose(self):
                if owner.gate_url == url:
                    owner.entered.set()
                    await owner.release.wait()
                owner.closed.append(url)

        headers = [("Content-Type", "application/json"), *self.headers.get(url, [])]
        return httpx.Response(self.status.get(url, 200), request=request,
                              headers=headers, stream=Stream())


@asynccontextmanager
async def client_for(fixture, **kwargs):
    async with httpx.AsyncClient(transport=httpx.MockTransport(fixture.handle), trust_env=False,
                                **kwargs) as client:
        prepare_httpx_client(client)
        yield client


async def exchange(client, chosen=None, **kwargs):
    return await oauth.exchange_and_observe(chosen or spec(), code="fixture-code",
        code_verifier=VERIFIER, http_client=client, **kwargs)


def refresh_spec(**changes):
    values = dict(client_id="123.456", app_id="ATEST", expected_team_id="TTEST",
                  expected_user_id="WTEST", required_scopes=SCOPES)
    values.update(changes)
    return oauth.SlackRefreshSpec(**values)


class RefreshFixture(Fixture):
    def __init__(self):
        super().__init__()
        # Slack's OAuthV2TokenRefreshResponse puts user token fields here,
        # unlike the authorization-code response's nested authed_user.
        self.token = {"ok": True, "app_id": "ATEST", "team": {"id": "TTEST"},
                      "enterprise": None, "is_enterprise_install": False,
                      "access_token": ACCESS, "token_type": "user",
                      "scope": ",".join(sorted(SCOPES)), "expires_in": 43200,
                      "refresh_token": "replacement-private-value"}


async def refresh(client, chosen=None, **kwargs):
    return await oauth.refresh_and_observe(chosen or refresh_spec(), refresh_token=SECRET,
                                          http_client=client, **kwargs)


class SlackOAuthTests(unittest.IsolatedAsyncioTestCase):
    def assert_safe_error(self, error):
        self.assertNotIn(SECRET, str(error))
        self.assertNotIn(ACCESS, repr(error))
        self.assertIsNone(error.__context__)
        self.assertIsNone(error.__cause__)

    def test_exact_authorization_request_and_localhost_profile(self):
        challenge = base64.urlsafe_b64encode(hashlib.sha256(VERIFIER.encode()).digest()).decode().rstrip("=")
        request = oauth.authorization_request(spec(), state=SECRET, code_challenge=challenge)
        url = urlsplit(request.url)
        self.assertEqual(url.scheme + "://" + url.netloc + url.path, oauth.AUTHORIZATION_ENDPOINT)
        self.assertEqual(parse_qs(url.query, keep_blank_values=True), {
            "client_id": ["123.456"], "redirect_uri": [spec().redirect_uri], "scope": [""],
            "user_scope": [",".join(sorted(SCOPES))], "state": [SECRET],
            "code_challenge": [challenge], "code_challenge_method": ["S256"], "team": ["TTEST"]})
        self.assertNotIn(SECRET, repr(request))
        for redirect in ("http://127.0.0.1:1234/a", "http://[::1]:1234/a", "https://localhost:1234/a",
                         "http://localhost/a", "http://localhost:1234", "http://localhost:1234/a?",
                         "http://localhost:1234/a#", "http://u:p@localhost:1234/a",
                         "http://localhost.:1234/a", "myapp://callback", "http://LOCALHOST:1234/a"):
            with self.subTest(redirect=redirect), self.assertRaises(oauth.SlackOAuthError) as caught:
                spec(redirect_uri=redirect)
            self.assert_safe_error(caught.exception)
        for scopes in (set(SCOPES), frozenset(), frozenset({"openid"}), frozenset({"chat:write"})):
            with self.assertRaises(oauth.SlackOAuthError):
                spec(requested_scopes=scopes)

    async def test_user_branch_fixed_requests_no_ambient_auth_and_private_result(self):
        fixture = Fixture()
        fixture.token.update(access_token=SECRET, token_type="bot", scope="chat:write",
                             refresh_token=SECRET, expires_in=1)
        fixture.headers[oauth.TOKEN_ENDPOINT] = [("Set-Cookie", "sid=private; Path=/")]

        class NeverAuth(httpx.Auth):
            def auth_flow(self, request):
                raise AssertionError(SECRET)
                yield request

        async with client_for(fixture, auth=NeverAuth(), headers={"X-Private": SECRET},
                              params={"secret": SECRET}, cookies={"ambient": SECRET}) as client:
            result = await exchange(client)
            self.assertFalse(client.is_closed)
        self.assertEqual([str(r.url) for r in fixture.calls], [oauth.TOKEN_ENDPOINT, oauth.IDENTITY_ENDPOINT])
        self.assertEqual([r.method for r in fixture.calls], ["POST", "POST"])
        self.assertEqual(parse_qs(fixture.calls[0].content.decode()), {
            "client_id": ["123.456"], "code": ["fixture-code"], "code_verifier": [VERIFIER],
            "redirect_uri": [spec().redirect_uri], "grant_type": ["authorization_code"]})
        self.assertNotIn("authorization", fixture.calls[0].headers)
        self.assertEqual(fixture.calls[1].headers["authorization"], "Bearer " + ACCESS)
        self.assertEqual(fixture.calls[1].content, b"")
        for request in fixture.calls:
            self.assertNotIn("cookie", request.headers)
            self.assertNotIn("x-private", request.headers)
        self.assertEqual(result.access_token, ACCESS)
        self.assertIsNone(result.refresh_token)
        self.assertIsNone(result.expires_in)
        self.assertIsNone(result.expires_at)
        self.assertEqual(result.observation.scope_evidence, oauth.ScopeEvidence.TOKEN_ENDPOINT_RESPONSE)
        self.assertIsNone(result.observation.exchange_is_enterprise_install)
        self.assertIsNone(result.observation.probe_is_enterprise_install)
        for secret in (SECRET, ACCESS, VERIFIER):
            self.assertNotIn(secret, repr(result))
            self.assertNotIn(secret, json.dumps(result.safe_view()))
        view = result.safe_view()
        view["granted_scopes"].clear()
        self.assertEqual(set(result.observation.granted_scopes), SCOPES)
        self.assertEqual(fixture.closed, [oauth.TOKEN_ENDPOINT, oauth.IDENTITY_ENDPOINT])

    async def test_current_scope_header_owns_full_set_without_union(self):
        for reported in (SCOPES, SCOPES | {"im:read"}, SCOPES | {"groups:read", "im:read"}):
            fixture = Fixture()
            fixture.token["authed_user"]["scope"] += ",groups:read"
            fixture.headers[oauth.IDENTITY_ENDPOINT] = [("X-OAuth-Scopes", ",".join(reported))]
            async with client_for(fixture) as client:
                result = await exchange(client)
            self.assertEqual(set(result.observation.granted_scopes), reported)
            self.assertEqual(set(result.observation.exchange_scopes), SCOPES | {"groups:read"})
        fixture = Fixture()
        fixture.token["authed_user"]["scope"] += ",groups:read"
        fixture.headers[oauth.IDENTITY_ENDPOINT] = [
            ("X-OAuth-Scopes", " channels:read,\tchannels:history ,im:read "),
            ("X-Accepted-OAuth-Scopes", "chat:write")]
        async with client_for(fixture) as client:
            result = await exchange(client)
        self.assertEqual(set(result.observation.exchange_scopes), SCOPES | {"groups:read"})
        self.assertEqual(set(result.observation.granted_scopes), SCOPES | {"im:read"})
        self.assertEqual(result.observation.introspection_scopes, result.observation.granted_scopes)
        self.assertEqual(result.observation.scope_evidence, oauth.ScopeEvidence.PROVIDER_INTROSPECTION)
        fixture = Fixture()
        fixture.headers[oauth.IDENTITY_ENDPOINT] = [("X-Accepted-OAuth-Scopes", "chat:write")]
        async with client_for(fixture) as client:
            result = await exchange(client)
        self.assertIsNone(result.observation.introspection_scopes)
        self.assertEqual(set(result.observation.granted_scopes), SCOPES)

    async def test_missing_or_malformed_scope_never_falls_back(self):
        for value in (None, "", "channels:read", "channels:read,channels:history,channels:read",
                      "channels:history,,channels:read", "channels:history,channels:read\n", "x" * 16385):
            for at_token in (True, False):
                fixture = Fixture()
                if at_token:
                    fixture.token["authed_user"]["scope"] = value
                    fixture.headers[oauth.IDENTITY_ENDPOINT] = [("X-OAuth-Scopes", ",".join(SCOPES))]
                else:
                    fixture.headers[oauth.IDENTITY_ENDPOINT] = [("X-OAuth-Scopes", value or "")]
                async with client_for(fixture) as client:
                    with self.subTest(value=value, at_token=at_token), self.assertRaises(oauth.SlackOAuthError) as caught:
                        await exchange(client)
                    self.assert_safe_error(caught.exception)
                self.assertEqual(len(fixture.calls), 1 if at_token else 2)
        fixture = Fixture()
        fixture.headers[oauth.IDENTITY_ENDPOINT] = [("X-OAuth-Scopes", ",".join(SCOPES))] * 2
        async with client_for(fixture) as client:
            with self.assertRaises(oauth.SlackOAuthError):
                await exchange(client)

    async def test_exact_account_and_kind_refusals(self):
        mutations = [
            ("token", lambda f: f.token.update(app_id="AOTHER")),
            ("token", lambda f: f.token["team"].update(id="TOTHER")),
            ("token", lambda f: f.token["authed_user"].update(id="WOTHER")),
            ("token", lambda f: f.token["authed_user"].update(token_type="bot")),
            ("token", lambda f: f.token.update(team=None)),
            ("identity", lambda f: f.probe.update(team_id="TOTHER")),
            ("identity", lambda f: f.probe.update(user_id="WOTHER")),
            ("identity", lambda f: f.probe.update(app_id="AOTHER")),
            ("identity", lambda f: f.probe.update(token_type="bot")),
            ("identity", lambda f: f.probe.update(bot_id="BOTHER")),
        ]
        for phase, mutate in mutations:
            fixture = Fixture()
            mutate(fixture)
            async with client_for(fixture) as client:
                with self.subTest(phase=phase), self.assertRaises(oauth.SlackOAuthError) as caught:
                    await exchange(client)
                self.assert_safe_error(caught.exception)
                self.assertEqual(caught.exception.phase, phase)
            self.assertEqual(len(fixture.calls), 1 if phase == "token" else 2)

    async def test_enterprise_binding_and_install_mode_presence(self):
        for pinned in (None, "ETEST"):
            fixture = Fixture()
            fixture.token.update(enterprise={"id": "ETEST"}, is_enterprise_install=False)
            fixture.probe.update(enterprise_id="ETEST", is_enterprise_install=False)
            async with client_for(fixture) as client:
                result = await exchange(client, spec(expected_enterprise_id=pinned))
            self.assertEqual(result.observation.exchange_enterprise_id, "ETEST")
            self.assertEqual(result.observation.probe_enterprise_id, "ETEST")
            self.assertIs(result.observation.exchange_is_enterprise_install, False)
        cases = [(None, "EOTHER", None), (None, None, "ETEST"), ("ETEST", "EOTHER", "ETEST"),
                 ("ETEST", None, "ETEST")]
        for exchange_id, probe_id, pinned in cases:
            fixture = Fixture()
            fixture.token["enterprise"] = None if exchange_id is None else {"id": exchange_id}
            if probe_id is not None:
                fixture.probe["enterprise_id"] = probe_id
            async with client_for(fixture) as client:
                with self.subTest(case=(exchange_id, probe_id, pinned)), self.assertRaises(oauth.SlackOAuthError):
                    await exchange(client, spec(expected_enterprise_id=pinned))
        fixture = Fixture()
        fixture.token.pop("enterprise")
        fixture.probe["enterprise_id"] = "ETEST"
        async with client_for(fixture) as client:
            result = await exchange(client, spec(expected_enterprise_id="ETEST"))
        self.assertFalse(result.observation.exchange_enterprise_present)
        for where in ("token", "probe"):
            for value in (True, None, 0, "false"):
                fixture = Fixture()
                getattr(fixture, where)["is_enterprise_install"] = value
                async with client_for(fixture) as client:
                    with self.subTest(where=where, value=value), self.assertRaises(oauth.SlackOAuthError):
                        await exchange(client)

    async def test_expiry_uses_exchange_time_and_user_rotation_pair_only(self):
        fixture = Fixture()
        fixture.token["authed_user"].update(expires_in=43200, refresh_token=SECRET)
        async with client_for(fixture) as client:
            result = await exchange(client)
        self.assertEqual(result.refresh_token, SECRET)
        self.assertEqual(result.expires_in, 43200)
        self.assertLessEqual(result.expires_at, result.token_received_at + 43200)
        for fields in ({"expires_in": 43200}, {"refresh_token": SECRET},
                       {"expires_in": True, "refresh_token": SECRET},
                       {"expires_in": 0, "refresh_token": SECRET}):
            fixture = Fixture()
            fixture.token["authed_user"].update(fields)
            async with client_for(fixture) as client:
                with self.assertRaises(oauth.SlackOAuthError):
                    await exchange(client)
            self.assertEqual(len(fixture.calls), 1)
        fixture = Fixture()
        fixture.token["authed_user"].update(expires_in=10, refresh_token=SECRET)
        clock = [1000.0]
        fixture.before[oauth.IDENTITY_ENDPOINT] = lambda: clock.__setitem__(0, 1011.0)
        async with client_for(fixture) as client:
            with patch.object(oauth.time, "time", side_effect=lambda: clock[0]):
                with self.assertRaises(oauth.SlackOAuthError) as caught:
                    await exchange(client)
        self.assertEqual(caught.exception.reason, "expired")
        self.assertEqual(len(fixture.calls), 2)

    async def test_json_wire_bounds_and_duplicate_fields_refuse_before_probe(self):
        raw_cases = [b'{"ok":true,"ok":false}', b'{"ok":NaN}', b'{"ok":true,"x":1.5}',
                     b'{"ok":true,"x":"\xff"}', b'{"ok":true,"x":' + b'[' * 14 + b'0' + b']' * 14 + b'}',
                     document({"ok": True, "x": list(range(4100))}), b" " * 65537]
        for raw in raw_cases:
            fixture = Fixture()
            fixture.raw[oauth.TOKEN_ENDPOINT] = raw
            async with client_for(fixture) as client:
                with self.subTest(size=len(raw)), self.assertRaises(oauth.SlackOAuthError) as caught:
                    await exchange(client)
                self.assert_safe_error(caught.exception)
            self.assertEqual(len(fixture.calls), 1)
            self.assertEqual(fixture.closed, [oauth.TOKEN_ENDPOINT])
        for headers in ([('Content-Encoding', 'gzip')], [('Content-Length', '65537')],
                        [('Content-Length', '1')], [('Content-Length', '1'), ('Content-Length', '1')]):
            fixture = Fixture()
            fixture.headers[oauth.TOKEN_ENDPOINT] = headers
            async with client_for(fixture) as client:
                with self.assertRaises(oauth.SlackOAuthError):
                    await exchange(client)
            self.assertEqual(len(fixture.calls), 1)

    async def test_provider_failures_redirects_and_transport_never_replay(self):
        for url in (oauth.TOKEN_ENDPOINT, oauth.IDENTITY_ENDPOINT):
            for status in (302, 401, 429, 500, 200):
                fixture = Fixture()
                fixture.status[url] = status
                fixture.headers[url] = [("Location", "https://unselected.invalid/"), ("Retry-After", "0")]
                if status == 200:
                    fixture.raw[url] = document({"ok": False, "error": SECRET})
                async with client_for(fixture, follow_redirects=True) as client:
                    with self.assertRaises(oauth.SlackOAuthError) as caught:
                        await exchange(client)
                    self.assert_safe_error(caught.exception)
                self.assertEqual(len(fixture.calls), 1 if url == oauth.TOKEN_ENDPOINT else 2)
                self.assertEqual(caught.exception.exchange_state, oauth.ExchangeState.RESPONSE_RECEIVED)
            fixture = Fixture()
            fixture.fail[url] = httpx.ConnectError(SECRET)
            async with client_for(fixture) as client:
                with self.assertRaises(oauth.SlackOAuthError) as caught:
                    await exchange(client)
                self.assert_safe_error(caught.exception)
            self.assertEqual(caught.exception.exchange_state, oauth.ExchangeState.UNCONFIRMED
                             if url == oauth.TOKEN_ENDPOINT else oauth.ExchangeState.RESPONSE_RECEIVED)

    async def test_invalid_inputs_and_unprepared_client_no_request(self):
        fixture = Fixture()
        async with httpx.AsyncClient(transport=httpx.MockTransport(fixture.handle)) as client:
            with self.assertRaises(oauth.SlackOAuthError) as caught:
                await exchange(client)
            self.assert_safe_error(caught.exception)
            prepare_httpx_client(client)
            for timeout in (True, 0, -1, 121, float("nan"), float("inf")):
                with self.assertRaises(oauth.SlackOAuthError) as caught:
                    await exchange(client, timeout_seconds=timeout)
                self.assertEqual(caught.exception.exchange_state, oauth.ExchangeState.NOT_STARTED)
            for verifier in ("short", "a" * 129, SECRET + "\n" + "v" * 50):
                with self.assertRaises(oauth.SlackOAuthError):
                    await oauth.exchange_and_observe(spec(), code="code", code_verifier=verifier, http_client=client)
        self.assertEqual(fixture.calls, [])

    async def test_repeated_task_cancellation_joins_each_response_close(self):
        for url in (oauth.TOKEN_ENDPOINT, oauth.IDENTITY_ENDPOINT):
            fixture = Fixture()
            fixture.gate_url = url
            async with client_for(fixture) as client:
                task = asyncio.create_task(exchange(client))
                try:
                    await asyncio.wait_for(fixture.entered.wait(), 2)
                    for _ in range(3):
                        task.cancel()
                        await asyncio.sleep(0)
                    self.assertFalse(task.done())
                    fixture.release.set()
                    with self.assertRaises(oauth.SlackOAuthCancelled) as caught:
                        await asyncio.wait_for(task, 2)
                    self.assert_safe_error(caught.exception)
                    self.assertFalse(client.is_closed)
                finally:
                    fixture.release.set()
                    with suppress(asyncio.CancelledError, Exception):
                        await task
            self.assertEqual(len(fixture.calls), 1 if url == oauth.TOKEN_ENDPOINT else 2)
            self.assertEqual(fixture.closed.count(url), 1)

    async def test_anyio_cancellation_and_deadline_join_without_next_request(self):
        for deadline in (False, True):
            fixture = Fixture()
            fixture.gate_url = oauth.TOKEN_ENDPOINT
            errors = []
            parent_scope = anyio.CancelScope()

            async def run(client):
                with parent_scope:
                    try:
                        await exchange(client, timeout_seconds=0.03 if deadline else 20)
                    except (oauth.SlackOAuthCancelled, oauth.SlackOAuthError) as error:
                        errors.append(error)

            async with client_for(fixture) as client:
                task = asyncio.create_task(run(client))
                try:
                    await asyncio.wait_for(fixture.entered.wait(), 2)
                    if deadline:
                        await asyncio.sleep(0.06)
                    else:
                        parent_scope.cancel()
                    await asyncio.sleep(0)
                    self.assertFalse(task.done())
                    fixture.release.set()
                    await asyncio.wait_for(task, 2)
                finally:
                    fixture.release.set()
                    with suppress(asyncio.CancelledError, Exception):
                        await task
            self.assertEqual(len(errors), 1)
            if deadline:
                self.assertEqual(errors[0].reason, "deadline")
            else:
                self.assertIsInstance(errors[0], oauth.SlackOAuthCancelled)
                # Cancellation is observed at the next step's pre-send
                # checkpoint after token close; no identity request occurred.
                self.assertEqual(errors[0].phase, "identity")
            self.assertEqual(len(fixture.calls), 1)
            self.assertEqual(fixture.closed, [oauth.TOKEN_ENDPOINT])


class SlackRefreshTests(unittest.IsolatedAsyncioTestCase):
    assert_safe_error = SlackOAuthTests.assert_safe_error

    async def test_public_refresh_exact_wire_probe_and_private_result(self):
        fixture = RefreshFixture()
        fixture.token["bot_user_id"] = "BANCILLARY"
        fixture.token["authed_user"] = {"id": "WOTHER", "access_token": SECRET,
                                        "scope": "chat:write", "token_type": "bot"}

        class NeverAuth(httpx.Auth):
            def auth_flow(self, request):
                raise AssertionError(SECRET)
                yield request

        async with client_for(fixture, auth=NeverAuth(), headers={"X-Private": SECRET},
                              params={"secret": SECRET}, cookies={"ambient": SECRET}) as client:
            result = await refresh(client)
            self.assertFalse(client.is_closed)
        self.assertEqual([str(r.url) for r in fixture.calls], [oauth.TOKEN_ENDPOINT, oauth.IDENTITY_ENDPOINT])
        self.assertEqual([r.method for r in fixture.calls], ["POST", "POST"])
        self.assertEqual(parse_qs(fixture.calls[0].content.decode()), {
            "client_id": ["123.456"], "grant_type": ["refresh_token"], "refresh_token": [SECRET]})
        self.assertNotIn("authorization", fixture.calls[0].headers)
        self.assertEqual(fixture.calls[1].headers["authorization"], "Bearer " + ACCESS)
        self.assertEqual(fixture.calls[1].content, b"")
        for request in fixture.calls:
            self.assertNotIn("cookie", request.headers)
            self.assertNotIn("x-private", request.headers)
        self.assertEqual(result.access_token, ACCESS)
        self.assertEqual(result.refresh_token, fixture.token["refresh_token"])
        self.assertEqual(result.expires_in, 43200)
        self.assertLessEqual(result.expires_at, result.token_received_at + 43200)
        self.assertEqual(result.observation.user_id, "WTEST")
        self.assertNotIn("user_id", fixture.token)  # Observed by auth.test, never inferred from token text.
        self.assertEqual(result.observation.scope_evidence, oauth.ScopeEvidence.TOKEN_ENDPOINT_RESPONSE)
        view = result.safe_view()
        for secret in (SECRET, ACCESS, result.refresh_token):
            self.assertNotIn(secret, repr(result))
            self.assertNotIn(secret, json.dumps(view))
        view["granted_scopes"].append("chat:write")
        self.assertNotIn("chat:write", result.observation.granted_scopes)
        self.assertEqual(fixture.closed, [oauth.TOKEN_ENDPOINT, oauth.IDENTITY_ENDPOINT])

    async def test_nested_code_exchange_cannot_stand_in_for_refresh(self):
        fixture = Fixture()
        fixture.token["authed_user"].update(refresh_token="replacement", expires_in=43200)
        async with client_for(fixture) as client:
            with self.assertRaises(oauth.SlackOAuthError) as caught:
                await refresh(client)
            self.assert_safe_error(caught.exception)
        self.assertEqual(len(fixture.calls), 1)

    async def test_refresh_pair_mandatory_bounded_and_replacement_required(self):
        for fields in ({"refresh_token": None}, {"refresh_token": ""}, {"refresh_token": SECRET},
                       {"refresh_token": "x" * 8193}, {"expires_in": None}, {"expires_in": True},
                       {"expires_in": 0}, {"expires_in": -1}, {"expires_in": 2**31},
                       {"access_token": None}, {"access_token": "bad\nvalue"}):
            fixture = RefreshFixture()
            fixture.token.update(fields)
            async with client_for(fixture) as client:
                with self.subTest(fields=list(fields)), self.assertRaises(oauth.SlackOAuthError) as caught:
                    await refresh(client)
                self.assert_safe_error(caught.exception)
            self.assertEqual(len(fixture.calls), 1)
            self.assertEqual(caught.exception.exchange_state, oauth.ExchangeState.RESPONSE_RECEIVED)
        for absent in ("expires_in", "refresh_token"):
            fixture = RefreshFixture()
            del fixture.token[absent]
            async with client_for(fixture) as client:
                with self.assertRaises(oauth.SlackOAuthError):
                    await refresh(client)
            self.assertEqual(len(fixture.calls), 1)

    async def test_refresh_exact_account_and_optional_identity_cannot_contradict(self):
        cases = [("token", "app_id", "AOTHER"), ("token", "team", {"id": "TOTHER"}),
                 ("token", "team", None), ("token", "token_type", "bot"),
                 ("token", "user_id", "WOTHER"), ("probe", "user_id", "WOTHER"),
                 ("probe", "user_id", None), ("probe", "team_id", "TOTHER"),
                 ("probe", "app_id", "AOTHER"), ("probe", "token_type", "bot"),
                 ("probe", "bot_id", None)]
        for where, field, value in cases:
            fixture = RefreshFixture()
            getattr(fixture, where)[field] = value
            async with client_for(fixture) as client:
                with self.subTest(where=where, field=field), self.assertRaises(oauth.SlackOAuthError) as caught:
                    await refresh(client)
                self.assert_safe_error(caught.exception)
            self.assertEqual(len(fixture.calls), 1 if where == "token" else 2)

    async def test_refresh_full_scope_reports_distinct_and_required_subset(self):
        for header in (None, "channels:read,channels:history,im:read"):
            fixture = RefreshFixture()
            fixture.token["scope"] += ",chat:write"
            if header:
                fixture.headers[oauth.IDENTITY_ENDPOINT] = [("X-OAuth-Scopes", header)]
            async with client_for(fixture) as client:
                result = await refresh(client)
            self.assertEqual(set(result.observation.exchange_scopes), SCOPES | {"chat:write"})
            self.assertEqual(set(result.observation.granted_scopes), SCOPES | ({"im:read"} if header else {"chat:write"}))
            self.assertEqual(result.observation.scope_evidence, oauth.ScopeEvidence.PROVIDER_INTROSPECTION
                             if header else oauth.ScopeEvidence.TOKEN_ENDPOINT_RESPONSE)
        for where in ("token", "probe"):
            for value in ("channels:read", "", "channels:read,channels:history,channels:read"):
                fixture = RefreshFixture()
                if where == "token":
                    fixture.token["scope"] = value
                else:
                    fixture.headers[oauth.IDENTITY_ENDPOINT] = [("X-OAuth-Scopes", value)]
                async with client_for(fixture) as client:
                    with self.subTest(where=where, value=value), self.assertRaises(oauth.SlackOAuthError):
                        await refresh(client)
                self.assertEqual(len(fixture.calls), 1 if where == "token" else 2)

    async def test_refresh_enterprise_pin_presence_and_organization_refusal(self):
        fixture = RefreshFixture()
        fixture.token.pop("enterprise")
        fixture.probe["enterprise_id"] = "ETEST"
        async with client_for(fixture) as client:
            result = await refresh(client, refresh_spec(expected_enterprise_id="ETEST"))
        self.assertFalse(result.observation.exchange_enterprise_present)
        self.assertTrue(result.observation.probe_enterprise_present)
        for where, field, value in (("token", "enterprise", {"id": "EOTHER"}),
                                    ("probe", "enterprise_id", None),
                                    ("probe", "enterprise_id", "EOTHER"),
                                    ("token", "is_enterprise_install", True),
                                    ("probe", "is_enterprise_install", True),
                                    ("token", "is_enterprise_install", 0)):
            fixture = RefreshFixture()
            fixture.token["enterprise"] = {"id": "ETEST"}
            fixture.probe["enterprise_id"] = "ETEST"
            getattr(fixture, where)[field] = value
            async with client_for(fixture) as client:
                with self.subTest(where=where, field=field), self.assertRaises(oauth.SlackOAuthError):
                    await refresh(client, refresh_spec(expected_enterprise_id="ETEST"))

    async def test_refresh_expires_before_probe_and_during_probe(self):
        for where in (oauth.TOKEN_ENDPOINT, oauth.IDENTITY_ENDPOINT):
            fixture = RefreshFixture()
            fixture.token["expires_in"] = 10
            clock = [1000.0]
            fixture.before[where] = lambda: clock.__setitem__(0, 1011.0)
            async with client_for(fixture) as client:
                with patch.object(oauth.time, "time", side_effect=lambda: clock[0]):
                    with self.assertRaises(oauth.SlackOAuthError) as caught:
                        await refresh(client)
            self.assertEqual(caught.exception.reason, "expired")
            self.assertEqual(len(fixture.calls), 1 if where == oauth.TOKEN_ENDPOINT else 2)

    async def test_refresh_invalid_inputs_never_enter_and_revalidate_frozen_spec(self):
        fixture = RefreshFixture()
        for fields in ({"required_scopes": frozenset()}, {"required_scopes": set(SCOPES)},
                       {"required_scopes": frozenset({"chat:write"})}, {"client_id": "bad\nvalue"},
                       {"expected_team_id": ""}, {"expected_enterprise_id": 1}):
            with self.assertRaises(oauth.SlackOAuthError) as caught:
                refresh_spec(**fields)
            self.assert_safe_error(caught.exception)
        async with httpx.AsyncClient(transport=httpx.MockTransport(fixture.handle)) as client:
            with self.assertRaises(oauth.SlackOAuthError):
                await refresh(client)
            prepare_httpx_client(client)
            forged = refresh_spec()
            object.__setattr__(forged, "required_scopes", frozenset({"chat:write"}))
            for chosen in (forged, spec()):
                with self.assertRaises(oauth.SlackOAuthError):
                    await refresh(client, chosen)
            for timeout in (True, 0, -1, 121, float("nan"), float("inf")):
                with self.assertRaises(oauth.SlackOAuthError):
                    await refresh(client, timeout_seconds=timeout)
            for secret in (None, "", "x" * 8193, SECRET + "\n"):
                with self.assertRaises(oauth.SlackOAuthError) as caught:
                    await oauth.refresh_and_observe(refresh_spec(), refresh_token=secret, http_client=client)
                self.assert_safe_error(caught.exception)
                self.assertEqual(caught.exception.exchange_state, oauth.ExchangeState.NOT_STARTED)
        self.assertEqual(fixture.calls, [])

    async def test_refresh_unknown_entry_and_rejection_never_retry_or_fallback(self):
        for where in (oauth.TOKEN_ENDPOINT, oauth.IDENTITY_ENDPOINT):
            for status in (None, 302, 429, 500, 200):
                fixture = RefreshFixture()
                if status is None:
                    fixture.fail[where] = httpx.ReadError(SECRET)
                else:
                    fixture.status[where] = status
                    fixture.raw[where] = document({"ok": False, "error": SECRET})
                    fixture.headers[where] = [("Location", "https://unselected.invalid/"), ("Retry-After", "0")]
                async with client_for(fixture, follow_redirects=True) as client:
                    with self.subTest(where=where, status=status), self.assertRaises(oauth.SlackOAuthError) as caught:
                        await refresh(client)
                    self.assert_safe_error(caught.exception)
                self.assertEqual(len(fixture.calls), 1 if where == oauth.TOKEN_ENDPOINT else 2)
                self.assertEqual(caught.exception.exchange_state, oauth.ExchangeState.UNCONFIRMED
                                 if status is None and where == oauth.TOKEN_ENDPOINT
                                 else oauth.ExchangeState.RESPONSE_RECEIVED)

    async def test_refresh_bounded_response_closes_before_error(self):
        for where, maximum in ((oauth.TOKEN_ENDPOINT, 65536), (oauth.IDENTITY_ENDPOINT, 32768)):
            for raw in (b" " * (maximum + 1), b'{"ok":true,"ok":false}', b'{"ok":true,"expires_in":1.5}'):
                fixture = RefreshFixture()
                fixture.raw[where] = raw
                async with client_for(fixture) as client:
                    with self.assertRaises(oauth.SlackOAuthError):
                        await refresh(client)
                self.assertEqual(fixture.closed[-1], where)
                self.assertEqual(len(fixture.calls), 1 if where == oauth.TOKEN_ENDPOINT else 2)

    async def test_refresh_repeated_cancellation_and_deadline_join_cleanup(self):
        for where, deadline in ((oauth.TOKEN_ENDPOINT, False), (oauth.IDENTITY_ENDPOINT, False),
                                (oauth.TOKEN_ENDPOINT, True)):
            fixture = RefreshFixture()
            fixture.gate_url = where
            async with client_for(fixture) as client:
                task = asyncio.create_task(refresh(client, timeout_seconds=0.03 if deadline else 20))
                try:
                    await asyncio.wait_for(fixture.entered.wait(), 2)
                    if deadline:
                        await asyncio.sleep(0.06)
                    else:
                        for _ in range(3):
                            task.cancel()
                            await asyncio.sleep(0)
                    self.assertFalse(task.done())
                    fixture.release.set()
                    with self.assertRaises(oauth.SlackOAuthError if deadline else oauth.SlackOAuthCancelled) as caught:
                        await asyncio.wait_for(task, 2)
                    self.assert_safe_error(caught.exception)
                    if deadline:
                        self.assertEqual(caught.exception.reason, "deadline")
                    self.assertFalse(client.is_closed)
                finally:
                    fixture.release.set()
                    with suppress(asyncio.CancelledError, Exception):
                        await task
            self.assertEqual(len(fixture.calls), 1 if where == oauth.TOKEN_ENDPOINT else 2)
            self.assertEqual(fixture.closed.count(where), 1)


class SlackUserScopeTests(unittest.IsolatedAsyncioTestCase):
    assert_safe_error = SlackOAuthTests.assert_safe_error

    def test_user_scope_is_an_explicit_authorization_choice(self):
        challenge = base64.urlsafe_b64encode(hashlib.sha256(VERIFIER.encode()).digest()).decode().rstrip("=")
        for scopes in (SCOPES, frozenset({"users:read"}), SCOPES | {"users:read"}):
            with self.subTest(scopes=sorted(scopes)):
                chosen = spec(requested_scopes=scopes)
                request = oauth.authorization_request(chosen, state=SECRET, code_challenge=challenge)
                query = parse_qs(urlsplit(request.url).query, keep_blank_values=True)
                self.assertEqual(query["user_scope"], [",".join(sorted(scopes))])
                self.assertEqual(query["scope"], [""])
                self.assertEqual(chosen.requested_scopes, scopes)
                self.assertEqual(refresh_spec(required_scopes=scopes).required_scopes, scopes)

    def test_user_scope_does_not_admit_email_write_or_unknown_scopes(self):
        for denied in ("users:read.email", "users:write", "chat:write", "unknown:read"):
            for refreshing in (False, True):
                with self.subTest(denied=denied, refreshing=refreshing):
                    scopes = SCOPES | {"users:read", denied}
                    with self.assertRaises(oauth.SlackOAuthError) as caught:
                        if refreshing:
                            refresh_spec(required_scopes=scopes)
                        else:
                            spec(requested_scopes=scopes)
                    self.assertEqual(caught.exception.reason, "invalid_inputs")
                    self.assertEqual(caught.exception.exchange_state, oauth.ExchangeState.NOT_STARTED)
                    self.assert_safe_error(caught.exception)

    async def test_user_scope_exchange_and_refresh_preserve_observed_grants(self):
        required = SCOPES | {"users:read"}
        for refreshing in (False, True):
            for introspection in (False, True):
                with self.subTest(refreshing=refreshing, introspection=introspection):
                    fixture = RefreshFixture() if refreshing else Fixture()
                    token = fixture.token if refreshing else fixture.token["authed_user"]
                    token["scope"] = ",".join(sorted(required | {"groups:read"}))
                    if introspection:
                        fixture.headers[oauth.IDENTITY_ENDPOINT] = [("X-OAuth-Scopes", ",".join(sorted(required)))]
                    async with client_for(fixture) as client:
                        result = (await refresh(client, refresh_spec(required_scopes=required)) if refreshing
                                  else await exchange(client, spec(requested_scopes=required)))
                    self.assertEqual(set(result.observation.exchange_scopes), required | {"groups:read"})
                    self.assertEqual(set(result.observation.granted_scopes), required if introspection
                                     else required | {"groups:read"})
                    self.assertEqual(result.observation.scope_evidence, oauth.ScopeEvidence.PROVIDER_INTROSPECTION
                                     if introspection else oauth.ScopeEvidence.TOKEN_ENDPOINT_RESPONSE)
                    self.assertEqual(result.observation.team_id, "TTEST")
                    self.assertEqual(result.observation.user_id, "WTEST")
                    self.assertEqual(fixture.calls[1].headers["Authorization"], f"Bearer {ACCESS}")
                    if refreshing:
                        self.assertEqual(set(parse_qs(fixture.calls[0].content.decode())),
                                         {"client_id", "grant_type", "refresh_token"})
                    self.assertEqual(len(fixture.calls), 2)
                    self.assertEqual(fixture.closed, [oauth.TOKEN_ENDPOINT, oauth.IDENTITY_ENDPOINT])

    async def test_missing_required_user_scope_refuses_exchange_and_refresh(self):
        required = SCOPES | {"users:read"}
        for refreshing in (False, True):
            for missing_at_token in (False, True):
                with self.subTest(refreshing=refreshing, missing_at_token=missing_at_token):
                    fixture = RefreshFixture() if refreshing else Fixture()
                    token = fixture.token if refreshing else fixture.token["authed_user"]
                    token["scope"] = ",".join(sorted(SCOPES if missing_at_token else required))
                    fixture.headers[oauth.IDENTITY_ENDPOINT] = [
                        ("X-OAuth-Scopes", ",".join(sorted(required if missing_at_token else SCOPES)))]
                    async with client_for(fixture) as client:
                        with self.assertRaises(oauth.SlackOAuthError) as caught:
                            if refreshing:
                                await refresh(client, refresh_spec(required_scopes=required))
                            else:
                                await exchange(client, spec(requested_scopes=required))
                    self.assertEqual(caught.exception.reason, "missing_scope")
                    self.assertEqual(caught.exception.exchange_state, oauth.ExchangeState.RESPONSE_RECEIVED)
                    self.assert_safe_error(caught.exception)
                    self.assertEqual(len(fixture.calls), 1 if missing_at_token else 2)
                    self.assertEqual(fixture.closed, [str(request.url) for request in fixture.calls])


if __name__ == "__main__":
    unittest.main()
