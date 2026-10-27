"""Named Toolkit credential carrier controls with inert HTTP/enrollment owners.

These tests qualify Forge's authenticated forwarding boundary only. Native
retained approvals, declared slots, credential use and browser outcomes remain
with Toolkit; no provider, real credential, runtime or native CLI is started.
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

PATH = "/host/toolkit/setup/credentials"
LIMIT = 64 * 1024
SENTINEL = "SYNTHETIC_CREDENTIAL_CARRIER_ONLY"
PAYLOAD = {"review_id": 41, "approval_id": "synthetic-approval", "decision": "approve",
           "credentials": [{"slot": "SYNTHETIC_TOKEN", "value": SENTINEL}]}
FORWARDED = b'{"state":"forwarded"}'


@pytest.fixture(autouse=True)
def credential_log_capture(caplog):
    # Leakage checks must also observe lower-severity transport diagnostics.
    caplog.set_level(logging.DEBUG)


def ready(owner):
    owner.release.set()


def json_bytes(payload):
    return json.dumps(payload, ensure_ascii=False, separators=(",", ":")).encode("utf-8")


def post_headers(front):
    return {"Origin": front.origin, "Content-Type": "application/json"}


@pytest.mark.parametrize("decision", ["approve", "deny"])
async def test_credentials_forward_exact_ceiling_bytes_under_original_enrolled_login(tmp_path, monkeypatch, decision, caplog):
    def native(request):
        return httpx.Response(202, content=FORWARDED, headers={"content-type": "application/json",
            "Location": "https://unselected.invalid/private", "Set-Cookie": "native-private=synthetic",
            "Authorization": "Bearer native-synthetic"})
    async with boundary(tmp_path, monkeypatch, configure=ready, upstream=native) as (
            client, front, owners, contexts, forwarded, minted, tasks):
        async def no_application_input(*args):
            raise AssertionError("Credential carrier must not inspect or deliver application input.")
        monkeypatch.setattr(front, "_submission", no_application_input)
        await login(client, front)
        session = client.cookies[front.cookie]
        actual_login = front._logins[session]
        payload = deepcopy(PAYLOAD)
        if decision == "deny":
            payload.update(decision="deny", credentials=[])
        raw = json_bytes(payload)
        raw += b" " * (LIMIT - len(raw))
        headers = {**post_headers(front), "Authorization": "Bearer browser-synthetic"}
        response = await client.post(PATH, content=raw, headers=headers)
        assert response.status_code == 202 and response.content == FORWARDED
        assert len(forwarded) == len(minted) == len(owners) == len(contexts) == 1
        request = forwarded[0]
        assert request.method == "POST" and request.url.path == PATH and not request.url.query
        assert request.url.host == "127.0.0.1" and request.url.port == 61347
        assert request.content == raw and len(request.content) == LIMIT
        assert request.headers["origin"] == front.origin
        assert request.headers.get("cookie") is None and "browser-synthetic" not in request.headers["authorization"]
        claims = jwt.decode(request.headers["authorization"].removeprefix("Bearer "), front.key,
                            algorithms=["HS256"], issuer=ISSUER, audience=AUDIENCE)
        assert claims["sub"] == claims["email"] == front.principal == contexts[0].principal
        assert claims["sid"] == actual_login.login_scope_id == owners[0].login_scope_id
        assert claims["host_scope"] == "native-console"
        assert owners[0].start_calls == 1 and owners[0].task.result() is owners[0].original
        assert response.headers["cache-control"] == "no-store"
        assert response.headers["referrer-policy"] == "no-referrer"
        assert response.headers["content-security-policy"] == "frame-ancestors 'none'"
        assert not {"authorization", "location", "set-cookie"} & set(response.headers)
        assert SENTINEL not in response.text and SENTINEL not in caplog.text
        # Native's forwarded response is passed through, without an invented
        # persisted approval, mutation result or credential echo from Forge.
        assert response.json() == {"state": "forwarded"}


async def test_credentials_require_human_same_origin_and_exact_named_route(tmp_path, monkeypatch, caplog):
    async with boundary(tmp_path, monkeypatch, configure=ready) as (
            client, front, owners, contexts, forwarded, minted, tasks):
        headers = post_headers(front)
        assert (await client.post(PATH, json=PAYLOAD, headers=headers)).status_code == 401
        assert (await client.post(PATH, json=PAYLOAD, headers={**headers,
            "Authorization": "Bearer browser-synthetic"})).status_code == 401
        await login(client, front)
        for method in ("GET", "HEAD", "PUT", "PATCH", "DELETE", "OPTIONS"):
            result = await client.request(method, PATH, headers={"Origin": front.origin})
            assert result.status_code == 405 and result.headers["allow"] == "POST"
        assert (await client.post(PATH, json=PAYLOAD)).status_code == 403
        assert (await client.post(PATH, json=PAYLOAD, headers={**headers,
            "Origin": "https://foreign.invalid"})).status_code == 403
        assert (await client.post(PATH, json=PAYLOAD, headers={**headers,
            "Host": "foreign.invalid"})).status_code == 400
        for suffix in ("?review_id=41", "?auth_token=synthetic", "?x=1&x=2"):
            assert (await client.post(PATH + suffix, json=PAYLOAD, headers=headers)).status_code == 400
        assert (await client.post(PATH + "/extra", json=PAYLOAD, headers=headers)).status_code == 404
        assert (await client.post(PATH, content=json_bytes(PAYLOAD), headers={
            "Origin": front.origin, "Content-Type": "text/plain"})).status_code == 400
        assert forwarded == minted == [] and SENTINEL not in caplog.text


async def test_credentials_refuse_oversized_body_before_native_forwarding(tmp_path, monkeypatch, caplog):
    async with boundary(tmp_path, monkeypatch, configure=ready) as (
            client, front, owners, contexts, forwarded, minted, tasks):
        await login(client, front)
        raw = json_bytes(PAYLOAD)
        raw += b" " * (LIMIT + 1 - len(raw))
        response = await client.post(PATH, content=raw, headers=post_headers(front))
        assert response.status_code == 400
        assert SENTINEL not in response.text and SENTINEL not in caplog.text
        assert forwarded == minted == []


async def test_credentials_refuse_malformed_closed_nested_carriers_without_echo(tmp_path, monkeypatch, caplog):
    invalid = [
        b'[]', b'null', b'not-json', b'[' * 1500 + b'0' + b']' * 1500,
        json_bytes({**PAYLOAD, "principal": "browser"}),
        json_bytes({key: value for key, value in PAYLOAD.items() if key != "credentials"}),
        *[json_bytes({**PAYLOAD, "review_id": value}) for value in (True, 41.0, 2**53, "41")],
        json_bytes({**PAYLOAD, "approval_id": ["synthetic-approval"]}),
        json_bytes({**PAYLOAD, "approval_id": "a" * 513}),
        json_bytes({**PAYLOAD, "approval_id": chr(197) * 257}),
        json_bytes({**PAYLOAD, "decision": "browser-approved"}),
        *[json_bytes({**PAYLOAD, "credentials": value}) for value in (None, {}, "not-an-array", [None], [[{}]])],
        json_bytes({**PAYLOAD, "credentials": [{"slot": "SYNTHETIC_TOKEN"}]}),
        json_bytes({**PAYLOAD, "credentials": [{"value": SENTINEL}]}),
        json_bytes({**PAYLOAD, "credentials": [{"slot": "SYNTHETIC_TOKEN", "value": SENTINEL, "owner": "browser"}]}),
        json_bytes({**PAYLOAD, "credentials": [{"slot": 7, "value": SENTINEL}]}),
        json_bytes({**PAYLOAD, "credentials": [{"slot": "SYNTHETIC_TOKEN", "value": [SENTINEL]}]}),
        b'{"review_id":41,"review_id":42,"approval_id":"synthetic","decision":"deny","credentials":[]}',
        b'{"review_id":41,"approval_id":"synthetic","decision":"deny","credentials":[],"credentials":[]}',
        b'{"review_id":41,"approval_id":"synthetic","decision":"approve","credentials":[{"slot":"one","slot":"two","value":"synthetic"}]}',
        b'{"review_id":41,"approval_id":"synthetic","decision":"approve","credentials":[{"slot":"one","value":"synthetic","value":"second"}]}',
    ]
    async with boundary(tmp_path, monkeypatch, configure=ready) as (
            client, front, owners, contexts, forwarded, minted, tasks):
        await login(client, front)
        for raw in invalid:
            response = await client.post(PATH, content=raw, headers=post_headers(front))
            assert response.status_code == 400
            assert SENTINEL not in response.text
        assert forwarded == minted == [] and SENTINEL not in caplog.text


@pytest.mark.parametrize("native_invalid", ["undeclared-slot", "duplicate-slot", "incomplete-approve", "nonempty-deny"])
async def test_credentials_leave_retained_slot_and_decision_semantics_with_native(tmp_path, monkeypatch, native_invalid, caplog):
    def native(request):
        return httpx.Response(409, content=b'{"native":"retained-credential-refusal"}',
                              headers={"content-type": "application/json"})
    async with boundary(tmp_path, monkeypatch, configure=ready, upstream=native) as (
            client, front, owners, contexts, forwarded, minted, tasks):
        await login(client, front)
        payload = deepcopy(PAYLOAD)
        if native_invalid == "undeclared-slot":
            payload["credentials"][0]["slot"] = "UNDECLARED_SYNTHETIC_SLOT"
        elif native_invalid == "duplicate-slot":
            payload["credentials"].append(deepcopy(payload["credentials"][0]))
        elif native_invalid == "incomplete-approve":
            payload["credentials"] = []
        else:
            payload["decision"] = "deny"
        raw = json_bytes(payload)
        response = await client.post(PATH, content=raw, headers=post_headers(front))
        assert response.status_code == 409 and response.json() == {"native": "retained-credential-refusal"}
        assert len(forwarded) == len(minted) == 1 and forwarded[0].content == raw
        assert forwarded[0].url.path == PATH
        assert SENTINEL not in response.text and SENTINEL not in caplog.text


@pytest.mark.parametrize("withdrawal", ["logout", "source-owner"])
async def test_credentials_wait_for_original_enrollment_and_refuse_late_currentness_loss(tmp_path, monkeypatch, withdrawal):
    async with boundary(tmp_path, monkeypatch) as (
            client, front, owners, contexts, forwarded, minted, tasks):
        await login(client, front)
        owner = owners[0]
        pending = asyncio.create_task(client.post(PATH, json=PAYLOAD, headers=post_headers(front)))
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
        assert SENTINEL not in response.text and forwarded == minted == []
        assert owner.start_calls == 1 and owner.task.result() is owner.original
        if withdrawal == "logout":
            assert (await asyncio.wait_for(retiring, 1)).status_code == 303


@pytest.mark.parametrize("phase", ["inbound", "native-body", "native-cleanup"])
@pytest.mark.parametrize("withdrawal", ["logout", "source-owner"])
async def test_credentials_recheck_original_login_after_suspended_body_or_native_cleanup(tmp_path, monkeypatch, phase, withdrawal):
    waiting, release = asyncio.Event(), asyncio.Event()
    cleaned = asyncio.Event()
    class NativeBody(httpx.AsyncByteStream):
        async def __aiter__(self):
            if phase == "native-body":
                waiting.set()
                await release.wait()
            yield FORWARDED
        async def aclose(self):
            if phase == "native-cleanup":
                waiting.set()
                await release.wait()
            cleaned.set()
    def native(request):
        return httpx.Response(202, stream=NativeBody(), headers={"content-type": "application/json"})
    raw = json_bytes(PAYLOAD)
    async def inbound():
        yield raw[:1]
        waiting.set()
        await release.wait()
        yield raw[1:]
    async with boundary(tmp_path, monkeypatch, configure=ready, upstream=native) as (
            client, front, owners, contexts, forwarded, minted, tasks):
        await login(client, front)
        pending = asyncio.create_task(client.post(PATH, content=inbound() if phase == "inbound" else raw,
                                                 headers=post_headers(front)))
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
            assert FORWARDED not in response.content and SENTINEL not in response.text
            assert len(forwarded) == len(minted) == (0 if phase == "inbound" else 1)
            assert owners[0].start_calls == 1
            if phase != "inbound":
                assert forwarded[0].content == raw and cleaned.is_set()
        finally:
            release.set()
