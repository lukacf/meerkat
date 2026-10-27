"""Durable channel inbox storage, migrated from PairedHost's existing owner.

The database and its existing tables remain authoritative. Provider admission,
identity policy, secret handling and media I/O belong to the host adapters.
"""

from __future__ import annotations

from collections.abc import Awaitable, Callable, Iterator, Mapping, Sequence
from contextlib import contextmanager
import fcntl
from dataclasses import dataclass
import hashlib
import inspect
import json
import os
from pathlib import Path
import sqlite3
import stat
import time
from typing import Any
import unicodedata

_MEDIA_RETRY_DELAYS_MS = (60_000, 300_000)
_DISPATCH_RETRY_DELAYS_MS = (60_000, 300_000)
_TRANSCRIPTION_RETRY_DELAYS_MS = (60_000, 300_000)


def _canonical_json(value: object) -> bytes:
    return json.dumps(
        value,
        allow_nan=False,
        ensure_ascii=False,
        separators=(",", ":"),
        sort_keys=True,
    ).encode("utf-8")


async def _await(value: Any) -> Any:
    return await value if inspect.isawaitable(value) else value


def _fsync_file(path: Path) -> None:
    descriptor = os.open(path, os.O_RDONLY)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def _fsync_directory(path: Path) -> None:
    descriptor = os.open(path, os.O_RDONLY)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


@dataclass(frozen=True)
class InboxSourceProvenance:
    """Original host transport attestation, never current permission or a seal.

    The host supplies these values outside the provider event. They preserve
    the admitted source binding; only native root publication can add native
    issuer, destination, lifetime and current-use authority.
    """

    channel_id: str
    extension_id: str
    service_subject: str
    provider_account: str
    requester: str
    sender_id: str
    partition: str
    policy_digest: str

    def __post_init__(self) -> None:
        if any(not isinstance(value, str) or not value for value in self.__dict__.values()):
            raise ValueError("source provenance fields must be nonempty text")
        try:
            partition = json.loads(self.partition)
        except (TypeError, ValueError) as error:
            raise ValueError("source partition is invalid") from error
        if not isinstance(partition, dict) or _canonical_json(partition).decode() != self.partition:
            raise ValueError("source partition must be a canonical object")

    def encode(self) -> str:
        return _canonical_json({"version": 1, **self.__dict__}).decode("utf-8")

    @classmethod
    def decode(cls, encoded: str) -> InboxSourceProvenance:
        value = json.loads(encoded)
        fields = {
            "channel_id",
            "extension_id",
            "service_subject",
            "provider_account",
            "requester",
            "sender_id",
            "partition",
            "policy_digest",
        }
        if not isinstance(value, dict) or set(value) != fields | {"version"}:
            raise ValueError("source provenance shape is invalid")
        version = value.pop("version")
        if type(version) is not int or version != 1:
            raise ValueError("source provenance version is unsupported")
        result = cls(**value)
        if result.encode() != encoded:
            raise ValueError("source provenance encoding is not canonical")
        return result


@dataclass(frozen=True)
class InboxRow:
    channel_id: str
    source_event_id: str
    state: str
    envelope: Mapping[str, Any]
    dispatch_key: str | None
    reject_reason: str | None
    error: str | None = None
    media: tuple[Mapping[str, Any], ...] = ()
    dispatch_continuity_version: int = 0
    source_provenance: InboxSourceProvenance | None = None


@dataclass(frozen=True)
class FrozenDispatchGroup:
    """One immutable logical input, not requester permission or a runtime lease."""

    channel_id: str
    head_source_event_id: str
    identity: str
    rows: tuple[InboxRow, ...]
    payload: Mapping[str, Any] | None
    source_provenance: InboxSourceProvenance | None = None


class DispatchGroupConflict(ValueError):
    """Another publication already fixed overlapping source membership."""


class SQLiteChannelInbox:
    """The single SQLite owner of admitted source rows and dispatch evidence."""

    def __init__(self, *, path: Path) -> None:
        original = Path(path)
        original.parent.mkdir(parents=True, exist_ok=True)
        # All aliases of the same path join the same native/Python entry fence.
        # Hardlinks have no unique pathname and are refused by both adapters.
        self.path = original.resolve()
        if self.path.exists() and self.path.stat().st_nlink != 1:
            raise ValueError("channel inbox hardlink aliases are unsupported")
        self._request_use_fence = Path(str(self.path) + ".request-use.lock")
        created = not self.path.exists()
        with self._connect() as connection:
            connection.execute("BEGIN IMMEDIATE")
            connection.execute(
                """
                CREATE TABLE IF NOT EXISTS channel_inbox (
                    channel_id TEXT NOT NULL,
                    source_event_id TEXT NOT NULL,
                    state TEXT NOT NULL,
                    envelope TEXT NOT NULL,
                    dispatch_key TEXT,
                    reject_reason TEXT,
                    error TEXT,
                    received_at_ms INTEGER NOT NULL,
                    dispatch_continuity_version INTEGER NOT NULL DEFAULT 0,
                    PRIMARY KEY (channel_id, source_event_id)
                )
                """
            )
            columns = {
                str(row[1])
                for row in connection.execute("PRAGMA table_info(channel_inbox)").fetchall()
            }
            if "received_at_ms" not in columns and "created_at_ms" in columns:
                connection.execute(
                    "ALTER TABLE channel_inbox RENAME COLUMN created_at_ms TO received_at_ms"
                )
                columns.remove("created_at_ms")
                columns.add("received_at_ms")
            if "error" not in columns:
                connection.execute("ALTER TABLE channel_inbox ADD COLUMN error TEXT")
            if "dispatch_continuity_version" not in columns:
                # Old accepted rows may already have reached the runtime with
                # no saved target/payload or retry record. Only this owner's
                # new admissions explicitly cross the continuity boundary.
                connection.execute(
                    "ALTER TABLE channel_inbox ADD COLUMN "
                    "dispatch_continuity_version INTEGER NOT NULL DEFAULT 0"
                )
            if "source_provenance" not in columns:
                # Old admissions are deliberately not signed or reinterpreted
                # from the current sender map during schema adoption.
                connection.execute("ALTER TABLE channel_inbox ADD COLUMN source_provenance TEXT")
            connection.execute(
                "UPDATE channel_inbox SET error = 'legacy failed inbox row' "
                "WHERE state = 'failed' AND (error IS NULL OR error = '')"
            )
            connection.execute(
                "UPDATE channel_inbox SET error = NULL WHERE state <> 'failed' AND error IS NOT NULL"
            )
            # Preserve the old source-state spelling while its version-zero
            # continuity classification prevents guessing a dispatch replay.
            connection.execute(
                "UPDATE channel_inbox SET state = 'accepted' WHERE state = 'dispatching'"
            )
            # Individual statements retain the constructor's BEGIN IMMEDIATE.
            # executescript would commit it before the schema observation and
            # allow two legacy openers to race the conditional ALTER below.
            connection.execute(
                """
                CREATE TABLE IF NOT EXISTS channel_dispatch_groups (
                    channel_id TEXT NOT NULL,
                    head_source_event_id TEXT NOT NULL,
                    identity TEXT NOT NULL,
                    member_count INTEGER NOT NULL CHECK (member_count > 0),
                    payload TEXT,
                    source_provenance TEXT,
                    PRIMARY KEY (channel_id, head_source_event_id)
                )
                """
            )
            connection.execute(
                """
                CREATE TABLE IF NOT EXISTS channel_dispatch_members (
                    channel_id TEXT NOT NULL,
                    source_event_id TEXT NOT NULL,
                    head_source_event_id TEXT NOT NULL,
                    ordinal INTEGER NOT NULL,
                    envelope_digest TEXT NOT NULL,
                    PRIMARY KEY (channel_id, source_event_id),
                    UNIQUE (channel_id, head_source_event_id, ordinal),
                    FOREIGN KEY (channel_id, source_event_id)
                        REFERENCES channel_inbox(channel_id, source_event_id),
                    FOREIGN KEY (channel_id, head_source_event_id)
                        REFERENCES channel_dispatch_groups(channel_id, head_source_event_id)
                )
                """
            )
            group_columns = {
                str(row[1])
                for row in connection.execute("PRAGMA table_info(channel_dispatch_groups)")
            }
            if "source_provenance" not in group_columns:
                connection.execute(
                    "ALTER TABLE channel_dispatch_groups ADD COLUMN source_provenance TEXT"
                )
            connection.execute(
                """
                CREATE TABLE IF NOT EXISTS channel_poll_cursor (
                    channel_id TEXT PRIMARY KEY,
                    cursor TEXT NOT NULL,
                    committed_at_ms INTEGER NOT NULL
                )
                """
            )
            connection.execute(
                """
                CREATE TABLE IF NOT EXISTS channel_media_parts (
                    channel_id TEXT NOT NULL,
                    source_event_id TEXT NOT NULL,
                    part_index INTEGER NOT NULL,
                    digest TEXT NOT NULL,
                    mime_type TEXT NOT NULL,
                    size_bytes INTEGER NOT NULL,
                    stored_path TEXT NOT NULL,
                    completed_at_ms INTEGER NOT NULL,
                    PRIMARY KEY (channel_id, source_event_id, part_index),
                    FOREIGN KEY (channel_id, source_event_id)
                        REFERENCES channel_inbox(channel_id, source_event_id)
                )
                """
            )
            connection.execute(
                """
                CREATE TABLE IF NOT EXISTS channel_media_retries (
                    channel_id TEXT NOT NULL,
                    source_event_id TEXT NOT NULL,
                    part_index INTEGER NOT NULL,
                    attempts INTEGER NOT NULL,
                    next_attempt_at_ms INTEGER,
                    last_error TEXT NOT NULL,
                    PRIMARY KEY (channel_id, source_event_id, part_index),
                    FOREIGN KEY (channel_id, source_event_id)
                        REFERENCES channel_inbox(channel_id, source_event_id)
                )
                """
            )
            connection.execute(
                """
                CREATE TABLE IF NOT EXISTS channel_dispatch_retries (
                    channel_id TEXT NOT NULL,
                    source_event_id TEXT NOT NULL,
                    attempts INTEGER NOT NULL,
                    next_attempt_at_ms INTEGER,
                    last_error TEXT NOT NULL,
                    PRIMARY KEY (channel_id, source_event_id),
                    FOREIGN KEY (channel_id, source_event_id)
                        REFERENCES channel_inbox(channel_id, source_event_id)
                )
                """
            )
            connection.execute(
                """
                CREATE TABLE IF NOT EXISTS channel_transcriptions (
                    content_digest TEXT NOT NULL,
                    model_config TEXT NOT NULL,
                    state TEXT NOT NULL,
                    attempts INTEGER NOT NULL,
                    next_attempt_at_ms INTEGER,
                    transcript TEXT,
                    error TEXT,
                    updated_at_ms INTEGER NOT NULL,
                    PRIMARY KEY (content_digest, model_config)
                )
                """
            )
            connection.commit()
        if created:
            _fsync_file(self.path)
            _fsync_directory(self.path.parent)

    @contextmanager
    def _connect(self) -> Iterator[sqlite3.Connection]:
        # Every existing transaction participates, including sibling failure,
        # retry exhaustion, retention and source/group publication. This short
        # physical fence is never held across media/provider/model awaits.
        flags = os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW
        descriptor = os.open(self._request_use_fence, flags, 0o600)
        try:
            opened = os.fstat(descriptor)
            named = self._request_use_fence.stat(follow_symlinks=False)
            if (
                not stat.S_ISREG(opened.st_mode)
                or not stat.S_ISREG(named.st_mode)
                or opened.st_nlink != 1
                or (opened.st_dev, opened.st_ino) != (named.st_dev, named.st_ino)
            ):
                raise ValueError("channel inbox entry fence identity changed")
            fcntl.flock(descriptor, fcntl.LOCK_EX)
            if self.path.resolve() != self.path or (
                self.path.exists() and self.path.stat().st_nlink != 1
            ):
                raise ValueError("channel inbox physical path changed")
            connection = sqlite3.connect(self.path)
            try:
                connection.execute("PRAGMA foreign_keys = ON")
                connection.execute("PRAGMA synchronous = FULL")
                with connection:
                    yield connection
            finally:
                connection.close()
        finally:
            # Closing releases flock on every exit, including failed SQLite
            # commit. Nothing infers that another process has settled work.
            os.close(descriptor)

    def get(self, *, channel_id: str, source_event_id: str) -> InboxRow | None:
        with self._connect() as connection:
            row = connection.execute(
                "SELECT channel_id, source_event_id, state, envelope, dispatch_key, "
                "reject_reason, error, dispatch_continuity_version, source_provenance FROM channel_inbox "
                "WHERE channel_id = ? AND source_event_id = ?",
                (channel_id, source_event_id),
            ).fetchone()
        return self._row(row) if row is not None else None

    def event(self, channel_id: str, source_event_id: str) -> InboxRow | None:
        """Phase-3 spelling for a previously admitted durable event."""

        return self.get(channel_id=channel_id, source_event_id=source_event_id)

    def event_count(self, channel_id: str, source_event_id: str) -> int:
        with self._connect() as connection:
            row = connection.execute(
                "SELECT COUNT(*) FROM channel_inbox WHERE channel_id = ? AND source_event_id = ?",
                (channel_id, source_event_id),
            ).fetchone()
        return int(row[0]) if row is not None else 0

    def cursor(self, channel_id: str) -> str | None:
        with self._connect() as connection:
            row = connection.execute(
                "SELECT cursor FROM channel_poll_cursor WHERE channel_id = ?",
                (channel_id,),
            ).fetchone()
        return str(row[0]) if row is not None else None

    def commit_cursor(self, *, channel_id: str, cursor: str) -> None:
        if not isinstance(channel_id, str) or not channel_id:
            raise ValueError("channel_id is required")
        if not isinstance(cursor, str) or not cursor:
            raise ValueError("poll cursor must be nonempty text")
        with self._connect() as connection:
            connection.execute(
                """
                INSERT INTO channel_poll_cursor (channel_id, cursor, committed_at_ms)
                VALUES (?, ?, ?)
                ON CONFLICT(channel_id) DO UPDATE SET
                    cursor = excluded.cursor,
                    committed_at_ms = excluded.committed_at_ms
                """,
                (channel_id, cursor, int(time.time_ns() // 1_000_000)),
            )
            connection.commit()
        _fsync_file(self.path)

    def media_part(
        self,
        channel_id: str,
        source_event_id: str,
        part_index: int,
    ) -> Mapping[str, Any] | None:
        with self._connect() as connection:
            row = connection.execute(
                """
                SELECT digest, mime_type, size_bytes, stored_path
                FROM channel_media_parts
                WHERE channel_id = ? AND source_event_id = ? AND part_index = ?
                """,
                (channel_id, source_event_id, part_index),
            ).fetchone()
        if row is None:
            return None
        return {
            "digest": row[0],
            "mime_type": row[1],
            "size_bytes": row[2],
            "stored_path": row[3],
        }

    def media_retry(
        self,
        channel_id: str,
        source_event_id: str,
        part_index: int,
    ) -> Mapping[str, Any] | None:
        with self._connect() as connection:
            row = connection.execute(
                """
                SELECT attempts, next_attempt_at_ms, last_error
                FROM channel_media_retries
                WHERE channel_id = ? AND source_event_id = ? AND part_index = ?
                """,
                (channel_id, source_event_id, part_index),
            ).fetchone()
        if row is None:
            return None
        return {
            "attempts": int(row[0]),
            "next_attempt_at_ms": int(row[1]) if row[1] is not None else None,
            "last_error": str(row[2]),
        }

    def dispatch_retry(
        self,
        channel_id: str,
        source_event_id: str,
    ) -> Mapping[str, Any] | None:
        with self._connect() as connection:
            row = connection.execute(
                """
                SELECT attempts, next_attempt_at_ms, last_error
                FROM channel_dispatch_retries
                WHERE channel_id = ? AND source_event_id = ?
                """,
                (channel_id, source_event_id),
            ).fetchone()
        if row is None:
            return None
        return {
            "attempts": int(row[0]),
            "next_attempt_at_ms": int(row[1]) if row[1] is not None else None,
            "last_error": str(row[2]),
        }

    def record_dispatch_failure(
        self,
        *,
        channel_id: str,
        source_event_id: str,
        error: str,
        retryable: bool,
        now_ms: int,
    ) -> bool:
        """Persist one dispatch failure and return whether a retry remains."""

        bounded = str(error).strip()[:2048]
        if (
            not bounded
            or not isinstance(retryable, bool)
            or not isinstance(now_ms, int)
            or isinstance(now_ms, bool)
            or now_ms < 0
        ):
            raise ValueError("dispatch failure receipt is invalid")
        with self._connect() as connection:
            connection.execute("BEGIN IMMEDIATE")
            parent = connection.execute(
                "SELECT state FROM channel_inbox WHERE channel_id = ? AND source_event_id = ?",
                (channel_id, source_event_id),
            ).fetchone()
            if parent is None or parent[0] != "accepted":
                raise ValueError("dispatch failure parent is not accepted")
            existing = connection.execute(
                """
                SELECT attempts FROM channel_dispatch_retries
                WHERE channel_id = ? AND source_event_id = ?
                """,
                (channel_id, source_event_id),
            ).fetchone()
            attempts = (int(existing[0]) if existing is not None else 0) + 1
            retry_index = attempts - 1
            retry_scheduled = retryable and retry_index < len(_DISPATCH_RETRY_DELAYS_MS)
            next_attempt_at_ms = (
                now_ms + _DISPATCH_RETRY_DELAYS_MS[retry_index] if retry_scheduled else None
            )
            connection.execute(
                """
                INSERT INTO channel_dispatch_retries
                    (channel_id, source_event_id, attempts, next_attempt_at_ms, last_error)
                VALUES (?, ?, ?, ?, ?)
                ON CONFLICT(channel_id, source_event_id) DO UPDATE SET
                    attempts = excluded.attempts,
                    next_attempt_at_ms = excluded.next_attempt_at_ms,
                    last_error = excluded.last_error
                """,
                (
                    channel_id,
                    source_event_id,
                    attempts,
                    next_attempt_at_ms,
                    bounded,
                ),
            )
            if not retry_scheduled:
                cursor = connection.execute(
                    "UPDATE channel_inbox SET state = 'failed', error = ? "
                    "WHERE channel_id = ? AND source_event_id = ? AND state = 'accepted'",
                    (bounded, channel_id, source_event_id),
                )
                if cursor.rowcount != 1:
                    raise ValueError("dispatch failure parent is not fail-able")
                connection.execute(
                    "UPDATE channel_inbox SET state = 'failed', error = ? WHERE channel_id = ? "
                    "AND state = 'accepted' AND source_event_id IN ("
                    "SELECT source_event_id FROM channel_dispatch_members "
                    "WHERE channel_id = ? AND head_source_event_id = ?)",
                    (bounded, channel_id, channel_id, source_event_id),
                )
            connection.commit()
        _fsync_file(self.path)
        return retry_scheduled

    def transcription(
        self,
        content_digest: str,
        model_config: str,
    ) -> Mapping[str, Any] | None:
        with self._connect() as connection:
            row = connection.execute(
                """
                SELECT state, attempts, next_attempt_at_ms, transcript, error
                FROM channel_transcriptions
                WHERE content_digest = ? AND model_config = ?
                """,
                (content_digest, model_config),
            ).fetchone()
        if row is None:
            return None
        return {
            "state": str(row[0]),
            "attempts": int(row[1]),
            "next_attempt_at_ms": int(row[2]) if row[2] is not None else None,
            "transcript": str(row[3]) if row[3] is not None else None,
            "error": str(row[4]) if row[4] is not None else None,
        }

    def reserve_transcription_attempt(
        self,
        *,
        content_digest: str,
        model_config: str,
        now_ms: int,
    ) -> int | None:
        """Reserve one of at most three transcription attempts."""

        if (
            not isinstance(content_digest, str)
            or not content_digest.startswith("sha256:")
            or not isinstance(model_config, str)
            or not model_config
            or not isinstance(now_ms, int)
            or isinstance(now_ms, bool)
            or now_ms < 0
        ):
            raise ValueError("transcription attempt key is invalid")
        with self._connect() as connection:
            connection.execute("BEGIN IMMEDIATE")
            existing = connection.execute(
                """
                SELECT state, attempts, next_attempt_at_ms
                FROM channel_transcriptions
                WHERE content_digest = ? AND model_config = ?
                """,
                (content_digest, model_config),
            ).fetchone()
            if existing is not None:
                state, attempts, due_at = str(existing[0]), int(existing[1]), existing[2]
                if state in {"completed", "failed"} or attempts >= 3:
                    connection.rollback()
                    return None
                if due_at is not None and now_ms < int(due_at):
                    connection.rollback()
                    return None
                attempt = attempts + 1
                connection.execute(
                    """
                    UPDATE channel_transcriptions
                    SET state = 'pending', attempts = ?, next_attempt_at_ms = NULL,
                        error = NULL, updated_at_ms = ?
                    WHERE content_digest = ? AND model_config = ?
                    """,
                    (attempt, now_ms, content_digest, model_config),
                )
            else:
                attempt = 1
                connection.execute(
                    """
                    INSERT INTO channel_transcriptions
                        (content_digest, model_config, state, attempts,
                         next_attempt_at_ms, transcript, error, updated_at_ms)
                    VALUES (?, ?, 'pending', 1, NULL, NULL, NULL, ?)
                    """,
                    (content_digest, model_config, now_ms),
                )
            connection.commit()
        _fsync_file(self.path)
        return attempt

    def complete_transcription(
        self,
        *,
        content_digest: str,
        model_config: str,
        transcript: str,
        now_ms: int,
    ) -> None:
        normalized = unicodedata.normalize("NFC", str(transcript)).strip()
        if len(normalized.encode("utf-8")) > 64 * 1024:
            raise ValueError("transcription exceeds the host text cap")
        with self._connect() as connection:
            existing = connection.execute(
                """
                SELECT state, transcript FROM channel_transcriptions
                WHERE content_digest = ? AND model_config = ?
                """,
                (content_digest, model_config),
            ).fetchone()
            if existing is None:
                raise ValueError("transcription attempt was not reserved")
            if existing[0] == "completed" and existing[1] != normalized:
                raise ValueError("completed transcription differs from its durable receipt")
            connection.execute(
                """
                UPDATE channel_transcriptions
                SET state = 'completed', next_attempt_at_ms = NULL,
                    transcript = ?, error = NULL, updated_at_ms = ?
                WHERE content_digest = ? AND model_config = ?
                """,
                (normalized, now_ms, content_digest, model_config),
            )
            connection.commit()
        _fsync_file(self.path)

    def record_transcription_failure(
        self,
        *,
        content_digest: str,
        model_config: str,
        error: str,
        now_ms: int,
        terminal: bool = False,
    ) -> bool:
        """Persist a failed attempt; return whether another attempt remains.

        ``terminal`` records the failure as final regardless of the attempt
        count: used when retrying the same bytes cannot change the outcome
        (silent audio).
        """

        bounded = str(error).strip()[:2048]
        if not bounded:
            raise ValueError("transcription failure error is empty")
        with self._connect() as connection:
            existing = connection.execute(
                """
                SELECT state, attempts FROM channel_transcriptions
                WHERE content_digest = ? AND model_config = ?
                """,
                (content_digest, model_config),
            ).fetchone()
            if existing is None or existing[0] != "pending":
                raise ValueError("transcription failure has no pending attempt")
            attempts = int(existing[1])
            retrying = attempts < 3 and not terminal
            next_attempt_at_ms = (
                now_ms + _TRANSCRIPTION_RETRY_DELAYS_MS[attempts - 1] if retrying else None
            )
            connection.execute(
                """
                UPDATE channel_transcriptions
                SET state = ?, next_attempt_at_ms = ?, transcript = NULL,
                    error = ?, updated_at_ms = ?
                WHERE content_digest = ? AND model_config = ?
                """,
                (
                    "pending" if retrying else "failed",
                    next_attempt_at_ms,
                    bounded,
                    now_ms,
                    content_digest,
                    model_config,
                ),
            )
            connection.commit()
        _fsync_file(self.path)
        return retrying

    def record_media_failure(
        self,
        *,
        channel_id: str,
        source_event_id: str,
        part_index: int,
        error: str,
        retryable: bool,
        now_ms: int,
    ) -> bool:
        """Persist one fetch failure and return whether another retry remains."""

        bounded = str(error).strip()[:2048]
        if (
            not bounded
            or not isinstance(part_index, int)
            or isinstance(part_index, bool)
            or part_index < 0
            or not isinstance(retryable, bool)
            or not isinstance(now_ms, int)
            or isinstance(now_ms, bool)
            or now_ms < 0
        ):
            raise ValueError("media failure receipt is invalid")
        with self._connect() as connection:
            parent = connection.execute(
                "SELECT state FROM channel_inbox WHERE channel_id = ? AND source_event_id = ?",
                (channel_id, source_event_id),
            ).fetchone()
            if parent is None or parent[0] != "accepted":
                raise ValueError("media failure parent is not accepted")
            existing = connection.execute(
                """
                SELECT attempts FROM channel_media_retries
                WHERE channel_id = ? AND source_event_id = ? AND part_index = ?
                """,
                (channel_id, source_event_id, part_index),
            ).fetchone()
            attempts = (int(existing[0]) if existing is not None else 0) + 1
            retry_index = attempts - 1
            retry_scheduled = retryable and retry_index < len(_MEDIA_RETRY_DELAYS_MS)
            next_attempt_at_ms = (
                now_ms + _MEDIA_RETRY_DELAYS_MS[retry_index] if retry_scheduled else None
            )
            connection.execute(
                """
                INSERT INTO channel_media_retries
                    (channel_id, source_event_id, part_index, attempts,
                     next_attempt_at_ms, last_error)
                VALUES (?, ?, ?, ?, ?, ?)
                ON CONFLICT(channel_id, source_event_id, part_index) DO UPDATE SET
                    attempts = excluded.attempts,
                    next_attempt_at_ms = excluded.next_attempt_at_ms,
                    last_error = excluded.last_error
                """,
                (
                    channel_id,
                    source_event_id,
                    part_index,
                    attempts,
                    next_attempt_at_ms,
                    bounded,
                ),
            )
            if not retry_scheduled:
                cursor = connection.execute(
                    "UPDATE channel_inbox SET state = 'failed', error = ? "
                    "WHERE channel_id = ? AND source_event_id = ? AND state = 'accepted'",
                    (bounded, channel_id, source_event_id),
                )
                if cursor.rowcount != 1:
                    raise ValueError("media failure parent is not fail-able")
            connection.commit()
        _fsync_file(self.path)
        return retry_scheduled

    def complete_media_part(
        self,
        *,
        channel_id: str,
        source_event_id: str,
        part_index: int,
        digest: str,
        mime_type: str,
        size_bytes: int,
        stored_path: str,
        max_channel_stored_bytes: int | None = None,
        max_event_bytes: int | None = None,
    ) -> None:
        if (
            not isinstance(part_index, int)
            or isinstance(part_index, bool)
            or part_index < 0
            or not isinstance(size_bytes, int)
            or isinstance(size_bytes, bool)
            or size_bytes < 0
            or not isinstance(digest, str)
            or not digest.startswith("sha256:")
            or not isinstance(mime_type, str)
            or not mime_type
            or not isinstance(stored_path, str)
            or not stored_path
            or (
                max_channel_stored_bytes is not None
                and (
                    not isinstance(max_channel_stored_bytes, int)
                    or isinstance(max_channel_stored_bytes, bool)
                    or max_channel_stored_bytes <= 0
                )
            )
            or (
                max_event_bytes is not None
                and (
                    not isinstance(max_event_bytes, int)
                    or isinstance(max_event_bytes, bool)
                    or max_event_bytes <= 0
                )
            )
        ):
            raise ValueError("completed media metadata is invalid")
        with self._connect() as connection:
            parent = connection.execute(
                "SELECT state, envelope FROM channel_inbox "
                "WHERE channel_id = ? AND source_event_id = ?",
                (channel_id, source_event_id),
            ).fetchone()
            if parent is None or parent[0] != "accepted":
                raise ValueError("media parent is not accepted")
            existing = connection.execute(
                """
                SELECT digest, mime_type, size_bytes, stored_path
                FROM channel_media_parts
                WHERE channel_id = ? AND source_event_id = ? AND part_index = ?
                """,
                (channel_id, source_event_id, part_index),
            ).fetchone()
            expected = (digest, mime_type, size_bytes, stored_path)
            if existing is not None and tuple(existing) != expected:
                raise ValueError("completed media part differs from durable receipt")
            if existing is None:
                if max_event_bytes is not None:
                    envelope = json.loads(str(parent[1]))
                    text = envelope.get("text", "") if isinstance(envelope, Mapping) else ""
                    caption = envelope.get("caption", "") if isinstance(envelope, Mapping) else ""
                    text_bytes = len(text.encode("utf-8")) if isinstance(text, str) else 0
                    caption_bytes = len(caption.encode("utf-8")) if isinstance(caption, str) else 0
                    media_total = connection.execute(
                        "SELECT COALESCE(SUM(size_bytes), 0) FROM channel_media_parts "
                        "WHERE channel_id = ? AND source_event_id = ?",
                        (channel_id, source_event_id),
                    ).fetchone()
                    if (
                        text_bytes + caption_bytes + int(media_total[0]) + size_bytes
                        > max_event_bytes
                    ):
                        raise ValueError("channel event byte cap exceeded")
                if max_channel_stored_bytes is not None:
                    total = connection.execute(
                        "SELECT COALESCE(SUM(size_bytes), 0) FROM ("
                        "SELECT MAX(size_bytes) AS size_bytes FROM channel_media_parts "
                        "WHERE channel_id = ? GROUP BY digest)",
                        (channel_id,),
                    ).fetchone()
                    already_charged = connection.execute(
                        "SELECT 1 FROM channel_media_parts "
                        "WHERE channel_id = ? AND digest = ? LIMIT 1",
                        (channel_id, digest),
                    ).fetchone()
                    added_bytes = 0 if already_charged is not None else size_bytes
                    if int(total[0]) + added_bytes > max_channel_stored_bytes:
                        raise ValueError("channel media storage cap exceeded")
                connection.execute(
                    """
                    INSERT INTO channel_media_parts
                        (channel_id, source_event_id, part_index, digest, mime_type,
                         size_bytes, stored_path, completed_at_ms)
                    VALUES (?, ?, ?, ?, ?, ?, ?, ?)
                    """,
                    (
                        channel_id,
                        source_event_id,
                        part_index,
                        digest,
                        mime_type,
                        size_bytes,
                        stored_path,
                        int(time.time_ns() // 1_000_000),
                    ),
                )
            connection.execute(
                "DELETE FROM channel_media_retries "
                "WHERE channel_id = ? AND source_event_id = ? AND part_index = ?",
                (channel_id, source_event_id, part_index),
            )
            connection.commit()
        _fsync_file(self.path)

    def claim_next(self, *, channel_id: str) -> InboxRow | None:
        with self._connect() as connection:
            row = connection.execute(
                "SELECT channel_id, source_event_id, state, envelope, dispatch_key, "
                "reject_reason, error, dispatch_continuity_version, source_provenance FROM channel_inbox "
                "WHERE channel_id = ? AND state = 'accepted' "
                "ORDER BY received_at_ms, source_event_id LIMIT 1",
                (channel_id,),
            ).fetchone()
        # This is an observation, not a lease. The consumer must check the
        # continuity version and replay only the frozen group and payload.
        return self._row(row) if row is not None else None

    def accepted_rows(self, *, channel_id: str, limit: int) -> tuple[InboxRow, ...]:
        """The oldest accepted rows, in the order :meth:`claim_next` walks them.

        Exists so a worker can see whether the row it claimed has siblings still
        waiting behind it (inbound fragment coalescing). Read-only and bounded:
        it claims nothing and changes no state.
        """

        if not isinstance(limit, int) or isinstance(limit, bool) or limit <= 0:
            raise ValueError("accepted row limit must be a positive integer")
        with self._connect() as connection:
            rows = connection.execute(
                "SELECT channel_id, source_event_id, state, envelope, dispatch_key, "
                "reject_reason, error, dispatch_continuity_version, source_provenance FROM channel_inbox "
                "WHERE channel_id = ? AND state = 'accepted' "
                "ORDER BY received_at_ms, source_event_id LIMIT ?",
                (channel_id, limit),
            ).fetchall()
        return tuple(self._row(row) for row in rows)

    @staticmethod
    def _read_dispatch_group(
        connection: sqlite3.Connection, channel_id: str, source_event_id: str
    ) -> FrozenDispatchGroup | None:
        owner = connection.execute(
            "SELECT g.head_source_event_id, g.identity, g.payload, g.member_count, g.source_provenance "
            "FROM channel_dispatch_members AS m JOIN channel_dispatch_groups AS g "
            "ON g.channel_id = m.channel_id AND "
            "g.head_source_event_id = m.head_source_event_id "
            "WHERE m.channel_id = ? AND m.source_event_id = ?",
            (channel_id, source_event_id),
        ).fetchone()
        if owner is None:
            return None
        members = connection.execute(
            "SELECT i.channel_id, i.source_event_id, i.state, i.envelope, i.dispatch_key, "
            "i.reject_reason, i.error, m.ordinal, m.envelope_digest, "
            "i.dispatch_continuity_version, i.source_provenance "
            "FROM channel_dispatch_members AS m JOIN channel_inbox AS i "
            "ON i.channel_id = m.channel_id AND i.source_event_id = m.source_event_id "
            "WHERE m.channel_id = ? AND m.head_source_event_id = ? ORDER BY m.ordinal",
            (channel_id, owner[0]),
        ).fetchall()
        if len(members) != owner[3] or not members or members[0][1] != owner[0]:
            raise ValueError("frozen dispatch head is missing")
        rows = []
        for ordinal, member in enumerate(members):
            if member[7] != ordinal or hashlib.sha256(member[3].encode()).hexdigest() != member[8]:
                raise ValueError("frozen dispatch member differs from admitted content")
            if member[10] != owner[4]:
                raise ValueError("frozen dispatch member source provenance differs")
            rows.append(SQLiteChannelInbox._row((*member[:7], member[9], member[10])))
        payload = json.loads(owner[2]) if owner[2] is not None else None
        if payload is not None and not isinstance(payload, dict):
            raise ValueError("frozen dispatch payload is invalid")
        provenance = InboxSourceProvenance.decode(owner[4]) if owner[4] is not None else None
        return FrozenDispatchGroup(channel_id, owner[0], owner[1], tuple(rows), payload, provenance)

    def dispatch_group(
        self, *, channel_id: str, source_event_id: str
    ) -> FrozenDispatchGroup | None:
        with self._connect() as connection:
            connection.execute("BEGIN")
            return self._read_dispatch_group(connection, channel_id, source_event_id)

    def freeze_dispatch_group(
        self, *, channel_id: str, rows: Sequence[InboxRow], identity: str
    ) -> FrozenDispatchGroup:
        """Freeze ordered membership before media or runtime awaits.

        Caller owns semantic grouping and target admission. The transaction
        verifies the exact admitted snapshots and exclusive membership. It is
        not a worker lease and cannot make a non-idempotent runtime safe.
        """
        if not rows or len(rows) > 64 or not isinstance(identity, str) or not identity:
            raise ValueError("dispatch group is invalid")
        provenance = rows[0].source_provenance
        if any(row.source_provenance != provenance for row in rows):
            raise DispatchGroupConflict("dispatch group has incompatible source provenance")
        encoded_provenance = provenance.encode() if provenance is not None else None
        ids = [row.source_event_id for row in rows]
        if len(ids) != len(set(ids)) or any(row.channel_id != channel_id for row in rows):
            raise ValueError("dispatch group membership is invalid")
        with self._connect() as connection:
            connection.execute("BEGIN IMMEDIATE")
            memberships = [self._read_dispatch_group(connection, channel_id, key) for key in ids]
            if any(group is not None for group in memberships):
                winner = memberships[0]
                if (
                    winner is None
                    or winner.identity != identity
                    or winner.source_provenance != provenance
                    or [row.source_event_id for row in winner.rows] != ids
                    or any(
                        group is None or group.head_source_event_id != winner.head_source_event_id
                        for group in memberships
                    )
                ):
                    raise DispatchGroupConflict("dispatch group is already fixed")
                for expected, actual in zip(rows, winner.rows, strict=True):
                    if _canonical_json(expected.envelope) != _canonical_json(actual.envelope):
                        raise ValueError("dispatch group content changed")
                return winner
            digests = []
            for row in rows:
                actual = connection.execute(
                    "SELECT state, envelope, dispatch_key, dispatch_continuity_version, source_provenance "
                    "FROM channel_inbox "
                    "WHERE channel_id = ? AND source_event_id = ?",
                    (channel_id, row.source_event_id),
                ).fetchone()
                encoded = _canonical_json(row.envelope).decode("utf-8")
                if (
                    actual is None
                    or actual[0] != "accepted"
                    or actual[1] != encoded
                    or actual[2] != row.dispatch_key
                    or not row.dispatch_key
                    or actual[3] != 1
                    or actual[4] != encoded_provenance
                ):
                    raise DispatchGroupConflict(
                        "dispatch source is no longer the expected accepted row"
                    )
                digests.append(hashlib.sha256(encoded.encode()).hexdigest())
            connection.execute(
                "INSERT INTO channel_dispatch_groups "
                "(channel_id, head_source_event_id, identity, member_count, source_provenance) "
                "VALUES (?, ?, ?, ?, ?)",
                (channel_id, ids[0], identity, len(ids), encoded_provenance),
            )
            connection.executemany(
                "INSERT INTO channel_dispatch_members "
                "(channel_id, source_event_id, head_source_event_id, ordinal, envelope_digest) "
                "VALUES (?, ?, ?, ?, ?)",
                [(channel_id, key, ids[0], n, digests[n]) for n, key in enumerate(ids)],
            )
            result = self._read_dispatch_group(connection, channel_id, ids[0])
            if result is None:
                raise ValueError("dispatch group publication is missing")
            connection.commit()
        return result

    def freeze_dispatch_payload(
        self, *, channel_id: str, source_event_id: str, identity: str, payload: Mapping[str, Any]
    ) -> Mapping[str, Any]:
        """Commit or recover exact wire input before handing it to the runtime."""
        encoded = _canonical_json(payload).decode("utf-8")
        with self._connect() as connection:
            connection.execute("BEGIN IMMEDIATE")
            group = self._read_dispatch_group(connection, channel_id, source_event_id)
            if (
                group is None
                or group.head_source_event_id != source_event_id
                or group.identity != identity
            ):
                raise ValueError("dispatch payload has no matching frozen group")
            if any(row.state != "accepted" for row in group.rows):
                raise ValueError("dispatch group is not accepted")
            if group.payload is not None:
                if _canonical_json(group.payload).decode("utf-8") != encoded:
                    raise ValueError("dispatch payload differs from committed input")
                return group.payload
            connection.execute(
                "UPDATE channel_dispatch_groups SET payload = ? "
                "WHERE channel_id = ? AND head_source_event_id = ? AND payload IS NULL",
                (encoded, channel_id, source_event_id),
            )
            connection.commit()
        return json.loads(encoded)

    def mark_dispatch_group_dispatched(self, *, channel_id: str, source_event_id: str) -> None:
        """Record runtime acceptance for all fragments in one transaction."""
        with self._connect() as connection:
            connection.execute("BEGIN IMMEDIATE")
            group = self._read_dispatch_group(connection, channel_id, source_event_id)
            if (
                group is None
                or group.head_source_event_id != source_event_id
                or group.payload is None
            ):
                raise ValueError("dispatch group has no committed payload")
            if any(row.state not in ("accepted", "dispatched") for row in group.rows):
                raise ValueError("dispatch group is not dispatchable")
            for row in group.rows:
                connection.execute(
                    "UPDATE channel_inbox SET state = 'dispatched' "
                    "WHERE channel_id = ? AND source_event_id = ?",
                    (channel_id, row.source_event_id),
                )
                connection.execute(
                    "DELETE FROM channel_dispatch_retries WHERE channel_id = ? AND source_event_id = ?",
                    (channel_id, row.source_event_id),
                )
            connection.commit()

    @staticmethod
    def retained_dispatch_sources(connection: sqlite3.Connection) -> set[tuple[str, str]]:
        """Select source dependencies retained by an opaque native root.

        The caller owns the transaction. This is a retention observation, not
        permission to read or dispatch. Unreadable media still belongs to its
        retained source, and only the native owner can retire that dependency.
        """
        columns = {
            row[1] for row in connection.execute("PRAGMA table_info(channel_dispatch_groups)")
        }
        if "native_request_root" not in columns:
            return set()
        return {
            (str(channel_id), str(source_event_id))
            for channel_id, source_event_id in connection.execute(
                "SELECT m.channel_id, m.source_event_id FROM channel_dispatch_members AS m "
                "JOIN channel_dispatch_groups AS g ON g.channel_id = m.channel_id "
                "AND g.head_source_event_id = m.head_source_event_id "
                "WHERE g.native_request_root IS NOT NULL"
            )
        }

    @staticmethod
    def prune_dispatch_groups(
        connection: sqlite3.Connection, channel_id: str, cutoff_ms: int
    ) -> None:
        """Retire membership only when every source is terminal and eligible.

        Caller owns the retention transaction and deletion of the source rows.
        Pending group members retain terminal siblings needed for exact replay.
        A native root pins the whole original group until its native owner can
        retire all dependents. Cancellation and expiry do not retire evidence.
        Python treats the root as opaque and never verifies or clears it here.
        """
        columns = {
            row[1] for row in connection.execute("PRAGMA table_info(channel_dispatch_groups)")
        }
        unpinned = "g.native_request_root IS NULL AND " if "native_request_root" in columns else ""
        heads = connection.execute(
            "SELECT g.head_source_event_id FROM channel_dispatch_groups AS g "
            f"WHERE g.channel_id = ? AND {unpinned}NOT EXISTS ("  # noqa: S608 - closed SQL fragment
            "SELECT 1 FROM channel_dispatch_members AS m JOIN channel_inbox AS i "
            "ON i.channel_id = m.channel_id AND i.source_event_id = m.source_event_id "
            "WHERE m.channel_id = g.channel_id AND m.head_source_event_id = g.head_source_event_id "
            "AND (i.state NOT IN ('dispatched', 'failed', 'rejected_known') OR i.received_at_ms > ?))",
            (channel_id, cutoff_ms),
        ).fetchall()
        for (head,) in heads:
            connection.execute(
                "DELETE FROM channel_dispatch_members WHERE channel_id = ? AND head_source_event_id = ?",
                (channel_id, head),
            )
            connection.execute(
                "DELETE FROM channel_dispatch_groups WHERE channel_id = ? AND head_source_event_id = ?",
                (channel_id, head),
            )

    def mark_dispatched(self, *, channel_id: str, source_event_id: str) -> None:
        with self._connect() as connection:
            connection.execute("BEGIN IMMEDIATE")
            cursor = connection.execute(
                "UPDATE channel_inbox SET state = 'dispatched' "
                "WHERE channel_id = ? AND source_event_id = ? "
                "AND state IN ('accepted', 'dispatching')",
                (channel_id, source_event_id),
            )
            if cursor.rowcount != 1:
                raise ValueError("inbox row is not dispatchable")
            connection.execute(
                "DELETE FROM channel_dispatch_retries WHERE channel_id = ? AND source_event_id = ?",
                (channel_id, source_event_id),
            )
            connection.commit()
        _fsync_file(self.path)

    def mark_failed(self, *, channel_id: str, source_event_id: str, error: str) -> None:
        bounded = str(error).strip()[:2048]
        if not bounded:
            raise ValueError("failed inbox row requires a bounded error")
        with self._connect() as connection:
            connection.execute("BEGIN IMMEDIATE")
            cursor = connection.execute(
                "UPDATE channel_inbox SET state = 'failed', error = ? "
                "WHERE channel_id = ? AND source_event_id = ? AND state = 'accepted'",
                (bounded, channel_id, source_event_id),
            )
            if cursor.rowcount != 1:
                raise ValueError("inbox row is not fail-able")
            connection.execute(
                "UPDATE channel_inbox SET state = 'failed', error = ? WHERE channel_id = ? "
                "AND state = 'accepted' AND source_event_id IN ("
                "SELECT sibling.source_event_id FROM channel_dispatch_members AS sibling "
                "JOIN channel_dispatch_members AS member ON sibling.channel_id = member.channel_id "
                "AND sibling.head_source_event_id = member.head_source_event_id "
                "WHERE member.channel_id = ? AND member.source_event_id = ?)",
                (bounded, channel_id, channel_id, source_event_id),
            )
            connection.commit()

    async def drain_once(
        self,
        *,
        channel_id: str,
        fetch_media: Callable[..., Awaitable[Any] | Any],
    ) -> int:
        # Admission already made deterministic rejections terminal, so only
        # accepted rows are eligible for post-ack provider media I/O.
        with self._connect() as connection:
            rows = connection.execute(
                "SELECT source_event_id, envelope FROM channel_inbox "
                "WHERE channel_id = ? AND state = 'accepted' ORDER BY received_at_ms",
                (channel_id,),
            ).fetchall()
        fetched = 0
        for source_event_id, envelope_json in rows:
            envelope = json.loads(envelope_json)
            for ref in envelope.get("media", []):
                await _await(fetch_media(ref))
                fetched += 1
            del source_event_id
        return fetched

    def _insert(
        self,
        *,
        channel_id: str,
        source_event_id: str,
        state: str,
        envelope: Mapping[str, Any],
        dispatch_key: str | None,
        reject_reason: str | None,
        source_provenance: InboxSourceProvenance | None = None,
    ) -> bool:
        if source_provenance is not None and source_provenance.channel_id != channel_id:
            raise ValueError("source provenance is bound to another channel")
        with self._connect() as connection:
            cursor = connection.execute(
                "INSERT OR IGNORE INTO channel_inbox "
                "(channel_id, source_event_id, state, envelope, dispatch_key, "
                "reject_reason, error, received_at_ms, dispatch_continuity_version, source_provenance) "
                "VALUES (?, ?, ?, ?, ?, ?, NULL, ?, 1, ?)",
                (
                    channel_id,
                    source_event_id,
                    state,
                    _canonical_json(envelope).decode("utf-8"),
                    dispatch_key,
                    reject_reason,
                    int(time.time_ns() // 1_000_000),
                    source_provenance.encode() if source_provenance is not None else None,
                ),
            )
            connection.commit()
            inserted = cursor.rowcount == 1
        if inserted:
            _fsync_file(self.path)
        return inserted

    @staticmethod
    def _dispatch_key(channel_id: str, source_event_id: str) -> str:
        digest = hashlib.sha256(f"{channel_id}\0{source_event_id}".encode()).hexdigest()
        return f"toolkit-dispatch-v1:{digest}"

    @staticmethod
    def _row(row: Sequence[Any]) -> InboxRow:
        envelope = json.loads(row[3])
        return InboxRow(
            channel_id=row[0],
            source_event_id=row[1],
            state=row[2],
            envelope=envelope,
            dispatch_key=row[4],
            reject_reason=row[5],
            error=row[6],
            media=tuple(envelope.get("media", ())),
            dispatch_continuity_version=row[7],
            source_provenance=(
                InboxSourceProvenance.decode(row[8]) if row[8] is not None else None
            ),
        )
