"""Real SQLite format adoption and reopen, without host or provider effects."""

from __future__ import annotations

import asyncio
from dataclasses import replace
from pathlib import Path
import sqlite3
import threading

import pytest

from meerkat_toolkit.frontend import egress
from meerkat_toolkit.frontend.egress import (
    EGRESS_MIGRATION_OPERATION,
    EGRESS_STATE_FORMAT,
    EgressMigrationBinding,
    ProactiveEgress,
)
from test_egress_migration import legacy, snapshot, related_rows, engine


def binding(mark="1"):
    return EgressMigrationBinding(mark * 64, "2" * 64, "3" * 64)


def managed(path):
    return ProactiveEgress(
        path=path,
        schema_mode="managed",
        senders={},
        incident_sink=lambda _: None,
        health_sink=lambda _: None,
        sleep=lambda _: None,
    )


def adopt(path):
    return ProactiveEgress.migrate_format(
        path=path, expected=ProactiveEgress.inspect_format(path=path), binding=binding()
    )


def test_inspect_absence_and_all_known_layouts_is_read_only(tmp_path):
    absent = tmp_path / "does not exist" / "channels.sqlite3"
    assert ProactiveEgress.inspect_format(path=absent).state == "absent_database"
    assert not absent.parent.exists()
    path = tmp_path / "channels.sqlite3"
    with sqlite3.connect(path) as db:
        db.execute("CREATE TABLE unrelated (value TEXT)")
    before = snapshot(path)
    assert ProactiveEgress.inspect_format(path=path).state == "absent_namespace"
    assert snapshot(path) == before
    for marker in [False, True]:
        path = tmp_path / f"legacy-{marker}.sqlite3"
        legacy(path, unknown_marker=marker)
        before = snapshot(path)
        assert ProactiveEgress.inspect_format(path=path).state == "unmarked_legacy"
        assert snapshot(path) == before
        engine(path)
        before = snapshot(path)
        assert ProactiveEgress.inspect_format(path=path).state == "unmarked_current"
        assert snapshot(path) == before
    fresh = tmp_path / "fresh.sqlite3"
    engine(fresh)
    before = snapshot(fresh)
    assert ProactiveEgress.inspect_format(path=fresh).state == "unmarked_current"
    assert snapshot(fresh) == before


@pytest.mark.parametrize(
    "kind",
    ["absent", "empty_namespace", "legacy", "unknown_marker", "fresh", "upgraded"],
)
def test_adopt_known_shapes_and_reopen_preserve_evidence(tmp_path, kind):
    path = tmp_path / "channels.sqlite3"
    if kind == "empty_namespace":
        with sqlite3.connect(path) as db:
            db.execute("CREATE TABLE unrelated (value TEXT)")
            db.execute("INSERT INTO unrelated VALUES ('retained')")
    elif kind in ["legacy", "unknown_marker", "upgraded"]:
        legacy(path, unknown_marker=kind == "unknown_marker")
        if kind == "upgraded":
            engine(path)
    elif kind == "fresh":
        engine(path)
    prior = ProactiveEgress.inspect_format(path=path)
    before = snapshot(path) if path.exists() else None
    result = ProactiveEgress.migrate_format(
        path=path, expected=prior, binding=binding()
    )
    assert result.state == "managed" and result.format == EGRESS_STATE_FORMAT
    assert result.transition.operation == EGRESS_MIGRATION_OPERATION
    assert result.transition.source_state == prior.state
    assert result.transition.source_observation_digest == prior.observation_digest
    assert result.transition.binding == binding()
    after = snapshot(path)
    assert ProactiveEgress.inspect_format(path=path) == result
    managed(path)
    engine(path)  # A new legacy-mode caller recognizes managed state without DDL.
    assert snapshot(path) == after
    if kind in ["legacy", "unknown_marker", "upgraded"]:
        assert related_rows(path) == {
            key: before["rows"][key] for key in related_rows(path)
        }
        with sqlite3.connect(path) as db:
            assert db.execute(
                "SELECT part_index,state,attempts,provider_msg_id,error FROM egress_parts ORDER BY part_index"
            ).fetchall() == [
                (0, "sent", 1, "receipt-0", None),
                (1, "unknown", 2, "uncertain-receipt", "retained uncertainty"),
                (2, "pending", 1, None, "provider result lost"),
                (3, "pending", 0, None, None),
            ]
    if kind == "empty_namespace":
        assert after["rows"]["unrelated"] == [("retained",)]
    with pytest.raises(ValueError, match="invalid egress migration request"):
        ProactiveEgress.migrate_format(path=path, expected=result, binding=binding("4"))
    assert snapshot(path) == after


def test_managed_open_refuses_absence_unmarked_and_removed_database(tmp_path):
    path = tmp_path / "absent" / "channels.sqlite3"
    with pytest.raises(ValueError, match="managed egress format required"):
        managed(path)
    assert not path.parent.exists()
    path = tmp_path / "channels.sqlite3"
    engine(path)
    with pytest.raises(ValueError, match="managed egress format required"):
        managed(path)
    adopt(path)
    owner = managed(path)
    path.unlink()
    with pytest.raises(sqlite3.OperationalError):
        owner.status(output_id="missing")
    assert not path.exists()


@pytest.mark.parametrize(
    "damage",
    [
        "future",
        "owner",
        "schema_digest",
        "binding",
        "missing_column",
        "missing_index",
        "trigger",
        "extra_column",
        "missing_marker",
    ],
)
def test_inspect_and_constructors_refuse_incompatible_managed_state(tmp_path, damage):
    path = tmp_path / "channels.sqlite3"
    adopt(path)
    with sqlite3.connect(path) as db:
        if damage == "future":
            db.execute("UPDATE egress_format SET format='future'")
        elif damage == "owner":
            db.execute("UPDATE egress_format SET owner='other'")
        elif damage == "schema_digest":
            db.execute("UPDATE egress_format SET schema_digest=?", ("0" * 64,))
        elif damage == "binding":
            db.execute("UPDATE egress_format SET request_binding='{}'")
        elif damage == "missing_column":
            db.execute("ALTER TABLE egress_parts DROP COLUMN last_outcome")
        elif damage == "missing_index":
            db.execute("DROP INDEX idx_egress_parts_state")
        elif damage == "trigger":
            db.execute(
                "CREATE TRIGGER alter_egress AFTER INSERT ON egress_parts BEGIN SELECT 1; END"
            )
        elif damage == "extra_column":
            db.execute("ALTER TABLE egress_parts ADD COLUMN future TEXT")
        else:
            db.execute("DROP TABLE egress_format")
    before = snapshot(path)
    if damage == "missing_marker":
        assert ProactiveEgress.inspect_format(path=path).state == "unmarked_current"
    else:
        with pytest.raises(ValueError):
            ProactiveEgress.inspect_format(path=path)
        with pytest.raises(ValueError):
            engine(path)
    with pytest.raises(ValueError):
        managed(path)
    assert snapshot(path) == before


@pytest.mark.parametrize(
    "alter", ["CHECK (attempts >= 0)", "COLLATE NOCASE", "UNIQUE ON CONFLICT REPLACE"]
)
def test_unknown_ddl_constraints_cannot_be_hidden_by_matching_columns(tmp_path, alter):
    path = tmp_path / "channels.sqlite3"
    legacy(path)
    with sqlite3.connect(path) as db:
        sql = db.execute(
            "SELECT sql FROM sqlite_master WHERE name='egress_parts'"
        ).fetchone()[0]
        sql = sql.replace(
            "attempts INTEGER NOT NULL DEFAULT 0",
            "attempts INTEGER NOT NULL DEFAULT 0 " + alter,
        )
        db.execute("ALTER TABLE egress_parts RENAME TO old_parts")
        db.execute(sql)
        db.execute("DROP TABLE old_parts")
    before = snapshot(path)
    with pytest.raises(ValueError):
        ProactiveEgress.inspect_format(path=path)
    assert snapshot(path) == before


def test_stale_prepare_and_concurrent_absent_admission(tmp_path):
    path = tmp_path / "channels.sqlite3"
    expected = ProactiveEgress.inspect_format(path=path)
    barrier = threading.Barrier(2)
    results = []
    errors = []

    def migrate():
        try:
            barrier.wait(timeout=3)
            results.append(
                ProactiveEgress.migrate_format(
                    path=path, expected=expected, binding=binding()
                )
            )
        except (FileExistsError, ValueError) as error:
            errors.append(type(error).__name__)

    threads = [threading.Thread(target=migrate) for _ in range(2)]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join(7)
    assert not any(thread.is_alive() for thread in threads)
    assert len(results) == 1 and len(errors) == 1
    assert ProactiveEgress.inspect_format(path=path) == results[0]
    legacy_path = tmp_path / "legacy.sqlite3"
    legacy(legacy_path)
    old = ProactiveEgress.inspect_format(path=legacy_path)
    engine(legacy_path)
    before = snapshot(legacy_path)
    with pytest.raises(ValueError, match="stale egress format observation"):
        ProactiveEgress.migrate_format(
            path=legacy_path, expected=old, binding=binding()
        )
    assert snapshot(legacy_path) == before


@pytest.mark.parametrize("deny", ["alter", "format_insert"])
def test_failed_migration_rolls_back_schema_rows_and_format(
    tmp_path, monkeypatch, deny
):
    path = tmp_path / "channels.sqlite3"
    legacy(path)
    expected = ProactiveEgress.inspect_format(path=path)
    before = snapshot(path)
    original = egress._format_connection

    def connect(path, mode):
        connection = original(path, mode)

        def authorizer(action, first, second, database, trigger):
            if (
                deny == "alter"
                and action == sqlite3.SQLITE_ALTER_TABLE
                and second == "egress_parts"
            ) or (
                deny == "format_insert"
                and action == sqlite3.SQLITE_INSERT
                and first == "egress_format"
            ):
                return sqlite3.SQLITE_DENY
            return sqlite3.SQLITE_OK

        connection.set_authorizer(authorizer)
        return connection

    with monkeypatch.context() as patch:
        patch.setattr(egress, "_format_connection", connect)
        with pytest.raises(sqlite3.DatabaseError):
            ProactiveEgress.migrate_format(
                path=path, expected=expected, binding=binding()
            )
    assert snapshot(path) == before
    assert ProactiveEgress.inspect_format(path=path) == expected
    assert (
        ProactiveEgress.migrate_format(
            path=path, expected=expected, binding=binding()
        ).state
        == "managed"
    )


def test_lost_post_commit_return_reopens_without_mutation_or_sender(
    tmp_path, monkeypatch
):
    path = tmp_path / "channels.sqlite3"
    expected = ProactiveEgress.inspect_format(path=path)
    with monkeypatch.context() as patch:
        patch.setattr(
            egress,
            "_fsync_file",
            lambda _: (_ for _ in ()).throw(OSError("injected after commit")),
        )
        with pytest.raises(OSError, match="injected after commit"):
            ProactiveEgress.migrate_format(
                path=path, expected=expected, binding=binding()
            )
    before = snapshot(path)
    original = egress._format_connection

    def readonly(path, mode):
        assert mode == "ro"
        connection = original(path, mode)

        def authorizer(action, *_):
            return (
                sqlite3.SQLITE_DENY
                if action
                in {
                    sqlite3.SQLITE_INSERT,
                    sqlite3.SQLITE_UPDATE,
                    sqlite3.SQLITE_DELETE,
                    sqlite3.SQLITE_CREATE_TABLE,
                    sqlite3.SQLITE_ALTER_TABLE,
                }
                else sqlite3.SQLITE_OK
            )

        connection.set_authorizer(authorizer)
        return connection

    monkeypatch.setattr(egress, "_format_connection", readonly)
    result = ProactiveEgress.inspect_format(path=path)
    assert result.state == "managed" and result.transition.binding == binding()
    assert result.transition.source_observation_digest == expected.observation_digest
    assert snapshot(path) == before


def test_managed_progress_keeps_historical_binding_and_unknown_attempts(tmp_path):
    path = tmp_path / "channels.sqlite3"
    legacy(path)
    result = adopt(path)
    calls = []
    owner = ProactiveEgress(
        path=path,
        schema_mode="managed",
        senders={
            "s": lambda **kw: calls.append(kw)
            or {"outcome": "sent", "provider_message_id": "new-receipt"}
        },
        incident_sink=lambda _: None,
        health_sink=lambda _: None,
        sleep=lambda _: None,
    )
    asyncio.run(owner.recover_pending())
    assert [call["part_index"] for call in calls] == [3]
    assert ProactiveEgress.inspect_format(path=path) == result
    with sqlite3.connect(path) as db:
        assert db.execute(
            "SELECT state,attempts,provider_msg_id FROM egress_parts ORDER BY part_index"
        ).fetchall() == [
            ("sent", 1, "receipt-0"),
            ("unknown", 2, "uncertain-receipt"),
            ("unknown", 1, None),
            ("sent", 1, "new-receipt"),
        ]
    before = snapshot(path)
    with pytest.raises(ValueError):
        ProactiveEgress.migrate_format(
            path=path,
            expected=replace(result, state="unmarked_current"),
            binding=binding("4"),
        )
    assert snapshot(path) == before


@pytest.mark.parametrize(
    "damage", ["case_marker", "misplaced_index", "empty_marker", "partial_namespace"]
)
def test_catalog_damage_is_not_a_legacy_initialization_opportunity(tmp_path, damage):
    path = tmp_path / "channels.sqlite3"
    adopt(path)
    with sqlite3.connect(path) as db:
        if damage == "case_marker":
            db.execute("ALTER TABLE egress_format RENAME TO temporary_marker")
            db.execute("ALTER TABLE temporary_marker RENAME TO EGRESS_FORMAT")
        elif damage == "misplaced_index":
            db.execute("DROP INDEX idx_egress_parts_state")
            db.execute("CREATE TABLE unrelated (state TEXT)")
            db.execute("CREATE INDEX idx_egress_parts_state ON unrelated(state)")
        elif damage == "empty_marker":
            db.execute("DELETE FROM egress_format")
        else:
            db.execute("DROP TABLE egress_parts")
    before = snapshot(path)
    for action in [
        lambda: ProactiveEgress.inspect_format(path=path),
        lambda: managed(path),
        lambda: engine(path),
    ]:
        with pytest.raises(ValueError):
            action()
        assert snapshot(path) == before


def test_two_legacy_preparations_commit_at_most_once(tmp_path):
    path = tmp_path / "channels.sqlite3"
    legacy(path)
    expected = ProactiveEgress.inspect_format(path=path)
    barrier = threading.Barrier(2)
    results = []
    errors = []

    def migrate():
        try:
            barrier.wait(timeout=3)
            results.append(
                ProactiveEgress.migrate_format(
                    path=path, expected=expected, binding=binding()
                )
            )
        except ValueError as error:
            errors.append(str(error))

    threads = [threading.Thread(target=migrate) for _ in range(2)]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join(7)
    assert not any(thread.is_alive() for thread in threads)
    assert len(results) == 1 and errors == ["stale egress format observation"]
    assert ProactiveEgress.inspect_format(path=path) == results[0]


def test_symlink_future_old_format_and_rebinding_refuse_without_effects(tmp_path):
    path = tmp_path / "channels.sqlite3"
    result = adopt(path)
    link = tmp_path / "alias.sqlite3"
    link.symlink_to(path)
    with pytest.raises(ValueError):
        ProactiveEgress.inspect_format(path=link)
    before = snapshot(path)
    with pytest.raises(ValueError):
        ProactiveEgress.migrate_format(path=path, expected=result, binding=binding("4"))
    assert snapshot(path) == before
    with sqlite3.connect(path) as db:
        db.execute("UPDATE egress_format SET format='egress-sqlite-0'")
    before = snapshot(path)
    for action in [
        lambda: ProactiveEgress.inspect_format(path=path),
        lambda: managed(path),
        lambda: engine(path),
    ]:
        with pytest.raises(ValueError):
            action()
    assert snapshot(path) == before


@pytest.mark.parametrize("damage", ["long_binding", "long_schema", "many_objects"])
def test_oversized_format_metadata_refuses_without_truncated_acceptance(
    tmp_path, damage
):
    path = tmp_path / "channels.sqlite3"
    adopt(path)
    with sqlite3.connect(path) as db:
        if damage == "long_binding":
            db.execute("UPDATE egress_format SET request_binding=?", (" " * 50000,))
        elif damage == "long_schema":
            db.execute(
                "CREATE TRIGGER oversized AFTER INSERT ON egress_parts BEGIN SELECT '"
                + ("x" * 20000)
                + "'; END"
            )
        else:
            for i in range(65):
                db.execute(f"CREATE TABLE egress_unknown_{i} (value TEXT)")
    before = snapshot(path)
    with pytest.raises(ValueError):
        ProactiveEgress.inspect_format(path=path)
    assert snapshot(path) == before
