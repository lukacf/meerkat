"""Real SDK + anonymous-pipe + ASGI controls, not native/provider qualification.

Run with the reviewed Connect and matching Meta SDK source on PYTHONPATH.
The peer below is an inert protocol fixture. No gateway, OAuth provider or
credential store is started, and every request is recorded for assertions.
"""
import asyncio
from contextlib import asynccontextmanager
import json
import os
from urllib.parse import urlencode

import httpx
import pytest

auth = pytest.importorskip("meerkat_toolkit.mcp_auth")
from meerkat_toolkit.host_control import PrivateHostClient
from launchkit.templates_v2.console_auth import ConsoleFront, HOSTED_MCP_PATH, ISSUER
from launchkit.templates_v2.native_console_login import NativeConsoleLogin
from test_console_public_origin import sign_in, PASSWORD

pytestmark = pytest.mark.asyncio
ORIGIN = "https://console.example.test"
REDIRECT = ORIGIN + "/native-auth/server"
TARGET = auth.McpTarget("Configured service", "https://service.example.test/mcp", oauth_account="fixture-account")
STATE = "fixture-state-not-a-credential"
CODE = "fixture-code-not-a-credential"


class Peer:
    def __init__(self):
        child_read, parent_write = os.pipe()
        parent_read, child_write = os.pipe()
        self.client = PrivateHostClient(parent_write, parent_read, timeout=3)
        self.reader = os.fdopen(child_read, "rb")
        self.writer = os.fdopen(child_write, "wb", buffering=0)
        self.calls = []
        self.redirect = REDIRECT
        self.block_action = None
        self.entered, self.release = asyncio.Event(), asyncio.Event()
        self.completed = False
        self.joined = False
        self.describe_failure = None
        self.start_case = None
        self.task = asyncio.create_task(self.run())

    async def run(self):
        while raw := await asyncio.to_thread(self.reader.readline):
            request = json.loads(raw)
            command = request["command"]
            self.calls.append(command)
            kind = command["kind"]
            if kind == "enroll_console_login":
                value = {"kind": "console_login_enrolled", "enrollment_id": request["id"]}
            elif kind == "retire_console_login":
                value = {"kind": "console_login_retired", "enrollment_id": command["enrollment_id"],
                         "observation": {"fenced": True, "reservations_drained": True, "uncertain": False}}
            else:
                assert kind == "hosted_mcp_auth"
                operation = command["operation"]
                action = operation["action"]
                if action == self.block_action:
                    self.entered.set()
                    await self.release.wait()
                result = {"mcp": TARGET._wire()}
                if action == "describe":
                    assert operation["target"] == TARGET._wire()
                    if self.describe_failure is not None:
                        result["auth_error"] = self.describe_failure
                    else:
                        result["redirect_uri"] = self.redirect
                elif action == "start":
                    assert operation["target"] == TARGET._wire()
                    result.update(disposition="joined" if self.joined else "started",
                        ceremony_id=None if self.joined else request["id"], state=STATE,
                        authorize_url="https://provider.example.test/authorize?state=" + STATE,
                        redirect_uri=self.redirect)
                    if self.start_case in {"invalid_target", "infrastructure", "refused"}:
                        result = {"mcp": TARGET._wire(), "auth_error": {
                            "reason": self.start_case, "detail": "remote_custody_required"}}
                    elif self.start_case == "invalid_presentation":
                        result["authorize_url"] = "not a provider URL"
                    elif self.start_case == "lost_reply":
                        continue
                elif action == "complete":
                    assert operation["redirect_uri"] == REDIRECT
                    assert operation["state"] == STATE and operation["code"] == CODE
                    self.completed = True
                    result.update(account_id=TARGET.oauth_account, expires_at=None,
                                  scopes=["fixture-read"], has_refresh_token=True)
                elif action == "status":
                    result.update(phase="authorized" if self.completed else "authorization_required",
                                  account_id=TARGET.oauth_account if self.completed else None, expires_at=None)
                elif action == "cancel":
                    result["cancelled"] = not self.completed
                elif action == "release":
                    result = {"released": True}
                else:
                    raise AssertionError(action)
                value = {"kind": "hosted_mcp_auth", "value": result}
            self.writer.write(json.dumps({"version": 1, "id": request["id"], "result": value}).encode() + b"\n")

    def actions(self):
        return [command["operation"]["action"] if command["kind"] == "hosted_mcp_auth"
                else command["kind"] for command in self.calls]

    async def close(self):
        self.release.set()
        self.client.close()
        await asyncio.wait_for(self.task, 3)
        self.reader.close()
        self.writer.close()


@asynccontextmanager
async def boundary(tmp_path, *, cleanup_error=None):
    peer = Peer()
    owners, contexts = [], []

    def factory(context):
        contexts.append(context)
        owner = NativeConsoleLogin(peer.client, context, issuer=ISSUER)
        owners.append(owner)
        return owner

    front = ConsoleFront(tmp_path, 8855, "owner", PASSWORD, public_origin=ORIGIN,
        hosted_mcp_targets=(TARGET,), console_login_factory=factory, runtime_namespace="meta",
        transport=httpx.MockTransport(lambda _: pytest.fail("No native HTTP proxy call is allowed.")))
    app = front.application("http://127.0.0.1:61347")
    key = next(iter(front._mcp_targets))
    async with httpx.AsyncClient(transport=httpx.ASGITransport(app=app), base_url=ORIGIN) as client:
        try:
            await sign_in(client, front)
            yield front, client, peer, key, contexts
        finally:
            peer.release.set()
            try:
                if cleanup_error is None:
                    await asyncio.wait_for(front.close(), 5)
                else:
                    # Match only teardown, so a body assertion cannot be
                    # replaced by the expected cleanup error and pass.
                    with pytest.raises(RuntimeError, match=cleanup_error):
                        await asyncio.wait_for(front.close(), 5)
            finally:
                await peer.close()


async def start(client, key):
    return await client.post(HOSTED_MCP_PATH + "/" + key + "/start", headers={"Origin": ORIGIN})


def observe_entry(monkeypatch, owner, name):
    """Signal actual lifecycle entry without replacing the owned operation."""
    entered = asyncio.Event()
    original = getattr(owner, name)

    async def run(*args, **kwargs):
        entered.set()
        return await original(*args, **kwargs)

    monkeypatch.setattr(owner, name, run)
    return entered


async def test_original_login_pipe_core_route_and_callback_receipt(tmp_path):
    async with boundary(tmp_path) as (front, client, peer, key, contexts):
        listing = await client.get(HOSTED_MCP_PATH)
        assert listing.status_code == 200 and TARGET.server_name in listing.text
        assert TARGET.server_url not in listing.text and TARGET.oauth_account not in listing.text
        assert peer.actions() == ["enroll_console_login"]
        response = await start(client, key)
        assert response.status_code == 303 and response.headers["location"].startswith("https://provider.example.test/")
        assert response.headers["cache-control"] == "no-store"
        login = next(iter(front._logins.values()))
        attempt = login.mcp_attempts[key]
        assert attempt.context is contexts[0] is login.context
        assert attempt.client._client is peer.client
        assert attempt.client._login is login.console_login._enrollment
        assert peer.actions() == ["enroll_console_login", "describe", "start"]

        # Public provider navigation does not inspect any login or pass queries
        # to native. The SDK's own hash-CSP landing performs the later POST.
        before = list(peer.calls)
        original = front._live_login
        front._live_login = lambda _: pytest.fail("Public callback GET inspected login")
        try:
            landing = await client.get(REDIRECT + "?" + urlencode({"code": CODE, "state": STATE}),
                                       headers={"Origin": "https://provider.example.test"})
        finally:
            front._live_login = original
        assert landing.status_code == 200 and CODE not in landing.text and STATE not in landing.text
        assert "sha256-" in landing.headers["content-security-policy"]
        assert landing.headers["referrer-policy"] == "no-referrer" and "set-cookie" not in landing.headers
        assert peer.calls == before
        accepted = await client.post(REDIRECT, headers={"Origin": ORIGIN},
                                    json={"query": urlencode({"code": CODE, "state": STATE})})
        assert accepted.status_code == 202
        assert "authenticated" not in accepted.text
        result = await asyncio.wait_for(attempt.binding.wait(contexts[0]), 3)
        assert result.phase == "authorized"  # The inert peer's credential observation only.
        assert (await client.post(REDIRECT, headers={"Origin": ORIGIN},
            json={"query": urlencode({"code": CODE, "state": STATE})})).status_code == 400
        observed = await client.get(HOSTED_MCP_PATH + "/" + key)
        assert observed.json() == {"phase": "authorized"}
        assert (await client.post("/logout", headers={"Origin": ORIGIN})).status_code == 303
        assert peer.actions()[-3:] == ["cancel", "release", "retire_console_login"]


@pytest.mark.parametrize("action", ["describe", "start"])
async def test_cancelled_http_waiter_is_retained_and_logout_joins_before_enrollment(tmp_path, monkeypatch, action):
    async with boundary(tmp_path) as (front, client, peer, key, contexts):
        peer.block_action = action
        pending = asyncio.create_task(start(client, key))
        await asyncio.wait_for(peer.entered.wait(), 3)
        pending.cancel()
        with pytest.raises(asyncio.CancelledError):
            await pending
        assert len(next(iter(front._logins.values())).mcp_attempts) == 1
        retiring = observe_entry(monkeypatch, front, "_close_mcp")
        logout = asyncio.create_task(client.post("/logout", headers={"Origin": ORIGIN}))
        await asyncio.wait_for(retiring.wait(), 3)
        assert not logout.done() and "retire_console_login" not in peer.actions()
        assert contexts[0].is_current() is False
        peer.release.set()
        assert (await asyncio.wait_for(logout, 3)).status_code == 303
        assert peer.actions().count(action) == 1
        if action == "start":
            assert peer.actions()[-3:] == ["cancel", "release", "retire_console_login"]
        else:
            assert "start" not in peer.actions()


async def test_second_login_cannot_complete_first_logins_attempt(tmp_path):
    async with boundary(tmp_path) as (front, client, peer, key, contexts):
        assert (await start(client, key)).status_code == 303
        await sign_in(client, front)
        response = await client.post(REDIRECT, headers={"Origin": ORIGIN},
            json={"query": urlencode({"code": CODE, "state": STATE})})
        assert response.status_code == 403
        assert "complete" not in peer.actions()


@pytest.mark.parametrize("case", ["foreign_origin", "foreign_host", "missing_origin", "query",
    "oversize", "duplicate_json", "target_json", "wrong_state", "content_type"])
async def test_callback_input_refusals_do_not_complete_native(tmp_path, case):
    async with boundary(tmp_path) as (front, client, peer, key, contexts):
        assert (await start(client, key)).status_code == 303
        headers = {"Origin": ORIGIN, "Content-Type": "application/json"}
        url = REDIRECT
        body = json.dumps({"query": urlencode({"code": CODE, "state": STATE})})
        if case == "foreign_origin": headers["Origin"] = "https://foreign.example.test"
        elif case == "foreign_host": headers["Host"] = "foreign.example.test"
        elif case == "missing_origin": headers.pop("Origin")
        elif case == "content_type": headers["Content-Type"] = "text/plain"
        elif case == "query": url += "?code=not-forwarded"
        elif case == "oversize": body = "x" * 65537
        elif case == "duplicate_json": body = '{"query":"one","query":"two"}'
        elif case == "target_json": body = json.dumps({"query": "x", "target": TARGET._wire()})
        elif case == "wrong_state": body = json.dumps({"query": urlencode({"code": CODE, "state": "wrong"})})
        response = await client.post(url, headers=headers, content=body)
        assert response.status_code in (400, 403)
        assert "complete" not in peer.actions() and CODE not in response.text


@pytest.mark.parametrize("redirect", ["https://foreign.example.test/callback", ORIGIN + "/login",
    ORIGIN + "/console/rpc", ORIGIN + "/native-auth/%73erver", ORIGIN + "/native-auth/caf\u00e9"])
async def test_unmountable_native_redirect_never_starts_or_presents(tmp_path, redirect):
    async with boundary(tmp_path) as (front, client, peer, key, contexts):
        peer.redirect = redirect
        response = await start(client, key)
        assert response.status_code in (400, 503) and "location" not in response.headers
        assert "start" not in peer.actions()


@pytest.mark.parametrize("case", ["application", "origin", "logout", "encoded_path"])
async def test_callback_rechecks_current_owner_and_exact_address(tmp_path, case):
    async with boundary(tmp_path) as (front, client, peer, key, contexts):
        assert (await start(client, key)).status_code == 303
        if case == "application":
            next(iter(front._logins.values())).console_login._application_current = lambda: False
        elif case == "origin":
            front.origin = "https://changed.example.test"
        elif case == "logout":
            assert (await client.post("/logout", headers={"Origin": ORIGIN})).status_code == 303
        url = REDIRECT if case != "encoded_path" else REDIRECT.replace("server", "%73erver")
        response = await client.post(url, headers={"Origin": ORIGIN},
            json={"query": urlencode({"state": STATE, "code": CODE})})
        assert response.status_code in (400, 401, 403, 503)
        assert response.headers["cache-control"] == "no-store"
        assert "complete" not in peer.actions()


async def test_target_and_callback_selection_cannot_be_submitted_in_start_body(tmp_path):
    async with boundary(tmp_path) as (front, client, peer, key, contexts):
        response = await client.post(HOSTED_MCP_PATH + "/" + key + "/start", headers={"Origin": ORIGIN},
            json={"target": TARGET._wire(), "redirect_uri": REDIRECT})
        assert response.status_code == 400
        assert peer.actions() == ["enroll_console_login"]
        assert next(iter(front._logins.values())).mcp_attempts == {}


async def test_credential_status_describes_without_starting_and_reuses_original_binding(tmp_path):
    async with boundary(tmp_path) as (front, client, peer, key, contexts):
        response = await client.get(HOSTED_MCP_PATH + "/" + key)
        assert response.json() == {"phase": "authorization_required"}
        assert peer.actions() == ["enroll_console_login", "describe", "status"]
        binding = next(iter(front._logins.values())).mcp_attempts[key].binding
        assert (await start(client, key)).status_code == 303
        assert next(iter(front._logins.values())).mcp_attempts[key].binding is binding
        assert peer.actions().count("describe") == 1 and peer.actions().count("start") == 1


async def test_joined_attempt_has_no_presentation_or_cancellation_authority(tmp_path):
    async with boundary(tmp_path) as (front, client, peer, key, contexts):
        peer.joined = True
        response = await start(client, key)
        assert response.status_code == 202 and response.json() == {"phase": "joined"}
        assert "location" not in response.headers and STATE not in response.text
        assert (await client.post("/logout", headers={"Origin": ORIGIN})).status_code == 303
        assert peer.actions() == ["enroll_console_login", "describe", "start", "retire_console_login"]


async def test_native_failure_tokens_survive_without_retrying_describe(tmp_path):
    async with boundary(tmp_path) as (front, client, peer, key, contexts):
        peer.describe_failure = {"reason": "invalid_target", "detail": "remote_custody_required"}
        for _ in range(2):
            response = await start(client, key)
            assert response.status_code == 503
            assert response.json() == {"error": "mcp_sign_in_unavailable",
                "native_failure": peer.describe_failure}
        assert peer.actions() == ["enroll_console_login", "describe"]


async def test_cancelled_logout_still_joins_original_attempt_and_keeps_transport_owned_by_runtime(tmp_path, monkeypatch):
    async with boundary(tmp_path) as (front, client, peer, key, contexts):
        peer.block_action = "start"
        pending = asyncio.create_task(start(client, key))
        await asyncio.wait_for(peer.entered.wait(), 3)
        pending.cancel()
        await asyncio.gather(pending, return_exceptions=True)
        retiring = observe_entry(monkeypatch, front, "_close_mcp")
        logout = asyncio.create_task(client.post("/logout", headers={"Origin": ORIGIN}))
        await asyncio.wait_for(retiring.wait(), 3)
        assert contexts[0].is_current() is False
        logout.cancel()
        await asyncio.gather(logout, return_exceptions=True)
        closing = observe_entry(monkeypatch, front, "_close_resources")
        shutdown = asyncio.create_task(front.close())
        await asyncio.wait_for(closing.wait(), 3)
        assert not shutdown.done()
        peer.release.set()
        await asyncio.wait_for(shutdown, 3)
        assert peer.actions() == ["enroll_console_login", "describe", "start", "cancel", "release", "retire_console_login"]
        assert not peer.task.done()  # Only the enclosing runtime/test closes the shared pipe.


async def test_failed_presentation_joins_owned_native_cleanup_before_retirement(tmp_path):
    async with boundary(tmp_path) as (front, client, peer, key, contexts):
        peer.start_case = "invalid_presentation"
        response = await start(client, key)
        assert response.status_code == 503 and "location" not in response.headers
        assert (await client.post("/logout", headers={"Origin": ORIGIN})).status_code == 303
        assert peer.actions() == ["enroll_console_login", "describe", "start", "cancel", "release", "retire_console_login"]
        await front.close()
        assert not peer.task.done()


@pytest.mark.parametrize("action", ["cancel", "release"])
async def test_cancelled_close_request_retains_cleanup_and_logout_joins_it(tmp_path, monkeypatch, action):
    async with boundary(tmp_path) as (front, client, peer, key, contexts):
        assert (await start(client, key)).status_code == 303
        attempt = next(iter(front._logins.values())).mcp_attempts[key]
        peer.block_action = action
        closing = asyncio.create_task(client.post(HOSTED_MCP_PATH + "/" + key + "/close",
                                                  headers={"Origin": ORIGIN}))
        await asyncio.wait_for(peer.entered.wait(), 3)
        closing.cancel()
        with pytest.raises(asyncio.CancelledError):
            await closing
        original_cleanup = attempt._close_task
        assert original_cleanup is not None and not original_cleanup.done()

        retiring = observe_entry(monkeypatch, front, "_close_mcp")
        logout = asyncio.create_task(client.post("/logout", headers={"Origin": ORIGIN}))
        await asyncio.wait_for(retiring.wait(), 3)
        assert contexts[0].is_current() is False
        assert not logout.done() and "retire_console_login" not in peer.actions()
        callback = await client.post(REDIRECT, headers={"Origin": ORIGIN},
            json={"query": urlencode({"code": CODE, "state": STATE})})
        assert callback.status_code == 401 and "complete" not in peer.actions()

        peer.release.set()
        assert (await asyncio.wait_for(logout, 3)).status_code == 303
        await attempt.close()
        await front.close()
        assert attempt._close_task is original_cleanup and original_cleanup.done()
        assert peer.actions() == ["enroll_console_login", "describe", "start", "cancel", "release", "retire_console_login"]
        assert not peer.task.done()


async def test_logout_keeps_pending_completion_uncertain_and_fences_browser(tmp_path, monkeypatch):
    async with boundary(tmp_path, cleanup_error="Console access cleanup is unconfirmed") as (front, client, peer, key, contexts):
        assert (await start(client, key)).status_code == 303
        attempt = next(iter(front._logins.values())).mcp_attempts[key]
        peer.block_action = "complete"
        accepted = await client.post(REDIRECT, headers={"Origin": ORIGIN},
            json={"query": urlencode({"code": CODE, "state": STATE})})
        assert accepted.status_code == 202
        await asyncio.wait_for(peer.entered.wait(), 3)
        retiring = observe_entry(monkeypatch, front, "_close_mcp")
        cancelling = observe_entry(monkeypatch, attempt.client, "close_owned_hosted_login")
        logout = asyncio.create_task(client.post("/logout", headers={"Origin": ORIGIN}))
        await asyncio.wait_for(retiring.wait(), 3)
        assert contexts[0].is_current() is False and not logout.done()
        assert "retire_console_login" not in peer.actions()
        assert (await client.get(HOSTED_MCP_PATH + "/" + key)).status_code == 401
        # Let SDK cleanup claim cancellation before the late completion reply.
        # The peer then completes, so cancel=False cannot confirm cancellation.
        await asyncio.wait_for(cancelling.wait(), 3)
        peer.release.set()
        assert (await asyncio.wait_for(logout, 3)).status_code == 503
        actions = peer.actions()
        assert not peer.task.done()
    assert actions == ["enroll_console_login", "describe", "start", "complete", "cancel", "release", "retire_console_login"]
    assert peer.actions() == actions


@pytest.mark.parametrize("reason", ["invalid_target", "infrastructure", "refused"])
async def test_typed_start_error_does_not_prove_native_nonadmission(tmp_path, reason):
    async with boundary(tmp_path, cleanup_error="Console access cleanup is unconfirmed") as (front, client, peer, key, contexts):
        peer.start_case = reason
        response = await start(client, key)
        logout = await client.post("/logout", headers={"Origin": ORIGIN})
        actions = peer.actions()
    assert response.status_code == 503 and "location" not in response.headers
    assert logout.status_code == 503
    # Retirement is attempted as a backstop; its success is not established by
    # the fixture. Its synthetic acknowledgment cannot make MCP cleanup clean.
    assert actions == ["enroll_console_login", "describe", "start", "retire_console_login"]
    assert peer.actions() == actions


async def test_lost_native_start_reply_keeps_cleanup_unconfirmed(tmp_path):
    async with boundary(tmp_path, cleanup_error="Console access cleanup is unconfirmed") as (front, client, peer, key, contexts):
        listing = await client.get(HOSTED_MCP_PATH + "/" + key)
        attempt = next(iter(front._logins.values())).mcp_attempts[key]
        attempt.client._timeout = 0.1  # Bound this lost-reply fixture's actual SDK call.
        peer.start_case = "lost_reply"
        response = await start(client, key)
        logout = await client.post("/logout", headers={"Origin": ORIGIN})
        actions = peer.actions()
    assert listing.status_code == 200
    assert response.status_code == 503 and "location" not in response.headers
    assert logout.status_code == 503
    # The initial detail GET observes status before the lost start reply.
    # Retirement is only a backstop attempt, not native success evidence.
    assert actions == ["enroll_console_login", "describe", "status", "start", "retire_console_login"]
    assert peer.actions() == actions


async def test_cancelled_describe_waiter_keeps_same_live_attempt_for_next_request(tmp_path):
    async with boundary(tmp_path) as (front, client, peer, key, contexts):
        peer.block_action = "describe"
        pending = asyncio.create_task(start(client, key))
        await asyncio.wait_for(peer.entered.wait(), 3)
        attempt = next(iter(front._logins.values())).mcp_attempts[key]
        pending.cancel()
        with pytest.raises(asyncio.CancelledError):
            await pending
        peer.release.set()
        assert (await start(client, key)).status_code == 303
        assert next(iter(front._logins.values())).mcp_attempts[key] is attempt
        assert peer.actions() == ["enroll_console_login", "describe", "start"]


@pytest.mark.parametrize("case", ["ambiguous", "changed_route"])
async def test_mount_rejects_ambiguous_login_or_changed_retained_route(tmp_path, case):
    from types import SimpleNamespace
    async with boundary(tmp_path) as (front, client, peer, key, contexts):
        assert (await client.get(HOSTED_MCP_PATH + "/" + key)).status_code == 200
        login = next(iter(front._logins.values()))
        attempt = login.mcp_attempts[key]
        if case == "ambiguous":
            login.mcp_attempts["other"] = SimpleNamespace(client=attempt.client)
        else:
            front._mcp_routes["/native-auth/server"] = (ORIGIN + "/prior-route", attempt.binding)
        try:
            with pytest.raises(ValueError, match="ambiguous|changed its selected route"):
                front._mount_mcp(login, attempt, REDIRECT)
        finally:
            login.mcp_attempts.pop("other", None)
        assert "start" not in peer.actions()


async def test_public_landing_rejects_query_over_64_kib_without_native_effect(tmp_path):
    from starlette.requests import Request
    async with boundary(tmp_path) as (front, client, peer, key, contexts):
        assert (await start(client, key)).status_code == 303
        before = list(peer.calls)
        request = Request({"type": "http", "method": "GET", "scheme": "https",
            "server": ("console.example.test", 443), "path": "/native-auth/server",
            "raw_path": b"/native-auth/server", "query_string": b"x" * 65537,
            "headers": [(b"host", b"console.example.test")]})
        assert (await front.handle(request)).status_code == 413
        assert peer.calls == before


async def test_mutated_configured_target_returns_conflict_before_native_effect(tmp_path):
    async with boundary(tmp_path) as (front, client, peer, key, contexts):
        front._mcp_targets = {key: auth.McpTarget("Changed service", "https://changed.example.test/mcp")}
        assert (await start(client, key)).status_code == 409
        assert peer.actions() == ["enroll_console_login"]


async def test_configured_targets_require_https_console(tmp_path):
    with pytest.raises(ValueError, match="HTTPS"):
        ConsoleFront(tmp_path, 8855, "owner", PASSWORD, hosted_mcp_targets=(TARGET,))
