"""Durable output cancellation across provider results, handles and reopening."""

from __future__ import annotations

import asyncio
from concurrent.futures import ThreadPoolExecutor
import json
import os
import sqlite3
import subprocess
import sys
import threading

import pytest

from meerkat_toolkit.frontend import egress
from meerkat_toolkit.frontend.egress import EgressAuthorizationDecision, EgressMigrationBinding
from test_egress import commit, engine, rows
from test_egress_migration import snapshot


def cancel(owner, output):
    return owner.cancel_output(output_id=output, issuer_id="issuer")


def binding():
    return EgressMigrationBinding("1" * 64, "2" * 64, "3" * 64)


def v1(path):
    """Retain a complete predecessor layout/marker with real delivery rows."""
    owner = engine(path)
    output = commit(owner, parts=tuple({"text": f"part-{i}"} for i in range(5)))
    with sqlite3.connect(path) as db:
        db.execute("UPDATE egress_parts SET state='sent',attempts=1,"
                   "last_outcome='provider_accepted',provider_msg_id='accepted' WHERE part_index=0")
        db.execute("UPDATE egress_parts SET state='unknown',attempts=2,unknown_attempts=1,"
                   "last_outcome='unknown',provider_msg_id='uncertain' WHERE part_index=1")
        db.execute("UPDATE egress_parts SET attempts=1,unknown_attempts=1,last_outcome='unknown' WHERE part_index=2")
        db.execute("UPDATE egress_parts SET attempts=1,last_outcome='known_not_sent',error='refused' WHERE part_index=3")
        db.execute("ALTER TABLE egress_outputs DROP COLUMN cancel_requested")
        db.execute("ALTER TABLE egress_outputs DROP COLUMN thread_id")
        db.execute("ALTER TABLE egress_outputs DROP COLUMN delivery_policy")
        for name in ("attempt_open", "unknown_resends_used", "unknown_history"):
            db.execute(f"ALTER TABLE egress_parts DROP COLUMN {name}")
    unmarked = egress.ProactiveEgress.inspect_format(path=path)
    with sqlite3.connect(path) as db:
        db.execute(egress._FORMAT_SQL)
        db.execute("INSERT INTO egress_format VALUES (?,?,?,?,?,?,?,?,?)", (
            1, egress.EGRESS_STATE_NAMESPACE, egress.EGRESS_STATE_OWNER,
            "egress-sqlite-1", "adopt-egress-sqlite-1", "unmarked_current",
            "4" * 64, unmarked.schema_digest,
            egress._canonical_json(egress.asdict(binding())).decode(),
        ))
    return output, egress.ProactiveEgress.inspect_format(path=path)


async def test_cancel_before_dispatch_is_durable_and_replay_cannot_resume(tmp_path):
    calls = []
    path = tmp_path / "outbox.sqlite3"
    owner = engine(path, lambda **kw: calls.append(kw))
    output = commit(owner, parts=({"text": "first"}, {"text": "second"}))
    status = cancel(owner, output)
    assert status.cancel_requested and status.part_states == ("cancelled", "cancelled")
    assert status.delivery_state == "cancelled"
    after = snapshot(path)
    assert cancel(owner, output) == status
    assert snapshot(path) == after
    assert commit(owner, parts=({"text": "first"}, {"text": "second"})) == output
    reopened = engine(path, lambda **kw: calls.append(kw))
    await reopened._dispatch_output(output)
    summary = await reopened.run_recovery_pass(respect_grace=False)
    assert summary["dispatched"] == summary["pending_remaining"] == 0
    assert reopened.backlog_summary()["pending_parts"] == 0
    assert calls == [] and reopened.attempt_count(output_id=output) == 0
    assert reopened.status(output_id=output) == status


@pytest.mark.parametrize("output_id,issuer_id", [(None, "issuer"), ("missing", "issuer"), ("actual", "other"), ("actual", None)])
def test_wrong_identity_is_nonmutating(tmp_path, output_id, issuer_id):
    owner = engine(tmp_path / "outbox.sqlite3")
    actual = commit(owner)
    before = snapshot(owner.path)
    with pytest.raises(ValueError):
        owner.cancel_output(output_id=actual if output_id == "actual" else output_id, issuer_id=issuer_id)
    assert snapshot(owner.path) == before


@pytest.mark.parametrize("outcome,state,unknown", [("sent", "sent", 0), ("unknown", "unknown", 1), ("known_not_sent", "cancelled", 0)])
async def test_stop_preserves_late_provider_fact_and_blocks_next_part(tmp_path, outcome, state, unknown):
    entered, release = asyncio.Event(), asyncio.Event()
    calls = []

    async def provider(**kw):
        calls.append(kw)
        entered.set()
        await release.wait()
        return {"outcome": outcome, "provider_message_id": "receipt", "retry_without_formatting": True}

    owner = engine(tmp_path / "outbox.sqlite3", provider, normalize_parts=lambda parts: parts)
    output = commit(owner, parts=({"text": "first", "entities": [{"type": "bold", "offset": 0, "length": 5}]}, {"text": "later"}))
    task = owner.spawn_dispatch(output)
    await entered.wait()
    before = cancel(owner, output)
    assert before.part_states == ("unknown", "cancelled") and before.cancel_requested
    assert not task.cancelled()
    release.set()
    await task
    assert len(calls) == 1
    assert rows(owner.path) == [(0, state, 1, unknown, "receipt"), (1, "cancelled", 0, 0, None)]
    await engine(owner.path, provider).recover_pending()
    assert len(calls) == 1


async def test_stop_during_authorization_spends_no_attempt(tmp_path):
    entered, release = asyncio.Event(), asyncio.Event()
    calls = []

    async def authorize(context):
        entered.set()
        await release.wait()
        return EgressAuthorizationDecision.ALLOWED

    owner = engine(tmp_path / "outbox.sqlite3", lambda **kw: calls.append(kw),
                   authorization_mode="enforced", authorize_delivery=authorize)
    output = commit(owner)
    task = owner.spawn_dispatch(output)
    await entered.wait()
    cancel(engine(owner.path), output)
    release.set()
    await task
    assert calls == [] and rows(owner.path) == [(0, "cancelled", 0, 0, None)]


async def test_stop_during_retry_backoff_and_recovery_preserves_refusal(tmp_path):
    entered, release = asyncio.Event(), asyncio.Event()
    calls = []

    async def backoff(delay):
        assert delay == 5
        entered.set()
        await release.wait()

    def provider(**kw):
        calls.append(kw)
        return {"outcome": "known_not_sent", "provider_message_id": "refused"}

    owner = engine(tmp_path / "outbox.sqlite3", provider, sleep=backoff)
    output = commit(owner)
    task = owner.spawn_dispatch(output)
    await entered.wait()
    cancel(engine(owner.path), output)
    release.set()
    await task
    await engine(owner.path, provider).recover_pending()
    assert len(calls) == 1 and rows(owner.path) == [(0, "cancelled", 1, 0, "refused")]


@pytest.mark.parametrize("operation", ["reserve", "refusal"])
def test_concurrent_transaction_order_never_resurrects_stopped_output(tmp_path, operation):
    # Threads use independent SQLite connections, not the per-owner asyncio lock.
    for trial in range(12):
        owner = engine(tmp_path / f"outbox-{trial}.sqlite3")
        peer = engine(owner.path)
        output = commit(owner)
        if operation == "refusal":
            assert owner._reserve_attempt(output, 0, 0)
        barrier = threading.Barrier(2)

        def stop():
            barrier.wait(timeout=5)
            return cancel(peer, output)

        def compete():
            barrier.wait(timeout=5)
            if operation == "reserve":
                return owner._reserve_attempt(output, 0, 0)
            return owner._update_part(output, 0, state="pending", attempts=1,
                                      unknown_attempts=0, provider_msg_id="refusal",
                                      last_outcome="known_not_sent", error="refused")

        with ThreadPoolExecutor(max_workers=2) as pool:
            stopped, competed = pool.submit(stop), pool.submit(compete)
            assert stopped.result(timeout=10).cancel_requested
            won = competed.result(timeout=10)
        if operation == "reserve" and won:
            assert rows(owner.path)[0][1:4] == ("unknown", 1, 1)
        else:
            assert rows(owner.path)[0][1] == "cancelled"
        assert not peer._reserve_attempt(output, 0, int(operation == "refusal"))


def test_fresh_process_recovery_retains_unknown_and_never_enters_provider(tmp_path):
    path = tmp_path / "outbox.sqlite3"
    owner = engine(path)
    output = commit(owner, parts=({"text": "entered"}, {"text": "later"}))
    assert owner._reserve_attempt(output, 0, 0)
    cancel(owner, output)
    script = """
import asyncio,json,sys
from pathlib import Path
from meerkat_toolkit.frontend.egress import ProactiveEgress
calls=[]
def forbidden(**kw):
    calls.append(kw)
    return {'outcome':'sent'}
owner=ProactiveEgress(path=Path(sys.argv[1]),senders={'s':forbidden},incident_sink=lambda _:None,health_sink=lambda _:None,sleep=lambda _:None)
asyncio.run(owner.recover_pending())
assert calls == []
s=owner.status(output_id=sys.argv[2])
print(json.dumps({'states':s.part_states,'cancel_requested':s.cancel_requested,'pending':owner.backlog_summary()['pending_parts']}))
"""
    result = subprocess.run([sys.executable, "-c", script, str(path), output],
                            capture_output=True, text=True, timeout=10, check=True, env=os.environ.copy())
    assert json.loads(result.stdout) == {"states": ["unknown", "cancelled"], "cancel_requested": True, "pending": 0}


def test_v1_upgrade_is_explicit_preserves_facts_and_links_exact_predecessor(tmp_path):
    path = tmp_path / "outbox.sqlite3"
    output, before = v1(path)
    retained = rows(path)
    assert before.state == "managed" and before.format == "egress-sqlite-1"
    unchanged = snapshot(path)
    for mode in ["managed", "legacy"]:
        with pytest.raises(ValueError, match="managed egress format required"):
            engine(path, schema_mode=mode)
        assert snapshot(path) == unchanged
    upgraded = egress.ProactiveEgress.migrate_format(path=path, expected=before, binding=binding())
    assert upgraded.format == egress.EGRESS_STATE_FORMAT
    assert upgraded.transition.source_state == "managed_v1"
    assert upgraded.transition.source_observation_digest == before.observation_digest
    assert rows(path) == retained
    owner = engine(path, schema_mode="managed")
    assert not owner.status(output_id=output).cancel_requested
    result = cancel(owner, output)
    assert result.part_states == ("sent", "unknown", "unknown", "cancelled", "cancelled")
    assert result.delivery_state == "unknown"
    with pytest.raises(ValueError, match="invalid egress migration request"):
        egress.ProactiveEgress.migrate_format(path=path, expected=upgraded, binding=binding())


@pytest.mark.parametrize("failure", ["alter", "record"])
def test_v1_upgrade_failure_restores_exact_old_schema_and_marker(tmp_path, monkeypatch, failure):
    path = tmp_path / "outbox.sqlite3"
    _, before = v1(path)
    retained = snapshot(path)
    original = egress._format_connection

    def connection(*args):
        db = original(*args)

        def authorize(action, first, second, database, caller):
            if failure == "alter" and action == sqlite3.SQLITE_ALTER_TABLE:
                return sqlite3.SQLITE_DENY
            if failure == "record" and action == sqlite3.SQLITE_INSERT and first == "egress_format":
                return sqlite3.SQLITE_DENY
            return sqlite3.SQLITE_OK

        db.set_authorizer(authorize)
        return db

    with monkeypatch.context() as patch:
        patch.setattr(egress, "_format_connection", connection)
        with pytest.raises(sqlite3.DatabaseError):
            egress.ProactiveEgress.migrate_format(path=path, expected=before, binding=binding())
    assert snapshot(path) == retained
    assert egress.ProactiveEgress.inspect_format(path=path) == before


@pytest.mark.parametrize("damage", ["v1_with_new_column", "v2_without_column", "wrong_operation", "wrong_source"])
def test_mixed_version_markers_refuse_without_mutation(tmp_path, damage):
    path = tmp_path / "outbox.sqlite3"
    _, before = v1(path)
    with sqlite3.connect(path) as db:
        if damage == "v1_with_new_column":
            db.execute("ALTER TABLE egress_outputs ADD COLUMN cancel_requested INTEGER NOT NULL DEFAULT 0")
        elif damage == "v2_without_column":
            db.execute("UPDATE egress_format SET format='egress-sqlite-2',operation='adopt-egress-sqlite-2'")
        elif damage == "wrong_operation":
            db.execute("UPDATE egress_format SET operation='adopt-egress-sqlite-2'")
        else:
            db.execute("UPDATE egress_format SET source_state='managed_v1'")
    retained = snapshot(path)
    with pytest.raises(ValueError):
        egress.ProactiveEgress.inspect_format(path=path)
    with pytest.raises(ValueError):
        egress.ProactiveEgress.migrate_format(path=path, expected=before, binding=binding())
    assert snapshot(path) == retained


def test_completed_and_incident_output_status_never_claims_recall(tmp_path):
    owner = engine(tmp_path / "outbox.sqlite3")
    output = owner.commit_output(issuer_id="incident_drain", logical_key="incident:1",
                                 candidates=(("s", "r"),), parts=({"text": "accepted"},), incident_delivery=True)
    assert owner._reserve_attempt(output, 0, 0)
    assert owner._update_part(output, 0, state="sent", attempts=1, unknown_attempts=0,
                              provider_msg_id="accepted", last_outcome="provider_accepted", error=None)
    status = owner.cancel_output(output_id=output, issuer_id="incident_drain")
    assert status.cancel_requested and status.delivery_state == "provider_accepted"
    assert owner.incident_cycles("incident") == [(1, output, "sent")]


async def test_fourth_attempt_refusal_after_stop_is_cancelled_without_changing_prior_terminals(tmp_path):
    entered, release = asyncio.Event(), asyncio.Event()

    async def provider(**kw):
        entered.set()
        await release.wait()
        return {"outcome": "known_not_sent", "provider_message_id": "fourth-refusal"}

    owner = engine(tmp_path / "outbox.sqlite3", provider)
    output = commit(owner, parts=tuple({"text": str(i)} for i in range(3)))
    with sqlite3.connect(owner.path) as db:
        db.execute("UPDATE egress_parts SET attempts=3,last_outcome='known_not_sent' WHERE part_index=0")
        db.execute("UPDATE egress_parts SET state='failed',attempts=4,last_outcome='known_not_sent',provider_msg_id='prior-refusal' WHERE part_index=1")
        db.execute("UPDATE egress_parts SET state='expired',error='prior expiry' WHERE part_index=2")
    task = owner.spawn_dispatch(output)
    await entered.wait()
    cancel(owner, output)
    release.set()
    await task
    assert rows(owner.path) == [(0, "cancelled", 4, 0, "fourth-refusal"),
                                (1, "failed", 4, 0, "prior-refusal"),
                                (2, "expired", 0, 0, None)]


def test_cancel_transaction_failure_rolls_back_intent_and_part_states(tmp_path, monkeypatch):
    owner = engine(tmp_path / "outbox.sqlite3")
    output = commit(owner)
    before = snapshot(owner.path)
    original = owner._connect

    def connect():
        db = original()
        db.set_authorizer(lambda action, first, second, database, caller:
                          sqlite3.SQLITE_DENY if action == sqlite3.SQLITE_UPDATE and first == "egress_parts"
                          else sqlite3.SQLITE_OK)
        return db

    with monkeypatch.context() as patch:
        patch.setattr(owner, "_connect", connect)
        with pytest.raises(sqlite3.DatabaseError):
            cancel(owner, output)
    assert snapshot(owner.path) == before
