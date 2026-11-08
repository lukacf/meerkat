"""Real private-pipe transport controls, not native/provider qualification."""
import asyncio
import json
import time
import unittest
from urllib.parse import urlencode

from meerkat_toolkit.host_control import ConsoleLoginSelection
from meerkat_toolkit.mcp_auth import McpAuthError, McpTarget, NativeAuthFailure, _observation, _target
from meerkat_toolkit.mcp_auth_host import HostedMcpAuthBinding
from meerkat_toolkit.mcp_auth_native import NativeHostedMcpClient
from test_host_control import pipes, receive, respond
from test_mcp_auth import TARGET, CODE, STATE
from test_mcp_auth_host import Login, REDIRECT, ORIGIN


class NativeBridgeTests(unittest.IsolatedAsyncioTestCase):
    async def make(self):
        host, reader, writer = pipes()
        self.addCleanup(writer.close)
        self.addCleanup(reader.close)
        self.addCleanup(host.close)
        login = host.begin_console_login(ConsoleLoginSelection("issuer", "subject", "scope",
            int(time.time() * 1000) + 60000))
        enrolled = await asyncio.to_thread(receive, reader)
        respond(writer, enrolled, {"kind":"console_login_enrolled", "enrollment_id":enrolled["id"]})
        await asyncio.to_thread(login.result)
        client = NativeHostedMcpClient(host, login, TARGET, redirect_uri=REDIRECT, timeout=2)
        return client, login, reader, writer

    async def start(self, client, reader, writer, *, joined=False):
        pending = asyncio.create_task(client.auth_mcp_login_start(TARGET.server_name,
            TARGET.server_url, REDIRECT, oauth_account=TARGET.oauth_account))
        request = await asyncio.to_thread(receive, reader)
        respond(writer, request, {"kind":"hosted_mcp_auth", "value": {
            "mcp":TARGET._wire(), "disposition":"joined" if joined else "started",
            "ceremony_id":None if joined else request["id"], "state":STATE,
            "authorize_url":"https://provider.invalid/authorize?state=" + STATE,
            "redirect_uri":REDIRECT}})
        await pending
        return request

    async def test_native_start_failure_survives_custodian_replay_without_new_attempt(self):
        client, _, reader, writer = await self.make()
        login = Login()
        binding = HostedMcpAuthBinding(client, TARGET, login_context=login, redirect_uri=REDIRECT)
        begin = asyncio.create_task(binding.begin(login))
        request = await asyncio.to_thread(receive, reader)
        respond(writer, request, {"kind":"hosted_mcp_auth", "value": {
            "mcp":TARGET._wire(), "auth_error":{"reason":"credential_access_pending"}}})
        for result, cleanup in ((begin, "not_requested"), (binding.begin(login), "not_requested"),
                                (binding.shutdown(), "unconfirmed")):
            with self.assertRaises(McpAuthError) as caught:
                await result
            error = caught.exception
            self.assertEqual(error.code, "native_auth_failure")
            self.assertEqual(error.native_failure, NativeAuthFailure("credential_access_pending"))
            self.assertEqual(error.cleanup.native_cancel, cleanup)
            self.assertIsNone(error.__context__)
        self.assertIsNotNone(client._close_task, "failed start must still reach owned cleanup")
        # Closing the transport exposes whether any unobserved retry was sent.
        client._client.close()
        self.assertEqual(reader.read(), b"")

    async def test_binding_shutdown_retires_verified_ceremony_after_response_rejection(self):
        for rejection in ("outer_validation", "private_handle"):
            with self.subTest(rejection=rejection):
                client, _, reader, writer = await self.make()
                login = Login()
                binding = HostedMcpAuthBinding(client, TARGET, login_context=login, redirect_uri=REDIRECT)
                begin = asyncio.create_task(binding.begin(login))
                request = await asyncio.to_thread(receive, reader)
                value = {"mcp": TARGET._wire(), "disposition": "started",
                    "ceremony_id": request["id"], "state": STATE,
                    "authorize_url": "https://provider.invalid/start", "redirect_uri": REDIRECT}
                if rejection == "outer_validation":
                    value["authorize_url"] = "not-a-provider-url"
                    expected = "invalid_start_response_uncertain"
                else:
                    value["host_attempt_handle"] = "PRIVATE-NATIVE-HANDLE"
                    expected = "start_uncertain"
                respond(writer, request, {"kind": "hosted_mcp_auth", "value": value})
                with self.assertRaises(McpAuthError) as caught:
                    await begin
                self.assertEqual(caught.exception.code, expected)
                self.assertEqual(client._ceremony_id, request["id"])
                closing = asyncio.create_task(binding.shutdown())
                try:
                    cancel = await asyncio.wait_for(asyncio.to_thread(receive, reader), 1)
                    self.assertEqual(cancel["command"]["operation"],
                        {"action": "cancel", "ceremony_id": request["id"]})
                    respond(writer, cancel, {"kind": "hosted_mcp_auth", "value": {
                        "mcp": TARGET._wire(), "cancelled": True, "completed": False}})
                    release = await asyncio.wait_for(asyncio.to_thread(receive, reader), 1)
                    self.assertEqual(release["command"]["operation"],
                        {"action": "release", "ceremony_id": request["id"]})
                    respond(writer, release, {"kind": "hosted_mcp_auth", "value": {"released": True}})
                    for result in (closing, binding.shutdown()):
                        with self.assertRaises(McpAuthError) as caught:
                            await result
                        self.assertEqual(caught.exception.code, expected)
                        self.assertEqual(caught.exception.cleanup.native_cancel, "acknowledged")
                        self.assertIsNone(caught.exception.__context__)
                        self.assertNotIn("PRIVATE-NATIVE-HANDLE", repr(caught.exception))
                    self.assertTrue((await client.close_owned_hosted_login())["cancelled"])
                    client._client.close()
                    self.assertEqual(reader.read(), b"", "no repeated start, cancel or release")
                finally:
                    client._client.close()
                    await asyncio.gather(closing, return_exceptions=True)

    async def test_binding_shutdown_without_verified_ceremony_sends_no_cancel(self):
        for substitution in ("correlation", "target", "missing_id"):
            with self.subTest(substitution=substitution):
                client, _, reader, writer = await self.make()
                login = Login()
                binding = HostedMcpAuthBinding(client, TARGET, login_context=login, redirect_uri=REDIRECT)
                begin = asyncio.create_task(binding.begin(login))
                request = await asyncio.to_thread(receive, reader)
                value = {"mcp": TARGET._wire(), "disposition": "started",
                    "ceremony_id": request["id"], "state": STATE,
                    "authorize_url": "https://provider.invalid/start", "redirect_uri": REDIRECT}
                if substitution == "correlation": value["ceremony_id"] += 1
                elif substitution == "target": value["mcp"] = {}
                else: del value["ceremony_id"]
                respond(writer, request, {"kind": "hosted_mcp_auth", "value": value})
                for result in (begin, binding.shutdown()):
                    with self.assertRaises(McpAuthError) as caught:
                        await result
                    self.assertEqual(caught.exception.code, "start_uncertain")
                    self.assertIsNone(caught.exception.__context__)
                self.assertIsNone(client._ceremony_id)
                self.assertIsNotNone(client._close_task)
                client._client.close()
                self.assertEqual(reader.read(), b"", "unverified reply cannot authorize cancellation")

    async def test_native_completion_failure_keeps_http_receipt_and_cleanup_separate(self):
        client, _, reader, writer = await self.make()
        login = Login()
        binding = HostedMcpAuthBinding(client, TARGET, login_context=login, redirect_uri=REDIRECT)
        begin = asyncio.create_task(binding.begin(login))
        started = await asyncio.to_thread(receive, reader)
        respond(writer, started, {"kind":"hosted_mcp_auth", "value": {
            "mcp":TARGET._wire(), "disposition":"started", "ceremony_id":started["id"],
            "state":STATE, "authorize_url":"https://provider.invalid/start", "redirect_uri":REDIRECT}})
        self.assertEqual(await begin, "started")
        response = await binding.handle_callback(login, method="POST", redirect_uri=REDIRECT,
            origin=ORIGIN, body=json.dumps({"query":urlencode({"state":STATE,"code":CODE})}).encode())
        self.assertEqual(response.status, 202)
        completion = await asyncio.to_thread(receive, reader)
        waiter = asyncio.create_task(binding.wait(login))
        await asyncio.sleep(0)
        self.assertFalse(waiter.done())
        waiter.cancel()
        with self.assertRaises(asyncio.CancelledError):
            await waiter
        # Native completion survives HTTP/observer return and cancellation.
        respond(writer, completion, {"kind":"hosted_mcp_auth", "value": {
            "mcp":TARGET._wire(), "auth_error":{"reason":"upstream_failure"}}})
        cancel = await asyncio.to_thread(receive, reader)
        self.assertEqual(cancel["command"]["operation"], {"action":"cancel", "ceremony_id":started["id"]})
        respond(writer, cancel, {"kind":"hosted_mcp_auth", "value": {
            "mcp":TARGET._wire(), "cancelled":False, "completed":False}})
        release = await asyncio.to_thread(receive, reader)
        self.assertEqual(release["command"]["operation"], {"action":"release", "ceremony_id":started["id"]})
        respond(writer, release, {"kind":"hosted_mcp_auth", "value":{"released":True}})
        with self.assertRaises(McpAuthError) as caught:
            await binding.wait(login)
        self.assertEqual(caught.exception.native_failure, NativeAuthFailure("upstream_failure"))
        self.assertEqual(caught.exception.cleanup.native_cancel, "unconfirmed")
        self.assertEqual(str(caught.exception), "native_auth_failure")
        self.assertIsNone(caught.exception.__context__)
        self.assertEqual((await binding.shutdown()).native_cancel, "unconfirmed")
        client._client.close()
        self.assertEqual(reader.read(), b"")

    async def test_native_status_and_describe_preserve_typed_details(self):
        client, enrollment, reader, writer = await self.make()
        login = Login()
        binding = HostedMcpAuthBinding(client, TARGET, login_context=login, redirect_uri=REDIRECT)
        for operation in (binding.observe(login),
                          NativeHostedMcpClient.for_target(client._client, enrollment, TARGET, timeout=2)):
            pending = asyncio.create_task(operation)
            request = await asyncio.to_thread(receive, reader)
            respond(writer, request, {"kind":"hosted_mcp_auth", "value": {
                "mcp":TARGET._wire(), "auth_error":{
                    "reason":"invalid_target", "detail":"remote_custody_required"}}})
            with self.assertRaises(McpAuthError) as caught:
                await pending
            self.assertEqual(caught.exception.native_failure,
                NativeAuthFailure("invalid_target", "remote_custody_required"))

    async def test_malformed_or_unbound_native_errors_remain_unconfirmed(self):
        client, _, reader, writer = await self.make()
        for value in (
            {"mcp":{}, "auth_error":{"reason":"upstream_failure"}},
            {"mcp":TARGET._wire(), "auth_error":{"reason":"upstream_failure", "detail":"private provider text"}},
            {"mcp":TARGET._wire(), "auth_error":{"reason":"upstream_failure", "message":CODE}},
            {"mcp":TARGET._wire(), "auth_error":{"reason":None}},
            {"mcp":TARGET._wire(), "auth_error":{"reason":"upstream_failure"}, "account_id":TARGET.oauth_account},
        ):
            pending = asyncio.create_task(client.auth_mcp_status(TARGET.server_name,
                TARGET.server_url, oauth_account=TARGET.oauth_account))
            request = await asyncio.to_thread(receive, reader)
            respond(writer, request, {"kind":"hosted_mcp_auth", "value":value})
            with self.assertRaises(McpAuthError) as caught:
                await pending
            self.assertEqual(caught.exception.code, "host_native_result_unconfirmed")
            self.assertIsNone(caught.exception.native_failure)
            self.assertIsNone(caught.exception.__context__)

    async def test_route_is_read_from_native_before_any_oauth_attempt(self):
        client, enrollment, reader, writer = await self.make()
        prepared = asyncio.create_task(NativeHostedMcpClient.for_target(client._client, enrollment, TARGET, timeout=2))
        request = await asyncio.to_thread(receive, reader)
        self.assertEqual(request["command"]["operation"], {"action":"describe", "target":TARGET._wire()})
        exact_redirect = REDIRECT + "/native-derived-target"
        respond(writer, request, {"kind":"hosted_mcp_auth", "value": {
            "mcp":TARGET._wire(),"redirect_uri":exact_redirect}})
        native = await prepared
        self.assertEqual(native.redirect_uri, exact_redirect)
        self.assertIsNone(native._start_task)

    async def test_authenticated_callback_uses_same_pipe_and_preserves_issuer(self):
        client, enrollment, reader, writer = await self.make()
        login = Login()
        binding = HostedMcpAuthBinding(client, TARGET, login_context=login, redirect_uri=REDIRECT)
        begin = asyncio.create_task(binding.begin(login))
        request = await asyncio.to_thread(receive, reader)
        self.assertEqual(request["command"], {"kind":"hosted_mcp_auth",
            "enrollment_id":enrollment.enrollment_id, "operation":{"action":"start", "target":TARGET._wire()}})
        respond(writer, request, {"kind":"hosted_mcp_auth", "value": {
            "mcp":TARGET._wire(), "disposition":"started", "ceremony_id":request["id"],
            "state":STATE, "authorize_url":"https://provider.invalid/authorize?state=" + STATE,
            "redirect_uri":REDIRECT}})
        self.assertEqual(await begin, "started")
        issuer = "https://issuer.invalid/exact%2Fvalue"
        response = await binding.handle_callback(login, method="POST", redirect_uri=REDIRECT,
            origin=ORIGIN, body=json.dumps({"query":urlencode({"state":STATE,"code":CODE,"iss":issuer})}).encode())
        self.assertEqual(response.status, 202)
        callback = await asyncio.to_thread(receive, reader)
        self.assertEqual(callback["command"], {"kind":"hosted_mcp_auth",
            "enrollment_id":enrollment.enrollment_id, "operation":{"action":"complete",
            "ceremony_id":request["id"],"redirect_uri":REDIRECT,"state":STATE,"code":CODE,"iss":issuer}})
        respond(writer, callback, {"kind":"hosted_mcp_auth", "value": {
            "mcp":TARGET._wire(),"account_id":TARGET.oauth_account,"expires_at":None,
            "has_refresh_token":True,"scopes":["read"]}})
        self.assertEqual((await binding.wait(login)).account_id, TARGET.oauth_account)
        shutdown = asyncio.create_task(binding.shutdown())
        cancel = await asyncio.to_thread(receive, reader)
        self.assertEqual(cancel["command"]["operation"], {"action":"cancel","ceremony_id":request["id"]})
        respond(writer, cancel, {"kind":"hosted_mcp_auth", "value": {
            "mcp":TARGET._wire(),"cancelled":False,"completed":True}})
        release = await asyncio.to_thread(receive, reader)
        self.assertEqual(release["command"]["operation"], {"action":"release","ceremony_id":request["id"]})
        respond(writer, release, {"kind":"hosted_mcp_auth", "value":{"released":True}})
        self.assertEqual((await shutdown).native_cancel, "not_needed")
        for marker in (CODE, STATE, issuer):
            self.assertNotIn(marker, repr(client))

    async def test_joiner_has_no_callback_or_cancel_custody(self):
        client, _, reader, writer = await self.make()
        await self.start(client, reader, writer, joined=True)
        with self.assertRaises(McpAuthError):
            await client.auth_mcp_login_complete_registered(TARGET.server_name, TARGET.server_url,
                oauth_account=TARGET.oauth_account, code=CODE, state=STATE, redirect_uri=REDIRECT)
        self.assertFalse((await client.close_owned_hosted_login())["cancelled"])

    async def test_cancelled_begin_waiter_still_closes_actual_owned_attempt(self):
        client, _, reader, writer = await self.make()
        begin = asyncio.create_task(client.auth_mcp_login_start(TARGET.server_name,
            TARGET.server_url, REDIRECT, oauth_account=TARGET.oauth_account))
        request = await asyncio.to_thread(receive, reader)
        begin.cancel()
        with self.assertRaises(asyncio.CancelledError):
            await begin
        close = asyncio.create_task(client.close_owned_hosted_login())
        respond(writer, request, {"kind":"hosted_mcp_auth", "value": {
            "mcp":TARGET._wire(),"disposition":"started","ceremony_id":request["id"],
            "state":STATE,"authorize_url":"https://provider.invalid/start","redirect_uri":REDIRECT}})
        cancellation = await asyncio.to_thread(receive, reader)
        self.assertEqual(cancellation["command"]["operation"], {"action":"cancel", "ceremony_id":request["id"]})
        respond(writer, cancellation, {"kind":"hosted_mcp_auth",
            "value":{"mcp":TARGET._wire(),"cancelled":True,"completed":False}})
        release = await asyncio.to_thread(receive, reader)
        self.assertEqual(release["command"]["operation"], {"action":"release","ceremony_id":request["id"]})
        respond(writer, release, {"kind":"hosted_mcp_auth", "value":{"released":True}})
        self.assertTrue((await close)["cancelled"])
        self.assertTrue((await client.close_owned_hosted_login())["cancelled"])

    async def test_substitutions_refuse_before_any_native_write(self):
        client, enrollment, reader, writer = await self.make()
        with self.assertRaises(McpAuthError):
            await client.auth_mcp_login_start("other", TARGET.server_url, REDIRECT,
                oauth_account=TARGET.oauth_account)
        host2, reader2, writer2 = pipes()
        try:
            with self.assertRaises(McpAuthError):
                NativeHostedMcpClient(host2, enrollment, TARGET, redirect_uri=REDIRECT)
        finally:
            host2.close(); reader2.close(); writer2.close()
        client._client.close()
        self.assertEqual(reader.read(), b"")

    async def test_wrong_ceremony_correlation_is_sanitized(self):
        client, _, reader, writer = await self.make()
        pending = asyncio.create_task(client.auth_mcp_login_start(TARGET.server_name,
            TARGET.server_url, REDIRECT, oauth_account=TARGET.oauth_account))
        request = await asyncio.to_thread(receive, reader)
        respond(writer, request, {"kind":"hosted_mcp_auth", "value": {
            "mcp":TARGET._wire(),"disposition":"started","ceremony_id":request["id"] + 1,
            "state":STATE,"authorize_url":"https://provider.invalid/start","redirect_uri":REDIRECT}})
        with self.assertRaises(McpAuthError) as caught:
            await pending
        self.assertEqual(str(caught.exception), "host_native_result_unconfirmed")
        self.assertIsNone(caught.exception.__context__)

    async def test_explicit_discover_and_unverified_roundtrip_without_account_invention(self):
        existing, enrollment, reader, writer = await self.make()
        for mode in ("discover", "unverified"):
            target = McpTarget(TARGET.server_name, TARGET.server_url, oauth_account_selection=mode)
            client = NativeHostedMcpClient(existing._client, enrollment, target, redirect_uri=REDIRECT, timeout=2)
            start = asyncio.create_task(client.auth_mcp_login_start(target.server_name, target.server_url,
                REDIRECT, oauth_account_selection=mode))
            request = await asyncio.to_thread(receive, reader)
            self.assertEqual(request["command"]["operation"]["target"], {
                "server_name":target.server_name,"server_url":target.server_url,"oauth_account_selection":mode})
            # Joined is enough to inspect selection serialization without
            # pretending this pipe fixture performed provider discovery.
            respond(writer, request, {"kind":"hosted_mcp_auth", "value": {
                "mcp":target._wire(),"disposition":"joined","ceremony_id":None,
                "state":STATE,"authorize_url":"https://provider.invalid/start","redirect_uri":REDIRECT}})
            await start
            with self.assertRaises(McpAuthError):
                await client.auth_mcp_status(target.server_name, target.server_url,
                    oauth_account_selection="unverified" if mode == "discover" else "discover")
            await client.close_owned_hosted_login()

    def test_account_observation_cannot_downgrade_or_invent_subject(self):
        for mode, account, verification in (("discover", "verified-subject", "verified"),
                                           ("unverified", None, "unverified")):
            target = McpTarget(TARGET.server_name, TARGET.server_url, oauth_account_selection=mode)
            _target(target)
            ready = {"mcp":target._wire(),"account_id":account,"account_verification":verification,
                "expires_at":None,"scopes":["read"],"has_refresh_token":True}
            self.assertEqual(_observation(ready, target, completed=True).account_id, account)
            for changed in ({**ready,"account_verification":"legacy"},
                            {**ready,"account_verification":None},
                            {**ready,"account_id":None if mode == "discover" else "invented"},
                            {**ready,"mcp":TARGET._wire()}):
                with self.assertRaises(ValueError):
                    _observation(changed, target, completed=True)
        for target in (McpTarget("x", TARGET.server_url),
                       McpTarget("x", TARGET.server_url,"subject","discover"),
                       McpTarget("x", TARGET.server_url,oauth_account_selection="legacy")):
            with self.assertRaises(ValueError):
                _target(target)
