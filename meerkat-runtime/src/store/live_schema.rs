//! Physical representation of the independent public Live component.
//!
//! These tables do not participate in actor WholeBlob/HeadCanonical revisions.
//! The generated Live owners prepare changes; one RuntimeStore transaction
//! commits the exact component head and all subordinate rows.

pub(super) const OBJECT_NAMES: &[&str] = &[
    "runtime_live_heads",
    "runtime_live_events",
    "runtime_live_sources",
    "runtime_live_attempts",
];

pub(super) fn initialize(tx: &rusqlite::Transaction<'_>) -> Result<(), rusqlite::Error> {
    tx.execute_batch(
        r"
        CREATE TABLE runtime_live_heads (
            session_id TEXT PRIMARY KEY NOT NULL,
            format_version INTEGER NOT NULL CHECK (format_version = 1),
            generation INTEGER NOT NULL CHECK (generation > 0),
            revision INTEGER NOT NULL CHECK (revision > 0),
            event_count INTEGER NOT NULL CHECK (event_count >= 0),
            prefix_digest BLOB NOT NULL CHECK (length(prefix_digest) = 32),
            commit_digest BLOB NOT NULL CHECK (length(commit_digest) = 32),
            used_records INTEGER NOT NULL CHECK (used_records >= 0),
            used_bytes INTEGER NOT NULL CHECK (used_bytes >= 0),
            reserved_records INTEGER NOT NULL CHECK (reserved_records >= 0),
            reserved_bytes INTEGER NOT NULL CHECK (reserved_bytes >= 0),
            ingress_generation INTEGER NOT NULL CHECK (ingress_generation > 0),
            transcript_snapshot BLOB NOT NULL,
            request_snapshot BLOB NOT NULL
        );
        CREATE TABLE runtime_live_events (
            session_id TEXT NOT NULL REFERENCES runtime_live_heads(session_id) ON DELETE CASCADE,
            sequence INTEGER NOT NULL CHECK (sequence > 0),
            channel_id TEXT NOT NULL CHECK (length(CAST(channel_id AS BLOB)) BETWEEN 1 AND 128),
            record BLOB NOT NULL,
            record_digest BLOB NOT NULL CHECK (length(record_digest) = 32),
            commit_revision INTEGER NOT NULL CHECK (commit_revision > 0),
            prefix_digest BLOB NOT NULL CHECK (length(prefix_digest) = 32),
            PRIMARY KEY (session_id, sequence)
        );
        CREATE TABLE runtime_live_sources (
            session_id TEXT NOT NULL REFERENCES runtime_live_heads(session_id) ON DELETE CASCADE,
            channel_id TEXT NOT NULL CHECK (length(CAST(channel_id AS BLOB)) BETWEEN 1 AND 128),
            source_identity BLOB NOT NULL,
            record_digest BLOB NOT NULL CHECK (length(record_digest) = 32),
            record BLOB NOT NULL,
            PRIMARY KEY (session_id, channel_id, source_identity)
        );
        CREATE TABLE runtime_live_attempts (
            session_id TEXT NOT NULL REFERENCES runtime_live_heads(session_id) ON DELETE CASCADE,
            attempt_identity BLOB NOT NULL,
            payload_digest BLOB NOT NULL CHECK (length(payload_digest) = 32),
            attempt BLOB NOT NULL,
            disposition BLOB NOT NULL,
            sequence INTEGER NOT NULL CHECK (sequence > 0),
            PRIMARY KEY (session_id, attempt_identity)
        );
        ",
    )
}
