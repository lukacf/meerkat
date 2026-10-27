"""Atomic legacy-schema adoption through the real SQLite egress owner."""
from __future__ import annotations

import asyncio
from pathlib import Path
import sqlite3
import threading
import time

import pytest

from meerkat_toolkit.frontend.egress import ProactiveEgress


def engine(path, cls=ProactiveEgress, sender=None):
    return cls(path=path, senders={'s': sender} if sender else {},
               incident_sink=lambda _: None, health_sink=lambda _: None,
               sleep=lambda _: None)


def legacy(path: Path, *, unknown_marker=False):
    with sqlite3.connect(path) as db:
        db.executescript('''
            CREATE TABLE egress_outputs (
                output_id TEXT PRIMARY KEY, issuer_id TEXT NOT NULL,
                logical_key TEXT NOT NULL, request_digest TEXT NOT NULL,
                sender_id TEXT NOT NULL, recipient TEXT NOT NULL,
                incident_delivery INTEGER NOT NULL, created_at_ms INTEGER NOT NULL,
                UNIQUE (issuer_id, logical_key));
            CREATE TABLE egress_parts (
                output_id TEXT NOT NULL, part_index INTEGER NOT NULL, state TEXT NOT NULL,
                text TEXT, caption TEXT, mime_type TEXT, media_ref TEXT,
                attempts INTEGER NOT NULL DEFAULT 0, provider_msg_id TEXT, error TEXT,
                PRIMARY KEY (output_id, part_index),
                FOREIGN KEY (output_id) REFERENCES egress_outputs(output_id));
            CREATE TABLE channel_inbox (
                channel_id TEXT NOT NULL, source_event_id TEXT NOT NULL,
                state TEXT NOT NULL, envelope TEXT NOT NULL, dispatch_key TEXT,
                reject_reason TEXT, error TEXT, received_at_ms INTEGER NOT NULL,
                dispatch_continuity_version INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (channel_id, source_event_id));
            CREATE TABLE channel_poll_cursor (
                channel_id TEXT PRIMARY KEY, cursor TEXT NOT NULL,
                committed_at_ms INTEGER NOT NULL);
            INSERT INTO channel_inbox VALUES
                ('channel','event-17','accepted','{"text":"pending turn"}',
                 'accepted-turn-17',NULL,NULL,17,1);
            INSERT INTO channel_poll_cursor VALUES ('channel','cursor-18',18);
        ''')
        db.execute('INSERT INTO egress_outputs VALUES (?,?,?,?,?,?,?,?)',
                   ('out', 'issuer', 'logical', 'retained-digest', 's', 'recipient',
                    0, time.time_ns() // 1_000_000))
        db.executemany('INSERT INTO egress_parts VALUES (?,?,?,?,?,?,?,?,?,?)', [
            ('out', 0, 'sent', 'first', None, None, None, 1, 'receipt-0', None),
            ('out', 1, 'unknown', None, 'uncertain caption', 'image/png', 'retained-image',
             2, 'uncertain-receipt', 'retained uncertainty'),
            ('out', 2, 'pending', 'entered, response lost', None, None, None,
             1, None, 'provider result lost'),
            ('out', 3, 'pending', 'never entered', None, None, None, 0, None, None),
        ])
        if unknown_marker:
            db.execute('ALTER TABLE egress_parts ADD COLUMN unknown_attempts INTEGER NOT NULL DEFAULT 0')
            db.execute('UPDATE egress_parts SET unknown_attempts=2 WHERE part_index=1')
            db.execute('UPDATE egress_parts SET unknown_attempts=1 WHERE part_index=2')


def snapshot(path):
    with sqlite3.connect(path) as db:
        schema = db.execute('SELECT type,name,tbl_name,sql FROM sqlite_master ORDER BY type,name').fetchall()
        tables = [r[1] for r in schema if r[0] == 'table']
        return {'schema': schema,
                'rows': {t: db.execute(f'SELECT * FROM "{t}" ORDER BY rowid').fetchall() for t in tables}}


def related_rows(path):
    with sqlite3.connect(path) as db:
        return {table: db.execute(f'SELECT * FROM {table}').fetchall()
                for table in ['channel_inbox', 'channel_poll_cursor']}


@pytest.mark.parametrize('deny_alter', [4, 6])
def test_late_alter_denial_rolls_back_complete_schema_and_data(tmp_path, deny_alter):
    path = tmp_path / 'channels.sqlite3'
    legacy(path)
    before = snapshot(path)
    alters = []

    class Refusing(ProactiveEgress):
        def _connect(self):
            db = super()._connect()
            def authorize(action, first, second, database, trigger):
                if action == sqlite3.SQLITE_ALTER_TABLE:
                    alters.append((first, second))
                    if len(alters) == deny_alter:
                        return sqlite3.SQLITE_DENY
                return sqlite3.SQLITE_OK
            db.set_authorizer(authorize)
            return db

    with pytest.raises(sqlite3.DatabaseError, match='not authorized'):
        engine(path, Refusing)
    assert len(alters) == deny_alter
    assert snapshot(path) == before
    # The rollback also releases the writer transaction. The existing owner can retry.
    engine(path)
    assert related_rows(path) == {k: before['rows'][k] for k in related_rows(path)}


def test_two_legacy_constructors_serialize_schema_observation(tmp_path):
    path = tmp_path / 'channels.sqlite3'
    legacy(path)
    before_related = related_rows(path)
    first_at_alter = threading.Event()
    release_first = threading.Event()
    second_entered_sql = threading.Event()
    second_inspected = threading.Event()
    errors = []
    completed = []

    class PausingConnection(sqlite3.Connection):
        def execute(self, sql, parameters=(), /):
            if threading.current_thread().name == 'second-migration':
                if sql == 'BEGIN IMMEDIATE' or sql.startswith('PRAGMA table_info(egress_outputs)'):
                    second_entered_sql.set()
                if sql.startswith('PRAGMA table_info(egress_outputs)'):
                    second_inspected.set()
            if (threading.current_thread().name == 'first-migration'
                    and sql.startswith('ALTER TABLE egress_outputs ADD COLUMN root')):
                first_at_alter.set()
                if not release_first.wait(3):
                    raise TimeoutError('test migration barrier timed out')
            return super().execute(sql, parameters)

    class Observed(ProactiveEgress):
        def _connect(self):
            db = sqlite3.connect(self.path, timeout=2, factory=PausingConnection)
            db.execute('PRAGMA foreign_keys = ON')
            db.execute('PRAGMA synchronous = FULL')
            return db

    def construct():
        try:
            engine(path, Observed)
            completed.append(threading.current_thread().name)
        except BaseException as error:
            errors.append((type(error).__name__, str(error)))

    first = threading.Thread(target=construct, name='first-migration')
    second = threading.Thread(target=construct, name='second-migration')
    first.start()
    try:
        assert first_at_alter.wait(3)
        second.start()
        assert second_entered_sql.wait(3)
        # The second constructor is at its writer admission, before schema inspection.
        assert not second_inspected.wait(0.2)
        assert related_rows(path) == before_related
    finally:
        release_first.set()
        first.join(4)
        if second.ident is not None:
            second.join(4)
    assert not first.is_alive() and not second.is_alive()
    assert errors == []
    assert sorted(completed) == ['first-migration', 'second-migration']
    assert second_inspected.is_set()
    after = snapshot(path)
    engine(path)
    assert snapshot(path) == after
    assert related_rows(path) == before_related


@pytest.mark.parametrize('unknown_marker', [False, True])
def test_multipart_evidence_and_shared_cursor_survive_migration_and_reopen(tmp_path, unknown_marker):
    path = tmp_path / 'channels.sqlite3'
    legacy(path, unknown_marker=unknown_marker)
    before = snapshot(path)
    calls = []

    def sender(**kwargs):
        calls.append(kwargs)
        return {'outcome': 'sent', 'provider_message_id': 'receipt-3'}

    owner = engine(path, sender=sender)
    with sqlite3.connect(path) as db:
        old_columns = [r[1] for r in db.execute('PRAGMA table_info(egress_parts)')
                       if r[1] not in {'unknown_attempts', 'last_outcome', 'entities', 'caption_entities',
                                       'attempt_open', 'unknown_resends_used', 'unknown_history'}]
        old_rows = db.execute('SELECT '+','.join(old_columns)+' FROM egress_parts ORDER BY part_index').fetchall()
        baseline_rows = [row[:10] for row in before['rows']['egress_parts']]
        assert old_rows == baseline_rows
        assert db.execute('SELECT root,part_count FROM egress_outputs').fetchall() == [('hc-notify-v1:issuer/logical', 4)]
        assert db.execute('SELECT last_outcome,entities,caption_entities FROM egress_parts').fetchall() == [(None,None,None)] * 4
        assert db.execute('SELECT unknown_attempts FROM egress_parts ORDER BY part_index').fetchall() == (
            [(0,), (2,), (1,), (0,)] if unknown_marker else [(0,)] * 4)
    assert calls == []
    assert related_rows(path) == {k: before['rows'][k] for k in related_rows(path)}
    migrated = snapshot(path)
    owner = engine(path, sender=sender)
    assert snapshot(path) == migrated
    asyncio.run(owner.recover_pending())
    assert [c['part_index'] for c in calls] == [3]
    assert calls[0]['part'] == {'text': 'never entered'}
    with sqlite3.connect(path) as db:
        assert db.execute('SELECT part_index,state,attempts,provider_msg_id,error FROM egress_parts ORDER BY part_index').fetchall() == [
            (0, 'sent', 1, 'receipt-0', None),
            (1, 'unknown', 2, 'uncertain-receipt', 'retained uncertainty'),
            (2, 'unknown', 1, None, 'provider result lost'),
            (3, 'sent', 1, 'receipt-3', None),
        ]
    asyncio.run(engine(path, sender=sender).recover_pending())
    assert [c['part_index'] for c in calls] == [3]
    assert related_rows(path) == {k: before['rows'][k] for k in related_rows(path)}
