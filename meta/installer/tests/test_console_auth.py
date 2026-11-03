import asyncio
from dataclasses import FrozenInstanceError
import html
from html.parser import HTMLParser
import json
import os
import socket
import sys
from pathlib import Path
from types import SimpleNamespace
from urllib.parse import urlencode

import httpx
import jwt
import pytest

from launchkit.templates_v2 import console_auth
from launchkit.templates_v2.console_auth import (ConsoleFront, console_configuration,
    health_headers, ISSUER, AUDIENCE, MAX_LOGIN_BODY)


# This subprocess is a wire peer only. It has no native session, presenter,
# approval/effect owner, provider or actual Toolkit executable.
CONNECT_HOST_PEER = r'''
import asyncio, json, os, sys, uuid
scope = "peer-" + uuid.uuid4().hex
prompt_id = "p:" + uuid.uuid4().hex
state = dict(available=True, can_send=True, can_stop=False, outcome_uncertain=False,
             active_send=None, last_completion=None)
pending = None
initial = None
startup_prompt = False
def emit(value):
    print(json.dumps(value), flush=True)
def reply(identity, result):
    emit(dict(jsonrpc="2.0", id=identity, result=result))
def event(value):
    emit(dict(jsonrpc="2.0", method="connect/event", params=dict(scope=scope, **value)))
async def main():
    global pending, initial, startup_prompt
    reader = asyncio.StreamReader()
    await asyncio.get_running_loop().connect_read_pipe(lambda: asyncio.StreamReaderProtocol(reader), sys.stdin)
    while True:
        line = await reader.readline()
        if not line:
            return
        value = json.loads(line)
        with open(os.environ["CONNECT_HOST_TRACE"], "a") as trace:
            trace.write(json.dumps(value) + "\n")
        identity, method, params = value["id"], value.get("method"), value.get("params", {})
        if method == "connect/initialize":
            initial = identity
            if os.environ.get("CONNECT_HOST_STARTUP_PROMPT"):
                startup_prompt = True
                emit(dict(jsonrpc="2.0", id=prompt_id, method="connect/prompt", params=dict(scope=scope,
                    prompt=dict(kind="forge_origin", permitted_origin=params["application"]["origin"]))))
                continue
            reply(identity, dict(protocol_version=1, scope=scope, session_id="peer-session",
                capabilities=dict(prompt_kinds=["forge_recovery", "forge_origin", "forge_password", "approval"], forge_configuration=True,
                                  service_preparation=False, homecore=False), state=state))
        elif method == "connect/history":
            reply(identity, dict(history=dict(messages=[dict(role="assistant", text="Canonical fixture text")],
                offset=params.get("offset", 0), total=1, hasMore=False), state=state))
        elif method == "connect/send":
            pending = identity
            state.update(can_send=False, can_stop=True,
                         active_send=dict(send_request_id=identity, cancellation_requested=False))
            event(dict(kind="state", state=state))
            emit(dict(jsonrpc="2.0", id=prompt_id, method="connect/prompt", params=dict(scope=scope,
                prompt=dict(kind="approval", approval_id="peer-approval", summary="Full fixture proposal",
                            body=dict(warning="PRIVATE_HOST_PROPOSAL_CANARY"),
                            allowed_decisions=["approve", "deny"], expires_at=None))))
        elif method is None and identity == prompt_id:
            event(dict(kind="prompt_closed", prompt_id=identity, disposition="decision_recorded"))
            if startup_prompt:
                startup_prompt = False
                reply(initial, dict(protocol_version=1, scope=scope, session_id="peer-session",
                    capabilities=dict(prompt_kinds=["forge_recovery", "forge_origin", "forge_password", "approval"],
                                      forge_configuration=True, service_preparation=False, homecore=False), state=state))
                continue
            done = pending
            pending = None
            state.update(can_send=True, can_stop=False, active_send=None,
                         last_completion=dict(send_request_id=done, completion="completed"))
            event(dict(kind="state", state=state))
            reply(done, dict(scope=scope, send_request_id=done, completion="completed", state=state))
        elif method == "connect/cancel":
            reply(identity, dict(scope=scope, send_request_id=params["send_request_id"],
                                 status="cancellation_requested" if pending == params["send_request_id"] else "not_current"))
        elif method == "connect/retire":
            while os.environ.get("CONNECT_HOST_RETIRE_RELEASE") and not os.path.exists(os.environ["CONNECT_HOST_RETIRE_RELEASE"]):
                await asyncio.sleep(0.005)
            if os.environ.get("CONNECT_HOST_RETIRE_ERROR"):
                emit(dict(jsonrpc="2.0", id=identity, error=dict(code=-32010, message="PRIVATE_RETIRE_ERROR_CANARY",
                          data=dict(code="shutdown_unconfirmed"))))
                continue
            reply(identity, dict(target=params["target"], retirement_id=params["retirement_id"],
                                 admission_closed=True, prompts_retired=True, entered_work="settling"))
        elif method == "connect/close":
            if pending:
                state.update(can_send=False, can_stop=False, active_send=None,
                             last_completion=dict(send_request_id=pending, completion="cancelled"))
                reply(pending, dict(scope=scope, send_request_id=pending, completion="cancelled", state=state))
            reply(identity, dict(target=params["target"], resources_closed=True))
            while os.environ.get("CONNECT_HOST_CLOSE_RELEASE") and not os.path.exists(os.environ["CONNECT_HOST_CLOSE_RELEASE"]):
                await asyncio.sleep(0.005)
            await asyncio.sleep(0.04)
            return
asyncio.run(main())
'''


def managed_connect_factory(tmp_path, *, release=None, application_current=None, **extra_environment):
    from launchkit.templates_v2.console_connect import ConsoleConnect
    binary = tmp_path / "connect-host-peer"
    binary.write_text("#!" + sys.executable + "\n" + CONNECT_HOST_PEER)
    binary.chmod(0o700)
    children, contexts, traces = [], [], []
    def factory(context):
        trace = tmp_path / ("wire-" + str(len(children)) + ".jsonl")
        environment = {**os.environ, "CONNECT_HOST_TRACE": str(trace), **extra_environment}
        if release:
            environment["CONNECT_HOST_RETIRE_RELEASE"] = str(release)
        bootstrap = {"protocol_version": 1,
            "application": {"application_id": "reading", "instance_id": "host-fixture-lifetime", "origin": context.origin},
            "viewer": {"principal_id": context.principal, "login_scope_id": context.login_scope_id,
                       "expires_at_ms": context.expires_at_ms},
            "realm": {"realm_id": "owned-fixture-realm", "state_root": str(tmp_path / "state"),
                      "context_root": str(tmp_path / "context"), "user_config_root": str(tmp_path / "config")}}
        child = ConsoleConnect(binary, bootstrap,
            is_current=(lambda: context.is_current() and application_current()) if application_current else context.is_current,
            environment=environment)
        contexts.append(context); children.append(child); traces.append(trace)
        return child
    return factory, children, contexts, traces


async def connect_ready(client, front):
    response = await client.post("/console/forge/connect/initialize", json={}, headers={"Origin": front.origin})
    assert response.status_code in (200, 202)
    for _ in range(6000):
        response = await client.get("/console/forge/connect/state")
        if response.status_code == 200 and response.json().get("status") == "ready":
            return response.json()
        await asyncio.sleep(0.005)
    raise AssertionError("The retained fixture initialization did not settle.")


async def connect_view(app, front, session, private=False):
    started, disconnected = asyncio.Event(), asyncio.Event()
    frames, responses = asyncio.Queue(), []
    received = False
    async def receive():
        nonlocal received
        if not received:
            received = True
            return {"type": "http.request", "body": b"", "more_body": False}
        await disconnected.wait()
        return {"type": "http.disconnect"}
    async def send(message):
        responses.append(message)
        if message["type"] == "http.response.start":
            started.set()
        elif message.get("body"):
            frames.put_nowait(json.loads(message["body"].decode().removeprefix("data: ").strip()))
    path = "/console/forge/connect/events/" + ("private" if private else "public")
    scope = {"type": "http", "asgi": {"version": "3.0", "spec_version": "2.3"},
        "http_version": "1.1", "method": "GET", "scheme": "http", "path": path,
        "raw_path": path.encode(), "query_string": b"", "root_path": "",
        "headers": [(b"host", front.origin.removeprefix("http://").encode()),
                    (b"cookie", (front.cookie + "=" + session).encode())],
        "client": ("127.0.0.1", 10002), "server": ("127.0.0.1", front.port)}
    task = asyncio.create_task(app(scope, receive, send))
    await asyncio.wait_for(started.wait(), 30)
    assert responses[0]["status"] == 200
    return task, frames, disconnected


async def test_managed_connect_defaults_refuse_host_routes_without_native_proxy(tmp_path):
    observed = []
    front = ConsoleFront(tmp_path, 8855, "reader", "local-password-123",
        transport=httpx.MockTransport(lambda request: observed.append(request) or httpx.Response(200)))
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=front.application("http://127.0.0.1:61347")),
                                     base_url=front.origin) as client:
            await login(client, front)
            response = await client.post("/console/forge/connect/initialize", json={}, headers={"Origin": front.origin})
            assert response.status_code == 501
            assert response.json() == {"error": {"code": "unsupported_presenter"}}
            assert (await client.get("/console/forge/connect/unknown")).status_code == 404
            assert observed == []
    finally:
        await front.close()


async def test_managed_connect_real_peer_retains_one_login_binding_and_same_private_prompt(tmp_path):
    factory, children, contexts, traces = managed_connect_factory(tmp_path)
    observed, tasks = [], []
    front = ConsoleFront(tmp_path, 8855, "reader", "local-password-123", connect_factory=factory,
        transport=httpx.MockTransport(lambda request: observed.append(request) or httpx.Response(200)))
    app = front.application("http://127.0.0.1:61347")
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=app), base_url=front.origin) as client:
            await login(client, front)
            session = client.cookies[front.cookie]
            await connect_ready(client, front)
            await connect_ready(client, front)
            assert len(children) == 1 and contexts[0].login_scope_id != session
            public_task, public, _ = await connect_view(app, front, session)
            private_task, private, disconnect = await connect_view(app, front, session, private=True)
            tasks.extend([public_task, private_task])
            assert (await public.get())["event"]["kind"] == "state"
            assert (await private.get())["event"]["kind"] == "state"
            sent = await client.post("/console/forge/connect/send", json={"text": "Set up my Reading channel"},
                                     headers={"Origin": front.origin})
            assert sent.status_code == 202
            request_id = sent.json()["request_id"]
            while (prompt := await asyncio.wait_for(private.get(), 1))["type"] != "prompt":
                pass
            assert prompt["prompt"]["body"] == {"warning": "PRIVATE_HOST_PROPOSAL_CANARY"}
            disconnect.set()
            await asyncio.wait_for(private_task, 1)
            task, remounted, _ = await connect_view(app, front, session, private=True)
            tasks.append(task)
            snapshot = await remounted.get()
            assert snapshot["event"]["state"]["active_send"]["send_request_id"] == request_id
            assert await remounted.get() == prompt
            history = (await client.get("/console/forge/connect/history")).json()
            assert "PRIVATE_HOST_PROPOSAL_CANARY" not in json.dumps(history)
            public_items = []
            while not public.empty():
                public_items.append(public.get_nowait())
            assert "PRIVATE_HOST_PROPOSAL_CANARY" not in json.dumps(public_items)
            response = await client.post("/console/forge/connect/answer",
                json={"prompt_id": prompt["id"], "answer": {"kind": "approval", "decision": "approve"}},
                headers={"Origin": front.origin})
            assert response.status_code == 202 and response.json() == {"status": "forwarded"}
            for _ in range(200):
                receipt = (await client.get("/console/forge/connect/result/" + request_id)).json()
                if receipt["status"] == "completed":
                    break
                await asyncio.sleep(0.005)
            assert receipt["result"]["completion"] == "completed"
            assert (await client.post("/console/forge/connect/answer",
                json={"prompt_id": prompt["id"], "answer": {"kind": "approval", "decision": "approve"}},
                headers={"Origin": front.origin})).status_code == 409
            assert observed == []
    finally:
        await front.close()
        await finish_stream_tasks(tasks)
    wire = [json.loads(row) for row in traces[0].read_text().splitlines()]
    assert sum(row.get("method") == "connect/initialize" for row in wire) == 1
    assert sum(row.get("method") == "connect/send" for row in wire) == 1
    assert sum(row.get("id") == prompt["id"] and "result" in row for row in wire) == 1
    assert children[0]._process.returncode == 0


async def test_managed_connect_startup_prompt_is_answerable_without_initialization_reply(tmp_path):
    factory, children, _, traces = managed_connect_factory(tmp_path, CONNECT_HOST_STARTUP_PROMPT="1")
    front = ConsoleFront(tmp_path, 8855, "reader", "local-password-123", connect_factory=factory)
    app, tasks = front.application("http://127.0.0.1:61347"), []
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=app), base_url=front.origin) as client:
            await login(client, front)
            response = await client.post("/console/forge/connect/initialize", json={}, headers={"Origin": front.origin})
            assert response.status_code == 202 and response.json() == {"status": "starting"}
            for _ in range(6000):
                if children[0]._prompts:
                    break
                await asyncio.sleep(0.005)
            assert (await client.get("/console/forge/connect/state")).json() == {"status": "starting"}
            task, frames, _ = await connect_view(app, front, client.cookies[front.cookie], private=True)
            tasks.append(task)
            assert (await frames.get())["event"]["kind"] == "state"
            prompt = await frames.get()
            assert prompt["prompt"] == {"kind": "forge_origin", "permitted_origin": front.origin}
            assert (await client.post("/console/forge/connect/answer",
                json={"prompt_id": prompt["id"], "answer": {"kind": "forge_origin", "origin": front.origin}},
                headers={"Origin": front.origin})).status_code == 202
            assert (await connect_ready(client, front))["status"] == "ready"
    finally:
        await front.close()
        await finish_stream_tasks(tasks)
    assert sum(json.loads(row).get("method") == "connect/initialize" for row in traces[0].read_text().splitlines()) == 1


async def test_managed_connect_two_logins_refuse_foreign_prompt_and_exact_send_controls(tmp_path):
    factory, children, contexts, traces = managed_connect_factory(tmp_path)
    front = ConsoleFront(tmp_path, 8855, "reader", "local-password-123", connect_factory=factory)
    app, tasks = front.application("http://127.0.0.1:61347"), []
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=app), base_url=front.origin) as alice, \
                   httpx.AsyncClient(transport=httpx.ASGITransport(app=app), base_url=front.origin) as bob:
            await login(alice, front); await login(bob, front)
            await connect_ready(alice, front); await connect_ready(bob, front)
            assert contexts[0].principal == contexts[1].principal
            assert contexts[0].login_scope_id != contexts[1].login_scope_id
            task, private, _ = await connect_view(app, front, alice.cookies[front.cookie], private=True)
            tasks.append(task); await private.get()
            request_id = (await alice.post("/console/forge/connect/send", json={"text": "Configure channel"},
                headers={"Origin": front.origin})).json()["request_id"]
            while (prompt := await private.get())["type"] != "prompt":
                pass
            before = traces[1].read_bytes()
            assert (await bob.post("/console/forge/connect/answer", json={"prompt_id": prompt["id"],
                "answer": {"kind": "approval", "decision": "approve"}}, headers={"Origin": front.origin})).status_code == 409
            assert (await bob.post("/console/forge/connect/cancel", json={"send_request_id": request_id},
                headers={"Origin": front.origin})).status_code == 409
            assert (await bob.get("/console/forge/connect/result/" + request_id)).status_code == 400
            assert traces[1].read_bytes() == before
            ack = (await alice.post("/console/forge/connect/cancel", json={"send_request_id": request_id},
                headers={"Origin": front.origin})).json()
            assert ack["status"] == "cancellation_requested" and "completion" not in ack
            assert (await alice.get("/console/forge/connect/result/" + request_id)).json()["status"] == "pending"
            assert (await alice.post("/logout", headers={"Origin": front.origin})).status_code == 303
            assert children[0]._process.returncode is None
            assert (await bob.get("/console/forge/connect/history")).status_code == 200
    finally:
        await front.close()
        await finish_stream_tasks(tasks)
    assert all(child._process.returncode == 0 for child in children)


@pytest.mark.parametrize("expiry", [False, True], ids=["logout", "idle-expiry"])
async def test_managed_connect_revokes_views_before_retirement_ack_and_joins_child_exit(tmp_path, monkeypatch, expiry):
    if expiry:
        monkeypatch.setattr(console_auth, "SESSION_SECONDS", 0.2)
    release, closed = tmp_path / "retire-release", tmp_path / "close-release"
    factory, children, _, traces = managed_connect_factory(tmp_path, release=release,
        CONNECT_HOST_CLOSE_RELEASE=str(closed))
    front = ConsoleFront(tmp_path, 8855, "reader", "local-password-123", connect_factory=factory)
    app, tasks, retiring = front.application("http://127.0.0.1:61347"), [], None
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=app), base_url=front.origin) as client:
            await login(client, front)
            session = next(iter(front.sessions))
            # The real server owner supports fractional expiry in this fixture;
            # HTTP cookie Max-Age itself is integer-only, so install its token.
            client.cookies.set(front.cookie, session)
            await connect_ready(client, front)
            task, frames, _ = await connect_view(app, front, session, private=True)
            tasks.append(task); await frames.get()
            if not expiry:
                retiring = asyncio.create_task(client.post("/logout", headers={"Origin": front.origin}))
            for _ in range(6000):
                if not front.sessions:
                    break
                await asyncio.sleep(0.005)
            assert not front.sessions
            await asyncio.wait_for(asyncio.gather(task, return_exceptions=True), 1)
            assert front._logins[session].retirement_task and not front._logins[session].retirement_task.done()
            assert contexts_current_false(front, session)
            assert (await client.post("/console/forge/connect/send", json={"text": "Must not enter"},
                headers={"Origin": front.origin, "Cookie": f"{front.cookie}={session}"})).status_code == 401
            release.touch()
            if retiring:
                assert (await retiring).status_code == 303
            else:
                await asyncio.wait_for(front._logins[session].retirement_task, 1)
            assert children[0]._process.returncode is None and session in front._logins
            shutdown = asyncio.create_task(front.close())
            for _ in range(6000):
                if '"connect/close"' in traces[0].read_text():
                    break
                await asyncio.sleep(0.005)
            assert not shutdown.done() and children[0]._process.returncode is None
            closed.touch()
            await asyncio.wait_for(shutdown, 1)
            assert children[0]._process.returncode == 0 and front._logins == {}
    finally:
        release.touch(); closed.touch()
        if retiring:
            await asyncio.gather(retiring, return_exceptions=True)
        await front.close()
        await finish_stream_tasks(tasks)
    wire = [json.loads(row) for row in traces[0].read_text().splitlines()]
    retire = next(row for row in wire if row.get("method") == "connect/retire")
    assert retire["params"]["reason"] == ("login_expired" if expiry else "logout")


def contexts_current_false(front, session):
    return front._login_current(front._logins[session]) is False


async def test_managed_connect_failed_retirement_stays_unconfirmed_and_private_through_close(tmp_path):
    factory, children, _, _ = managed_connect_factory(tmp_path, CONNECT_HOST_RETIRE_ERROR="1")
    front = ConsoleFront(tmp_path, 8855, "reader", "local-password-123", connect_factory=factory)
    async with httpx.AsyncClient(transport=httpx.ASGITransport(app=front.application("http://127.0.0.1:61347")),
                                 base_url=front.origin) as client:
        await login(client, front); await connect_ready(client, front)
        session = client.cookies[front.cookie]
        response = await client.post("/logout", headers={"Origin": front.origin})
        assert response.status_code == 503 and "PRIVATE_RETIRE_ERROR_CANARY" not in response.text
        assert not front.sessions and session in front._logins
        with pytest.raises(RuntimeError, match="cleanup is unconfirmed") as failure:
            await front.close()
        assert "PRIVATE_RETIRE_ERROR_CANARY" not in str(failure.value)
        assert children[0]._process.returncode == 0


@pytest.mark.parametrize("path,raw", [
    ("/initialize", '{"realm":"browser"}'),
    ("/initialize", '{"viewer":{"principal_id":"browser"}}'),
    ("/send", '{"text":"first","text":"second"}'),
    ("/send", '{"text":"unsafe","scope":"browser"}'),
    ("/answer", '{"prompt_id":"p:x","answer":{"kind":"approval","decision":"approve","actor":"browser"}}'),
    ("/send", '{"text":NaN}'),
])
async def test_managed_connect_closed_requests_never_forward_browser_authority(tmp_path, path, raw):
    factory, children, _, traces = managed_connect_factory(tmp_path)
    front = ConsoleFront(tmp_path, 8855, "reader", "local-password-123", connect_factory=factory)
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=front.application("http://127.0.0.1:61347")),
                                     base_url=front.origin) as client:
            await login(client, front); await connect_ready(client, front)
            before = traces[0].read_bytes()
            response = await client.post("/console/forge/connect" + path, content=raw,
                headers={"Origin": front.origin, "Content-Type": "application/json"})
            assert response.status_code in (400, 409)
            assert traces[0].read_bytes() == before
            assert (await client.get("/console/forge/connect/history?scope=browser")).status_code == 400
            assert (await client.get("/console/forge/connect/history?offset=0&offset=1")).status_code == 400
    finally:
        await front.close()
    assert children[0]._process.returncode == 0


async def test_managed_connect_lost_send_response_recovers_original_control_without_replay(tmp_path):
    factory, children, _, traces = managed_connect_factory(tmp_path)
    front = ConsoleFront(tmp_path, 8855, "reader", "local-password-123", connect_factory=factory)
    app, tasks = front.application("http://127.0.0.1:61347"), []
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=app), base_url=front.origin) as client:
            await login(client, front); await connect_ready(client, front)
            session = client.cookies[front.cookie]
            body = json.dumps({"text": "Configure once despite lost browser response"}).encode()
            response_entered, disconnect = asyncio.Event(), asyncio.Event()
            async def receive():
                return {"type": "http.request", "body": body, "more_body": False}
            async def lost_response(message):
                response_entered.set()
                await disconnect.wait()
                raise asyncio.CancelledError()
            scope = {"type": "http", "asgi": {"version": "3.0", "spec_version": "2.3"},
                "http_version": "1.1", "method": "POST", "scheme": "http",
                "path": "/console/forge/connect/send", "raw_path": b"/console/forge/connect/send",
                "query_string": b"", "root_path": "", "headers": [
                    (b"host", front.origin.removeprefix("http://").encode()),
                    (b"origin", front.origin.encode()), (b"content-type", b"application/json"),
                    (b"cookie", (front.cookie + "=" + session).encode())],
                "client": ("127.0.0.1", 10003), "server": ("127.0.0.1", front.port)}
            task = asyncio.create_task(app(scope, receive, lost_response)); tasks.append(task)
            await asyncio.wait_for(response_entered.wait(), 30)
            task.cancel(); await asyncio.gather(task, return_exceptions=True)
            recovered = (await client.get("/console/forge/connect/state")).json()["state"]["state"]
            request_id = recovered["active_send"]["send_request_id"]
            assert not recovered["can_send"] and recovered["can_stop"]
            assert (await client.post("/console/forge/connect/send", json={"text": "Must not repeat"},
                headers={"Origin": front.origin})).status_code == 409
            view, frames, _ = await connect_view(app, front, session, private=True); tasks.append(view)
            assert (await frames.get())["event"]["state"]["active_send"]["send_request_id"] == request_id
            while (prompt := await frames.get())["type"] != "prompt":
                pass
            assert (await client.post("/console/forge/connect/answer", json={"prompt_id": prompt["id"],
                "answer": {"kind": "approval", "decision": "approve"}}, headers={"Origin": front.origin})).status_code == 202
            for _ in range(200):
                receipt = (await client.get("/console/forge/connect/result/" + request_id)).json()
                if receipt["status"] == "completed":
                    break
                await asyncio.sleep(0.005)
            assert receipt["result"]["send_request_id"] == request_id and receipt["result"]["completion"] == "completed"
    finally:
        await front.close(); await finish_stream_tasks(tasks)
    assert sum(json.loads(row).get("method") == "connect/send" for row in traces[0].read_text().splitlines()) == 1
    assert children[0]._process.returncode == 0


async def test_managed_connect_queued_answer_body_cannot_forward_after_logout(tmp_path):
    factory, children, _, traces = managed_connect_factory(tmp_path)
    front = ConsoleFront(tmp_path, 8855, "reader", "local-password-123", connect_factory=factory)
    app, tasks = front.application("http://127.0.0.1:61347"), []
    waiting, release = asyncio.Event(), asyncio.Event()
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=app), base_url=front.origin) as client:
            await login(client, front); await connect_ready(client, front)
            task, frames, _ = await connect_view(app, front, client.cookies[front.cookie], private=True)
            tasks.append(task); await frames.get()
            await client.post("/console/forge/connect/send", json={"text": "Configure channel"}, headers={"Origin": front.origin})
            while (prompt := await frames.get())["type"] != "prompt":
                pass
            async def held_answer():
                waiting.set(); await release.wait()
                yield json.dumps({"prompt_id": prompt["id"], "answer": {"kind": "approval", "decision": "approve"}}).encode()
            answer = asyncio.create_task(client.post("/console/forge/connect/answer", content=held_answer(),
                headers={"Origin": front.origin, "Content-Type": "application/json"})); tasks.append(answer)
            await waiting.wait()
            assert (await client.post("/logout", headers={"Origin": front.origin})).status_code == 303
            assert children[0]._process.returncode is None
            release.set()
            assert (await answer).status_code == 401
    finally:
        release.set(); await front.close(); await finish_stream_tasks(tasks)
    wire = [json.loads(row) for row in traces[0].read_text().splitlines()]
    assert not any(row.get("id") == prompt["id"] and "result" in row for row in wire)
    assert not any(row.get("method") == "connect/cancel" for row in wire)
    assert children[0]._process.returncode == 0


async def test_managed_connect_health_and_changed_host_binding_cannot_admit_or_project(tmp_path):
    current = {"realm": True}
    factory, children, _, traces = managed_connect_factory(tmp_path, application_current=lambda: current["realm"])
    observed = []
    front = ConsoleFront(tmp_path, 8855, "reader", "local-password-123", connect_factory=factory,
        transport=httpx.MockTransport(lambda request: observed.append(request) or httpx.Response(200)))
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=front.application("http://127.0.0.1:61347")),
                                     base_url=front.origin) as client:
            for scope in ("host-health", "native-console"):
                token = console_auth._token(front.key, front.principal, scope=scope)
                assert (await client.post("/console/forge/connect/initialize", json={},
                    headers={"Origin": front.origin, "Authorization": "Bearer " + token})).status_code == 401
                assert (await client.get("/console/forge/connect/events/private",
                    headers={"Authorization": "Bearer " + token})).status_code == 401
            assert children == [] and observed == []
            await login(client, front); await connect_ready(client, front)
            before = traces[0].read_bytes()
            current["realm"] = False
            assert (await client.get("/console/forge/connect/history")).status_code == 409
            assert (await client.get("/console/forge/connect/events/private")).status_code == 409
            assert (await client.post("/console/forge/connect/send", json={"text": "Wrong current realm"},
                headers={"Origin": front.origin})).status_code == 409
            assert traces[0].read_bytes() == before and observed == []
    finally:
        await front.close()
    assert children[0]._process.returncode == 0

FIELDS = [
    {"key": "CONSOLE_USER_ID", "label": "Console user", "secret": False, "required": True},
    {"key": "CONSOLE_LOGIN_PASSWORD", "label": "Console password", "secret": True, "required": True},
]
ACCEPTED = {"identity": "lead:main", "interaction_id": "interaction-native-1",
            "session_id": "session-native-2", "input_frame_id": "input-native-3",
            "cursor": "42", "status": "queued", "conversation_id": "conversation-native-4"}
SUBMISSION = {"identity": "lead:main", "content": "Approve job 37 for tonight only.",
              "origin": "caller-forged-origin", "principal": "forged-human",
              "idempotency_key": "ui-submission-5", "handling_mode": "queue"}


def metadata(root):
    (root / "application").mkdir(exist_ok=True)
    (root / "application/console.json").write_text('{"authentication":"local-password"}')
    (root / "application/configuration.json").write_text(json.dumps({"fields": FIELDS}))


async def login(client, front):
    response = await client.get("/login")
    assert response.status_code == 200
    nonce = client.cookies[front.cookie + "_login"]
    response = await client.post("/login", data={"nonce": nonce, "password": "local-password-123"},
                                 headers={"Origin": front.origin})
    assert response.status_code == 303
    assert "httponly" in response.headers["set-cookie"].lower()
    assert "samesite=strict" in response.headers["set-cookie"].lower()


class LoginFormParser(HTMLParser):
    def __init__(self):
        super().__init__()
        self.forms, self.inputs, self.text, self.event_handlers = [], [], [], []
        self.scripts = self.suppressed = 0

    def handle_starttag(self, tag, attrs):
        self.event_handlers.extend(name for name, _ in attrs if name.startswith("on"))
        if tag == "form":
            self.forms.append(dict(attrs))
        elif tag == "input":
            self.inputs.append(dict(attrs))
        if tag in {"style", "script"}:
            self.suppressed += 1
        if tag == "script":
            self.scripts += 1

    def handle_endtag(self, tag):
        if tag in {"style", "script"}:
            self.suppressed -= 1

    def handle_data(self, data):
        if not self.suppressed and data.strip():
            self.text.append(data.strip())


def password_form(response, client, front, *private_values):
    assert response.headers["content-type"].startswith("text/html")
    assert response.headers["cache-control"] == "no-store"
    directives = {parts[0]: parts[1:] for item in response.headers["content-security-policy"].split(";")
                  if (parts := item.strip().split())}
    assert directives["default-src"] == ["'none'"]
    assert directives["form-action"] == ["'self'"]
    assert directives["frame-ancestors"] == ["'none'"]
    assert all(values == ["'none'"] for name, values in directives.items() if name.startswith("script-src"))
    parser = LoginFormParser()
    parser.feed(response.text)
    assert not parser.scripts and not parser.event_handlers
    assert len(parser.forms) == 1
    assert parser.forms[0].get("method", "").lower() == "post"
    assert parser.forms[0].get("action") == "/login"
    assert len(parser.inputs) == 2
    inputs = {item.get("name"): item for item in parser.inputs}
    assert set(inputs) == {"nonce", "password"}
    assert inputs["password"]["type"] == "password"
    assert inputs["password"].get("value", "") == ""
    assert inputs["nonce"]["type"] == "hidden"
    nonce = inputs["nonce"]["value"]
    assert nonce and nonce == client.cookies[front.cookie + "_login"]
    assert "httponly" in response.headers["set-cookie"].lower()
    assert "samesite=strict" in response.headers["set-cookie"].lower()
    for value in private_values:
        assert value not in html.unescape(response.text)
        assert value not in str(response.headers)
    return nonce, " ".join(parser.text)


async def test_password_only_login_recovers_wrong_password_and_refuses_consumed_nonce_replay(tmp_path):
    principal, password, wrong = "private-principal-canary@example.test", "correct-password-canary-<tag>&\"", "wrong-password-canary-<tag>&\""
    observed, delivered = [], []
    front = ConsoleFront(tmp_path, 8855, principal, password, delivered.append,
        transport=httpx.MockTransport(lambda request: observed.append(request) or httpx.Response(200, content=b"native-console")))
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=front.application("http://127.0.0.1:61347")),
                                     base_url=front.origin) as client:
            initial = await client.get("/login")
            assert initial.status_code == 200
            old_nonce, initial_text = password_form(initial, client, front, principal, password, wrong)
            failure = await client.post("/login", data={"nonce": old_nonce, "password": wrong},
                                        headers={"Origin": front.origin})
            assert failure.status_code == 401
            retry_nonce, failure_text = password_form(failure, client, front, principal, password, wrong)
            assert retry_nonce != old_nonce and failure_text != initial_text
            assert front.sessions == {} and front.cookie not in client.cookies
            assert (await client.post("/console/rpc", json={}, headers={"Origin": front.origin})).status_code == 401
            assert observed == [] and delivered == []

            # Replay the consumed nonce with its matching old cookie and a correct
            # password; a cookie mismatch alone must not account for refusal.
            replay = await client.post("/login", data={"nonce": old_nonce, "password": password},
                headers={"Origin": front.origin, "Cookie": f"{front.cookie}_login={old_nonce}"})
            assert replay.status_code == 403
            current_nonce, replay_text = password_form(replay, client, front, principal, password, wrong)
            assert current_nonce not in {old_nonce, retry_nonce} and replay_text != initial_text
            assert front.sessions == {} and observed == [] and delivered == []
            accepted = await client.post("/login", data={"nonce": current_nonce, "password": password},
                                         headers={"Origin": front.origin})
            assert accepted.status_code == 303 and accepted.headers["location"] == "/console"
            assert len(front.sessions) == 1 and front.cookie in client.cookies
            assert observed == [] and delivered == []
            assert (await client.get("/console")).content == b"native-console"
            assert len(observed) == 1 and delivered == []
    finally:
        await front.close()


async def test_expired_password_only_login_reissues_form_and_rejects_old_nonce(tmp_path, monkeypatch):
    clock = {"now": 1000.0}
    monkeypatch.setattr(console_auth, "time", SimpleNamespace(time=lambda: clock["now"]))
    principal, password = "private-principal-canary@example.test", "correct-password-canary-<tag>&\""
    observed, delivered = [], []
    front = ConsoleFront(tmp_path, 8855, principal, password, delivered.append,
        transport=httpx.MockTransport(lambda request: observed.append(request) or httpx.Response(200, content=b"native-console")))
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=front.application("http://127.0.0.1:61347")),
                                     base_url=front.origin) as client:
            initial = await client.get("/login")
            assert initial.status_code == 200
            old_nonce, initial_text = password_form(initial, client, front, principal, password)
            clock["now"] += 301
            expired = await client.post("/login", data={"nonce": old_nonce, "password": password},
                                        headers={"Origin": front.origin})
            assert expired.status_code == 403
            retry_nonce, expired_text = password_form(expired, client, front, principal, password)
            assert retry_nonce != old_nonce and expired_text != initial_text
            assert front.sessions == {} and front.cookie not in client.cookies
            assert (await client.post("/console/rpc", json={}, headers={"Origin": front.origin})).status_code == 401
            assert observed == [] and delivered == []
            replay = await client.post("/login", data={"nonce": old_nonce, "password": password},
                headers={"Origin": front.origin, "Cookie": f"{front.cookie}_login={old_nonce}"})
            assert replay.status_code == 403
            current_nonce, _ = password_form(replay, client, front, principal, password)
            assert current_nonce not in {old_nonce, retry_nonce}
            assert front.sessions == {} and observed == [] and delivered == []
            accepted = await client.post("/login", data={"nonce": current_nonce, "password": password},
                                         headers={"Origin": front.origin})
            assert accepted.status_code == 303 and accepted.headers["location"] == "/console"
            assert len(front.sessions) == 1 and observed == [] and delivered == []
            assert (await client.get("/console")).content == b"native-console"
            assert len(observed) == 1 and delivered == []
    finally:
        await front.close()


async def test_invalid_nonce_retry_preserves_one_hundred_live_token_cap(tmp_path, monkeypatch):
    monkeypatch.setattr(console_auth, "time", SimpleNamespace(time=lambda: 1000.0))
    observed, delivered = [], []
    front = ConsoleFront(tmp_path, 8855, "private-principal-canary@example.test", "correct-password-canary", delivered.append,
        transport=httpx.MockTransport(lambda request: observed.append(request) or httpx.Response(200)))
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=front.application("http://127.0.0.1:61347")),
                                     base_url=front.origin) as client:
            for _ in range(100):
                assert (await client.get("/login")).status_code == 200
            live_tokens = dict(front.login_tokens)
            assert len(live_tokens) == 100
            reply = await client.post("/login", data={"nonce": "unknown-nonce", "password": "correct-password-canary"},
                                      headers={"Origin": front.origin})
            assert reply.status_code == 429 and "set-cookie" not in reply.headers
            assert front.login_tokens == live_tokens and front.sessions == {}
            assert front.cookie not in client.cookies and observed == [] and delivered == []
            assert (await client.get("/login")).status_code == 429
            assert (await client.post("/console/rpc", json={}, headers={"Origin": front.origin})).status_code == 401
            assert front.login_tokens == live_tokens and observed == [] and delivered == []
    finally:
        await front.close()


async def test_ten_wrong_passwords_block_correct_login_until_failure_window_expires(tmp_path, monkeypatch):
    clock = {"now": 1000.0}
    monkeypatch.setattr(console_auth, "time", SimpleNamespace(time=lambda: clock["now"]))
    principal, password, wrong = "private-principal-canary@example.test", "correct-password-canary", "wrong-password-canary"
    observed, delivered = [], []
    front = ConsoleFront(tmp_path, 8855, principal, password, delivered.append,
        transport=httpx.MockTransport(lambda request: observed.append(request) or httpx.Response(200, content=b"native-console")))
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=front.application("http://127.0.0.1:61347")),
                                     base_url=front.origin) as client:
            first = await client.get("/login")
            assert first.status_code == 200
            nonce, _ = password_form(first, client, front, principal, password, wrong)
            for _ in range(10):
                failed = await client.post("/login", data={"nonce": nonce, "password": wrong},
                                           headers={"Origin": front.origin})
                assert failed.status_code == 401
                previous_nonce = nonce
                nonce, _ = password_form(failed, client, front, principal, password, wrong)
                assert nonce != previous_nonce
            refused = await client.post("/login", data={"nonce": nonce, "password": password},
                                        headers={"Origin": front.origin})
            assert refused.status_code == 429 and front.sessions == {} and front.cookie not in client.cookies
            assert observed == [] and delivered == []
            clock["now"] += 61
            refreshed = await client.get("/login")
            assert refreshed.status_code == 200
            nonce, _ = password_form(refreshed, client, front, principal, password, wrong)
            accepted = await client.post("/login", data={"nonce": nonce, "password": password},
                                         headers={"Origin": front.origin})
            assert accepted.status_code == 303 and accepted.headers["location"] == "/console"
            assert len(front.sessions) == 1 and observed == [] and delivered == []
            assert (await client.get("/console")).content == b"native-console"
            assert len(observed) == 1 and delivered == []
    finally:
        await front.close()


@pytest.mark.parametrize("password", ["\U0001f600" * 4096, "+" * 4096])
async def test_login_accepts_maximum_password_after_utf8_form_encoding(tmp_path, password):
    observed = []
    front = ConsoleFront(tmp_path, 8855, "owner", password,
        transport=httpx.MockTransport(lambda request: observed.append(request) or httpx.Response(200, content=b"native-console")))
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=front.application("http://127.0.0.1:61347")),
                                     base_url=front.origin) as client:
            assert (await client.get("/login")).status_code == 200
            body = urlencode({"nonce": client.cookies[front.cookie + "_login"], "password": password}).encode("ascii")
            assert 8192 < len(body) <= MAX_LOGIN_BODY
            response = await client.post("/login", content=body,
                headers={"Origin": front.origin, "Content-Type": "application/x-www-form-urlencoded"})
            assert response.status_code == 303 and front.cookie in client.cookies
            assert (await client.get("/console")).content == b"native-console"
            assert len(observed) == 1
    finally:
        await front.close()


async def test_login_body_remains_bounded_without_session_or_native_access(tmp_path):
    observed = []
    front = ConsoleFront(tmp_path, 8855, "owner", "local-password-123",
        transport=httpx.MockTransport(lambda request: observed.append(request) or httpx.Response(200)))
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=front.application("http://127.0.0.1:61347")),
                                     base_url=front.origin) as client:
            assert (await client.get("/login")).status_code == 200
            response = await client.post("/login", content=b"x" * (MAX_LOGIN_BODY + 1),
                headers={"Origin": front.origin, "Content-Type": "application/x-www-form-urlencoded"})
            assert response.status_code == 400 and response.text == "Invalid login form."
            assert front.sessions == {} and observed == []
    finally:
        await front.close()


def test_metadata_requires_explicit_private_fields_without_source_values(tmp_path):
    assert not console_configuration(tmp_path, [])
    metadata(tmp_path)
    assert console_configuration(tmp_path, FIELDS)
    with pytest.raises(ValueError, match="CONSOLE_LOGIN_PASSWORD"):
        console_configuration(tmp_path, FIELDS[:1])
    with pytest.raises(ValueError, match="CONSOLE_USER_ID"):
        console_configuration(tmp_path, [{**FIELDS[0], "default": "source-user"}, FIELDS[1]])
    (tmp_path / "application/console.json").write_text('{"authentication":"none"}')
    with pytest.raises(ValueError, match="local-password"):
        console_configuration(tmp_path, FIELDS)


async def test_native_console_is_unchanged_but_only_authenticated_browser_admission_has_principal(tmp_path, monkeypatch):
    clock = {"now": float(int(console_auth.time.time()) - 10)}
    monkeypatch.setattr(console_auth.time, "time", lambda: clock["now"])
    observed, delivered = [], []
    async def native(request):
        observed.append(request)
        if request.url.path == "/console/rpc":
            assert json.loads(request.content)["params"] == SUBMISSION
            return httpx.Response(200, json={"jsonrpc": "2.0", "id": "1", "result": ACCEPTED})
        return httpx.Response(200, content=b"native-console-unchanged", headers={"content-type": "text/html"})
    front = ConsoleFront(tmp_path, 8855, "owner@example.test", "local-password-123", delivered.append,
                         transport=httpx.MockTransport(native))
    app = front.application("http://127.0.0.1:61347")
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=app), base_url=front.origin) as client:
            assert (await client.get("/console")).status_code == 303
            assert (await client.post("/console/rpc", json={}, headers={"Origin": front.origin})).status_code == 401
            assert observed == [] and delivered == []
            await login(client, front)
            session = client.cookies[front.cookie]
            actual_login = front._logins[session]
            response = await client.get("/console")
            assert response.content == b"native-console-unchanged"
            assert "set-cookie" not in response.headers
            initial_token = observed[-1].headers["Authorization"][7:]
            request = {"jsonrpc": "2.0", "id": "1", "method": "mobkit/console/send", "params": SUBMISSION}
            response = await client.post("/console/rpc", json=request, headers={"Origin": front.origin})
            assert response.json()["result"] == ACCEPTED
            assert len(delivered) == 1
            event = delivered[0]
            assert event.channel == "host-authenticated-console"
            assert event.principal == "owner@example.test" and event.content == SUBMISSION["content"]
            assert event.identity == ACCEPTED["identity"] and event.session_id == ACCEPTED["session_id"]
            assert event.interaction_id == ACCEPTED["interaction_id"] and event.input_frame_id == ACCEPTED["input_frame_id"]
            assert event.cursor == "42"
            with pytest.raises(FrozenInstanceError):
                event.principal = "agent-forged"
            response = await client.post("/console/rpc", json=request, headers={"Origin": front.origin})
            assert response.status_code == 200 and len(delivered) == 1
            token = observed[-1].headers["Authorization"][7:]
            claims = jwt.decode(token, front.key, algorithms=["HS256"], issuer=ISSUER, audience=AUDIENCE)
            assert claims["sub"] == "owner@example.test" and claims["host_scope"] == "native-console"
            assert claims["sid"] == actual_login.login_scope_id and token == initial_token
            assert observed[-1].url.port == 61347
            assert "cookie" not in observed[-1].headers and "origin" not in observed[-1].headers
            clock["now"] += 1
            response = await client.get("/console")
            assert response.content == b"native-console-unchanged" and len(delivered) == 1
            rotated_token = observed[-1].headers["Authorization"][7:]
            rotated = jwt.decode(rotated_token, front.key, algorithms=["HS256"], issuer=ISSUER, audience=AUDIENCE)
            assert rotated_token != token
            assert rotated["iat"] == claims["iat"] + 1 and rotated["exp"] == claims["exp"] + 1
            assert rotated["sub"] == claims["sub"] and rotated["sid"] == actual_login.login_scope_id
            assert front._logins[session] is actual_login and not actual_login.retired
    finally:
        await front.close()


async def test_csrf_and_host_guards_precede_native_forwarding_and_logout_revokes_session(tmp_path):
    observed = []
    front = ConsoleFront(tmp_path, 8855, "owner", "local-password-123",
                         transport=httpx.MockTransport(lambda request: observed.append(request) or httpx.Response(200)))
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=front.application("http://127.0.0.1:61347")), base_url=front.origin) as client:
            await login(client, front)
            for headers in ({}, {"Origin": "https://hostile.example"}, {"Origin": "null"}):
                assert (await client.post("/console/rpc", json={}, headers=headers)).status_code == 403
            assert (await client.get("/console", headers={"Host": "hostile.example"})).status_code == 400
            assert (await client.get("/console?auth_token=caller-token")).status_code == 400
            assert observed == []
            assert (await client.post("/logout", headers={"Origin": front.origin})).status_code == 303
            assert (await client.get("/console/identities")).status_code == 401
    finally:
        await front.close()


async def test_authenticated_front_restarts_immediately_on_its_owned_port_and_revokes_old_sessions(tmp_path):
    with socket.socket() as available:
        available.bind(("127.0.0.1", 0))
        port = available.getsockname()[1]
    stale_cookies = None
    for _ in range(3):
        front = ConsoleFront(tmp_path, port, "owner", "local-password-123",
            transport=httpx.MockTransport(lambda request: httpx.Response(200, content=b"native-console")))
        try:
            await front.start("http://127.0.0.1:61347")
            async with httpx.AsyncClient(base_url=front.origin, trust_env=False,
                    timeout=3, headers={"Connection": "close"}) as client:
                if stale_cookies:
                    client.cookies.update(stale_cookies)
                # Startup and a saved browser cookie cannot establish a new human session.
                assert (await client.get("/console")).status_code == 303
                assert (await client.post("/console/rpc", json={}, headers={"Origin": front.origin})).status_code == 401
                await login(client, front)
                assert (await client.get("/console")).content == b"native-console"
                stale_cookies = httpx.Cookies(client.cookies)
            # The restarted address still cannot be claimed while this listener lives.
            with socket.socket() as competing:
                competing.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
                with pytest.raises(OSError):
                    competing.bind(("127.0.0.1", port))
        finally:
            await front.close()


async def test_private_health_credential_cannot_send_or_emit_human_event(tmp_path):
    metadata(tmp_path)
    (tmp_path / ".env").write_text('CONSOLE_USER_ID="owner@example.test"\nCONSOLE_LOGIN_PASSWORD="local-password-123"\n')
    os.chmod(tmp_path / ".env", 0o600)
    observed, delivered = [], []
    front = ConsoleFront(tmp_path, 8855, "owner@example.test", "local-password-123", delivered.append,
                         transport=httpx.MockTransport(lambda request: observed.append(request) or httpx.Response(200, json={"native": "projection"})))
    headers = health_headers(tmp_path)
    assert set(headers) == {"Authorization"}
    health_claims = jwt.decode(headers["Authorization"][7:], front.key, algorithms=["HS256"],
        issuer=ISSUER, audience=console_auth.HEALTH_AUDIENCE)
    assert health_claims["sub"] == front.principal and health_claims["host_scope"] == "host-health"
    assert "sid" not in health_claims
    # The pinned native validator ignores custom scope claims, so a health
    # credential must also be rejected by its required native audience.
    with pytest.raises(jwt.InvalidAudienceError):
        jwt.decode(headers["Authorization"][7:], front.key, algorithms=["HS256"], issuer=ISSUER, audience=AUDIENCE)
    assert (tmp_path / ".mobkit/console-signing.key").stat().st_mode & 0o777 == 0o600
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=front.application("http://127.0.0.1:61347")), base_url=front.origin) as client:
            assert (await client.get("/console/experience", headers=headers)).json() == {"native": "projection"}
            assert (await client.get("/console/identities", headers=headers)).status_code == 200
            assert (await client.get("/console/timeline", headers=headers)).status_code == 401
            assert (await client.post("/console/rpc", headers={**headers, "Origin": front.origin}, json={})).status_code == 401
            assert len(observed) == 2 and delivered == []
            assert front.sessions == {} and front._logins == {}
            for request in observed:
                claims = jwt.decode(request.headers["Authorization"][7:], front.key, algorithms=["HS256"],
                    issuer=ISSUER, audience=AUDIENCE)
                assert claims["sub"] == front.principal and claims["host_scope"] == "native-console"
                assert "sid" not in claims
    finally:
        await front.close()


@pytest.mark.parametrize("accepted,status", [(None, 200), ({**ACCEPTED, "identity": "another:main"}, 503),
                                            ({**ACCEPTED, "session_id": None}, 503)])
async def test_rejected_or_incomplete_native_admission_never_delivers_trusted_input(tmp_path, accepted, status):
    delivered = []
    result = {"error": {"code": -32001, "message": "not admitted"}} if accepted is None else {"result": accepted}
    front = ConsoleFront(tmp_path, 8855, "owner", "local-password-123", delivered.append,
                         transport=httpx.MockTransport(lambda request: httpx.Response(200, json=result)))
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=front.application("http://127.0.0.1:61347")), base_url=front.origin) as client:
            await login(client, front)
            response = await client.post("/console/rpc", json={"method": "mobkit/console/send", "params": SUBMISSION}, headers={"Origin": front.origin})
            assert response.status_code == status and delivered == []
    finally:
        await front.close()


async def test_failed_application_delivery_can_retry_same_native_admission(tmp_path):
    attempts = []
    def receive(event):
        attempts.append(event)
        if len(attempts) == 1:
            raise RuntimeError("private application failure")
    front = ConsoleFront(tmp_path, 8855, "owner", "local-password-123", receive,
                         transport=httpx.MockTransport(lambda request: httpx.Response(200, json={"result": ACCEPTED})))
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=front.application("http://127.0.0.1:61347")), base_url=front.origin) as client:
            await login(client, front)
            request = {"method": "mobkit/console/send", "params": SUBMISSION}
            failed = await client.post("/console/rpc", json=request, headers={"Origin": front.origin})
            assert failed.status_code == 503 and "private application failure" not in failed.text
            assert (await client.post("/console/rpc", json=request, headers={"Origin": front.origin})).status_code == 200
            assert (await client.post("/console/rpc", json=request, headers={"Origin": front.origin})).status_code == 200
            assert len(attempts) == 2 and attempts[0].interaction_id == attempts[1].interaction_id
    finally:
        await front.close()


async def test_native_multipart_input_preserves_submitted_text_and_native_admission(tmp_path):
    delivered, observed = [], []
    def native(request):
        observed.append(request)
        return httpx.Response(200, json={"result": ACCEPTED})
    front = ConsoleFront(tmp_path, 8855, "owner", "local-password-123", delivered.append,
                         transport=httpx.MockTransport(native))
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=front.application("http://127.0.0.1:61347")), base_url=front.origin) as client:
            await login(client, front)
            request = {"method": "mobkit/console/send", "params": SUBMISSION}
            response = await client.post("/console/rpc/multipart", files={"payload": (None, json.dumps(request)), "file:upload-1": ("sky.png", b"native-upload-bytes", "image/png")}, headers={"Origin": front.origin})
            assert response.status_code == 200
            assert delivered[0].content == SUBMISSION["content"]
            assert b"native-upload-bytes" in observed[0].content
    finally:
        await front.close()


@pytest.mark.parametrize("scope", ["native-console", "host-health"])
def test_console_tokens_always_carry_exp_and_iat(monkeypatch, scope):
    # The Console lists a principal whose token has no exp as non-expiring;
    # every token this host mints expires two minutes after it is issued.
    monkeypatch.setattr(console_auth.time, "time", lambda: 1_900_000_000.5)
    key = "0" * 64
    token = console_auth._token(key, "owner@example.test", scope=scope)
    claims = jwt.decode(token, key, algorithms=["HS256"],
                        options={"verify_exp": False, "verify_iat": False, "verify_aud": False})
    assert claims["iat"] == 1_900_000_000
    assert claims["exp"] == 1_900_000_120


def test_private_key_rejects_linked_or_public_source_and_upstream_requires_loopback(tmp_path):
    front = ConsoleFront(tmp_path, 8855, "owner", "local-password-123")
    os.chmod(tmp_path / ".mobkit/console-signing.key", 0o644)
    with pytest.raises(ValueError, match="private owned"):
        ConsoleFront(tmp_path, 8855, "owner", "local-password-123")
    with pytest.raises(ValueError, match="loopback"):
        front.application("https://remote.example")


async def test_native_sse_bytes_and_cursor_are_forwarded_without_a_shadow_event_stream(tmp_path):
    delivered, observed = [], []
    class NativeStream(httpx.AsyncByteStream):
        async def __aiter__(self):
            yield b'id: native-42\nevent: frame_added\ndata: {"native":true}\n\n'
    def native(request):
        observed.append(request)
        return httpx.Response(200, stream=NativeStream(), headers={"content-type": "text/event-stream"})
    front = ConsoleFront(tmp_path, 8855, "owner", "local-password-123", delivered.append,
                         transport=httpx.MockTransport(native))
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=front.application("http://127.0.0.1:61347")), base_url=front.origin) as client:
            await login(client, front)
            response = await client.get("/console/timeline/stream?after_cursor=41", headers={"Last-Event-ID": "native-41"})
            assert response.content == b'id: native-42\nevent: frame_added\ndata: {"native":true}\n\n'
            assert observed[0].url.query == b"after_cursor=41"
            assert observed[0].headers["Last-Event-ID"] == "native-41"
            assert observed[0].extensions["timeout"]["read"] is None
            assert delivered == []
    finally:
        await front.close()


class HeldNativeStream(httpx.AsyncByteStream):
    def __init__(self):
        self.frames = asyncio.Queue()
        self.waiting, self.closed = asyncio.Event(), asyncio.Event()

    async def __aiter__(self):
        while True:
            self.waiting.set()
            frame = await self.frames.get()
            if frame is None:
                return
            yield frame

    async def aclose(self):
        self.closed.set()


async def held_console_stream(app, front, session):
    """Exercise the actual ASGI response without buffering an endless SSE body."""
    started, disconnected = asyncio.Event(), asyncio.Event()
    frames, responses = asyncio.Queue(), []
    received = False
    async def receive():
        nonlocal received
        if not received:
            received = True
            return {"type": "http.request", "body": b"", "more_body": False}
        await disconnected.wait()
        return {"type": "http.disconnect"}
    async def send(message):
        responses.append(message)
        if message["type"] == "http.response.start":
            started.set()
        elif message.get("body"):
            frames.put_nowait(message["body"])
    # The existing Uvicorn HTTP listener supplies ASGI HTTP spec 2.3.
    scope = {"type": "http", "asgi": {"version": "3.0", "spec_version": "2.3"},
        "http_version": "1.1", "method": "GET", "scheme": "http",
        "path": "/console/timeline/stream", "raw_path": b"/console/timeline/stream",
        "query_string": b"after_cursor=41", "root_path": "",
        "headers": [(b"host", front.origin.removeprefix("http://").encode()),
                    (b"cookie", (front.cookie + "=" + session).encode()),
                    (b"last-event-id", b"native-41")],
        "client": ("127.0.0.1", 10001), "server": ("127.0.0.1", front.port)}
    task = asyncio.create_task(app(scope, receive, send))
    await asyncio.wait_for(started.wait(), 30)
    return task, frames, responses, disconnected


async def finish_stream_tasks(tasks):
    for task in tasks:
        if not task.done():
            task.cancel()
    await asyncio.gather(*tasks, return_exceptions=True)


@pytest.mark.parametrize("active", [False, True], ids=["idle", "active"])
async def test_login_retirement_logout_closes_only_its_stream_and_preserves_native_bytes(tmp_path, active):
    native_streams, observed, delivered, tasks = [], [], [], []
    def native(request):
        observed.append(request)
        stream = HeldNativeStream()
        native_streams.append(stream)
        return httpx.Response(200, stream=stream, headers={"content-type": "text/event-stream"})
    front = ConsoleFront(tmp_path, 8855, "owner", "local-password-123", delivered.append,
        transport=httpx.MockTransport(native))
    app = front.application("http://127.0.0.1:61347")
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=app), base_url=front.origin) as first, \
                   httpx.AsyncClient(transport=httpx.ASGITransport(app=app), base_url=front.origin) as second:
            await login(first, front)
            await login(second, front)
            first_session, second_session = first.cookies[front.cookie], second.cookies[front.cookie]
            assert first_session != second_session
            one, first_frames, first_responses, _ = await held_console_stream(app, front, first_session)
            tasks.append(one)
            two, second_frames, second_responses, _ = await held_console_stream(app, front, second_session)
            tasks.append(two)
            await asyncio.wait_for(native_streams[0].waiting.wait(), 30)
            await asyncio.wait_for(native_streams[1].waiting.wait(), 30)
            frame = b'id: native-42\nevent: frame_added\ndata: {"native":"unchanged"}\n\n'
            if active:
                native_streams[0].frames.put_nowait(frame)
                assert await asyncio.wait_for(first_frames.get(), 1) == frame
            response = await first.post("/logout", headers={"Origin": front.origin})
            assert response.status_code == 303
            await asyncio.wait_for(native_streams[0].closed.wait(), 0.2)
            await asyncio.wait_for(asyncio.gather(one, return_exceptions=True), 0.2)
            assert first_session not in front.sessions and second_session in front.sessions
            native_streams[0].frames.put_nowait(b'data: {"retired":"must-not-forward"}\n\n')
            native_streams[1].frames.put_nowait(frame)
            assert await asyncio.wait_for(second_frames.get(), 1) == frame
            assert first_frames.empty() and not two.done() and not native_streams[1].closed.is_set()
            assert all(request.url.query == b"after_cursor=41" and request.headers["Last-Event-ID"] == "native-41"
                       for request in observed)
            assert delivered == [] and len(observed) == 2
            assert (await first.get("/console/timeline/stream")).status_code == 401
            assert len(observed) == 2
            public = repr(first_responses + second_responses)
            assert "local-password-123" not in public and front.key not in public
            assert first_session not in public and second_session not in public
    finally:
        await finish_stream_tasks(tasks)
        await front.close()


@pytest.mark.parametrize("active", [False, True], ids=["idle", "active"])
async def test_login_retirement_expiry_closes_stream_without_another_browser_request(tmp_path, monkeypatch, active):
    monkeypatch.setattr(console_auth, "SESSION_SECONDS", 1)
    stream, observed, delivered, tasks = HeldNativeStream(), [], [], []
    def native(request):
        observed.append(request)
        return httpx.Response(200, stream=stream, headers={"content-type": "text/event-stream"})
    front = ConsoleFront(tmp_path, 8855, "owner", "local-password-123", delivered.append,
        transport=httpx.MockTransport(native))
    app = front.application("http://127.0.0.1:61347")
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=app), base_url=front.origin) as client:
            await login(client, front)
            session = client.cookies[front.cookie]
            task, frames, responses, _ = await held_console_stream(app, front, session)
            tasks.append(task)
            await asyncio.wait_for(stream.waiting.wait(), 30)
            if active:
                frame = b'id: native-42\ndata: {"native":true}\n\n'
                stream.frames.put_nowait(frame)
                assert await asyncio.wait_for(frames.get(), 1) == frame
            # No request performs the old opportunistic session-pruning step.
            await asyncio.wait_for(stream.closed.wait(), 1.3)
            await asyncio.wait_for(asyncio.gather(task, return_exceptions=True), 0.2)
            stream.frames.put_nowait(b'data: expired-private-canary\n\n')
            assert session not in front.sessions and frames.empty()
            assert len(observed) == 1 and delivered == []
            assert "expired-private-canary" not in repr(responses)
    finally:
        await finish_stream_tasks(tasks)
        await front.close()


async def test_login_retirement_shutdown_stops_admission_and_joins_idle_stream_before_server(tmp_path):
    stream, observed, delivered, tasks = HeldNativeStream(), [], [], []
    def native(request):
        observed.append(request)
        return httpx.Response(200, stream=stream, headers={"content-type": "text/event-stream"})
    front = ConsoleFront(tmp_path, 8855, "owner", "local-password-123", delivered.append,
        transport=httpx.MockTransport(native))
    app = front.application("http://127.0.0.1:61347")
    release_server = asyncio.Event()
    front.server = SimpleNamespace(should_exit=False)
    front.task = asyncio.create_task(release_server.wait())
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=app), base_url=front.origin) as client:
            await login(client, front)
            task, frames, _, _ = await held_console_stream(app, front, client.cookies[front.cookie])
            tasks.append(task)
            await asyncio.wait_for(stream.waiting.wait(), 30)
            closing = asyncio.create_task(front.close())
            tasks.append(closing)
            await asyncio.wait_for(stream.closed.wait(), 0.2)
            await asyncio.wait_for(asyncio.gather(task, return_exceptions=True), 0.2)
            assert front.sessions == {} and not closing.done()
            refused = await client.post("/console/rpc", json={"method": "mobkit/console/send", "params": SUBMISSION},
                                        headers={"Origin": front.origin})
            assert refused.status_code == 503
            assert (await client.get("/login")).status_code == 503
            assert len(observed) == 1 and delivered == [] and frames.empty()
            release_server.set()
            await asyncio.wait_for(closing, 1)
            assert front.task.done() and front.client.is_closed
    finally:
        release_server.set()
        await finish_stream_tasks(tasks[:1])
        await asyncio.gather(*tasks[1:], return_exceptions=True)
        await front.close()


DESCRIPTOR_PATH = "/console/forge/frontends/descriptor"
DESCRIPTOR_HOST = {"protocol": 1, "host_id": "forge:" + "1" * 64,
                   "application_id": "synthetic-app", "source_digest": "2" * 64}
DESCRIPTOR_UNAVAILABLE = {"detail": "The application frontend owner is unavailable. No activation is claimed."}
DESCRIPTOR_CHANGED = {"detail": "The loaded application source changed. No frontend command was admitted."}


async def test_frontend_descriptor_reads_actual_loaded_private_declarations_without_values(tmp_path, monkeypatch):
    from test_frontend_private_declarations import _package, _wire
    host, application, fields = _package(tmp_path, monkeypatch)
    front = None
    native, managed = [], []
    try:
        await application.load()
        wired = _wire(host, application, fields)
        # The accepted wiring uses the loaded owner's declarations, not .env
        # values or an inventory callback. No runtime was constructed.
        (tmp_path / ".env").write_text("EARLY_TOKEN=inert-private-value-canary\n")
        application.integration.frontend_management = managed.append
        front = ConsoleFront(tmp_path, 8855, "owner", "local-password-123",
            transport=httpx.MockTransport(lambda request: native.append(request) or httpx.Response(200)))
        front.frontend_host = wired.frontend_host
        front.frontend_management = wired.frontend_management
        front.frontend_private_fields = wired.frontend_private_fields
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=front.application("http://127.0.0.1:61347")),
                                     base_url=front.origin) as client:
            await login(client, front)
            response = await client.get(DESCRIPTOR_PATH)
            assert response.status_code == 200, response.text
            assert response.json() == {"protocol": 1, "host": application.frontend_host(),
                                       "private_fields": list(application.frontend_private_fields())}
            assert "EARLY_TOKEN" in response.json()["private_fields"]
            assert "CONSOLE_LOGIN_PASSWORD" in response.json()["private_fields"]
            assert "PUBLIC_CAPTION" not in response.json()["private_fields"]
            assert b"inert-private-value-canary" not in response.content
            assert response.headers["content-type"] == "application/json"
            assert response.headers["cache-control"] == "no-store"
            assert "set-cookie" not in response.headers
            assert native == managed == [] and application.context.runtime is None
    finally:
        if front is not None:
            await front.close()
        await application.stop()


@pytest.mark.parametrize("credential", ["absent", "health", "native"])
async def test_frontend_descriptor_requires_original_human_login(tmp_path, credential):
    native, managed = [], []
    front = ConsoleFront(tmp_path, 8855, "owner", "local-password-123",
        transport=httpx.MockTransport(lambda request: native.append(request) or httpx.Response(200)))
    front.frontend_host = lambda: dict(DESCRIPTOR_HOST)
    front.frontend_management = managed.append
    front.frontend_private_fields = ("INERT_TOKEN",)
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=front.application("http://127.0.0.1:61347")),
                                     base_url=front.origin) as client:
            headers = {} if credential == "absent" else {"Authorization": "Bearer " + console_auth._token(
                front.key, "owner", scope="host-health" if credential == "health" else "native-console")}
            response = await client.get(DESCRIPTOR_PATH, headers=headers)
            assert response.status_code == 401 and response.json() == {"error": "Console login required."}
            assert "set-cookie" not in response.headers and b"INERT_TOKEN" not in response.content
            assert native == managed == []
    finally:
        await front.close()


@pytest.mark.parametrize("declaration", ["empty", "names", "undeclared", "duplicate", "wrong-type", "oversized", "host-extra", "host-type"])
async def test_frontend_descriptor_is_closed_bounded_and_has_no_partial_declarations(tmp_path, declaration):
    native, managed = [], []
    front = ConsoleFront(tmp_path, 8855, "owner", "local-password-123",
        transport=httpx.MockTransport(lambda request: native.append(request) or httpx.Response(200)))
    selected = dict(DESCRIPTOR_HOST)
    if declaration == "host-extra":
        selected["unselected"] = "inert-private-value-canary"
    if declaration == "host-type":
        selected["protocol"] = True
    if declaration != "undeclared":
        front.frontend_host = lambda: selected
        front.frontend_management = managed.append
    front.frontend_private_fields = {
        "empty": (), "names": ("declared name / \u03bb",), "undeclared": (), "duplicate": ("INERT_TOKEN", "INERT_TOKEN"),
        "wrong-type": ("INERT_TOKEN", 1), "oversized": ("x" * console_auth.MAX_FRONTEND_BODY,),
        "host-extra": ("INERT_TOKEN",), "host-type": ("INERT_TOKEN",),
    }[declaration]
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=front.application("http://127.0.0.1:61347")),
                                     base_url=front.origin) as client:
            await login(client, front)
            response = await client.get(DESCRIPTOR_PATH)
            if declaration in {"empty", "names"}:
                assert response.status_code == 200
                assert response.json() == {"protocol": 1, "host": DESCRIPTOR_HOST,
                                           "private_fields": list(front.frontend_private_fields)}
            elif declaration == "undeclared":
                assert response.status_code == 501
                assert response.json() == {"detail": "This application has not declared frontend management support."}
            else:
                assert response.status_code == 503 and response.json() == DESCRIPTOR_UNAVAILABLE
            assert len(response.content) <= console_auth.MAX_FRONTEND_BODY
            assert response.headers["cache-control"] == "no-store" and "set-cookie" not in response.headers
            assert b"INERT_TOKEN" not in response.content and b"inert-private-value-canary" not in response.content
            assert native == managed == []
    finally:
        await front.close()


@pytest.mark.parametrize("request_kind", ["query", "body", "method"])
async def test_frontend_descriptor_refuses_caller_selected_inputs(tmp_path, request_kind):
    managed, native = [], []
    front = ConsoleFront(tmp_path, 8855, "owner", "local-password-123",
        transport=httpx.MockTransport(lambda request: native.append(request) or httpx.Response(200)))
    front.frontend_host = lambda: dict(DESCRIPTOR_HOST)
    front.frontend_management = managed.append
    front.frontend_private_fields = ("INERT_TOKEN",)
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=front.application("http://127.0.0.1:61347")),
                                     base_url=front.origin) as client:
            await login(client, front)
            response = await client.request("POST" if request_kind == "method" else "GET",
                DESCRIPTOR_PATH + ("?component=unselected" if request_kind == "query" else ""),
                content=b'{"private_fields":["UNSELECTED"]}' if request_kind == "body" else b"",
                headers={"Origin": front.origin})
            assert response.status_code == (405 if request_kind == "method" else 400)
            assert "private_fields" not in response.json() and "set-cookie" not in response.headers
            assert native == managed == []
    finally:
        await front.close()


@pytest.mark.parametrize("phase,change", [("body", "logout"), ("body", "new-login"),
    ("body", "source"), ("body", "imports"), ("body", "shutdown"),
    ("enrollment", "source"), ("enrollment", "imports")])
async def test_frontend_descriptor_rechecks_loaded_owner_after_suspended_read(tmp_path, monkeypatch, phase, change):
    from test_frontend_private_declarations import _package, _wire
    host, application, fields = _package(tmp_path, monkeypatch)
    waiting, release = asyncio.Event(), asyncio.Event()
    native, managed, tasks = [], [], []
    front = None
    async def body():
        waiting.set()
        await release.wait()
        yield b""
    async def ready(selected):
        waiting.set()
        await release.wait()
        return False
    try:
        await application.load()
        wired = _wire(host, application, fields)
        application.integration.frontend_management = managed.append
        front = ConsoleFront(tmp_path, 8855, "owner", "local-password-123",
            transport=httpx.MockTransport(lambda request: native.append(request) or httpx.Response(200)))
        front.frontend_host = wired.frontend_host
        front.frontend_management = wired.frontend_management
        front.frontend_private_fields = wired.frontend_private_fields
        app = front.application("http://127.0.0.1:61347")
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=app), base_url=front.origin) as client:
            await login(client, front)
            if phase == "enrollment":
                monkeypatch.setattr(front, "_ready_console_login", ready)
            pending = asyncio.create_task(client.request("GET", DESCRIPTOR_PATH, content=body()))
            tasks.append(pending)
            await asyncio.wait_for(waiting.wait(), 30)
            if change in {"logout", "new-login"}:
                assert (await client.post("/logout", headers={"Origin": front.origin})).status_code == 303
                if change == "new-login":
                    await login(client, front)
                    assert (await client.get(DESCRIPTOR_PATH)).status_code == 200
            elif change == "source":
                path = tmp_path / "application/configuration.json"
                path.write_bytes(path.read_bytes() + b"\n")
            elif change == "imports":
                application._frontend_imports.mismatch = True
            else:
                await front.close()
            release.set()
            response = await asyncio.wait_for(pending, 1)
            expected = 401 if change in {"logout", "new-login"} else 503 if change == "shutdown" else 409
            assert response.status_code == expected, response.text
            assert response.json() == ({"error": "Console login required."} if expected == 401 else
                                       DESCRIPTOR_UNAVAILABLE if expected == 503 else DESCRIPTOR_CHANGED)
            assert b"EARLY_TOKEN" not in response.content and b"private_fields" not in response.content
            assert response.headers["cache-control"] == "no-store" and "set-cookie" not in response.headers
            assert native == managed == [] and application.context.runtime is None
    finally:
        release.set()
        await finish_stream_tasks(tasks)
        if front is not None:
            await front.close()
        await application.stop()


@pytest.mark.parametrize("change", ["unavailable", "shutdown", "retired"])
async def test_frontend_descriptor_outer_login_wait_preserves_exact_refusal(tmp_path, change, monkeypatch):
    waiting, release = asyncio.Event(), asyncio.Event()
    native, managed, tasks = [], [], []
    front = ConsoleFront(tmp_path, 8855, "owner", "local-password-123",
        transport=httpx.MockTransport(lambda request: native.append(request) or httpx.Response(200)))
    front.frontend_host = lambda: dict(DESCRIPTOR_HOST)
    front.frontend_management = managed.append
    front.frontend_private_fields = ("INERT_TOKEN",)
    async def ready(selected):
        waiting.set()
        await release.wait()
        return False if change == "unavailable" else True
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=front.application("http://127.0.0.1:61347")),
                                     base_url=front.origin) as client:
            await login(client, front)
            monkeypatch.setattr(front, "_ready_console_login", ready)
            pending = asyncio.create_task(client.get(DESCRIPTOR_PATH))
            tasks.append(pending)
            await asyncio.wait_for(waiting.wait(), 30)
            if change == "shutdown":
                await front.close()
            elif change == "retired":
                assert (await client.post("/logout", headers={"Origin": front.origin})).status_code == 303
            release.set()
            response = await asyncio.wait_for(pending, 1)
            assert response.status_code == (401 if change == "retired" else 503)
            assert response.json() == ({"error": "Console login required."} if change == "retired" else DESCRIPTOR_UNAVAILABLE)
            assert b"INERT_TOKEN" not in response.content and "set-cookie" not in response.headers
            assert native == managed == []
    finally:
        release.set()
        await finish_stream_tasks(tasks)
        await front.close()


@pytest.mark.parametrize("frontend", [False, True], ids=["native", "application-frontend"])
async def test_login_retirement_refuses_a_body_waiting_before_new_admission(tmp_path, frontend):
    observed, managed, tasks = [], [], []
    waiting, release = asyncio.Event(), asyncio.Event()
    front = ConsoleFront(tmp_path, 8855, "owner", "local-password-123",
        transport=httpx.MockTransport(lambda request: observed.append(request) or httpx.Response(200)))
    front.frontend_host = lambda: {"application_id": "reading", "source_sha256": "a" * 64}
    front.frontend_management = managed.append
    async def body():
        waiting.set()
        await release.wait()
        yield json.dumps({"configuration": {}, "expected_revision": None} if frontend else
                         {"method": "mobkit/console/send", "params": SUBMISSION}).encode()
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=front.application("http://127.0.0.1:61347")),
                                     base_url=front.origin) as client:
            await login(client, front)
            path = "/console/forge/frontends/prepare" if frontend else "/console/rpc"
            pending = asyncio.create_task(client.post(path, content=body(),
                headers={"Origin": front.origin, "Content-Type": "application/json"}))
            tasks.append(pending)
            await asyncio.wait_for(waiting.wait(), 30)
            assert (await client.post("/logout", headers={"Origin": front.origin})).status_code == 303
            release.set()
            assert (await asyncio.wait_for(pending, 1)).status_code == 401
            assert observed == [] and managed == []
    finally:
        release.set()
        await finish_stream_tasks(tasks)
        await front.close()


@pytest.mark.parametrize("wait", ["upstream", "callback"])
async def test_login_retirement_keeps_an_entered_native_input_and_its_application_delivery(tmp_path, wait):
    entered, release = asyncio.Event(), asyncio.Event()
    observed, delivered, tasks = [], [], []
    async def native(request):
        observed.append(request)
        if wait == "upstream":
            entered.set()
            await release.wait()
        return httpx.Response(200, json={"result": ACCEPTED})
    async def receive(event):
        delivered.append(event)
        if wait == "callback":
            entered.set()
            await release.wait()
    front = ConsoleFront(tmp_path, 8855, "owner", "local-password-123", receive,
        transport=httpx.MockTransport(native))
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=front.application("http://127.0.0.1:61347")),
                                     base_url=front.origin) as client:
            await login(client, front)
            pending = asyncio.create_task(client.post("/console/rpc",
                json={"method": "mobkit/console/send", "params": SUBMISSION}, headers={"Origin": front.origin}))
            tasks.append(pending)
            await asyncio.wait_for(entered.wait(), 30)
            assert (await client.post("/logout", headers={"Origin": front.origin})).status_code == 303
            assert not pending.done() and front.sessions == {}
            release.set()
            result = await asyncio.wait_for(pending, 1)
            assert result.status_code == 401 and result.json() == {"error": "Console login required."}
            assert ACCEPTED["input_frame_id"].encode() not in result.content
            assert len(observed) == 1 and len(delivered) == 1
            assert delivered[0].principal == "owner" and delivered[0].input_frame_id == ACCEPTED["input_frame_id"]
            assert front.delivered == {ACCEPTED["interaction_id"]}
    finally:
        release.set()
        await finish_stream_tasks(tasks)
        await front.close()


@pytest.mark.parametrize("path,content_type,status,phase,retirement", [
    ("/console/timeline", "application/json", 200, "body", "logout"),
    ("/console/timeline", "application/json", 200, "cleanup", "logout"),
    ("/console", "text/html", 200, "body", "logout"),
    ("/console", "text/html", 200, "cleanup", "logout"),
    ("/blobs/private-object", "application/octet-stream", 206, "body", "logout"),
    ("/blobs/private-object", "application/octet-stream", 206, "cleanup", "logout"),
    ("/console/timeline", "application/json", 200, "body", "close"),
    ("/console/timeline", "application/json", 200, "cleanup", "close"),
    ("/console/timeline", "application/json", 200, "body", "current"),
], ids=["json-body-logout", "json-cleanup-logout", "html-body-logout", "html-cleanup-logout",
        "blob-body-logout", "blob-cleanup-logout", "json-body-close", "json-cleanup-close", "current-login"])
async def test_ordinary_finite_console_response_discloses_only_to_its_current_login(
        tmp_path, path, content_type, status, phase, retirement):
    entered, release = asyncio.Event(), asyncio.Event()
    observed, tasks = [], []
    payload = b"private-finite-console-response-canary"
    close_calls = 0

    class FiniteBody(httpx.AsyncByteStream):
        async def __aiter__(self):
            if phase == "body":
                entered.set()
                await release.wait()
            yield payload

    def native(request):
        observed.append(request)
        if request.url.path == "/console/identities":
            return httpx.Response(200, json={"current_login": "B"})
        assert request.url.path == path
        response = httpx.Response(status, stream=FiniteBody(), headers={
            "content-type": content_type, "etag": "private-finite-etag",
            "content-range": "bytes 0-36/37", "accept-ranges": "bytes"})
        original_close = response.aclose

        async def close():
            nonlocal close_calls
            close_calls += 1
            # HTTPX closes on exhaustion first. Hold the proxy's own final
            # close after the complete private body has already been read.
            if phase == "cleanup" and close_calls == 2:
                entered.set()
                await release.wait()
            await original_close()

        response.aclose = close
        return response

    front = ConsoleFront(tmp_path, 8855, "owner", "local-password-123",
        transport=httpx.MockTransport(native))
    app = front.application("http://127.0.0.1:61347")
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=app), base_url=front.origin) as first, \
                httpx.AsyncClient(transport=httpx.ASGITransport(app=app), base_url=front.origin) as second:
            await login(first, front)
            first_session = first.cookies[front.cookie]
            if retirement == "logout":
                await login(second, front)
                second_session = second.cookies[front.cookie]
                assert first_session != second_session
            pending = asyncio.create_task(first.get(path))
            tasks.append(pending)
            await asyncio.wait_for(entered.wait(), 30)
            assert not pending.done() and len(observed) == 1
            if retirement == "logout":
                assert (await first.post("/logout", headers={"Origin": front.origin})).status_code == 303
                assert first_session not in front.sessions and second_session in front.sessions
                # A current login cannot authorize the old request's disclosure.
                assert (await second.get("/console/identities")).json() == {"current_login": "B"}
            elif retirement == "close":
                await front.close()
                assert front._closing and front.sessions == {} and not pending.done()
            release.set()
            result = await asyncio.wait_for(pending, 1)
            if retirement == "current":
                assert result.status_code == status and result.content == payload
                assert result.headers["etag"] == "private-finite-etag"
            else:
                assert result.status_code == 401 and result.json() == {"error": "Console login required."}
                assert payload not in result.content
                assert "etag" not in result.headers and "content-range" not in result.headers
                assert result.headers["cache-control"] == "no-store"
                assert result.headers["referrer-policy"] == "no-referrer"
                assert result.headers["content-security-policy"] == "frame-ancestors 'none'"
            assert close_calls == 2
            assert len(observed) == (2 if retirement == "logout" else 1)
    finally:
        release.set()
        await finish_stream_tasks(tasks)
        await front.close()


async def test_login_retirement_joins_a_stream_waiting_for_native_headers(tmp_path):
    entered, cancelled = asyncio.Event(), asyncio.Event()
    observed, tasks = [], []
    async def native(request):
        observed.append(request)
        entered.set()
        try:
            await asyncio.Event().wait()
        finally:
            cancelled.set()
    front = ConsoleFront(tmp_path, 8855, "owner", "local-password-123",
        transport=httpx.MockTransport(native))
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=front.application("http://127.0.0.1:61347")),
                                     base_url=front.origin) as client:
            await login(client, front)
            pending = asyncio.create_task(client.get("/console/timeline/stream?after_cursor=41"))
            tasks.append(pending)
            await asyncio.wait_for(entered.wait(), 30)
            assert (await client.post("/logout", headers={"Origin": front.origin})).status_code == 303
            assert cancelled.is_set() and pending.done() and pending.cancelled()
            assert len(observed) == 1 and front.sessions == {} and front.delivered == set()
    finally:
        await finish_stream_tasks(tasks)
        await front.close()


async def test_login_retirement_keeps_failed_upstream_cleanup_unconfirmed_and_private(tmp_path):
    class FailingCloseStream(HeldNativeStream):
        close_calls = 0
        async def aclose(self):
            self.close_calls += 1
            self.closed.set()
            raise RuntimeError("private-upstream-close-canary")
    stream, responses, tasks = FailingCloseStream(), [], []
    def native(request):
        response = httpx.Response(200, stream=stream, headers={"content-type": "text/event-stream"})
        responses.append(response)
        return response
    front = ConsoleFront(tmp_path, 8855, "owner", "local-password-123",
        transport=httpx.MockTransport(native))
    app = front.application("http://127.0.0.1:61347")
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=app),
                                     base_url=front.origin) as client:
            await login(client, front)
            session = client.cookies[front.cookie]
            task, _, public, _ = await held_console_stream(app, front, session)
            tasks.append(task)
            await asyncio.wait_for(stream.waiting.wait(), 30)
            result = await client.post("/logout", headers={"Origin": front.origin})
            assert result.status_code == 503
            assert "cleanup is unconfirmed" in result.text
            assert responses[0].is_closed and stream.close_calls == 1
            assert session not in front.sessions
            assert (await client.get("/console/timeline/stream")).status_code == 401
            with pytest.raises(RuntimeError, match="subscription cleanup is unconfirmed"):
                await front.close()
            assert task.done() and front.client.is_closed
            assert "private-upstream-close-canary" not in result.text + repr(public)
            assert session not in result.text + repr(public)
    finally:
        await finish_stream_tasks(tasks)
        try:
            await front.close()
        except RuntimeError:
            pass


async def test_login_retirement_joins_the_first_slow_upstream_close_before_logout_success(tmp_path):
    closing, release = asyncio.Event(), asyncio.Event()
    class SlowCloseStream(HeldNativeStream):
        close_calls = 0
        async def aclose(self):
            self.close_calls += 1
            closing.set()
            await release.wait()
            self.closed.set()
    stream, tasks = SlowCloseStream(), []
    front = ConsoleFront(tmp_path, 8855, "owner", "local-password-123",
        transport=httpx.MockTransport(lambda request: httpx.Response(200, stream=stream,
            headers={"content-type": "text/event-stream"})))
    app = front.application("http://127.0.0.1:61347")
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=app), base_url=front.origin) as client:
            await login(client, front)
            task, _, _, _ = await held_console_stream(app, front, client.cookies[front.cookie])
            tasks.append(task)
            await asyncio.wait_for(stream.waiting.wait(), 30)
            logout = asyncio.create_task(client.post("/logout", headers={"Origin": front.origin}))
            tasks.append(logout)
            await asyncio.wait_for(closing.wait(), 30)
            assert front.sessions == {} and not logout.done() and not stream.closed.is_set()
            assert (await client.get("/console/timeline/stream")).status_code == 401
            release.set()
            assert (await asyncio.wait_for(logout, 1)).status_code == 303
            assert stream.closed.is_set() and stream.close_calls == 1 and task.done()
    finally:
        release.set()
        await finish_stream_tasks(tasks)
        await front.close()


@pytest.mark.parametrize("failing", [False, True], ids=["held-first-close", "failed-first-close"])
async def test_login_retirement_owns_httpx_finite_stream_exhaustion_close(tmp_path, failing):
    closing, release = asyncio.Event(), asyncio.Event()
    class FiniteStream(httpx.AsyncByteStream):
        close_calls = 0
        closed = False
        async def __aiter__(self):
            yield b'id: native-42\ndata: {"native":"finite"}\n\n'
        async def aclose(self):
            self.close_calls += 1
            closing.set()
            if failing:
                raise RuntimeError("finite-private-close-canary")
            await release.wait()
            self.closed = True
    stream, tasks = FiniteStream(), []
    front = ConsoleFront(tmp_path, 8855, "owner", "local-password-123",
        transport=httpx.MockTransport(lambda request: httpx.Response(200, stream=stream,
            headers={"content-type": "text/event-stream"})))
    app = front.application("http://127.0.0.1:61347")
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=app), base_url=front.origin) as client:
            await login(client, front)
            task, frames, public, _ = await held_console_stream(app, front, client.cookies[front.cookie])
            tasks.append(task)
            assert await asyncio.wait_for(frames.get(), 1) == b'id: native-42\ndata: {"native":"finite"}\n\n'
            await asyncio.wait_for(closing.wait(), 30)
            logout = asyncio.create_task(client.post("/logout", headers={"Origin": front.origin}))
            tasks.append(logout)
            if failing:
                assert (await asyncio.wait_for(logout, 1)).status_code == 503
                outcome = await asyncio.gather(task, return_exceptions=True)
                assert "finite-private-close-canary" not in repr(outcome) + repr(public)
                with pytest.raises(RuntimeError, match="subscription cleanup is unconfirmed"):
                    await front.close()
            else:
                with pytest.raises(asyncio.TimeoutError):
                    await asyncio.wait_for(asyncio.shield(logout), 0.05)
                assert not stream.closed and front.sessions == {}
                release.set()
                assert (await asyncio.wait_for(logout, 1)).status_code == 303
                assert stream.closed and task.done()
            assert stream.close_calls == 1
    finally:
        release.set()
        await finish_stream_tasks(tasks)
        try:
            await front.close()
        except RuntimeError:
            pass


def test_native_builder_emits_real_pinned_auth_contract_without_browser_token(tmp_path):
    from meerkat_mobkit import MobKit
    from meerkat_mobkit.runtime import MobKitRuntime
    front = ConsoleFront(tmp_path, 8855, "owner", "local-password-123")
    builder = MobKit.builder().auth(front.native_auth).http_listen("127.0.0.1:0").console_auth_required(True)
    params = MobKitRuntime(builder._config)._build_init_params()
    assert params["runtime_options"]["auth_config"] == {"provider": "jwt", "shared_secret": front.key,
                                    "issuer": ISSUER, "audience": AUDIENCE, "email_allowlist": ["owner"]}
    assert params["runtime_options"]["console_require_app_auth"] is True
    assert params["runtime_options"]["http_listen"] == "127.0.0.1:0"
    assert "auth_token" not in front.origin


async def test_front_start_and_stop_own_server_client_and_signal_lifecycle(tmp_path, monkeypatch):
    import uvicorn
    instances = []
    class Server:
        def __init__(self, config):
            self.started, self.should_exit = False, False
            instances.append(self)
        async def serve(self, sockets):
            self.started = True
            while not self.should_exit:
                await asyncio.sleep(0.01)
    monkeypatch.setattr(uvicorn, "Server", Server)
    front = ConsoleFront(tmp_path, 0, "owner", "local-password-123")
    await front.start("http://127.0.0.1:61347")
    assert instances[0].started and front.listener.getsockname()[0] == "127.0.0.1"
    front.sessions["session"] = 12345678900
    await front.close()
    assert front.task.done() and front.client.is_closed and front.sessions == {}
    assert front.listener.fileno() == -1


async def test_front_start_failure_closes_owned_resources(tmp_path, monkeypatch):
    import uvicorn
    class Server:
        started = False
        def __init__(self, config):
            pass
        async def serve(self, sockets):
            raise RuntimeError("host startup failed")
    monkeypatch.setattr(uvicorn, "Server", Server)
    front = ConsoleFront(tmp_path, 0, "owner", "local-password-123")
    with pytest.raises(RuntimeError, match="host startup failed"):
        await front.start("http://127.0.0.1:61347")
    assert front.task.done() and front.client.is_closed and front.listener.fileno() == -1


async def test_toolkit_panel_proxy_uses_existing_login_and_server_bearer(tmp_path, monkeypatch):
    clock = {"now": float(int(console_auth.time.time()) - 10)}
    monkeypatch.setattr(console_auth.time, "time", lambda: clock["now"])
    paths = {
        "/host/toolkit/setup": "text/html; charset=utf-8",
        "/host/toolkit/setup/data": "application/json",
        "/host/toolkit/setup/panel.js": "application/javascript",
        "/host/toolkit/setup/view.js": "application/javascript",
        "/host/toolkit/setup/projection.js": "application/javascript",
        "/host/toolkit/setup/style.css": "text/css",
    }
    observed, delivered = [], []
    def native(request):
        observed.append(request)
        return httpx.Response(200, content=("native:" + request.url.path).encode(), headers={
            "Content-Type": paths.get(request.url.path, "text/html"),
            "Location": "https://unselected.example/private",
            "Set-Cookie": "native-private-cookie=canary", "Authorization": "Bearer native-canary",
            "Content-Security-Policy": "frame-ancestors *", "Cache-Control": "public",
        })
    front = ConsoleFront(tmp_path, 8855, "owner", "local-password-123", delivered.append,
        transport=httpx.MockTransport(native))
    app = front.application("http://127.0.0.1:61347")
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=app),
                                     base_url=front.origin) as client:
            await login(client, front)
            session = client.cookies[front.cookie]
            actual_login = front._logins[session]
            for path in paths:
                response = await client.get(path, headers={"Authorization": "Bearer browser-credential-canary"})
                assert response.status_code == 200 and response.content == ("native:" + path).encode()
                request = observed[-1]
                assert request.method == "GET" and not request.url.query
                assert request.url.path == path
                assert request.url.raw_path == path.encode(), "Native setup rejects even an empty query delimiter."
                assert request.url.host == "127.0.0.1" and request.url.port == 61347
                assert "origin" not in request.headers
                assert front.cookie not in request.headers.get("cookie", "")
                assert session not in request.headers.get("cookie", "")
                token = request.headers["authorization"].removeprefix("Bearer ")
                assert token != "browser-credential-canary"
                claims = jwt.decode(token, front.key, algorithms=["HS256"], issuer=ISSUER, audience=AUDIENCE)
                assert claims["sub"] == "owner" and claims["host_scope"] == "native-console"
                assert claims["sid"] == actual_login.login_scope_id
                assert claims["iat"] == int(clock["now"])
                expected = "frame-ancestors 'self'" if path == "/host/toolkit/setup" else "frame-ancestors 'none'"
                assert response.headers["content-security-policy"] == expected
                assert response.headers["cache-control"] == "no-store"
                assert response.headers["referrer-policy"] == "no-referrer"
                assert response.headers["x-content-type-options"] == "nosniff"
                assert all(name not in response.headers for name in ("location", "set-cookie", "authorization"))
                assert session not in response.text and front.key not in response.text
                assert observed[-1].headers["authorization"] not in response.text
            ordinary = await client.get("/console")
            assert ordinary.status_code == 200
            assert ordinary.headers["content-security-policy"] == "frame-ancestors 'none'"
            first_token = observed[-1].headers["authorization"].removeprefix("Bearer ")
            assert first_token == token
            # A non-HTML native refusal at the panel path must remain unframeable.
            paths["/host/toolkit/setup"] = "application/json"
            refusal = await client.get("/host/toolkit/setup")
            assert refusal.headers["content-security-policy"] == "frame-ancestors 'none'"
            assert len(observed) == 8 and delivered == []
            async with httpx.AsyncClient(transport=httpx.ASGITransport(app=app), base_url=front.origin) as second:
                await login(second, front)
                second_session = second.cookies[front.cookie]
                second_login = front._logins[second_session]
                assert second_session != session and second_login is not actual_login
                assert second_login.login_scope_id != actual_login.login_scope_id
                for path in ("/host/toolkit/setup/data", "/console"):
                    response = await second.get(path, headers={"Authorization": "Bearer browser-credential-canary"})
                    assert response.status_code == 200
                    second_token = observed[-1].headers["authorization"].removeprefix("Bearer ")
                    second_claims = jwt.decode(second_token, front.key, algorithms=["HS256"], issuer=ISSUER, audience=AUDIENCE)
                    assert second_token not in ("browser-credential-canary", first_token)
                    assert second_claims["sub"] == claims["sub"] == front.principal
                    assert second_claims["iat"] == claims["iat"] == int(clock["now"])
                    assert second_claims["sid"] == second_login.login_scope_id
                    assert front._logins[session] is actual_login and front._logins[second_session] is second_login
                assert observed[-2].headers["authorization"] == observed[-1].headers["authorization"]
                # Browser association claims cannot select the other current login.
                for key in ("sid", "login_scope_id"):
                    assert (await client.get("/host/toolkit/setup/data", params={key: second_login.login_scope_id})).status_code == 400
                    response = await client.post("/host/toolkit/setup/answer", json={"review_id": 7,
                        "approval_id": "opaque", "decision": "deny", key: second_login.login_scope_id},
                        headers={"Origin": front.origin})
                    assert response.status_code == 400
                assert len(observed) == 10
                response = await client.get("/host/toolkit/setup/data", headers={
                    "Authorization": "Bearer " + second_token, "sid": second_login.login_scope_id,
                    "X-Login-Scope-Id": second_login.login_scope_id})
                assert response.status_code == 200
                request = observed[-1]
                assert request.headers["authorization"] == "Bearer " + first_token
                assert "sid" not in request.headers and "x-login-scope-id" not in request.headers
                assert front._logins[session] is actual_login and not actual_login.retired and not second_login.retired
                assert len(observed) == 11 and delivered == []
    finally:
        await front.close()


async def test_toolkit_panel_proxy_refuses_other_paths_methods_queries_and_credentials(tmp_path):
    observed = []
    front = ConsoleFront(tmp_path, 8855, "owner", "local-password-123",
        transport=httpx.MockTransport(lambda request: observed.append(request) or httpx.Response(200)))
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=front.application("http://127.0.0.1:61347")),
                                     base_url=front.origin) as client:
            path = "/host/toolkit/setup"
            assert (await client.get(path)).status_code == 401
            assert (await client.get(path, headers={"Cookie": f"{front.cookie}=wrong-session"})).status_code == 401
            assert (await client.get(path, headers={"Authorization": "Bearer browser-credential-canary"})).status_code == 401
            headers = {"Authorization": "Bearer " + console_auth._token(front.key, "owner", scope="host-health")}
            assert (await client.get(path, headers=headers)).status_code == 401
            await login(client, front)
            for route in ("/host", "/host/toolkit", path + "/", path + "/unknown", path + "/data/extra",
                          path + ".js", "/host/toolkit/other", "/host/other/setup"):
                assert (await client.get(route)).status_code == 404
            for suffix in ("?auth_token=browser-canary", "?selector=other", "?realm=other", "?x=", "?x=1&x=2"):
                assert (await client.get(path + suffix)).status_code == 400
            for method in ("HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"):
                response = await client.request(method, path, headers={"Origin": front.origin})
                assert response.status_code == 405 and response.headers["allow"] == "GET"
            assert (await client.get(path, headers={"Host": "hostile.example"})).status_code == 400
            assert (await client.get(path, headers={"Origin": "https://hostile.example"})).status_code == 403
            assert observed == []
    finally:
        await front.close()


@pytest.mark.parametrize("phase", ["body", "cleanup"])
@pytest.mark.parametrize("retirement", ["logout", "replacement"])
async def test_toolkit_panel_proxy_rechecks_exact_login_after_body_and_cleanup(tmp_path, phase, retirement):
    entered, release = asyncio.Event(), asyncio.Event()
    observed, tasks = [], []
    payload = b"private-toolkit-panel-canary"
    close_calls = 0
    class PanelBody(httpx.AsyncByteStream):
        async def __aiter__(self):
            if phase == "body":
                entered.set()
                await release.wait()
            yield payload
    def native(request):
        observed.append(request)
        response = httpx.Response(200, stream=PanelBody(), headers={"content-type": "text/html"})
        original_close = response.aclose
        async def close():
            nonlocal close_calls
            close_calls += 1
            # HTTPX closes on iterator exhaustion first. Hold the proxy's own
            # final cleanup call, after its complete body has already returned.
            if phase == "cleanup" and close_calls == 2:
                entered.set()
                await release.wait()
            await original_close()
        response.aclose = close
        return response
    front = ConsoleFront(tmp_path, 8855, "owner", "local-password-123", transport=httpx.MockTransport(native))
    original_login = None
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=front.application("http://127.0.0.1:61347")),
                                     base_url=front.origin) as client:
            await login(client, front)
            session = client.cookies[front.cookie]
            original_login = front._logins[session]
            path = "/host/toolkit/setup/data" if phase == "body" else "/host/toolkit/setup"
            pending = asyncio.create_task(client.get(path))
            tasks.append(pending)
            await asyncio.wait_for(entered.wait(), 30)
            assert not pending.done() and len(observed) == 1
            if retirement == "logout":
                assert (await client.post("/logout", headers={"Origin": front.origin})).status_code == 303
                assert front.sessions == {}
            else:
                # Same cookie and expiry remain live, but its exact login owner
                # has changed. A truthy login check must not release old bytes.
                front._logins[session] = console_auth._ConsoleLogin(session, original_login.expires)
                assert front.sessions[session] == original_login.expires
            release.set()
            result = await asyncio.wait_for(pending, 1)
            assert result.status_code == 401 and result.json() == {"error": "Console login required."}
            assert payload not in result.content
            assert result.headers["cache-control"] == "no-store"
            assert result.headers["referrer-policy"] == "no-referrer"
            assert result.headers["content-security-policy"] == "frame-ancestors 'none'"
            assert close_calls == 2 and len(observed) == 1
    finally:
        release.set()
        await finish_stream_tasks(tasks)
        if retirement == "replacement" and original_login is not None:
            front._logins[original_login.session] = original_login
        await front.close()


@pytest.mark.parametrize("route,payload,limit", [
    ("start", {"review_id": 7}, 1024),
    ("callback", {"query": "state=opaque&state=duplicate&code=opaque&error=native-owned"}, 64 * 1024),
    ("answer", {"review_id": -(2**53 - 1), "approval_id": "Å" * 256, "decision": "approve"}, 4 * 1024),
])
async def test_toolkit_protected_post_forwards_exact_bounded_bytes_without_application_input(tmp_path, monkeypatch, route, payload, limit):
    observed, delivered = [], []
    native_payload = b'{"native":"opaque-settlement"}'
    def native(request):
        observed.append(request)
        return httpx.Response(202, content=native_payload, headers={"content-type": "application/json",
            "Location": "https://unselected.example/private", "Authorization": "Bearer native-canary"})
    front = ConsoleFront(tmp_path, 8855, "owner", "local-password-123", delivered.append,
        transport=httpx.MockTransport(native))
    async def never_application_input(*args):
        raise AssertionError("Toolkit requests must not inspect or deliver application input.")
    monkeypatch.setattr(front, "_submission", never_application_input)
    path = "/host/toolkit/setup/" + route
    raw = json.dumps(payload, ensure_ascii=False, separators=(", ", ": ")).encode("utf-8")
    raw += b" " * (limit - len(raw))
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=front.application("http://127.0.0.1:61347")),
                                     base_url=front.origin) as client:
            await login(client, front)
            session = client.cookies[front.cookie]
            response = await client.post(path, content=raw, headers={"Origin": front.origin,
                "Content-Type": "application/json; charset=utf-8", "Authorization": "Bearer browser-canary"})
            assert response.status_code == 202 and response.content == native_payload
            assert len(observed) == 1 and delivered == []
            request = observed[0]
            assert request.method == "POST" and request.url.path == path and not request.url.query
            assert request.url.raw_path == path.encode(), "Native setup mutations require no query delimiter."
            assert request.url.host == "127.0.0.1" and request.url.port == 61347
            assert request.content == raw and len(request.content) == limit
            assert request.headers["origin"] == front.origin
            assert front.cookie not in request.headers.get("cookie", "")
            assert session not in request.headers.get("cookie", "")
            token = request.headers["authorization"].removeprefix("Bearer ")
            assert token != "browser-canary"
            claims = jwt.decode(token, front.key, algorithms=["HS256"], issuer=ISSUER, audience=AUDIENCE)
            assert claims["sub"] == "owner" and claims["host_scope"] == "native-console"
            assert response.headers["content-security-policy"] == "frame-ancestors 'none'"
            assert response.headers["cache-control"] == "no-store" and response.headers["referrer-policy"] == "no-referrer"
            assert all(name not in response.headers for name in ("authorization", "location", "set-cookie"))
            assert token not in response.text and session not in response.text and front.key not in response.text
    finally:
        await front.close()


@pytest.mark.parametrize("route", ["start", "callback", "answer"])
async def test_toolkit_protected_post_refuses_unselected_transport_and_closed_shape_without_native_effect(tmp_path, caplog, route):
    observed, delivered = [], []
    front = ConsoleFront(tmp_path, 8855, "owner", "local-password-123", delivered.append,
        transport=httpx.MockTransport(lambda request: observed.append(request) or httpx.Response(202)))
    path = "/host/toolkit/setup/" + route
    valid = ({"query": "opaque-private-callback-canary"} if route == "callback" else
             {"review_id": 7} if route == "start" else
             {"review_id": 7, "approval_id": "opaque-private-approval-canary", "decision": "deny"})
    limit = 64 * 1024 if route == "callback" else 1024 if route == "start" else 4 * 1024
    headers = {"Origin": front.origin, "Content-Type": "application/json"}
    invalid = ([b'{"query":"one","query":"two"}', json.dumps({**valid, "issuer": "browser"}).encode(),
        json.dumps({"query": ["opaque-private-callback-canary"]}).encode(),
        json.dumps({"query": "Å" * 8193}).encode(), b'{"query":"\\ud800"}'] if route == "callback" else [
        b'{"review_id":7,"review_id":8}', json.dumps({**valid, "principal": "browser"}).encode(),
        *[json.dumps({"review_id": value}).encode() for value in (True, 7.0, 2**53, -(2**53), "7")],
        json.dumps({}).encode()] if route == "start" else [
        b'{"review_id":7,"review_id":8,"approval_id":"opaque","decision":"deny"}',
        json.dumps({**valid, "target": "browser"}).encode(),
        *[json.dumps({**valid, "review_id": value}).encode() for value in (True, 7.0, 2**53, -(2**53), "7")],
        json.dumps({**valid, "approval_id": "Å" * 257}).encode(),
        json.dumps({**valid, "approval_id": ["opaque-private-approval-canary"]}).encode(),
        b'{"review_id":7,"approval_id":"\\ud800","decision":"deny"}',
        json.dumps({**valid, "decision": "browser-approved"}).encode()])
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=front.application("http://127.0.0.1:61347")),
                                     base_url=front.origin) as client:
            assert (await client.post(path, json=valid, headers={"Origin": front.origin})).status_code == 401
            health = {"Origin": front.origin, "Authorization": "Bearer " + console_auth._token(front.key, "owner", scope="host-health")}
            assert (await client.post(path, json=valid, headers=health)).status_code == 401
            assert (await client.post(path, json=valid, headers={"Origin": front.origin,
                "Authorization": "Bearer browser-canary"})).status_code == 401
            await login(client, front)
            for method in (("HEAD", "PUT", "PATCH", "DELETE", "OPTIONS") if route == "callback" else
                           ("GET", "HEAD", "PUT", "PATCH", "DELETE", "OPTIONS")):
                result = await client.request(method, path, headers={"Origin": front.origin})
                assert result.status_code == 405 and result.headers["allow"] == ("GET, POST" if route == "callback" else "POST")
            assert (await client.post(path, json=valid)).status_code == 403
            assert (await client.post(path, json=valid, headers={"Origin": "https://foreign.example"})).status_code == 403
            assert (await client.post(path, json=valid, headers={"Origin": front.origin, "Host": "foreign.example"})).status_code == 400
            for suffix in ("?state=opaque", "?approval_id=browser", "?auth_token=browser", "?x=1&x=2"):
                assert (await client.post(path + suffix, json=valid, headers={"Origin": front.origin})).status_code == 400
            assert (await client.post(path + "/extra", json=valid, headers={"Origin": front.origin})).status_code == 404
            assert (await client.post(path, content=json.dumps(valid), headers={"Origin": front.origin,
                "Content-Type": "text/plain"})).status_code == 400
            for body in [b"[]", b"null", b"not-json", b"[" * 1500 + b"0" + b"]" * 1500, *invalid, b" " * (limit + 1)]:
                result = await client.post(path, content=body, headers=headers)
                assert result.status_code == 400
                assert b"opaque-private" not in result.content
            # A review-shaped body is not a valid native data-write request.
            assert (await client.post("/host/toolkit/setup/data", json=valid, headers={"Origin": front.origin})).status_code == 400
            assert observed == [] and delivered == []
            assert "opaque-private-callback-canary" not in caplog.text
            assert "opaque-private-approval-canary" not in caplog.text
    finally:
        await front.close()


@pytest.mark.parametrize("phase", ["inbound", "body", "cleanup"])
@pytest.mark.parametrize("retirement", ["logout", "replacement"])
async def test_toolkit_protected_post_rechecks_exact_login_across_all_body_and_cleanup_awaits(tmp_path, phase, retirement):
    entered, release = asyncio.Event(), asyncio.Event()
    observed, delivered, tasks = [], [], []
    private_response = b"private-toolkit-answer-response-canary"
    close_calls = 0
    route = "callback" if retirement == "logout" else "answer"
    payload = {"query": "state=opaque&code=opaque-private-callback-canary"} if route == "callback" else {
        "review_id": 9, "approval_id": "opaque-approval", "decision": "deny"}
    raw = json.dumps(payload).encode()
    class ProtectedBody(httpx.AsyncByteStream):
        async def __aiter__(self):
            if phase == "body":
                entered.set()
                await release.wait()
            yield private_response
    def native(request):
        observed.append(request)
        response = httpx.Response(202, stream=ProtectedBody(), headers={"content-type": "application/json"})
        original_close = response.aclose
        async def close():
            nonlocal close_calls
            close_calls += 1
            if phase == "cleanup" and close_calls == 2:
                entered.set()
                await release.wait()
            await original_close()
        response.aclose = close
        return response
    async def inbound():
        yield raw[:1]
        entered.set()
        await release.wait()
        yield raw[1:]
    front = ConsoleFront(tmp_path, 8855, "owner", "local-password-123", delivered.append,
        transport=httpx.MockTransport(native))
    original_login = None
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=front.application("http://127.0.0.1:61347")),
                                     base_url=front.origin) as client:
            await login(client, front)
            session = client.cookies[front.cookie]
            original_login = front._logins[session]
            pending = asyncio.create_task(client.post("/host/toolkit/setup/" + route,
                content=inbound() if phase == "inbound" else raw,
                headers={"Origin": front.origin, "Content-Type": "application/json"}))
            tasks.append(pending)
            await asyncio.wait_for(entered.wait(), 30)
            assert not pending.done() and len(observed) == (0 if phase == "inbound" else 1)
            if retirement == "logout":
                assert (await client.post("/logout", headers={"Origin": front.origin})).status_code == 303
            else:
                front._logins[session] = console_auth._ConsoleLogin(session, original_login.expires)
                assert front.sessions[session] == original_login.expires
            release.set()
            result = await asyncio.wait_for(pending, 1)
            assert result.status_code == 401 and result.json() == {"error": "Console login required."}
            assert private_response not in result.content and delivered == []
            assert len(observed) == (0 if phase == "inbound" else 1)
            assert close_calls == (0 if phase == "inbound" else 2)
            if observed:
                assert observed[0].content == raw
            assert result.headers.get("content-security-policy", "frame-ancestors 'none'") == "frame-ancestors 'none'"
    finally:
        release.set()
        await finish_stream_tasks(tasks)
        if retirement == "replacement" and original_login is not None:
            front._logins[original_login.session] = original_login
        await front.close()


async def test_toolkit_protected_post_sse_label_cannot_bypass_final_cleanup_login_fence(tmp_path):
    entered, release = asyncio.Event(), asyncio.Event()
    observed, delivered, tasks = [], [], []
    private_payload = b"data: private-toolkit-protected-answer-canary\n\n"
    close_calls = 0
    class FiniteBody(httpx.AsyncByteStream):
        async def __aiter__(self):
            yield private_payload
    def native(request):
        observed.append(request)
        response = httpx.Response(202, stream=FiniteBody(), headers={"content-type": "text/event-stream"})
        original_close = response.aclose
        async def close():
            nonlocal close_calls
            close_calls += 1
            if close_calls == 2:
                entered.set()
                await release.wait()
            await original_close()
        response.aclose = close
        return response
    front = ConsoleFront(tmp_path, 8855, "owner", "local-password-123", delivered.append,
        transport=httpx.MockTransport(native))
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=front.application("http://127.0.0.1:61347")),
                                     base_url=front.origin) as client:
            await login(client, front)
            pending = asyncio.create_task(client.post("/host/toolkit/setup/answer",
                json={"review_id": 4, "approval_id": "opaque-retained-approval", "decision": "deny"},
                headers={"Origin": front.origin}))
            tasks.append(pending)
            await asyncio.wait_for(entered.wait(), 30)
            assert not pending.done() and close_calls == 2 and len(observed) == 1
            assert (await client.post("/logout", headers={"Origin": front.origin})).status_code == 303
            release.set()
            response = await asyncio.wait_for(pending, 1)
            assert response.status_code == 401 and response.json() == {"error": "Console login required."}
            assert private_payload not in response.content and delivered == []
            assert len(observed) == 1 and close_calls == 2
            assert response.headers["cache-control"] == "no-store"
            assert response.headers["content-security-policy"] == "frame-ancestors 'none'"
    finally:
        release.set()
        await finish_stream_tasks(tasks)
        await front.close()


async def test_toolkit_protected_post_bounded_decoder_recursion_is_a_generic_refusal(tmp_path):
    observed, delivered = [], []
    body = b"[" * 10000 + b"0" + b"]" * 10000
    assert len(body) == 20001 and len(body) < 64 * 1024
    front = ConsoleFront(tmp_path, 8855, "owner", "local-password-123", delivered.append,
        transport=httpx.MockTransport(lambda request: observed.append(request) or httpx.Response(202)))
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=front.application("http://127.0.0.1:61347")),
                                     base_url=front.origin) as client:
            await login(client, front)
            # This bounded body reaches the C decoder's depth limit on the
            # supported Python 3.12 runtime without altering process limits.
            response = await client.post("/host/toolkit/setup/callback", content=body,
                headers={"Origin": front.origin, "Content-Type": "application/json"})
            assert response.status_code == 400 and response.text == "Invalid Console request."
            assert observed == [] and delivered == []
    finally:
        await front.close()


TOOLKIT_LANDING_CSP = "default-src 'none'; script-src 'sha256-AlzvtVcRKoCs4IRppBW6S8w1qxYZGgAzUD3jY+7iSOM='; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'"


async def test_toolkit_callback_landing_strips_query_and_start_refreshes_same_login(tmp_path, monkeypatch, caplog):
    clock = {"now": float(int(console_auth.time.time()) - 10)}
    monkeypatch.setattr(console_auth.time, "time", lambda: clock["now"])
    observed, delivered = [], []
    landing = b'<script src="/host/toolkit/setup/panel.js"></script>'
    def native(request):
        observed.append(request)
        if request.method == "GET":
            return httpx.Response(200, content=landing, headers={"Content-Type": "text/html", "Content-Security-Policy": TOOLKIT_LANDING_CSP})
        return httpx.Response(202, json={"forwarded": True})
    front = ConsoleFront(tmp_path, 8855, "owner", "local-password-123", delivered.append,
        transport=httpx.MockTransport(native))
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=front.application("http://127.0.0.1:61347")),
                                     base_url=front.origin) as client:
            path = "/host/toolkit/setup/callback"
            query = "state=opaque-private-state&code=opaque-private-code&state=second"
            public = await client.get(path + "?" + query, headers={"Origin": "https://provider.example",
                "Authorization": "Bearer browser-canary"})
            assert public.status_code == 200 and public.content == landing
            assert not {"authorization", "cookie", "origin"} & set(observed[-1].headers)
            assert observed[-1].url.raw_path == path.encode(), "The fixed callback landing must have no query."
            assert public.headers["content-security-policy"] == TOOLKIT_LANDING_CSP
            assert not front._logins and not front.sessions
            assert (await client.post(path, json={"query": query}, headers={"Origin": front.origin})).status_code == 401
            await login(client, front)
            actual_login = front._logins[client.cookies[front.cookie]]
            first = await client.get(path + "?" + query)
            assert first.status_code == 200 and first.content == landing
            assert observed[-1].method == "GET" and observed[-1].url.path == path
            assert not observed[-1].url.query and not observed[-1].content
            assert "Origin" not in observed[-1].headers
            assert "Authorization" not in observed[-1].headers, "Even a current login must not be borrowed by public fixed GET"
            assert first.headers["content-security-policy"] == TOOLKIT_LANDING_CSP
            await client.get("/host/toolkit/setup/data")
            first_token = observed[-1].headers["Authorization"].removeprefix("Bearer ")
            first_claims = jwt.decode(first_token, front.key, algorithms=["HS256"], issuer=ISSUER, audience=AUDIENCE)
            clock["now"] += 1
            response = await client.post("/host/toolkit/setup/start", json={"review_id": 41},
                headers={"Origin": front.origin, "Authorization": "Bearer browser-canary"})
            assert response.status_code == 202 and response.json() == {"forwarded": True}
            second_token = observed[-1].headers["Authorization"].removeprefix("Bearer ")
            second_claims = jwt.decode(second_token, front.key, algorithms=["HS256"], issuer=ISSUER, audience=AUDIENCE)
            assert first_token != second_token and second_token != "browser-canary"
            assert first_claims["sid"] == second_claims["sid"] == actual_login.login_scope_id
            assert second_claims["iat"] == first_claims["iat"] + 1
            assert observed[-1].headers["Origin"] == front.origin
            assert json.loads(observed[-1].content) == {"review_id": 41} and not observed[-1].url.query
            callback = await client.post(path, json={"query": query}, headers={"Origin": front.origin})
            assert callback.status_code == 202 and json.loads(observed[-1].content) == {"query": query}
            assert all(not item.url.query and front.cookie not in item.headers.get("Cookie", "") for item in observed)
            assert delivered == [] and "opaque-private" not in caplog.text
            assert first.headers["referrer-policy"] == "no-referrer" and first.headers["cache-control"] == "no-store"
    finally:
        await front.close()


@pytest.mark.parametrize("route,method,payload", [
    ("/host/toolkit/setup/start", "POST", {"review_id": 41}),
])
async def test_toolkit_new_account_routes_withhold_late_body_after_original_login_retirement(tmp_path, route, method, payload):
    entered, release = asyncio.Event(), asyncio.Event()
    observed, tasks = [], []
    class NativeBody(httpx.AsyncByteStream):
        async def __aiter__(self):
            entered.set()
            await release.wait()
            yield b"late-private-native-account-response"
    def native(request):
        observed.append(request)
        return httpx.Response(202 if method == "POST" else 200, stream=NativeBody(),
                              headers={"Content-Type": "application/json" if method == "POST" else "text/html"})
    front = ConsoleFront(tmp_path, 8855, "owner", "local-password-123", transport=httpx.MockTransport(native))
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=front.application("http://127.0.0.1:61347")),
                                     base_url=front.origin) as client:
            await login(client, front)
            pending = asyncio.create_task(client.request(method, route, json=payload,
                headers={"Origin": front.origin} if method == "POST" else {}))
            tasks.append(pending)
            await asyncio.wait_for(entered.wait(), 30)
            assert not pending.done() and len(observed) == 1
            assert (await client.post("/logout", headers={"Origin": front.origin})).status_code == 303
            release.set()
            response = await asyncio.wait_for(pending, 1)
            assert response.status_code == 401 and response.json() == {"error": "Console login required."}
            assert b"late-private" not in response.content and len(observed) == 1
    finally:
        release.set()
        await finish_stream_tasks(tasks)
        await front.close()


@pytest.mark.parametrize("status,content_type,policy", [
    (503, "text/html", TOOLKIT_LANDING_CSP),
    (200, "application/json", TOOLKIT_LANDING_CSP),
    (200, "text/html", "frame-ancestors 'none'"),
    (200, "text/html", "default-src *; script-src 'unsafe-inline'"),
])
async def test_toolkit_public_callback_only_forwards_fixed_landing_not_native_refusals_or_relaxed_policy(tmp_path, status, content_type, policy):
    observed = []
    def native(request):
        observed.append(request)
        return httpx.Response(status, content=b"private-native-response-canary", headers={
            "Content-Type": content_type, "Content-Security-Policy": policy,
            "Location": "https://unselected.example", "Set-Cookie": "borrowed=canary"})
    front = ConsoleFront(tmp_path, 8855, "owner", "local-password-123", transport=httpx.MockTransport(native))
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=front.application("http://127.0.0.1:61347")),
                                     base_url=front.origin) as client:
            path = "/host/toolkit/setup/callback"
            assert (await client.get(path + "?" + "x" * (16384 + 1))).status_code == 413
            assert (await client.get(path + "?state=opaque", headers={"Host": "unselected.example"})).status_code == 400
            assert observed == []
            response = await client.get(path + "?state=opaque&auth_token=opaque", headers={"Origin": "https://provider.example"})
            assert response.status_code == 502 and b"private-native-response-canary" not in response.content
            assert len(observed) == 1 and not observed[0].url.query
            assert not {"authorization", "cookie", "origin"} & set(observed[0].headers)
            assert not {"location", "set-cookie"} & set(response.headers)
            assert not front.sessions and not front._logins
    finally:
        await front.close()


async def test_toolkit_public_callback_does_not_enter_expired_login_housekeeping_or_viewer_lookup(tmp_path, monkeypatch):
    observed = []
    def native(request):
        observed.append(request)
        return httpx.Response(200, content=b"fixed-public-landing", headers={
            "Content-Type": "text/html", "Content-Security-Policy": TOOLKIT_LANDING_CSP})
    def forbidden(*args, **kwargs):
        raise AssertionError("Public landing cannot look up, mint or retire a Human owner.")
    front = ConsoleFront(tmp_path, 8855, "owner", "local-password-123", transport=httpx.MockTransport(native))
    app = front.application("http://127.0.0.1:61347")
    front.sessions["expired-original"] = console_auth.time.time() - 1
    front._logins["expired-original"] = console_auth._ConsoleLogin("expired-original", front.sessions["expired-original"])
    try:
        with monkeypatch.context() as guard:
            guard.setattr(front, "_live_login", forbidden)
            guard.setattr(front, "_retire_login", forbidden)
            guard.setattr(console_auth, "_token", forbidden)
            async with httpx.AsyncClient(transport=httpx.ASGITransport(app=app), base_url=front.origin) as client:
                result = await client.get("/host/toolkit/setup/callback?state=opaque&code=opaque",
                    headers={"Origin": "https://provider.example", "Authorization": "Bearer browser-canary"})
                assert result.status_code == 200 and result.content == b"fixed-public-landing"
                assert not {"authorization", "cookie", "origin"} & set(observed[0].headers)
                assert "expired-original" in front.sessions
    finally:
        await front.close()
