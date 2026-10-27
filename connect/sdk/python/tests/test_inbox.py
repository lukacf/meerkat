"""Actual SQLite owner tests, independent of PairedHost adapters and SDK models."""

from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
import sqlite3
import threading

import pytest

from meerkat_toolkit.frontend.inbox import DispatchGroupConflict, SQLiteChannelInbox


def seed(inbox, number, *, channel="channel"):
    assert inbox._insert(
        channel_id=channel,
        source_event_id=f"i:{number}",
        state="accepted",
        envelope={"text": f"part {number}"},
        dispatch_key=inbox._dispatch_key(channel, f"i:{number}"),
        reject_reason=None,
    )
    return inbox.get(channel_id=channel, source_event_id=f"i:{number}")


def test_overlapping_publications_have_one_winner_and_no_partial_members(tmp_path: Path):
    path = tmp_path / "inbox.sqlite"
    owner = SQLiteChannelInbox(path=path)
    rows = [seed(owner, n) for n in range(3)]
    gate = threading.Barrier(2)

    def publish(indices):
        peer = SQLiteChannelInbox(path=path)
        gate.wait()
        try:
            group = peer.freeze_dispatch_group(
                channel_id="channel", rows=[rows[n] for n in indices], identity="person"
            )
            return [row.source_event_id for row in group.rows]
        except DispatchGroupConflict:
            return None

    with ThreadPoolExecutor(max_workers=2) as pool:
        results = list(pool.map(publish, [(0, 1), (1, 2)]))
    assert sum(value is not None for value in results) == 1
    winner = next(value for value in results if value is not None)
    reopened = SQLiteChannelInbox(path=path)
    for row in rows:
        actual = reopened.dispatch_group(channel_id="channel", source_event_id=row.source_event_id)
        if row.source_event_id in winner:
            assert [part.source_event_id for part in actual.rows] == winner
        else:
            assert actual is None
    with sqlite3.connect(path) as connection:
        assert connection.execute("SELECT count(*) FROM channel_dispatch_groups").fetchone()[0] == 1
        assert (
            connection.execute("SELECT count(*) FROM channel_dispatch_members").fetchone()[0] == 2
        )


def test_frozen_group_rejects_changed_members_content_and_identity(tmp_path: Path):
    path = tmp_path / "inbox.sqlite"
    inbox = SQLiteChannelInbox(path=path)
    rows = [seed(inbox, n) for n in range(3)]
    inbox.freeze_dispatch_group(channel_id="channel", rows=rows[:2], identity="person")
    for group, identity in [
        (rows, "person"),
        (list(reversed(rows[:2])), "person"),
        (rows[:2], "other"),
    ]:
        with pytest.raises(DispatchGroupConflict):
            inbox.freeze_dispatch_group(channel_id="channel", rows=group, identity=identity)
    payload = {"content": "part 0\npart 1", "key": rows[0].dispatch_key}
    inbox.freeze_dispatch_payload(
        channel_id="channel", source_event_id="i:0", identity="person", payload=payload
    )
    reopened = SQLiteChannelInbox(path=path)
    assert reopened.dispatch_group(channel_id="channel", source_event_id="i:1").payload == payload
    with pytest.raises(ValueError, match="differs"):
        reopened.freeze_dispatch_payload(
            channel_id="channel",
            source_event_id="i:0",
            identity="person",
            payload={"content": "changed"},
        )
    with sqlite3.connect(path) as connection:
        connection.execute("UPDATE channel_inbox SET envelope = '{}' WHERE source_event_id = 'i:1'")
    with pytest.raises(ValueError, match="differs"):
        reopened.dispatch_group(channel_id="channel", source_event_id="i:0")


def test_group_retention_waits_for_all_members_and_refuses_missing_tail(tmp_path: Path):
    path = tmp_path / "inbox.sqlite"
    inbox = SQLiteChannelInbox(path=path)
    rows = [seed(inbox, n) for n in range(2)]
    inbox.freeze_dispatch_group(channel_id="channel", rows=rows, identity="person")
    inbox.freeze_dispatch_payload(
        channel_id="channel", source_event_id="i:0", identity="person", payload={"content": "fixed"}
    )
    with sqlite3.connect(path) as connection:
        connection.execute("UPDATE channel_inbox SET received_at_ms = 0")
        connection.execute(
            "UPDATE channel_inbox SET state = 'dispatched' WHERE source_event_id = 'i:1'"
        )
        SQLiteChannelInbox.prune_dispatch_groups(connection, "channel", 100)
        assert (
            connection.execute("SELECT count(*) FROM channel_dispatch_members").fetchone()[0] == 2
        )
    assert len(inbox.dispatch_group(channel_id="channel", source_event_id="i:0").rows) == 2
    inbox.mark_dispatch_group_dispatched(channel_id="channel", source_event_id="i:0")
    with sqlite3.connect(path) as connection:
        SQLiteChannelInbox.prune_dispatch_groups(connection, "channel", 100)
    assert inbox.dispatch_group(channel_id="channel", source_event_id="i:0") is None

    newer = seed(inbox, 3)
    last = seed(inbox, 4)
    inbox.freeze_dispatch_group(channel_id="channel", rows=[newer, last], identity="person")
    with sqlite3.connect(path) as connection:
        # Simulate corrupt ownership metadata, not an allowed API operation.
        connection.execute("DELETE FROM channel_dispatch_members WHERE source_event_id = 'i:4'")
    with pytest.raises(ValueError, match="missing"):
        inbox.dispatch_group(channel_id="channel", source_event_id="i:3")


def test_failed_publication_rolls_back_group_and_members(tmp_path: Path):
    path = tmp_path / "inbox.sqlite"
    inbox = SQLiteChannelInbox(path=path)
    rows = [seed(inbox, n) for n in range(2)]
    with sqlite3.connect(path) as connection:
        connection.execute(
            "CREATE TRIGGER fail_member BEFORE INSERT ON channel_dispatch_members "
            "WHEN NEW.ordinal = 1 BEGIN SELECT RAISE(ABORT, 'refuse tail'); END"
        )
    with pytest.raises(sqlite3.IntegrityError, match="refuse tail"):
        inbox.freeze_dispatch_group(channel_id="channel", rows=rows, identity="person")
    with sqlite3.connect(path) as connection:
        assert connection.execute("SELECT count(*) FROM channel_dispatch_groups").fetchone()[0] == 0
        assert (
            connection.execute("SELECT count(*) FROM channel_dispatch_members").fetchone()[0] == 0
        )


def test_old_schema_cutover_never_promotes_ambiguous_admissions(tmp_path: Path):
    import json

    path = tmp_path / "inbox.sqlite"
    with sqlite3.connect(path) as connection:
        connection.execute(
            "CREATE TABLE channel_inbox (channel_id TEXT NOT NULL, source_event_id TEXT NOT NULL, "
            "state TEXT NOT NULL, envelope TEXT NOT NULL, dispatch_key TEXT, reject_reason TEXT, "
            "created_at_ms INTEGER NOT NULL, PRIMARY KEY(channel_id,source_event_id))"
        )
        for number, state in enumerate(["accepted", "dispatching", "failed", "dispatched"]):
            connection.execute(
                "INSERT INTO channel_inbox VALUES ('channel', ?, ?, ?, ?, NULL, 0)",
                (
                    f"i:{number}",
                    state,
                    json.dumps({"text": "legacy"}, separators=(",", ":")),
                    SQLiteChannelInbox._dispatch_key("channel", f"i:{number}"),
                ),
            )
    owner = SQLiteChannelInbox(path=path)
    for number in range(4):
        row = owner.get(channel_id="channel", source_event_id=f"i:{number}")
        assert row.dispatch_continuity_version == 0
        if number < 2:
            assert row.state == "accepted"
            with pytest.raises(DispatchGroupConflict):
                owner.freeze_dispatch_group(channel_id="channel", rows=[row], identity="person")
    # Duplicate transport delivery never erases the old ambiguity classification.
    assert not owner._insert(
        channel_id="channel",
        source_event_id="i:0",
        state="accepted",
        envelope={"text": "legacy"},
        dispatch_key=owner._dispatch_key("channel", "i:0"),
        reject_reason=None,
    )
    assert owner.get(channel_id="channel", source_event_id="i:0").dispatch_continuity_version == 0
    fresh = seed(owner, 4)
    assert fresh.dispatch_continuity_version == 1
    reopened = SQLiteChannelInbox(path=path)
    assert (
        reopened.get(channel_id="channel", source_event_id="i:4").dispatch_continuity_version == 1
    )
    assert reopened.freeze_dispatch_group(channel_id="channel", rows=[fresh], identity="person")
    assert (
        reopened.get(channel_id="channel", source_event_id="i:2").error == "legacy failed inbox row"
    )


def source_provenance(**changes):
    from meerkat_toolkit.frontend.inbox import InboxSourceProvenance

    values = {
        "channel_id": "channel",
        "extension_id": "telegram-extension",
        "service_subject": "service:telegram",
        "provider_account": "account-one",
        "requester": "person",
        "sender_id": "provider-user",
        "partition": '{"chat":"private","thread":null}',
        "policy_digest": "policy-one",
    }
    return InboxSourceProvenance(**(values | changes))


def seed_provenance(inbox, number, provenance):
    assert inbox._insert(
        channel_id="channel",
        source_event_id=f"i:{number}",
        state="accepted",
        envelope={"text": f"part {number}"},
        dispatch_key=inbox._dispatch_key("channel", f"i:{number}"),
        reject_reason=None,
        source_provenance=provenance,
    )
    return inbox.get(channel_id="channel", source_event_id=f"i:{number}")


def test_original_source_provenance_survives_reopen_group_and_duplicate(tmp_path: Path):
    path = tmp_path / "inbox.sqlite"
    inbox = SQLiteChannelInbox(path=path)
    original = source_provenance()
    rows = [seed_provenance(inbox, number, original) for number in (1, 2)]
    group = inbox.freeze_dispatch_group(channel_id="channel", rows=rows, identity="person")
    inbox.freeze_dispatch_payload(
        channel_id="channel",
        source_event_id="i:1",
        identity="person",
        payload={"input": "fixed"},
    )
    assert not inbox._insert(
        channel_id="channel",
        source_event_id="i:1",
        state="accepted",
        envelope={"text": "changed"},
        dispatch_key="new-key",
        reject_reason=None,
        source_provenance=source_provenance(requester="someone-else"),
    )
    reopened = SQLiteChannelInbox(path=path)
    restored = reopened.dispatch_group(channel_id="channel", source_event_id="i:2")
    assert restored.source_provenance == group.source_provenance == original
    assert all(row.source_provenance == original for row in restored.rows)
    assert restored.payload == {"input": "fixed"}
    assert restored.rows[0].envelope == {"text": "part 1"}


@pytest.mark.parametrize(
    "changed",
    [
        {"requester": "someone-else"},
        {"service_subject": "another-service"},
        {"provider_account": "another-account"},
        {"extension_id": "another-extension"},
        {"sender_id": "another-sender"},
        {"partition": '{"chat":"public","thread":null}'},
        {"policy_digest": "policy-two"},
        None,
    ],
)
def test_group_refuses_mixed_original_authority_without_partial_publication(tmp_path, changed):
    inbox = SQLiteChannelInbox(path=tmp_path / "inbox.sqlite")
    first = seed_provenance(inbox, 1, source_provenance())
    other = None if changed is None else source_provenance(**changed)
    second = seed_provenance(inbox, 2, other)
    with pytest.raises(DispatchGroupConflict, match="incompatible source"):
        inbox.freeze_dispatch_group(channel_id="channel", rows=[first, second], identity="person")
    assert inbox.dispatch_group(channel_id="channel", source_event_id="i:1") is None
    assert inbox.dispatch_group(channel_id="channel", source_event_id="i:2") is None


def test_frontend_can_route_to_an_agent_distinct_from_the_original_requester(tmp_path):
    inbox = SQLiteChannelInbox(path=tmp_path / "inbox.sqlite")
    row = seed_provenance(inbox, 1, source_provenance())
    group = inbox.freeze_dispatch_group(channel_id="channel", rows=[row], identity="router-agent")
    assert group.identity == "router-agent"
    assert group.source_provenance.requester == "person"
    with pytest.raises(DispatchGroupConflict):
        inbox.freeze_dispatch_group(channel_id="channel", rows=[row], identity="other-agent")


def test_frozen_group_detects_replaced_source_attestation(tmp_path):
    inbox = SQLiteChannelInbox(path=tmp_path / "inbox.sqlite")
    row = seed_provenance(inbox, 1, source_provenance())
    inbox.freeze_dispatch_group(channel_id="channel", rows=[row], identity="person")
    with sqlite3.connect(inbox.path) as connection:
        connection.execute(
            "UPDATE channel_inbox SET source_provenance = ? WHERE source_event_id = 'i:1'",
            (source_provenance(policy_digest="policy-two").encode(),),
        )
    with pytest.raises(ValueError, match="source provenance differs"):
        inbox.dispatch_group(channel_id="channel", source_event_id="i:1")


def test_changed_source_provenance_after_observation_cannot_freeze(tmp_path):
    inbox = SQLiteChannelInbox(path=tmp_path / "inbox.sqlite")
    row = seed_provenance(inbox, 1, source_provenance())
    with sqlite3.connect(inbox.path) as connection:
        connection.execute(
            "UPDATE channel_inbox SET source_provenance = ? WHERE source_event_id = 'i:1'",
            (source_provenance(policy_digest="policy-two").encode(),),
        )
    with pytest.raises(DispatchGroupConflict, match="expected accepted"):
        inbox.freeze_dispatch_group(channel_id="channel", rows=[row], identity="person")
    assert inbox.dispatch_group(channel_id="channel", source_event_id="i:1") is None


@pytest.mark.parametrize("version", [True, 0, 2, "1"])
def test_source_provenance_rejects_unknown_or_bool_versions(version):
    import json

    from meerkat_toolkit.frontend.inbox import InboxSourceProvenance

    value = json.loads(source_provenance().encode())
    value["version"] = version
    with pytest.raises(ValueError, match="version"):
        InboxSourceProvenance.decode(json.dumps(value))


def test_provenance_schema_adoption_does_not_upgrade_historical_rows_or_groups(
    tmp_path,
):
    path = tmp_path / "inbox.sqlite"
    inbox = SQLiteChannelInbox(path=path)
    row = seed(inbox, 1)
    inbox.freeze_dispatch_group(channel_id="channel", rows=[row], identity="person")
    # Reconstruct the exact pre-provenance schema while retaining prior
    # dispatch continuity. Adoption must not infer authentication from it.
    with sqlite3.connect(path) as connection:
        connection.execute("ALTER TABLE channel_inbox DROP COLUMN source_provenance")
        connection.execute("ALTER TABLE channel_dispatch_groups DROP COLUMN source_provenance")
    reopened = SQLiteChannelInbox(path=path)
    assert reopened.get(channel_id="channel", source_event_id="i:1").source_provenance is None
    group = reopened.dispatch_group(channel_id="channel", source_event_id="i:1")
    assert group.source_provenance is None
    assert group.rows[0].dispatch_continuity_version == 1


def test_source_provenance_cannot_move_to_another_channel(tmp_path):
    inbox = SQLiteChannelInbox(path=tmp_path / "inbox.sqlite")
    with pytest.raises(ValueError, match="another channel"):
        seed_provenance(inbox, 1, source_provenance(channel_id="other-channel"))
    assert inbox.get(channel_id="channel", source_event_id="i:1") is None


def test_concurrent_duplicate_admission_keeps_one_complete_original_binding(tmp_path):
    path = tmp_path / "inbox.sqlite"
    SQLiteChannelInbox(path=path)
    ready = threading.Barrier(2)

    def admit(number):
        owner = SQLiteChannelInbox(path=path)
        provenance = source_provenance(
            requester=f"person-{number}", policy_digest=f"policy-{number}"
        )
        ready.wait()
        won = owner._insert(
            channel_id="channel",
            source_event_id="i:1",
            state="accepted",
            envelope={"text": f"content-{number}"},
            dispatch_key=owner._dispatch_key("channel", "i:1"),
            reject_reason=None,
            source_provenance=provenance,
        )
        return number, won, provenance

    with ThreadPoolExecutor(max_workers=2) as pool:
        results = list(pool.map(admit, (1, 2)))
    assert sum(won for _, won, _ in results) == 1
    number, _, provenance = next(result for result in results if result[1])
    row = SQLiteChannelInbox(path=path).get(channel_id="channel", source_event_id="i:1")
    assert row.source_provenance == provenance
    assert row.envelope == {"text": f"content-{number}"}


@pytest.mark.parametrize(
    "encoded",
    [
        "[]",
        "null",
        "{}",
        '{"version":1}',
    ],
)
def test_source_provenance_rejects_non_record_shapes(encoded):
    from meerkat_toolkit.frontend.inbox import InboxSourceProvenance

    with pytest.raises(ValueError, match="shape"):
        InboxSourceProvenance.decode(encoded)


def test_legacy_group_migration_holds_sqlite_writer_custody_until_commit(tmp_path):
    path = tmp_path / "inbox.sqlite"
    owner = SQLiteChannelInbox(path=path)
    row = seed(owner, 1)
    owner.freeze_dispatch_group(channel_id="channel", rows=[row], identity="person")
    with sqlite3.connect(path) as connection:
        connection.execute("ALTER TABLE channel_inbox DROP COLUMN source_provenance")
        connection.execute("ALTER TABLE channel_dispatch_groups DROP COLUMN source_provenance")

    observed = threading.Event()
    release = threading.Event()
    transaction_at_observation = []

    class PausedConnection(sqlite3.Connection):
        def execute(self, statement, parameters=()):
            cursor = super().execute(statement, parameters)
            if statement == "PRAGMA table_info(channel_dispatch_groups)":
                rows = cursor.fetchall()
                cursor.close()
                transaction_at_observation.append(self.in_transaction)
                observed.set()
                if not release.wait(timeout=5):
                    raise TimeoutError("schema observation was not released")
                return iter(rows)
            return cursor

    class PausedInbox(SQLiteChannelInbox):
        def _connect(self):
            connection = sqlite3.connect(self.path, factory=PausedConnection)
            connection.execute("PRAGMA foreign_keys = ON")
            connection.execute("PRAGMA synchronous = FULL")
            return connection

    class CompetingInbox(SQLiteChannelInbox):
        def _connect(self):
            # Execute the competing real constructor immediately. SQLITE_BUSY
            # proves the writer exclusion; scheduler timing cannot satisfy it.
            connection = sqlite3.connect(self.path, timeout=0)
            connection.execute("PRAGMA foreign_keys = ON")
            connection.execute("PRAGMA synchronous = FULL")
            return connection

    with ThreadPoolExecutor(max_workers=1) as pool:
        first = pool.submit(PausedInbox, path=path)
        try:
            assert observed.wait(timeout=5)
            with pytest.raises(sqlite3.OperationalError, match="locked"):
                CompetingInbox(path=path)
        finally:
            release.set()
        first.result(timeout=5)
    assert transaction_at_observation == [True]

    # The competing constructor can complete after the existing writer has
    # committed. Historical admissions and their frozen groups stay unsigned.
    reopened = CompetingInbox(path=path)
    restored = reopened.get(channel_id="channel", source_event_id="i:1")
    assert restored.envelope == row.envelope
    assert restored.source_provenance is None
    group = reopened.dispatch_group(channel_id="channel", source_event_id="i:1")
    assert group.identity == "person"
    assert group.source_provenance is None
    assert group.rows == (restored,)


@pytest.mark.parametrize("root_bytes", ["opaque native root", "malformed root"])
@pytest.mark.parametrize("cancelled", [0, 1])
def test_retention_keeps_native_root_members_and_prunes_unpinned_groups(
    tmp_path: Path, root_bytes: str, cancelled: int
):
    path = tmp_path / "native-retention.sqlite3"
    inbox = SQLiteChannelInbox(path=path)
    for number in range(2):
        row = seed(inbox, number)
        inbox.freeze_dispatch_group(channel_id="channel", rows=[row], identity="person")
        inbox.freeze_dispatch_payload(
            channel_id="channel",
            source_event_id=row.source_event_id,
            identity="person",
            payload={"content": f"fixed {number}"},
        )
        inbox.mark_dispatch_group_dispatched(
            channel_id="channel", source_event_id=row.source_event_id
        )
    with sqlite3.connect(path) as connection:
        # Exact retention columns/trigger installed by the native owner. The
        # Python owner intentionally cannot interpret even valid root bytes.
        connection.execute(
            "ALTER TABLE channel_dispatch_groups ADD COLUMN native_request_root TEXT"
        )
        connection.execute(
            "ALTER TABLE channel_dispatch_groups ADD COLUMN native_request_cancelled INTEGER "
            "NOT NULL DEFAULT 0 CHECK(native_request_cancelled IN (0, 1))"
        )
        connection.execute(
            "CREATE TRIGGER channel_request_retention_guard BEFORE DELETE ON channel_dispatch_groups "
            "WHEN OLD.native_request_root IS NOT NULL BEGIN "
            "SELECT RAISE(ABORT, 'native request evidence is retained'); END;"
        )
        connection.execute(
            "UPDATE channel_dispatch_groups SET native_request_root = ?, "
            "native_request_cancelled = ? WHERE head_source_event_id = 'i:0'",
            (root_bytes, cancelled),
        )
        connection.execute("UPDATE channel_inbox SET received_at_ms = 0")
    with sqlite3.connect(path) as connection:
        connection.execute("BEGIN IMMEDIATE")
        SQLiteChannelInbox.prune_dispatch_groups(connection, "channel", 100)
        connection.execute(
            "DELETE FROM channel_inbox WHERE NOT EXISTS ("
            "SELECT 1 FROM channel_dispatch_members AS m "
            "WHERE m.channel_id = channel_inbox.channel_id "
            "AND m.source_event_id = channel_inbox.source_event_id)"
        )
    reopened = SQLiteChannelInbox(path=path)
    retained = reopened.dispatch_group(channel_id="channel", source_event_id="i:0")
    assert [row.source_event_id for row in retained.rows] == ["i:0"]
    assert retained.payload == {"content": "fixed 0"}
    assert reopened.get(channel_id="channel", source_event_id="i:0").state == "dispatched"
    assert reopened.dispatch_group(channel_id="channel", source_event_id="i:1") is None
    assert reopened.get(channel_id="channel", source_event_id="i:1") is None
    with sqlite3.connect(path) as connection:
        assert connection.execute(
            "SELECT native_request_root, native_request_cancelled FROM channel_dispatch_groups"
        ).fetchall() == [(root_bytes, cancelled)]
