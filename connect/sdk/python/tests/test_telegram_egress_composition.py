"""Independent SQLite/Telegram composition checks with actual loopback HTTP.

The small SQLite permission table and recipient mapping are host fixtures.
These tests do not exercise a live Telegram account or native permissions.
"""

from __future__ import annotations

import asyncio
from collections.abc import Mapping
from contextlib import asynccontextmanager
from email.parser import BytesParser
from email.policy import default
import json
from pathlib import Path
import sqlite3
import sys

import pytest

import meerkat_toolkit.frontend.egress as egress_module
from meerkat_toolkit.frontend.egress import EgressAuthorizationDecision as Decision
from meerkat_toolkit.frontend.egress import ProactiveEgress
from meerkat_toolkit.frontend.sender import SenderAuthority
from meerkat_toolkit.frontend.telegram import TelegramBotApiTransport
from meerkat_toolkit.frontend.telegram_adapter import TelegramPresenter


CHAT = "-10077"
TOPIC = 42
RECIPIENT = f"{CHAT}/{TOPIC}"
PHOTO = b"\x89PNG\r\nindependent-loopback-fixture"
REFUSAL = (400, {"ok": False, "error_code": 400, "description": "bad entities"})


def plain(value):
    if isinstance(value, Mapping):
        return {key: plain(item) for key, item in value.items()}
    if isinstance(value, (tuple, list)):
        return [plain(item) for item in value]
    return value


def part_for(kind):
    entities = [{"type": "bold", "offset": 0, "length": 5}]
    if kind == "text":
        return {"text": "hello", "entities": entities}
    return {"media_ref": "fixture://photo", "mime_type": "image/png",
            "caption": "hello", "caption_entities": entities}


def snapshot(path):
    with sqlite3.connect(path) as db:
        db.row_factory = sqlite3.Row
        return dict(db.execute(
            "SELECT p.*,o.request_digest,o.issuer_id,o.logical_key,o.sender_id,o.recipient "
            "FROM egress_parts p JOIN egress_outputs o USING(output_id)"
        ).fetchone())


class HostPolicy:
    def __init__(self, path, outbox):
        self.path, self.outbox, self.contexts = path, outbox, []
        with sqlite3.connect(path) as db:
            db.execute("CREATE TABLE IF NOT EXISTS permission (allowed INTEGER NOT NULL)")
            if not db.execute("SELECT 1 FROM permission").fetchone():
                db.execute("INSERT INTO permission VALUES (1)")

    def set_allowed(self, allowed):
        with sqlite3.connect(self.path) as db:
            db.execute("UPDATE permission SET allowed=?", (int(allowed),))

    def __call__(self, context):
        assert (context.issuer_id, context.logical_key, context.sender_id, context.recipient) == (
            "issuer/person", "reply/one", "telegram", RECIPIENT)
        self.contexts.append({"part": plain(context.part), "digest": context.request_digest,
                              "part_index": context.part_index,
                              "attempts": snapshot(self.outbox)["attempts"]})
        with sqlite3.connect(self.path) as db:
            allowed = db.execute("SELECT allowed FROM permission").fetchone()[0]
        return Decision.ALLOWED if allowed else Decision.DENIED


def telegram_sender(transport, *, mutation=None):
    """Explicit host composition, forwarding only the bounded presentation hint."""
    presenter, receipts = TelegramPresenter(), []

    async def send(*, sender_id, recipient, part_index, part):
        assert (sender_id, recipient, part_index) == ("telegram", RECIPIENT, 0)
        if "media_ref" in part:
            assert part["media_ref"] == "fixture://photo"
            receipt = await presenter.send_photo(
                transport, recipient=CHAT, message_thread_id=TOPIC, data=PHOTO,
                mime_type=part["mime_type"], caption=part.get("caption"),
                caption_entities=part.get("caption_entities", ()),
            )
        else:
            receipt = await presenter.send_text(
                transport, recipient=CHAT, message_thread_id=TOPIC,
                text=part["text"], entities=part.get("entities", ()),
            )
        receipts.append(receipt)
        if mutation is not None:
            mutation(part, receipt)
        return {"outcome": {"provider_accepted": "sent", "provider_queued": "unknown"}.get(
                    receipt.outcome, receipt.outcome),
                "provider_message_id": receipt.provider_message_id, "error": receipt.error,
                "retry_without_formatting": getattr(receipt, "retry_without_formatting", False)}

    send.receipts, send.presenter = receipts, presenter
    return send


def owner(path, send, policy, **kwargs):
    return ProactiveEgress(
        path=path, senders={"telegram": send}, incident_sink=lambda _: None,
        health_sink=lambda _: None, normalize_parts=lambda parts: parts,
        authorization_mode="enforced", authorize_delivery=policy,
        sleep=kwargs.pop("sleep", lambda _: None), **kwargs,
    )


def commit(engine, kind="text"):
    return engine.commit_output(
        issuer_id="issuer/person", logical_key="reply/one",
        candidates=(("telegram", RECIPIENT),), parts=(part_for(kind),),
        incident_delivery=False,
    )


def decode_wire(headers, body):
    content_type = headers["content-type"]
    if content_type.startswith("application/json"):
        return json.loads(body)
    message = BytesParser(policy=default).parsebytes(
        b"Content-Type: " + content_type.encode() + b"\r\nMIME-Version: 1.0\r\n\r\n" + body)
    fields = {}
    for item in message.iter_parts():
        name = item.get_param("name", header="content-disposition")
        data = item.get_payload(decode=True)
        fields[name] = data if name == "photo" else data.decode()
    if "caption_entities" in fields:
        fields["caption_entities"] = json.loads(fields["caption_entities"])
    return fields


@asynccontextmanager
async def telegram_wire(path, respond):
    """Use production urllib transport, with its endpoint changed only to loopback."""
    calls, failures, tasks = [], [], set()

    async def handle(reader, writer):
        task = asyncio.current_task()
        tasks.add(task)
        try:
            head = await asyncio.wait_for(reader.readuntil(b"\r\n\r\n"), 5)
            lines = head.decode().split("\r\n")
            headers = dict(line.lower().split(": ", 1) for line in lines[1:] if ": " in line)
            body = await reader.readexactly(int(headers["content-length"]))
            call = {"route": lines[0].split()[1], "fields": decode_wire(headers, body),
                    "reservation": snapshot(path)}
            calls.append(call)
            status, payload = await respond(len(calls), call)
            encoded = json.dumps(payload).encode()
            writer.write(f"HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\n"
                         f"Content-Length: {len(encoded)}\r\nConnection: close\r\n\r\n".encode()
                         + encoded)
            await writer.drain()
        except Exception as error:
            failures.append(error)
        finally:
            writer.close()
            await writer.wait_closed()
            tasks.discard(task)

    server = await asyncio.start_server(handle, "127.0.0.1", 0)
    port = server.sockets[0].getsockname()[1]
    transport = TelegramBotApiTransport("123:independent-fixture")
    transport._api_base = f"http://127.0.0.1:{port}/bot123:independent-fixture/"
    try:
        yield transport, calls, port
    finally:
        await transport.close()
        server.close()
        await server.wait_closed()
        if tasks:
            await asyncio.wait_for(asyncio.gather(*tasks), 5)
        assert not failures


def accepted(number=7):
    return 200, {"ok": True, "result": {"message_id": number,
                "chat": {"id": int(CHAT)}, "message_thread_id": TOPIC}}


@pytest.mark.parametrize("kind", ["text", "photo"])
async def test_host_formatted_unicode_survives_reopen_and_telegram_wire(tmp_path, kind):
    # Offsets count UTF-16 code units. NFC would shorten the accent and silently
    # move this valid bold entity from A to B, even with an astral character.
    text = "e\u0301 \U0001f43e AB"
    entities = [{"type": "bold", "offset": 6, "length": 1}]
    text_field = "text" if kind == "text" else "caption"
    entity_field = "entities" if kind == "text" else "caption_entities"
    part = {text_field: text, entity_field: entities}
    if kind == "photo":
        part.update(media_ref="fixture://photo", mime_type="image/png")
    path = tmp_path / "outbox.sqlite3"
    policy = HostPolicy(tmp_path / "policy.sqlite3", path)

    async def respond(_, __):
        return accepted()

    def retain(engine):
        return engine.commit_output(
            issuer_id="issuer/person", logical_key="reply/one",
            candidates=(("telegram", RECIPIENT),), parts=(part,),
            incident_delivery=False,
        )

    async with telegram_wire(path, respond) as (transport, calls, _):
        sender = telegram_sender(transport)
        first = owner(path, sender, policy)
        output = retain(first)
        before = snapshot(path)
        reopened = owner(path, sender, policy)
        assert retain(reopened) == output
        await reopened._dispatch_output(output)
        assert len(calls) == len(policy.contexts) == 1
        wire = calls[0]["fields"]
        entity = wire[entity_field][0]
        selected = wire[text_field].encode("utf-16-le")[
            entity["offset"] * 2:(entity["offset"] + entity["length"]) * 2
        ].decode("utf-16-le")
        assert selected == "A", "delivery changed the host-selected formatted span"
        assert wire[text_field] == before[text_field] == text
        assert wire[entity_field] == json.loads(before[entity_field]) == entities
        assert policy.contexts[0]["part"][text_field] == text
        assert str(wire["chat_id"]) == CHAT and int(wire["message_thread_id"]) == TOPIC
        if kind == "photo":
            assert wire["photo"] == PHOTO
        assert "parse_mode" not in wire
        after = snapshot(path)
        assert after["request_digest"] == before["request_digest"]
        assert (after["state"], after["attempts"]) == ("sent", 1)
        assert retain(reopened) == output
        await reopened._dispatch_output(output)
        assert len(calls) == 1


def assert_identity(calls, kind):
    for call in calls:
        fields = call["fields"]
        assert str(fields["chat_id"]) == CHAT
        assert int(fields["message_thread_id"]) == TOPIC
        assert "parse_mode" not in fields
        assert call["route"].endswith("sendMessage" if kind == "text" else "sendPhoto")
        if kind == "photo":
            assert fields["photo"] == PHOTO
            assert fields["caption"] == "hello"
        else:
            assert fields["text"] == "hello"


@pytest.mark.parametrize("kind", ["text", "photo"])
async def test_refusal_then_revocation_blocks_plain_wire_entry(tmp_path, kind):
    path = tmp_path / "outbox.sqlite3"
    policy = HostPolicy(tmp_path / "policy.sqlite3", path)

    async def respond(number, _):
        if number == 1:
            policy.set_allowed(False)
            return REFUSAL
        return accepted()

    async with telegram_wire(path, respond) as (transport, calls, _):
        engine = owner(path, telegram_sender(transport), policy)
        output = commit(engine, kind)
        before = snapshot(path)
        await engine._dispatch_output(output)
        assert len(calls) == 1
        assert_identity(calls, kind)
        after = snapshot(path)
        assert (after["state"], after["attempts"], after["unknown_attempts"]) == ("pending", 1, 0)
        assert after["entities"] is after["caption_entities"] is None
        assert after["request_digest"] == before["request_digest"]
        assert len(policy.contexts) == 2
        assert policy.contexts[1]["part"] == {
            key: value for key, value in part_for(kind).items()
            if key not in {"entities", "caption_entities"}}
        assert engine.status(output_id=output).delivery_state != "provider_accepted"


@pytest.mark.parametrize("kind", ["text", "photo"])
async def test_plain_retry_has_fresh_authorization_and_durable_reservation(tmp_path, kind):
    path = tmp_path / "outbox.sqlite3"
    policy = HostPolicy(tmp_path / "policy.sqlite3", path)

    async def respond(number, _):
        return REFUSAL if number == 1 else accepted()

    async with telegram_wire(path, respond) as (transport, calls, _):
        engine = owner(path, telegram_sender(transport), policy)
        output = commit(engine, kind)
        before = snapshot(path)
        await engine._dispatch_output(output)
        assert len(calls) == len(policy.contexts) == 2
        assert_identity(calls, kind)
        field = "entities" if kind == "text" else "caption_entities"
        assert calls[0]["fields"][field] == part_for(kind)[field]
        assert field not in calls[1]["fields"]
        assert [item["attempts"] for item in policy.contexts] == [0, 1]
        assert [call["reservation"]["attempts"] for call in calls] == [1, 2]
        assert all(call["reservation"]["last_outcome"] == "unknown" for call in calls)
        assert all(call["reservation"]["unknown_attempts"] == 1 for call in calls)
        assert all(item["digest"] == before["request_digest"] for item in policy.contexts)
        assert snapshot(path)["attempts"] == 2
        assert engine.status(output_id=output).delivery_state == "provider_accepted"
        assert commit(engine, kind) == output
        await engine.run_recovery_pass(respect_grace=False)
        assert len(calls) == 2


@pytest.mark.parametrize("kind", ["text", "photo"])
async def test_refusal_cannot_bypass_expiry_during_provider_wait(tmp_path, monkeypatch, kind):
    path = tmp_path / "outbox.sqlite3"
    policy = HostPolicy(tmp_path / "policy.sqlite3", path)
    now = [egress_module.time.time_ns()]
    monkeypatch.setattr(egress_module.time, "time_ns", lambda: now[0])

    async def respond(number, _):
        if number == 1:
            now[0] += 2_000_000_000
            return REFUSAL
        return accepted()

    async with telegram_wire(path, respond) as (transport, calls, _):
        engine = owner(path, telegram_sender(transport), policy, freshness_seconds=1)
        await engine._dispatch_output(commit(engine, kind))
        assert len(calls) == len(policy.contexts) == 1
        assert (snapshot(path)["state"], snapshot(path)["attempts"]) == ("expired", 1)


async def test_sender_replacement_after_refusal_routes_plain_retry_to_current_sender(tmp_path):
    path = tmp_path / "outbox.sqlite3"
    policy = HostPolicy(tmp_path / "policy.sqlite3", path)
    authority = SenderAuthority(path=tmp_path / "sender.sqlite3")
    old_entries, new_entries = [], []

    async def respond(number, _):
        if number == 1:
            authority.attach_active(sender_id="telegram", owner_id="host", sender=replacement)
            return REFUSAL
        return accepted()

    async with telegram_wire(path, respond) as (transport, calls, _):
        base = telegram_sender(transport)

        async def original(**kwargs):
            old_entries.append(plain(kwargs))
            return await base(**kwargs)

        async def replacement(**kwargs):
            new_entries.append(plain(kwargs))
            return await base(**kwargs)

        authority.attach_active(sender_id="telegram", owner_id="host", sender=original)
        engine = owner(path, original, policy, sender_authority=authority)
        await engine._dispatch_output(commit(engine))
        assert len(calls) == 2
        assert len(old_entries) == len(new_entries) == 1
        assert new_entries[0]["part"] == {"text": "hello"}
        assert snapshot(path)["attempts"] == 2


@pytest.mark.parametrize("kind", ["text", "photo"])
@pytest.mark.parametrize("response_kind", ["ambiguous_400", "wrong_topic", "queued"])
async def test_unproved_or_queued_outcome_never_advises_or_retries(tmp_path, kind, response_kind):
    path = tmp_path / "outbox.sqlite3"
    policy = HostPolicy(tmp_path / "policy.sqlite3", path)

    async def respond(_, __):
        if response_kind == "ambiguous_400":
            return 400, {**REFUSAL[1], "result": {"message_id": 9}}
        status, payload = accepted(0 if response_kind == "queued" else 7)
        if response_kind == "wrong_topic":
            payload["result"]["message_thread_id"] = TOPIC + 1
        return status, payload

    async with telegram_wire(path, respond) as (transport, calls, _):
        sender = telegram_sender(transport)
        engine = owner(path, sender, policy)
        await engine._dispatch_output(commit(engine, kind))
        assert len(calls) == 1
        assert sender.receipts[0].retry_without_formatting is False
        assert snapshot(path)["state"] == "unknown"
        assert snapshot(path)["entities" if kind == "text" else "caption_entities"] is not None
        await engine.run_recovery_pass(respect_grace=False)
        assert len(calls) == 1


async def test_cancelled_in_flight_refusal_stays_unknown_and_never_retries(tmp_path):
    path = tmp_path / "outbox.sqlite3"
    policy = HostPolicy(tmp_path / "policy.sqlite3", path)
    entered, release = asyncio.Event(), asyncio.Event()

    async def respond(_, __):
        entered.set()
        await release.wait()
        return REFUSAL

    async with telegram_wire(path, respond) as (transport, calls, _):
        engine = owner(path, telegram_sender(transport), policy)
        task = asyncio.create_task(engine._dispatch_output(commit(engine)))
        try:
            await asyncio.wait_for(entered.wait(), 3)
            task.cancel()
            await asyncio.sleep(0)
            assert not task.done()  # Production transport retains its entered thread.
        finally:
            release.set()
            if not task.done():
                task.cancel()
            joined = await asyncio.wait_for(asyncio.gather(task, return_exceptions=True), 3)
        assert isinstance(joined[0], asyncio.CancelledError)
        assert len(calls) == 1
        assert (snapshot(path)["state"], snapshot(path)["attempts"]) == ("unknown", 1)
        assert snapshot(path)["entities"] is not None
        await owner(path, telegram_sender(transport), policy).run_recovery_pass(respect_grace=False)
        assert len(calls) == 1


async def recover_child(path, policy_path, port):
    transport = TelegramBotApiTransport("123:independent-fixture")
    transport._api_base = f"http://127.0.0.1:{port}/bot123:independent-fixture/"
    policy = HostPolicy(Path(policy_path), Path(path))
    try:
        engine = owner(Path(path), telegram_sender(transport), policy)
        result = await engine.run_recovery_pass(respect_grace=False)
        print(json.dumps({"summary": result, "row": snapshot(path), "contexts": policy.contexts,
                          "module": egress_module.__file__}))
    finally:
        await transport.close()


@pytest.mark.parametrize("kind", ["text", "photo"])
async def test_cancelled_after_refusal_recovers_plain_in_fresh_process(tmp_path, kind):
    path, policy_path = tmp_path / "outbox.sqlite3", tmp_path / "policy.sqlite3"
    policy = HostPolicy(policy_path, path)
    backoff = asyncio.Event()

    async def sleep(_):
        backoff.set()
        await asyncio.Event().wait()

    async def respond(number, _):
        return REFUSAL if number == 1 else accepted()

    async with telegram_wire(path, respond) as (transport, calls, port):
        engine = owner(path, telegram_sender(transport), policy, sleep=sleep)
        output = commit(engine, kind)
        digest = snapshot(path)["request_digest"]
        task = asyncio.create_task(engine._dispatch_output(output))
        try:
            await asyncio.wait_for(backoff.wait(), 3)
        finally:
            task.cancel()
            joined = await asyncio.gather(task, return_exceptions=True)
        assert isinstance(joined[0], asyncio.CancelledError)
        assert len(calls) == 1
        assert (snapshot(path)["state"], snapshot(path)["attempts"]) == ("pending", 1)
        assert snapshot(path)["entities"] is snapshot(path)["caption_entities"] is None
        policy.set_allowed(False)
        for allowed in (False, True):
            policy.set_allowed(allowed)
            process = await asyncio.create_subprocess_exec(
                sys.executable, str(Path(__file__).resolve()), "--recover", str(path),
                str(policy_path), str(port), stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE,
            )
            try:
                stdout, stderr = await asyncio.wait_for(process.communicate(), 10)
            except BaseException:
                if process.returncode is None:
                    process.kill()
                await process.communicate()
                raise
            assert process.returncode == 0, stderr.decode()
            assert stderr == b""
            result = json.loads(stdout)
            assert Path(result["module"]).resolve() == Path(egress_module.__file__).resolve()
            assert result["row"]["request_digest"] == digest
            assert len(result["contexts"]) == 1
            assert result["contexts"][0]["attempts"] == 1
            assert "entities" not in result["contexts"][0]["part"]
            assert "caption_entities" not in result["contexts"][0]["part"]
            assert len(calls) == (2 if allowed else 1)
        assert_identity(calls, kind)
        assert snapshot(path)["state"] == "sent"
        assert snapshot(path)["attempts"] == 2


async def test_late_refusal_cannot_clear_formatting_after_other_handle_accepts(tmp_path):
    path = tmp_path / "outbox.sqlite3"
    policy = HostPolicy(tmp_path / "policy.sqlite3", path)

    async def respond(_, __):
        # A separate durable owner has a stronger same-attempt observation.
        assert second._update_part(output, 0, state="sent", attempts=1,
                                   unknown_attempts=0, provider_msg_id="concurrent-proof",
                                   error=None, last_outcome="provider_accepted")
        return REFUSAL

    async with telegram_wire(path, respond) as (transport, calls, _):
        sender = telegram_sender(transport)
        first, second = owner(path, sender, policy), owner(path, sender, policy)
        output = commit(first)
        await first._dispatch_output(output)
        row = snapshot(path)
        assert len(calls) == len(policy.contexts) == 1
        assert (row["state"], row["attempts"], row["provider_msg_id"]) == (
            "sent", 1, "concurrent-proof")
        assert row["entities"] is not None


@pytest.mark.parametrize("flag", [1, "true", [], {}, None, False])
async def test_non_boolean_retry_advice_cannot_mutate_retained_formatting(tmp_path, flag):
    path = tmp_path / "outbox.sqlite3"
    policy = HostPolicy(tmp_path / "policy.sqlite3", path)
    received = []

    async def send(**kwargs):
        received.append(plain(kwargs["part"]))
        if len(received) == 1:
            return {"outcome": "known_not_sent", "retry_without_formatting": flag}
        return {"outcome": "sent"}

    engine = owner(path, send, policy)
    await engine._dispatch_output(commit(engine))
    assert received == [part_for("text"), part_for("text")]
    assert snapshot(path)["entities"] is not None
    assert snapshot(path)["attempts"] == 2


async def test_provider_mutation_does_not_replace_authorized_plain_retry(tmp_path):
    path = tmp_path / "outbox.sqlite3"
    policy = HostPolicy(tmp_path / "policy.sqlite3", path)

    async def respond(number, _):
        return REFUSAL if number == 1 else accepted()

    def mutate(part, receipt):
        if receipt.outcome == "known_not_sent":
            part["text"] = "unauthorized replacement"
            part["entities"][0]["length"] = 1

    async with telegram_wire(path, respond) as (transport, calls, _):
        engine = owner(path, telegram_sender(transport, mutation=mutate), policy)
        await engine._dispatch_output(commit(engine))
        assert len(calls) == 2
        assert_identity(calls, "text")
        assert policy.contexts[1]["part"] == {"text": "hello"}
        assert snapshot(path)["text"] == "hello"


async def test_wrapper_dropping_hint_retries_only_formatted_with_full_attempt_accounting(tmp_path):
    path = tmp_path / "outbox.sqlite3"
    policy = HostPolicy(tmp_path / "policy.sqlite3", path)

    async def respond(_, __):
        return REFUSAL

    async with telegram_wire(path, respond) as (transport, calls, _):
        base = telegram_sender(transport)

        async def legacy_wrapper(**kwargs):
            result = await base(**kwargs)
            result.pop("retry_without_formatting")
            return result

        engine = owner(path, legacy_wrapper, policy)
        await engine._dispatch_output(commit(engine))
        assert len(calls) == len(policy.contexts) == 4
        assert all(call["fields"]["entities"] == part_for("text")["entities"] for call in calls)
        assert [call["reservation"]["attempts"] for call in calls] == [1, 2, 3, 4]
        assert [context["attempts"] for context in policy.contexts] == [0, 1, 2, 3]
        assert snapshot(path)["state"] == "failed"
        assert snapshot(path)["entities"] is not None


@pytest.mark.parametrize("outcome", ["sent", "acked", "unknown", "provider_queued", None])
async def test_true_hint_without_known_refusal_never_changes_presentation(tmp_path, outcome):
    path = tmp_path / "outbox.sqlite3"
    policy = HostPolicy(tmp_path / "policy.sqlite3", path)
    calls = []

    async def send(**kwargs):
        calls.append(kwargs)
        return {"outcome": outcome, "retry_without_formatting": True}

    engine = owner(path, send, policy)
    await engine._dispatch_output(commit(engine))
    assert len(calls) == 1
    assert snapshot(path)["entities"] is not None
    assert snapshot(path)["attempts"] == 1


if __name__ == "__main__":
    if len(sys.argv) != 5 or sys.argv[1] != "--recover":
        raise SystemExit("Expected --recover OUTBOX POLICY PORT")
    asyncio.run(recover_child(sys.argv[2], sys.argv[3], int(sys.argv[4])))
