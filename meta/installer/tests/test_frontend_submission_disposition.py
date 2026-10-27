"""Authenticated submission disposition boundaries with inert native transport.

These controls exercise the original HTTP, protected approval and generated
application hook. They do not start a server, runtime, provider or native CLI.
"""
from __future__ import annotations

import asyncio
from contextlib import asynccontextmanager
from copy import deepcopy
from dataclasses import replace

import httpx
import pytest

from launchkit.templates_v2 import console_auth as console_module
from test_console_auth import DESCRIPTOR_PATH, login
from test_frontend_management import HOST, PATH, Owner, boundary, prepare, submission
from test_frontend_private_declarations import _package, _wire

OFFER = "X-Forge-Frontend-Submission-Outcome"
OBSERVATION = {"status": "unknown", "job": {"status": "running"},
               "reply": {"complete": False}, "owner_extension": ["retained"]}


class SubmissionOwner(Owner):
    def __init__(self, result):
        super().__init__()
        self.result = result

    async def __call__(self, request):
        if request.operation != "submit":
            return await super().__call__(request)
        self.calls.append(request)
        result = self.result(request)
        return await result if asyncio.iscoroutine(result) else result


def typed_result(request, *, disposition="known", observation=None):
    selected = (console_module.FrontendSubmissionDisposition.KNOWN if disposition == "known"
                else console_module.FrontendSubmissionDisposition.UNKNOWN)
    return console_module.FrontendSubmissionResult(
        deepcopy(OBSERVATION if observation is None else observation), deepcopy(request.host),
        request.payload["proposal"]["change_id"], selected)


async def submit(client, front, body, offer=True):
    headers = {"Origin": front.origin}
    if offer:
        headers[OFFER] = "1"
    return await client.post(PATH + "/submit", json=body, headers=headers)


@pytest.mark.parametrize("disposition,status", [("known", "unknown"), ("unknown", "succeeded")])
async def test_explicit_disposition_does_not_interpret_opaque_job_or_reply_state(tmp_path, disposition, status):
    observation = {**deepcopy(OBSERVATION), "status": status}
    owner = SubmissionOwner(lambda request: typed_result(request, disposition=disposition, observation=observation))
    async with boundary(tmp_path, owner) as (client, front, native):
        await login(client, front)
        prepared = await prepare(client, front)
        response = await submit(client, front, submission(prepared))
        assert response.status_code == 200, response.text
        assert response.json() == {"protocol": 1, "host": HOST, "observation": observation,
            "submission_outcome": {"change_id": prepared["proposal"]["change_id"], "disposition": disposition}}
        assert response.headers["cache-control"] == "no-store"
        assert [request.operation for request in owner.calls] == ["prepare", "submit"]
        assert owner.calls[-1].approved is True and owner.calls[-1].principal == "owner"
        assert owner.calls[-1].payload == {"proposal": prepared["proposal"], "approval": {
            "principal": "owner", "expires_at": prepared["approval"]["expires_at"]}}
        assert native == []


async def test_legacy_dictionary_cannot_promote_status_or_nested_claim_to_disposition(tmp_path):
    observation = {"status": "succeeded", "submission_outcome": {"change_id": "change:one", "disposition": "known"}}
    owner = SubmissionOwner(lambda request: deepcopy(observation))
    async with boundary(tmp_path, owner) as (client, front, native):
        await login(client, front)
        response = await submit(client, front, submission(await prepare(client, front)))
        assert response.status_code == 200, response.text
        assert response.json() == {"protocol": 1, "host": HOST, "observation": observation}
        assert native == []


async def test_typed_submission_keeps_exact_legacy_envelope_without_offer(tmp_path):
    owner = SubmissionOwner(typed_result)
    async with boundary(tmp_path, owner) as (client, front, native):
        await login(client, front)
        response = await submit(client, front, submission(await prepare(client, front)), offer=False)
        assert response.status_code == 200, response.text
        assert response.json() == {"protocol": 1, "host": HOST, "observation": OBSERVATION}
        assert native == []


@pytest.mark.parametrize("mutation", ["change-id", "source", "boolean-protocol", "disposition"])
async def test_invalid_typed_result_cannot_disclose_a_known_outcome(tmp_path, mutation):
    def result(request):
        selected = typed_result(request)
        if mutation == "change-id":
            return replace(selected, change_id="change:other")
        if mutation == "disposition":
            return replace(selected, disposition="known")
        host = deepcopy(selected.host)
        host["source_digest" if mutation == "source" else "protocol"] = "3" * 64 if mutation == "source" else True
        return replace(selected, host=host)
    owner = SubmissionOwner(result)
    async with boundary(tmp_path, owner) as (client, front, native):
        await login(client, front)
        prepared = await prepare(client, front)
        response = await submit(client, front, submission(prepared))
        assert response.status_code == 503, response.text
        assert response.json()["status"] == "unknown"
        assert response.json()["change_id"] == prepared["proposal"]["change_id"]
        assert "submission_outcome" not in response.json() and "observation" not in response.json()
        assert [request.operation for request in owner.calls] == ["prepare", "submit"]
        assert native == []


@pytest.mark.parametrize("change", ["source", "logout"])
async def test_suspended_submission_rechecks_source_and_same_human_login(tmp_path, change):
    entered, release = asyncio.Event(), asyncio.Event()
    async def result(request):
        selected = typed_result(request)
        entered.set()
        await release.wait()
        return selected
    owner, host = SubmissionOwner(result), deepcopy(HOST)
    async with boundary(tmp_path, owner, host) as (client, front, native):
        await login(client, front)
        prepared = await prepare(client, front)
        pending = asyncio.create_task(submit(client, front, submission(prepared)))
        try:
            await asyncio.wait_for(entered.wait(), 30)
            if change == "source":
                host["source_digest"] = "3" * 64
            else:
                assert (await client.post("/logout", headers={"Origin": front.origin})).status_code == 303
            release.set()
            response = await asyncio.wait_for(pending, 1)
            assert response.status_code == (503 if change == "source" else 401), response.text
            assert "submission_outcome" not in response.json() and "observation" not in response.json()
            if change == "source":
                assert response.json()["status"] == "unknown"
                assert response.json()["change_id"] == prepared["proposal"]["change_id"]
            assert [request.operation for request in owner.calls] == ["prepare", "submit"]
            assert native == []
        finally:
            release.set()
            await asyncio.gather(pending, return_exceptions=True)


@pytest.mark.parametrize("values", [("0",), ("1, 1",), (" 1",), ("1", "1")],
                         ids=["unsupported", "combined", "whitespace", "duplicate"])
async def test_only_exact_singleton_offer_negotiates_the_extension(tmp_path, values):
    owner = SubmissionOwner(typed_result)
    async with boundary(tmp_path, owner) as (client, front, native):
        await login(client, front)
        body = submission(await prepare(client, front))
        response = await client.post(PATH + "/submit", json=body,
            headers=[("Origin", front.origin), *((OFFER, value) for value in values)])
        assert response.status_code == 200, response.text
        assert response.json() == {"protocol": 1, "host": HOST, "observation": OBSERVATION}
        assert [request.operation for request in owner.calls] == ["prepare", "submit"]
        assert native == []


async def test_offer_does_not_change_descriptor_read_status_or_protected_prepare(tmp_path):
    owner = SubmissionOwner(typed_result)
    async with boundary(tmp_path, owner) as (client, front, native):
        await login(client, front)
        descriptor = await client.get(DESCRIPTOR_PATH, headers={OFFER: "1"})
        inventory = await client.get(PATH, headers={OFFER: "1"})
        status = await client.get(PATH + "/changes/change:one", headers={OFFER: "1"})
        prepared = await client.post(PATH + "/prepare", json={"configuration": {"component": "inert"},
            "expected_revision": 7}, headers={"Origin": front.origin, OFFER: "1"})
        assert descriptor.status_code == inventory.status_code == status.status_code == prepared.status_code == 200
        assert set(descriptor.json()) == {"protocol", "host", "private_fields"}
        assert set(inventory.json()) == set(status.json()) == {"protocol", "host", "observation"}
        assert set(prepared.json()) == {"protocol", "host", "proposal", "approval"}
        assert inventory.json()["observation"] == {"status": "unknown", "operation": "inventory"}
        assert status.json()["observation"] == {"status": "unknown", "operation": "status", "change_id": "change:one"}
        assert all("submission_outcome" not in reply.json() for reply in (descriptor, inventory, status, prepared))
        assert [request.operation for request in owner.calls] == ["inventory", "status", "prepare"]
        assert all(not request.approved for request in owner.calls)
        assert native == []


APPLICATION = '''from dataclasses import replace
class Integration:
    def __init__(self, context):
        self.context = context
        self.commands = []
        self.known = True
        self.mode = "ordinary"
        self.last_result = None
    def frontend_management(self, request):
        self.commands.append(request)
        if request.operation == "prepare":
            return {"proposal": {"change_id": "change:application", "configuration": request.payload["configuration"],
                                 "expected_revision": request.payload["expected_revision"]}}
        if request.operation != "submit":
            return {"status": "unknown"}
        observation = {"status": "unknown", "job": {"status": "running"}, "reply": {"complete": False}}
        command, known = request, self.known
        if self.mode == "unapproved":
            command = replace(request, approved=False)
        elif self.mode == "inventory":
            command = replace(request, operation="inventory")
        elif self.mode in {"foreign-source", "boolean-protocol"}:
            host = dict(request.host)
            host["source_digest" if self.mode == "foreign-source" else "protocol"] = "3" * 64 if self.mode == "foreign-source" else True
            command = replace(request, host=host)
        elif self.mode == "nonboolean":
            known = 1
        elif self.mode == "nonobject":
            observation = ["not-an-observation-object"]
        self.last_result = self.context.frontend_submission_result(command, observation, known=known)
        return self.last_result

def build(context):
    return Integration(context)
'''


@asynccontextmanager
async def application_boundary(tmp_path, monkeypatch):
    host, application, fields = _package(tmp_path, monkeypatch)
    (tmp_path / "application/__init__.py").write_text(APPLICATION)
    native, front = [], None
    try:
        await application.load()
        wired = _wire(host, application, fields)
        # Use the generated ConsoleFront so both sides share the actual trusted
        # request/result types imported by the generated application host.
        front = host.ConsoleFront(tmp_path, 8855, "owner", "local-password-123",
            transport=httpx.MockTransport(lambda request: native.append(request) or httpx.Response(200)))
        front.frontend_management = wired.frontend_management
        front.frontend_host = wired.frontend_host
        front.frontend_private_fields = wired.frontend_private_fields
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=front.application("http://127.0.0.1:61347")),
                                     base_url=front.origin) as client:
            yield client, front, native, application, host
    finally:
        if front is not None:
            await front.close()
        await application.stop()


@pytest.mark.parametrize("known", [True, False], ids=["known", "unknown"])
async def test_loaded_application_helper_binds_original_http_approved_command(tmp_path, monkeypatch, known):
    async with application_boundary(tmp_path, monkeypatch) as (client, front, native, application, host):
        application.integration.known = known
        await login(client, front)
        prepared = await prepare(client, front)
        response = await submit(client, front, submission(prepared))
        assert response.status_code == 200, response.text
        assert response.json() == {"protocol": 1, "host": application.frontend_host(),
            "observation": {"status": "unknown", "job": {"status": "running"}, "reply": {"complete": False}},
            "submission_outcome": {"change_id": prepared["proposal"]["change_id"],
                "disposition": "known" if known else "unknown"}}
        commands = application.integration.commands
        assert [command.operation for command in commands] == ["prepare", "submit"]
        assert commands[-1].approved is True and commands[-1].host == application.frontend_host()
        selected = application.integration.last_result
        assert type(selected) is host.FrontendSubmissionResult
        assert selected.change_id == commands[-1].payload["proposal"]["change_id"]
        assert selected.disposition is (host.FrontendSubmissionDisposition.KNOWN if known
                                        else host.FrontendSubmissionDisposition.UNKNOWN)
        assert native == [] and application.context.runtime is None


@pytest.mark.parametrize("mode", ["unapproved", "inventory", "foreign-source", "boolean-protocol", "nonboolean", "nonobject"])
async def test_application_helper_refuses_to_relabel_an_invalid_original_submission(tmp_path, monkeypatch, mode):
    async with application_boundary(tmp_path, monkeypatch) as (client, front, native, application, host):
        application.integration.mode = mode
        await login(client, front)
        prepared = await prepare(client, front)
        response = await submit(client, front, submission(prepared))
        assert response.status_code == 503, response.text
        assert response.json()["status"] == "unknown"
        assert response.json()["change_id"] == prepared["proposal"]["change_id"]
        assert "submission_outcome" not in response.json() and "observation" not in response.json()
        assert [command.operation for command in application.integration.commands] == ["prepare", "submit"]
        assert application.integration.commands[-1].approved is True
        assert application.integration.last_result is None
        assert native == [] and application.context.runtime is None
