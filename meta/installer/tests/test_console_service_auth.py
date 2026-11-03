"""Authenticated pre-session relay controls, using a local wire peer only.

The peer has no native runtime, provider, credential store, resource preparer,
account, approval owner or service effects. These controls do not enable the
proposed service composition in generated applications.
"""
import asyncio
import json
import sys

import httpx
import pytest

from launchkit.templates_v2.console_auth import ConsoleFront
from launchkit.templates_v2.console_connect import ConsoleConnect
from test_console_auth import connect_view, finish_stream_tasks, login


SERVICE_HOST_PEER = r'''
import json, os, sys
def reply(identity, result):
    print(json.dumps(dict(jsonrpc="2.0", id=identity, result=result)), flush=True)
initialize_id = None
for line in sys.stdin:
    frame = json.loads(line)
    with open(os.environ["SERVICE_HOST_TRACE"], "a") as output:
        output.write(json.dumps(frame) + "\n")
    identity, method, params = frame["id"], frame.get("method"), frame.get("params", {})
    if method == "connect/initialize":
        assert params["protocol_version"] == 3 and params["adapter"] == "forge_services"
        initialize_id = identity
        print(json.dumps(dict(jsonrpc="2.0", id="p:provider-key", method="connect/prompt",
            params=dict(target=dict(initialize_request_id=identity), prompt=dict(
                kind="provider_api_key", target=dict(model="gpt-6.1-sol", provider="openai",
                    realm="native-selected-realm", binding="native-selected-binding"), max_bytes=8192)))), flush=True)
        # Initialization intentionally stays pending, so no scope/readiness is
        # fabricated for a provider operation that has not actually completed.
    elif method == "connect/retire":
        # This peer owns no entered service work. Close its retained prompt
        # before acknowledging the protocol3 settled retirement contract.
        print(json.dumps(dict(jsonrpc="2.0", method="connect/event", params=dict(
            target=params["target"], kind="prompt_closed", prompt_id="p:provider-key",
            disposition="cancelled"))), flush=True)
        if initialize_id is not None:
            print(json.dumps(dict(jsonrpc="2.0", id=initialize_id, error=dict(
                code=-32010, message="Synthetic initialization retired.",
                data=dict(code="binding_retired")))), flush=True)
            initialize_id = None
        reply(identity, dict(target=params["target"], retirement_id=params["retirement_id"],
            admission_closed=True, prompts_retired=True, entered_work="settled"))
    elif method == "connect/close":
        reply(identity, dict(target=params["target"], resources_closed=True))
        break
'''


def service_factory(tmp_path):
    binary = tmp_path / "service-host-peer"
    binary.write_text("#!" + sys.executable + "\n" + SERVICE_HOST_PEER)
    binary.chmod(0o700)
    children, traces = [], []

    def factory(context):
        trace = tmp_path / ("service-wire-" + str(len(children)) + ".jsonl")
        bootstrap = {
            "protocol_version": 3, "adapter": "forge_services",
            "application": {"application_id": "reading", "instance_id": "owned-app-instance",
                            "origin": context.origin},
            "viewer": {"principal_id": context.principal, "login_scope_id": context.login_scope_id,
                       "expires_at_ms": context.expires_at_ms},
            "realm": {"realm_id": "owned-fixture-realm", "state_root": str(tmp_path / "state"),
                      "context_root": str(tmp_path / "context"), "user_config_root": str(tmp_path / "user")},
            "services": {"resource_manifest_sha256": "ab" * 32, "preparation": "permitted"},
        }
        child = ConsoleConnect(binary, bootstrap, is_current=context.is_current,
            environment={"SERVICE_HOST_TRACE": str(trace)})
        children.append(child)
        traces.append(trace)
        return child

    return factory, children, traces


async def startup_binding(client):
    for _ in range(6000):
        response = await client.get("/console/forge/connect/state")
        assert response.status_code == 202
        value = response.json()
        if value.get("binding"):
            return value
        await asyncio.sleep(0.005)
    raise AssertionError("The original initialization target was not published.")


async def test_service_private_prompt_is_answerable_on_original_start_without_session(tmp_path):
    factory, children, traces = service_factory(tmp_path)
    front = ConsoleFront(tmp_path, 8855, "reader", "local-password-123", connect_factory=factory)
    app, tasks = front.application("http://127.0.0.1:61347"), []
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=app), base_url=front.origin) as client:
            await login(client, front)
            assert (await client.post("/console/forge/connect/initialize", json={},
                headers={"Origin": front.origin})).status_code == 202
            task, frames, disconnect = await connect_view(app, front, client.cookies[front.cookie], private=True)
            tasks.append(task)
            binding = await asyncio.wait_for(frames.get(), 1)
            prompt = await asyncio.wait_for(frames.get(), 1)
            assert binding == {"type": "binding", "protocol_version": 3,
                               "target": {"initialize_request_id": children[0]._initialize_id}}
            assert prompt["target"] == binding["target"] and prompt["id"] == "p:provider-key"
            assert prompt["protocol_version"] == 3 and prompt["prompt"]["kind"] == "provider_api_key"
            assert "scope" not in prompt and children[0]._scope is None
            assert not children[0]._initialized
            observed = await startup_binding(client)
            assert observed == {"status": "starting", "binding": binding}
            disconnect.set()
            await asyncio.wait_for(task, 1)
            task, remounted, _ = await connect_view(app, front, client.cookies[front.cookie], private=True)
            tasks.append(task)
            assert await asyncio.wait_for(remounted.get(), 1) == binding
            assert await asyncio.wait_for(remounted.get(), 1) == prompt
            body = {"prompt_id": prompt["id"], "answer": {"kind": "provider_api_key", "key": " fixture-key "}}
            forwarded = await client.post("/console/forge/connect/answer", json=body,
                headers={"Origin": front.origin})
            assert forwarded.status_code == 202 and forwarded.json() == {"status": "forwarded"}
            duplicate = await client.post("/console/forge/connect/answer", json=body,
                headers={"Origin": front.origin})
            assert duplicate.status_code == 409 and duplicate.json() == {"error": {"code": "stale_prompt"}}
            assert (await startup_binding(client))["binding"] == binding
    finally:
        await front.close()
        await finish_stream_tasks(tasks)
    rows = [json.loads(row) for row in traces[0].read_text().splitlines()]
    assert sum(row.get("method") == "connect/initialize" for row in rows) == 1
    answers = [row for row in rows if row.get("id") == "p:provider-key"]
    assert answers == [{"jsonrpc": "2.0", "id": "p:provider-key",
                        "result": {"answer": {"kind": "provider_api_key", "key": " fixture-key "}}}]
    assert children[0]._process.returncode == 0
    retire = next(row for row in rows if row.get("method") == "connect/retire")
    assert retire["params"]["target"] == binding["target"]


async def test_service_startup_prompt_is_private_login_owned_and_rejects_browser_authority(tmp_path):
    factory, children, traces = service_factory(tmp_path)
    front = ConsoleFront(tmp_path, 8855, "reader", "local-password-123", connect_factory=factory)
    app, tasks = front.application("http://127.0.0.1:61347"), []
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=app), base_url=front.origin) as first, \
                httpx.AsyncClient(transport=httpx.ASGITransport(app=app), base_url=front.origin) as second:
            await login(first, front)
            await login(second, front)
            headers = {"Origin": front.origin}
            changed = await first.post("/console/forge/connect/initialize",
                json={"services": {"preparation": "permitted"}}, headers=headers)
            assert changed.status_code == 400 and not children
            assert (await first.post("/console/forge/connect/initialize", json={}, headers=headers)).status_code == 202
            await startup_binding(first)
            task, frames, _ = await connect_view(app, front, first.cookies[front.cookie], private=True)
            tasks.append(task)
            await asyncio.wait_for(frames.get(), 1)
            prompt = await asyncio.wait_for(frames.get(), 1)
            public = await first.get("/console/forge/connect/events/public")
            assert public.status_code == 503 and "provider_api_key" not in public.text
            foreign = await second.post("/console/forge/connect/answer",
                json={"prompt_id": prompt["id"], "answer": {"kind": "cancel"}}, headers=headers)
            assert foreign.status_code == 503 and len(children) == 1
            invalid = await first.post("/console/forge/connect/answer",
                json={"prompt_id": prompt["id"], "answer": {"kind": "provider_api_key", "key": "bad\nkey"}},
                headers=headers)
            assert invalid.status_code == 400 and invalid.json() == {"error": {"code": "invalid_answer"}}
            assert prompt["id"] in children[0]._prompts
            assert (await first.post("/logout", headers=headers)).status_code == 303
            refused = await first.post("/console/forge/connect/answer",
                json={"prompt_id": prompt["id"], "answer": {"kind": "cancel"}}, headers=headers)
            assert refused.status_code == 401
    finally:
        await front.close()
        await finish_stream_tasks(tasks)
    rows = [json.loads(row) for row in traces[0].read_text().splitlines()]
    assert not any(row.get("id") == "p:provider-key" for row in rows)
    assert children[0]._process.returncode == 0
