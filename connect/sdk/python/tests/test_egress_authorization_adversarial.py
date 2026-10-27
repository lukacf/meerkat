"""Independent policy-entry checks using real SQLite and loopback transport.

The SQLite audience table is a host test fixture, not Toolkit/native authority.
Legacy restart remains an explicit adoption boundary in the current contract.
"""

from __future__ import annotations

import asyncio
from collections.abc import Mapping
from contextlib import asynccontextmanager
from dataclasses import FrozenInstanceError
import json
import os
from pathlib import Path
import sqlite3
import sys

import pytest

import meerkat_toolkit.frontend.egress as egress_module
from meerkat_toolkit.frontend.egress import (
    EgressAuthorizationDecision as Decision,
    ProactiveEgress,
)
from meerkat_toolkit.frontend.sender import SenderAuthority


def plain(value):
    if isinstance(value, Mapping):
        return {key: plain(item) for key, item in value.items()}
    if isinstance(value, (tuple, list)):
        return [plain(item) for item in value]
    return value


@asynccontextmanager
async def transport(outcomes=()):
    """Record actual HTTP entries and distinguish accepted from refused calls."""
    calls, accepted, failures = [], [], []
    pending = list(outcomes)

    async def handle(reader, writer):
        try:
            header = await asyncio.wait_for(reader.readuntil(b"\r\n\r\n"), 2)
            length = next(
                int(line.split(b":", 1)[1])
                for line in header.split(b"\r\n")
                if line.lower().startswith(b"content-length:")
            )
            call = json.loads(await reader.readexactly(length))
            calls.append(call)
            outcome = pending.pop(0) if pending else "sent"
            if outcome == "sent":
                accepted.append(call)
            response = json.dumps({
                "outcome": outcome,
                "provider_message_id": str(len(accepted)) if outcome == "sent" else None,
            }).encode()
            writer.write(b"HTTP/1.1 200 OK\r\nContent-Length: " + str(len(response)).encode()
                         + b"\r\nConnection: close\r\n\r\n" + response)
            await writer.drain()
        except Exception as error:
            failures.append(error)
        finally:
            writer.close()
            await writer.wait_closed()

    server = await asyncio.start_server(handle, "127.0.0.1", 0)
    port = server.sockets[0].getsockname()[1]

    async def send(**kwargs):
        body = json.dumps(plain(kwargs)).encode()
        reader, writer = await asyncio.open_connection("127.0.0.1", port)
        try:
            writer.write(b"POST /send HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: "
                         + str(len(body)).encode() + b"\r\n\r\n" + body)
            await writer.drain()
            response = await asyncio.wait_for(reader.read(), 2)
            return json.loads(response.split(b"\r\n\r\n", 1)[1])
        finally:
            writer.close()
            await writer.wait_closed()

    send.port = port
    try:
        yield send, calls, accepted
    finally:
        server.close()
        await server.wait_closed()
        assert not failures


class HostAudience:
    def __init__(self, path):
        self.path = path
        self.contexts = []
        with sqlite3.connect(path) as db:
            db.execute("CREATE TABLE IF NOT EXISTS audiences (issuer TEXT, logical_key TEXT, "
                       "sender TEXT, recipient TEXT, PRIMARY KEY(issuer,logical_key,sender,recipient))")

    def grant(self, *, issuer="issuer", key="answer", sender="s", recipient="permitted"):
        with sqlite3.connect(self.path) as db:
            db.execute("INSERT OR IGNORE INTO audiences VALUES (?,?,?,?)",
                       (issuer, key, sender, recipient))

    def revoke(self):
        with sqlite3.connect(self.path) as db:
            db.execute("DELETE FROM audiences")

    def __call__(self, context):
        self.contexts.append(context)
        with sqlite3.connect(self.path) as db:
            row = db.execute("SELECT 1 FROM audiences WHERE issuer=? AND logical_key=? "
                             "AND sender=? AND recipient=?",
                             (context.issuer_id, context.logical_key,
                              context.sender_id, context.recipient)).fetchone()
        return Decision.ALLOWED if row else Decision.DENIED


def owner(path, sender, policy, **kwargs):
    return ProactiveEgress(
        path=path, senders={"s": sender}, incident_sink=lambda _: None,
        health_sink=kwargs.pop("health_sink", lambda _: None),
        sleep=kwargs.pop("sleep", lambda _: None), authorization_mode="enforced",
        authorize_delivery=policy, **kwargs,
    )


def commit(engine, *, key="answer", recipient="permitted", parts=({"text": "original"},)):
    return engine.commit_output(
        issuer_id="issuer", logical_key=key, candidates=(("s", recipient),),
        parts=parts, incident_delivery=False,
    )


def rows(path):
    with sqlite3.connect(path) as db:
        return db.execute("SELECT part_index,state,attempts,unknown_attempts,last_outcome "
                          "FROM egress_parts ORDER BY part_index").fetchall()


async def test_current_audience_revocation_between_parts_blocks_second_effect(tmp_path):
    audience = HostAudience(tmp_path / "host.sqlite3")
    audience.grant()
    path = tmp_path / "outbox.sqlite3"
    async with transport() as (send, calls, accepted):
        async def first_then_revoke(**kwargs):
            result = await send(**kwargs)
            audience.revoke()
            return result

        engine = owner(path, first_then_revoke, audience)
        output = commit(engine, parts=({"text": "one"}, {"text": "two"}))
        await engine._dispatch_output(output)
        assert len(calls) == len(accepted) == 1
        assert [c.part_index for c in audience.contexts] == [0, 1]
        assert rows(path) == [(0, "sent", 1, 0, "provider_accepted"),
                              (1, "pending", 0, 0, None)]
        assert engine.status(output_id=output).delivery_state != "provider_accepted"


@pytest.mark.parametrize("recipient,key", [("foreign", "answer"), ("permitted", "unassociated")])
async def test_missing_exact_host_association_never_enters_transport(tmp_path, recipient, key):
    audience = HostAudience(tmp_path / "host.sqlite3")
    audience.grant()
    path = tmp_path / "outbox.sqlite3"
    async with transport() as (send, calls, accepted):
        engine = owner(path, send, audience)
        output = commit(engine, recipient=recipient, key=key)
        await engine._dispatch_output(output)
        await engine.run_recovery_pass(respect_grace=False)
        assert not calls and not accepted
        assert rows(path) == [(0, "pending", 0, 0, None)]


async def test_enforced_reopen_rechecks_policy_and_can_later_resume_same_output(tmp_path):
    audience = HostAudience(tmp_path / "host.sqlite3")
    audience.grant()
    path = tmp_path / "outbox.sqlite3"
    async with transport() as (send, calls, accepted):
        first = owner(path, send, audience)
        output = commit(first)
        audience.revoke()
        reopened = owner(path, send, HostAudience(audience.path))
        assert commit(reopened) == output
        await reopened.run_recovery_pass(respect_grace=False)
        assert not calls and rows(path) == [(0, "pending", 0, 0, None)]
        audience.grant()
        await reopened.run_recovery_pass(respect_grace=False)
        await reopened.run_recovery_pass(respect_grace=False)
        assert len(calls) == len(accepted) == 1
        assert reopened.status(output_id=output).delivery_state == "provider_accepted"


async def test_known_not_sent_backoff_cannot_reuse_previous_allow(tmp_path):
    audience = HostAudience(tmp_path / "host.sqlite3")
    audience.grant()
    path = tmp_path / "outbox.sqlite3"
    async with transport(["known_not_sent"]) as (send, calls, accepted):
        engine = owner(path, send, audience, sleep=lambda _: audience.revoke())
        output = commit(engine)
        await engine._dispatch_output(output)
        assert len(calls) == 1 and not accepted
        assert len(audience.contexts) == 2
        assert rows(path) == [(0, "pending", 1, 0, "known_not_sent")]


async def test_sender_mutation_does_not_change_reauthorized_retry_payload(tmp_path):
    path = tmp_path / "outbox.sqlite3"
    seen = []
    async with transport(["known_not_sent", "sent"]) as (send, calls, accepted):
        def authorize(context):
            seen.append((context.request_digest, plain(context.part)))
            return Decision.ALLOWED

        async def mutating_sender(**kwargs):
            result = await send(**kwargs)
            kwargs["part"]["text"] = "sender injected text"
            kwargs["part"]["entities"][0]["type"] = "sender injected entity"
            return result

        engine = owner(path, mutating_sender, authorize, normalize_parts=lambda p: p)
        output = commit(engine, parts=({"text": "original", "entities": [
            {"type": "bold", "offset": 0, "length": 8},
        ]},))
        await engine._dispatch_output(output)
        assert len(calls) == 2 and len(accepted) == 1
        assert seen[0] == seen[1]
        assert calls[0]["part"] == calls[1]["part"] == seen[0][1]
        assert rows(path) == [(0, "sent", 2, 0, "provider_accepted")]


async def test_authorizer_input_is_deeply_immutable_and_matches_wire(tmp_path):
    seen = []
    async with transport() as (send, calls, _):
        def authorize(context):
            with pytest.raises((FrozenInstanceError, AttributeError, TypeError)):
                context.recipient = "foreign"
            with pytest.raises(TypeError):
                context.part["text"] = "foreign"
            with pytest.raises(TypeError):
                context.part["entities"][0]["type"] = "foreign"
            seen.append(plain(context.part))
            return Decision.ALLOWED

        engine = owner(tmp_path / "outbox.sqlite3", send, authorize, normalize_parts=lambda p: p)
        output = commit(engine, parts=({"text": "original", "entities": [
            {"type": "bold", "offset": 0, "length": 8},
        ]},))
        await engine._dispatch_output(output)
        assert len(calls) == 1
        assert calls[0]["part"] == seen[0]


@pytest.mark.parametrize("result", [True, False, None, "allowed", {"allowed": True}, object()])
async def test_ambiguous_policy_return_is_not_permission(tmp_path, result):
    path = tmp_path / "outbox.sqlite3"
    async with transport() as (send, calls, _):
        engine = owner(path, send, lambda _: result)
        await engine._dispatch_output(commit(engine))
        assert not calls
        assert rows(path) == [(0, "pending", 0, 0, None)]


@pytest.mark.parametrize("failure", [
    "error", "timeout", "suppressed_timeout", "cancel", "suppressed_cancel",
])
async def test_async_policy_failure_does_not_reserve_or_leak(tmp_path, failure):
    path = tmp_path / "outbox.sqlite3"
    entered, exited = asyncio.Event(), asyncio.Event()
    health = []

    async def policy(_):
        entered.set()
        try:
            if failure == "error":
                raise RuntimeError("private policy credential sentinel")
            try:
                await asyncio.Event().wait()
            except asyncio.CancelledError:
                if failure in {"suppressed_timeout", "suppressed_cancel"}:
                    return Decision.ALLOWED
                raise
        finally:
            exited.set()

    async with transport() as (send, calls, _):
        engine = owner(path, send, policy, dispatch_timeout_seconds=0.03, health_sink=health.append)
        task = asyncio.create_task(engine._dispatch_output(commit(engine)))
        await asyncio.wait_for(entered.wait(), 1)
        if failure in {"cancel", "suppressed_cancel"}:
            task.cancel()
            with pytest.raises(asyncio.CancelledError):
                await task
        else:
            await asyncio.wait_for(task, 1)
        assert exited.is_set()
        assert not calls
        assert rows(path) == [(0, "pending", 0, 0, None)]
        assert "private policy credential sentinel" not in json.dumps(health)


async def test_policy_wait_cannot_authorize_an_expired_output(tmp_path):
    path = tmp_path / "outbox.sqlite3"

    async def policy(_):
        await asyncio.sleep(0.04)
        return Decision.ALLOWED

    async with transport() as (send, calls, _):
        engine = owner(path, send, policy, freshness_seconds=0.02, dispatch_timeout_seconds=1)
        await engine._dispatch_output(commit(engine))
        assert not calls
        assert rows(path) == [(0, "expired", 0, 0, None)]


async def test_two_enforced_handles_have_only_one_durable_provider_entry(tmp_path):
    path = tmp_path / "outbox.sqlite3"
    both, release = asyncio.Event(), asyncio.Event()
    authorizations = 0

    async def policy(_):
        nonlocal authorizations
        authorizations += 1
        if authorizations == 2:
            both.set()
        await release.wait()
        return Decision.ALLOWED

    async with transport() as (send, calls, accepted):
        first, second = owner(path, send, policy), owner(path, send, policy)
        output = commit(first)
        tasks = [asyncio.create_task(handle._dispatch_output(output)) for handle in (first, second)]
        await asyncio.wait_for(both.wait(), 1)
        release.set()
        await asyncio.gather(*tasks)
        assert len(calls) == len(accepted) == 1
        assert rows(path) == [(0, "sent", 1, 0, "provider_accepted")]


async def test_restart_without_enforced_configuration_is_documented_legacy_boundary(tmp_path):
    path = tmp_path / "outbox.sqlite3"
    async with transport() as (send, calls, _):
        first = owner(path, send, lambda _: Decision.DENIED)
        output = commit(first)
        await first._dispatch_output(output)
        assert not calls
        legacy = ProactiveEgress(path=path, senders={"s": send}, incident_sink=lambda _: None,
                                 health_sink=lambda _: None, sleep=lambda _: None)
        await legacy._dispatch_output(output)
        assert len(calls) == 1
        assert legacy.status(output_id=output).delivery_state == "provider_accepted"


@asynccontextmanager
async def blocked_sender_activation(authority, send):
    entered, release = asyncio.Event(), asyncio.Event()

    class Candidate:
        async def activate(self):
            entered.set()
            await release.wait()

        async def deactivate(self):
            pass

        async def __call__(self, **kwargs):
            return await send(**kwargs)

    handle = authority.stage_activation(sender_id="s", to_owner_id="owner", sender=Candidate())
    task = asyncio.create_task(authority.commit_activation(handle))
    try:
        await asyncio.wait_for(entered.wait(), 1)
        yield
    finally:
        release.set()
        await asyncio.wait_for(task, 1)


def observe_sender_selection(authority, monkeypatch):
    waiting = asyncio.Event()
    select = authority.select_dispatch

    async def observed(**kwargs):
        waiting.set()
        return await select(**kwargs)

    monkeypatch.setattr(authority, "select_dispatch", observed)
    return waiting


async def test_sender_transition_does_not_queue_a_previously_allowed_entry(tmp_path, monkeypatch):
    path = tmp_path / "outbox.sqlite3"
    audience = HostAudience(tmp_path / "host.sqlite3")
    audience.grant()
    checked = asyncio.Event()

    def policy(context):
        decision = audience(context)
        checked.set()
        return decision

    async with transport() as (send, calls, _):
        authority = SenderAuthority(path=tmp_path / "sender.sqlite3")
        authority.attach_active(sender_id="s", owner_id="owner", sender=send)
        waiting = observe_sender_selection(authority, monkeypatch)
        engine = owner(path, send, policy, sender_authority=authority)
        output = commit(engine)
        async with blocked_sender_activation(authority, send):
            task = asyncio.create_task(engine._dispatch_output(output))
            await asyncio.wait_for(waiting.wait(), 1)
            assert not checked.is_set() and not task.done()
            audience.revoke()
        await asyncio.wait_for(task, 1)
        assert not calls
        assert rows(path) == [(0, "pending", 0, 0, None)]


@pytest.mark.parametrize("failure", ["timeout", "cancel"])
async def test_waiting_for_sender_is_not_a_provider_attempt(tmp_path, failure, monkeypatch):
    path = tmp_path / "outbox.sqlite3"

    async with transport() as (send, calls, _):
        authority = SenderAuthority(path=tmp_path / "sender.sqlite3")
        authority.attach_active(sender_id="s", owner_id="owner", sender=send)
        waiting = observe_sender_selection(authority, monkeypatch)
        engine = owner(path, send, lambda _: Decision.ALLOWED, sender_authority=authority,
                       dispatch_timeout_seconds=0.03)
        async with blocked_sender_activation(authority, send):
            task = asyncio.create_task(engine._dispatch_output(commit(engine)))
            await asyncio.wait_for(waiting.wait(), 1)
            if failure == "cancel":
                task.cancel()
                with pytest.raises(asyncio.CancelledError):
                    await task
            else:
                await asyncio.wait_for(task, 1)
            assert not calls
            assert rows(path) == [(0, "pending", 0, 0, None)]


async def test_same_owner_reattachment_during_policy_wait_invalidates_selection(tmp_path):
    path = tmp_path / "outbox.sqlite3"
    entered, release = asyncio.Event(), asyncio.Event()

    async def policy(_):
        entered.set()
        await release.wait()
        return Decision.ALLOWED

    async with transport() as (send, calls, _):
        async def old_sender(**kwargs):
            return await send(route="old", **kwargs)

        async def replacement(**kwargs):
            return await send(route="replacement", **kwargs)

        authority = SenderAuthority(path=tmp_path / "sender.sqlite3")
        authority.attach_active(sender_id="s", owner_id="owner", sender=old_sender)
        engine = owner(path, old_sender, policy, sender_authority=authority)
        output = commit(engine)
        task = asyncio.create_task(engine._dispatch_output(output))
        await asyncio.wait_for(entered.wait(), 1)
        authority.attach_active(sender_id="s", owner_id="owner", sender=replacement)
        release.set()
        await asyncio.wait_for(task, 1)
        assert not calls
        assert rows(path) == [(0, "pending", 0, 0, None)]
        await engine._dispatch_output(output)
        assert len(calls) == 1 and calls[0]["route"] == "replacement"


async def test_corrupt_optional_formatting_authorizes_exact_reconstructed_wire(tmp_path):
    path = tmp_path / "outbox.sqlite3"
    contexts = []
    async with transport() as (send, calls, _):
        def policy(context):
            contexts.append(context)
            return Decision.ALLOWED

        engine = owner(path, send, policy, normalize_parts=lambda p: p)
        output = commit(engine, parts=({"text": "original", "entities": [
            {"type": "bold", "offset": 0, "length": 8},
        ]},))
        with sqlite3.connect(path) as db:
            db.execute("UPDATE egress_parts SET entities='invalid-json'")
            digest = db.execute("SELECT request_digest FROM egress_outputs").fetchone()[0]
        await engine._dispatch_output(output)
        assert len(contexts) == len(calls) == 1
        assert plain(contexts[0].part) == calls[0]["part"] == {"text": "original"}
        # Correlation is retained; this gate does not revalidate old request bytes.
        assert contexts[0].request_digest == digest


async def test_fresh_process_recovery_uses_current_host_policy(tmp_path):
    path = tmp_path / "outbox.sqlite3"
    audience = HostAudience(tmp_path / "host.sqlite3")
    source = r'''
import asyncio, json, sqlite3, sys
from pathlib import Path
import meerkat_toolkit.frontend.egress as egress_module
from meerkat_toolkit.frontend.egress import EgressAuthorizationDecision as D, ProactiveEgress
path, audience, port = sys.argv[1:]
def authorize(context):
    with sqlite3.connect(audience) as db:
        allowed = db.execute('SELECT 1 FROM audiences WHERE issuer=? AND logical_key=? AND sender=? AND recipient=?',
            (context.issuer_id,context.logical_key,context.sender_id,context.recipient)).fetchone()
    return D.ALLOWED if allowed else D.DENIED
async def sender(**kwargs):
    reader, writer = await asyncio.open_connection('127.0.0.1', int(port))
    try:
        body = json.dumps(kwargs).encode()
        writer.write(b'POST /send HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: '
            + str(len(body)).encode() + b'\r\n\r\n' + body)
        await writer.drain()
        reply = await reader.read()
        return json.loads(reply.split(b'\r\n\r\n',1)[1])
    finally:
        writer.close()
        await writer.wait_closed()
async def main():
    owner = ProactiveEgress(path=Path(path),senders={'s':sender},incident_sink=lambda _:None,
        health_sink=lambda _:None,sleep=lambda _:None,authorization_mode='enforced',authorize_delivery=authorize)
    output = owner.commit_output(issuer_id='issuer',logical_key='answer',candidates=(('s','permitted'),),
        parts=({'text':'original'},),incident_delivery=False)
    await owner.run_recovery_pass(respect_grace=False)
    print(json.dumps({'output_id':output,'attempts':owner.attempt_count(output_id=output),
        'state':owner.status(output_id=output).delivery_state,'origin':egress_module.__file__}))
asyncio.run(main())
'''

    async def child(port):
        process = await asyncio.create_subprocess_exec(
            sys.executable, "-B", "-c", source, str(path), str(audience.path), str(port),
            stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE, env=dict(os.environ),
        )
        stdout, stderr = await asyncio.wait_for(process.communicate(), 5)
        assert process.returncode == 0, stderr.decode()
        assert not stderr
        result = json.loads(stdout)
        assert Path(result["origin"]).resolve() == Path(egress_module.__file__).resolve()
        return result

    async with transport() as (send, calls, accepted):
        denied = await child(send.port)
        assert denied["state"] == "queued" and denied["attempts"] == 0
        assert not calls
        audience.grant()
        permitted = await child(send.port)
        assert permitted["state"] == "provider_accepted" and permitted["attempts"] == 1
        assert permitted["output_id"] == denied["output_id"]
        assert len(calls) == len(accepted) == 1
