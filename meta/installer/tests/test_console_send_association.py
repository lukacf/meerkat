"""Original HTTP send association against the existing private subprocess peer.

This exercises host/pipe custody, not native Toolkit approval or effect truth.
"""
import asyncio
import json

import httpx
import pytest

from launchkit.templates_v2.console_auth import ConsoleFront
from test_console_auth import (connect_ready, connect_view, finish_stream_tasks,
                               login, managed_connect_factory)


PREFIX = "/console/forge/connect"
A = "b:" + "a" * 32
B = "b:" + "b" * 32


async def finish_original(client, app, front, tasks):
    view, frames, _ = await connect_view(app, front, client.cookies[front.cookie], private=True)
    tasks.append(view)
    while (prompt := await asyncio.wait_for(frames.get(), 1))["type"] != "prompt":
        pass
    response = await client.post(PREFIX + "/answer", json={"prompt_id": prompt["id"],
        "answer": {"kind": "approval", "decision": "approve"}}, headers={"Origin": front.origin})
    assert response.status_code == 202


async def settled(client, association):
    for _ in range(200):
        response = await client.get(PREFIX + "/send/" + association)
        assert response.status_code == 200
        receipt = response.json()
        if receipt["status"] == "completed":
            return receipt
        await asyncio.sleep(0.005)
    raise AssertionError("The original fixture send did not complete.")


def wire_sends(trace):
    return [value for row in trace.read_text().splitlines()
            if (value := json.loads(row)).get("method") == "connect/send"]


async def test_same_login_other_tab_cannot_resolve_unknown_association_or_repeat_original(tmp_path):
    factory, children, _, traces = managed_connect_factory(tmp_path)
    front = ConsoleFront(tmp_path, 8855, "reader", "local-password-123", connect_factory=factory)
    app, tasks = front.application("http://127.0.0.1:61347"), []
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=app), base_url=front.origin) as a:
            await login(a, front); await connect_ready(a, front)
            async with httpx.AsyncClient(transport=httpx.ASGITransport(app=app), base_url=front.origin,
                                         cookies=a.cookies) as b:
                unknown = {"association": A, "status": "unknown"}
                assert (await a.get(PREFIX + "/send/" + A)).json() == unknown
                competing = await asyncio.gather(*(client.post(PREFIX + "/send",
                    json={"text": "identical draft", "association": B}, headers={"Origin": front.origin})
                    for client in (a, b)))
                assert sorted(response.status_code for response in competing) == [202, 400]
                sent = next(response for response in competing if response.status_code == 202)
                assert sent.status_code == 202
                original = sent.json()
                assert original["association"] == B and original["status"] == "pending"
                identity = original["request_id"]
                assert (await a.get(PREFIX + "/send/" + A)).json() == unknown
                assert (await b.get(PREFIX + "/send/" + B)).json() == original
                for text in ("identical draft", "changed text cannot rebind"):
                    refusal = await a.post(PREFIX + "/send", json={"text": text, "association": B},
                                           headers={"Origin": front.origin})
                    assert refusal.status_code == 400 and refusal.json() == {"error": {"code": "invalid_message"}}
                await finish_original(b, app, front, tasks)
                result = await settled(a, B)
                assert result["request_id"] == identity == result["result"]["send_request_id"]
                assert result["result"]["completion"] == "completed"
                assert (await a.get(PREFIX + "/send/" + A)).json() == unknown
                for text in ("identical draft", "changed after completion"):
                    assert (await a.post(PREFIX + "/send", json={"text": text, "association": B},
                                        headers={"Origin": front.origin})).status_code == 400
                assert (await a.get(PREFIX + "/send/" + B)).json() == result
    finally:
        await front.close(); await finish_stream_tasks(tasks)
    sends = wire_sends(traces[0])
    assert len(sends) == 1 and len(children) == 1
    assert sends[0]["id"] == identity
    assert set(sends[0]["params"]) == {"scope", "text"}
    assert sends[0]["params"]["text"] == "identical draft"
    assert children[0]._process.returncode == 0


async def test_entered_send_lost_http_reply_observes_only_original_future_and_login(tmp_path):
    factory, children, _, traces = managed_connect_factory(tmp_path)
    front = ConsoleFront(tmp_path, 8855, "reader", "local-password-123", connect_factory=factory)
    app, tasks = front.application("http://127.0.0.1:61347"), []
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=app), base_url=front.origin) as client:
            await login(client, front); await connect_ready(client, front)
            body = json.dumps({"text": "entered once", "association": A}).encode()
            response_entered = asyncio.Event()
            async def receive():
                return {"type": "http.request", "body": body, "more_body": False}
            async def lose_reply(message):
                response_entered.set()
                await asyncio.Event().wait()
            scope = {"type": "http", "asgi": {"version": "3.0", "spec_version": "2.3"},
                "http_version": "1.1", "method": "POST", "scheme": "http",
                "path": PREFIX + "/send", "raw_path": (PREFIX + "/send").encode(),
                "query_string": b"", "root_path": "", "headers": [
                    (b"host", front.origin.removeprefix("http://").encode()),
                    (b"origin", front.origin.encode()), (b"content-type", b"application/json"),
                    (b"cookie", (front.cookie + "=" + client.cookies[front.cookie]).encode())],
                "client": ("127.0.0.1", 10003), "server": ("127.0.0.1", front.port)}
            task = asyncio.create_task(app(scope, receive, lose_reply)); tasks.append(task)
            await asyncio.wait_for(response_entered.wait(), 30)
            task.cancel(); await asyncio.gather(task, return_exceptions=True)
            original = (await client.get(PREFIX + "/send/" + A)).json()
            assert original["status"] == "pending" and original["association"] == A
            identity = original["request_id"]
            assert (await client.get(PREFIX + "/send/" + B)).json() == {"association": B, "status": "unknown"}
            await finish_original(client, app, front, tasks)
            result = await settled(client, A)
            assert result["request_id"] == identity == result["result"]["send_request_id"]
            assert result["result"]["completion"] == "completed"
            assert (await client.get(PREFIX + "/result/" + identity)).json() == {
                key: value for key, value in result.items() if key != "association"}
            async with httpx.AsyncClient(transport=httpx.ASGITransport(app=app), base_url=front.origin) as other:
                assert (await other.get(PREFIX + "/send/" + A)).status_code == 401
                await login(other, front); await connect_ready(other, front)
                assert (await other.get(PREFIX + "/send/" + A)).json() == {"association": A, "status": "unknown"}
            assert (await client.post("/logout", headers={"Origin": front.origin})).status_code == 303
            assert (await client.get(PREFIX + "/send/" + A)).status_code == 401
    finally:
        await front.close(); await finish_stream_tasks(tasks)
    sends = wire_sends(traces[0])
    assert len(sends) == 1 and sends[0]["id"] == identity
    assert set(sends[0]["params"]) == {"scope", "text"}
    assert all(child._process.returncode == 0 for child in children)


@pytest.mark.parametrize("association", [None, True, 1, "", "b:" + "A" * 32,
                                        "b:" + "a" * 31, "h:" + "a" * 32,
                                        "b:" + "a" * 32 + "\n"])
async def test_invalid_association_is_refused_before_child_creation(tmp_path, association):
    factory, children, _, _ = managed_connect_factory(tmp_path)
    front = ConsoleFront(tmp_path, 8855, "reader", "local-password-123", connect_factory=factory)
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=front.application("http://127.0.0.1:61347")),
                                     base_url=front.origin) as client:
            await login(client, front)
            response = await client.post(PREFIX + "/send", json={"text": "not entered", "association": association},
                                         headers={"Origin": front.origin})
            assert response.status_code == 400 and response.json() == {"error": {"code": "invalid_message"}}
    finally:
        await front.close()
    assert children == []


async def test_association_observation_is_get_only_closed_and_legacy_send_still_works(tmp_path):
    factory, children, _, traces = managed_connect_factory(tmp_path)
    front = ConsoleFront(tmp_path, 8855, "reader", "local-password-123", connect_factory=factory)
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=front.application("http://127.0.0.1:61347")),
                                     base_url=front.origin) as client:
            await login(client, front); await connect_ready(client, front)
            assert (await client.get(PREFIX + "/send/" + A + "?scope=forged")).status_code == 400
            assert (await client.post(PREFIX + "/send/" + A, json={}, headers={"Origin": front.origin})).status_code == 405
            assert (await client.get(PREFIX + "/send/" + A + "/extra")).status_code == 404
            assert (await client.post(PREFIX + "/send", json={"text": "not entered", "association": A, "scope": "forged"},
                                      headers={"Origin": front.origin})).status_code == 400
            legacy = await client.post(PREFIX + "/send", json={"text": "legacy original"}, headers={"Origin": front.origin})
            assert legacy.status_code == 202 and set(legacy.json()) == {"request_id", "status"}
            assert legacy.json()["status"] == "pending"
            assert (await client.get(PREFIX + "/send/" + A)).json() == {"association": A, "status": "unknown"}
    finally:
        await front.close()
    sends = wire_sends(traces[0])
    assert len(sends) == 1 and sends[0]["params"]["text"] == "legacy original"
    assert set(sends[0]["params"]) == {"scope", "text"}
    assert children[0]._process.returncode == 0
