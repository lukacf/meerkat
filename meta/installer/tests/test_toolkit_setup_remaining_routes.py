"""Finite shipped Toolkit panel carriers through the real enrolled HTTP front.

The native HTTP and enrollment peers are inert. Status pages, current review,
notice cancellation, one-use access offers and grant effects remain Toolkit's
owners. This module cannot qualify a native or browser completion.
"""
from __future__ import annotations

import asyncio
from copy import deepcopy
import json
import logging

import httpx
import jwt
import pytest

from launchkit.templates_v2.console_auth import AUDIENCE, ISSUER
from test_console_auth import login
from test_console_login_enrollment import boundary, entered

PREFIX = "/host/toolkit/setup/"
MAX_ID = 2**53 - 1
ROUTES = {
    "data": {"limit": 256, "id": "page_id", "body": {"page_id": 41, "direction": "refresh"},
             "status": 200, "observation": {"page_id": 42, "rows": [], "native_extension": "opaque"}},
    "cancel-sign-in": {"limit": 256, "id": "review_id", "body": {"review_id": 41},
                       "status": 202, "observation": {"state": "forwarded"}},
    "access/revoke": {"limit": 1024, "id": "offer_id", "body": {"offer_id": 41, "row_id": "a" * 64},
                      "status": 200, "observation": {"scope": "application_issued_access", "state": "controller_in_use"}},
}


@pytest.fixture(autouse=True)
def panel_route_log_capture(caplog):
    caplog.set_level(logging.DEBUG)


def ready(owner):
    owner.release.set()


def json_bytes(value):
    return json.dumps(value, separators=(",", ":")).encode("utf-8")


def headers(front):
    return {"Origin": front.origin, "Content-Type": "application/json; charset=utf-8"}


@pytest.mark.parametrize("route,direction", [("data", "refresh"), ("data", "next"),
                                             ("cancel-sign-in", None), ("access/revoke", None)])
async def test_panel_carriers_forward_ceiling_bytes_and_preserve_both_data_methods(tmp_path, monkeypatch, route, direction):
    case = ROUTES[route]
    observation = deepcopy(case["observation"])
    def native(request):
        return httpx.Response(case["status"], content=json_bytes(observation), headers={
            "Content-Type": "application/json", "Location": "https://unselected.invalid/private",
            "Authorization": "Bearer native-synthetic", "Set-Cookie": "native-private=synthetic"})
    async with boundary(tmp_path, monkeypatch, configure=ready, upstream=native) as (
            client, front, owners, contexts, forwarded, minted, tasks):
        async def no_application_input(*args):
            raise AssertionError("Panel carrier must not inspect or deliver application input.")
        monkeypatch.setattr(front, "_submission", no_application_input)
        await login(client, front)
        actual_login = front._logins[client.cookies[front.cookie]]
        payload = deepcopy(case["body"])
        payload[case["id"]] = MAX_ID
        if direction is not None:
            payload["direction"] = direction
            first = await client.get(PREFIX + route, headers={"Authorization": "Bearer browser-synthetic"})
            assert first.status_code == 200 and first.json() == observation
            assert forwarded[-1].method == "GET" and forwarded[-1].content == b""
            assert "origin" not in forwarded[-1].headers
        raw = json_bytes(payload)
        raw += b" " * (case["limit"] - len(raw))
        response = await client.post(PREFIX + route, content=raw,
            headers={**headers(front), "Authorization": "Bearer browser-synthetic"})
        assert response.status_code == case["status"] and response.json() == observation
        request = forwarded[-1]
        assert request.method == "POST" and request.url.path == PREFIX + route
        assert request.content == raw and len(raw) == case["limit"]
        assert request.headers["origin"] == front.origin
        if route == "data":
            again = await client.get(PREFIX + route)
            assert again.status_code == 200 and again.json() == observation
            assert forwarded[-1].method == "GET" and forwarded[-1].content == b""
            assert "origin" not in forwarded[-1].headers
        assert len(forwarded) == len(minted) == (3 if route == "data" else 1)
        for request in forwarded:
            assert request.url.host == "127.0.0.1" and request.url.port == 61347 and not request.url.query
            assert request.headers.get("cookie") is None
            token = request.headers["authorization"].removeprefix("Bearer ")
            assert token != "browser-synthetic"
            claims = jwt.decode(token, front.key, algorithms=["HS256"], issuer=ISSUER, audience=AUDIENCE)
            assert claims["sub"] == claims["email"] == front.principal == contexts[0].principal
            assert claims["sid"] == actual_login.login_scope_id == owners[0].login_scope_id
            assert claims["host_scope"] == "native-console"
        assert len(owners) == 1 and owners[0].start_calls == 1
        assert owners[0].task.result() is owners[0].original
        assert response.headers["cache-control"] == "no-store"
        assert response.headers["referrer-policy"] == "no-referrer"
        assert response.headers["content-security-policy"] == "frame-ancestors 'none'"
        assert not {"authorization", "location", "set-cookie"} & set(response.headers)
        # Cancel's forwarded acknowledgement is not completed cancellation;
        # revoke's controller-in-use result must not become a success claim.
        assert response.json() == observation


@pytest.mark.parametrize("route", ROUTES)
async def test_panel_carriers_require_original_human_host_origin_and_exact_transport(tmp_path, monkeypatch, route):
    case, path = ROUTES[route], PREFIX + route
    async with boundary(tmp_path, monkeypatch, configure=ready) as (
            client, front, owners, contexts, forwarded, minted, tasks):
        assert (await client.post(path, json=case["body"], headers=headers(front))).status_code == 401
        assert (await client.post(path, json=case["body"], headers={**headers(front),
            "Authorization": "Bearer browser-synthetic"})).status_code == 401
        await login(client, front)
        methods = ("HEAD", "PUT", "PATCH", "DELETE", "OPTIONS") if route == "data" else (
            "GET", "HEAD", "PUT", "PATCH", "DELETE", "OPTIONS")
        for method in methods:
            response = await client.request(method, path, headers={"Origin": front.origin})
            assert response.status_code == 405
            assert set(item.strip() for item in response.headers["allow"].split(",")) == (
                {"GET", "POST"} if route == "data" else {"POST"})
        assert (await client.post(path, json=case["body"])).status_code == 403
        assert (await client.post(path, json=case["body"], headers={**headers(front),
            "Origin": "https://foreign.invalid"})).status_code == 403
        assert (await client.post(path, json=case["body"], headers={**headers(front),
            "Host": "foreign.invalid"})).status_code == 400
        for suffix in ("?page_id=41", "?auth_token=synthetic", "?x=1&x=2"):
            assert (await client.post(path + suffix, json=case["body"], headers=headers(front))).status_code == 400
        assert (await client.post(path + "/extra", json=case["body"], headers=headers(front))).status_code == 404
        assert (await client.post(path, content=json_bytes(case["body"]), headers={
            "Origin": front.origin, "Content-Type": "text/plain"})).status_code == 400
        if route == "data":
            assert (await client.get(path + "?page_id=41")).status_code == 400
        assert forwarded == minted == []


@pytest.mark.parametrize("route", ROUTES)
async def test_panel_carriers_refuse_bounds_types_unknown_duplicate_and_nested_fields(tmp_path, monkeypatch, route, caplog):
    case, path = ROUTES[route], PREFIX + route
    payload, key = case["body"], case["id"]
    invalid = [b"[]", b"null", b"not-json", b"[" * 1500 + b"0" + b"]" * 1500,
        json_bytes({**payload, "principal": "SYNTHETIC_NESTED_CARRIER_CANARY"}),
        json_bytes({name: value for name, value in payload.items() if name != key}),
        *[json_bytes({**payload, key: value}) for value in (0, -1, True, 41.0, 2**53, "41", {}, [])]]
    raw = json_bytes(payload)
    first = b'{"' + key.encode() + b'":41'
    assert first in raw
    invalid.append(raw.replace(first, first + b',"' + key.encode() + b'":42', 1))
    if route == "data":
        invalid.extend(json_bytes({**payload, "direction": value}) for value in ("previous", True, None, [], {}))
        invalid.append(b'{"page_id":41,"direction":"refresh","direction":"next"}')
    elif route == "access/revoke":
        invalid.extend(json_bytes({**payload, "row_id": value}) for value in (
            "A" * 64, "g" * 64, "a" * 63, "a" * 65, chr(65345) * 64, True, ["a" * 64], {}))
        invalid.append(json_bytes(payload)[:-1] + b',"row_id":"' + b'b' * 64 + b'"}')
    invalid.append(raw + b" " * (case["limit"] + 1 - len(raw)))
    async with boundary(tmp_path, monkeypatch, configure=ready) as (
            client, front, owners, contexts, forwarded, minted, tasks):
        await login(client, front)
        for raw in invalid:
            response = await client.post(path, content=raw, headers=headers(front))
            assert response.status_code == 400
            assert "SYNTHETIC_NESTED_CARRIER_CANARY" not in response.text
        assert forwarded == minted == [] and "SYNTHETIC_NESTED_CARRIER_CANARY" not in caplog.text


@pytest.mark.parametrize("route", ROUTES)
async def test_panel_carriers_delegate_stale_and_refused_native_observations_once(tmp_path, monkeypatch, route):
    status = 200 if route == "access/revoke" else 409
    observation = ({"scope": "application_issued_access", "state": "refused"} if route == "access/revoke"
                   else {"error": "Protected setup is unavailable"})
    async with boundary(tmp_path, monkeypatch, configure=ready,
            upstream=lambda request: httpx.Response(status, json=observation)) as (
            client, front, owners, contexts, forwarded, minted, tasks):
        await login(client, front)
        raw = json_bytes(ROUTES[route]["body"])
        response = await client.post(PREFIX + route, content=raw, headers=headers(front))
        assert response.status_code == status and response.json() == observation
        assert len(forwarded) == len(minted) == 1 and forwarded[0].content == raw
        assert forwarded[0].url.path == PREFIX + route
        assert owners[0].start_calls == 1
        # No automatic retry, substitute review answer, grant permission or
        # cancellation completion is inferred from a native refusal.
        assert response.json() == observation


@pytest.mark.parametrize("route", ROUTES)
@pytest.mark.parametrize("withdrawal", ["logout", "source-owner"])
async def test_panel_carriers_wait_for_original_enrollment_without_borrowing_late_owner(tmp_path, monkeypatch, route, withdrawal):
    async with boundary(tmp_path, monkeypatch) as (
            client, front, owners, contexts, forwarded, minted, tasks):
        await login(client, front)
        owner = owners[0]
        pending = asyncio.create_task(client.post(PREFIX + route, json=ROUTES[route]["body"], headers=headers(front)))
        tasks.append(pending)
        await entered(owner.ready_entered)
        assert not pending.done() and forwarded == minted == []
        if withdrawal == "logout":
            retiring = asyncio.create_task(client.post("/logout", headers={"Origin": front.origin}))
            tasks.append(retiring)
            await entered(owner.revoke_called)
        else:
            owner.source_current = False
        owner.release.set()
        response = await asyncio.wait_for(pending, 1)
        assert response.status_code == (401 if withdrawal == "logout" else 503)
        assert forwarded == minted == []
        assert owner.start_calls == 1 and owner.task.result() is owner.original
        if withdrawal == "logout":
            assert (await asyncio.wait_for(retiring, 1)).status_code == 303


@pytest.mark.parametrize("route,phase,withdrawal", [
    ("data", "inbound", "logout"), ("data", "native-body", "source-owner"),
    ("cancel-sign-in", "native-body", "logout"), ("cancel-sign-in", "inbound", "source-owner"),
    ("access/revoke", "native-cleanup", "logout"), ("access/revoke", "native-body", "source-owner"),
])
async def test_panel_carriers_withhold_late_native_reply_after_original_login_loss(tmp_path, monkeypatch, route, phase, withdrawal, caplog):
    waiting, release, cleaned = asyncio.Event(), asyncio.Event(), asyncio.Event()
    private_reply = b"SYNTHETIC_LATE_PANEL_REPLY_CANARY"
    class NativeBody(httpx.AsyncByteStream):
        async def __aiter__(self):
            if phase == "native-body":
                waiting.set()
                await release.wait()
            yield private_reply
        async def aclose(self):
            if phase == "native-cleanup":
                waiting.set()
                await release.wait()
            cleaned.set()
    raw = json_bytes(ROUTES[route]["body"])
    async def inbound():
        yield raw[:1]
        waiting.set()
        await release.wait()
        yield raw[1:]
    async with boundary(tmp_path, monkeypatch, configure=ready,
            upstream=lambda request: httpx.Response(ROUTES[route]["status"], stream=NativeBody(),
                                                    headers={"Content-Type": "application/json"})) as (
            client, front, owners, contexts, forwarded, minted, tasks):
        await login(client, front)
        pending = asyncio.create_task(client.post(PREFIX + route,
            content=inbound() if phase == "inbound" else raw, headers=headers(front)))
        tasks.append(pending)
        try:
            await entered(waiting)
            assert not pending.done() and len(forwarded) == (0 if phase == "inbound" else 1)
            if withdrawal == "logout":
                assert (await client.post("/logout", headers={"Origin": front.origin})).status_code == 303
            else:
                owners[0].source_current = False
            release.set()
            response = await asyncio.wait_for(pending, 1)
            assert response.status_code == 401 and response.json() == {"error": "Console login required."}
            assert private_reply not in response.content and private_reply.decode() not in caplog.text
            assert len(forwarded) == len(minted) == (0 if phase == "inbound" else 1)
            assert owners[0].start_calls == 1
            if phase != "inbound":
                assert forwarded[0].content == raw and cleaned.is_set()
        finally:
            release.set()


async def test_public_callback_never_reuses_native_cookie_jar_or_retired_enrolled_login(tmp_path, monkeypatch, caplog):
    from launchkit.templates_v2 import console_auth
    from test_console_auth import TOOLKIT_LANDING_CSP
    native_cookie = "SYNTHETIC_NATIVE_COOKIE_CARRIER"
    landing = b"<!doctype html><title>Synthetic fixed callback landing</title>"
    def native(request):
        if request.url.path == PREFIX + "data":
            return httpx.Response(200, json={"page_id": 42, "rows": []}, headers={
                "Set-Cookie": "native_cookie=" + native_cookie + "; Path=/"})
        assert request.url.path == PREFIX + "callback" and request.method == "GET"
        return httpx.Response(200, content=landing, headers={
            "Content-Type": "text/html", "Content-Security-Policy": TOOLKIT_LANDING_CSP})
    async with boundary(tmp_path, monkeypatch, configure=ready, upstream=native) as (
            client, front, owners, contexts, forwarded, minted, tasks):
        await login(client, front)
        session = client.cookies[front.cookie]
        original_login = front._logins[session]
        response = await client.get(PREFIX + "data")
        assert response.status_code == 200 and "set-cookie" not in response.headers
        assert len(forwarded) == len(minted) == len(owners) == 1
        assert forwarded[0].headers.get("cookie") is None
        assert front.client.cookies.get("native_cookie") == native_cookie
        owner = owners[0]
        ready_calls = owner.ready_calls
        assert (await client.post("/logout", headers={"Origin": front.origin})).status_code == 303
        assert original_login.retired and owner.revoked and not front.sessions
        assert front.client.cookies.get("native_cookie") == native_cookie
        def forbidden(*args, **kwargs):
            raise AssertionError("Public callback cannot borrow a viewer or mint a native human token.")
        with monkeypatch.context() as guard:
            guard.setattr(front, "_live_login", forbidden)
            guard.setattr(console_auth, "_token", forbidden)
            callback = await client.get(PREFIX + "callback?state=synthetic-state&code=synthetic-code", headers={
                "Origin": "https://provider.invalid", "Authorization": "Bearer browser-synthetic",
                "Cookie": front.cookie + "=" + session})
        assert callback.status_code == 200 and callback.content == landing
        assert callback.headers["content-security-policy"] == TOOLKIT_LANDING_CSP
        assert callback.headers["cache-control"] == "no-store"
        assert callback.headers["referrer-policy"] == "no-referrer"
        request = forwarded[-1]
        assert len(forwarded) == 2 and len(minted) == 1
        assert request.url.host == "127.0.0.1" and request.url.port == 61347
        assert request.url.path == PREFIX + "callback" and not request.url.query and request.content == b""
        assert not {"authorization", "origin", "cookie"} & set(request.headers)
        assert len(owners) == len(contexts) == 1 and owner.start_calls == 1 and owner.ready_calls == ready_calls
        assert original_login.retired and not front.sessions
        # The request must strip any automatic Cookie without deleting the
        # shared client's retained native jar or recreating its login owner.
        assert front.client.cookies.get("native_cookie") == native_cookie
        assert native_cookie not in callback.text and native_cookie not in caplog.text
