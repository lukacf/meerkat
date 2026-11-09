"""Token-refusal diagnostics retain existing outcome and response custody."""

import asyncio
import json
import unittest

import httpx

from meerkat_toolkit.auth import google_oauth as google, slack_oauth as slack
from meerkat_toolkit.httpx_client import prepare_httpx_client


PRIVATE = "synthetic-private-description-never-exported"
CODES = {slack: ("bad_redirect_uri", "invalid_code", "invalid_refresh_token", "pkce_not_allowed"),
         google: ("invalid_grant", "invalid_client")}


async def invoke(module, client, *, refresh=False, timeout=2):
    if module is slack:
        common = dict(client_id="123.456", app_id="AAPP", expected_team_id="TTEAM",
                      expected_user_id="UUSER")
        scopes = frozenset({"channels:history", "users:read"})
        if refresh:
            spec = module.SlackRefreshSpec(**common, required_scopes=scopes)
        else:
            spec = module.SlackReconnectSpec(**common, requested_scopes=scopes,
                                             redirect_uri="http://localhost:41234/callback")
    else:
        common = dict(client_id="synthetic.apps.googleusercontent.com", expected_subject="123456789")
        scopes = frozenset({"openid", "https://www.googleapis.com/auth/gmail.send"})
        if refresh:
            spec = module.GoogleRefreshSpec(**common, required_scopes=scopes)
        else:
            spec = module.GoogleReconnectSpec(**common, requested_scopes=scopes,
                                              redirect_uri="http://127.0.0.1:41234/callback")
    if refresh:
        return await module.refresh_and_observe(spec, refresh_token="synthetic-refresh",
                                                http_client=client, timeout_seconds=timeout)
    args = {} if module is slack else {"expected_nonce": "synthetic-nonce"}
    return await module.exchange_and_observe(spec, code="synthetic-code", code_verifier="V" * 43,
                                             http_client=client, timeout_seconds=timeout, **args)


class Fixture:
    def __init__(self, module, payload=None, *, status=None, raw=None, headers=()):
        self.module = module
        self.status = (200 if module is slack else 400) if status is None else status
        default = {"error": CODES[module][0], "error_description": PRIVATE}
        if module is slack:
            default["ok"] = False
        self.raw = json.dumps(default if payload is None else payload).encode() if raw is None else raw
        self.headers = [("content-type", "application/json"), *headers]
        self.requests, self.closed, self.yields = [], 0, 0
        self.body_gate = self.close_gate = self.fail_body = False
        self.entered, self.closing, self.release = asyncio.Event(), asyncio.Event(), asyncio.Event()

    async def handle(self, request):
        assert str(request.url) == self.module.TOKEN_ENDPOINT
        self.requests.append(request)
        owner = self

        class Stream(httpx.AsyncByteStream):
            async def __aiter__(self):
                owner.entered.set()
                if owner.body_gate:
                    await asyncio.Event().wait()
                if owner.fail_body:
                    raise httpx.ReadError(PRIVATE)
                for position in range(0, len(owner.raw), 4096):
                    owner.yields += 1
                    yield owner.raw[position:position + 4096]

            async def aclose(self):
                owner.closing.set()
                if owner.close_gate:
                    await owner.release.wait()
                owner.closed += 1

        return httpx.Response(self.status, headers=self.headers, stream=Stream(), request=request)


class OAuthRefusalDiagnostics(unittest.IsolatedAsyncioTestCase):
    async def refusal(self, module, fixture, **kwargs):
        async with httpx.AsyncClient(transport=httpx.MockTransport(fixture.handle), trust_env=False) as client:
            prepare_httpx_client(client)
            error_type = module.SlackOAuthError if module is slack else module.GoogleOAuthError
            with self.assertRaises(error_type) as caught:
                await invoke(module, client, **kwargs)
        error = caught.exception
        self.assertEqual(len(fixture.requests), 1)
        self.assertEqual(fixture.closed, 1)
        self.assertEqual(error.phase, "token")
        self.assertEqual(error.exchange_state, module.ExchangeState.RESPONSE_RECEIVED)
        self.assertNotIn(PRIVATE, str(error))
        self.assertNotIn(PRIVATE, repr(vars(error)))
        self.assertIsNone(error.__context__)
        return error

    async def test_documented_codes_for_exchange_and_refresh(self):
        for module, codes in CODES.items():
            for code in codes:
                for refresh in (False, True):
                    for status in ((200,) if module is slack else (400, 401)):
                        with self.subTest(provider=module.__name__, code=code, refresh=refresh, status=status):
                            payload = {"error": code, "error_description": PRIVATE}
                            if module is slack:
                                payload["ok"] = False
                            error = await self.refusal(module, Fixture(module, payload, status=status), refresh=refresh)
                            self.assertEqual(error.reason, "provider_rejected")
                            self.assertEqual(getattr(error, "provider_code", None), code)
                            self.assertNotIn(code, str(error))

    async def test_unknown_mixed_and_malformed_envelopes_have_no_code(self):
        for module in CODES:
            base = {"error": CODES[module][0], **({"ok": False} if module is slack else {})}
            variants = [
                {**base, "error": "unknown_" + PRIVATE}, {**base, "error": None},
                {**base, "error": [CODES[module][0]]}, {**base, "access_token": PRIVATE},
                {**base, "authed_user": {}}, {**base, "extra": "unknown"},
                {**base, "error_description": {}}, {**base, "error_uri": []},
            ]
            for payload in variants:
                with self.subTest(provider=module.__name__, payload_type=list(payload)):
                    error = await self.refusal(module, Fixture(module, payload), refresh=True)
                    self.assertEqual(error.reason, "provider_rejected")
                    self.assertIsNone(getattr(error, "provider_code", None))
            for raw in (b'{"error":', b'{"error":"invalid_grant","error":"invalid_client"}',
                        b'\xff', b'[]', b'{"error":NaN}'):
                with self.subTest(provider=module.__name__, raw=raw):
                    error = await self.refusal(module, Fixture(module, raw=raw), refresh=True)
                    self.assertIsNone(getattr(error, "provider_code", None))

    async def test_non_token_statuses_are_not_promoted(self):
        for module in CODES:
            for status in ((400, 401, 403, 500) if module is slack else (403, 429, 500)):
                with self.subTest(provider=module.__name__, status=status):
                    fixture = Fixture(module, status=status)
                    error = await self.refusal(module, fixture, refresh=True)
                    self.assertEqual(error.reason, "provider_rejected")
                    self.assertIsNone(getattr(error, "provider_code", None))
                    self.assertEqual(fixture.yields, 0)

    async def test_exact_response_cap_and_oversize(self):
        for module in CODES:
            value = {"error": CODES[module][0], "error_description": ""}
            if module is slack:
                value["ok"] = False
            overhead = len(json.dumps(value).encode())
            for extra in (0, 1):
                with self.subTest(provider=module.__name__, extra=extra):
                    value["error_description"] = "x" * (65536 - overhead + extra)
                    raw = json.dumps(value).encode()
                    self.assertEqual(len(raw), 65536 + extra)
                    fixture = Fixture(module, raw=raw)
                    error = await self.refusal(module, fixture, refresh=True)
                    self.assertEqual(getattr(error, "provider_code", None), None if extra else CODES[module][0])

    async def test_identity_endpoint_never_reports_token_refusal_code(self):
        for module in CODES:
            with self.subTest(provider=module.__name__):
                calls = []
                if module is slack:
                    success = {"ok": True, "app_id": "AAPP", "team": {"id": "TTEAM"},
                        "access_token": "synthetic-access", "refresh_token": "synthetic-next-refresh",
                        "token_type": "user", "expires_in": 3600, "scope": "channels:history,users:read"}
                else:
                    success = {"access_token": "synthetic-access", "token_type": "Bearer", "expires_in": 3600,
                        "scope": "openid https://www.googleapis.com/auth/gmail.send"}

                async def handle(request):
                    calls.append(str(request.url))
                    if str(request.url) == module.TOKEN_ENDPOINT:
                        return httpx.Response(200, json=success)
                    refusal = {"error": CODES[module][0], **({"ok": False} if module is slack else {})}
                    return httpx.Response(200 if module is slack else 400, json=refusal)

                async with httpx.AsyncClient(transport=httpx.MockTransport(handle), trust_env=False) as client:
                    prepare_httpx_client(client)
                    error_type = module.SlackOAuthError if module is slack else module.GoogleOAuthError
                    with self.assertRaises(error_type) as caught:
                        await invoke(module, client, refresh=True)
                self.assertEqual(caught.exception.reason, "provider_rejected")
                self.assertIsNone(getattr(caught.exception, "provider_code", None))
                self.assertEqual(len(calls), 2)

    def test_error_constructor_retains_only_finite_token_refusal_metadata(self):
        for module in CODES:
            error_type = module.SlackOAuthError if module is slack else module.GoogleOAuthError
            for code in (PRIVATE, None, [], {"code": PRIVATE}, True):
                with self.subTest(provider=module.__name__, code_type=type(code).__name__):
                    error = error_type("provider_rejected", "token", module.ExchangeState.RESPONSE_RECEIVED,
                                       provider_code=code)
                    self.assertIsNone(error.provider_code)
            for reason, phase, state in (("missing_scope", "token", module.ExchangeState.RESPONSE_RECEIVED),
                ("provider_rejected", "identity", module.ExchangeState.RESPONSE_RECEIVED),
                ("provider_rejected", "token", module.ExchangeState.UNCONFIRMED)):
                error = error_type(reason, phase, state, provider_code=CODES[module][0])
                self.assertIsNone(error.provider_code)

    async def test_google_refusal_read_failures_do_not_erase_http_refusal(self):
        cases = [Fixture(google, headers=[("content-encoding", "gzip")]),
                 Fixture(google, headers=[("content-length", "999999")]),
                 Fixture(google, headers=[("content-length", "2")]),
                 Fixture(google, headers=[("content-length", "2"), ("content-length", "2")]),
                 Fixture(google, raw=b'{"error":"invalid_grant","error_description":"' + b'x' * 65536 + b'"}')]
        broken = Fixture(google)
        broken.fail_body = True
        cases.append(broken)
        for fixture in cases:
            with self.subTest(headers=fixture.headers, fail_body=fixture.fail_body):
                error = await self.refusal(google, fixture, refresh=True)
                self.assertEqual(error.reason, "provider_rejected")
                self.assertIsNone(getattr(error, "provider_code", None))
                self.assertLessEqual(fixture.yields, 17)

    async def test_google_diagnostic_body_cancel_joins_held_close(self):
        fixture = Fixture(google)
        fixture.body_gate = fixture.close_gate = True
        async with httpx.AsyncClient(transport=httpx.MockTransport(fixture.handle), trust_env=False) as client:
            prepare_httpx_client(client)
            task = asyncio.create_task(invoke(google, client, refresh=True))
            try:
                await asyncio.wait_for(fixture.entered.wait(), 1)
                task.cancel()
                await asyncio.wait_for(fixture.closing.wait(), 1)
                task.cancel()
                await asyncio.sleep(0)
                self.assertFalse(task.done())
            finally:
                fixture.release.set()
            with self.assertRaises(google.GoogleOAuthCancelled) as caught:
                await task
        self.assertEqual(caught.exception.exchange_state, google.ExchangeState.RESPONSE_RECEIVED)
        self.assertEqual(fixture.closed, 1)
        self.assertEqual(len(fixture.requests), 1)

    async def test_google_diagnostic_deadline_joins_close(self):
        fixture = Fixture(google)
        fixture.body_gate = fixture.close_gate = True
        async with httpx.AsyncClient(transport=httpx.MockTransport(fixture.handle), trust_env=False) as client:
            prepare_httpx_client(client)
            task = asyncio.create_task(invoke(google, client, refresh=True, timeout=.02))
            try:
                await asyncio.wait_for(fixture.entered.wait(), 1)
                await asyncio.wait_for(fixture.closing.wait(), 1)
                self.assertFalse(task.done())
            finally:
                fixture.release.set()
            with self.assertRaises(google.GoogleOAuthError) as caught:
                await task
        self.assertEqual(caught.exception.reason, "deadline")
        self.assertIsNone(getattr(caught.exception, "provider_code", None))
        self.assertEqual(fixture.closed, 1)


if __name__ == "__main__":
    unittest.main()
