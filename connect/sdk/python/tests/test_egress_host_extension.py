"""Optional host metadata uses the existing owner, transaction and sender."""
import asyncio
from contextvars import ContextVar
from dataclasses import replace
import hashlib
import json
from pathlib import Path
import re
import sqlite3
import unicodedata

import pytest

from meerkat_toolkit.frontend.egress import (
    EgressAuthorizationDecision as Decision, EgressAuthorizationRefusal,
    EgressDeliveryPolicy, EgressHostExtension, EgressMigrationBinding, ProactiveEgress,
)
from meerkat_toolkit.frontend.sender import SenderAuthority

FIXTURE = Path(__file__).parent / "fixtures/paired-host-egress-199.sql"
BINDING = EgressMigrationBinding("1" * 64, "2" * 64, "3" * 64)


def normalize_values(raw):
    row = {"delivery_class": "governed", "caller_identity": None,
           "recipient_member": None, "choice_ref": None, **raw}
    if row["delivery_class"] not in {"governed", "ordinary_reply"}:
        raise ValueError("invalid delivery class")
    if any(value is not None and (type(value) is not str or len(value) > 128)
           for value in row.values()):
        raise ValueError("invalid host correlation")
    return row


def normalize_extra(raw):
    choices = raw.get("choices")
    if choices is None or choices == () or choices == []:
        return {}
    if not isinstance(choices, (list, tuple)) or len(choices) > 8:
        raise ValueError("invalid choices")
    rows, seen = [], set()
    for row in choices:
        if set(row) != {"label", "value"} or any(type(value) is not str for value in row.values()):
            raise ValueError("invalid choice")
        label, value = unicodedata.normalize("NFC", row["label"]), row["value"]
        if not 1 <= len(label) <= 32 or re.fullmatch(r"[A-Za-z0-9_-]{1,32}", value) is None or value in seen:
            raise ValueError("invalid choice")
        seen.add(value)
        rows.append({"label": label, "value": value})
    return {"choices": rows}


def derive_values(output_id, values, parts):
    return {**values, "choice_ref": output_id.replace("-", "")[:16]
            if any(part.get("choices") for part in parts) else None}


def host_extension(**overrides):
    """Test-only host-shaped declaration; no production host policy import."""
    values = dict(
        output_columns={"delivery_class": "text not null default 'governed'",
                        "caller_identity": "text", "recipient_member": "text", "choice_ref": "text"},
        part_columns=("choices",),
        tables={"egress_choice_taps": """CREATE TABLE egress_choice_taps (
            output_id TEXT PRIMARY KEY, channel_id TEXT NOT NULL,
            source_event_id TEXT NOT NULL, callback_query_id TEXT, value TEXT NOT NULL,
            tapped_at_ms INTEGER NOT NULL,
            FOREIGN KEY(output_id) REFERENCES egress_outputs(output_id))"""},
        indexes={"idx_egress_outputs_choice_ref": "CREATE INDEX idx_egress_outputs_choice_ref ON egress_outputs(choice_ref)"},
        normalize_output=normalize_values, normalize_part=normalize_extra,
        derive_output=derive_values,
        digest_fields=lambda values: {"delivery_class": "ordinary_reply"}
            if values["delivery_class"] == "ordinary_reply" else {},
        pass_output_id=True,
    )
    return EgressHostExtension(**(values | overrides))


def prepare(path, extension):
    with sqlite3.connect(path) as db:
        db.executescript(FIXTURE.read_text())
    observed = ProactiveEgress.inspect_format(path=path, host_extension=extension)
    return ProactiveEgress.migrate_format(path=path, expected=observed,
                                         binding=BINDING, host_extension=extension)


def owner(path, sender, extension, **kwargs):
    if not path.exists():
        prepare(path, extension)
    return ProactiveEgress(path=path, schema_mode="managed", host_extension=extension,
        senders={"s": sender}, incident_sink=kwargs.pop("incident_sink", lambda _: None),
        health_sink=kwargs.pop("health_sink", lambda _: None), sleep=kwargs.pop("sleep", lambda _: None),
        authorization_mode="enforced", authorize_delivery=kwargs.pop("authorize", lambda _: Decision.ALLOWED), **kwargs)


def commit(engine, key="reply", parts=({"text": "h\u00e9llo"},), **kwargs):
    return engine.commit_output(issuer_id="host/person", logical_key=key,
        candidates=(("s", "host-selected"),), parts=parts, incident_delivery=False, **kwargs)


def rows(path, table):
    with sqlite3.connect(path) as db:
        db.row_factory = sqlite3.Row
        return [dict(row) for row in db.execute("SELECT * FROM " + table)]


def canonical(value):
    return json.dumps(value, ensure_ascii=False, allow_nan=False, sort_keys=True, separators=(",", ":"))


def test_host_prepares_exact_schema_and_adopts_without_touching_host_rows(tmp_path):
    path, extension = tmp_path / "outbox.db", host_extension()
    with pytest.raises(ValueError, match="host must prepare"):
        ProactiveEgress.inspect_format(path=path, host_extension=extension)
    assert not path.exists()
    observed = prepare(path, extension)
    assert observed.state == "managed" and observed.format == "egress-sqlite-4"
    with pytest.raises(ValueError):
        ProactiveEgress.inspect_format(path=path)
    assert ProactiveEgress.inspect_format(path=path, host_extension=extension) == observed
    assert rows(path, "egress_choice_taps") == []
    with pytest.raises(ValueError, match="prior host schema adoption"):
        ProactiveEgress(path=tmp_path / "absent" / "outbox.db", host_extension=extension,
            senders={}, incident_sink=lambda _: None, health_sink=lambda _: None, sleep=lambda _: None)
    assert not (tmp_path / "absent").exists()


def test_missing_declared_column_refuses_without_repair(tmp_path):
    path, extension = tmp_path / "outbox.db", host_extension()
    with sqlite3.connect(path) as db:
        db.executescript(FIXTURE.read_text().replace("ALTER TABLE egress_parts ADD COLUMN choices TEXT;", ""))
    before = path.read_bytes()
    with pytest.raises(ValueError, match="incomplete egress schema"):
        ProactiveEgress.inspect_format(path=path, host_extension=extension)
    assert path.read_bytes() == before


@pytest.mark.parametrize("delivery_class", ["governed", "ordinary_reply"])
def test_atomic_metadata_empty_choices_historical_digest_and_replay(tmp_path, delivery_class):
    calls = []
    extension = host_extension(derive_output=lambda *args: calls.append(args[0]) or derive_values(*args))
    path = tmp_path / "outbox.db"
    engine = owner(path, lambda **_: None, extension)
    output = commit(engine, parts=({"text": "h\u00e9llo", "choices": []},),
                    host_values={"delivery_class": delivery_class, "caller_identity": "first"})
    saved = rows(path, "egress_outputs")[0]
    digest = {"candidates": [["s", "host-selected"]], "parts": [{"text": "h\u00e9llo"}], "incident_delivery": False}
    if delivery_class == "ordinary_reply":
        digest["delivery_class"] = delivery_class
    assert saved["request_digest"] == hashlib.sha256(canonical(digest).encode()).hexdigest()
    assert rows(path, "egress_parts")[0]["choices"] is None
    assert commit(engine, host_values={"delivery_class": "ordinary_reply" if delivery_class == "governed" else "governed",
                                      "caller_identity": "later"}) == output
    assert rows(path, "egress_outputs")[0] == saved and calls == [output]
    with pytest.raises(ValueError, match="logical-key reuse"):
        commit(engine, parts=({"text": "changed"},))


@pytest.mark.parametrize("failure", ["derive", "digest", "insert"])
def test_first_commit_rolls_back_every_row_on_codec_or_insert_failure(tmp_path, failure):
    def fail(*_):
        raise ValueError("host codec failed")
    extension = host_extension(**({"derive_output": fail} if failure == "derive" else
                                  {"digest_fields": fail} if failure == "digest" else {}))
    path = tmp_path / "outbox.db"
    engine = owner(path, lambda **_: None, extension)
    if failure == "insert":
        # SQLite's authorizer rejects just the second insert. This is a
        # connection-local test seam, not an undeclared persistent trigger.
        connect = engine._connect
        def rejecting():
            db = connect()
            db.set_authorizer(lambda action, name, *_: sqlite3.SQLITE_DENY
                              if action == sqlite3.SQLITE_INSERT and name == "egress_parts"
                              else sqlite3.SQLITE_OK)
            return db
        engine._connect = rejecting
    with pytest.raises((ValueError, sqlite3.DatabaseError)):
        commit(engine)
    assert rows(path, "egress_outputs") == rows(path, "egress_parts") == []


async def test_choice_identity_and_retained_context_are_explicit_and_immutable(tmp_path):
    calls, contexts = [], []
    async def provider(*, output_id, **kwargs):
        calls.append((output_id, kwargs))
        return {"outcome": "sent", "provider_message_id": "42"}
    provider.supports_choices = True
    def authorize(context):
        contexts.append(context)
        with pytest.raises(TypeError):
            context.host_values["caller_identity"] = "changed"
        with pytest.raises(TypeError):
            context.part["choices"][0]["value"] = "changed"
        return Decision.ALLOWED
    engine = owner(tmp_path / "outbox.db", provider, host_extension(), authorize=authorize)
    output = commit(engine, parts=({"text": "choose", "choices": [{"label": "Yes", "value": "yes"}]},),
                    host_values={"caller_identity": "trusted-first", "recipient_member": "member"})
    await engine.spawn_dispatch(output)
    saved = rows(engine.path, "egress_outputs")[0]
    assert saved["choice_ref"] == output.replace("-", "")[:16]
    assert contexts[0].host_values["choice_ref"] == saved["choice_ref"]
    assert calls[0][0] == output and calls[0][1]["part"]["choices"][0]["value"] == "yes"
    assert rows(engine.path, "egress_choice_taps") == []


@pytest.mark.parametrize("choice_support,identity_support", [(False, True), (True, False)])
async def test_missing_sender_capability_consumes_no_attempt(tmp_path, choice_support, identity_support):
    calls = []
    def with_identity(*, output_id, **kwargs):
        calls.append(output_id)
    def catch_all(**kwargs):
        calls.append(kwargs)
    sender = with_identity if identity_support else catch_all
    sender.supports_choices = choice_support
    engine = owner(tmp_path / "outbox.db", sender, host_extension())
    output = commit(engine, parts=({"text": "choose", "choices": [{"label": "Yes", "value": "yes"}]},))
    await engine._dispatch_output(output)
    assert not calls and engine.attempt_count(output_id=output) == 0


async def test_old_sender_gets_no_new_keyword_without_explicit_opt_in(tmp_path):
    calls = []
    def sender(*, sender_id, recipient, part_index, part):
        calls.append(part)
        return {"outcome": "sent"}
    engine = owner(tmp_path / "outbox.db", sender, host_extension(pass_output_id=False))
    await engine.send(issuer_id="h", logical_key="k", candidates=(("s", "r"),), parts=({"text": "ok"},), incident_delivery=False)
    assert calls == [{"text": "ok"}]


async def test_typed_refusal_is_terminal_without_attempt_but_denied_is_pending(tmp_path):
    calls, health = [], []
    async def sender(*, output_id, **kwargs):
        calls.append(output_id)
    engine = owner(tmp_path / "outbox.db", sender, host_extension(), health_sink=health.append,
                   authorize=lambda _: EgressAuthorizationRefusal("policy_removed"))
    output = commit(engine)
    await engine._dispatch_output(output)
    assert engine.status(output_id=output).part_states == ("refused",)
    assert not calls and engine.attempt_count(output_id=output) == 0
    engine._authorize_delivery = lambda _: Decision.ALLOWED
    await engine._dispatch_output(output)
    assert not calls
    engine._authorize_delivery = lambda _: Decision.DENIED
    denied = commit(engine, "denied")
    await engine._dispatch_output(denied)
    assert engine.status(output_id=denied).part_states == ("pending",)
    assert not [event for event in health if event["kind"] == "egress_terminal_unknown"]


async def test_stop_emits_settled_unknown_fact_but_open_reservation_waits(tmp_path):
    health = []
    engine = owner(tmp_path / "outbox.db", lambda **_: {"outcome": "unknown"},
                   host_extension(pass_output_id=False), health_sink=health.append,
                   delivery_policy=EgressDeliveryPolicy("at_least_once", 1))
    output = commit(engine)
    assert engine._reserve_attempt(output, 0, 0)
    engine.cancel_output(output_id=output, issuer_id="host/person")
    assert not [event for event in health if event["kind"] == "egress_terminal_unknown"]
    await engine.recover_pending(previous_dispatches_quiesced=True)
    terminal = [event for event in health if event["kind"] == "egress_terminal_unknown"]
    assert len(terminal) == 1 and terminal[0]["delivery_policy"] == {"mode": "at_least_once", "unknown_resends": 1}
    assert engine.status(output_id=output).delivery_state == "unknown"
    assert engine.attempt_count(output_id=output) == 1


async def test_fresh_detached_context_and_inline_compatibility(tmp_path):
    ambient = ContextVar("ambient", default=None)
    seen = []
    def provider(**kwargs):
        seen.append(ambient.get())
        return {"outcome": "sent"}
    engine = owner(tmp_path / "outbox.db", provider, host_extension(pass_output_id=False))
    token = ambient.set("caller")
    try:
        await engine.spawn_dispatch(commit(engine, "detached"))
        await engine.send(issuer_id="h", logical_key="inline", candidates=(("s", "r"),), parts=({"text": "ok"},), incident_delivery=False)
        async def drain():
            seen.append(ambient.get())
        engine._drain_until_empty = drain
        engine.ensure_recovery_drainer()
        await engine._drainer
    finally:
        ambient.reset(token)
    assert seen == [None, "caller", None]


def test_concurrent_first_commit_retains_one_complete_host_and_policy_winner(tmp_path):
    from concurrent.futures import ThreadPoolExecutor
    from threading import Barrier
    path, derived = tmp_path / "outbox.db", []
    extension = host_extension(derive_output=lambda *args: derived.append(args[0]) or derive_values(*args))
    prepare(path, extension)
    engines = [owner(path, lambda **_: None, extension) for _ in range(2)]
    barrier = Barrier(2)
    def write(index):
        barrier.wait(timeout=2)
        return commit(engines[index], parts=({"text": "same", "choices": [{"label": "Yes", "value": "yes"}]},),
            host_values={"delivery_class": ("governed", "ordinary_reply")[index],
                         "caller_identity": str(index), "recipient_member": "member-" + str(index)},
            delivery_policy=EgressDeliveryPolicy() if index == 0 else EgressDeliveryPolicy("at_least_once", 2))
    with ThreadPoolExecutor(max_workers=2) as pool:
        futures = [pool.submit(write, index) for index in range(2)]
        outputs = [future.result(timeout=5) for future in futures]
    assert outputs[0] == outputs[1] and derived == [outputs[0]]
    saved = rows(path, "egress_outputs")[0]
    winner = int(saved["caller_identity"])
    assert saved["delivery_class"] == ("governed", "ordinary_reply")[winner]
    assert saved["recipient_member"] == "member-" + str(winner)
    assert saved["choice_ref"] == outputs[0].replace("-", "")[:16]
    assert json.loads(saved["delivery_policy"])["unknown_resends"] == (0, 2)[winner]
    assert len(rows(path, "egress_parts")) == 1


async def test_all_sender_routes_and_handoff_use_one_current_capability_selection(tmp_path):
    calls = []
    class Provider:
        supports_choices = True
        def __init__(self, label):
            self.label = label
        async def activate(self):
            pass
        async def deactivate(self):
            pass
        def __call__(self, *, output_id, **kwargs):
            calls.append((self.label, output_id, kwargs))
            return {"outcome": "sent"}
    authority = SenderAuthority(path=tmp_path / "senders.db")
    old, new = Provider("old"), Provider("new")
    authority.attach_active(sender_id="s", owner_id="old", sender=old)
    assert authority.supports_choices("s") is True
    engine = owner(tmp_path / "outbox.db", old, host_extension(), sender_authority=authority)
    part = {"text": "same", "choices": [{"label": "Yes", "value": "yes"}]}
    # The explicit authority dispatch route preserves the same opt-in contract.
    await engine._dispatch_sender(sender_id="s", recipient="r", part_index=0, part=part, output_id="direct")
    output = commit(engine, parts=(part,))
    async def switch(_):
        handle = authority.stage_activation(sender_id="s", to_owner_id="new", sender=new)
        await authority.commit_activation(handle)
        return Decision.ALLOWED
    engine._authorize_delivery = switch
    await engine._dispatch_output(output)
    assert engine.attempt_count(output_id=output) == 0 and len(calls) == 1
    engine._authorize_delivery = lambda _: Decision.ALLOWED
    new.supports_choices = False
    await engine._dispatch_output(output)
    assert engine.attempt_count(output_id=output) == 0 and len(calls) == 1
    new.supports_choices = True
    await engine._dispatch_output(output)
    assert calls[-1][0:2] == ("new", output) and engine.attempt_count(output_id=output) == 1
    # Raw mapping route is also covered by the first choice/context test.


async def test_failed_terminal_callback_reconciles_without_suppressing_legacy_incident(tmp_path):
    calls, incidents, facts = [], [], []
    def provider(**kwargs):
        calls.append(kwargs)
        return {"outcome": "unknown"}
    def broken(event):
        if event["kind"] == "egress_terminal_unknown":
            raise RuntimeError("host health unavailable")
    engine = owner(tmp_path / "outbox.db", provider, host_extension(pass_output_id=False),
        delivery_policy=EgressDeliveryPolicy("at_least_once", 1),
        health_sink=broken, incident_sink=incidents.append)
    output = commit(engine)
    with pytest.raises(RuntimeError, match="health unavailable"):
        await engine._dispatch_output(output)
    assert len(calls) == 2 and len(incidents) == 1
    before = engine.path.read_bytes()
    engine.health_sink = facts.append
    assert engine.report_terminal_uncertainty(output) == 1
    assert engine.report_terminal_uncertainty(output) == 1
    assert engine.path.read_bytes() == before and len(calls) == 2 and len(incidents) == 1
    assert facts[0] == facts[1] and facts[0]["delivery_policy"] == {"mode": "at_least_once", "unknown_resends": 1}
    incident = engine.commit_output(issuer_id="h", logical_key="incident", candidates=(("s", "r"),),
        parts=({"text": "incident"},), incident_delivery=True)
    await engine._dispatch_output(incident)
    assert engine.report_terminal_uncertainty(incident) == 0


@pytest.mark.parametrize("field", ["caller_identity", "not_declared"])
def test_invalid_host_values_and_digest_fields_refuse_before_insert(tmp_path, field):
    extension = host_extension(digest_fields=(lambda _: {"not_declared": "x"}) if field == "not_declared" else lambda _: {})
    engine = owner(tmp_path / "outbox.db", lambda **_: None, extension)
    with pytest.raises(ValueError):
        commit(engine, host_values={"caller_identity": ["not text"]} if field == "caller_identity" else {})
    assert rows(engine.path, "egress_outputs") == []


async def test_declared_keyword_columns_commit_and_reconstruct_as_identifiers(tmp_path):
    base = host_extension()
    extension = host_extension(
        output_columns={**base.output_columns, "select": "text"},
        part_columns=("choices", "order"),
        normalize_output=lambda raw: {"select": None, **normalize_values(raw)},
        normalize_part=lambda raw: {**normalize_extra({key: value for key, value in raw.items() if key == "choices"}),
                                    **({"order": raw["order"]} if "order" in raw else {})},
        pass_output_id=False,
    )
    path, calls = tmp_path / "outbox.db", []
    with sqlite3.connect(path) as db:
        db.executescript(FIXTURE.read_text())
        db.execute('ALTER TABLE egress_outputs ADD COLUMN "select" TEXT')
        db.execute('ALTER TABLE egress_parts ADD COLUMN "order" TEXT')
    ProactiveEgress.migrate_format(path=path, expected=ProactiveEgress.inspect_format(path=path, host_extension=extension),
        binding=BINDING, host_extension=extension)
    def sender(**kwargs):
        calls.append(kwargs)
        return {"outcome": "sent"}
    engine = owner(path, sender, extension)
    output = commit(engine, parts=({"text": "ok", "order": ["first"]},), host_values={"select": "value"})
    assert rows(path, "egress_outputs")[0]["select"] == "value"
    await owner(path, sender, extension)._dispatch_output(output)
    assert calls[0]["part"]["order"] == ["first"]
