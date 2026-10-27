"""Signed local fixtures for the stateless Google protocol helper."""

import asyncio
import base64
from contextlib import asynccontextmanager, suppress
from datetime import datetime, timedelta, timezone
import hashlib
import json
import time
from types import SimpleNamespace
import unittest
from unittest.mock import patch
from urllib.parse import parse_qs, urlsplit

from cryptography import x509
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import padding, rsa
from cryptography.x509.oid import NameOID
import httpx
import anyio

from meerkat_toolkit.auth import google_oauth as oauth
from meerkat_toolkit.httpx_client import prepare_httpx_client


PRIVATE = "private-fixture-value-never-in-safe-output"
CLIENT_ID = "fixture.apps.googleusercontent.com"
SUBJECT = "100000000000000000001"
NONCE = "fixture-nonce-opaque-value"
ACCESS = "fixture-access-token"
VERIFIER = "V" * 43
REQUIRED = frozenset({"openid", "https://www.googleapis.com/auth/gmail.send"})


def encoded(value):
    return base64.urlsafe_b64encode(value).decode().rstrip("=")


def document(value):
    return json.dumps(value, separators=(",", ":")).encode()


def key_and_certificate():
    key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
    name = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, "Local fixture")])
    now = datetime.now(timezone.utc)
    certificate = (x509.CertificateBuilder().subject_name(name).issuer_name(name)
        .public_key(key.public_key()).serial_number(x509.random_serial_number())
        .not_valid_before(now - timedelta(days=1)).not_valid_after(now + timedelta(days=1))
        .sign(key, hashes.SHA256()))
    return key, certificate.public_bytes(serialization.Encoding.PEM).decode()


KEY, CERTIFICATE = key_and_certificate()
WRONG_KEY, _ = key_and_certificate()


def spec(**changes):
    values = dict(client_id=CLIENT_ID, redirect_uri="http://127.0.0.1:43123/callback",
                  expected_subject=SUBJECT, requested_scopes=REQUIRED)
    values.update(changes)
    return oauth.GoogleReconnectSpec(**values)


def claims(**changes):
    values = dict(iss=oauth.ISSUER, sub=SUBJECT, aud=CLIENT_ID, azp=CLIENT_ID,
                  nonce=NONCE, iat=int(time.time()) - 30, exp=int(time.time()) + 600,
                  at_hash=encoded(hashlib.sha256(ACCESS.encode()).digest()[:16]))
    values.update(changes)
    return values


def signed(payload=None, header=None, key=KEY, raw_header=None, raw_payload=None):
    head = raw_header if raw_header is not None else document(header or {"alg": "RS256", "kid": "fixture-key", "typ": "JWT"})
    body = raw_payload if raw_payload is not None else document(claims() if payload is None else payload)
    prefix = encoded(head) + "." + encoded(body)
    signature = key.sign(prefix.encode(), padding.PKCS1v15(), hashes.SHA256())
    return prefix + "." + encoded(signature)


class Fixture:
    def __init__(self):
        self.token = {"access_token": ACCESS, "id_token": signed(), "refresh_token": "fixture-refresh",
                      "token_type": "Bearer", "expires_in": 3600, "scope": " ".join(sorted(REQUIRED))}
        self.certificates = {"fixture-key": CERTIFICATE}
        self.user = {"sub": SUBJECT, "picture": "https://unselected.invalid/private", "email": PRIVATE}
        self.calls, self.closed = [], []
        self.raw = {}
        self.status = {}
        self.headers = {}
        self.fail = {}
        self.gate_url = None
        self.before = {}
        self.entered = asyncio.Event()
        self.release = asyncio.Event()

    async def handle(self, request):
        self.calls.append(request)
        url = str(request.url)
        if url in self.before:
            self.before[url]()
        if url in self.fail:
            raise self.fail[url]
        values = {oauth.TOKEN_ENDPOINT: self.token, oauth.CERTIFICATES_ENDPOINT: self.certificates,
                  oauth.USERINFO_ENDPOINT: self.user}
        if url not in values:
            raise AssertionError("Unexpected network destination")
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

        return httpx.Response(self.status.get(url, 200), request=request, stream=Stream(),
            headers={"Content-Type": "application/json", **self.headers.get(url, {})})


@asynccontextmanager
async def client_for(fixture, **kwargs):
    async with httpx.AsyncClient(transport=httpx.MockTransport(fixture.handle), trust_env=False,
                                **kwargs) as client:
        prepare_httpx_client(client)
        yield client


async def exchange(client, **kwargs):
    return await oauth.exchange_and_observe(spec(), code="fixture-code", code_verifier=VERIFIER,
        expected_nonce=NONCE, http_client=client, **kwargs)


def refresh_spec(**changes):
    values = dict(client_id=CLIENT_ID, expected_subject=SUBJECT, required_scopes=REQUIRED)
    values.update(changes)
    return oauth.GoogleRefreshSpec(**values)


class RefreshFixture(Fixture):
    def __init__(self):
        super().__init__()
        self.token.pop("id_token")
        self.token.pop("refresh_token")


async def refresh(client, chosen=None, **kwargs):
    return await oauth.refresh_and_observe(chosen or refresh_spec(), refresh_token=PRIVATE,
                                          http_client=client, **kwargs)


class GoogleOAuthTests(unittest.IsolatedAsyncioTestCase):
    def assert_safe_error(self, error):
        self.assertNotIn(PRIVATE, str(error))
        self.assertNotIn(ACCESS, repr(error))
        self.assertIsNone(error.__cause__)
        self.assertIsNone(error.__context__)

    def test_authorization_request_has_only_explicit_desktop_parameters(self):
        challenge = encoded(hashlib.sha256(VERIFIER.encode()).digest())
        request = oauth.authorization_request(spec(), state=PRIVATE, nonce=NONCE,
            code_challenge=challenge, offline=True, prompt_consent=True)
        url = urlsplit(request.url)
        self.assertEqual(url.scheme + "://" + url.netloc + url.path, oauth.AUTHORIZATION_ENDPOINT)
        self.assertEqual(parse_qs(url.query), {"client_id": [CLIENT_ID],
            "redirect_uri": [spec().redirect_uri], "response_type": ["code"],
            "scope": [" ".join(sorted(REQUIRED))], "state": [PRIVATE], "nonce": [NONCE],
            "code_challenge": [challenge], "code_challenge_method": ["S256"],
            "login_hint": [SUBJECT], "access_type": ["offline"], "prompt": ["consent"]})
        self.assertNotIn(PRIVATE, repr(request))
        for invalid in ("https://host.invalid/callback", "http://localhost:1234/", "http://127.0.0.1/",
                        "http://u:p@127.0.0.1:4321/", "http://127.0.0.1:4321/?x=y"):
            with self.subTest(redirect=invalid), self.assertRaises(oauth.GoogleOAuthError) as caught:
                spec(redirect_uri=invalid)
            self.assert_safe_error(caught.exception)
        for scopes in (frozenset(), frozenset({"openid"}), frozenset({"openid", "email"}),
                       frozenset({"openid", "https://www.googleapis.com/auth/drive"})):
            with self.assertRaises(oauth.GoogleOAuthError):
                spec(requested_scopes=scopes)

    async def test_signed_exchange_exact_requests_scopes_and_private_result(self):
        fixture = Fixture()
        fixture.token["scope"] += " https://www.googleapis.com/auth/gmail.readonly"
        async with client_for(fixture) as client:
            result = await exchange(client, client_secret=PRIVATE)
            self.assertFalse(client.is_closed)
        self.assertEqual([str(r.url) for r in fixture.calls],
                         [oauth.TOKEN_ENDPOINT, oauth.CERTIFICATES_ENDPOINT, oauth.USERINFO_ENDPOINT])
        self.assertEqual([r.method for r in fixture.calls], ["POST", "GET", "GET"])
        self.assertEqual(parse_qs(fixture.calls[0].content.decode()), {"code": ["fixture-code"],
            "client_id": [CLIENT_ID], "redirect_uri": [spec().redirect_uri],
            "code_verifier": [VERIFIER], "grant_type": ["authorization_code"], "client_secret": [PRIVATE]})
        self.assertNotIn("authorization", fixture.calls[0].headers)
        self.assertNotIn("authorization", fixture.calls[1].headers)
        self.assertEqual(fixture.calls[2].headers["authorization"], "Bearer " + ACCESS)
        self.assertEqual(result.access_token, ACCESS)
        self.assertEqual(result.refresh_token, "fixture-refresh")
        self.assertLessEqual(result.expires_at, result.token_received_at + result.expires_in)
        self.assertLess(result.observation.verified_at, result.expires_at)
        safe = result.safe_view()
        self.assertEqual(safe["subject"], SUBJECT)
        self.assertEqual(safe["scope_evidence"], "google_token_endpoint")
        self.assertEqual(set(safe["reported_scopes"]), REQUIRED | {"https://www.googleapis.com/auth/gmail.readonly"})
        for hidden in (PRIVATE, ACCESS, "fixture-refresh", fixture.token["id_token"]):
            self.assertNotIn(hidden, repr(result))
            self.assertNotIn(hidden, json.dumps(safe))
        self.assertEqual(len(fixture.closed), 3)

    async def test_missing_refresh_and_documented_issuer_alias(self):
        fixture = Fixture()
        fixture.token.pop("refresh_token")
        fixture.token["id_token"] = signed(claims(iss="accounts.google.com"))
        async with client_for(fixture) as client:
            result = await exchange(client)
        self.assertIsNone(result.refresh_token)
        self.assertEqual(result.observation.issuer, oauth.ISSUER)

    async def test_wrong_signed_claims_never_accept_or_call_userinfo(self):
        cases = [{"iss": "https://unselected.invalid"}, {"aud": "other-client"},
            {"aud": [CLIENT_ID]}, {"azp": "other-client"}, {"nonce": PRIVATE},
            {"sub": "100000000000000000002"}, {"exp": int(time.time()) - 1},
            {"iat": int(time.time()) + 300}, {"iat": True}, {"exp": True},
            {"nbf": int(time.time()) + 300}, {"auth_time": False}, {"at_hash": "incorrect"}]
        cases.append({"at_hash": None})
        for changed in cases:
            with self.subTest(changed=changed):
                fixture = Fixture()
                fixture.token["id_token"] = signed(claims(**changed))
                async with client_for(fixture) as client:
                    with self.assertRaises(oauth.GoogleOAuthError) as caught:
                        await exchange(client)
                self.assert_safe_error(caught.exception)
                self.assertEqual(caught.exception.exchange_state, oauth.ExchangeState.RESPONSE_RECEIVED)
                self.assertEqual(sum(r.method == "POST" for r in fixture.calls), 1)
                self.assertNotIn(oauth.USERINFO_ENDPOINT, [str(r.url) for r in fixture.calls])

    async def test_wrong_signature_jose_key_sources_and_duplicates_refuse(self):
        tokens = [signed(key=WRONG_KEY), signed(header={"alg": "none", "kid": "fixture-key"}),
            signed(header={"alg": "RS256", "kid": "other-key"}),
            signed(header={"alg": "RS256", "kid": "fixture-key", "jku": "https://unselected.invalid/"}),
            signed(header={"alg": "RS256", "kid": "fixture-key", "crit": []}),
            signed(raw_header=b'{"alg":"RS256","alg":"RS256","kid":"fixture-key"}'),
            signed(raw_payload=document(claims())[:-1] + b',"sub":"' + SUBJECT.encode() + b'"}')]
        for token in tokens:
            fixture = Fixture()
            fixture.token["id_token"] = token
            async with client_for(fixture) as client:
                with self.assertRaises(oauth.GoogleOAuthError) as caught:
                    await exchange(client)
            self.assert_safe_error(caught.exception)
            self.assertEqual(sum(r.method == "POST" for r in fixture.calls), 1)
            self.assertNotIn(oauth.USERINFO_ENDPOINT, [str(r.url) for r in fixture.calls])

    async def test_same_user_access_substitution_and_userinfo_subject_mismatch_refuse(self):
        for substituted in (True, False):
            fixture = Fixture()
            if substituted:
                fixture.token["access_token"] = "other-token-for-same-user"
            else:
                fixture.user["sub"] = "100000000000000000002"
            async with client_for(fixture) as client:
                with self.assertRaises(oauth.GoogleOAuthError) as caught:
                    await exchange(client)
            self.assertEqual(caught.exception.reason, "identity_mismatch")
            self.assertEqual(sum(r.method == "POST" for r in fixture.calls), 1)

    async def test_strict_token_fields_and_required_scope(self):
        cases = [{"scope": "openid"}, {"scope": None}, {"scope": "openid  extra"},
            {"scope": "openid openid"}, {"access_token": ""}, {"refresh_token": ""},
            {"expires_in": True}, {"expires_in": 0}, {"expires_in": 1.5},
            {"token_type": "other"}, {"access_token": PRIVATE + "\r\n"}]
        for changed in cases:
            fixture = Fixture()
            fixture.token.update(changed)
            async with client_for(fixture) as client:
                with self.assertRaises(oauth.GoogleOAuthError) as caught:
                    await exchange(client)
            self.assert_safe_error(caught.exception)
            self.assertEqual(len(fixture.calls), 1)

    async def test_duplicate_json_bounds_and_no_verification_fallback(self):
        cases = [(oauth.TOKEN_ENDPOINT, b'{"scope":"a","scope":"b"}'),
            (oauth.TOKEN_ENDPOINT, b"x" * 65537),
            (oauth.CERTIFICATES_ENDPOINT, b"x" * 262145),
            (oauth.CERTIFICATES_ENDPOINT, document({"keys": []})),
            (oauth.USERINFO_ENDPOINT, b'{"sub":"x","sub":"x"}'),
            (oauth.USERINFO_ENDPOINT, b"x" * 32769)]
        for url, raw in cases:
            fixture = Fixture()
            fixture.raw[url] = raw
            async with client_for(fixture) as client:
                with self.assertRaises(oauth.GoogleOAuthError) as caught:
                    await exchange(client)
            self.assert_safe_error(caught.exception)
            self.assertEqual(sum(r.method == "POST" for r in fixture.calls), 1)
            self.assertTrue(all(str(r.url) in {oauth.TOKEN_ENDPOINT, oauth.CERTIFICATES_ENDPOINT, oauth.USERINFO_ENDPOINT}
                                for r in fixture.calls))

    async def test_defaults_auth_cookies_and_query_never_leak(self):
        class ForbiddenAuth(httpx.Auth):
            def auth_flow(self, request):
                raise AssertionError(PRIVATE)
                yield request

        fixture = Fixture()
        fixture.headers[oauth.TOKEN_ENDPOINT] = {"Set-Cookie": "leak=" + PRIVATE + "; Domain=.googleapis.com"}
        async with client_for(fixture, auth=ForbiddenAuth(), headers={"Authorization": PRIVATE,
                "X-Private": PRIVATE}, cookies={"ambient": PRIVATE}, params={"secret": PRIVATE},
                follow_redirects=True) as client:
            result = await exchange(client)
            self.assertEqual(result.access_token, ACCESS)
        for request in fixture.calls:
            self.assertNotIn(PRIVATE, str(request.url))
            self.assertNotIn(PRIVATE, str(request.headers))
            self.assertNotIn("cookie", request.headers)

    async def test_redirects_and_transport_errors_preserve_exchange_state_without_retry(self):
        for url in (oauth.TOKEN_ENDPOINT, oauth.CERTIFICATES_ENDPOINT, oauth.USERINFO_ENDPOINT):
            fixture = Fixture()
            fixture.status[url] = 302
            fixture.headers[url] = {"Location": "https://unselected.invalid/"}
            async with client_for(fixture, follow_redirects=True) as client:
                with self.assertRaises(oauth.GoogleOAuthError) as caught:
                    await exchange(client)
            self.assertEqual(caught.exception.exchange_state, oauth.ExchangeState.RESPONSE_RECEIVED)
            self.assertEqual(sum(r.method == "POST" for r in fixture.calls), 1)
        fixture = Fixture()
        fixture.fail[oauth.TOKEN_ENDPOINT] = httpx.ReadError(PRIVATE)
        async with client_for(fixture) as client:
            with self.assertRaises(oauth.GoogleOAuthError) as caught:
                await exchange(client)
        self.assertEqual(caught.exception.exchange_state, oauth.ExchangeState.UNCONFIRMED)
        self.assert_safe_error(caught.exception)
        self.assertEqual(len(fixture.calls), 1)

    async def test_cancellation_retains_cleanup_and_response_state(self):
        for url in (oauth.TOKEN_ENDPOINT, oauth.USERINFO_ENDPOINT):
            fixture = Fixture()
            fixture.gate_url = url
            async with client_for(fixture) as client:
                task = asyncio.create_task(exchange(client))
                try:
                    await asyncio.wait_for(fixture.entered.wait(), 3)
                    task.cancel(PRIVATE)
                    await asyncio.sleep(.02)
                    task.cancel(PRIVATE)
                    await asyncio.sleep(.02)
                    self.assertFalse(task.done())
                    self.assertNotIn(url, fixture.closed)
                finally:
                    fixture.release.set()
                with self.assertRaises(oauth.GoogleOAuthCancelled) as caught:
                    await asyncio.wait_for(task, 3)
                self.assertEqual(caught.exception.exchange_state, oauth.ExchangeState.RESPONSE_RECEIVED)
                self.assert_safe_error(caught.exception)
                self.assertFalse(client.is_closed)
            self.assertEqual(sum(r.method == "POST" for r in fixture.calls), 1)
            self.assertIn(url, fixture.closed)

    async def test_deadline_joins_cleanup_before_refusal(self):
        fixture = Fixture()
        fixture.gate_url = oauth.TOKEN_ENDPOINT
        async with client_for(fixture) as client:
            task = asyncio.create_task(exchange(client, timeout_seconds=.03))
            try:
                await asyncio.wait_for(fixture.entered.wait(), 3)
                await asyncio.sleep(.06)
                self.assertFalse(task.done())
            finally:
                fixture.release.set()
            with self.assertRaises(oauth.GoogleOAuthError) as caught:
                await asyncio.wait_for(task, 3)
            self.assertEqual(caught.exception.reason, "deadline")
            self.assertEqual(caught.exception.exchange_state, oauth.ExchangeState.RESPONSE_RECEIVED)
        self.assertEqual(len(fixture.calls), 1)

    async def test_outer_anyio_cancellation_shields_join_without_an_extra_request(self):
        fixture = Fixture()
        fixture.gate_url = oauth.TOKEN_ENDPOINT
        scopes, caught = [], []
        async with client_for(fixture) as client:
            async def operation():
                with anyio.CancelScope() as scope:
                    scopes.append(scope)
                    try:
                        await exchange(client)
                    except oauth.GoogleOAuthCancelled as error:
                        caught.append(error)
                        raise
            task = asyncio.create_task(operation())
            try:
                await asyncio.wait_for(fixture.entered.wait(), 3)
                scopes[0].cancel()
                await asyncio.sleep(.03)
                self.assertFalse(task.done())
                self.assertEqual(fixture.closed, [])
            finally:
                fixture.release.set()
            with self.assertRaises(oauth.GoogleOAuthCancelled):
                await asyncio.wait_for(task, 3)
        self.assertEqual(len(caught), 1)
        self.assert_safe_error(caught[0])
        self.assertEqual(caught[0].exchange_state, oauth.ExchangeState.RESPONSE_RECEIVED)
        self.assertEqual(len(fixture.calls), 1)

    async def test_access_and_identity_must_still_be_unexpired_at_return(self):
        for which in ("access", "identity"):
            fixture = Fixture()
            now = int(time.time())
            clock = [now]
            if which == "access":
                fixture.token["expires_in"] = 10
            else:
                fixture.token["id_token"] = signed(claims(exp=now + 10))
            fixture.before[oauth.USERINFO_ENDPOINT] = lambda: clock.__setitem__(0, now + 11)
            async with client_for(fixture) as client, _clock(clock):
                with self.assertRaises(oauth.GoogleOAuthError) as caught:
                    await exchange(client)
            self.assertEqual(caught.exception.reason, "expired")
            self.assertEqual(caught.exception.exchange_state, oauth.ExchangeState.RESPONSE_RECEIVED)
            self.assertEqual(sum(r.method == "POST" for r in fixture.calls), 1)


class GoogleRefreshTests(unittest.IsolatedAsyncioTestCase):
    assert_safe_error = GoogleOAuthTests.assert_safe_error

    async def test_exact_refresh_wire_optional_secret_and_new_token_userinfo(self):
        for secret in (None, PRIVATE):
            fixture = RefreshFixture()

            class NeverAuth(httpx.Auth):
                def auth_flow(self, request):
                    raise AssertionError(PRIVATE)
                    yield request

            async with client_for(fixture, auth=NeverAuth(), headers={"X-Private": PRIVATE},
                                  params={"private": PRIVATE}, cookies={"ambient": PRIVATE}) as client:
                result = await refresh(client, client_secret=secret)
                self.assertFalse(client.is_closed)
            self.assertEqual([str(r.url) for r in fixture.calls], [oauth.TOKEN_ENDPOINT, oauth.USERINFO_ENDPOINT])
            self.assertEqual([r.method for r in fixture.calls], ["POST", "GET"])
            expected = {"client_id": [CLIENT_ID], "refresh_token": [PRIVATE], "grant_type": ["refresh_token"]}
            if secret:
                expected["client_secret"] = [secret]
            self.assertEqual(parse_qs(fixture.calls[0].content.decode()), expected)
            self.assertNotIn("authorization", fixture.calls[0].headers)
            self.assertEqual(fixture.calls[1].headers["authorization"], "Bearer " + ACCESS)
            self.assertEqual(fixture.calls[1].content, b"")
            for request in fixture.calls:
                self.assertNotIn("cookie", request.headers)
                self.assertNotIn("x-private", request.headers)
            self.assertIsNone(result.refresh_token)
            self.assertIsNone(result.refresh_token_expires_in)
            self.assertIsNone(result.refresh_token_expires_at)
            self.assertEqual(result.observation.configured_client_id, CLIENT_ID)
            self.assertEqual(result.observation.subject, SUBJECT)
            self.assertEqual(result.observation.identity_evidence, "google_userinfo")
            self.assertEqual(result.observation.provider, "google")
            self.assertFalse(hasattr(result, "id_token"))
            self.assertFalse(hasattr(result.observation, "issuer"))
            self.assertFalse(hasattr(result.observation, "aud"))
            view = result.safe_view()
            for value in (PRIVATE, ACCESS):
                self.assertNotIn(value, repr(result))
                self.assertNotIn(value, json.dumps(view))
            view["reported_scopes"].clear()
            self.assertEqual(set(result.observation.reported_scopes), REQUIRED)
            self.assertEqual(fixture.closed, [oauth.TOKEN_ENDPOINT, oauth.USERINFO_ENDPOINT])

    async def test_scope_omission_is_unknown_and_no_id_token_claim_is_consumed(self):
        for optional_id in (None, "not-a-jwt", signed(claims(sub="OTHER", nonce="wrong"))):
            fixture = RefreshFixture()
            fixture.token.pop("scope")
            if optional_id is not None:
                fixture.token["id_token"] = optional_id
            fixture.user["scope"] = " ".join(REQUIRED)
            async with client_for(fixture) as client:
                with patch.object(oauth, "_claims", side_effect=AssertionError("ID token must not be consumed")):
                    result = await refresh(client)
            self.assertIsNone(result.observation.reported_scopes)
            self.assertIsNone(result.observation.scope_evidence)
            self.assertIsNone(result.safe_view()["reported_scopes"])
            self.assertIsNone(result.safe_view()["scope_evidence"])
            self.assertEqual(len(fixture.calls), 2)
            self.assertNotIn(oauth.CERTIFICATES_ENDPOINT, [str(r.url) for r in fixture.calls])
            self.assertNotIn("id_token", result.__dict__)
            for invented in ("ready", "authorized", "granted_scopes", "nonce", "at_hash", "issuer", "aud"):
                self.assertNotIn(invented, result.safe_view())

    async def test_explicit_scope_preserves_surplus_without_userinfo_or_prior_union(self):
        fixture = RefreshFixture()
        extra = {"https://www.googleapis.com/auth/userinfo.email", "https://www.googleapis.com/auth/gmail.readonly"}
        fixture.token["scope"] = " ".join(REQUIRED | extra)
        fixture.user["scope"] = "https://mail.google.com/"
        async with client_for(fixture) as client:
            result = await refresh(client)
        self.assertEqual(set(result.observation.reported_scopes), REQUIRED | extra)
        self.assertEqual(result.observation.scope_evidence, "google_token_endpoint")
        self.assertNotIn(fixture.user["email"], json.dumps(result.safe_view()))

    async def test_present_bad_or_insufficient_scope_never_becomes_unknown(self):
        for value in (None, "", [], "openid", "https://www.googleapis.com/auth/gmail.send",
                      "openid openid https://www.googleapis.com/auth/gmail.send",
                      "openid  https://www.googleapis.com/auth/gmail.send",
                      "openid\thttps://www.googleapis.com/auth/gmail.send", "x" * 16385):
            fixture = RefreshFixture()
            fixture.token["scope"] = value
            async with client_for(fixture) as client:
                with self.subTest(scope=value), self.assertRaises(oauth.GoogleOAuthError) as caught:
                    await refresh(client)
                self.assert_safe_error(caught.exception)
            self.assertEqual(caught.exception.phase, "token")
            self.assertEqual(len(fixture.calls), 1)

    async def test_userinfo_exact_subject_cannot_be_replaced_by_token_or_email(self):
        for value in (None, "", 1, "OTHER", SUBJECT + " ", "bad\nsubject"):
            fixture = RefreshFixture()
            fixture.token.update(sub=SUBJECT, id_token=signed())
            fixture.user["sub"] = value
            async with client_for(fixture) as client:
                with self.subTest(subject=value), self.assertRaises(oauth.GoogleOAuthError) as caught:
                    await refresh(client)
                self.assert_safe_error(caught.exception)
            self.assertEqual(caught.exception.phase, "userinfo")
            self.assertEqual(len(fixture.calls), 2)

    async def test_replacement_and_time_based_refresh_lifetime_are_only_reported(self):
        for replacement in (None, PRIVATE, "new-refresh-value"):
            fixture = RefreshFixture()
            fixture.token["refresh_token_expires_in"] = 7
            if replacement is not None:
                fixture.token["refresh_token"] = replacement
            clock = [1000.0]
            fake_time = SimpleNamespace(time=lambda: clock[0], monotonic=lambda: clock[0])
            fixture.before[oauth.USERINFO_ENDPOINT] = lambda: clock.__setitem__(0, 1010.0)
            async with client_for(fixture) as client:
                with patch.object(oauth, "time", fake_time):
                    result = await refresh(client)
            self.assertEqual(result.refresh_token, replacement)
            self.assertEqual(result.refresh_token_expires_in, 7)
            self.assertEqual(result.refresh_token_expires_at, 1007)
            self.assertEqual(result.expires_at, 4600)
            # A still-valid access token may outlive the reported refresh-token
            # expiry. No fresh refresh lifetime starts at result handoff.
            self.assertLess(result.refresh_token_expires_at, result.observation.verified_at)

    async def test_invalid_access_refresh_and_lifetime_fields_refuse(self):
        cases = [("expires_in", value) for value in (None, True, 0, -1, 2**31)]
        cases += [("refresh_token_expires_in", value) for value in (None, True, 0, -1, 2**31)]
        cases += [("refresh_token", value) for value in (None, "", "x" * 8193)]
        cases += [("access_token", ""), ("access_token", PRIVATE + "\n"), ("token_type", "DPoP")]
        for field, value in cases:
            fixture = RefreshFixture()
            fixture.token[field] = value
            async with client_for(fixture) as client:
                with self.subTest(field=field, value_type=type(value).__name__), self.assertRaises(oauth.GoogleOAuthError) as caught:
                    await refresh(client)
                self.assert_safe_error(caught.exception)
            self.assertEqual(len(fixture.calls), 1)

    async def test_access_lifetime_checked_before_and_after_userinfo_with_both_clocks(self):
        for after in (False, True):
            for advancing in ("wall", "monotonic"):
                fixture = RefreshFixture()
                fixture.token["expires_in"] = 10
                clocks = {"wall": 1000.0, "monotonic": 20.0}
                fake_time = SimpleNamespace(time=lambda: clocks["wall"], monotonic=lambda: clocks["monotonic"])
                def advance():
                    clocks[advancing] += 11
                fixture.before[oauth.USERINFO_ENDPOINT if after else oauth.TOKEN_ENDPOINT] = advance
                async with client_for(fixture) as client:
                    with patch.object(oauth, "time", fake_time):
                        with self.assertRaises(oauth.GoogleOAuthError) as caught:
                            await refresh(client)
                self.assertEqual(caught.exception.reason, "expired")
                self.assertEqual(len(fixture.calls), 2 if after else 1)

    async def test_invalid_and_forged_inputs_do_not_enter_provider(self):
        fixture = RefreshFixture()
        for fields in ({"required_scopes": frozenset()}, {"required_scopes": set(REQUIRED)},
                       {"required_scopes": frozenset({"openid"})},
                       {"required_scopes": REQUIRED | {"email"}}, {"expected_subject": ""},
                       {"client_id": PRIVATE + "\n"}):
            with self.assertRaises(oauth.GoogleOAuthError) as caught:
                refresh_spec(**fields)
            self.assert_safe_error(caught.exception)
        async with httpx.AsyncClient(transport=httpx.MockTransport(fixture.handle)) as client:
            with self.assertRaises(oauth.GoogleOAuthError):
                await refresh(client)
            prepare_httpx_client(client)
            forged = refresh_spec()
            object.__setattr__(forged, "required_scopes", frozenset({"openid"}))
            for chosen in (forged, spec()):
                with self.assertRaises(oauth.GoogleOAuthError):
                    await refresh(client, chosen)
            for secret in (None, "", PRIVATE + "\n", "x" * 8193):
                with self.assertRaises(oauth.GoogleOAuthError):
                    await oauth.refresh_and_observe(refresh_spec(), refresh_token=secret, http_client=client)
            for timeout in (True, 0, -1, 121, float("nan"), float("inf")):
                with self.assertRaises(oauth.GoogleOAuthError):
                    await refresh(client, timeout_seconds=timeout)
        self.assertEqual(fixture.calls, [])

    async def test_error_or_transport_failure_never_replays_or_retains_raw_error(self):
        for where in (oauth.TOKEN_ENDPOINT, oauth.USERINFO_ENDPOINT):
            for status in (None, 302, 400, 429, 500):
                fixture = RefreshFixture()
                if status is None:
                    fixture.fail[where] = httpx.ReadError(PRIVATE)
                else:
                    fixture.status[where] = status
                    fixture.headers[where] = {"Location": "https://unselected.invalid/", "Retry-After": "0"}
                    fixture.raw[where] = document({"error": PRIVATE})
                async with client_for(fixture, follow_redirects=True) as client:
                    with self.assertRaises(oauth.GoogleOAuthError) as caught:
                        await refresh(client)
                    self.assert_safe_error(caught.exception)
                self.assertEqual(caught.exception.exchange_state, oauth.ExchangeState.UNCONFIRMED
                                 if where == oauth.TOKEN_ENDPOINT and status is None else oauth.ExchangeState.RESPONSE_RECEIVED)
                self.assertEqual(len(fixture.calls), 1 if where == oauth.TOKEN_ENDPOINT else 2)

    async def test_error_payload_with_other_valid_fields_is_not_success(self):
        for where in ("token", "user"):
            fixture = RefreshFixture()
            getattr(fixture, where)["error"] = PRIVATE
            async with client_for(fixture) as client:
                with self.assertRaises(oauth.GoogleOAuthError) as caught:
                    await refresh(client)
                self.assert_safe_error(caught.exception)
            self.assertEqual(caught.exception.reason, "provider_rejected")
            self.assertEqual(len(fixture.calls), 1 if where == "token" else 2)

    async def test_malformed_bounded_responses_join_close(self):
        for where, maximum in ((oauth.TOKEN_ENDPOINT, 65536), (oauth.USERINFO_ENDPOINT, 32768)):
            for raw in (b" " * (maximum + 1), b'{"sub":"x","sub":"x"}', b'{"expires_in":1.2}',
                        b'{"sub":"\xff"}', b'{"x":' + b'[' * 14 + b'0' + b']' * 14 + b'}'):
                fixture = RefreshFixture()
                fixture.raw[where] = raw
                async with client_for(fixture) as client:
                    with self.assertRaises(oauth.GoogleOAuthError):
                        await refresh(client)
                self.assertEqual(fixture.closed[-1], where)

    async def test_refresh_cancellation_and_deadline_join_without_next_send(self):
        for where, deadline in ((oauth.TOKEN_ENDPOINT, False), (oauth.USERINFO_ENDPOINT, False),
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
                    with self.assertRaises(oauth.GoogleOAuthError if deadline else oauth.GoogleOAuthCancelled) as caught:
                        await asyncio.wait_for(task, 2)
                    self.assert_safe_error(caught.exception)
                    self.assertFalse(client.is_closed)
                    if deadline:
                        self.assertEqual(caught.exception.reason, "deadline")
                finally:
                    fixture.release.set()
                    with suppress(asyncio.CancelledError, Exception):
                        await task
            self.assertEqual(len(fixture.calls), 1 if where == oauth.TOKEN_ENDPOINT else 2)
            self.assertEqual(fixture.closed.count(where), 1)


@asynccontextmanager
async def _clock(clock):
    with patch.object(oauth.time, "time", side_effect=lambda: clock[0]):
        yield


if __name__ == "__main__":
    unittest.main()
