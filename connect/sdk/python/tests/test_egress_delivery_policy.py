"""Bounded UNKNOWN retries in the existing SQLite delivery owner."""
import asyncio
import json
from pathlib import Path
import sqlite3

import pytest

from meerkat_toolkit.frontend.egress import (
    EgressAuthorizationDecision as Decision,
    EgressDeliveryPolicy,
    EgressMigrationBinding,
    ProactiveEgress,
)


ONCE = EgressDeliveryPolicy("at_least_once", 1)


def owner(path, provider, *, policy=ONCE, **kwargs):
    if not path.exists():
        ProactiveEgress.migrate_format(path=path,
            expected=ProactiveEgress.inspect_format(path=path),
            binding=EgressMigrationBinding("1" * 64, "2" * 64, "3" * 64))
    return ProactiveEgress(path=path, schema_mode="managed", senders={"s": provider},
        delivery_policy=policy, authorization_mode="enforced",
        authorize_delivery=kwargs.pop("authorize", lambda _: Decision.ALLOWED),
        incident_sink=kwargs.pop("incident_sink", lambda _: None),
        health_sink=lambda _: None, sleep=kwargs.pop("sleep", lambda _: None), **kwargs)


def commit(engine, key="reply", parts=({"text": "hello"},)):
    return engine.commit_output(issuer_id="host/person", logical_key=key,
        candidates=(("s", "host-selected"),), parts=parts, incident_delivery=False)


def row(path):
    with sqlite3.connect(path) as db:
        db.row_factory = sqlite3.Row
        return dict(db.execute("SELECT * FROM egress_parts").fetchone())


def stop_backoff(_):
    raise RuntimeError("host stopped between attempts")


@pytest.mark.parametrize("mode,count", [
    ("at_most_once", 1), ("at_least_once", 0), ("at_least_once", 4),
    ("at_least_once", True), ("unknown", 1),
])
def test_policy_rejects_unbounded_or_ambiguous_configuration(mode, count):
    with pytest.raises(ValueError, match="delivery policy"):
        EgressDeliveryPolicy(mode, count)


async def test_default_never_resends_and_retains_fixed_reason(tmp_path):
    calls = []
    def send(**kwargs):
        calls.append(kwargs)
        return {"outcome": "unknown", "error": "private provider prose"}
    path = tmp_path / "outbox.sqlite3"
    first = owner(path, send, policy=EgressDeliveryPolicy())
    output = commit(first)
    await first._dispatch_output(output)
    reopened = owner(path, send)
    assert commit(reopened) == output
    await reopened.recover_pending(previous_dispatches_quiesced=True)
    assert len(calls) == 1
    status = reopened.status(output_id=output)
    assert status.delivery_state == "unknown"
    assert status.delivery_policy == EgressDeliveryPolicy()
    assert [(x.attempt, x.reason) for x in status.uncertain_attempts] == [(1, "provider_unknown")]
    assert "private provider prose" not in row(path)["unknown_history"]


async def test_unknown_then_success_retains_history_and_reauthorizes(tmp_path):
    calls, authorized, delays = [], [], []
    def send(**kwargs):
        calls.append(kwargs)
        return {"outcome": "unknown"} if len(calls) == 1 else {"outcome": "sent", "provider_message_id": "42"}
    path = tmp_path / "outbox.sqlite3"
    engine = owner(path, send, sleep=delays.append,
        authorize=lambda context: authorized.append(context) or Decision.ALLOWED)
    output = commit(engine)
    await engine._dispatch_output(output)
    assert len(calls) == len(authorized) == 2
    assert delays == [5]
    assert engine.status(output_id=output).delivery_state == "provider_accepted"
    assert len(engine.status(output_id=output).uncertain_attempts) == 1
    assert row(path)["unknown_resends_used"] == 1
    assert row(path)["provider_msg_id"] == "42"


async def test_mixed_outcomes_reopen_do_not_replenish_unknown_quota(tmp_path):
    path = tmp_path / "outbox.sqlite3"
    outcomes = iter(["unknown", "known_not_sent", "unknown"])
    calls, incidents = [], []
    def send(**kwargs):
        calls.append(kwargs)
        return {"outcome": next(outcomes)}
    first = owner(path, send, sleep=stop_backoff)
    output = commit(first)
    with pytest.raises(RuntimeError, match="host stopped"):
        await first._dispatch_output(output)
    reopened = owner(path, send, policy=EgressDeliveryPolicy("at_least_once", 3),
                     incident_sink=incidents.append)
    assert commit(reopened) == output
    await reopened.recover_pending()
    assert len(calls) == 3 and row(path)["unknown_resends_used"] == 1
    status = reopened.status(output_id=output)
    assert status.delivery_policy == ONCE and status.delivery_state == "unknown"
    assert [x.attempt for x in status.uncertain_attempts] == [1, 3]
    assert len(incidents) == 1 and incidents[0]["payload"]["attempts"] == 3


async def test_unknown_resend_cannot_exceed_four_total_attempts(tmp_path):
    calls, delays, incidents, authorized = [], [], [], []
    outcomes = iter(["known_not_sent", "known_not_sent", "unknown", "unknown"])
    def send(**kwargs):
        calls.append(kwargs)
        return {"outcome": next(outcomes)}
    engine = owner(tmp_path / "outbox.sqlite3", send, sleep=delays.append,
        incident_sink=incidents.append,
        authorize=lambda context: authorized.append(context) or Decision.ALLOWED)
    output = commit(engine)
    await engine._dispatch_output(output)
    assert len(calls) == len(authorized) == 4 and delays == [5, 30, 120]
    assert [x.attempt for x in engine.status(output_id=output).uncertain_attempts] == [3, 4]
    assert len(incidents) == 1 and incidents[0]["payload"]["attempts"] == 4


async def test_reauthorization_denial_and_stop_fence_a_recorded_unknown(tmp_path):
    path = tmp_path / "outbox.sqlite3"
    calls = []
    def send(**kwargs):
        calls.append(kwargs)
        return {"outcome": "unknown"}
    first = owner(path, send, sleep=stop_backoff)
    output = commit(first)
    with pytest.raises(RuntimeError):
        await first._dispatch_output(output)
    denied = owner(path, send, authorize=lambda _: Decision.DENIED)
    await denied.recover_pending()
    assert len(calls) == 1 and row(path)["unknown_resends_used"] == 0
    denied.cancel_output(output_id=output, issuer_id="host/person")
    await owner(path, send).recover_pending(previous_dispatches_quiesced=True)
    assert len(calls) == 1
    assert denied.status(output_id=output).cancel_requested
    assert denied.status(output_id=output).delivery_state == "unknown"


async def test_abandoned_reservation_requires_explicit_quiescence(tmp_path):
    path = tmp_path / "outbox.sqlite3"
    calls, incidents = [], []
    def send(**kwargs):
        calls.append(kwargs)
        return {"outcome": "sent"}
    first = owner(path, send)
    output = commit(first)
    assert first._reserve_attempt(output, 0, 0)
    reopened = owner(path, send, incident_sink=incidents.append)
    await reopened.recover_pending()
    assert calls == [] and row(path)["attempt_open"] == 1
    assert incidents == []  # A competing live handle is not exhaustion.
    for invalid in [1, "true", None]:
        with pytest.raises(ValueError, match="boolean"):
            await reopened.recover_pending(previous_dispatches_quiesced=invalid)
    await reopened.recover_pending(previous_dispatches_quiesced=True)
    assert len(calls) == 1 and row(path)["attempts"] == 2
    assert row(path)["unknown_resends_used"] == 1
    assert [(x.attempt, x.reason) for x in reopened.status(output_id=output).uncertain_attempts] == [(1, "result_lost")]


async def test_cancelled_dispatch_is_uncertainty_not_user_stop(tmp_path):
    path = tmp_path / "outbox.sqlite3"
    entered = asyncio.Event()
    async def send(**kwargs):
        entered.set()
        await asyncio.Event().wait()
    first = owner(path, send)
    output = commit(first)
    task = asyncio.create_task(first._dispatch_output(output))
    await entered.wait()
    task.cancel()
    with pytest.raises(asyncio.CancelledError):
        await task
    assert not first.status(output_id=output).cancel_requested
    assert first.status(output_id=output).uncertain_attempts[0].reason == "dispatch_cancelled"
    calls = []
    reopened = owner(path, lambda **kwargs: calls.append(kwargs) or {"outcome": "sent"})
    await reopened.recover_pending()
    assert len(calls) == 1 and reopened.status(output_id=output).delivery_state == "provider_accepted"


async def test_incident_callback_failure_does_not_erase_unknown_evidence(tmp_path):
    def incident(_):
        raise RuntimeError("host sink unavailable")
    engine = owner(tmp_path / "outbox.sqlite3", lambda **_: {"outcome": "unknown"},
                   incident_sink=incident)
    output = commit(engine)
    with pytest.raises(RuntimeError, match="host sink unavailable"):
        await engine._dispatch_output(output)
    status = engine.status(output_id=output)
    assert status.delivery_state == "unknown" and len(status.uncertain_attempts) == 2


@pytest.mark.parametrize("terminal", ["budget", "stop", "expiry"])
async def test_later_refusal_never_erases_earlier_unknown(tmp_path, terminal):
    path = tmp_path / "outbox.sqlite3"
    calls, incidents = [], []
    def send(**kwargs):
        calls.append(kwargs)
        return {"outcome": "unknown" if len(calls) == 1 else "known_not_sent"}
    def pause(delay):
        if terminal != "budget" and len(calls) == 2:
            stop_backoff(delay)
    engine = owner(path, send, sleep=pause, incident_sink=incidents.append)
    output = commit(engine)
    if terminal == "budget":
        await engine._dispatch_output(output)
        assert len(calls) == 4
    else:
        with pytest.raises(RuntimeError):
            await engine._dispatch_output(output)
        assert engine.status(output_id=output).delivery_state == "unknown"
        if terminal == "stop":
            engine.cancel_output(output_id=output, issuer_id="host/person")
        else:
            owner(path, send, freshness_seconds=-1, incident_sink=incidents.append).expire_stale()
    status = engine.status(output_id=output)
    assert status.part_states == ("unknown",) and status.delivery_state == "unknown"
    assert [x.attempt for x in status.uncertain_attempts] == [1]
    assert len(incidents) == (0 if terminal == "stop" else 1)


@pytest.mark.parametrize("photo", [False, True])
async def test_plain_candidate_survives_restart_under_selected_policy(tmp_path, photo):
    path = tmp_path / "outbox.sqlite3"
    formatting = [{"type": "bold", "offset": 0, "length": 5}]
    part = ({"media_ref": "host:photo", "caption": "hello", "caption_entities": formatting}
            if photo else {"text": "hello", "entities": formatting})
    calls, delays = [], []
    def send(**kwargs):
        calls.append(kwargs["part"])
        return ({"outcome": "known_not_sent", "retry_without_formatting": True}
                if len(calls) == 1 else {"outcome": "sent"})
    def pause(delay):
        delays.append(delay)
        stop_backoff(delay)
    first = owner(path, send, sleep=pause, normalize_parts=lambda p: p)
    output = commit(first, parts=(part,))
    with sqlite3.connect(path) as db:
        digest = db.execute("SELECT request_digest FROM egress_outputs").fetchone()[0]
    with pytest.raises(RuntimeError):
        await first._dispatch_output(output)
    reopened = owner(path, send, normalize_parts=lambda p: p)
    await reopened.recover_pending()
    assert delays == [5] and len(calls) == 2
    assert "entities" not in calls[1] and "caption_entities" not in calls[1]
    with sqlite3.connect(path) as db:
        assert db.execute("SELECT request_digest FROM egress_outputs").fetchone()[0] == digest
    assert row(path)["unknown_resends_used"] == 0


async def test_actual_v3_rows_upgrade_without_new_resend_authority(tmp_path):
    path = tmp_path / "outbox.sqlite3"
    with sqlite3.connect(path) as db:
        db.executescript(Path(__file__).with_name("fixtures").joinpath("egress-sqlite-3.sql").read_text())
        original = db.execute("SELECT * FROM egress_parts ORDER BY part_index").fetchall()
        columns = [r[1] for r in db.execute("PRAGMA table_info(egress_parts)")]
        output_columns = [r[1] for r in db.execute("PRAGMA table_info(egress_outputs)")]
        original_output = db.execute("SELECT * FROM egress_outputs").fetchone()
    before = ProactiveEgress.inspect_format(path=path)
    original_bytes = path.read_bytes()
    assert before.format == "egress-sqlite-3"
    for mode in ("legacy", "managed"):
        with pytest.raises(ValueError, match="managed egress format required"):
            ProactiveEgress(path=path, schema_mode=mode, senders={}, delivery_policy=ONCE,
                incident_sink=lambda _: None, health_sink=lambda _: None, sleep=lambda _: None)
        assert path.read_bytes() == original_bytes
    after = ProactiveEgress.migrate_format(path=path, expected=before,
        binding=EgressMigrationBinding("4" * 64, "5" * 64, "6" * 64))
    assert after.format == "egress-sqlite-4" and after.transition.source_state == "managed_v3"
    assert after.transition.source_observation_digest == before.observation_digest
    with sqlite3.connect(path) as db:
        assert db.execute("SELECT " + ",".join(columns) + " FROM egress_parts ORDER BY part_index").fetchall() == original
        assert db.execute("SELECT " + ",".join(output_columns) + " FROM egress_outputs").fetchone() == original_output
        assert db.execute("SELECT delivery_policy FROM egress_outputs").fetchone() == (None,)
        assert db.execute("SELECT attempt_open,unknown_resends_used,unknown_history FROM egress_parts").fetchall() == [(0, 0, None)] * 5
        output = original_output[0]
        db.execute("UPDATE egress_outputs SET created_at_ms=?", (10**16,))
    calls, contexts = [], []
    def send(*, thread_id=None, **kwargs):
        calls.append((kwargs["part_index"], thread_id))
        return {"outcome": "unknown"}
    current = owner(path, send, authorize=lambda c: contexts.append(c) or Decision.ALLOWED)
    await current.recover_pending(previous_dispatches_quiesced=True)
    # Old sent/unknown/open rows are never replayed. Previously unentered and
    # proved-refusal rows may continue, but receive no UNKNOWN resend quota.
    assert calls == [(3, "47"), (4, "47")] and len(contexts) == 2
    status = current.status(output_id=output)
    assert status.delivery_policy is None and status.part_states == ("sent", "unknown", "unknown", "unknown", "unknown")
    assert [(x.part_index, x.attempt) for x in status.uncertain_attempts] == [(3, 1), (4, 2)]
    with sqlite3.connect(path) as db:
        assert db.execute("SELECT unknown_resends_used FROM egress_parts").fetchall() == [(0,)] * 5
        assert db.execute("SELECT provider_msg_id FROM egress_parts WHERE part_index<2 ORDER BY part_index").fetchall() == [("accepted-1",), ("uncertain-1",)]
