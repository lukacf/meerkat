"""Offline host transport checks. All accounts, URLs and bearer markers are fixtures."""
import asyncio
import dataclasses
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch
from urllib.parse import urlencode, urlsplit

from meerkat_toolkit.mcp_auth import (
    JoinedMcpLogin, McpAuthCancelled, McpAuthError, McpTarget, StartedMcpLogin,
    begin_mcp_login, observe_mcp_auth,
)

TARGET = McpTarget("fixture", "https://mcp.invalid/api", "account-a")
STATE = "FIXTURE-STATE-SECRET"
CODE = "FIXTURE-CODE-SECRET"
URL = f"https://auth.invalid/authorize?state={STATE}&private=FIXTURE-URL-SECRET"


def ready(target=TARGET):
    return {"mcp": target._wire(), "account_id": target.oauth_account,
            "expires_at": None, "scopes": ["read"], "has_refresh_token": True}


class Client:
    def __init__(self):
        self.calls = []
        self.redirect = None
        self.target = TARGET
        self.disposition = "started"
        self.complete_entered = asyncio.Event()
        self.cancel_entered = asyncio.Event()
        self.complete_gate = None
        self.cancel_gate = None
        self.start_gate = None
        self.start_error = self.complete_error = self.cancel_error = None
        self.result = None
        self.start_override = None
        self.cancel_self = False

    async def auth_mcp_login_start(self, name, url, redirect_uri, *, oauth_account):
        self.calls.append("start")
        self.target = McpTarget(name, url, oauth_account)
        self.redirect = redirect_uri
        # A real connection must already be possible before native start.
        port = urlsplit(redirect_uri).port
        _, writer = await asyncio.open_connection("127.0.0.1", port)
        writer.close()
        await writer.wait_closed()
        if self.start_gate:
            await self.start_gate.wait()
        if self.start_error:
            raise RuntimeError(self.start_error)
        result = {"mcp": self.target._wire(), "state": STATE, "authorize_url": URL,
                  "redirect_uri": redirect_uri, "disposition": self.disposition}
        if self.disposition == "joined":
            result["redirect_uri"] = "http://127.0.0.1:41234/callback"
        if self.start_override:
            result.update(self.start_override)
        return result

    async def auth_mcp_login_complete(self, name, url, *, code, state, redirect_uri,
                                      oauth_account):
        self.calls.append(("complete", name, url, code, state, redirect_uri, oauth_account))
        self.complete_entered.set()
        if self.complete_gate:
            await self.complete_gate.wait()
        if self.complete_error:
            raise RuntimeError(self.complete_error)
        return self.result if self.result is not None else ready(self.target)

    async def auth_mcp_login_cancel(self, name, url, state, *, oauth_account):
        self.calls.append(("cancel", name, url, state, oauth_account))
        self.cancel_entered.set()
        if self.cancel_self:
            raise asyncio.CancelledError("FIXTURE-NATIVE-CANCEL-PRIVATE")
        if self.cancel_gate:
            await self.cancel_gate.wait()
        if self.cancel_error:
            raise RuntimeError(self.cancel_error)
        return {"mcp": self.target._wire(), "cancelled": True}

    async def auth_mcp_status(self, name, url, *, oauth_account):
        self.calls.append("status")
        return {"mcp": McpTarget(name, url, oauth_account)._wire(),
                "phase": "authorized", "account_id": oauth_account}


async def request(redirect, query, *, path="/callback", host=None, method="GET"):
    address = urlsplit(redirect)
    reader, writer = await asyncio.open_connection("127.0.0.1", address.port)
    writer.write(f"{method} {path}?{query} HTTP/1.1\r\nHost: {host or address.netloc}\r\n\r\n".encode())
    await writer.drain()
    result = await asyncio.wait_for(reader.read(), 3)
    writer.close()
    await writer.wait_closed()
    return result


async def callback(redirect, **kw):
    return await request(redirect, urlencode({"state": STATE, "code": CODE, **kw}))


async def closed(redirect):
    try:
        _, writer = await asyncio.open_connection("127.0.0.1", urlsplit(redirect).port)
    except OSError:
        return True
    writer.close()
    await writer.wait_closed()
    return False


class HostAdapterTests(unittest.IsolatedAsyncioTestCase):
    async def start(self, client=None, **kwargs):
        client = client or Client()
        attempt = await begin_mcp_login(client, TARGET, **kwargs)
        if isinstance(attempt, StartedMcpLogin):
            self.addAsyncCleanup(attempt.close)
        return client, attempt

    async def test_real_callback_native_complete_and_safe_projection(self):
        client, attempt = await self.start()
        seen = []
        async def presenter(url):
            seen.append(url)
        self.assertTrue(await attempt.present(presenter))
        self.assertEqual(seen, [URL])
        self.assertEqual(dataclasses.asdict(attempt.presentation), {
            "authorization_origin": "https://auth.invalid", "requested_account_label": "account-a"})
        response = await callback(client.redirect, scope="untrusted-extra", account="account-b")
        self.assertTrue(response.startswith(b"HTTP/1.1 202"))
        result = await attempt.wait()
        self.assertEqual(result.scopes, ("read",))
        self.assertEqual(result.account_id, "account-a")
        self.assertEqual(client.calls[1], ("complete", TARGET.server_name, TARGET.server_url,
                                         CODE, STATE, client.redirect, TARGET.oauth_account))
        self.assertEqual(len(client.calls), 2)
        self.assertTrue(await closed(client.redirect))
        for marker in (STATE, CODE, "FIXTURE-URL-SECRET"):
            self.assertNotIn(marker, repr((attempt, attempt.presentation, result, response)))
        self.assertEqual((await attempt.close()).native_cancel, "not_needed")

    async def test_join_retires_only_unused_socket(self):
        client = Client()
        client.disposition = "joined"
        client, joined = await self.start(client)
        self.assertIsInstance(joined, JoinedMcpLogin)
        self.assertFalse(hasattr(joined, "close"))
        self.assertFalse(hasattr(joined, "present"))
        self.assertTrue(await closed(client.redirect))
        self.assertEqual(client.calls, ["start"])

    async def test_invalid_callback_never_consumes_valid_attempt(self):
        client, attempt = await self.start()
        cases = [
            (f"state=wrong&code={CODE}", {}),
            (f"state={STATE}&code={CODE}&code=duplicate", {}),
            (f"state={STATE}&code={CODE}&error=denied", {}),
            (f"state={STATE}&code=%QQ", {}),
            (f"state={STATE}&code=%ff", {}),
            (f"state={STATE}&code={CODE}", {"path": "/other"}),
            (f"state={STATE}&code={CODE}", {"host": "evil.invalid"}),
            (f"state={STATE}&code={CODE}", {"method": "POST"}),
            ("&".join(f"x{i}=1" for i in range(33)), {}),
        ]
        for query, options in cases:
            self.assertTrue((await request(client.redirect, query, **options)).startswith(b"HTTP/1.1 400"))
        self.assertEqual(client.calls, ["start"])
        await callback(client.redirect)
        await attempt.wait()
        self.assertEqual(sum(isinstance(x, tuple) and x[0] == "complete" for x in client.calls), 1)

    async def test_duplicate_callback_and_cancelled_waiter(self):
        client = Client()
        client.complete_gate = asyncio.Event()
        client, attempt = await self.start(client)
        waiter = asyncio.create_task(attempt.wait())
        waiter.cancel()
        with self.assertRaises(asyncio.CancelledError):
            await waiter
        await callback(client.redirect)
        await client.complete_entered.wait()
        self.assertTrue((await callback(client.redirect)).startswith(b"HTTP/1.1 400"))
        client.complete_gate.set()
        self.assertEqual((await attempt.wait()).phase, "authorized")
        self.assertFalse(any(isinstance(x, tuple) and x[0] == "cancel" for x in client.calls))

    async def test_denial_calls_native_cancel_once_without_error_echo(self):
        client, attempt = await self.start()
        response = await request(client.redirect, urlencode({"state": STATE, "error": "access_denied",
                                                             "error_description": "FIXTURE-PRIVATE"}))
        self.assertNotIn(b"FIXTURE-PRIVATE", response)
        with self.assertRaises(McpAuthError) as caught:
            await attempt.wait()
        self.assertEqual(caught.exception.code, "authorization_denied")
        self.assertEqual(caught.exception.cleanup.native_cancel, "acknowledged")
        await attempt.close()
        self.assertEqual(len(client.calls), 2)

    async def test_presenter_failure_is_advisory(self):
        client, attempt = await self.start()
        async def presenter(url):
            raise RuntimeError(URL)
        self.assertFalse(await attempt.present(presenter))
        self.assertEqual(client.calls, ["start"])
        await callback(client.redirect)
        await attempt.wait()

    async def test_two_accounts_keep_callback_and_native_arguments_separate(self):
        first, a = await self.start()
        second = Client()
        other = McpTarget(TARGET.server_name, TARGET.server_url, "account-b")
        b = await begin_mcp_login(second, other)
        self.addAsyncCleanup(b.close)
        # Wrong-account completion cannot turn the first account into the second.
        first.result = ready(other)
        await asyncio.gather(callback(first.redirect), callback(second.redirect))
        with self.assertRaises(McpAuthError) as caught:
            await a.wait()
        self.assertEqual(caught.exception.code, "invalid_completion_response")
        self.assertEqual((await b.wait()).account_id, "account-b")
        self.assertNotEqual(first.redirect, second.redirect)
        self.assertEqual(second.calls[1][-1], "account-b")
        self.assertFalse(any(isinstance(x, tuple) and x[0] == "cancel" for x in second.calls))

    async def test_close_joins_partial_http_and_repeated_cancel(self):
        client = Client()
        client.cancel_gate = asyncio.Event()
        client, attempt = await self.start(client)
        reader, writer = await asyncio.open_connection("127.0.0.1", urlsplit(client.redirect).port)
        writer.write(b"GET /callback?")
        await writer.drain()
        closing = asyncio.create_task(attempt.close())
        await client.cancel_entered.wait()
        closing.cancel()
        await asyncio.sleep(0)
        closing.cancel()
        client.cancel_gate.set()
        with self.assertRaises(McpAuthCancelled) as caught:
            await closing
        self.assertTrue(caught.exception.cleanup.callback_closed)
        self.assertTrue(await closed(client.redirect))
        await asyncio.wait_for(reader.read(), 2)
        writer.close()
        await writer.wait_closed()
        self.assertEqual((await attempt.close()).native_cancel, "acknowledged")
        self.assertEqual(sum(isinstance(x, tuple) and x[0] == "cancel" for x in client.calls), 1)

    async def test_entered_complete_and_cancel_uncertainty_are_not_success(self):
        client = Client()
        client.complete_gate = asyncio.Event()
        client.cancel_error = "FIXTURE-NATIVE-PRIVATE"
        client, attempt = await self.start(client)
        await callback(client.redirect)
        await client.complete_entered.wait()
        cleanup = await attempt.close()
        self.assertEqual(cleanup.native_cancel, "unconfirmed")
        with self.assertRaises(McpAuthError) as caught:
            await attempt.wait()
        self.assertEqual(caught.exception.code, "completion_uncertain")
        self.assertNotIn("FIXTURE", repr(caught.exception))
        self.assertEqual(len(client.calls), 3)

    async def test_start_timeout_cancel_and_malformed_reply_leave_no_socket(self):
        for mode in ("timeout", "cancel", "malformed", "error"):
            client = Client()
            if mode in ("timeout", "cancel"):
                client.start_gate = asyncio.Event()
            elif mode == "malformed":
                client.start_override = {"mcp": {"server_name": "wrong"}}
            else:
                client.start_error = "FIXTURE-START-PRIVATE"
            call = asyncio.create_task(begin_mcp_login(client, TARGET, rpc_timeout=.05))
            if mode == "cancel":
                while client.redirect is None:
                    await asyncio.sleep(0)
                call.cancel()
            with self.assertRaises((McpAuthError, McpAuthCancelled)) as caught:
                await call
            self.assertNotIn("FIXTURE", repr(caught.exception))
            if mode == "malformed":
                self.assertEqual(caught.exception.code, "invalid_start_response_uncertain")
                self.assertEqual(caught.exception.cleanup.native_cancel, "not_requested")
            self.assertTrue(await closed(client.redirect))
            self.assertEqual(client.calls, ["start"])

    async def test_bind_and_prerequisite_refusal_precede_native_call(self):
        client = Client()
        for target in (McpTarget("x", TARGET.server_url, ""), McpTarget("x", "javascript:bad", "a")):
            with self.assertRaises(McpAuthError):
                await begin_mcp_login(client, target)
        with patch("asyncio.start_server", side_effect=OSError("FIXTURE-BIND-PRIVATE")):
            with self.assertRaises(McpAuthError) as caught:
                await begin_mcp_login(client, TARGET)
            self.assertEqual(caught.exception.code, "callback_bind_failed")
        self.assertEqual(client.calls, [])

    async def test_callback_wait_budget_cancels_owned_attempt_not_expiry(self):
        client, attempt = await self.start(callback_timeout=.02)
        with self.assertRaises(McpAuthError) as caught:
            await attempt.wait()
        self.assertEqual(caught.exception.code, "callback_wait_abandoned")
        self.assertEqual(caught.exception.cleanup.native_cancel, "acknowledged")
        self.assertTrue(await closed(client.redirect))
        self.assertEqual((await observe_mcp_auth(client, TARGET)).phase, "authorized")

    async def test_immediate_close_settles_original_callback(self):
        client = Client()
        attempt = await begin_mcp_login(client, TARGET)
        cleanup = await attempt.close()
        self.assertEqual(cleanup.native_cancel, "acknowledged")
        self.assertTrue(await closed(client.redirect))
        self.assertEqual(len(client.calls), 2)

    async def test_native_cancelled_cleanup_is_unconfirmed_without_spin(self):
        client = Client()
        client.cancel_self = True
        attempt = await begin_mcp_login(client, TARGET)
        cleanup = await asyncio.wait_for(attempt.close(), 1)
        self.assertEqual(cleanup.native_cancel, "unconfirmed")
        self.assertTrue(cleanup.callback_closed)
        self.assertTrue(await closed(client.redirect))
        self.assertEqual(len(client.calls), 2)


# Executed by the exact public SDK as its explicitly selected JSON-RPC child.
# It is a protocol fixture, not native Meerkat or an OAuth provider.
RPC_FIXTURE = '''import json,sys
from pathlib import Path
from meerkat.generated.types import CONTRACT_VERSION
log=Path(__file__).with_suffix('.calls')
redirect=None
for line in sys.stdin:
 r=json.loads(line); m=r['method']; p=r['params']
 with log.open('a') as out: out.write(json.dumps(r)+'\\n')
 if m=='initialize': result={'contract_version':CONTRACT_VERSION,'methods':[]}
 elif m=='capabilities/get': result={'capabilities':[]}
 elif m=='auth/login/start':
  redirect=p['redirect_uri']; result={'mcp':p['mcp'],'state':'FIXTURE-STATE-SECRET','authorize_url':'https://auth.invalid/?state=FIXTURE-STATE-SECRET','redirect_uri':redirect,'disposition':'started'}
 elif m=='auth/login/complete':
  assert p['redirect_uri']==redirect and p['state']=='FIXTURE-STATE-SECRET' and p['code']=='FIXTURE-CODE-SECRET'
  result={'mcp':p['mcp'],'account_id':p['mcp']['oauth_account'],'scopes':['read'],'has_refresh_token':True,'extra_private':'FIXTURE-RESPONSE-PRIVATE'}
 elif m=='auth/login/cancel': result={'mcp':p['mcp'],'cancelled':True}
 elif m=='auth/status/get': result={'mcp':p['mcp'],'phase':'authorized','account_id':p['mcp']['oauth_account']}
 else: raise AssertionError(m)
 print(json.dumps({'jsonrpc':'2.0','id':r['id'],'result':result}),flush=True)
'''


@unittest.skipUnless(os.environ.get("TOOLKIT_TEST_PUBLIC_SDK"), "exact public SDK source path required")
class PublicSdkFramingTests(unittest.IsolatedAsyncioTestCase):
    async def test_actual_sdk_framing_and_loopback_without_native(self):
        sdk = Path(os.environ["TOOLKIT_TEST_PUBLIC_SDK"]).resolve()
        sys.path.insert(0, str(sdk))
        self.addCleanup(sys.path.remove, str(sdk))
        from meerkat import MeerkatClient
        with tempfile.TemporaryDirectory(prefix="toolkit-auth-rpc-") as temp:
            script = Path(temp) / "public-rpc-fixture"
            script.write_text(f"#!{sys.executable}\n" + RPC_FIXTURE)
            script.chmod(0o700)
            # Parent already runs with an explicit minimal environment. The
            # fixture needs only the pinned Python source, never native lookup.
            with patch.dict(os.environ, {"PYTHONPATH": str(sdk)}):
                client = MeerkatClient(str(script))
                await client.connect()
                self.addAsyncCleanup(client.close)
                attempt = await begin_mcp_login(client, TARGET)
                self.addAsyncCleanup(attempt.close)
                calls = [json.loads(x) for x in script.with_suffix('.calls').read_text().splitlines()]
                redirect = calls[-1]['params']['redirect_uri']
                await callback(redirect)
                result = await attempt.wait()
                self.assertEqual(result.account_id, TARGET.oauth_account)
                self.assertNotIn("FIXTURE", repr(result))
                self.assertEqual((await observe_mcp_auth(client, TARGET)).phase, "authorized")
                other = await begin_mcp_login(client, TARGET)
                self.assertEqual((await other.close()).native_cancel, "acknowledged")
                await client.close()
                calls = [json.loads(x) for x in script.with_suffix('.calls').read_text().splitlines()]
                self.assertEqual([x['method'] for x in calls], [
                    'initialize', 'capabilities/get', 'auth/login/start', 'auth/login/complete',
                    'auth/status/get', 'auth/login/start', 'auth/login/cancel'])
                self.assertEqual(calls[3]['params'], {'mcp':TARGET._wire(), 'code':CODE,
                                                     'state':STATE, 'redirect_uri':redirect})


if __name__ == "__main__":
    unittest.main()
