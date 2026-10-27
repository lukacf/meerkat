"""Real SQLite delivery outcomes and process-loss regressions."""

from __future__ import annotations

import asyncio
from contextlib import closing
from copy import deepcopy
import importlib.util
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import sys

import pytest

from meerkat_toolkit.frontend.egress import ProactiveEgress as CurrentEgress
from meerkat_toolkit.frontend.egress import EgressAuthorizationDecision
from meerkat_toolkit.frontend.egress import EgressMigrationBinding
from meerkat_toolkit.frontend.sender import SenderAuthority


# The same behavioral tests can run against the captured predecessor. No oracle
# or provider fixture changes are needed to demonstrate the former retransmits.
if os.environ.get("EGRESS_PREDECESSOR"):
    spec = importlib.util.spec_from_file_location(
        "egress_predecessor", os.environ["EGRESS_PREDECESSOR"]
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    ProactiveEgress = module.ProactiveEgress
else:
    ProactiveEgress = CurrentEgress


def engine(path: Path, sender=None, **kwargs):
    return ProactiveEgress(
        path=path,
        senders={"s": sender} if sender is not None else {},
        incident_sink=lambda _: None,
        health_sink=lambda _: None,
        sleep=kwargs.pop("sleep", lambda _: None),
        **kwargs,
    )


def commit(owner, *, key="one", recipient="r", parts=({"text": "hello"},)):
    return owner.commit_output(
        issuer_id="issuer",
        logical_key=key,
        candidates=(("s", recipient),),
        parts=parts,
        incident_delivery=False,
    )


def rows(path):
    with sqlite3.connect(path) as db:
        return db.execute(
            "SELECT part_index,state,attempts,unknown_attempts,provider_msg_id "
            "FROM egress_parts ORDER BY part_index"
        ).fetchall()


@pytest.mark.parametrize("schema_mode", ["legacy", "managed"])
async def test_connections_close_before_provider_and_after_rollback_reopen(tmp_path, monkeypatch, schema_mode):
    original_connect = sqlite3.connect
    opened, calls = [], []
    reject_part = False

    def connect(*args, **kwargs):
        connection = original_connect(*args, **kwargs)
        opened.append(connection)
        if reject_part:
            connection.set_authorizer(lambda action, table, *_:
                sqlite3.SQLITE_DENY if action == sqlite3.SQLITE_INSERT and table == "egress_parts"
                else sqlite3.SQLITE_OK)
        return connection

    def assert_closed():
        assert opened
        for connection in opened:
            with pytest.raises(sqlite3.ProgrammingError, match="closed database"):
                connection.execute("SELECT 1")

    async def provider(**kwargs):
        # The committed attempt, not an open SQL connection, owns this await.
        assert_closed()
        calls.append(kwargs)
        return {"outcome": "sent", "provider_message_id": "controlled-receipt"}

    path = tmp_path / "outbox.sqlite3"
    monkeypatch.setattr(sqlite3, "connect", connect)
    try:
        if schema_mode == "managed":
            CurrentEgress.migrate_format(path=path,
                expected=CurrentEgress.inspect_format(path=path),
                binding=EgressMigrationBinding("1" * 64, "2" * 64, "3" * 64))
            assert_closed()
        owner = engine(path, provider, schema_mode=schema_mode)
        assert_closed()
        output = commit(owner)
        assert_closed()
        await owner._dispatch_output(output)
        assert owner.status(output_id=output).part_states == ("sent",)
        assert owner.attempt_count(output_id=output) == 1
        assert len(calls) == 1
        assert_closed()

        reject_part = True
        with pytest.raises(sqlite3.DatabaseError, match="not authorized"):
            commit(owner, key="rollback")
        reject_part = False
        assert owner.find_output(issuer_id="issuer", logical_key="rollback") is None
        assert_closed()
        with closing(original_connect(path)) as connection:
            assert connection.execute("SELECT COUNT(*) FROM egress_outputs").fetchone()[0] == 1
            assert connection.execute("SELECT provider_msg_id FROM egress_parts").fetchone()[0] == "controlled-receipt"

        pending = commit(owner, key="cancel")
        assert owner.cancel_output(output_id=pending, issuer_id="issuer").part_states == ("cancelled",)
        reopened = engine(path, provider, schema_mode=schema_mode)
        assert commit(reopened) == output
        await reopened.recover_pending()
        assert len(calls) == 1
        assert reopened.status(output_id=output).part_states == ("sent",)
        assert reopened.status(output_id=pending).part_states == ("cancelled",)
        assert_closed()
    finally:
        for connection in opened:
            connection.close()


@pytest.mark.parametrize("pragma", ["foreign_keys", "synchronous"])
@pytest.mark.parametrize("close_fails", [False, True])
def test_legacy_setup_failure_closes_real_connection_and_preserves_error(tmp_path, monkeypatch, pragma, close_fails):
    original_connect = sqlite3.connect
    opened, errors = [], []

    class Connection(sqlite3.Connection):
        def execute(self, *args, **kwargs):
            try:
                return super().execute(*args, **kwargs)
            except sqlite3.DatabaseError as error:
                errors.append(error)
                raise

        def close(self):
            super().close()
            if close_fails:
                raise OSError("controlled close failure after actual closure")

    def connect(*args, **kwargs):
        connection = original_connect(*args, **kwargs, factory=Connection)
        opened.append(connection)
        connection.set_authorizer(lambda action, name, *_:
            sqlite3.SQLITE_DENY if action == sqlite3.SQLITE_PRAGMA and name == pragma else sqlite3.SQLITE_OK)
        return connection

    monkeypatch.setattr(sqlite3, "connect", connect)
    try:
        with pytest.raises(sqlite3.DatabaseError) as caught:
            engine(tmp_path / "outbox.sqlite3")
        assert len(opened) == 1
        assert caught.value is errors[0]
        with pytest.raises(sqlite3.ProgrammingError, match="closed database"):
            opened[0].execute("SELECT 1")
    finally:
        for connection in opened:
            sqlite3.Connection.close(connection)


@pytest.mark.parametrize(
    "result",
    [
        None,
        {},
        {"outcome": None},
        {"outcome": "false"},
        {"outcome": False},
        {"outcome": "unknown"},
        {"outcome": "sent", "provider_message_id": {"invalid": 1}},
    ],
)
async def test_unrecognized_or_unknown_outcome_never_retransmits(tmp_path, result):
    calls = []

    async def provider(**kwargs):
        calls.append(kwargs)
        return result

    path = tmp_path / "outbox.sqlite3"
    first = engine(path, provider)
    output = commit(first)
    await first._dispatch_output(output)
    assert len(calls) == 1
    assert rows(path)[0][1:4] == ("unknown", 1, 1)
    reopened = engine(path, provider)
    assert commit(reopened) == output
    await reopened.recover_pending()
    await reopened._dispatch_output(output)
    assert len(calls) == 1


@pytest.mark.parametrize("failure", ["exception", "timeout", "cancellation"])
async def test_entered_failure_never_proves_non_delivery(tmp_path, failure):
    calls = []
    entered = asyncio.Event()

    async def provider(**kwargs):
        calls.append(kwargs)
        entered.set()
        if failure == "exception":
            raise RuntimeError("private provider detail")
        await asyncio.Event().wait()

    path = tmp_path / "outbox.sqlite3"
    first = engine(path, provider, dispatch_timeout_seconds=0.03)
    output = commit(first)
    task = asyncio.create_task(first._dispatch_output(output))
    await entered.wait()
    if failure == "cancellation":
        task.cancel()
        with pytest.raises(asyncio.CancelledError):
            await task
    else:
        await task
    reopened = engine(path, provider, dispatch_timeout_seconds=0.03)
    await reopened.recover_pending()
    assert len(calls) == 1
    assert rows(path)[0][1:4] == ("unknown", 1, 1)


async def test_actual_process_loss_retains_receipt_gap_without_another_send(tmp_path):
    path, accepted = tmp_path / "outbox.sqlite3", tmp_path / "accepted.txt"
    source = """
import asyncio, importlib.util, os, sys
from pathlib import Path
from meerkat_toolkit.frontend.egress import ProactiveEgress
if os.environ.get('EGRESS_PREDECESSOR'):
    spec = importlib.util.spec_from_file_location('egress_predecessor', os.environ['EGRESS_PREDECESSOR'])
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    ProactiveEgress = module.ProactiveEgress
async def provider(**kwargs):
    with Path(os.environ['ACCEPTED']).open('a') as f:
        f.write('accepted\\n'); f.flush(); os.fsync(f.fileno())
    os._exit(23)
owner=ProactiveEgress(path=Path(os.environ['OUTBOX']), senders={'s':provider},
    incident_sink=lambda _:None,health_sink=lambda _:None,sleep=lambda _:None)
asyncio.run(owner.send(issuer_id='issuer',logical_key='one',candidates=(('s','r'),),
    parts=({'text':'hello'},),incident_delivery=False))
"""
    child = subprocess.run(
        [sys.executable, "-c", source],
        env={**os.environ, "OUTBOX": str(path), "ACCEPTED": str(accepted)},
        capture_output=True,
        timeout=10,
    )
    assert child.returncode == 23, child.stderr.decode()
    assert rows(path)[0][1:4] == ("pending", 1, 1)
    calls = []
    reopened = engine(path, lambda **kwargs: calls.append(kwargs) or {"outcome": "sent"})
    await reopened.recover_pending()
    assert calls == []
    assert accepted.read_text() == "accepted\n"
    assert rows(path)[0][1:4] == ("unknown", 1, 1)


@pytest.mark.parametrize("unknown_attempts", [0, 1, 2])
async def test_legacy_attempted_pending_row_is_unresolved_even_without_marker(
    tmp_path, unknown_attempts
):
    path = tmp_path / "outbox.sqlite3"
    first = engine(path)
    output = commit(first)
    with sqlite3.connect(path) as db:
        db.execute(
            "UPDATE egress_parts SET attempts=1,unknown_attempts=?,provider_msg_id='old-receipt',"
            "error='retained old uncertainty'",
            (unknown_attempts,),
        )
        # Emulate the predecessor schema itself, not merely a null new column.
        columns = {r[1] for r in db.execute("PRAGMA table_info(egress_parts)")}
        if "last_outcome" in columns:
            db.execute("ALTER TABLE egress_parts DROP COLUMN last_outcome")
        before = db.execute("SELECT * FROM egress_outputs").fetchall()
    calls = []
    reopened = engine(path, lambda **kwargs: calls.append(kwargs) or {"outcome": "sent"})
    await reopened.recover_pending()
    assert calls == []
    assert rows(path) == [(0, "unknown", 1, unknown_attempts, "old-receipt")]
    with sqlite3.connect(path) as db:
        assert db.execute("SELECT * FROM egress_outputs").fetchall() == before
        assert (
            db.execute("SELECT error FROM egress_parts").fetchone()[0] == "retained old uncertainty"
        )
    assert commit(reopened) == output


async def test_expiry_cannot_erase_a_prior_unknown_attempt(tmp_path):
    path = tmp_path / "outbox.sqlite3"
    first = engine(path)
    commit(first)
    with sqlite3.connect(path) as db:
        db.execute("UPDATE egress_parts SET attempts=1,unknown_attempts=1")
        db.execute("UPDATE egress_outputs SET created_at_ms=1")
    reopened = engine(path, lambda **_: pytest.fail("uncertain old output was resent"))
    await reopened.recover_pending()
    assert rows(path)[0][1:4] == ("unknown", 1, 1)


def test_stale_expiry_snapshot_cannot_overwrite_concurrent_acceptance(tmp_path, monkeypatch):
    path = tmp_path / "outbox.sqlite3"
    events = []
    owner = engine(path)
    owner.health_sink = events.append
    output = commit(owner)
    with sqlite3.connect(path) as db:
        db.execute("UPDATE egress_outputs SET created_at_ms=1")
        db.execute("UPDATE egress_parts SET attempts=1,unknown_attempts=1,last_outcome='unknown'")
    original = owner._retire_unresolved

    def accept_after_snapshot(output_id, index, **kwargs):
        with sqlite3.connect(path) as db:
            db.execute(
                "UPDATE egress_parts SET state='sent',unknown_attempts=0,"
                "last_outcome='provider_accepted',provider_msg_id='accepted-between-reads'"
            )
        return original(output_id, index, **kwargs)

    monkeypatch.setattr(owner, "_retire_unresolved", accept_after_snapshot)
    assert owner.expire_stale() == 0
    assert rows(path) == [(0, "sent", 1, 0, "accepted-between-reads")]
    assert events == []
    assert owner.status(output_id=output).part_states == ("sent",)


async def test_explicit_known_not_sent_allows_bounded_retry_after_reopen(tmp_path):
    path = tmp_path / "outbox.sqlite3"
    calls, delays = [], []

    def provider(**kwargs):
        calls.append(kwargs)
        return {"outcome": "known_not_sent", "error": "refused before acceptance"}

    def process_loss(delay):
        delays.append(delay)
        raise RuntimeError("stop during backoff")

    first = engine(path, provider, sleep=process_loss)
    output = commit(first)
    with pytest.raises(RuntimeError, match="stop during backoff"):
        await first._dispatch_output(output)
    reopened = engine(path, provider, sleep=delays.append)
    await reopened.recover_pending()
    assert len(calls) == 4
    assert delays == [5, 30, 120]
    assert rows(path)[0][1:4] == ("failed", 4, 0)


@pytest.mark.parametrize("photo", [False, True])
@pytest.mark.parametrize("enforced", [False, True])
async def test_plain_candidate_is_durable_and_reauthorized_without_content_change(
    tmp_path, photo, enforced
):
    path = tmp_path / "outbox.sqlite3"
    formatting = [{"type": "bold", "offset": 0, "length": 5}]
    part = ({"media_ref": "host:media", "caption": "hello", "mime_type": "image/png",
             "caption_entities": formatting} if photo else
            {"text": "hello", "entities": formatting})
    plain = {key: value for key, value in part.items()
             if key not in {"entities", "caption_entities"}}
    calls, contexts = [], []

    def provider(**kwargs):
        calls.append(deepcopy(kwargs))
        if len(calls) == 1:
            # A sender must not rewrite the retained retry candidate.
            kwargs["part"].update(text="sender mutation", media_ref="other media")
            return {"outcome": "known_not_sent", "retry_without_formatting": True}
        return {"outcome": "sent", "provider_message_id": "plain-receipt"}

    def authorize(context):
        contexts.append(context)
        return EgressAuthorizationDecision.ALLOWED

    owner = engine(path, provider, normalize_parts=lambda parts: parts,
                   **({"authorization_mode": "enforced", "authorize_delivery": authorize}
                      if enforced else {}))
    output = commit(owner, parts=(part,))
    with sqlite3.connect(path) as db:
        original = db.execute("SELECT * FROM egress_outputs").fetchone()
    await owner._dispatch_output(output)
    assert [call["part"] for call in calls] == [
        {**part, "caption_entities" if photo else "entities": tuple(formatting)}, plain]
    assert all(call["recipient"] == "r" and call["part_index"] == 0 for call in calls)
    assert rows(path) == [(0, "sent", 2, 0, "plain-receipt")]
    if enforced:
        assert len(contexts) == 2
        assert contexts[0].request_digest == contexts[1].request_digest
        assert dict(contexts[1].part) == plain
    with sqlite3.connect(path) as db:
        assert db.execute("SELECT * FROM egress_outputs").fetchone() == original
        assert db.execute("SELECT entities,caption_entities FROM egress_parts").fetchone() == (None, None)
    assert commit(owner, parts=(part,)) == output


@pytest.mark.parametrize("hint", [None, False, 1, "true", {}, []])
async def test_only_literal_true_hint_changes_presentation(tmp_path, hint):
    path = tmp_path / "outbox.sqlite3"
    part = {"text": "hello", "entities": [{"type": "bold", "offset": 0, "length": 5}]}
    calls = []

    def provider(**kwargs):
        calls.append(deepcopy(kwargs["part"]))
        return ({"outcome": "known_not_sent", "retry_without_formatting": hint}
                if len(calls) == 1 else {"outcome": "sent"})

    owner = engine(path, provider, normalize_parts=lambda parts: parts)
    output = commit(owner, parts=(part,))
    await owner._dispatch_output(output)
    assert len(calls) == 2 and calls[0] == calls[1]
    with sqlite3.connect(path) as db:
        assert db.execute("SELECT entities FROM egress_parts").fetchone()[0] is not None


@pytest.mark.parametrize("outcome", ["sent", "unknown"])
async def test_hint_cannot_rewrite_accepted_or_unknown_candidate(tmp_path, outcome):
    path = tmp_path / "outbox.sqlite3"
    calls = []
    owner = engine(path, lambda **kw: calls.append(kw) or {
        "outcome": outcome, "retry_without_formatting": True}, normalize_parts=lambda parts: parts)
    output = commit(owner, parts=({"text": "hello", "entities": [
        {"type": "bold", "offset": 0, "length": 5}]},))
    await owner._dispatch_output(output)
    assert len(calls) == 1
    with sqlite3.connect(path) as db:
        assert db.execute("SELECT state,entities FROM egress_parts").fetchone()[0] == outcome
        assert db.execute("SELECT entities FROM egress_parts").fetchone()[0] is not None


async def test_failed_refusal_compare_and_swap_cannot_clear_formatting_or_retry(tmp_path):
    path = tmp_path / "outbox.sqlite3"
    calls = []

    def provider(**kwargs):
        calls.append(kwargs)
        with sqlite3.connect(path) as db:
            db.execute("UPDATE egress_parts SET state='sent',unknown_attempts=0,"
                       "last_outcome='provider_accepted',provider_msg_id='other-handle'")
        return {"outcome": "known_not_sent", "retry_without_formatting": True}

    owner = engine(path, provider, normalize_parts=lambda parts: parts)
    output = commit(owner, parts=({"text": "hello", "entities": [
        {"type": "bold", "offset": 0, "length": 5}]},))
    await owner._dispatch_output(output)
    assert len(calls) == 1
    assert rows(path) == [(0, "sent", 1, 0, "other-handle")]
    with sqlite3.connect(path) as db:
        assert db.execute("SELECT entities FROM egress_parts").fetchone()[0] is not None


async def test_refusal_and_formatting_update_are_one_transaction(tmp_path):
    path = tmp_path / "outbox.sqlite3"
    calls = []
    owner = engine(path, lambda **kw: calls.append(kw) or {
        "outcome": "known_not_sent", "retry_without_formatting": True},
        normalize_parts=lambda parts: parts)
    output = commit(owner, parts=({"text": "hello", "entities": [
        {"type": "bold", "offset": 0, "length": 5}]},))
    with sqlite3.connect(path) as db:
        db.execute("CREATE TRIGGER refuse_plain BEFORE UPDATE ON egress_parts "
                   "WHEN OLD.entities IS NOT NULL AND NEW.entities IS NULL "
                   "BEGIN SELECT RAISE(ABORT, 'test format update refusal'); END")
    with pytest.raises(sqlite3.IntegrityError, match="test format update refusal"):
        await owner._dispatch_output(output)
    with sqlite3.connect(path) as db:
        state, attempts, unknown, outcome, entities = db.execute(
            "SELECT state,attempts,unknown_attempts,last_outcome,entities FROM egress_parts"
        ).fetchone()
        assert (state, attempts, unknown, outcome) == ("pending", 1, 1, "unknown")
        assert entities is not None
        db.execute("DROP TRIGGER refuse_plain")
    reopened = engine(path, lambda **kw: pytest.fail("uncommitted refusal was retried"))
    await reopened.recover_pending()
    assert len(calls) == 1 and rows(path)[0][1] == "unknown"


async def test_multipart_receipts_and_payload_survive_unknown_reopen(tmp_path):
    path = tmp_path / "outbox.sqlite3"
    calls = []

    def provider(**kwargs):
        calls.append(kwargs)
        return (
            {"outcome": "sent", "provider_message_id": "receipt-0"}
            if kwargs["part_index"] == 0
            else {"outcome": "unknown"}
        )

    first = engine(path, provider)
    output = commit(first, parts=({"text": "accepted"}, {"text": "uncertain"}))
    await first._dispatch_output(output)
    assert [c["part_index"] for c in calls] == [0, 1]
    before = rows(path)
    assert before == [(0, "sent", 1, 0, "receipt-0"), (1, "unknown", 1, 1, None)]
    reopened = engine(path, provider)
    assert commit(reopened, parts=({"text": "accepted"}, {"text": "uncertain"})) == output
    await reopened.recover_pending()
    assert rows(path) == before
    assert len(calls) == 2


@pytest.mark.parametrize("change", ["recipient", "payload", "formatting"])
def test_same_key_changed_request_rejected_before_provider_entry(tmp_path, change):
    calls = []
    owner = engine(tmp_path / "outbox.sqlite3", lambda **kwargs: calls.append(kwargs))
    commit(owner)
    with pytest.raises(ValueError):
        commit(
            owner,
            recipient="other" if change == "recipient" else "r",
            parts=(
                {"text": "changed"}
                if change == "payload"
                else {"text": "hello", "entities": [{"type": "bold", "offset": 0, "length": 5}]}
                if change == "formatting"
                else {"text": "hello"},
            ),
        )
    assert calls == []


async def test_duplicate_owner_does_not_repeat_an_entered_part(tmp_path):
    entered, release = asyncio.Event(), asyncio.Event()
    calls = []

    async def provider(**kwargs):
        calls.append(kwargs)
        entered.set()
        await release.wait()
        return {"outcome": "sent", "provider_message_id": "real-receipt"}

    path = tmp_path / "outbox.sqlite3"
    owner = engine(path, provider)
    output = commit(owner)
    first = owner.spawn_dispatch(output)
    await entered.wait()
    duplicate = engine(path, provider)
    await duplicate._dispatch_output(output)
    assert len(calls) == 1
    release.set()
    await first
    assert rows(path) == [(0, "sent", 1, 0, "real-receipt")]


async def test_caller_cancel_and_sender_replacement_retain_entered_dispatch(tmp_path):
    entered, release = asyncio.Event(), asyncio.Event()
    old_calls, new_calls = [], []

    async def old_sender(**kwargs):
        old_calls.append(kwargs)
        entered.set()
        await release.wait()
        return {"outcome": "sent", "provider_message_id": "old-accepted"}

    authority = SenderAuthority(path=tmp_path / "senders.sqlite3")
    authority.attach_active(sender_id="s", owner_id="old", sender=old_sender)
    owner = engine(tmp_path / "outbox.sqlite3", sender_authority=authority)
    output = commit(owner)
    dispatch = owner.spawn_dispatch(output)

    async def waiter():
        await asyncio.wait({dispatch})

    caller = asyncio.create_task(waiter())
    await entered.wait()
    caller.cancel()
    with pytest.raises(asyncio.CancelledError):
        await caller

    class Candidate:
        async def activate(self):
            pass

        async def deactivate(self):
            pass

        async def __call__(self, **kw):
            new_calls.append(kw)
            return {"outcome": "sent"}

    candidate = Candidate()
    activation = authority.stage_activation(sender_id="s", to_owner_id="new", sender=candidate)
    await authority.commit_activation(activation)
    assert dispatch in owner._inflight and not dispatch.done()
    release.set()
    await dispatch
    await asyncio.sleep(0)
    assert new_calls == [] and len(old_calls) == 1
    assert rows(owner.path) == [(0, "sent", 1, 0, "old-accepted")]
    assert dispatch not in owner._inflight


async def test_detached_sender_spends_no_attempt_before_attachment(tmp_path):
    authority = SenderAuthority(path=tmp_path / "senders.sqlite3")
    authority.attach_active(sender_id="s", owner_id="old", sender=lambda **_: {"outcome": "sent"})
    reopened_authority = SenderAuthority(path=tmp_path / "senders.sqlite3")
    owner = engine(tmp_path / "outbox.sqlite3", sender_authority=reopened_authority)
    output = commit(owner)
    await owner._dispatch_output(output)
    assert rows(owner.path)[0][1:4] == ("pending", 0, 0)
    reopened_authority.attach_active(
        sender_id="s", owner_id="old", sender=lambda **_: {"outcome": "acked"}
    )
    await owner._dispatch_output(output)
    assert rows(owner.path)[0][1:4] == ("sent", 1, 0)


def test_host_normalizer_keeps_exact_rich_payload_and_freezes_input(tmp_path):
    payload = [{"text": "hello", "entities": [{"type": "bold", "offset": 0, "length": 5}]}]
    owner = engine(tmp_path / "outbox.sqlite3", normalize_parts=lambda parts: parts)
    commit(owner, parts=payload)
    payload[0]["entities"][0]["length"] = 1
    with sqlite3.connect(owner.path) as db:
        assert (
            json.loads(db.execute("SELECT entities FROM egress_parts").fetchone()[0])[0]["length"]
            == 5
        )


def test_plain_text_keeps_nfc_and_canonical_replay(tmp_path):
    owner = engine(tmp_path / "outbox.sqlite3")
    output = commit(owner, parts=({"text": "e\u0301 AB"},))
    with sqlite3.connect(owner.path) as db:
        assert db.execute("SELECT text FROM egress_parts").fetchone() == ("\u00e9 AB",)
        digest = db.execute("SELECT request_digest FROM egress_outputs").fetchone()
    reopened = engine(owner.path)
    assert commit(reopened, parts=({"text": "\u00e9 AB"},)) == output
    assert commit(reopened, parts=({"text": "e\u0301 AB"},)) == output
    with sqlite3.connect(owner.path) as db:
        assert db.execute("SELECT request_digest FROM egress_outputs").fetchone() == digest
        assert db.execute("SELECT COUNT(*) FROM egress_outputs").fetchone() == (1,)


def test_rich_replay_does_not_reinterpret_predecessor_payload(tmp_path):
    # This is the exact text/entity pair retained by the predecessor after it
    # normalized e + combining acute. Its existing digest must stay immutable.
    stored = {"text": "\u00e9 AB", "entities": [{"type": "bold", "offset": 3, "length": 1}]}
    corrected = {"text": "e\u0301 AB", "entities": stored["entities"]}
    owner = engine(tmp_path / "outbox.sqlite3", normalize_parts=lambda parts: parts)
    output = commit(owner, parts=(stored,))
    with sqlite3.connect(owner.path) as db:
        before = tuple(db.iterdump())
    reopened = engine(owner.path, normalize_parts=lambda parts: parts)
    assert commit(reopened, parts=(stored,)) == output
    with pytest.raises(ValueError, match="normalized payload"):
        commit(reopened, parts=(corrected,))
    with sqlite3.connect(owner.path) as db:
        assert tuple(db.iterdump()) == before
