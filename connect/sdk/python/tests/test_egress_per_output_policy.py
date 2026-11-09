"""First-commit delivery policy selection on the existing shared owner."""
from concurrent.futures import ThreadPoolExecutor
import json
from pathlib import Path
import sqlite3
import threading

import pytest

from meerkat_toolkit.frontend.egress import EgressDeliveryPolicy, ProactiveEgress
from test_egress_delivery_policy import ONCE, owner


CONSERVATIVE = EgressDeliveryPolicy()


def request(key='reply'):
    return dict(issuer_id='host/person', logical_key=key, candidates=(('s', 'host-selected'),),
                parts=({'text': 'hello'},), incident_delivery=False)


def stored(path):
    with sqlite3.connect(path) as db:
        return {table: db.execute(f'SELECT * FROM {table} ORDER BY rowid').fetchall()
                for table in ('egress_outputs', 'egress_parts')}


async def test_one_owner_selects_two_policies_and_send_forwards_selection(tmp_path):
    calls = []
    def send(**kwargs):
        calls.append(kwargs)
        return {'outcome': 'unknown'}
    engine = owner(tmp_path / 'egress.sqlite3', send, policy=CONSERVATIVE)
    conservative = await engine.send(**request('default'))
    selected = await engine.send(**request('reply'), delivery_policy=ONCE)
    assert engine.status(output_id=conservative).delivery_policy == CONSERVATIVE
    assert engine.status(output_id=selected).delivery_policy == ONCE
    assert engine.attempt_count(output_id=conservative) == 1
    assert engine.attempt_count(output_id=selected) == 2
    assert len(calls) == 3
    # Explicit conservative selection overrides an opt-in constructor for a new row.
    another = owner(engine.path, send)
    explicit = await another.send(**request('explicit-safe'), delivery_policy=CONSERVATIVE)
    assert another.status(output_id=explicit).delivery_policy == CONSERVATIVE
    assert another.attempt_count(output_id=explicit) == 1


async def test_reopen_replay_preserves_policy_receipt_history_quota_and_digest(tmp_path):
    calls = []
    def send(**kwargs):
        calls.append(kwargs)
        return {'outcome': 'unknown'} if len(calls) == 1 else {'outcome': 'sent', 'provider_message_id': 'receipt'}
    engine = owner(tmp_path / 'egress.sqlite3', send, policy=CONSERVATIVE)
    selected = await engine.send(**request(), delivery_policy=ONCE)
    before = stored(engine.path)
    reopened = owner(engine.path, send, policy=EgressDeliveryPolicy('at_least_once', 3))
    for selection in [None, CONSERVATIVE, EgressDeliveryPolicy('at_least_once', 3)]:
        assert reopened.commit_output(**request(), delivery_policy=selection) == selected
        assert stored(engine.path) == before
        assert reopened.status(output_id=selected).delivery_policy == ONCE
    await reopened.send(**request(), delivery_policy=EgressDeliveryPolicy('at_least_once', 3))
    assert len(calls) == 2 and stored(engine.path) == before
    assert len(reopened.status(output_id=selected).uncertain_attempts) == 1
    with sqlite3.connect(engine.path) as db:
        assert db.execute('SELECT unknown_resends_used,provider_msg_id FROM egress_parts').fetchone() == (1, 'receipt')
    with pytest.raises(ValueError, match='logical-key reuse'):
        reopened.commit_output(**{**request(), 'parts': ({'text': 'changed'},)}, delivery_policy=ONCE)
    assert stored(engine.path) == before


def test_conflicting_writer_selections_retain_the_first_committed_policy(tmp_path):
    path = tmp_path / 'egress.sqlite3'
    first = owner(path, lambda **_: {'outcome': 'sent'}, policy=CONSERVATIVE)
    second = owner(path, lambda **_: {'outcome': 'sent'}, policy=ONCE)
    barrier = threading.Barrier(2)
    def commit(engine, selected):
        barrier.wait(timeout=3)
        output = engine.commit_output(**request(), delivery_policy=selected)
        return output, engine.status(output_id=output).delivery_policy
    with ThreadPoolExecutor(max_workers=2) as pool:
        one = pool.submit(commit, first, CONSERVATIVE)
        two = pool.submit(commit, second, ONCE)
        left, right = one.result(timeout=5), two.result(timeout=5)
    assert left == right and left[1] in (CONSERVATIVE, ONCE)
    before = stored(path)
    assert len(before['egress_outputs']) == len(before['egress_parts']) == 1
    for engine, selected in [(first, ONCE), (second, CONSERVATIVE)]:
        assert engine.commit_output(**request(), delivery_policy=selected) == left[0]
    assert stored(path) == before


@pytest.mark.parametrize('historical_null', [False, True])
def test_unmanaged_replay_keeps_saved_policy_but_new_opt_in_refuses(tmp_path, historical_null):
    path = tmp_path / 'egress.sqlite3'
    args = dict(path=path, senders={}, incident_sink=lambda _: None,
                health_sink=lambda _: None, sleep=lambda _: None)
    engine = ProactiveEgress(**args)
    output = engine.commit_output(**request())
    if historical_null:
        # Represent the existing pre-policy row contract, without migration claims.
        with sqlite3.connect(path) as db:
            db.execute('UPDATE egress_outputs SET delivery_policy=NULL')
    before = stored(path)
    reopened = ProactiveEgress(**args, delivery_policy=ONCE)
    for options in [{}, {'delivery_policy': ONCE}]:
        assert reopened.commit_output(**request(), **options) == output
        assert stored(path) == before
    assert reopened.status(output_id=output).delivery_policy == (None if historical_null else CONSERVATIVE)
    for options in [{}, {'delivery_policy': ONCE}]:
        with pytest.raises(ValueError, match='UNKNOWN resends require managed'):
            reopened.commit_output(**request('new'), **options)
        assert stored(path) == before
    safe = reopened.commit_output(**request('new'), delivery_policy=CONSERVATIVE)
    assert reopened.status(output_id=safe).delivery_policy == CONSERVATIVE


@pytest.mark.parametrize('selection', [False, {'mode': 'at_least_once', 'unknown_resends': 1}, 'at_most_once'])
def test_untyped_policy_refuses_before_persistent_effect_even_on_replay(tmp_path, selection):
    engine = owner(tmp_path / 'egress.sqlite3', lambda **_: {'outcome': 'sent'})
    engine.commit_output(**request(), delivery_policy=CONSERVATIVE)
    before = stored(engine.path)
    for key in ['reply', 'new']:
        with pytest.raises(ValueError, match='invalid egress delivery policy'):
            engine.commit_output(**request(key), delivery_policy=selection)
        assert stored(engine.path) == before


def test_policy_value_is_revalidated_before_commit(tmp_path):
    engine = owner(tmp_path / 'egress.sqlite3', lambda **_: {'outcome': 'sent'})
    forged = EgressDeliveryPolicy()
    object.__setattr__(forged, 'unknown_resends', 999)
    before = stored(engine.path)
    with pytest.raises(ValueError, match='invalid egress delivery policy'):
        engine.commit_output(**request(), delivery_policy=forged)
    assert stored(engine.path) == before
