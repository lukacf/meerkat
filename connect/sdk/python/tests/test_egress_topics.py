"""Explicit durable thread targets through the real SQLite output owner."""

from __future__ import annotations

import asyncio
from dataclasses import asdict
import hashlib
import sqlite3

import pytest

from meerkat_toolkit.frontend import EgressCandidate
from meerkat_toolkit.frontend import egress
from meerkat_toolkit.frontend.egress import EgressAuthorizationDecision as Decision
from meerkat_toolkit.frontend.sender import SenderAuthority
from test_egress import engine, rows
from test_egress_migration import snapshot


def binding():
    return egress.EgressMigrationBinding("1" * 64, "2" * 64, "3" * 64)


def managed(path, sender=None, **kwargs):
    if not path.exists():
        egress.ProactiveEgress.migrate_format(
            path=path, expected=egress.ProactiveEgress.inspect_format(path=path), binding=binding()
        )
    return engine(path, sender, schema_mode="managed", **kwargs)


def commit(owner, thread="42", key="one", **kwargs):
    return owner.commit_output(
        issuer_id="issuer", logical_key=key,
        candidates=kwargs.pop("candidates", (EgressCandidate("s", "chat", thread),)),
        parts=kwargs.pop("parts", ({"text": "hello"},)), incident_delivery=False, **kwargs,
    )


def v2(path):
    """Construct the exact supported predecessor schema and its managed record."""
    owner = engine(path)
    output = commit(owner, None)
    with sqlite3.connect(path) as db:
        db.execute("ALTER TABLE egress_outputs DROP COLUMN thread_id")
        db.execute("ALTER TABLE egress_outputs DROP COLUMN delivery_policy")
        for name in ("attempt_open", "unknown_resends_used", "unknown_history"):
            db.execute(f"ALTER TABLE egress_parts DROP COLUMN {name}")
        db.execute("UPDATE egress_parts SET state='unknown',attempts=1,unknown_attempts=1,"
                   "last_outcome='unknown',provider_msg_id='retained'")
    shape = egress.ProactiveEgress.inspect_format(path=path)
    with sqlite3.connect(path) as db:
        db.execute(egress._FORMAT_SQL)
        db.execute("INSERT INTO egress_format VALUES (?,?,?,?,?,?,?,?,?)", (
            1, egress.EGRESS_STATE_NAMESPACE, egress.EGRESS_STATE_OWNER,
            "egress-sqlite-2", "adopt-egress-sqlite-2", "unmarked_current",
            "4" * 64, shape.schema_digest, egress._canonical_json(asdict(binding())).decode(),
        ))
    return output, egress.ProactiveEgress.inspect_format(path=path)


@pytest.mark.parametrize("thread", [False, 0, "", " ", "bad\n", "bad\x00", "bad\x7f", "\ud800", "é" * 513])
def test_invalid_explicit_target_is_rejected_before_database_use(thread):
    with pytest.raises(ValueError):
        EgressCandidate("s", "chat", thread)


async def test_no_thread_typed_candidate_has_identical_legacy_digest_and_call(tmp_path):
    calls = []

    def legacy(**kwargs):
        calls.append(kwargs)
        return {"outcome": "sent"}

    owner = engine(tmp_path / "outbox", legacy)
    output = commit(owner, None, candidates=(("s", "chat"),))
    assert commit(owner, None) == output
    expected = hashlib.sha256(egress._canonical_json({
        "candidates": (("s", "chat"),), "parts": ({"text": "hello"},), "incident_delivery": False,
    })).hexdigest()
    with sqlite3.connect(owner.path) as db:
        assert db.execute("SELECT request_digest,thread_id FROM egress_outputs").fetchone() == (expected, None)
    await owner._dispatch_output(output)
    assert calls == [{"sender_id": "s", "recipient": "chat", "part_index": 0, "part": {"text": "hello"}}]


def test_every_explicit_candidate_requires_managed_format_even_if_not_selected(tmp_path):
    owner = engine(tmp_path / "outbox", lambda **kw: None)
    before = snapshot(owner.path)
    for candidates in [
        (EgressCandidate("s", "chat", "42"),),
        (("s", "chat"), EgressCandidate("detached", "chat", "42")),
    ]:
        with pytest.raises(ValueError, match="require managed"):
            commit(owner, candidates=candidates)
        assert snapshot(owner.path) == before


async def test_two_explicit_topics_survive_reopen_and_each_authorization(tmp_path):
    path = tmp_path / "outbox"
    owner = managed(path)
    targets = ["42", "cafe\u0301"]
    outputs = [commit(owner, target, key=target) for target in targets]
    contexts, calls = [], []

    def authorize(context):
        contexts.append((context.recipient, context.thread_id, context.request_digest))
        return Decision.ALLOWED

    def sender(*, sender_id, recipient, part_index, part, thread_id=None):
        calls.append((recipient, thread_id, part))
        return {"outcome": "sent"}

    reopened = managed(path, sender, authorization_mode="enforced", authorize_delivery=authorize)
    for output in outputs:
        await reopened._dispatch_output(output)
    assert [c[:2] for c in contexts] == [("chat", target) for target in targets]
    assert len({c[2] for c in contexts}) == 2
    assert calls == [("chat", target, {"text": "hello"}) for target in targets]
    assert all(reopened.status(output_id=output).delivery_state == "provider_accepted" for output in outputs)


def test_all_candidates_bind_digest_and_selected_destination_is_frozen(tmp_path):
    owner = managed(tmp_path / "outbox", lambda **kw: None)
    candidates = [EgressCandidate("absent", "other", "11"), EgressCandidate("s", "chat", "42")]
    output = commit(owner, candidates=candidates)
    with sqlite3.connect(owner.path) as db:
        assert db.execute("SELECT sender_id,recipient,thread_id FROM egress_outputs").fetchone() == ("s", "chat", "42")
    assert commit(owner, candidates=candidates) == output
    for changed in [
        [EgressCandidate("absent", "other", "12"), candidates[1]],
        [candidates[0], EgressCandidate("s", "chat", "43")],
        [candidates[0], EgressCandidate("s", "chat")],
    ]:
        with pytest.raises(ValueError, match="logical-key reuse"):
            commit(owner, candidates=changed)


@pytest.mark.parametrize("kind", ["old", "catch_all", "positional_only"])
async def test_unsupported_sender_never_enters_or_spends_attempt(tmp_path, kind):
    calls, health = [], []

    def old(*, sender_id, recipient, part_index, part):
        calls.append(part)

    def catch_all(**kwargs):
        calls.append(kwargs)

    def positional_only(thread_id=None, /, **kwargs):
        calls.append(kwargs)

    owner = managed(tmp_path / "outbox", {"old": old, "catch_all": catch_all, "positional_only": positional_only}[kind])
    owner.health_sink = health.append
    output = commit(owner)
    await owner._dispatch_output(output)
    assert calls == [] and owner.attempt_count(output_id=output) == 0
    assert owner.status(output_id=output).part_states == ("pending",)
    assert health[-1]["kind"] == "egress_thread_target_unsupported"


async def test_sender_authority_direct_dispatch_keeps_explicit_target_contract(tmp_path):
    authority = SenderAuthority(path=tmp_path / "senders")
    calls = []

    def target(*, thread_id=None, **kwargs):
        calls.append((thread_id, kwargs))
        return {"outcome": "sent"}

    authority.attach_active(sender_id="s", owner_id="owner", sender=target)
    await authority.dispatch(sender_id="s", recipient="chat", part_index=0, part={"text": "hello"}, thread_id="42")
    assert calls[0][0] == "42"
    authority.attach_active(sender_id="s", owner_id="owner", sender=lambda **kw: calls.append(kw))
    with pytest.raises(ValueError, match="explicitly support"):
        await authority.dispatch(sender_id="s", recipient="chat", part_index=0, part={"text": "hello"}, thread_id="42")
    assert len(calls) == 1


async def test_legacy_authorization_thread_rechecks_age_after_sender_wait(tmp_path, monkeypatch):
    authority = SenderAuthority(path=tmp_path / "senders")
    calls, entered = [], asyncio.Event()
    now = [egress.time.time_ns()]
    monkeypatch.setattr(egress.time, "time_ns", lambda: now[0])

    def sender(*, thread_id=None, **kwargs):
        calls.append(thread_id)
        return {"outcome": "sent"}

    authority.attach_active(sender_id="s", owner_id="owner", sender=sender)
    owner = managed(tmp_path / "outbox", sender_authority=authority, freshness_seconds=1)
    output = commit(owner)
    select = authority.select_dispatch

    async def waiting(**kwargs):
        entered.set()
        return await select(**kwargs)

    monkeypatch.setattr(authority, "select_dispatch", waiting)
    activating, release = asyncio.Event(), asyncio.Event()

    class Candidate:
        async def activate(self):
            activating.set()
            await release.wait()

        async def deactivate(self):
            pass

        async def __call__(self, *, thread_id=None, **kwargs):
            return sender(thread_id=thread_id, **kwargs)

    handle = authority.stage_activation(sender_id="s", to_owner_id="owner", sender=Candidate())
    activation = asyncio.create_task(authority.commit_activation(handle))
    try:
        await asyncio.wait_for(activating.wait(), 1)
        task = owner.spawn_dispatch(output)
        await asyncio.wait_for(entered.wait(), 1)
        assert not task.done()
        now[0] += 2_000_000_000
    finally:
        release.set()
        await asyncio.wait_for(activation, 1)
    await task
    assert calls == [] and owner.attempt_count(output_id=output) == 0
    assert owner.status(output_id=output).part_states == ("expired",)


@pytest.mark.parametrize("after_refusal", ["retry", "deny", "stop", "restart"])
async def test_format_refusal_retains_topic_and_rechecks_before_next_attempt(tmp_path, after_refusal):
    calls, contexts = [], []
    allowed = True
    owner = None
    output = None

    def authorize(context):
        contexts.append((context.thread_id, context.request_digest, dict(context.part)))
        return Decision.ALLOWED if allowed else Decision.DENIED

    def sender(*, thread_id=None, **kwargs):
        calls.append((thread_id, kwargs["part"]))
        if len(calls) == 1:
            return {"outcome": "known_not_sent", "retry_without_formatting": True}
        return {"outcome": "sent"}

    async def backoff(_delay):
        nonlocal allowed
        if after_refusal == "deny":
            allowed = False
        elif after_refusal == "stop":
            owner.cancel_output(output_id=output, issuer_id="issuer")
        elif after_refusal == "restart":
            raise asyncio.CancelledError

    owner = managed(tmp_path / "outbox", sender, sleep=backoff, normalize_parts=lambda p: p,
                    authorization_mode="enforced", authorize_delivery=authorize)
    output = commit(owner, parts=({"text": "hello", "entities": [{"type": "bold", "offset": 0, "length": 5}]},))
    if after_refusal == "restart":
        with pytest.raises(asyncio.CancelledError):
            await owner._dispatch_output(output)
        owner = managed(owner.path, sender, normalize_parts=lambda p: p,
                        authorization_mode="enforced", authorize_delivery=authorize)
        await owner._dispatch_output(output)
    else:
        await owner._dispatch_output(output)
    assert all(call[0] == "42" for call in calls)
    assert all(context[0] == "42" for context in contexts)
    assert len({context[1] for context in contexts}) == 1
    if after_refusal in {"retry", "restart"}:
        assert len(calls) == 2 and calls[1][1] == {"text": "hello"}
        assert owner.attempt_count(output_id=output) == 2
    else:
        assert len(calls) == 1 and owner.attempt_count(output_id=output) == 1


async def test_cancellation_after_entry_keeps_topic_unknown_without_replay(tmp_path):
    entered = asyncio.Event()
    calls = []

    async def sender(*, thread_id=None, **kw):
        calls.append(thread_id)
        entered.set()
        await asyncio.Event().wait()

    owner = managed(tmp_path / "outbox", sender)
    output = commit(owner)
    task = owner.spawn_dispatch(output)
    await entered.wait()
    task.cancel()
    with pytest.raises(asyncio.CancelledError):
        await task
    reopened = managed(owner.path, sender)
    await reopened._dispatch_output(output)
    assert calls == ["42"] and rows(owner.path) == [(0, "unknown", 1, 1, None)]


def test_v2_observation_is_read_only_and_upgrade_links_predecessor(tmp_path):
    path = tmp_path / "outbox"
    output, before = v2(path)
    retained, delivery = snapshot(path), rows(path)
    assert before.format == "egress-sqlite-2"
    assert egress.ProactiveEgress.inspect_format(path=path) == before
    for mode in ["managed", "legacy"]:
        with pytest.raises(ValueError, match="managed egress format required"):
            engine(path, schema_mode=mode)
        assert snapshot(path) == retained
    after = egress.ProactiveEgress.migrate_format(path=path, expected=before, binding=binding())
    assert after.format == "egress-sqlite-4"
    assert after.transition.source_state == "managed_v2"
    assert after.transition.source_observation_digest == before.observation_digest
    assert rows(path) == delivery
    assert managed(path).status(output_id=output).part_states == ("unknown",)
    with sqlite3.connect(path) as db:
        assert db.execute("SELECT thread_id FROM egress_outputs").fetchone() == (None,)
    with pytest.raises(ValueError, match="invalid egress migration request"):
        egress.ProactiveEgress.migrate_format(path=path, expected=after, binding=binding())


@pytest.mark.parametrize("failure", ["alter", "record"])
def test_v2_migration_failure_rolls_back_schema_marker_and_rows(tmp_path, monkeypatch, failure):
    path = tmp_path / "outbox"
    _, before = v2(path)
    retained = snapshot(path)
    connect = egress._format_connection

    def failing(*args):
        db = connect(*args)

        def authorize(action, first, second, database, caller):
            if failure == "alter" and action == sqlite3.SQLITE_ALTER_TABLE:
                return sqlite3.SQLITE_DENY
            if failure == "record" and action == sqlite3.SQLITE_INSERT and first == "egress_format":
                return sqlite3.SQLITE_DENY
            return sqlite3.SQLITE_OK

        db.set_authorizer(authorize)
        return db

    with monkeypatch.context() as patch:
        patch.setattr(egress, "_format_connection", failing)
        with pytest.raises(sqlite3.DatabaseError):
            egress.ProactiveEgress.migrate_format(path=path, expected=before, binding=binding())
    assert snapshot(path) == retained
    assert egress.ProactiveEgress.inspect_format(path=path) == before


def test_stale_v2_migration_refused_without_relabeling(tmp_path):
    path = tmp_path / "outbox"
    _, before = v2(path)
    with sqlite3.connect(path) as db:
        db.execute("UPDATE egress_format SET source_observation_digest=?", ("5" * 64,))
    retained = snapshot(path)
    with pytest.raises(ValueError, match="stale egress"):
        egress.ProactiveEgress.migrate_format(path=path, expected=before, binding=binding())
    assert snapshot(path) == retained
