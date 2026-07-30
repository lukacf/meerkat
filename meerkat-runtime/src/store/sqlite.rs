//! SQLite-backed RuntimeStore with atomic cross-table commits.

#[cfg(feature = "sqlite-store")]
mod inner {
    use std::collections::{HashMap, HashSet};
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use meerkat_core::lifecycle::{InputId, RunBoundaryReceipt, RunId};
    use meerkat_store::json_column::JsonColumnBytes;
    use meerkat_store::sqlite_store::begin_immediate_transaction;
    use rusqlite::{Connection, OptionalExtension, Transaction, params};

    use crate::identifiers::{IdempotencyKey, LogicalRuntimeId};
    use crate::input_state::{InputStatePersistenceRecord, StoredInputState};
    use crate::runtime_state::RuntimeState;
    use crate::store::{
        AuthOAuthFlowSnapshotUpdate, CommittedRecoveryBoundary, CommittedWholeBlobProvisionalTail,
        CommittedWholeBlobSnapshot, ExactInputStateObservation, FencedInputStateBatchCasOutcome,
        FencedMachineLifecycleCasOutcome, HeadCanonicalProvisionalTailAuthority,
        HeadCanonicalStoreAuthority, InputStateBatchCasImplementationProfile,
        InputStateBatchCasOutcome, InputStateRow, MachineLifecycleCasOutcome,
        MachineLifecycleCommit, MachineLifecycleExpectedVersion, MachineLifecycleObservation,
        MachineLifecycleObservationVersion, MachineLifecycleSnapshot, MachineLifecycleStoreRecord,
        PreparedDurableTailRecoverySource, PreparedHeadCanonicalProvisionalTail,
        PreparedRecoveryInputSnapshot, PreparedRecoveryInputStateMutation,
        PreparedRecoveryReceiptSource, PreparedRuntimeSessionCommit,
        PreparedRuntimeSessionCommitPayload, PreparedRuntimeSessionCommitResult,
        PreparedWholeBlobProvisionalTail, PreparedWholeBlobRewriteStoreParts,
        PreparedWholeBlobSnapshotCas, RecoveryCommitStatus, RecoveryInputSetRevision,
        RecoveryInputStateMutation, RuntimeDeliveryAuthorityCasOutcome,
        RuntimeDeliveryAuthorityRecord, RuntimeDeliveryStoreRecord, RuntimeSessionAuthority,
        RuntimeSessionAuthorityReadCost, RuntimeSessionPersistenceProfile, RuntimeStore,
        RuntimeStoreError, RuntimeStoreWriteFence, RuntimeStoreWriteFenceOutcome,
        SerializedSessionSnapshot, WholeBlobProvisionalTailAuthority, WholeBlobSnapshotCasOutcome,
        WholeBlobStoreAuthority, classify_machine_lifecycle_record,
        complete_compaction_projection_intent, decoded_prepared_machine_lifecycle_replacement,
        execute_runtime_store_write_fence, parsed_whole_blob_snapshot,
        prepare_input_state_batch_cas, prepare_machine_lifecycle_replacement,
        prepare_recovery_input_state_mutations, validate_input_state_batch_read_ids,
        validate_machine_lifecycle_replacement,
    };

    const CREATE_RUNTIME_SCHEMA_SQL: &str = r"
CREATE TABLE IF NOT EXISTS runtime_input_states (
    runtime_id TEXT NOT NULL,
    input_id TEXT NOT NULL,
    state_json BLOB NOT NULL,
    PRIMARY KEY (runtime_id, input_id)
);
CREATE TABLE IF NOT EXISTS runtime_boundary_receipts (
    runtime_id TEXT NOT NULL,
    run_id TEXT NOT NULL,
    sequence INTEGER NOT NULL,
    receipt_json BLOB NOT NULL,
    PRIMARY KEY (runtime_id, run_id, sequence)
);
CREATE TABLE IF NOT EXISTS runtime_session_snapshots (
    runtime_id TEXT PRIMARY KEY,
    session_snapshot BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS runtime_states (
    runtime_id TEXT PRIMARY KEY,
    runtime_state_json BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS runtime_ops_lifecycle (
    runtime_id TEXT PRIMARY KEY,
    state_json BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS runtime_retired_ops_epochs (
    runtime_id TEXT NOT NULL,
    epoch_id TEXT NOT NULL,
    PRIMARY KEY (runtime_id, epoch_id)
);
CREATE TABLE IF NOT EXISTS runtime_auth_oauth_flow_state (
    id TEXT PRIMARY KEY,
    state_json BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS runtime_projection_quarantine (
    runtime_id TEXT PRIMARY KEY
);
CREATE TABLE IF NOT EXISTS runtime_compaction_projection_outbox (
    runtime_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    parent_revision TEXT NOT NULL,
    revision TEXT NOT NULL,
    commit_fingerprint TEXT NOT NULL,
    intent_json BLOB NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('pending', 'finalized')),
    PRIMARY KEY (runtime_id, session_id, parent_revision, revision, commit_fingerprint)
);
CREATE TABLE IF NOT EXISTS runtime_mob_host_bindings (
    mob_id TEXT PRIMARY KEY,
    record_json BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS runtime_mob_host_revocations (
    mob_id TEXT PRIMARY KEY,
    receipt_json BLOB NOT NULL
)";

    fn migration_0001_runtime_schema(
        tx: &rusqlite::Transaction<'_>,
    ) -> Result<(), rusqlite::Error> {
        tx.execute_batch(CREATE_RUNTIME_SCHEMA_SQL)
    }

    const CREATE_RUNTIME_DELIVERY_SCHEMA_SQL: &str = r"
CREATE TABLE IF NOT EXISTS runtime_delivery_authority (
    runtime_id TEXT PRIMARY KEY,
    revision BLOB NOT NULL CHECK (length(revision) = 8),
    state_json BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS runtime_delivery_inbox (
    runtime_id TEXT NOT NULL,
    delivery_id TEXT NOT NULL,
    sequence BLOB NOT NULL CHECK (length(sequence) = 8),
    submission_json BLOB NOT NULL,
    PRIMARY KEY (runtime_id, delivery_id),
    UNIQUE (runtime_id, sequence)
);
CREATE INDEX IF NOT EXISTS idx_runtime_delivery_inbox_sequence
    ON runtime_delivery_inbox (runtime_id, sequence)";

    fn migration_0001_runtime_delivery_inbox(
        tx: &rusqlite::Transaction<'_>,
    ) -> Result<(), rusqlite::Error> {
        tx.execute_batch(CREATE_RUNTIME_DELIVERY_SCHEMA_SQL)
    }

    const HEAD_CANONICAL_FROZEN_SNAPSHOT_ERROR: &str =
        "runtime session snapshot is frozen by head-canonical authority";

    const CREATE_RUNTIME_SESSION_AUTHORITY_SQL: &str = r"
CREATE TABLE runtime_session_authority (
    runtime_id TEXT PRIMARY KEY,
    authority_version INTEGER NOT NULL CHECK (authority_version = 1),
    session_id TEXT NOT NULL,
    store_revision INTEGER NOT NULL CHECK (store_revision >= 1),
    boundary_head_json BLOB NOT NULL CHECK (length(boundary_head_json) > 0),
    committed_head_token TEXT NOT NULL CHECK (length(committed_head_token) > 0)
);
CREATE TABLE runtime_head_canonical_provisional_tails (
    runtime_id TEXT PRIMARY KEY,
    authority_version INTEGER NOT NULL CHECK (authority_version = 1),
    session_id TEXT NOT NULL,
    base_store_revision INTEGER NOT NULL CHECK (base_store_revision >= 1),
    base_committed_head_token TEXT NOT NULL CHECK (length(base_committed_head_token) > 0),
    physical_store_revision INTEGER NOT NULL
        CHECK (physical_store_revision > base_store_revision),
    physical_head_token TEXT NOT NULL CHECK (length(physical_head_token) > 0),
    run_id TEXT NOT NULL CHECK (length(run_id) > 0),
    candidate_sequence INTEGER NOT NULL CHECK (
        candidate_sequence >= 1
        AND physical_store_revision = base_store_revision + candidate_sequence
    ),
    candidate_message_count INTEGER NOT NULL CHECK (candidate_message_count >= 0),
    candidate_conversation_digest TEXT NOT NULL
        CHECK (length(candidate_conversation_digest) > 0),
    catalog_json BLOB NOT NULL CHECK (length(catalog_json) > 0),
    compaction_intents_json BLOB NOT NULL CHECK (length(compaction_intents_json) > 0),
    predecessor_candidate_message_count INTEGER,
    predecessor_candidate_conversation_digest TEXT,
    predecessor_catalog_json BLOB,
    predecessor_compaction_intents_json BLOB,
    CHECK (
        (predecessor_candidate_message_count IS NULL
         AND predecessor_candidate_conversation_digest IS NULL
         AND predecessor_catalog_json IS NULL
         AND predecessor_compaction_intents_json IS NULL)
        OR
        (predecessor_candidate_message_count IS NOT NULL
         AND predecessor_candidate_conversation_digest IS NOT NULL
         AND predecessor_catalog_json IS NOT NULL
         AND predecessor_compaction_intents_json IS NOT NULL
         AND predecessor_candidate_message_count >= 0
         AND length(predecessor_candidate_conversation_digest) > 0
         AND length(predecessor_catalog_json) > 0
         AND length(predecessor_compaction_intents_json) > 0)
    )
);
CREATE TRIGGER runtime_session_snapshots_head_authority_no_insert
BEFORE INSERT ON runtime_session_snapshots
WHEN EXISTS (
    SELECT 1 FROM runtime_session_authority
    WHERE runtime_id = NEW.runtime_id
)
BEGIN
    SELECT RAISE(ABORT, 'runtime session snapshot is frozen by head-canonical authority');
END;
CREATE TRIGGER runtime_session_snapshots_head_authority_no_update
BEFORE UPDATE ON runtime_session_snapshots
WHEN EXISTS (
        SELECT 1 FROM runtime_session_authority
        WHERE runtime_id = OLD.runtime_id
    )
    OR EXISTS (
        SELECT 1 FROM runtime_session_authority
        WHERE runtime_id = NEW.runtime_id
    )
BEGIN
    SELECT RAISE(ABORT, 'runtime session snapshot is frozen by head-canonical authority');
END;
CREATE TRIGGER runtime_session_snapshots_head_authority_no_delete
BEFORE DELETE ON runtime_session_snapshots
WHEN EXISTS (
    SELECT 1 FROM runtime_session_authority
    WHERE runtime_id = OLD.runtime_id
)
BEGIN
    SELECT RAISE(ABORT, 'runtime session snapshot is frozen by head-canonical authority');
END;
CREATE TRIGGER runtime_session_authority_after_delete
AFTER DELETE ON runtime_session_authority
BEGIN
    DELETE FROM runtime_head_canonical_provisional_tails
    WHERE runtime_id = OLD.runtime_id;
END";

    const CREATE_RUNTIME_WHOLE_BLOB_AUTHORITY_SQL: &str = r"
CREATE TABLE runtime_whole_blob_authority (
    runtime_id TEXT PRIMARY KEY,
    authority_version INTEGER NOT NULL CHECK (authority_version = 1),
    session_id TEXT NOT NULL,
    store_revision INTEGER NOT NULL CHECK (store_revision >= 1),
    blob_sha256 TEXT NOT NULL CHECK (length(blob_sha256) > 0)
);
CREATE TABLE runtime_whole_blob_bodies (
    blob_sha256 TEXT PRIMARY KEY CHECK (length(blob_sha256) > 0),
    session_snapshot BLOB NOT NULL
);
CREATE TABLE runtime_whole_blob_provisional_tails (
    runtime_id TEXT PRIMARY KEY,
    authority_version INTEGER NOT NULL CHECK (authority_version = 1),
    session_id TEXT NOT NULL,
    base_store_revision INTEGER NOT NULL CHECK (base_store_revision >= 1),
    base_blob_sha256 TEXT NOT NULL CHECK (length(base_blob_sha256) > 0),
    run_id TEXT NOT NULL,
    candidate_sequence INTEGER NOT NULL CHECK (candidate_sequence >= 1),
    candidate_blob_sha256 TEXT NOT NULL CHECK (length(candidate_blob_sha256) > 0),
    conversation_digest TEXT NOT NULL CHECK (length(conversation_digest) > 0),
    message_count INTEGER NOT NULL CHECK (message_count >= 0),
    catalog_json BLOB NOT NULL CHECK (length(catalog_json) > 0),
    compaction_intents_json BLOB NOT NULL CHECK (length(compaction_intents_json) > 0)
);
CREATE TABLE runtime_session_catalog (
    runtime_id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL UNIQUE,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    entry_json BLOB NOT NULL CHECK (length(entry_json) > 0)
);
CREATE INDEX runtime_session_catalog_updated
ON runtime_session_catalog(updated_at_ms DESC, session_id ASC)";

    const CREATE_RUNTIME_RECOVERY_BOUNDARY_SCHEMA_SQL: &str = r"
CREATE TABLE runtime_recovery_boundaries (
    runtime_id TEXT NOT NULL,
    candidate_id TEXT NOT NULL CHECK (length(candidate_id) > 0),
    boundary_json BLOB NOT NULL CHECK (length(boundary_json) > 0),
    PRIMARY KEY (runtime_id, candidate_id)
)";

    const CREATE_RUNTIME_ORDINARY_BOUNDARY_WITNESS_SCHEMA_SQL: &str = r"
CREATE TABLE runtime_session_boundary_witnesses (
    runtime_id TEXT NOT NULL,
    boundary_key TEXT NOT NULL CHECK (length(boundary_key) > 0),
    witness_version INTEGER NOT NULL CHECK (witness_version = 1),
    request_digest TEXT NOT NULL CHECK (length(request_digest) > 0),
    PRIMARY KEY (runtime_id, boundary_key)
)";

    /// One-time migration authority for receipts that exact 0.8.10 wrote before
    /// ordinary-boundary request witnesses existed.
    ///
    /// Hashing the exact released bytes during v1 -> v2 activation prevents a
    /// missing witness on a current store from being mistaken for legacy state
    /// without duplicating every historical receipt body. A successful lazy
    /// adoption consumes exactly one marker atomically with installation of the
    /// current request witness.
    const CREATE_RELEASED_0810_BOUNDARY_RECEIPT_MARKERS_SQL: &str = r"
CREATE TABLE runtime_released_0810_boundary_receipts (
    runtime_id TEXT NOT NULL,
    run_id TEXT NOT NULL,
    sequence INTEGER NOT NULL,
    receipt_sha256 TEXT NOT NULL CHECK (length(receipt_sha256) = 71),
    PRIMARY KEY (runtime_id, run_id, sequence)
)";

    fn capture_released_0810_boundary_receipt_markers(
        tx: &rusqlite::Transaction<'_>,
    ) -> Result<(), rusqlite::Error> {
        use sha2::Digest as _;

        let receipts = {
            let mut statement = tx.prepare(
                r"
                SELECT runtime_id, run_id, sequence, receipt_json
                FROM runtime_boundary_receipts
                ORDER BY runtime_id, run_id, sequence
                ",
            )?;
            statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, JsonColumnBytes>(3)?.into_bytes(),
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?
        };
        for (runtime_id, run_id, sequence, receipt_json) in receipts {
            let invalid_receipt = |detail: String| {
                rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "released runtime boundary receipt {runtime_id}/{run_id}/{sequence} \
                         failed marker capture: {detail}"
                    ),
                )))
            };
            let receipt: RunBoundaryReceipt = serde_json::from_slice(&receipt_json)
                .map_err(|error| invalid_receipt(error.to_string()))?;
            if receipt.run_id.0.to_string() != run_id
                || encode_receipt_sequence(receipt.sequence) != sequence
            {
                return Err(invalid_receipt(
                    "receipt body identity differs from its primary key".to_string(),
                ));
            }
            let receipt_sha256 = format!("sha256:{:x}", sha2::Sha256::digest(&receipt_json));
            tx.execute(
                r"
                INSERT INTO runtime_released_0810_boundary_receipts (
                    runtime_id, run_id, sequence, receipt_sha256
                ) VALUES (?1, ?2, ?3, ?4)
                ",
                params![runtime_id, run_id, sequence, receipt_sha256],
            )?;
        }
        Ok(())
    }

    const CREATE_RUNTIME_PENDING_TERMINAL_OWNER_SCHEMA_SQL: &str = r"
CREATE TABLE runtime_pending_terminal_owners (
    runtime_id TEXT NOT NULL,
    owner_input_id TEXT NOT NULL,
    PRIMARY KEY (runtime_id, owner_input_id)
);

INSERT OR IGNORE INTO runtime_pending_terminal_owners
    (runtime_id, owner_input_id)
SELECT runtime_id, input_id
FROM runtime_input_states
WHERE CASE WHEN json_valid(state_json) THEN
    (
        json_extract(state_json, '$.terminal_completion.owner_input_id') = input_id
        AND json_extract(state_json, '$.terminal_completion.phase.phase') = 'pending'
    )
    OR
    (
        json_extract(state_json, '$.interaction_terminal_outbox.candidate_owner_input_id') = input_id
        AND json_extract(state_json, '$.interaction_terminal_outbox.phase.phase')
            IN ('candidate', 'finalized')
    )
ELSE 0 END";
    const LOAD_PENDING_TERMINAL_OWNER_FIRST_PAGE_SQL: &str = r"
SELECT owner_input_id
FROM runtime_pending_terminal_owners
WHERE runtime_id = ?1
ORDER BY owner_input_id
LIMIT ?2";
    const LOAD_PENDING_TERMINAL_OWNER_CONTINUATION_PAGE_SQL: &str = r"
SELECT owner_input_id
FROM runtime_pending_terminal_owners
WHERE runtime_id = ?1
  AND owner_input_id > ?2
ORDER BY owner_input_id
LIMIT ?3";

    const CREATE_RUNTIME_HEAD_CANONICAL_ACTIVATION_SCHEMA_SQL: &str = r"
CREATE TABLE runtime_head_canonical_profile_pin (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    pin_version INTEGER NOT NULL CHECK (pin_version = 1),
    persistence_profile TEXT NOT NULL CHECK (persistence_profile = 'head_canonical_v1'),
    pinned_at_ms INTEGER NOT NULL CHECK (pinned_at_ms >= 0)
);
CREATE TRIGGER runtime_head_canonical_profile_pin_no_update
BEFORE UPDATE ON runtime_head_canonical_profile_pin
BEGIN
    SELECT RAISE(ABORT, 'runtime head-canonical profile pin is immutable');
END;
CREATE TRIGGER runtime_head_canonical_profile_pin_no_delete
BEFORE DELETE ON runtime_head_canonical_profile_pin
BEGIN
    SELECT RAISE(ABORT, 'runtime head-canonical profile pin is immutable');
END;
CREATE TABLE runtime_head_canonical_activations (
    runtime_id TEXT PRIMARY KEY,
    activation_version INTEGER NOT NULL CHECK (activation_version = 1),
    state TEXT NOT NULL CHECK (state IN ('in_progress', 'complete')),
    session_id TEXT NOT NULL,
    source_snapshot_token TEXT,
    source_snapshot_bytes INTEGER,
    source_message_count INTEGER,
    started_at_ms INTEGER NOT NULL CHECK (started_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= started_at_ms),
    completed_at_ms INTEGER,
    elapsed_ms INTEGER,
    boundary_message_count INTEGER,
    physical_message_count INTEGER,
    boundary_head_cas_token TEXT,
    physical_head_cas_token TEXT,
    CHECK (
        (
            state = 'in_progress'
            AND source_snapshot_token IS NULL
            AND source_snapshot_bytes IS NULL
            AND source_message_count IS NULL
            AND completed_at_ms IS NULL
            AND elapsed_ms IS NULL
            AND boundary_message_count IS NULL
            AND physical_message_count IS NULL
            AND boundary_head_cas_token IS NULL
            AND physical_head_cas_token IS NULL
        )
        OR
        (
            state = 'complete'
            AND source_snapshot_token IS NOT NULL
            AND length(source_snapshot_token) > 0
            AND source_snapshot_bytes IS NOT NULL
            AND source_snapshot_bytes >= 0
            AND source_message_count IS NOT NULL
            AND source_message_count >= 0
            AND completed_at_ms IS NOT NULL
            AND completed_at_ms >= started_at_ms
            AND elapsed_ms IS NOT NULL
            AND elapsed_ms >= 0
            AND boundary_message_count IS NOT NULL
            AND boundary_message_count >= 0
            AND physical_message_count IS NOT NULL
            AND physical_message_count >= boundary_message_count
            AND boundary_head_cas_token IS NOT NULL
            AND length(boundary_head_cas_token) > 0
            AND physical_head_cas_token IS NOT NULL
            AND length(physical_head_cas_token) > 0
        )
    )
);
CREATE TRIGGER runtime_session_snapshots_hc_activation_no_insert
BEFORE INSERT ON runtime_session_snapshots
WHEN EXISTS (
    SELECT 1 FROM runtime_head_canonical_profile_pin
    WHERE singleton = 1
)
OR EXISTS (
    SELECT 1 FROM runtime_head_canonical_activations
    WHERE runtime_id = NEW.runtime_id
)
BEGIN
    SELECT RAISE(ABORT, 'runtime session snapshot is frozen by head-canonical authority');
END;
CREATE TRIGGER runtime_session_snapshots_hc_activation_no_update
BEFORE UPDATE ON runtime_session_snapshots
WHEN EXISTS (
        SELECT 1 FROM runtime_head_canonical_profile_pin
        WHERE singleton = 1
    )
    OR EXISTS (
        SELECT 1 FROM runtime_head_canonical_activations
        WHERE runtime_id = OLD.runtime_id
    )
    OR EXISTS (
        SELECT 1 FROM runtime_head_canonical_activations
        WHERE runtime_id = NEW.runtime_id
    )
BEGIN
    SELECT RAISE(ABORT, 'runtime session snapshot is frozen by head-canonical authority');
END;
CREATE TRIGGER runtime_session_snapshots_hc_activation_no_delete
BEFORE DELETE ON runtime_session_snapshots
WHEN EXISTS (
    SELECT 1 FROM runtime_head_canonical_profile_pin
    WHERE singleton = 1
)
OR EXISTS (
    SELECT 1 FROM runtime_head_canonical_activations
    WHERE runtime_id = OLD.runtime_id
)
BEGIN
    SELECT RAISE(ABORT, 'runtime session snapshot is frozen by head-canonical authority');
END";

    /// Single definition shared by the partial index and every exact witness
    /// read. Keeping classification in one predicate prevents a future phase
    /// addition from making the indexed set and transactional recheck diverge.
    const RECOVERY_NONTERMINAL_INPUT_PREDICATE_SQL: &str = r"
CASE
    WHEN json_valid(state_json) THEN COALESCE(
        json_extract(state_json, '$.current_state') NOT IN (
            'consumed',
            'superseded',
            'coalesced',
            'abandoned'
        ),
        1
    )
    ELSE 1
END";

    const CREATE_RUNTIME_INPUT_SET_REVISION_SCHEMA_SQL: &str = r"
CREATE TABLE runtime_input_set_revisions (
    runtime_id TEXT PRIMARY KEY,
    revision INTEGER NOT NULL
        CHECK (
            typeof(revision) = 'integer'
            AND revision >= 1
            AND revision <= 9223372036854775807
        )
);

INSERT OR IGNORE INTO runtime_input_set_revisions (runtime_id, revision)
SELECT DISTINCT runtime_id, 1
FROM runtime_input_states;

CREATE TRIGGER runtime_input_states_revision_after_insert
AFTER INSERT ON runtime_input_states
BEGIN
    INSERT INTO runtime_input_set_revisions (runtime_id, revision)
    VALUES (NEW.runtime_id, 1)
    ON CONFLICT(runtime_id) DO UPDATE SET revision = revision + 1;
END;

CREATE TRIGGER runtime_input_states_revision_after_update_old
AFTER UPDATE OF runtime_id, input_id, state_json ON runtime_input_states
BEGIN
    INSERT INTO runtime_input_set_revisions (runtime_id, revision)
    VALUES (OLD.runtime_id, 1)
    ON CONFLICT(runtime_id) DO UPDATE SET revision = revision + 1;
END;

CREATE TRIGGER runtime_input_states_revision_after_update_new
AFTER UPDATE OF runtime_id, input_id, state_json ON runtime_input_states
WHEN NEW.runtime_id <> OLD.runtime_id
BEGIN
    INSERT INTO runtime_input_set_revisions (runtime_id, revision)
    VALUES (NEW.runtime_id, 1)
    ON CONFLICT(runtime_id) DO UPDATE SET revision = revision + 1;
END;

CREATE TRIGGER runtime_input_states_revision_after_delete
AFTER DELETE ON runtime_input_states
BEGIN
    INSERT INTO runtime_input_set_revisions (runtime_id, revision)
    VALUES (OLD.runtime_id, 1)
    ON CONFLICT(runtime_id) DO UPDATE SET revision = revision + 1;
END;

CREATE TABLE runtime_input_idempotency_keys (
    runtime_id TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    input_id TEXT NOT NULL,
    PRIMARY KEY (runtime_id, idempotency_key),
    UNIQUE (runtime_id, input_id)
);

INSERT INTO runtime_input_idempotency_keys
    (runtime_id, idempotency_key, input_id)
SELECT
    runtime_id,
    json_extract(state_json, '$.idempotency_key'),
    input_id
FROM runtime_input_states
WHERE CASE
    WHEN json_valid(state_json)
    THEN json_type(state_json, '$.idempotency_key') = 'text'
    ELSE 0
END;

CREATE TRIGGER runtime_input_idempotency_after_insert
AFTER INSERT ON runtime_input_states
WHEN CASE
    WHEN json_valid(NEW.state_json)
    THEN json_type(NEW.state_json, '$.idempotency_key') = 'text'
    ELSE 0
END
BEGIN
    INSERT INTO runtime_input_idempotency_keys
        (runtime_id, idempotency_key, input_id)
    VALUES (
        NEW.runtime_id,
        json_extract(NEW.state_json, '$.idempotency_key'),
        NEW.input_id
    );
END;

CREATE TRIGGER runtime_input_idempotency_after_update
AFTER UPDATE OF runtime_id, input_id, state_json ON runtime_input_states
BEGIN
    DELETE FROM runtime_input_idempotency_keys
    WHERE runtime_id = OLD.runtime_id AND input_id = OLD.input_id;

    INSERT INTO runtime_input_idempotency_keys
        (runtime_id, idempotency_key, input_id)
    SELECT
        NEW.runtime_id,
        json_extract(NEW.state_json, '$.idempotency_key'),
        NEW.input_id
    WHERE CASE
        WHEN json_valid(NEW.state_json)
        THEN json_type(NEW.state_json, '$.idempotency_key') = 'text'
        ELSE 0
    END;
END;

CREATE TRIGGER runtime_input_idempotency_after_delete
AFTER DELETE ON runtime_input_states
BEGIN
    DELETE FROM runtime_input_idempotency_keys
    WHERE runtime_id = OLD.runtime_id AND input_id = OLD.input_id;
END";

    const CREATE_RUNTIME_INPUT_IDEMPOTENCY_UNINDEXABLE_SCHEMA_SQL: &str = r"
CREATE TABLE runtime_input_idempotency_unindexable_rows (
    runtime_id TEXT NOT NULL,
    input_id TEXT NOT NULL,
    reason TEXT NOT NULL CHECK (length(reason) > 0),
    PRIMARY KEY (runtime_id, input_id)
);

INSERT INTO runtime_input_idempotency_unindexable_rows
    (runtime_id, input_id, reason)
SELECT
    runtime_id,
    input_id,
    CASE
        WHEN NOT json_valid(state_json)
        THEN 'state_json is not valid JSON'
        WHEN (
            SELECT COUNT(*)
            FROM json_each(state_json) AS member
            WHERE member.key = 'idempotency_key'
        ) > 1
        THEN 'idempotency_key appears more than once'
        ELSE 'idempotency_key is not text'
    END
FROM runtime_input_states
WHERE CASE
    WHEN json_valid(state_json)
    THEN
        (
            SELECT COUNT(*)
            FROM json_each(state_json) AS member
            WHERE member.key = 'idempotency_key'
        ) > 1
        OR (
            json_type(state_json, '$.idempotency_key') IS NOT NULL
            AND json_type(state_json, '$.idempotency_key') NOT IN ('null', 'text')
        )
    ELSE 1
END;

CREATE TRIGGER runtime_input_idempotency_unindexable_after_insert
AFTER INSERT ON runtime_input_states
WHEN CASE
    WHEN json_valid(NEW.state_json)
    THEN
        (
            SELECT COUNT(*)
            FROM json_each(NEW.state_json) AS member
            WHERE member.key = 'idempotency_key'
        ) > 1
        OR (
            json_type(NEW.state_json, '$.idempotency_key') IS NOT NULL
            AND json_type(NEW.state_json, '$.idempotency_key') NOT IN ('null', 'text')
        )
    ELSE 1
END
BEGIN
    INSERT OR REPLACE INTO runtime_input_idempotency_unindexable_rows
        (runtime_id, input_id, reason)
    VALUES (
        NEW.runtime_id,
        NEW.input_id,
        CASE
            WHEN NOT json_valid(NEW.state_json)
            THEN 'state_json is not valid JSON'
            WHEN (
                SELECT COUNT(*)
                FROM json_each(NEW.state_json) AS member
                WHERE member.key = 'idempotency_key'
            ) > 1
            THEN 'idempotency_key appears more than once'
            ELSE 'idempotency_key is not text'
        END
    );
END;

CREATE TRIGGER runtime_input_idempotency_unindexable_after_update
AFTER UPDATE OF runtime_id, input_id, state_json ON runtime_input_states
BEGIN
    DELETE FROM runtime_input_idempotency_unindexable_rows
    WHERE runtime_id = OLD.runtime_id AND input_id = OLD.input_id;

    INSERT INTO runtime_input_idempotency_unindexable_rows
        (runtime_id, input_id, reason)
    SELECT
        NEW.runtime_id,
        NEW.input_id,
        CASE
            WHEN NOT json_valid(NEW.state_json)
            THEN 'state_json is not valid JSON'
            WHEN (
                SELECT COUNT(*)
                FROM json_each(NEW.state_json) AS member
                WHERE member.key = 'idempotency_key'
            ) > 1
            THEN 'idempotency_key appears more than once'
            ELSE 'idempotency_key is not text'
        END
    WHERE CASE
        WHEN json_valid(NEW.state_json)
        THEN
            (
                SELECT COUNT(*)
                FROM json_each(NEW.state_json) AS member
                WHERE member.key = 'idempotency_key'
            ) > 1
            OR (
                json_type(NEW.state_json, '$.idempotency_key') IS NOT NULL
                AND json_type(NEW.state_json, '$.idempotency_key') NOT IN ('null', 'text')
            )
        ELSE 1
    END;
END;

CREATE TRIGGER runtime_input_idempotency_unindexable_after_delete
AFTER DELETE ON runtime_input_states
BEGIN
    DELETE FROM runtime_input_idempotency_unindexable_rows
    WHERE runtime_id = OLD.runtime_id AND input_id = OLD.input_id;
END";

    const CREATE_RUNTIME_HEAD_CANONICAL_ACTIVATION_QUEUE_SCHEMA_SQL: &str = r"
CREATE TABLE runtime_head_canonical_activation_queue (
    runtime_id TEXT PRIMARY KEY
);

INSERT OR IGNORE INTO runtime_head_canonical_activation_queue (runtime_id)
SELECT whole_blob.runtime_id
FROM runtime_whole_blob_authority AS whole_blob
WHERE NOT EXISTS (
    SELECT 1
    FROM runtime_session_authority AS authority
    WHERE authority.runtime_id = whole_blob.runtime_id
)
  AND NOT EXISTS (
    SELECT 1
    FROM runtime_head_canonical_activations AS activation
    WHERE activation.runtime_id = whole_blob.runtime_id
      AND activation.state = 'in_progress'
);

CREATE TRIGGER runtime_whole_blob_authority_activation_queue_after_insert
AFTER INSERT ON runtime_whole_blob_authority
WHEN NOT EXISTS (
    SELECT 1
    FROM runtime_session_authority AS authority
    WHERE authority.runtime_id = NEW.runtime_id
)
AND NOT EXISTS (
    SELECT 1
    FROM runtime_head_canonical_activations AS activation
    WHERE activation.runtime_id = NEW.runtime_id
      AND activation.state = 'in_progress'
)
BEGIN
    INSERT OR IGNORE INTO runtime_head_canonical_activation_queue (runtime_id)
    VALUES (NEW.runtime_id);
END;

CREATE TRIGGER runtime_whole_blob_authority_activation_queue_after_update
AFTER UPDATE OF runtime_id ON runtime_whole_blob_authority
BEGIN
    DELETE FROM runtime_head_canonical_activation_queue
    WHERE runtime_id = OLD.runtime_id;

    INSERT OR IGNORE INTO runtime_head_canonical_activation_queue (runtime_id)
    SELECT NEW.runtime_id
    WHERE NOT EXISTS (
        SELECT 1
        FROM runtime_session_authority AS authority
        WHERE authority.runtime_id = NEW.runtime_id
    )
      AND NOT EXISTS (
        SELECT 1
        FROM runtime_head_canonical_activations AS activation
        WHERE activation.runtime_id = NEW.runtime_id
          AND activation.state = 'in_progress'
    );
END;

CREATE TRIGGER runtime_whole_blob_authority_activation_queue_after_delete
AFTER DELETE ON runtime_whole_blob_authority
BEGIN
    DELETE FROM runtime_head_canonical_activation_queue
    WHERE runtime_id = OLD.runtime_id;
END;

CREATE TRIGGER runtime_session_authority_activation_queue_after_insert
AFTER INSERT ON runtime_session_authority
BEGIN
    DELETE FROM runtime_head_canonical_activation_queue
    WHERE runtime_id = NEW.runtime_id;
END;

CREATE TRIGGER runtime_session_authority_activation_queue_after_update
AFTER UPDATE OF runtime_id ON runtime_session_authority
BEGIN
    DELETE FROM runtime_head_canonical_activation_queue
    WHERE runtime_id IN (OLD.runtime_id, NEW.runtime_id);
END;

CREATE TRIGGER runtime_head_canonical_activations_queue_after_complete_insert
AFTER INSERT ON runtime_head_canonical_activations
WHEN NEW.state = 'complete'
AND EXISTS (
    SELECT 1
    FROM runtime_session_authority AS authority
    WHERE authority.runtime_id = NEW.runtime_id
)
BEGIN
    DELETE FROM runtime_head_canonical_activation_queue
    WHERE runtime_id = NEW.runtime_id;
END;

CREATE TRIGGER runtime_head_canonical_activations_queue_after_complete_update
AFTER UPDATE OF state ON runtime_head_canonical_activations
WHEN NEW.state = 'complete'
AND EXISTS (
    SELECT 1
    FROM runtime_session_authority AS authority
    WHERE authority.runtime_id = NEW.runtime_id
)
BEGIN
    DELETE FROM runtime_head_canonical_activation_queue
    WHERE runtime_id = NEW.runtime_id;
END";

    fn backfill_whole_blob_store_authority(
        tx: &rusqlite::Transaction<'_>,
    ) -> Result<(), rusqlite::Error> {
        let migration_data_error = |runtime_id: &str, detail: String| {
            rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("released WholeBlob row for runtime {runtime_id} {detail}"),
            )))
        };
        let rows = {
            let mut statement = tx.prepare(
                "SELECT runtime_id, session_snapshot FROM runtime_session_snapshots ORDER BY runtime_id",
            )?;
            statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, JsonColumnBytes>(1)?.into_bytes(),
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?
        };
        for (runtime_id, bytes) in rows {
            let imported =
                meerkat_core::import_released_0810_session(&bytes).map_err(|error| {
                    rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!(
                            "released WholeBlob row for runtime {runtime_id} failed strict 0.8.10 import: {error}"
                        ),
                    )))
            })?;
            let (session, receipt) = imported.into_parts();
            let logical_runtime_id = LogicalRuntimeId(runtime_id.clone());
            use sha2::Digest as _;
            let observed_source_sha256: [u8; 32] = sha2::Sha256::digest(&bytes).into();
            if receipt.source_document_sha256() != &observed_source_sha256 {
                return Err(rusqlite::Error::ToSqlConversionFailure(Box::new(
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!(
                            "released WholeBlob row for runtime {runtime_id} changed during exact import"
                        ),
                    ),
                )));
            }
            if LogicalRuntimeId::for_session(receipt.session_id()).0 != runtime_id {
                return Err(rusqlite::Error::ToSqlConversionFailure(Box::new(
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!(
                            "released WholeBlob row {runtime_id} belongs to session {}",
                            receipt.session_id()
                        ),
                    ),
                )));
            }
            let artifact = session.to_persisted_artifact().map_err(|error| {
                rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "released WholeBlob row for runtime {runtime_id} failed current representation encoding: {error}"
                    ),
                )))
            })?;
            let blob_sha256 = artifact.row_sha256_token().to_string();
            let catalog_entry = crate::store::RuntimeSessionCatalogEntry::from_session(
                &session,
                RuntimeSessionPersistenceProfile::WholeBlobV1,
                None,
            )
            .map_err(|error| {
                migration_data_error(
                    &runtime_id,
                    format!("failed current catalog projection: {error}"),
                )
            })?;
            tx.execute(
                r"
                INSERT INTO runtime_whole_blob_bodies (blob_sha256, session_snapshot)
                VALUES (?1, ?2)
                ",
                params![&blob_sha256, artifact.bytes()],
            )?;
            tx.execute(
                r"
                INSERT INTO runtime_whole_blob_authority
                    (runtime_id, authority_version, session_id, store_revision, blob_sha256)
                VALUES (?1, 1, ?2, 1, ?3)
                ",
                params![&runtime_id, receipt.session_id().to_string(), &blob_sha256],
            )?;
            upsert_runtime_session_catalog_entry_in_txn(tx, &logical_runtime_id, &catalog_entry)
                .map_err(|error| {
                    migration_data_error(
                        &runtime_id,
                        format!("failed atomic catalog adoption: {error}"),
                    )
                })?;
        }
        Ok(())
    }

    /// Retire 0.8.10 ingress payloads whose released machine seed is already
    /// terminal and whose row carries no unfinished completion/publication
    /// obligation.
    ///
    /// This belongs to the exact v1 -> v2 activation importer, not ordinary
    /// startup recovery: closed terminal rows are intentionally outside the
    /// live recovery index, so scanning them on every boot would reintroduce
    /// O(history) work. The generated seed authorizer is run before each
    /// current replacement is encoded, and the update is exact-byte fenced
    /// inside the same migration transaction. Directed rows without an outbox,
    /// malformed directed carriers, and pending terminal sagas retain content.
    fn retire_released_terminal_input_payloads(
        tx: &rusqlite::Transaction<'_>,
    ) -> Result<(), rusqlite::Error> {
        let migration_data_error = |runtime_id: &str, input_id: &str, detail: String| {
            rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "released runtime input {runtime_id}/{input_id} failed terminal payload import: {detail}"
                ),
            )))
        };
        let rows = {
            let mut statement = tx.prepare(
                "SELECT runtime_id, input_id, state_json
                 FROM runtime_input_states
                 WHERE json_valid(state_json)
                   AND json_extract(state_json, '$.current_state')
                       IN ('consumed', 'superseded', 'coalesced', 'abandoned')
                 ORDER BY runtime_id, input_id",
            )?;
            statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, JsonColumnBytes>(2)?.into_bytes(),
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?
        };
        for (runtime_id, input_id, source_bytes) in rows {
            let mut stored: StoredInputState = serde_json::from_slice(&source_bytes)
                .map_err(|error| migration_data_error(&runtime_id, &input_id, error.to_string()))?;
            if stored.state.persisted_input.is_none()
                || !crate::store::input_state_payload_is_retirable(&stored)
            {
                continue;
            }
            stored.state.persisted_input = None;
            let replacement = InputStatePersistenceRecord::from_machine_snapshot(stored)
                .map_err(|error| migration_data_error(&runtime_id, &input_id, error))?;
            let replacement_bytes = serde_json::to_vec(replacement.as_stored())
                .map_err(|error| migration_data_error(&runtime_id, &input_id, error.to_string()))?;
            let changed = tx.execute(
                "UPDATE runtime_input_states
                 SET state_json = ?1
                 WHERE runtime_id = ?2 AND input_id = ?3 AND state_json = ?4",
                params![replacement_bytes, &runtime_id, &input_id, &source_bytes],
            )?;
            if changed != 1 {
                return Err(migration_data_error(
                    &runtime_id,
                    &input_id,
                    "source row changed inside the activation transaction".to_string(),
                ));
            }
        }
        Ok(())
    }

    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct FrozenConversationContextAppend0810 {
        key: String,
        content: meerkat_core::lifecycle::run_primitive::CoreRenderable,
    }

    fn render_released_runtime_context_0810(
        append: &FrozenConversationContextAppend0810,
    ) -> String {
        let mut rendered = String::from("[Runtime System Context]\nsource: ");
        rendered.push_str(&append.key);
        rendered.push_str("\n\n");
        rendered.push_str(append.content.render_text().trim());
        rendered
    }

    /// Convert the exact released continuation sidecar carrier before current
    /// `Input` deserialization can discard its now-unknown field.
    ///
    /// The raw input row remains the recovery/idempotency owner. A typed Steer
    /// becomes an ordinary User turn append: live realization projects it as
    /// request-only context and suppresses transcript persistence, while the
    /// machine's idle Queue normalization persists it as an ordinary User.
    /// Instruction-like continuations become ordinary ordered System appends
    /// using the frozen 0.8.10 visible rendering. Existing `turn_append`
    /// already owns the current representation and wins unchanged.
    fn migrate_released_context_append_input_payloads(
        tx: &rusqlite::Transaction<'_>,
    ) -> Result<(), rusqlite::Error> {
        let migration_data_error = |runtime_id: &str, input_id: &str, detail: String| {
            rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "released runtime input {runtime_id}/{input_id} failed context-append import: {detail}"
                ),
            )))
        };
        let rows = {
            let mut statement = tx.prepare(
                "SELECT runtime_id, input_id, state_json
                 FROM runtime_input_states
                 WHERE json_valid(state_json)
                   AND json_type(state_json, '$.persisted_input.context_append') IS NOT NULL
                 ORDER BY runtime_id, input_id",
            )?;
            statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, JsonColumnBytes>(2)?.into_bytes(),
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?
        };
        for (runtime_id, input_id, source_bytes) in rows {
            let mut encoded: serde_json::Value = serde_json::from_slice(&source_bytes)
                .map_err(|error| migration_data_error(&runtime_id, &input_id, error.to_string()))?;
            let row = encoded.as_object_mut().ok_or_else(|| {
                migration_data_error(
                    &runtime_id,
                    &input_id,
                    "input-state row is not a JSON object".to_string(),
                )
            })?;
            let semantics = row
                .get("runtime_semantics")
                .cloned()
                .ok_or_else(|| {
                    migration_data_error(
                        &runtime_id,
                        &input_id,
                        "context append has no released runtime semantics".to_string(),
                    )
                })
                .and_then(|value| {
                    serde_json::from_value::<crate::ingress_types::RuntimeInputSemantics>(value)
                        .map_err(|error| {
                            migration_data_error(&runtime_id, &input_id, error.to_string())
                        })
                })?;
            let input = row
                .get_mut("persisted_input")
                .and_then(serde_json::Value::as_object_mut)
                .ok_or_else(|| {
                    migration_data_error(
                        &runtime_id,
                        &input_id,
                        "context append has no persisted input object".to_string(),
                    )
                })?;
            if input.get("input_type").and_then(serde_json::Value::as_str) != Some("continuation") {
                return Err(migration_data_error(
                    &runtime_id,
                    &input_id,
                    "context append belongs to a non-continuation input".to_string(),
                ));
            }
            let frozen = input.remove("context_append").ok_or_else(|| {
                migration_data_error(
                    &runtime_id,
                    &input_id,
                    "selected row lost its context append".to_string(),
                )
            })?;
            if frozen.is_null() {
                // Null means absent in the released shape.
            } else {
                let frozen: FrozenConversationContextAppend0810 = serde_json::from_value(frozen)
                    .map_err(|error| {
                        migration_data_error(&runtime_id, &input_id, error.to_string())
                    })?;
                if input
                    .get("turn_append")
                    .is_none_or(serde_json::Value::is_null)
                {
                    let is_steer = semantics.live_interrupt_required
                        && semantics.peer_response_terminal_apply_intent.is_none();
                    let (role, text) = if is_steer {
                        ("user", frozen.content.render_text())
                    } else {
                        ("system", render_released_runtime_context_0810(&frozen))
                    };
                    let mut append = serde_json::json!({
                        "role": role,
                        "content": {
                            "type": "text",
                            "text": text,
                        },
                    });
                    if !is_steer {
                        let identity_key = frozen.key;
                        append["identity"] = serde_json::json!({
                            "source": identity_key.clone(),
                            "idempotency_key": identity_key,
                        });
                    }
                    input.insert("turn_append".to_string(), append);
                }
            }
            let stored: StoredInputState = serde_json::from_value(encoded)
                .map_err(|error| migration_data_error(&runtime_id, &input_id, error.to_string()))?;
            let replacement = InputStatePersistenceRecord::from_machine_snapshot(stored)
                .map_err(|error| migration_data_error(&runtime_id, &input_id, error))?;
            let replacement_bytes = serde_json::to_vec(replacement.as_stored())
                .map_err(|error| migration_data_error(&runtime_id, &input_id, error.to_string()))?;
            let changed = tx.execute(
                "UPDATE runtime_input_states
                 SET state_json = ?1
                 WHERE runtime_id = ?2 AND input_id = ?3 AND state_json = ?4",
                params![replacement_bytes, &runtime_id, &input_id, &source_bytes],
            )?;
            if changed != 1 {
                return Err(migration_data_error(
                    &runtime_id,
                    &input_id,
                    "source row changed inside the activation transaction".to_string(),
                ));
            }
        }
        Ok(())
    }

    fn migration_0002_current_runtime_schema(
        tx: &rusqlite::Transaction<'_>,
    ) -> Result<(), rusqlite::Error> {
        migrate_released_context_append_input_payloads(tx)?;
        retire_released_terminal_input_payloads(tx)?;
        tx.execute_batch(CREATE_RUNTIME_SESSION_AUTHORITY_SQL)?;
        tx.execute_batch(CREATE_RUNTIME_WHOLE_BLOB_AUTHORITY_SQL)?;
        backfill_whole_blob_store_authority(tx)?;
        tx.execute_batch(CREATE_RUNTIME_RECOVERY_BOUNDARY_SCHEMA_SQL)?;
        tx.execute_batch(CREATE_RUNTIME_ORDINARY_BOUNDARY_WITNESS_SCHEMA_SQL)?;
        tx.execute_batch(CREATE_RELEASED_0810_BOUNDARY_RECEIPT_MARKERS_SQL)?;
        capture_released_0810_boundary_receipt_markers(tx)?;
        tx.execute_batch(CREATE_RUNTIME_PENDING_TERMINAL_OWNER_SCHEMA_SQL)?;
        tx.execute_batch(CREATE_RUNTIME_HEAD_CANONICAL_ACTIVATION_SCHEMA_SQL)?;
        tx.execute_batch(&format!(
            r"
            CREATE INDEX idx_runtime_input_states_recovery_nonterminal
            ON runtime_input_states (runtime_id, input_id)
            WHERE {RECOVERY_NONTERMINAL_INPUT_PREDICATE_SQL}
            "
        ))?;
        tx.execute_batch(CREATE_RUNTIME_INPUT_SET_REVISION_SCHEMA_SQL)?;
        tx.execute_batch(CREATE_RUNTIME_INPUT_IDEMPOTENCY_UNINDEXABLE_SCHEMA_SQL)?;
        tx.execute_batch(CREATE_RUNTIME_HEAD_CANONICAL_ACTIVATION_QUEUE_SCHEMA_SQL)
    }

    fn initialize_current_runtime_schema(
        tx: &rusqlite::Transaction<'_>,
    ) -> Result<(), rusqlite::Error> {
        migration_0001_runtime_schema(tx)?;
        migration_0002_current_runtime_schema(tx)
    }

    const RELEASED_0_8_10_RUNTIME_OBJECTS: &[meerkat_sqlite::SchemaObject] = &[
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Table,
            name: "runtime_input_states",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Table,
            name: "runtime_boundary_receipts",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Table,
            name: "runtime_session_snapshots",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Table,
            name: "runtime_states",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Table,
            name: "runtime_ops_lifecycle",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Table,
            name: "runtime_retired_ops_epochs",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Table,
            name: "runtime_auth_oauth_flow_state",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Table,
            name: "runtime_projection_quarantine",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Table,
            name: "runtime_compaction_projection_outbox",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Table,
            name: "runtime_mob_host_bindings",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Table,
            name: "runtime_mob_host_revocations",
        },
    ];

    fn verify_released_0_8_10_runtime_schema(conn: &Connection) -> Result<(), String> {
        meerkat_sqlite::verify_released_schema_fingerprint(
            conn,
            &RUNTIME_STORE_DOMAIN,
            RELEASED_0_8_10_RUNTIME_OBJECTS,
            migration_0001_runtime_schema,
        )
    }

    /// The runtime store's schema domain in the per-file migration ledger.
    /// (Co-tenants the sessions file in the sqlite realm backend.)
    ///
    /// Version 1 is the complete schema published by 0.8.10. Version 2 is the
    /// single 0.8.11 upgrade: it installs the current head-canonical authority,
    /// exact boundary and recovery witnesses, a one-time allowlist of exact
    /// released boundary receipts, bounded recovery indexes, idempotency
    /// evidence, terminal-payload retirement, and the marker-first
    /// profile-activation state machine. Its backfills read only version-1
    /// tables. Whole-BLOB runtimes are materialized once into the activation
    /// queue; ordinary reopen then reads only pending work and interrupted
    /// activations.
    /// This intentionally makes older binaries refuse the file: they do not
    /// understand the authority split and must never resume writing the frozen
    /// BLOB after a head-canonical boundary has committed.
    pub const RUNTIME_STORE_DOMAIN: meerkat_sqlite::SchemaDomain = meerkat_sqlite::SchemaDomain {
        name: "runtime-store",
        migrations: &[
            meerkat_sqlite::Migration {
                version: 1,
                name: "base-schema",
                apply: migration_0001_runtime_schema,
            },
            meerkat_sqlite::Migration {
                version: 2,
                name: "current-runtime-schema",
                apply: migration_0002_current_runtime_schema,
            },
        ],
        initialize_current: initialize_current_runtime_schema,
        allowed_existing_versions: &[1, 2],
        released_predecessors: &[meerkat_sqlite::SchemaPredecessor {
            version: 1,
            verify: verify_released_0_8_10_runtime_schema,
        }],
        owned_objects: &[
            meerkat_sqlite::SchemaObject {
                kind: meerkat_sqlite::SchemaObjectKind::Table,
                name: "runtime_input_states",
            },
            meerkat_sqlite::SchemaObject {
                kind: meerkat_sqlite::SchemaObjectKind::Table,
                name: "runtime_boundary_receipts",
            },
            meerkat_sqlite::SchemaObject {
                kind: meerkat_sqlite::SchemaObjectKind::Table,
                name: "runtime_session_snapshots",
            },
            meerkat_sqlite::SchemaObject {
                kind: meerkat_sqlite::SchemaObjectKind::Table,
                name: "runtime_whole_blob_authority",
            },
            meerkat_sqlite::SchemaObject {
                kind: meerkat_sqlite::SchemaObjectKind::Table,
                name: "runtime_whole_blob_bodies",
            },
            meerkat_sqlite::SchemaObject {
                kind: meerkat_sqlite::SchemaObjectKind::Table,
                name: "runtime_whole_blob_provisional_tails",
            },
            meerkat_sqlite::SchemaObject {
                kind: meerkat_sqlite::SchemaObjectKind::Table,
                name: "runtime_session_catalog",
            },
            meerkat_sqlite::SchemaObject {
                kind: meerkat_sqlite::SchemaObjectKind::Index,
                name: "runtime_session_catalog_updated",
            },
            meerkat_sqlite::SchemaObject {
                kind: meerkat_sqlite::SchemaObjectKind::Table,
                name: "runtime_states",
            },
            meerkat_sqlite::SchemaObject {
                kind: meerkat_sqlite::SchemaObjectKind::Table,
                name: "runtime_ops_lifecycle",
            },
            meerkat_sqlite::SchemaObject {
                kind: meerkat_sqlite::SchemaObjectKind::Table,
                name: "runtime_retired_ops_epochs",
            },
            meerkat_sqlite::SchemaObject {
                kind: meerkat_sqlite::SchemaObjectKind::Table,
                name: "runtime_auth_oauth_flow_state",
            },
            meerkat_sqlite::SchemaObject {
                kind: meerkat_sqlite::SchemaObjectKind::Table,
                name: "runtime_projection_quarantine",
            },
            meerkat_sqlite::SchemaObject {
                kind: meerkat_sqlite::SchemaObjectKind::Table,
                name: "runtime_compaction_projection_outbox",
            },
            meerkat_sqlite::SchemaObject {
                kind: meerkat_sqlite::SchemaObjectKind::Table,
                name: "runtime_mob_host_bindings",
            },
            meerkat_sqlite::SchemaObject {
                kind: meerkat_sqlite::SchemaObjectKind::Table,
                name: "runtime_mob_host_revocations",
            },
            meerkat_sqlite::SchemaObject {
                kind: meerkat_sqlite::SchemaObjectKind::Table,
                name: "runtime_session_authority",
            },
            meerkat_sqlite::SchemaObject {
                kind: meerkat_sqlite::SchemaObjectKind::Table,
                name: "runtime_head_canonical_provisional_tails",
            },
            meerkat_sqlite::SchemaObject {
                kind: meerkat_sqlite::SchemaObjectKind::Trigger,
                name: "runtime_session_authority_after_delete",
            },
            meerkat_sqlite::SchemaObject {
                kind: meerkat_sqlite::SchemaObjectKind::Table,
                name: "runtime_recovery_boundaries",
            },
            meerkat_sqlite::SchemaObject {
                kind: meerkat_sqlite::SchemaObjectKind::Table,
                name: "runtime_session_boundary_witnesses",
            },
            meerkat_sqlite::SchemaObject {
                kind: meerkat_sqlite::SchemaObjectKind::Table,
                name: "runtime_released_0810_boundary_receipts",
            },
            meerkat_sqlite::SchemaObject {
                kind: meerkat_sqlite::SchemaObjectKind::Table,
                name: "runtime_pending_terminal_owners",
            },
            meerkat_sqlite::SchemaObject {
                kind: meerkat_sqlite::SchemaObjectKind::Table,
                name: "runtime_head_canonical_profile_pin",
            },
            meerkat_sqlite::SchemaObject {
                kind: meerkat_sqlite::SchemaObjectKind::Table,
                name: "runtime_head_canonical_activations",
            },
            meerkat_sqlite::SchemaObject {
                kind: meerkat_sqlite::SchemaObjectKind::Table,
                name: "runtime_input_set_revisions",
            },
            meerkat_sqlite::SchemaObject {
                kind: meerkat_sqlite::SchemaObjectKind::Table,
                name: "runtime_input_idempotency_keys",
            },
            meerkat_sqlite::SchemaObject {
                kind: meerkat_sqlite::SchemaObjectKind::Table,
                name: "runtime_input_idempotency_unindexable_rows",
            },
            meerkat_sqlite::SchemaObject {
                kind: meerkat_sqlite::SchemaObjectKind::Table,
                name: "runtime_head_canonical_activation_queue",
            },
            meerkat_sqlite::SchemaObject {
                kind: meerkat_sqlite::SchemaObjectKind::Index,
                name: "idx_runtime_input_states_recovery_nonterminal",
            },
            meerkat_sqlite::SchemaObject {
                kind: meerkat_sqlite::SchemaObjectKind::Trigger,
                name: "runtime_session_snapshots_head_authority_no_insert",
            },
            meerkat_sqlite::SchemaObject {
                kind: meerkat_sqlite::SchemaObjectKind::Trigger,
                name: "runtime_session_snapshots_head_authority_no_update",
            },
            meerkat_sqlite::SchemaObject {
                kind: meerkat_sqlite::SchemaObjectKind::Trigger,
                name: "runtime_session_snapshots_head_authority_no_delete",
            },
            meerkat_sqlite::SchemaObject {
                kind: meerkat_sqlite::SchemaObjectKind::Trigger,
                name: "runtime_head_canonical_profile_pin_no_update",
            },
            meerkat_sqlite::SchemaObject {
                kind: meerkat_sqlite::SchemaObjectKind::Trigger,
                name: "runtime_head_canonical_profile_pin_no_delete",
            },
            meerkat_sqlite::SchemaObject {
                kind: meerkat_sqlite::SchemaObjectKind::Trigger,
                name: "runtime_session_snapshots_hc_activation_no_insert",
            },
            meerkat_sqlite::SchemaObject {
                kind: meerkat_sqlite::SchemaObjectKind::Trigger,
                name: "runtime_session_snapshots_hc_activation_no_update",
            },
            meerkat_sqlite::SchemaObject {
                kind: meerkat_sqlite::SchemaObjectKind::Trigger,
                name: "runtime_session_snapshots_hc_activation_no_delete",
            },
            meerkat_sqlite::SchemaObject {
                kind: meerkat_sqlite::SchemaObjectKind::Trigger,
                name: "runtime_input_states_revision_after_insert",
            },
            meerkat_sqlite::SchemaObject {
                kind: meerkat_sqlite::SchemaObjectKind::Trigger,
                name: "runtime_input_states_revision_after_update_old",
            },
            meerkat_sqlite::SchemaObject {
                kind: meerkat_sqlite::SchemaObjectKind::Trigger,
                name: "runtime_input_states_revision_after_update_new",
            },
            meerkat_sqlite::SchemaObject {
                kind: meerkat_sqlite::SchemaObjectKind::Trigger,
                name: "runtime_input_states_revision_after_delete",
            },
            meerkat_sqlite::SchemaObject {
                kind: meerkat_sqlite::SchemaObjectKind::Trigger,
                name: "runtime_input_idempotency_after_insert",
            },
            meerkat_sqlite::SchemaObject {
                kind: meerkat_sqlite::SchemaObjectKind::Trigger,
                name: "runtime_input_idempotency_after_update",
            },
            meerkat_sqlite::SchemaObject {
                kind: meerkat_sqlite::SchemaObjectKind::Trigger,
                name: "runtime_input_idempotency_after_delete",
            },
            meerkat_sqlite::SchemaObject {
                kind: meerkat_sqlite::SchemaObjectKind::Trigger,
                name: "runtime_input_idempotency_unindexable_after_insert",
            },
            meerkat_sqlite::SchemaObject {
                kind: meerkat_sqlite::SchemaObjectKind::Trigger,
                name: "runtime_input_idempotency_unindexable_after_update",
            },
            meerkat_sqlite::SchemaObject {
                kind: meerkat_sqlite::SchemaObjectKind::Trigger,
                name: "runtime_input_idempotency_unindexable_after_delete",
            },
            meerkat_sqlite::SchemaObject {
                kind: meerkat_sqlite::SchemaObjectKind::Trigger,
                name: "runtime_whole_blob_authority_activation_queue_after_insert",
            },
            meerkat_sqlite::SchemaObject {
                kind: meerkat_sqlite::SchemaObjectKind::Trigger,
                name: "runtime_whole_blob_authority_activation_queue_after_update",
            },
            meerkat_sqlite::SchemaObject {
                kind: meerkat_sqlite::SchemaObjectKind::Trigger,
                name: "runtime_whole_blob_authority_activation_queue_after_delete",
            },
            meerkat_sqlite::SchemaObject {
                kind: meerkat_sqlite::SchemaObjectKind::Trigger,
                name: "runtime_session_authority_activation_queue_after_insert",
            },
            meerkat_sqlite::SchemaObject {
                kind: meerkat_sqlite::SchemaObjectKind::Trigger,
                name: "runtime_session_authority_activation_queue_after_update",
            },
            meerkat_sqlite::SchemaObject {
                kind: meerkat_sqlite::SchemaObjectKind::Trigger,
                name: "runtime_head_canonical_activations_queue_after_complete_insert",
            },
            meerkat_sqlite::SchemaObject {
                kind: meerkat_sqlite::SchemaObjectKind::Trigger,
                name: "runtime_head_canonical_activations_queue_after_complete_update",
            },
        ],
        retired_objects: &[],
    };

    /// Durable-delivery schema domain (delivery authority + inbox tables).
    ///
    /// Deliberately NOT applied by [`open_runtime_connection`]: it is
    /// provisioned lazily by the first delivery *write*
    /// ([`open_runtime_delivery_write_connection`]). Reads of an unused
    /// feature remain schema-free; only files where durable delivery was
    /// actually used carry the domain row and its owned objects.
    pub const RUNTIME_DELIVERY_DOMAIN: meerkat_sqlite::SchemaDomain =
        meerkat_sqlite::SchemaDomain {
            name: "runtime-delivery",
            migrations: &[meerkat_sqlite::Migration {
                version: 1,
                name: "delivery-inbox",
                apply: migration_0001_runtime_delivery_inbox,
            }],
            initialize_current: migration_0001_runtime_delivery_inbox,
            allowed_existing_versions: &[1],
            released_predecessors: &[],
            owned_objects: &[
                meerkat_sqlite::SchemaObject {
                    kind: meerkat_sqlite::SchemaObjectKind::Table,
                    name: "runtime_delivery_authority",
                },
                meerkat_sqlite::SchemaObject {
                    kind: meerkat_sqlite::SchemaObjectKind::Table,
                    name: "runtime_delivery_inbox",
                },
                meerkat_sqlite::SchemaObject {
                    kind: meerkat_sqlite::SchemaObjectKind::Index,
                    name: "idx_runtime_delivery_inbox_sequence",
                },
            ],
            retired_objects: &[],
        };

    #[cfg(test)]
    mod schema_floor_tests {
        use super::*;

        fn released_v1() -> Connection {
            let mut conn = Connection::open_in_memory().expect("open");
            let tx = conn.transaction().expect("tx");
            migration_0001_runtime_schema(&tx).expect("released schema");
            tx.commit().expect("commit");
            conn.execute_batch(
                "CREATE TABLE meerkat_schema (
                     domain TEXT PRIMARY KEY,
                     version INTEGER NOT NULL
                 );
                 INSERT INTO meerkat_schema VALUES ('runtime-store', 1);",
            )
            .expect("ledger");
            conn
        }

        #[test]
        fn exact_released_v1_upgrades_to_current() {
            let mut conn = released_v1();
            let report = meerkat_sqlite::apply_domain_migrations(&mut conn, &RUNTIME_STORE_DOMAIN)
                .expect("upgrade");
            assert_eq!(report.from_version, 1);
            assert_eq!(report.to_version, 2);
        }

        #[test]
        fn released_v1_final_table_index_and_trigger_collisions_are_refused_unmutated() {
            for collision in [
                "CREATE TABLE runtime_session_authority (candidate INTEGER)",
                "CREATE INDEX idx_runtime_input_states_recovery_nonterminal
                     ON runtime_input_states(runtime_id)",
                "CREATE TRIGGER runtime_session_snapshots_head_authority_no_insert
                     BEFORE INSERT ON runtime_session_snapshots BEGIN SELECT 1; END",
            ] {
                let mut conn = released_v1();
                conn.execute_batch(collision).expect("collision");
                let err = meerkat_sqlite::apply_domain_migrations(&mut conn, &RUNTIME_STORE_DOMAIN)
                    .expect_err("refuse collision");
                assert!(matches!(
                    err,
                    meerkat_sqlite::SqliteStoreError::SchemaFingerprintMismatch { version: 1, .. }
                ));
                assert_eq!(
                    meerkat_sqlite::domain_version(&conn, RUNTIME_STORE_DOMAIN.name)
                        .expect("ledger"),
                    Some(1)
                );
            }
        }
    }

    fn map_shared_sqlite_error(err: meerkat_sqlite::SqliteStoreError) -> RuntimeStoreError {
        match err {
            meerkat_sqlite::SqliteStoreError::SchemaFromTheFuture {
                domain,
                found,
                supported,
            } => RuntimeStoreError::SchemaFromTheFuture {
                domain,
                found,
                supported,
            },
            meerkat_sqlite::SqliteStoreError::MaintenanceFenceHeld { path } => {
                RuntimeStoreError::MaintenanceFenceHeld {
                    path: path.display().to_string(),
                }
            }
            other => RuntimeStoreError::WriteFailed(other.to_string()),
        }
    }

    /// Per-operation connection: fence guard lives exactly as long as the
    /// connection it admits.
    struct RuntimeConn {
        conn: Connection,
        _guard: meerkat_sqlite::OperationGuard,
    }

    impl std::ops::Deref for RuntimeConn {
        type Target = Connection;
        fn deref(&self) -> &Connection {
            &self.conn
        }
    }

    impl std::ops::DerefMut for RuntimeConn {
        fn deref_mut(&mut self) -> &mut Connection {
            &mut self.conn
        }
    }

    fn open_runtime_connection(path: &Path) -> Result<RuntimeConn, RuntimeStoreError> {
        let guard =
            meerkat_sqlite::OperationGuard::for_database(path).map_err(map_shared_sqlite_error)?;
        let mut conn = meerkat_sqlite::open_with(
            path,
            meerkat_sqlite::ConnectionProfile::PRIMARY,
            meerkat_sqlite::OpenOptions {
                // The runtime store preflights its own domain (not its
                // co-tenants'): an ineligible runtime-store file is refused
                // typed before the Primary profile's WAL conversion.
                schema_preflight: &[&RUNTIME_STORE_DOMAIN],
                ..meerkat_sqlite::OpenOptions::default()
            },
        )
        .map_err(map_shared_sqlite_error)?;
        meerkat_sqlite::apply_domain_migrations(&mut conn, &RUNTIME_STORE_DOMAIN)
            .map_err(map_shared_sqlite_error)?;
        Ok(RuntimeConn {
            conn,
            _guard: guard,
        })
    }

    /// Runtime connection for the combined head-canonical boundary.
    ///
    /// This operation owns rows from both schema domains in one SQLite
    /// transaction, so it must preflight and migrate both before the Primary
    /// profile is allowed to touch WAL. Ordinary RuntimeStore-only operations
    /// continue to use [`open_runtime_connection`] and do not claim ownership
    /// of the co-tenant session schema.
    fn open_head_canonical_runtime_connection(
        path: &Path,
    ) -> Result<RuntimeConn, RuntimeStoreError> {
        let guard =
            meerkat_sqlite::OperationGuard::for_database(path).map_err(map_shared_sqlite_error)?;
        let mut conn = meerkat_sqlite::open_with(
            path,
            meerkat_sqlite::ConnectionProfile::PRIMARY,
            meerkat_sqlite::OpenOptions {
                schema_preflight: &[
                    &RUNTIME_STORE_DOMAIN,
                    &meerkat_store::sqlite_store::SESSION_STORE_DOMAIN,
                ],
                ..meerkat_sqlite::OpenOptions::default()
            },
        )
        .map_err(map_shared_sqlite_error)?;
        meerkat_sqlite::apply_domain_migrations(&mut conn, &RUNTIME_STORE_DOMAIN)
            .map_err(map_shared_sqlite_error)?;
        meerkat_sqlite::apply_domain_migrations(
            &mut conn,
            &meerkat_store::sqlite_store::SESSION_STORE_DOMAIN,
        )
        .map_err(map_shared_sqlite_error)?;
        Ok(RuntimeConn {
            conn,
            _guard: guard,
        })
    }

    /// Delivery-verb connection: preflights only the delivery domain (the
    /// operation reads nothing else) and never provisions it — a read on a
    /// file where durable delivery was never used must not stamp the file.
    /// Returns `Ok(None)` when the domain is absent, which reads as "no
    /// delivery state".
    fn open_runtime_delivery_read_connection(
        path: &Path,
    ) -> Result<Option<RuntimeConn>, RuntimeStoreError> {
        let guard =
            meerkat_sqlite::OperationGuard::for_database(path).map_err(map_shared_sqlite_error)?;
        let conn = meerkat_sqlite::open_with(
            path,
            meerkat_sqlite::ConnectionProfile::PRIMARY,
            meerkat_sqlite::OpenOptions {
                schema_preflight: &[&RUNTIME_DELIVERY_DOMAIN],
                ..meerkat_sqlite::OpenOptions::default()
            },
        )
        .map_err(map_shared_sqlite_error)?;
        let provisioned = meerkat_sqlite::domain_version(&conn, RUNTIME_DELIVERY_DOMAIN.name)
            .map_err(map_shared_sqlite_error)?
            .is_some();
        Ok(provisioned.then_some(RuntimeConn {
            conn,
            _guard: guard,
        }))
    }

    /// Delivery-write connection: provisions the delivery domain on first
    /// use. This is the ONLY place the `runtime-delivery` ledger row is
    /// stamped — actually using durable delivery marks the file, opening a
    /// realm does not.
    fn open_runtime_delivery_write_connection(
        path: &Path,
    ) -> Result<RuntimeConn, RuntimeStoreError> {
        let guard =
            meerkat_sqlite::OperationGuard::for_database(path).map_err(map_shared_sqlite_error)?;
        let mut conn = meerkat_sqlite::open_with(
            path,
            meerkat_sqlite::ConnectionProfile::PRIMARY,
            meerkat_sqlite::OpenOptions {
                schema_preflight: &[&RUNTIME_DELIVERY_DOMAIN],
                ..meerkat_sqlite::OpenOptions::default()
            },
        )
        .map_err(map_shared_sqlite_error)?;
        meerkat_sqlite::apply_domain_migrations(&mut conn, &RUNTIME_DELIVERY_DOMAIN)
            .map_err(map_shared_sqlite_error)?;
        Ok(RuntimeConn {
            conn,
            _guard: guard,
        })
    }

    fn begin_runtime_transaction(
        conn: &mut Connection,
    ) -> Result<Transaction<'_>, RuntimeStoreError> {
        begin_immediate_transaction(conn)
            .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))
    }

    fn runtime_id_text(runtime_id: &LogicalRuntimeId) -> &str {
        &runtime_id.0
    }

    fn session_authority_conflict(
        runtime_id: &LogicalRuntimeId,
        detail: impl Into<String>,
    ) -> RuntimeStoreError {
        RuntimeStoreError::SessionPersistenceAuthorityConflict {
            runtime_id: runtime_id_text(runtime_id).to_owned(),
            detail: detail.into(),
        }
    }

    fn validate_exact_head_prefix_authority(
        runtime_id: &LogicalRuntimeId,
        head: &meerkat_core::session_store::SessionHead,
        label: &str,
    ) -> Result<(), RuntimeStoreError> {
        let prefix = head.message_row_prefix.as_ref().ok_or_else(|| {
            session_authority_conflict(
                runtime_id,
                format!("{label} has no exact message-row prefix authority"),
            )
        })?;
        if prefix.row_count() != head.message_count {
            return Err(session_authority_conflict(
                runtime_id,
                format!(
                    "{label} covers {} exact message rows but declares {} messages",
                    prefix.row_count(),
                    head.message_count
                ),
            ));
        }
        if head.rewrite_prefix.occurrence_count() != head.rewrite_count {
            return Err(session_authority_conflict(
                runtime_id,
                format!("{label} rewrite count and exact rewrite-prefix authority differ"),
            ));
        }
        Ok(())
    }

    fn validate_head_canonical_provisional_progress(
        runtime_id: &LogicalRuntimeId,
        predecessor: &meerkat_core::session_store::SessionHead,
        successor: &meerkat_core::session_store::SessionHead,
    ) -> Result<(), RuntimeStoreError> {
        if predecessor.id != successor.id
            || predecessor.created_at != successor.created_at
            || successor.version < predecessor.version
            || successor.updated_at < predecessor.updated_at
            || successor.rewrite_count < predecessor.rewrite_count
            || (successor.rewrite_count == predecessor.rewrite_count
                && (successor.strand != predecessor.strand
                    || successor.message_count < predecessor.message_count))
        {
            return Err(session_authority_conflict(
                runtime_id,
                "provisional HeadCanonical successor moves a store-owned monotonic head axis backwards",
            ));
        }
        Ok(())
    }

    fn map_runtime_snapshot_mutation_error(
        runtime_id: &LogicalRuntimeId,
        error: rusqlite::Error,
    ) -> RuntimeStoreError {
        if error
            .to_string()
            .contains(HEAD_CANONICAL_FROZEN_SNAPSHOT_ERROR)
        {
            return session_authority_conflict(
                runtime_id,
                "whole-BLOB mutation refused because head-canonical authority or profile activation is installed",
            );
        }
        RuntimeStoreError::WriteFailed(error.to_string())
    }

    fn load_head_canonical_authority(
        conn: &Connection,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<RuntimeSessionAuthority>, RuntimeStoreError> {
        let row = conn
            .query_row(
                r"
                SELECT authority_version, session_id, store_revision,
                       boundary_head_json, committed_head_token
                FROM runtime_session_authority
                WHERE runtime_id = ?1
                ",
                params![runtime_id_text(runtime_id)],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, JsonColumnBytes>(3)?.into_bytes(),
                        row.get::<_, String>(4)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
        let Some((version, session_id, store_revision, boundary_head_json, head_token)) = row
        else {
            return Ok(None);
        };
        if version != i64::from(HeadCanonicalStoreAuthority::VERSION) {
            return Err(session_authority_conflict(
                runtime_id,
                format!(
                    "unsupported HeadCanonical authority version {version}; supported version is {}",
                    HeadCanonicalStoreAuthority::VERSION
                ),
            ));
        }
        let store_revision = u64::try_from(store_revision).map_err(|_| {
            session_authority_conflict(
                runtime_id,
                "persisted HeadCanonical store revision is not positive",
            )
        })?;
        let session_id = meerkat_core::types::SessionId::parse(&session_id).map_err(|error| {
            session_authority_conflict(
                runtime_id,
                format!("persisted authority session id is invalid: {error}"),
            )
        })?;
        let boundary_head: meerkat_core::session_store::SessionHead =
            serde_json::from_slice(&boundary_head_json).map_err(|error| {
                session_authority_conflict(
                    runtime_id,
                    format!("persisted boundary head authority is invalid: {error}"),
                )
            })?;
        validate_exact_head_prefix_authority(
            runtime_id,
            &boundary_head,
            "persisted boundary head",
        )?;
        let authority = HeadCanonicalStoreAuthority::issued(
            session_id,
            store_revision,
            boundary_head,
            head_token,
        )?;
        let owner = LogicalRuntimeId::for_session(authority.session_id());
        if &owner != runtime_id {
            return Err(session_authority_conflict(
                runtime_id,
                format!("persisted session authority belongs to runtime {owner}, not {runtime_id}"),
            ));
        }
        Ok(Some(RuntimeSessionAuthority::HeadCanonical(authority)))
    }

    #[derive(Debug)]
    struct StoredHeadCanonicalProvisionalTail {
        authority: HeadCanonicalProvisionalTailAuthority,
        candidate_message_count: usize,
        candidate_conversation_digest: String,
        catalog_entry: crate::store::RuntimeSessionCatalogEntry,
        compaction_projection_intents: Vec<meerkat_core::CompactionProjectionIntent>,
        predecessor_projection: Option<(
            usize,
            String,
            crate::store::RuntimeSessionCatalogEntry,
            Vec<meerkat_core::CompactionProjectionIntent>,
        )>,
    }

    fn load_head_canonical_provisional_tail(
        conn: &Connection,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<StoredHeadCanonicalProvisionalTail>, RuntimeStoreError> {
        let row = conn
            .query_row(
                r"
                SELECT authority_version, session_id, base_store_revision,
                       base_committed_head_token, physical_store_revision,
                       physical_head_token, run_id, candidate_sequence,
                       candidate_message_count, candidate_conversation_digest,
                       catalog_json, compaction_intents_json,
                       predecessor_candidate_message_count,
                       predecessor_candidate_conversation_digest,
                       predecessor_catalog_json,
                       predecessor_compaction_intents_json
                FROM runtime_head_canonical_provisional_tails
                WHERE runtime_id = ?1
                ",
                params![runtime_id_text(runtime_id)],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, i64>(7)?,
                        row.get::<_, i64>(8)?,
                        row.get::<_, String>(9)?,
                        row.get::<_, JsonColumnBytes>(10)?.into_bytes(),
                        row.get::<_, JsonColumnBytes>(11)?.into_bytes(),
                        row.get::<_, Option<i64>>(12)?,
                        row.get::<_, Option<String>>(13)?,
                        row.get::<_, Option<JsonColumnBytes>>(14)?
                            .map(JsonColumnBytes::into_bytes),
                        row.get::<_, Option<JsonColumnBytes>>(15)?
                            .map(JsonColumnBytes::into_bytes),
                    ))
                },
            )
            .optional()
            .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
        let Some((
            version,
            session_id,
            base_store_revision,
            base_committed_head_token,
            physical_store_revision,
            physical_head_token,
            run_id,
            candidate_sequence,
            candidate_message_count,
            candidate_conversation_digest,
            catalog_json,
            compaction_intents_json,
            predecessor_candidate_message_count,
            predecessor_candidate_conversation_digest,
            predecessor_catalog_json,
            predecessor_compaction_intents_json,
        )) = row
        else {
            return Ok(None);
        };
        if version != i64::from(HeadCanonicalProvisionalTailAuthority::VERSION) {
            return Err(session_authority_conflict(
                runtime_id,
                format!(
                    "unsupported provisional HeadCanonical authority version {version}; supported version is {}",
                    HeadCanonicalProvisionalTailAuthority::VERSION
                ),
            ));
        }
        let session_id = meerkat_core::types::SessionId::parse(&session_id).map_err(|error| {
            session_authority_conflict(
                runtime_id,
                format!("provisional HeadCanonical session id is invalid: {error}"),
            )
        })?;
        let owner = LogicalRuntimeId::for_session(&session_id);
        if &owner != runtime_id {
            return Err(session_authority_conflict(
                runtime_id,
                format!("provisional HeadCanonical authority belongs to {owner}, not {runtime_id}"),
            ));
        }
        let base_store_revision = u64::try_from(base_store_revision).map_err(|_| {
            session_authority_conflict(
                runtime_id,
                "provisional HeadCanonical base store revision is not positive",
            )
        })?;
        let physical_store_revision = u64::try_from(physical_store_revision).map_err(|_| {
            session_authority_conflict(
                runtime_id,
                "provisional HeadCanonical physical store revision is not positive",
            )
        })?;
        let run_id = uuid::Uuid::parse_str(&run_id)
            .map(RunId::from_uuid)
            .map_err(|error| {
                session_authority_conflict(
                    runtime_id,
                    format!("provisional HeadCanonical run id is invalid: {error}"),
                )
            })?;
        let candidate_sequence = u64::try_from(candidate_sequence).map_err(|_| {
            session_authority_conflict(
                runtime_id,
                "provisional HeadCanonical candidate sequence is not positive",
            )
        })?;
        let authority = HeadCanonicalProvisionalTailAuthority::issued(
            session_id,
            base_store_revision,
            base_committed_head_token,
            physical_store_revision,
            physical_head_token,
            run_id,
            candidate_sequence,
        )
        .map_err(|error| session_authority_conflict(runtime_id, error.to_string()))?;
        let candidate_message_count = usize::try_from(candidate_message_count).map_err(|_| {
            session_authority_conflict(
                runtime_id,
                "provisional HeadCanonical candidate message count is invalid",
            )
        })?;
        let catalog_entry =
            serde_json::from_slice::<crate::store::RuntimeSessionCatalogEntry>(&catalog_json)
                .map_err(|error| {
                    session_authority_conflict(
                        runtime_id,
                        format!("provisional HeadCanonical catalog is invalid: {error}"),
                    )
                })?;
        if catalog_entry.session_id() != authority.session_id()
            || catalog_entry.persistence_profile()
                != RuntimeSessionPersistenceProfile::HeadCanonicalV1
            || catalog_entry.message_count() != candidate_message_count
        {
            return Err(session_authority_conflict(
                runtime_id,
                "provisional HeadCanonical catalog does not bind its exact candidate",
            ));
        }
        let compaction_projection_intents = serde_json::from_slice::<
            Vec<meerkat_core::CompactionProjectionIntent>,
        >(&compaction_intents_json)
        .map_err(|error| {
            session_authority_conflict(
                runtime_id,
                format!("provisional HeadCanonical compaction intents are invalid: {error}"),
            )
        })?;
        if compaction_projection_intents
            .iter()
            .any(|intent| intent.projection.session_id() != authority.session_id())
        {
            return Err(session_authority_conflict(
                runtime_id,
                "provisional HeadCanonical compaction intents do not bind the candidate session",
            ));
        }
        let predecessor_projection = match (
            predecessor_candidate_message_count,
            predecessor_candidate_conversation_digest,
            predecessor_catalog_json,
            predecessor_compaction_intents_json,
        ) {
            (None, None, None, None) => None,
            (
                Some(message_count),
                Some(conversation_digest),
                Some(catalog_json),
                Some(compaction_intents_json),
            ) => {
                let message_count = usize::try_from(message_count).map_err(|_| {
                    session_authority_conflict(
                        runtime_id,
                        "provisional HeadCanonical predecessor message count is invalid",
                    )
                })?;
                let catalog_entry = serde_json::from_slice::<
                    crate::store::RuntimeSessionCatalogEntry,
                >(&catalog_json)
                .map_err(|error| {
                    session_authority_conflict(
                        runtime_id,
                        format!(
                            "provisional HeadCanonical predecessor catalog is invalid: {error}"
                        ),
                    )
                })?;
                let compaction_projection_intents = serde_json::from_slice::<
                    Vec<meerkat_core::CompactionProjectionIntent>,
                >(&compaction_intents_json)
                .map_err(|error| {
                    session_authority_conflict(
                        runtime_id,
                        format!(
                            "provisional HeadCanonical predecessor compaction intents are invalid: {error}"
                        ),
                    )
                })?;
                if compaction_projection_intents
                    .iter()
                    .any(|intent| intent.projection.session_id() != authority.session_id())
                {
                    return Err(session_authority_conflict(
                        runtime_id,
                        "provisional HeadCanonical predecessor compaction intents do not bind the candidate session",
                    ));
                }
                if catalog_entry.session_id() != authority.session_id()
                    || catalog_entry.persistence_profile()
                        != RuntimeSessionPersistenceProfile::HeadCanonicalV1
                    || catalog_entry.message_count() != message_count
                {
                    return Err(session_authority_conflict(
                        runtime_id,
                        "provisional HeadCanonical predecessor projection is inconsistent",
                    ));
                }
                Some((
                    message_count,
                    conversation_digest,
                    catalog_entry,
                    compaction_projection_intents,
                ))
            }
            _ => {
                return Err(session_authority_conflict(
                    runtime_id,
                    "provisional HeadCanonical predecessor projection shape is invalid",
                ));
            }
        };
        Ok(Some(StoredHeadCanonicalProvisionalTail {
            authority,
            candidate_message_count,
            candidate_conversation_digest,
            catalog_entry,
            compaction_projection_intents,
            predecessor_projection,
        }))
    }

    fn load_head_canonical_provisional_tail_authority(
        conn: &Connection,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<HeadCanonicalProvisionalTailAuthority>, RuntimeStoreError> {
        load_head_canonical_provisional_tail(conn, runtime_id)
            .map(|stored| stored.map(|stored| stored.authority))
    }

    fn issue_head_canonical_authority_in_txn(
        tx: &Transaction<'_>,
        runtime_id: &LogicalRuntimeId,
        boundary_head: meerkat_core::session_store::SessionHead,
    ) -> Result<RuntimeSessionAuthority, RuntimeStoreError> {
        validate_exact_head_prefix_authority(
            runtime_id,
            &boundary_head,
            "HeadCanonical authority boundary head",
        )?;
        let head_token = meerkat_core::session_head_cas_token(&boundary_head).map_err(|error| {
            session_authority_conflict(
                runtime_id,
                format!("HeadCanonical boundary token is invalid: {error}"),
            )
        })?;
        let current = load_head_canonical_authority(tx, runtime_id)?;
        if let Some(RuntimeSessionAuthority::HeadCanonical(current)) = current.as_ref()
            && current.boundary_head() == &boundary_head
            && current.committed_head_token() == head_token.as_str()
        {
            return Ok(RuntimeSessionAuthority::HeadCanonical(current.clone()));
        }
        let store_revision = match current
            .as_ref()
            .and_then(RuntimeSessionAuthority::head_canonical)
        {
            Some(current) => {
                let base = match load_head_canonical_provisional_tail_authority(tx, runtime_id)? {
                    Some(provisional)
                        if provisional.session_id() == current.session_id()
                            && provisional.base_store_revision() == current.store_revision()
                            && provisional.base_committed_head_token()
                                == current.committed_head_token() =>
                    {
                        provisional.physical_store_revision()
                    }
                    Some(_) => {
                        return Err(session_authority_conflict(
                            runtime_id,
                            "provisional HeadCanonical authority does not descend from the current committed authority",
                        ));
                    }
                    None => current.store_revision(),
                };
                base.checked_add(1).ok_or_else(|| {
                    session_authority_conflict(runtime_id, "HeadCanonical store revision overflow")
                })?
            }
            None => 1,
        };
        let authority = HeadCanonicalStoreAuthority::issued(
            boundary_head.id.clone(),
            store_revision,
            boundary_head,
            head_token,
        )?;
        Ok(RuntimeSessionAuthority::HeadCanonical(authority))
    }

    fn write_head_canonical_authority_in_txn(
        tx: &Transaction<'_>,
        runtime_id: &LogicalRuntimeId,
        authority: &RuntimeSessionAuthority,
    ) -> Result<(), RuntimeStoreError> {
        let authority = authority.head_canonical().ok_or_else(|| {
            session_authority_conflict(
                runtime_id,
                "cannot write WholeBlob authority into the HeadCanonical table",
            )
        })?;
        let owner = LogicalRuntimeId::for_session(authority.session_id());
        if &owner != runtime_id {
            return Err(session_authority_conflict(
                runtime_id,
                format!("head-canonical authority belongs to runtime {owner}, not {runtime_id}"),
            ));
        }
        let boundary_head = authority.boundary_head();
        validate_exact_head_prefix_authority(
            runtime_id,
            boundary_head,
            "head-canonical authority boundary head",
        )?;
        let boundary_head_json = serde_json::to_vec(boundary_head)
            .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
        let current = load_head_canonical_authority(tx, runtime_id)?;
        let advances_authority = match current.as_ref() {
            None if authority.store_revision() == 1 => true,
            Some(RuntimeSessionAuthority::HeadCanonical(current)) if current == authority => false,
            Some(RuntimeSessionAuthority::HeadCanonical(current))
                if current.session_id() == authority.session_id() =>
            {
                let base = match load_head_canonical_provisional_tail_authority(tx, runtime_id)? {
                    Some(provisional)
                        if provisional.session_id() == current.session_id()
                            && provisional.base_store_revision() == current.store_revision()
                            && provisional.base_committed_head_token()
                                == current.committed_head_token() =>
                    {
                        provisional.physical_store_revision()
                    }
                    Some(_) => {
                        return Err(session_authority_conflict(
                            runtime_id,
                            "provisional HeadCanonical authority does not descend from the current committed authority",
                        ));
                    }
                    None => current.store_revision(),
                };
                if base.checked_add(1) != Some(authority.store_revision()) {
                    return Err(session_authority_conflict(
                        runtime_id,
                        "HeadCanonical successor does not carry the exact next store revision",
                    ));
                }
                true
            }
            Some(_) | None => {
                return Err(session_authority_conflict(
                    runtime_id,
                    "HeadCanonical authority write is neither an exact retry nor the next monotonic store revision",
                ));
            }
        };
        tx.execute(
            r"
            INSERT INTO runtime_session_authority (
                runtime_id, authority_version, session_id, store_revision,
                boundary_head_json, committed_head_token
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            ON CONFLICT(runtime_id) DO UPDATE SET
                authority_version = excluded.authority_version,
                session_id = excluded.session_id,
                store_revision = excluded.store_revision,
                boundary_head_json = excluded.boundary_head_json,
                committed_head_token = excluded.committed_head_token
            ",
            params![
                runtime_id_text(runtime_id),
                i64::from(authority.authority_version()),
                authority.session_id().to_string(),
                i64::try_from(authority.store_revision()).map_err(|_| {
                    session_authority_conflict(
                        runtime_id,
                        "HeadCanonical store revision exceeds SQLite INTEGER",
                    )
                })?,
                boundary_head_json,
                authority.committed_head_token(),
            ],
        )
        .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
        meerkat_store::sqlite_store::retain_runtime_boundary_head_metadata_in_txn(
            tx,
            boundary_head,
        )
        .map_err(|error| map_head_canonical_session_store_error(runtime_id, error))?;
        if advances_authority {
            tx.execute(
                "DELETE FROM runtime_head_canonical_provisional_tails WHERE runtime_id = ?1",
                params![runtime_id_text(runtime_id)],
            )
            .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
        }
        Ok(())
    }

    // The store issues this authority from one exact prepared physical
    // boundary; each argument is an independently checked part of that seal.
    #[allow(clippy::too_many_arguments)]
    fn issue_head_canonical_provisional_tail_authority_in_txn(
        tx: &Transaction<'_>,
        runtime_id: &LogicalRuntimeId,
        committed: &HeadCanonicalStoreAuthority,
        run_id: &RunId,
        successor_head: &meerkat_core::session_store::SessionHead,
        successor_head_token: &str,
        candidate_message_count: usize,
        candidate_conversation_digest: &str,
        catalog_entry: &crate::store::RuntimeSessionCatalogEntry,
        compaction_projection_intents: &[meerkat_core::CompactionProjectionIntent],
    ) -> Result<HeadCanonicalProvisionalTailAuthority, RuntimeStoreError> {
        let owner = LogicalRuntimeId::for_session(committed.session_id());
        if &owner != runtime_id {
            return Err(session_authority_conflict(
                runtime_id,
                format!("provisional HeadCanonical intent belongs to {owner}, not {runtime_id}"),
            ));
        }
        if &successor_head.id != committed.session_id() {
            return Err(session_authority_conflict(
                runtime_id,
                "provisional HeadCanonical successor belongs to another session",
            ));
        }
        let derived_successor_token = meerkat_core::session_head_cas_token(successor_head)
            .map_err(|error| {
                session_authority_conflict(
                    runtime_id,
                    format!("provisional HeadCanonical successor is invalid: {error}"),
                )
            })?;
        if derived_successor_token.as_str() != successor_head_token
            || successor_head_token == committed.committed_head_token()
        {
            return Err(session_authority_conflict(
                runtime_id,
                "provisional HeadCanonical successor token is not the exact distinct target head",
            ));
        }
        let stored_committed = load_head_canonical_authority(tx, runtime_id)?
            .and_then(|authority| authority.head_canonical().cloned())
            .ok_or_else(|| {
                session_authority_conflict(
                    runtime_id,
                    "provisional HeadCanonical authority has no committed base",
                )
            })?;
        if &stored_committed != committed {
            return Err(session_authority_conflict(
                runtime_id,
                "provisional HeadCanonical intent does not name the exact committed base",
            ));
        }
        let catalog_json = serde_json::to_vec(catalog_entry)
            .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
        let compaction_intents_json = serde_json::to_vec(compaction_projection_intents)
            .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
        if catalog_entry.session_id() != committed.session_id()
            || catalog_entry.persistence_profile()
                != RuntimeSessionPersistenceProfile::HeadCanonicalV1
            || catalog_entry.message_count() != candidate_message_count
            || candidate_conversation_digest != successor_head.head_revision
        {
            return Err(session_authority_conflict(
                runtime_id,
                "provisional HeadCanonical candidate projections do not bind its exact successor",
            ));
        }
        if let Some(existing) = load_head_canonical_provisional_tail(tx, runtime_id)? {
            if existing.authority.session_id() != committed.session_id()
                || existing.authority.base_store_revision() != committed.store_revision()
                || existing.authority.base_committed_head_token()
                    != committed.committed_head_token()
                || existing.authority.run_id() != run_id
            {
                return Err(session_authority_conflict(
                    runtime_id,
                    "a divergent provisional HeadCanonical base or run already exists",
                ));
            }
            if existing.authority.physical_head_token() == successor_head_token {
                if existing.candidate_message_count != candidate_message_count
                    || existing.candidate_conversation_digest != candidate_conversation_digest
                    || existing.catalog_entry != *catalog_entry
                    || existing.compaction_projection_intents.as_slice()
                        != compaction_projection_intents
                {
                    return Err(session_authority_conflict(
                        runtime_id,
                        "exact provisional HeadCanonical target carries divergent candidate projections",
                    ));
                }
                return Ok(existing.authority);
            }
            let (physical_head, physical_head_token) =
                meerkat_store::sqlite_store::load_head_canonical_for_runtime_in_txn(
                    tx,
                    committed.session_id(),
                )
                .map_err(|error| map_head_canonical_session_store_error(runtime_id, error))?
                .ok_or_else(|| {
                    session_authority_conflict(
                        runtime_id,
                        "provisional HeadCanonical chain has no physical head",
                    )
                })?;
            if physical_head_token != existing.authority.physical_head_token() {
                return Err(session_authority_conflict(
                    runtime_id,
                    "next provisional HeadCanonical intent does not descend from the latest applied physical checkpoint",
                ));
            }
            validate_exact_head_prefix_authority(
                runtime_id,
                &physical_head,
                "latest provisional physical head",
            )?;
            validate_head_canonical_provisional_progress(
                runtime_id,
                &physical_head,
                successor_head,
            )?;
            let physical_store_revision = existing
                .authority
                .physical_store_revision()
                .checked_add(1)
                .ok_or_else(|| {
                    session_authority_conflict(
                        runtime_id,
                        "provisional HeadCanonical physical revision overflow",
                    )
                })?;
            let successor = HeadCanonicalProvisionalTailAuthority::issued(
                committed.session_id().clone(),
                committed.store_revision(),
                committed.committed_head_token().to_string(),
                physical_store_revision,
                successor_head_token.to_string(),
                run_id.clone(),
                physical_store_revision
                    .checked_sub(committed.store_revision())
                    .ok_or_else(|| {
                        session_authority_conflict(
                            runtime_id,
                            "provisional HeadCanonical candidate sequence underflow",
                        )
                    })?,
            )
            .map_err(|error| session_authority_conflict(runtime_id, error.to_string()))?;
            let updated = tx
                .execute(
                    r"
                    UPDATE runtime_head_canonical_provisional_tails
                    SET physical_store_revision = ?2,
                        physical_head_token = ?3,
                        candidate_sequence = ?4,
                        candidate_message_count = ?13,
                        candidate_conversation_digest = ?14,
                        catalog_json = ?15,
                        compaction_intents_json = ?16,
                        predecessor_candidate_message_count = ?17,
                        predecessor_candidate_conversation_digest = ?18,
                        predecessor_catalog_json = ?19,
                        predecessor_compaction_intents_json = ?20
                    WHERE runtime_id = ?1
                      AND authority_version = ?5
                      AND session_id = ?6
                      AND base_store_revision = ?7
                      AND base_committed_head_token = ?8
                      AND physical_store_revision = ?9
                      AND physical_head_token = ?10
                      AND run_id = ?11
                      AND candidate_sequence = ?12
                    ",
                    params![
                        runtime_id_text(runtime_id),
                        i64::try_from(successor.physical_store_revision()).map_err(|_| {
                            session_authority_conflict(
                                runtime_id,
                                "provisional physical store revision exceeds SQLite INTEGER",
                            )
                        })?,
                        successor.physical_head_token(),
                        i64::try_from(successor.candidate_sequence()).map_err(|_| {
                            session_authority_conflict(
                                runtime_id,
                                "provisional candidate sequence exceeds SQLite INTEGER",
                            )
                        })?,
                        i64::from(existing.authority.authority_version()),
                        existing.authority.session_id().to_string(),
                        i64::try_from(existing.authority.base_store_revision()).map_err(|_| {
                            session_authority_conflict(
                                runtime_id,
                                "provisional base store revision exceeds SQLite INTEGER",
                            )
                        })?,
                        existing.authority.base_committed_head_token(),
                        i64::try_from(existing.authority.physical_store_revision()).map_err(
                            |_| {
                                session_authority_conflict(
                                    runtime_id,
                                    "provisional physical store revision exceeds SQLite INTEGER",
                                )
                            }
                        )?,
                        existing.authority.physical_head_token(),
                        existing.authority.run_id().to_string(),
                        i64::try_from(existing.authority.candidate_sequence()).map_err(|_| {
                            session_authority_conflict(
                                runtime_id,
                                "provisional candidate sequence exceeds SQLite INTEGER",
                            )
                        })?,
                        i64::try_from(candidate_message_count).map_err(|_| {
                            session_authority_conflict(
                                runtime_id,
                                "provisional candidate message count exceeds SQLite INTEGER",
                            )
                        })?,
                        candidate_conversation_digest,
                        catalog_json,
                        compaction_intents_json,
                        i64::try_from(existing.candidate_message_count).map_err(|_| {
                            session_authority_conflict(
                                runtime_id,
                                "provisional predecessor message count exceeds SQLite INTEGER",
                            )
                        })?,
                        existing.candidate_conversation_digest,
                        serde_json::to_vec(&existing.catalog_entry)
                            .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?,
                        serde_json::to_vec(&existing.compaction_projection_intents)
                            .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?,
                    ],
                )
                .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
            if updated != 1 {
                return Err(session_authority_conflict(
                    runtime_id,
                    "provisional HeadCanonical authority changed during monotonic advance",
                ));
            }
            return Ok(successor);
        }
        let (physical_head, physical_head_token) =
            meerkat_store::sqlite_store::load_head_canonical_for_runtime_in_txn(
                tx,
                committed.session_id(),
            )
            .map_err(|error| map_head_canonical_session_store_error(runtime_id, error))?
            .ok_or_else(|| {
                session_authority_conflict(
                    runtime_id,
                    "first provisional HeadCanonical intent has no physical committed head",
                )
            })?;
        if &physical_head != committed.boundary_head()
            || physical_head_token != committed.committed_head_token()
        {
            return Err(session_authority_conflict(
                runtime_id,
                "first provisional HeadCanonical intent does not start at the exact committed physical head",
            ));
        }
        validate_head_canonical_provisional_progress(
            runtime_id,
            committed.boundary_head(),
            successor_head,
        )?;
        let physical_store_revision =
            committed.store_revision().checked_add(1).ok_or_else(|| {
                session_authority_conflict(
                    runtime_id,
                    "provisional HeadCanonical physical revision overflow",
                )
            })?;
        let authority = HeadCanonicalProvisionalTailAuthority::issued(
            committed.session_id().clone(),
            committed.store_revision(),
            committed.committed_head_token().to_string(),
            physical_store_revision,
            successor_head_token.to_string(),
            run_id.clone(),
            1,
        )
        .map_err(|error| session_authority_conflict(runtime_id, error.to_string()))?;
        tx.execute(
            r"
            INSERT INTO runtime_head_canonical_provisional_tails (
                runtime_id, authority_version, session_id, base_store_revision,
                base_committed_head_token, physical_store_revision,
                physical_head_token, run_id, candidate_sequence
                , candidate_message_count, candidate_conversation_digest,
                catalog_json, compaction_intents_json
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
            ",
            params![
                runtime_id_text(runtime_id),
                i64::from(authority.authority_version()),
                authority.session_id().to_string(),
                i64::try_from(authority.base_store_revision()).map_err(|_| {
                    session_authority_conflict(
                        runtime_id,
                        "provisional base store revision exceeds SQLite INTEGER",
                    )
                })?,
                authority.base_committed_head_token(),
                i64::try_from(authority.physical_store_revision()).map_err(|_| {
                    session_authority_conflict(
                        runtime_id,
                        "provisional physical store revision exceeds SQLite INTEGER",
                    )
                })?,
                authority.physical_head_token(),
                authority.run_id().to_string(),
                i64::try_from(authority.candidate_sequence()).map_err(|_| {
                    session_authority_conflict(
                        runtime_id,
                        "provisional candidate sequence exceeds SQLite INTEGER",
                    )
                })?,
                i64::try_from(candidate_message_count).map_err(|_| {
                    session_authority_conflict(
                        runtime_id,
                        "provisional candidate message count exceeds SQLite INTEGER",
                    )
                })?,
                candidate_conversation_digest,
                catalog_json,
                compaction_intents_json,
            ],
        )
        .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
        Ok(authority)
    }

    fn discard_head_canonical_provisional_tail_authority_in_txn(
        tx: &Transaction<'_>,
        runtime_id: &LogicalRuntimeId,
        expected: &HeadCanonicalProvisionalTailAuthority,
    ) -> Result<bool, RuntimeStoreError> {
        let stored = match load_head_canonical_provisional_tail(tx, runtime_id)? {
            Some(current) if current.authority == *expected => current,
            Some(_) => {
                return Err(session_authority_conflict(
                    runtime_id,
                    "refusing to discard a divergent provisional HeadCanonical authority",
                ));
            }
            None => return Ok(false),
        };
        let committed = load_head_canonical_authority(tx, runtime_id)?
            .and_then(|authority| authority.head_canonical().cloned())
            .ok_or_else(|| {
                session_authority_conflict(
                    runtime_id,
                    "provisional HeadCanonical discard has no committed base",
                )
            })?;
        if expected.session_id() != committed.session_id()
            || expected.base_store_revision() != committed.store_revision()
            || expected.base_committed_head_token() != committed.committed_head_token()
        {
            return Err(session_authority_conflict(
                runtime_id,
                "provisional HeadCanonical discard base is stale",
            ));
        }
        let (physical_head, physical_head_token) =
            meerkat_store::sqlite_store::load_head_canonical_for_runtime_in_txn(
                tx,
                committed.session_id(),
            )
            .map_err(|error| map_head_canonical_session_store_error(runtime_id, error))?
            .ok_or_else(|| {
                session_authority_conflict(
                    runtime_id,
                    "provisional HeadCanonical discard has no physical head",
                )
            })?;
        if physical_head_token == expected.physical_head_token() {
            return Err(session_authority_conflict(
                runtime_id,
                "refusing to discard an already-applied provisional HeadCanonical checkpoint",
            ));
        }
        if expected.candidate_sequence() == 1 {
            if &physical_head != committed.boundary_head()
                || physical_head_token != committed.committed_head_token()
            {
                return Err(session_authority_conflict(
                    runtime_id,
                    "first provisional HeadCanonical intent cannot be discarded from a divergent physical head",
                ));
            }
        } else {
            meerkat_store::sqlite_store::verify_physical_head_retains_boundary_prefix_for_runtime_in_txn(
                tx,
                committed.boundary_head(),
                &physical_head,
            )
            .map_err(|error| map_head_canonical_session_store_error(runtime_id, error))?;
            let restored_revision = expected
                .physical_store_revision()
                .checked_sub(1)
                .ok_or_else(|| {
                    session_authority_conflict(
                        runtime_id,
                        "provisional HeadCanonical physical revision underflow",
                    )
                })?;
            let restored = HeadCanonicalProvisionalTailAuthority::issued(
                expected.session_id().clone(),
                expected.base_store_revision(),
                expected.base_committed_head_token().to_string(),
                restored_revision,
                physical_head_token,
                expected.run_id().clone(),
                expected
                    .candidate_sequence()
                    .checked_sub(1)
                    .ok_or_else(|| {
                        session_authority_conflict(
                            runtime_id,
                            "restored provisional candidate sequence underflow",
                        )
                    })?,
            )
            .map_err(|error| session_authority_conflict(runtime_id, error.to_string()))?;
            let (
                predecessor_message_count,
                predecessor_conversation_digest,
                predecessor_catalog,
                predecessor_compaction_intents,
            ) = stored.predecessor_projection.ok_or_else(|| {
                session_authority_conflict(
                    runtime_id,
                    "provisional HeadCanonical rollback has no exact predecessor projections",
                )
            })?;
            if physical_head.message_count
                != u64::try_from(predecessor_message_count).map_err(|_| {
                    session_authority_conflict(
                        runtime_id,
                        "provisional predecessor message count exceeds u64",
                    )
                })?
                || physical_head.head_revision != predecessor_conversation_digest
            {
                return Err(session_authority_conflict(
                    runtime_id,
                    "provisional HeadCanonical rollback physical head differs from predecessor projections",
                ));
            }
            let updated = tx
                .execute(
                    r"
                    UPDATE runtime_head_canonical_provisional_tails
                    SET physical_store_revision = ?2,
                        physical_head_token = ?3,
                        candidate_sequence = ?4,
                        candidate_message_count = ?13,
                        candidate_conversation_digest = ?14,
                        catalog_json = ?15,
                        compaction_intents_json = ?16,
                        predecessor_candidate_message_count = NULL,
                        predecessor_candidate_conversation_digest = NULL,
                        predecessor_catalog_json = NULL,
                        predecessor_compaction_intents_json = NULL
                    WHERE runtime_id = ?1
                      AND authority_version = ?5
                      AND session_id = ?6
                      AND base_store_revision = ?7
                      AND base_committed_head_token = ?8
                      AND physical_store_revision = ?9
                      AND physical_head_token = ?10
                      AND run_id = ?11
                      AND candidate_sequence = ?12
                    ",
                    params![
                        runtime_id_text(runtime_id),
                        i64::try_from(restored.physical_store_revision()).map_err(|_| {
                            session_authority_conflict(
                                runtime_id,
                                "restored provisional physical revision exceeds SQLite INTEGER",
                            )
                        })?,
                        restored.physical_head_token(),
                        i64::try_from(restored.candidate_sequence()).map_err(|_| {
                            session_authority_conflict(
                                runtime_id,
                                "restored provisional candidate sequence exceeds SQLite INTEGER",
                            )
                        })?,
                        i64::from(expected.authority_version()),
                        expected.session_id().to_string(),
                        i64::try_from(expected.base_store_revision()).map_err(|_| {
                            session_authority_conflict(
                                runtime_id,
                                "provisional base store revision exceeds SQLite INTEGER",
                            )
                        })?,
                        expected.base_committed_head_token(),
                        i64::try_from(expected.physical_store_revision()).map_err(|_| {
                            session_authority_conflict(
                                runtime_id,
                                "provisional physical store revision exceeds SQLite INTEGER",
                            )
                        })?,
                        expected.physical_head_token(),
                        expected.run_id().to_string(),
                        i64::try_from(expected.candidate_sequence()).map_err(|_| {
                            session_authority_conflict(
                                runtime_id,
                                "provisional candidate sequence exceeds SQLite INTEGER",
                            )
                        })?,
                        i64::try_from(predecessor_message_count).map_err(|_| {
                            session_authority_conflict(
                                runtime_id,
                                "provisional predecessor message count exceeds SQLite INTEGER",
                            )
                        })?,
                        predecessor_conversation_digest,
                        serde_json::to_vec(&predecessor_catalog)
                            .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?,
                        serde_json::to_vec(&predecessor_compaction_intents)
                            .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?,
                    ],
                )
                .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
            if updated != 1 {
                return Err(session_authority_conflict(
                    runtime_id,
                    "provisional HeadCanonical authority changed during rollback",
                ));
            }
            return Ok(true);
        }
        let deleted = tx
            .execute(
                r"
                DELETE FROM runtime_head_canonical_provisional_tails
                WHERE runtime_id = ?1
                  AND authority_version = ?2
                  AND session_id = ?3
                  AND base_store_revision = ?4
                  AND base_committed_head_token = ?5
                  AND physical_store_revision = ?6
                  AND physical_head_token = ?7
                  AND run_id = ?8
                  AND candidate_sequence = ?9
                ",
                params![
                    runtime_id_text(runtime_id),
                    i64::from(expected.authority_version()),
                    expected.session_id().to_string(),
                    i64::try_from(expected.base_store_revision()).map_err(|_| {
                        session_authority_conflict(
                            runtime_id,
                            "provisional base store revision exceeds SQLite INTEGER",
                        )
                    })?,
                    expected.base_committed_head_token(),
                    i64::try_from(expected.physical_store_revision()).map_err(|_| {
                        session_authority_conflict(
                            runtime_id,
                            "provisional physical store revision exceeds SQLite INTEGER",
                        )
                    })?,
                    expected.physical_head_token(),
                    expected.run_id().to_string(),
                    i64::try_from(expected.candidate_sequence()).map_err(|_| {
                        session_authority_conflict(
                            runtime_id,
                            "provisional candidate sequence exceeds SQLite INTEGER",
                        )
                    })?,
                ],
            )
            .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
        if deleted != 1 {
            return Err(session_authority_conflict(
                runtime_id,
                "provisional HeadCanonical authority changed during exact discard",
            ));
        }
        Ok(true)
    }

    fn map_head_canonical_session_store_error(
        runtime_id: &LogicalRuntimeId,
        error: meerkat_core::SessionStoreError,
    ) -> RuntimeStoreError {
        match error {
            meerkat_core::SessionStoreError::TranscriptRevisionConflict {
                expected,
                actual,
                ..
            } => RuntimeStoreError::TranscriptRevisionConflict { expected, actual },
            meerkat_core::SessionStoreError::Io(error) => {
                RuntimeStoreError::WriteFailed(error.to_string())
            }
            meerkat_core::SessionStoreError::Serialization(detail)
            | meerkat_core::SessionStoreError::Internal(detail) => {
                RuntimeStoreError::WriteFailed(detail)
            }
            other => session_authority_conflict(runtime_id, other.to_string()),
        }
    }

    fn refuse_whole_blob_write_under_head_authority(
        conn: &Connection,
        runtime_id: &LogicalRuntimeId,
        operation: &str,
    ) -> Result<(), RuntimeStoreError> {
        if load_head_canonical_authority(conn, runtime_id)?.is_some() {
            return Err(session_authority_conflict(
                runtime_id,
                format!(
                    "{operation} refused because head-canonical runtime authority is installed"
                ),
            ));
        }
        Ok(())
    }

    const HEAD_CANONICAL_ACTIVATION_VERSION: i64 = 1;
    const HEAD_CANONICAL_PROFILE_PIN_VERSION: i64 = 1;
    const HEAD_CANONICAL_ACTIVATION_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(25);

    fn head_canonical_profile_is_pinned(conn: &Connection) -> Result<bool, RuntimeStoreError> {
        let row = conn
            .query_row(
                r"
                SELECT pin_version, persistence_profile
                FROM runtime_head_canonical_profile_pin
                WHERE singleton = 1
                ",
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
        let Some((pin_version, profile)) = row else {
            return Ok(false);
        };
        if pin_version != HEAD_CANONICAL_PROFILE_PIN_VERSION
            || profile != RuntimeSessionPersistenceProfile::HeadCanonicalV1.to_string()
        {
            return Err(RuntimeStoreError::ReadFailed(format!(
                "unsupported SQLite runtime session profile pin version/profile: {pin_version}/{profile}"
            )));
        }
        Ok(true)
    }

    fn head_canonical_profile_has_durable_claim(
        conn: &Connection,
    ) -> Result<bool, RuntimeStoreError> {
        if head_canonical_profile_is_pinned(conn)? {
            return Ok(true);
        }
        conn.query_row(
            r"
            SELECT EXISTS (
                SELECT 1 FROM runtime_session_authority
                UNION ALL
                SELECT 1 FROM runtime_head_canonical_activations
            )
            ",
            [],
            |row| row.get::<_, bool>(0),
        )
        .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))
    }

    fn pin_head_canonical_profile(conn: &mut Connection) -> Result<(), RuntimeStoreError> {
        let tx = begin_runtime_transaction(conn)?;
        if head_canonical_profile_is_pinned(&tx)? {
            tx.commit()
                .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
            return Ok(());
        }
        let pinned_at_ms = unix_time_millis()?;
        tx.execute(
            r"
            INSERT INTO runtime_head_canonical_profile_pin (
                singleton, pin_version, persistence_profile, pinned_at_ms
            ) VALUES (1, ?1, ?2, ?3)
            ",
            params![
                HEAD_CANONICAL_PROFILE_PIN_VERSION,
                RuntimeSessionPersistenceProfile::HeadCanonicalV1.to_string(),
                pinned_at_ms,
            ],
        )
        .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
        tx.commit()
            .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
        tracing::info!(
            persistence_profile = %RuntimeSessionPersistenceProfile::HeadCanonicalV1,
            "committed irreversible SQLite runtime session profile pin"
        );
        Ok(())
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum HeadCanonicalActivationState {
        InProgress,
        Complete,
    }

    impl HeadCanonicalActivationState {
        fn parse(runtime_id: &LogicalRuntimeId, value: &str) -> Result<Self, RuntimeStoreError> {
            match value {
                "in_progress" => Ok(Self::InProgress),
                "complete" => Ok(Self::Complete),
                other => Err(session_authority_conflict(
                    runtime_id,
                    format!("unsupported head-canonical activation state '{other}'"),
                )),
            }
        }
    }

    #[derive(Debug)]
    struct StoreOwnedHeadCanonicalActivationSource {
        session: meerkat_core::Session,
        session_id: meerkat_core::types::SessionId,
        rewrite_prefix: meerkat_core::TranscriptRewritePrefixAccumulator,
        snapshot_token: String,
        snapshot_bytes: i64,
        message_count: i64,
    }

    #[derive(Debug)]
    struct HeadCanonicalActivationMarker {
        activation_version: i64,
        state: HeadCanonicalActivationState,
        session_id: String,
        source_snapshot_token: Option<String>,
        source_snapshot_bytes: Option<i64>,
        source_message_count: Option<i64>,
        started_at_ms: i64,
    }

    struct HeadCanonicalActivationHeartbeat {
        stopped: Arc<AtomicBool>,
        phase: Arc<AtomicU8>,
        handle: Option<std::thread::JoinHandle<()>>,
    }

    impl HeadCanonicalActivationHeartbeat {
        fn start(
            runtime_id: &LogicalRuntimeId,
            session_id: String,
        ) -> Result<Self, RuntimeStoreError> {
            let stopped = Arc::new(AtomicBool::new(false));
            let phase = Arc::new(AtomicU8::new(0));
            let thread_stopped = Arc::clone(&stopped);
            let thread_phase = Arc::clone(&phase);
            let runtime_id = runtime_id_text(runtime_id).to_owned();
            let started = Instant::now();
            let handle = std::thread::Builder::new()
                .name("rkat-hc-activation-heartbeat".to_string())
                .spawn(move || {
                    while !thread_stopped.load(Ordering::Acquire) {
                        std::thread::park_timeout(HEAD_CANONICAL_ACTIVATION_HEARTBEAT_INTERVAL);
                        if thread_stopped.load(Ordering::Acquire) {
                            break;
                        }
                        tracing::info!(
                            runtime_id = %runtime_id,
                            session_id = %session_id,
                            phase = activation_phase_label(thread_phase.load(Ordering::Acquire)),
                            elapsed_ms = u64::try_from(started.elapsed().as_millis())
                                .unwrap_or(u64::MAX),
                            "head-canonical profile activation is still in progress"
                        );
                    }
                })
                .map_err(|error| {
                    RuntimeStoreError::Internal(format!(
                        "failed to spawn head-canonical activation heartbeat: {error}"
                    ))
                })?;
            Ok(Self {
                stopped,
                phase,
                handle: Some(handle),
            })
        }

        fn set_phase(
            &self,
            runtime_id: &LogicalRuntimeId,
            session_id: &meerkat_core::types::SessionId,
            phase: u8,
            source_message_count: Option<i64>,
        ) {
            self.phase.store(phase, Ordering::Release);
            tracing::info!(
                runtime_id = %runtime_id,
                session_id = %session_id,
                phase = activation_phase_label(phase),
                source_message_count = ?source_message_count,
                "head-canonical profile activation phase started"
            );
        }

        fn stop(&mut self) {
            self.stopped.store(true, Ordering::Release);
            if let Some(handle) = self.handle.take() {
                handle.thread().unpark();
                if handle.join().is_err() {
                    tracing::warn!(
                        "head-canonical profile activation heartbeat thread terminated unexpectedly"
                    );
                }
            }
        }
    }

    impl Drop for HeadCanonicalActivationHeartbeat {
        fn drop(&mut self) {
            self.stop();
        }
    }

    fn activation_phase_label(phase: u8) -> &'static str {
        match phase {
            0 => "commit_in_progress_marker",
            1 => "verify_frozen_snapshot",
            2 => "canonicalize_physical_session",
            3 => "derive_exact_runtime_boundary",
            4 => "commit_authority_and_completion",
            _ => "unknown",
        }
    }

    fn unix_time_millis() -> Result<i64, RuntimeStoreError> {
        let elapsed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| {
                RuntimeStoreError::Internal(format!(
                    "system clock is before the Unix epoch during profile activation: {error}"
                ))
            })?;
        i64::try_from(elapsed.as_millis()).map_err(|_| {
            RuntimeStoreError::Internal(
                "system clock millisecond value exceeds SQLite INTEGER".to_string(),
            )
        })
    }

    fn activation_source_snapshot_token(bytes: &[u8]) -> String {
        use sha2::Digest as _;

        format!("sha256:{:x}", sha2::Sha256::digest(bytes))
    }

    fn load_head_canonical_activation_marker(
        conn: &Connection,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<HeadCanonicalActivationMarker>, RuntimeStoreError> {
        let row = conn
            .query_row(
                r"
                SELECT activation_version, state, session_id,
                       source_snapshot_token, source_snapshot_bytes,
                       source_message_count, started_at_ms
                FROM runtime_head_canonical_activations
                WHERE runtime_id = ?1
                ",
                params![runtime_id_text(runtime_id)],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<i64>>(4)?,
                        row.get::<_, Option<i64>>(5)?,
                        row.get::<_, i64>(6)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
        row.map(
            |(
                activation_version,
                state,
                session_id,
                source_snapshot_token,
                source_snapshot_bytes,
                source_message_count,
                started_at_ms,
            )| {
                if activation_version != HEAD_CANONICAL_ACTIVATION_VERSION {
                    return Err(session_authority_conflict(
                        runtime_id,
                        format!(
                            "unsupported head-canonical activation version {activation_version}"
                        ),
                    ));
                }
                Ok(HeadCanonicalActivationMarker {
                    activation_version,
                    state: HeadCanonicalActivationState::parse(runtime_id, &state)?,
                    session_id,
                    source_snapshot_token,
                    source_snapshot_bytes,
                    source_message_count,
                    started_at_ms,
                })
            },
        )
        .transpose()
    }

    fn load_store_owned_head_canonical_activation_source_in_txn(
        tx: &Transaction<'_>,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<StoreOwnedHeadCanonicalActivationSource, RuntimeStoreError> {
        let authority = load_whole_blob_store_authority(tx, runtime_id)?.ok_or_else(|| {
            session_authority_conflict(
                runtime_id,
                "head-canonical activation requires imported WholeBlob store authority",
            )
        })?;
        let snapshot = tx
            .query_row(
                "SELECT session_snapshot FROM runtime_whole_blob_bodies WHERE blob_sha256 = ?1",
                params![authority.blob_sha256()],
                |row| Ok(row.get::<_, JsonColumnBytes>(0)?.into_bytes()),
            )
            .optional()
            .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
        let Some(snapshot) = snapshot else {
            return Err(session_authority_conflict(
                runtime_id,
                "head-canonical activation WholeBlob authority references a missing body",
            ));
        };
        use sha2::Digest as _;
        let observed_blob_sha256 = format!("row-sha256:{:x}", sha2::Sha256::digest(&snapshot));
        if authority.blob_sha256() != observed_blob_sha256 {
            return Err(session_authority_conflict(
                runtime_id,
                "head-canonical activation WholeBlob body differs from store-issued authority",
            ));
        }
        let session = deserialize_persisted_session(&snapshot)?;
        let session_id = session.id().clone();
        let owner = LogicalRuntimeId::for_session(&session_id);
        if &owner != runtime_id || authority.session_id() != &session_id {
            return Err(session_authority_conflict(
                runtime_id,
                format!(
                    "head-canonical activation body belongs to runtime {owner}, not {runtime_id}"
                ),
            ));
        }
        let rewrite_prefix = match session.transcript_rewrite_prefix_authority() {
            Some(prefix) => prefix,
            None => session
                .transcript_history_state_shared()
                .map_err(|error| {
                    session_authority_conflict(
                        runtime_id,
                        format!(
                            "store-owned WholeBlob rewrite-prefix authority is malformed: {error}"
                        ),
                    )
                })?
                .map_or_else(Default::default, |history| history.rewrite_prefix().clone()),
        };
        let snapshot_bytes = i64::try_from(snapshot.len()).map_err(|_| {
            session_authority_conflict(
                runtime_id,
                "store-owned WholeBlob predecessor exceeds SQLite INTEGER byte accounting",
            )
        })?;
        let message_count = i64::try_from(session.messages().len()).map_err(|_| {
            session_authority_conflict(
                runtime_id,
                "store-owned WholeBlob predecessor exceeds SQLite INTEGER row accounting",
            )
        })?;
        Ok(StoreOwnedHeadCanonicalActivationSource {
            session,
            session_id,
            rewrite_prefix,
            snapshot_token: activation_source_snapshot_token(&snapshot),
            snapshot_bytes,
            message_count,
        })
    }

    fn validate_activation_source_against_authority(
        runtime_id: &LogicalRuntimeId,
        source: &StoreOwnedHeadCanonicalActivationSource,
        authority: Option<&RuntimeSessionAuthority>,
    ) -> Result<(), RuntimeStoreError> {
        let Some(authority) = authority else {
            return Ok(());
        };
        if authority.session_id() != &source.session_id {
            return Err(session_authority_conflict(
                runtime_id,
                "store-owned WholeBlob predecessor differs from persisted runtime authority",
            ));
        }
        Ok(())
    }

    fn validate_in_progress_activation_marker(
        runtime_id: &LogicalRuntimeId,
        marker: &HeadCanonicalActivationMarker,
        expected_session_id: &meerkat_core::types::SessionId,
    ) -> Result<(), RuntimeStoreError> {
        if marker.activation_version != HEAD_CANONICAL_ACTIVATION_VERSION
            || marker.state != HeadCanonicalActivationState::InProgress
            || marker.session_id != expected_session_id.to_string()
            || marker.source_snapshot_token.is_some()
            || marker.source_snapshot_bytes.is_some()
            || marker.source_message_count.is_some()
        {
            return Err(session_authority_conflict(
                runtime_id,
                "in-progress head-canonical activation marker is not the exact marker-first shape",
            ));
        }
        Ok(())
    }

    const LOAD_HEAD_CANONICAL_ACTIVATION_CANDIDATES_SQL: &str = r"
SELECT runtime_id
FROM runtime_head_canonical_activations
WHERE state = 'in_progress'
UNION
SELECT runtime_id
FROM runtime_head_canonical_activation_queue
ORDER BY runtime_id";

    fn head_canonical_activation_candidate_ids(
        conn: &Connection,
    ) -> Result<Vec<LogicalRuntimeId>, RuntimeStoreError> {
        // A complete marker is an immutable migration receipt, not current
        // authority. Ordinary boundaries advance runtime_session_authority
        // after activation, so completed receipts and installed authorities
        // must never be startup work. Migration v2 materializes the published
        // v1 whole-BLOB set once; ordinary reopen therefore touches only
        // queued work and interrupted activations, never retained history.
        let mut statement = conn
            .prepare(LOAD_HEAD_CANONICAL_ACTIVATION_CANDIDATES_SQL)
            .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
        let runtime_ids = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
        Ok(runtime_ids.into_iter().map(LogicalRuntimeId::new).collect())
    }

    fn head_canonical_activation_error_class(error: &RuntimeStoreError) -> &'static str {
        match error {
            RuntimeStoreError::SessionPersistenceAuthorityConflict { .. } => "authority_conflict",
            RuntimeStoreError::SchemaFromTheFuture { .. } => "schema_from_the_future",
            RuntimeStoreError::MaintenanceFenceHeld { .. } => "maintenance_fence",
            RuntimeStoreError::ReadFailed(_) => "read_failed",
            RuntimeStoreError::WriteFailed(_) => "write_failed",
            _ => "activation_failed",
        }
    }

    fn activate_one_head_canonical_runtime(
        conn: &mut Connection,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<(), RuntimeStoreError> {
        let session_label = runtime_id
            .session_id()
            .map(|session_id| session_id.to_string())
            .unwrap_or_else(|| "<unverified>".to_string());
        tracing::info!(
            runtime_id = %runtime_id,
            session_id = %session_label,
            phase = activation_phase_label(0),
            "starting synchronous head-canonical profile activation"
        );
        let process_started = Instant::now();
        let mut heartbeat =
            match HeadCanonicalActivationHeartbeat::start(runtime_id, session_label.clone()) {
                Ok(heartbeat) => heartbeat,
                Err(error) => {
                    tracing::error!(
                        runtime_id = %runtime_id,
                        session_id = %session_label,
                        phase = activation_phase_label(0),
                        source_message_count = ?Option::<i64>::None,
                        boundary_message_count = ?Option::<i64>::None,
                        physical_message_count = ?Option::<i64>::None,
                        elapsed_ms = u64::try_from(process_started.elapsed().as_millis())
                            .unwrap_or(u64::MAX),
                        refusal_class = head_canonical_activation_error_class(&error),
                        "refused synchronous head-canonical profile activation"
                    );
                    return Err(error);
                }
            };

        let mut observed_session_id = None;
        let mut observed_source_message_count = None;
        let mut observed_boundary_message_count = None;
        let mut observed_physical_message_count = None;
        let result = (|| {
            let expected_session_id = runtime_id.session_id().ok_or_else(|| {
                session_authority_conflict(
                    runtime_id,
                    "head-canonical activation runtime id does not encode a session identity",
                )
            })?;
            if &LogicalRuntimeId::for_session(&expected_session_id) != runtime_id {
                return Err(session_authority_conflict(
                    runtime_id,
                    "head-canonical activation requires the canonical session runtime identity",
                ));
            }

            // Commit the observable/fencing marker before touching document
            // bytes. Its trigger freezes this runtime's predecessor row, so
            // the one conversion transaction below can read and verify A
            // exactly once without a marker-to-source race.
            let started_at_ms = {
                let tx = begin_runtime_transaction(conn)?;
                let whole_blob_authority = load_whole_blob_store_authority(&tx, runtime_id)?
                    .ok_or_else(|| {
                        session_authority_conflict(
                            runtime_id,
                            "head-canonical activation requires store-owned WholeBlob authority",
                        )
                    })?;
                if whole_blob_authority.session_id() != &expected_session_id {
                    return Err(session_authority_conflict(
                        runtime_id,
                        "WholeBlob activation authority belongs to another session",
                    ));
                }
                let authority = load_head_canonical_authority(&tx, runtime_id)?;
                let authority_session_id =
                    authority.as_ref().map(RuntimeSessionAuthority::session_id);
                if authority_session_id.is_some_and(|session_id| session_id != &expected_session_id)
                {
                    return Err(session_authority_conflict(
                        runtime_id,
                        "persisted activation authority belongs to another session",
                    ));
                }
                let marker = load_head_canonical_activation_marker(&tx, runtime_id)?;
                let now_ms = unix_time_millis()?;
                let started_at_ms = match marker {
                    None => {
                        tx.execute(
                            r"
                            INSERT INTO runtime_head_canonical_activations (
                                runtime_id, activation_version, state, session_id,
                                started_at_ms, updated_at_ms
                            ) VALUES (?1, ?2, 'in_progress', ?3, ?4, ?4)
                            ",
                            params![
                                runtime_id_text(runtime_id),
                                HEAD_CANONICAL_ACTIVATION_VERSION,
                                expected_session_id.to_string(),
                                now_ms,
                            ],
                        )
                        .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
                        now_ms
                    }
                    Some(marker) => {
                        if marker.state == HeadCanonicalActivationState::Complete {
                            return Err(session_authority_conflict(
                                runtime_id,
                                "complete head-canonical activation marker has no exact authority",
                            ));
                        }
                        validate_in_progress_activation_marker(
                            runtime_id,
                            &marker,
                            &expected_session_id,
                        )?;
                        let changed = tx
                            .execute(
                                r"
                                UPDATE runtime_head_canonical_activations
                                SET updated_at_ms = ?2
                                WHERE runtime_id = ?1 AND state = 'in_progress'
                                ",
                                params![
                                    runtime_id_text(runtime_id),
                                    now_ms.max(marker.started_at_ms),
                                ],
                            )
                            .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
                        if changed != 1 {
                            return Err(session_authority_conflict(
                                runtime_id,
                                "in-progress activation marker changed before retry admission",
                            ));
                        }
                        marker.started_at_ms
                    }
                };
                tx.commit()
                    .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
                started_at_ms
            };

            heartbeat.set_phase(runtime_id, &expected_session_id, 1, None);
            let tx = begin_runtime_transaction(conn)?;
            let current_source =
                load_store_owned_head_canonical_activation_source_in_txn(&tx, runtime_id)?;
            observed_session_id = Some(current_source.session_id.to_string());
            observed_source_message_count = Some(current_source.message_count);
            let source_message_count = Some(current_source.message_count);
            let marker =
                load_head_canonical_activation_marker(&tx, runtime_id)?.ok_or_else(|| {
                    session_authority_conflict(
                        runtime_id,
                        "head-canonical activation marker disappeared before conversion",
                    )
                })?;
            validate_in_progress_activation_marker(
                runtime_id,
                &marker,
                &current_source.session_id,
            )?;
            let existing_authority = load_head_canonical_authority(&tx, runtime_id)?;
            validate_activation_source_against_authority(
                runtime_id,
                &current_source,
                existing_authority.as_ref(),
            )?;
            heartbeat.set_phase(
                runtime_id,
                &current_source.session_id,
                2,
                source_message_count,
            );
            let Some((physical_head, physical_head_token)) =
                meerkat_store::sqlite_store::ensure_head_canonical_for_runtime_in_txn(
                    &tx,
                    &current_source.session_id,
                )
                .map_err(|error| map_head_canonical_session_store_error(runtime_id, error))?
            else {
                return Err(session_authority_conflict(
                    runtime_id,
                    "the co-tenant session store has no physical session to canonicalize",
                ));
            };
            observed_physical_message_count = i64::try_from(physical_head.message_count).ok();

            heartbeat.set_phase(
                runtime_id,
                &current_source.session_id,
                3,
                source_message_count,
            );
            let boundary_head =
                meerkat_store::sqlite_store::derive_runtime_boundary_head_for_activation_in_txn(
                    &tx,
                    &current_source.session,
                    &current_source.rewrite_prefix,
                    &physical_head,
                )
                .map_err(|error| map_head_canonical_session_store_error(runtime_id, error))?;
            observed_boundary_message_count = i64::try_from(boundary_head.message_count).ok();
            if boundary_head.rewrite_prefix != current_source.rewrite_prefix {
                return Err(session_authority_conflict(
                    runtime_id,
                    "derived runtime boundary rewrite authority differs from the store-owned predecessor",
                ));
            }
            let successor_authority =
                issue_head_canonical_authority_in_txn(&tx, runtime_id, boundary_head)?;
            if let Some(existing) = existing_authority.as_ref()
                && existing != &successor_authority
            {
                return Err(session_authority_conflict(
                    runtime_id,
                    "existing exact runtime authority differs from the reconstructed frozen boundary",
                ));
            }

            heartbeat.set_phase(
                runtime_id,
                &current_source.session_id,
                4,
                source_message_count,
            );
            write_head_canonical_authority_in_txn(&tx, runtime_id, &successor_authority)?;
            let runtime_state = load_runtime_session_catalog_entry_in_txn(&tx, runtime_id)?
                .and_then(|entry| entry.runtime_state());
            let catalog_entry = crate::store::RuntimeSessionCatalogEntry::from_head(
                successor_authority
                    .head_canonical()
                    .ok_or_else(|| {
                        session_authority_conflict(
                            runtime_id,
                            "activation successor authority is not HeadCanonical",
                        )
                    })?
                    .boundary_head(),
                RuntimeSessionPersistenceProfile::HeadCanonicalV1,
                runtime_state,
            )?;
            upsert_runtime_session_catalog_entry_in_txn(&tx, runtime_id, &catalog_entry)?;
            let completed_at_ms = unix_time_millis()?.max(started_at_ms);
            let elapsed_ms = completed_at_ms.saturating_sub(started_at_ms);
            let successor_boundary_head = successor_authority
                .head_canonical()
                .ok_or_else(|| {
                    session_authority_conflict(
                        runtime_id,
                        "reconstructed authority is not HeadCanonical",
                    )
                })?
                .boundary_head();
            let boundary_message_count = i64::try_from(successor_boundary_head.message_count)
                .map_err(|_| {
                    session_authority_conflict(
                        runtime_id,
                        "runtime boundary row count exceeds SQLite INTEGER",
                    )
                })?;
            let physical_message_count =
                i64::try_from(physical_head.message_count).map_err(|_| {
                    session_authority_conflict(
                        runtime_id,
                        "physical head row count exceeds SQLite INTEGER",
                    )
                })?;
            let boundary_head_token = successor_authority
                .head_canonical()
                .ok_or_else(|| {
                    session_authority_conflict(
                        runtime_id,
                        "reconstructed authority is not HeadCanonical",
                    )
                })?
                .committed_head_token();
            let changed = tx
                .execute(
                    r"
                    UPDATE runtime_head_canonical_activations
                    SET state = 'complete',
                        updated_at_ms = ?2,
                        completed_at_ms = ?2,
                        elapsed_ms = ?3,
                        source_snapshot_token = ?4,
                        source_snapshot_bytes = ?5,
                        source_message_count = ?6,
                        boundary_message_count = ?7,
                        physical_message_count = ?8,
                        boundary_head_cas_token = ?9,
                        physical_head_cas_token = ?10
                    WHERE runtime_id = ?1
                      AND state = 'in_progress'
                      AND session_id = ?11
                    ",
                    params![
                        runtime_id_text(runtime_id),
                        completed_at_ms,
                        elapsed_ms,
                        current_source.snapshot_token,
                        current_source.snapshot_bytes,
                        current_source.message_count,
                        boundary_message_count,
                        physical_message_count,
                        boundary_head_token,
                        physical_head_token,
                        current_source.session_id.to_string(),
                    ],
                )
                .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
            if changed != 1 {
                return Err(session_authority_conflict(
                    runtime_id,
                    "head-canonical activation marker changed before completion",
                ));
            }
            tx.commit()
                .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
            Ok((
                current_source.session_id,
                current_source.message_count,
                boundary_message_count,
                physical_message_count,
                elapsed_ms,
            ))
        })();

        heartbeat.stop();
        match result {
            Ok((
                session_id,
                source_message_count,
                boundary_message_count,
                physical_message_count,
                elapsed_ms,
            )) => {
                tracing::info!(
                    runtime_id = %runtime_id,
                    session_id = %session_id,
                    source_message_count,
                    boundary_message_count,
                    physical_message_count,
                    elapsed_ms,
                    "committed synchronous head-canonical profile activation"
                );
                Ok(())
            }
            Err(error) => {
                tracing::error!(
                    runtime_id = %runtime_id,
                    session_id = %observed_session_id.as_deref().unwrap_or(&session_label),
                    phase = activation_phase_label(heartbeat.phase.load(Ordering::Acquire)),
                    source_message_count = ?observed_source_message_count,
                    boundary_message_count = ?observed_boundary_message_count,
                    physical_message_count = ?observed_physical_message_count,
                    elapsed_ms = u64::try_from(process_started.elapsed().as_millis())
                        .unwrap_or(u64::MAX),
                    refusal_class = head_canonical_activation_error_class(&error),
                    "refused synchronous head-canonical profile activation"
                );
                Err(error)
            }
        }
    }

    fn activate_head_canonical_profiles(conn: &mut Connection) -> Result<(), RuntimeStoreError> {
        for runtime_id in head_canonical_activation_candidate_ids(conn)? {
            let session_label = runtime_id
                .session_id()
                .map(|session_id| session_id.to_string())
                .unwrap_or_else(|| "<unverified>".to_string());
            let authority =
                load_head_canonical_authority(conn, &runtime_id).inspect_err(|error| {
                    tracing::error!(
                        runtime_id = %runtime_id,
                        session_id = %session_label,
                        phase = "activation_preflight",
                        source_message_count = ?Option::<i64>::None,
                        boundary_message_count = ?Option::<i64>::None,
                        physical_message_count = ?Option::<i64>::None,
                        elapsed_ms = 0_u64,
                        refusal_class = head_canonical_activation_error_class(&error),
                        "refused synchronous head-canonical profile activation"
                    );
                })?;
            let marker =
                load_head_canonical_activation_marker(conn, &runtime_id).inspect_err(|error| {
                    tracing::error!(
                        runtime_id = %runtime_id,
                        session_id = %session_label,
                        phase = "activation_preflight",
                        source_message_count = ?Option::<i64>::None,
                        boundary_message_count = ?Option::<i64>::None,
                        physical_message_count = ?Option::<i64>::None,
                        elapsed_ms = 0_u64,
                        refusal_class = head_canonical_activation_error_class(&error),
                        "refused synchronous head-canonical profile activation"
                    );
                })?;
            match (
                authority.as_ref(),
                marker.as_ref().map(|marker| marker.state),
            ) {
                (Some(_), None | Some(HeadCanonicalActivationState::Complete)) => continue,
                (None, Some(HeadCanonicalActivationState::Complete)) => {
                    return Err(session_authority_conflict(
                        &runtime_id,
                        "complete head-canonical activation receipt has no exact installed authority",
                    ));
                }
                (Some(_), Some(HeadCanonicalActivationState::InProgress)) | (None, _) => {
                    activate_one_head_canonical_runtime(conn, &runtime_id)?;
                }
            }
        }
        Ok(())
    }

    fn load_boundary_receipt(
        conn: &Connection,
        runtime_id: &LogicalRuntimeId,
        receipt: &RunBoundaryReceipt,
    ) -> Result<Option<RunBoundaryReceipt>, RuntimeStoreError> {
        conn.query_row(
            r"
            SELECT receipt_json
            FROM runtime_boundary_receipts
            WHERE runtime_id = ?1 AND run_id = ?2 AND sequence = ?3
            ",
            params![
                runtime_id_text(runtime_id),
                receipt.run_id.0.to_string(),
                encode_receipt_sequence(receipt.sequence),
            ],
            |row| Ok(row.get::<_, JsonColumnBytes>(0)?.into_bytes()),
        )
        .optional()
        .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?
        .map(|bytes| {
            serde_json::from_slice(&bytes)
                .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))
        })
        .transpose()
    }

    #[derive(Debug, Clone)]
    struct OrdinaryBoundaryWitness {
        boundary_key: String,
        request_digest: String,
    }

    fn hash_ordinary_boundary_component(hasher: &mut sha2::Sha256, label: &str, bytes: &[u8]) {
        use sha2::Digest as _;

        hasher.update((label.len() as u64).to_be_bytes());
        hasher.update(label.as_bytes());
        hasher.update((bytes.len() as u64).to_be_bytes());
        hasher.update(bytes);
    }

    fn hash_ordinary_boundary_json<T: serde::Serialize>(
        hasher: &mut sha2::Sha256,
        label: &str,
        value: &T,
    ) -> Result<(), RuntimeStoreError> {
        let bytes = serde_json::to_vec(value)
            .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
        hash_ordinary_boundary_component(hasher, label, &bytes);
        Ok(())
    }

    fn prepare_ordinary_boundary_witness(
        runtime_id: &LogicalRuntimeId,
        prepared_session: Option<&PreparedHeadCanonicalSqliteSession>,
        receipt: Option<&RunBoundaryReceipt>,
        lifecycle_expected: Option<&MachineLifecycleExpectedVersion>,
        lifecycle_snapshot: Option<&MachineLifecycleSnapshot>,
        input_updates: &[(StoredInputState, Option<String>)],
    ) -> Result<OrdinaryBoundaryWitness, RuntimeStoreError> {
        use sha2::Digest as _;

        let boundary_key = if let Some(receipt) = receipt {
            format!(
                "receipt:{}:{}",
                receipt.run_id.0,
                encode_receipt_sequence(receipt.sequence)
            )
        } else if let Some(prepared) = prepared_session {
            format!("session-head:{}", prepared.mutation.successor_head_token())
        } else {
            return Err(session_authority_conflict(
                runtime_id,
                "ordinary boundary has neither session authority nor receipt identity",
            ));
        };

        let mut hasher = sha2::Sha256::new();
        hash_ordinary_boundary_component(
            &mut hasher,
            "witness-domain",
            b"meerkat-runtime/sqlite/ordinary-boundary/v1",
        );
        hash_ordinary_boundary_component(
            &mut hasher,
            "runtime-id",
            runtime_id_text(runtime_id).as_bytes(),
        );
        hash_ordinary_boundary_component(&mut hasher, "boundary-key", boundary_key.as_bytes());

        match prepared_session {
            Some(prepared) => {
                hash_ordinary_boundary_component(&mut hasher, "session-present", b"1");
                match prepared.mutation.predecessor_head() {
                    Some(head) => {
                        hash_ordinary_boundary_json(
                            &mut hasher,
                            "physical-predecessor-head",
                            head,
                        )?;
                    }
                    None => hash_ordinary_boundary_component(
                        &mut hasher,
                        "physical-predecessor-head",
                        b"absent",
                    ),
                }
                hash_ordinary_boundary_json(
                    &mut hasher,
                    "physical-successor-head",
                    prepared.mutation.successor_head(),
                )?;
                hash_ordinary_boundary_component(
                    &mut hasher,
                    "physical-successor-token",
                    prepared.mutation.successor_head_token().as_bytes(),
                );
                match &prepared.mutation {
                    meerkat_core::lifecycle::core_executor::PreparedHeadCanonicalPhysicalMutation::Ordinary(
                        mutation,
                    ) => {
                        hash_ordinary_boundary_component(
                            &mut hasher,
                            "suffix-base-seq",
                            &mutation.base_seq().to_be_bytes(),
                        );
                        hash_ordinary_boundary_component(
                            &mut hasher,
                            "suffix-count",
                            &(mutation.serialized_suffix().len() as u64).to_be_bytes(),
                        );
                        for row in mutation.serialized_suffix() {
                            hash_ordinary_boundary_component(&mut hasher, "suffix-row", row);
                        }
                    }
                    meerkat_core::lifecycle::core_executor::PreparedHeadCanonicalPhysicalMutation::Rewrite(
                        mutation,
                    ) => {
                        hash_ordinary_boundary_component(
                            &mut hasher,
                            "physical-mutation-kind",
                            b"rewrite",
                        );
                        for step in mutation.steps() {
                            match step.parent_transition() {
                                meerkat_core::session_store::PreparedHeadCanonicalParentTransition::ExactAppend => {
                                    hash_ordinary_boundary_component(
                                        &mut hasher,
                                        "rewrite-parent-transition",
                                        b"exact-append",
                                    );
                                }
                                meerkat_core::session_store::PreparedHeadCanonicalParentTransition::ExactSplice(
                                    splice,
                                ) => {
                                    hash_ordinary_boundary_component(
                                        &mut hasher,
                                        "rewrite-parent-transition",
                                        b"exact-splice",
                                    );
                                    hash_ordinary_boundary_component(
                                        &mut hasher,
                                        "rewrite-parent-splice-source",
                                        splice.source_strand().as_str().as_bytes(),
                                    );
                                    hash_ordinary_boundary_json(
                                        &mut hasher,
                                        "rewrite-parent-splice",
                                        &splice.link_splice(),
                                    )?;
                                    for row in splice.serialized_replacement() {
                                        hash_ordinary_boundary_component(
                                            &mut hasher,
                                            "rewrite-parent-splice-row",
                                            row,
                                        );
                                    }
                                }
                            }
                            hash_ordinary_boundary_json(
                                &mut hasher,
                                "rewrite-commit",
                                step.commit(),
                            )?;
                            hash_ordinary_boundary_component(
                                &mut hasher,
                                "rewrite-parent-strand",
                                step.parent_strand().as_str().as_bytes(),
                            );
                            hash_ordinary_boundary_component(
                                &mut hasher,
                                "rewrite-parent-base",
                                &step.parent_base_seq().to_be_bytes(),
                            );
                            for row in step.serialized_parent_suffix() {
                                hash_ordinary_boundary_component(
                                    &mut hasher,
                                    "rewrite-parent-row",
                                    row,
                                );
                            }
                            hash_ordinary_boundary_component(
                                &mut hasher,
                                "rewrite-strand",
                                step.strand().as_str().as_bytes(),
                            );
                            hash_ordinary_boundary_json(
                                &mut hasher,
                                "rewrite-link-splice",
                                &step.link_splice(),
                            )?;
                            for row in step.serialized_replacement() {
                                hash_ordinary_boundary_component(
                                    &mut hasher,
                                    "rewrite-replacement-row",
                                    row,
                                );
                            }
                        }
                        hash_ordinary_boundary_component(
                            &mut hasher,
                            "rewrite-tail-base",
                            &mutation.tail_base_seq().to_be_bytes(),
                        );
                        for row in mutation.serialized_tail() {
                            hash_ordinary_boundary_component(
                                &mut hasher,
                                "rewrite-tail-row",
                                row,
                            );
                        }
                    }
                }
                hash_ordinary_boundary_component(
                    &mut hasher,
                    "compaction-intent-count",
                    &(prepared.compaction_intents.len() as u64).to_be_bytes(),
                );
                for intent in &prepared.compaction_intents {
                    hash_ordinary_boundary_json(&mut hasher, "compaction-intent", intent)?;
                }
            }
            None => {
                hash_ordinary_boundary_component(&mut hasher, "session-present", b"0");
            }
        }

        match receipt {
            Some(receipt) => {
                hash_ordinary_boundary_json(&mut hasher, "receipt", receipt)?;
            }
            None => {
                hash_ordinary_boundary_component(&mut hasher, "receipt", b"absent");
            }
        }

        match lifecycle_expected {
            Some(MachineLifecycleExpectedVersion::Missing) => {
                hash_ordinary_boundary_component(&mut hasher, "lifecycle-expected", b"missing-row");
            }
            Some(MachineLifecycleExpectedVersion::Version(version)) => {
                hash_ordinary_boundary_component(
                    &mut hasher,
                    "lifecycle-expected",
                    version.as_str().as_bytes(),
                );
            }
            None => {
                hash_ordinary_boundary_component(
                    &mut hasher,
                    "lifecycle-expected",
                    b"not-applicable",
                );
            }
        }
        match lifecycle_snapshot {
            Some(snapshot) => {
                let bytes = MachineLifecycleStoreRecord::from_snapshot(snapshot).encode()?;
                hash_ordinary_boundary_component(&mut hasher, "lifecycle-target", &bytes);
            }
            None => {
                hash_ordinary_boundary_component(&mut hasher, "lifecycle-target", b"absent");
            }
        }

        hash_ordinary_boundary_component(
            &mut hasher,
            "input-update-count",
            &(input_updates.len() as u64).to_be_bytes(),
        );
        for (input, expected) in input_updates {
            hash_ordinary_boundary_json(&mut hasher, "input-target", input)?;
            hash_ordinary_boundary_component(
                &mut hasher,
                "input-expected",
                expected.as_deref().unwrap_or("not-fenced").as_bytes(),
            );
        }

        Ok(OrdinaryBoundaryWitness {
            boundary_key,
            request_digest: format!("sha256:{:x}", hasher.finalize()),
        })
    }

    fn prepare_head_canonical_promotion_witness(
        runtime_id: &LogicalRuntimeId,
        authority: &HeadCanonicalProvisionalTailAuthority,
        receipt: &RunBoundaryReceipt,
        lifecycle_expected: Option<&MachineLifecycleExpectedVersion>,
        lifecycle_snapshot: Option<&MachineLifecycleSnapshot>,
        input_updates: &[(StoredInputState, Option<String>)],
    ) -> Result<OrdinaryBoundaryWitness, RuntimeStoreError> {
        use sha2::Digest as _;

        let base = prepare_ordinary_boundary_witness(
            runtime_id,
            None,
            Some(receipt),
            lifecycle_expected,
            lifecycle_snapshot,
            input_updates,
        )?;
        let mut hasher = sha2::Sha256::new();
        hash_ordinary_boundary_component(
            &mut hasher,
            "witness-domain",
            b"meerkat-runtime/sqlite/head-canonical-promotion/v1",
        );
        hash_ordinary_boundary_component(
            &mut hasher,
            "ordinary-effects-digest",
            base.request_digest.as_bytes(),
        );
        hash_ordinary_boundary_component(
            &mut hasher,
            "authority-version",
            &authority.authority_version().to_be_bytes(),
        );
        hash_ordinary_boundary_component(
            &mut hasher,
            "session-id",
            authority.session_id().to_string().as_bytes(),
        );
        hash_ordinary_boundary_component(
            &mut hasher,
            "base-store-revision",
            &authority.base_store_revision().to_be_bytes(),
        );
        hash_ordinary_boundary_component(
            &mut hasher,
            "base-head-token",
            authority.base_committed_head_token().as_bytes(),
        );
        hash_ordinary_boundary_component(
            &mut hasher,
            "physical-store-revision",
            &authority.physical_store_revision().to_be_bytes(),
        );
        hash_ordinary_boundary_component(
            &mut hasher,
            "physical-head-token",
            authority.physical_head_token().as_bytes(),
        );
        hash_ordinary_boundary_component(
            &mut hasher,
            "candidate-sequence",
            &authority.candidate_sequence().to_be_bytes(),
        );
        Ok(OrdinaryBoundaryWitness {
            boundary_key: base.boundary_key,
            request_digest: format!("sha256:{:x}", hasher.finalize()),
        })
    }

    fn load_ordinary_boundary_witness(
        conn: &Connection,
        runtime_id: &LogicalRuntimeId,
        boundary_key: &str,
    ) -> Result<Option<String>, RuntimeStoreError> {
        conn.query_row(
            r"
            SELECT witness_version, request_digest
            FROM runtime_session_boundary_witnesses
            WHERE runtime_id = ?1 AND boundary_key = ?2
            ",
            params![runtime_id_text(runtime_id), boundary_key],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?
        .map(|(version, digest)| {
            if version != 1 {
                return Err(session_authority_conflict(
                    runtime_id,
                    format!("unsupported ordinary boundary witness version {version}"),
                ));
            }
            if digest.is_empty() {
                return Err(session_authority_conflict(
                    runtime_id,
                    "ordinary boundary witness has an empty request digest",
                ));
            }
            Ok(digest)
        })
        .transpose()
    }

    fn insert_ordinary_boundary_witness_in_txn(
        tx: &Transaction<'_>,
        runtime_id: &LogicalRuntimeId,
        witness: &OrdinaryBoundaryWitness,
    ) -> Result<(), RuntimeStoreError> {
        tx.execute(
            r"
            INSERT INTO runtime_session_boundary_witnesses (
                runtime_id, boundary_key, witness_version, request_digest
            ) VALUES (?1, ?2, 1, ?3)
            ",
            params![
                runtime_id_text(runtime_id),
                witness.boundary_key,
                witness.request_digest,
            ],
        )
        .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
        Ok(())
    }

    fn load_released_0810_boundary_receipt_marker(
        tx: &Transaction<'_>,
        runtime_id: &LogicalRuntimeId,
        receipt: &RunBoundaryReceipt,
    ) -> Result<Option<String>, RuntimeStoreError> {
        use sha2::Digest as _;

        let marker = tx
            .query_row(
                r"
                SELECT receipt_sha256
                FROM runtime_released_0810_boundary_receipts
                WHERE runtime_id = ?1 AND run_id = ?2 AND sequence = ?3
                ",
                params![
                    runtime_id_text(runtime_id),
                    receipt.run_id.0.to_string(),
                    encode_receipt_sequence(receipt.sequence),
                ],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
        let Some(marker) = marker else {
            return Ok(None);
        };
        let current = tx
            .query_row(
                r"
                SELECT receipt_json
                FROM runtime_boundary_receipts
                WHERE runtime_id = ?1 AND run_id = ?2 AND sequence = ?3
                ",
                params![
                    runtime_id_text(runtime_id),
                    receipt.run_id.0.to_string(),
                    encode_receipt_sequence(receipt.sequence),
                ],
                |row| Ok(row.get::<_, JsonColumnBytes>(0)?.into_bytes()),
            )
            .optional()
            .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
        let Some(current) = current else {
            return Err(session_authority_conflict(
                runtime_id,
                "released boundary receipt marker has no current durable receipt",
            ));
        };
        let current_sha256 = format!("sha256:{:x}", sha2::Sha256::digest(&current));
        if current_sha256 != marker {
            return Err(session_authority_conflict(
                runtime_id,
                "released boundary receipt marker differs from the current durable receipt bytes",
            ));
        }
        let released: RunBoundaryReceipt = serde_json::from_slice(&current).map_err(|error| {
            RuntimeStoreError::ReadFailed(format!(
                "released-marked boundary receipt failed to decode: {error}"
            ))
        })?;
        if released != *receipt {
            return Err(session_authority_conflict(
                runtime_id,
                "released boundary receipt marker conflicts with the prepared receipt",
            ));
        }
        Ok(Some(marker))
    }

    fn consume_released_0810_boundary_receipt_marker(
        tx: &Transaction<'_>,
        runtime_id: &LogicalRuntimeId,
        receipt: &RunBoundaryReceipt,
        marker: &str,
    ) -> Result<(), RuntimeStoreError> {
        let changed = tx
            .execute(
                r"
                DELETE FROM runtime_released_0810_boundary_receipts
                WHERE runtime_id = ?1
                  AND run_id = ?2
                  AND sequence = ?3
                  AND receipt_sha256 = ?4
                ",
                params![
                    runtime_id_text(runtime_id),
                    receipt.run_id.0.to_string(),
                    encode_receipt_sequence(receipt.sequence),
                    marker,
                ],
            )
            .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
        if changed != 1 {
            return Err(session_authority_conflict(
                runtime_id,
                "released boundary receipt marker changed before witness installation",
            ));
        }
        Ok(())
    }

    fn verify_current_ordinary_boundary_effects(
        tx: &Transaction<'_>,
        runtime_id: &LogicalRuntimeId,
        prepared_session: Option<&PreparedHeadCanonicalSqliteSession>,
        lifecycle_snapshot: Option<&MachineLifecycleSnapshot>,
        input_updates: &[(StoredInputState, Option<String>)],
    ) -> Result<(), RuntimeStoreError> {
        if let Some(snapshot) = lifecycle_snapshot {
            let target = MachineLifecycleStoreRecord::from_snapshot(snapshot).encode()?;
            let current = tx
                .query_row(
                    r"
                    SELECT runtime_state_json
                    FROM runtime_states
                    WHERE runtime_id = ?1
                    ",
                    params![runtime_id_text(runtime_id)],
                    |row| Ok(row.get::<_, JsonColumnBytes>(0)?.into_bytes()),
                )
                .optional()
                .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
            if current.as_deref() != Some(target.as_slice()) {
                return Err(session_authority_conflict(
                    runtime_id,
                    "ordinary boundary has a missing or divergent lifecycle target",
                ));
            }
        }

        for (input, _) in input_updates {
            let target = serde_json::to_vec(input)
                .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
            let current = tx
                .query_row(
                    r"
                    SELECT state_json
                    FROM runtime_input_states
                    WHERE runtime_id = ?1 AND input_id = ?2
                    ",
                    params![
                        runtime_id_text(runtime_id),
                        input.state.input_id.to_string()
                    ],
                    |row| Ok(row.get::<_, JsonColumnBytes>(0)?.into_bytes()),
                )
                .optional()
                .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
            if current.as_deref() != Some(target.as_slice()) {
                return Err(session_authority_conflict(
                    runtime_id,
                    format!(
                        "ordinary boundary has a missing or divergent input target {}",
                        input.state.input_id
                    ),
                ));
            }
        }

        if let Some(prepared) = prepared_session {
            let quarantined = tx
                .query_row(
                    r"
                    SELECT EXISTS(
                        SELECT 1
                        FROM runtime_projection_quarantine
                        WHERE runtime_id = ?1
                    )
                    ",
                    params![runtime_id_text(runtime_id)],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
            if quarantined {
                return Err(session_authority_conflict(
                    runtime_id,
                    "ordinary boundary still has a quarantined runtime projection",
                ));
            }
            for intent in &prepared.compaction_intents {
                let current = tx
                    .query_row(
                        r"
                        SELECT intent_json
                        FROM runtime_compaction_projection_outbox
                        WHERE runtime_id = ?1
                          AND session_id = ?2
                          AND parent_revision = ?3
                          AND revision = ?4
                          AND commit_fingerprint = ?5
                        ",
                        params![
                            runtime_id_text(runtime_id),
                            intent.projection.session_id().to_string(),
                            intent.projection.parent_revision(),
                            intent.projection.revision(),
                            intent.projection.commit_fingerprint(),
                        ],
                        |row| Ok(row.get::<_, JsonColumnBytes>(0)?.into_bytes()),
                    )
                    .optional()
                    .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
                let Some(current) = current else {
                    return Err(session_authority_conflict(
                        runtime_id,
                        format!(
                            "ordinary boundary is missing compaction intent {}",
                            intent.projection.revision()
                        ),
                    ));
                };
                let decoded: meerkat_core::CompactionProjectionIntent =
                    serde_json::from_slice(&current).map_err(|error| {
                        RuntimeStoreError::ReadFailed(format!(
                            "ordinary boundary compaction intent failed to decode: {error}"
                        ))
                    })?;
                if decoded != *intent {
                    return Err(session_authority_conflict(
                        runtime_id,
                        format!(
                            "ordinary boundary has a divergent compaction intent {}",
                            intent.projection.revision()
                        ),
                    ));
                }
            }
        }
        Ok(())
    }

    fn verify_released_0810_boundary_effects_for_adoption(
        tx: &Transaction<'_>,
        runtime_id: &LogicalRuntimeId,
        prepared_session: Option<&PreparedHeadCanonicalSqliteSession>,
        lifecycle_expected: Option<&MachineLifecycleExpectedVersion>,
        lifecycle_snapshot: Option<&MachineLifecycleSnapshot>,
        input_updates: &[(StoredInputState, Option<String>)],
    ) -> Result<(), RuntimeStoreError> {
        // Exact 0.8.10 retained committed targets, but not the prior row
        // versions against which a request may have been fenced. Do not mint a
        // current exact-request witness when any unprovable CAS precondition is
        // present.
        if lifecycle_expected.is_some()
            || input_updates.iter().any(|(_, expected)| expected.is_some())
        {
            return Err(session_authority_conflict(
                runtime_id,
                "released boundary cannot prove current lifecycle/input CAS preconditions",
            ));
        }
        verify_current_ordinary_boundary_effects(
            tx,
            runtime_id,
            prepared_session,
            lifecycle_snapshot,
            input_updates,
        )
    }

    fn verify_prepared_suffix_rows_for_exact_retry(
        tx: &Transaction<'_>,
        runtime_id: &LogicalRuntimeId,
        prepared: &PreparedHeadCanonicalSqliteSession,
    ) -> Result<(), RuntimeStoreError> {
        match &prepared.mutation {
            meerkat_core::lifecycle::core_executor::PreparedHeadCanonicalPhysicalMutation::Ordinary(
                mutation,
            ) => meerkat_store::sqlite_store::verify_prepared_head_canonical_rows_for_exact_retry_in_txn(
                tx,
                mutation,
            ),
            meerkat_core::lifecycle::core_executor::PreparedHeadCanonicalPhysicalMutation::Rewrite(
                mutation,
            ) => meerkat_store::sqlite_store::verify_prepared_head_canonical_rewrite_rows_for_exact_retry_in_txn(
                tx,
                mutation,
            ),
        }
        .map_err(|error| map_head_canonical_session_store_error(runtime_id, error))
    }

    fn apply_prepared_head_canonical_physical_mutation_in_txn(
        tx: &Transaction<'_>,
        runtime_id: &LogicalRuntimeId,
        prepared: &PreparedHeadCanonicalSqliteSession,
    ) -> Result<(), RuntimeStoreError> {
        match &prepared.mutation {
            meerkat_core::lifecycle::core_executor::PreparedHeadCanonicalPhysicalMutation::Ordinary(
                mutation,
            ) => meerkat_store::sqlite_store::apply_prepared_head_canonical_mutation_in_txn(
                tx, mutation,
            ),
            meerkat_core::lifecycle::core_executor::PreparedHeadCanonicalPhysicalMutation::Rewrite(
                mutation,
            ) => meerkat_store::sqlite_store::apply_prepared_head_canonical_rewrite_mutation_in_txn(
                tx, mutation,
            ),
        }
        .map(|_outcome| ())
        .map_err(|error| map_head_canonical_session_store_error(runtime_id, error))
    }

    fn load_recovery_boundary(
        conn: &Connection,
        runtime_id: &LogicalRuntimeId,
        candidate_id: &str,
    ) -> Result<Option<CommittedRecoveryBoundary>, RuntimeStoreError> {
        let boundary = conn
            .query_row(
                r"
                SELECT boundary_json
                FROM runtime_recovery_boundaries
                WHERE runtime_id = ?1 AND candidate_id = ?2
                ",
                params![runtime_id_text(runtime_id), candidate_id],
                |row| Ok(row.get::<_, JsonColumnBytes>(0)?.into_bytes()),
            )
            .optional()
            .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?
            .map(|bytes| CommittedRecoveryBoundary::decode(&bytes))
            .transpose()?;
        let Some(boundary) = boundary else {
            return Ok(None);
        };
        if boundary.evidence().candidate_id() != candidate_id {
            return Err(session_authority_conflict(
                runtime_id,
                "committed recovery row key and encoded candidate identity differ",
            ));
        }
        let owner = LogicalRuntimeId::for_session(boundary.evidence().session_id());
        if &owner != runtime_id {
            return Err(session_authority_conflict(
                runtime_id,
                format!(
                    "committed recovery candidate belongs to runtime {owner}, not {runtime_id}"
                ),
            ));
        }
        match load_boundary_receipt(conn, runtime_id, boundary.receipt())? {
            Some(receipt) if &receipt == boundary.receipt() => {}
            Some(_) => {
                return Err(session_authority_conflict(
                    runtime_id,
                    "committed recovery witness and boundary receipt differ",
                ));
            }
            None => {
                return Err(session_authority_conflict(
                    runtime_id,
                    "committed recovery witness has no atomic boundary receipt",
                ));
            }
        }
        Ok(Some(boundary))
    }

    fn insert_recovery_boundary_in_txn(
        tx: &Transaction<'_>,
        runtime_id: &LogicalRuntimeId,
        boundary: &CommittedRecoveryBoundary,
    ) -> Result<(), RuntimeStoreError> {
        let bytes = boundary.encode()?;
        tx.execute(
            r"
            INSERT INTO runtime_recovery_boundaries (
                runtime_id, candidate_id, boundary_json
            ) VALUES (?1, ?2, ?3)
            ",
            params![
                runtime_id_text(runtime_id),
                boundary.evidence().candidate_id(),
                bytes,
            ],
        )
        .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
        Ok(())
    }

    fn apply_recovery_receipt_digest_enrichments_in_txn(
        tx: &Transaction<'_>,
        runtime_id: &LogicalRuntimeId,
        boundary: &CommittedRecoveryBoundary,
    ) -> Result<(), RuntimeStoreError> {
        for enrichment in boundary.evidence().receipt_digest_enrichments() {
            let original = enrichment.original_receipt();
            let current_bytes = tx
                .query_row(
                    r"
                    SELECT receipt_json
                    FROM runtime_boundary_receipts
                    WHERE runtime_id = ?1 AND run_id = ?2 AND sequence = ?3
                    ",
                    params![
                        runtime_id_text(runtime_id),
                        original.run_id.0.to_string(),
                        encode_receipt_sequence(original.sequence),
                    ],
                    |row| Ok(row.get::<_, JsonColumnBytes>(0)?.into_bytes()),
                )
                .optional()
                .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?
                .ok_or_else(|| {
                    session_authority_conflict(
                        runtime_id,
                        format!(
                            "recovery receipt enrichment source {}:{} is absent",
                            original.run_id, original.sequence
                        ),
                    )
                })?;
            let source = PreparedRecoveryReceiptSource::from_serialized_row(&current_bytes)?;
            if source.receipt() != original
                || source.exact_row_token() != enrichment.original_exact_row_token()
            {
                return Err(session_authority_conflict(
                    runtime_id,
                    format!(
                        "recovery receipt enrichment source {}:{} changed after classification",
                        original.run_id, original.sequence
                    ),
                ));
            }
            let enriched = enrichment.enriched_receipt();
            let enriched_bytes = serde_json::to_vec(&enriched)
                .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
            let updated = tx
                .execute(
                    r"
                    UPDATE runtime_boundary_receipts
                    SET receipt_json = ?4
                    WHERE runtime_id = ?1 AND run_id = ?2 AND sequence = ?3
                    ",
                    params![
                        runtime_id_text(runtime_id),
                        original.run_id.0.to_string(),
                        encode_receipt_sequence(original.sequence),
                        enriched_bytes,
                    ],
                )
                .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
            if updated != 1 {
                return Err(session_authority_conflict(
                    runtime_id,
                    "recovery receipt enrichment lost its exact source row",
                ));
            }
        }
        Ok(())
    }

    fn verify_recovery_receipt_digest_enrichments_in_txn(
        tx: &Transaction<'_>,
        runtime_id: &LogicalRuntimeId,
        boundary: &CommittedRecoveryBoundary,
    ) -> Result<(), RuntimeStoreError> {
        for enrichment in boundary.evidence().receipt_digest_enrichments() {
            let enriched = enrichment.enriched_receipt();
            match load_boundary_receipt(tx, runtime_id, &enriched)? {
                Some(current) if current == enriched => {}
                Some(_) => {
                    return Err(session_authority_conflict(
                        runtime_id,
                        format!(
                            "committed recovery receipt enrichment {}:{} was superseded",
                            enriched.run_id, enriched.sequence
                        ),
                    ));
                }
                None => {
                    return Err(session_authority_conflict(
                        runtime_id,
                        format!(
                            "committed recovery receipt enrichment {}:{} is absent",
                            enriched.run_id, enriched.sequence
                        ),
                    ));
                }
            }
        }
        Ok(())
    }

    fn verify_materialized_head_authority_in_txn(
        tx: &Transaction<'_>,
        runtime_id: &LogicalRuntimeId,
        authority: &RuntimeSessionAuthority,
    ) -> Result<(), RuntimeStoreError> {
        let authority = authority.head_canonical().ok_or_else(|| {
            session_authority_conflict(
                runtime_id,
                "materialized HeadCanonical verification received WholeBlob authority",
            )
        })?;
        let head = authority.boundary_head();
        let session =
            meerkat_store::sqlite_store::materialize_runtime_boundary_head_canonical_in_txn(
                tx, head,
            )
            .map_err(|error| map_head_canonical_session_store_error(runtime_id, error))?;
        if session.id() != authority.session_id()
            || session.messages().len() as u64 != head.message_count
            || session.version() != head.version
            || session.created_at() != head.created_at
            || session.updated_at() != head.updated_at
            || session.total_usage() != head.usage
            || !head.matches_session_metadata(&session).map_err(|error| {
                session_authority_conflict(
                    runtime_id,
                    format!("materialized boundary metadata is invalid: {error}"),
                )
            })?
        {
            return Err(session_authority_conflict(
                runtime_id,
                "materialized boundary head differs from runtime session authority",
            ));
        }
        let token = meerkat_core::session_head_cas_token(head).map_err(|error| {
            session_authority_conflict(
                runtime_id,
                format!("materialized boundary head token is invalid: {error}"),
            )
        })?;
        if token != authority.committed_head_token() {
            return Err(session_authority_conflict(
                runtime_id,
                "materialized boundary token differs from store authority",
            ));
        }
        Ok(())
    }

    #[derive(Debug)]
    struct PreparedHeadCanonicalSqliteSession {
        mutation: meerkat_core::lifecycle::core_executor::PreparedHeadCanonicalPhysicalMutation,
        successor_head: meerkat_core::session_store::SessionHead,
        catalog_entry: crate::store::RuntimeSessionCatalogEntry,
        compaction_intents: Vec<meerkat_core::CompactionProjectionIntent>,
    }

    fn prepare_head_canonical_sqlite_session(
        runtime_id: &LogicalRuntimeId,
        session_commit: &meerkat_core::lifecycle::core_executor::BoundSessionCommit,
        session_store_key: Option<&meerkat_core::types::SessionId>,
    ) -> Result<PreparedHeadCanonicalSqliteSession, RuntimeStoreError> {
        let boundary = session_commit.head_canonical().ok_or_else(|| {
            session_authority_conflict(
                runtime_id,
                "session boundary has no prepared ordinary head mutation; rewrite and unprepared snapshots are unsupported",
            )
        })?;
        let session_id = boundary.mutation().session_id();
        let owner = LogicalRuntimeId::for_session(session_id);
        if &owner != runtime_id {
            return Err(session_authority_conflict(
                runtime_id,
                format!("session {session_id} belongs to runtime {owner}, not {runtime_id}"),
            ));
        }
        if let Some(expected) = session_store_key
            && session_id != expected
        {
            return Err(RuntimeStoreError::SessionKeyMismatch {
                expected: expected.clone(),
                actual: session_id.clone(),
            });
        }
        if let Some(predecessor) = boundary.mutation().predecessor_head() {
            validate_exact_head_prefix_authority(
                runtime_id,
                predecessor,
                "prepared physical predecessor head",
            )?;
        }
        validate_exact_head_prefix_authority(
            runtime_id,
            boundary.mutation().successor_head(),
            "prepared physical successor head",
        )?;
        let compaction_intents = boundary.compaction_projection_intents().to_vec();
        let catalog_entry = crate::store::RuntimeSessionCatalogEntry::from_head_facts(
            boundary.mutation().successor_head(),
            boundary.catalog_labels().clone(),
            boundary.catalog_lifecycle_terminal(),
            RuntimeSessionPersistenceProfile::HeadCanonicalV1,
            None,
        )?;
        Ok(PreparedHeadCanonicalSqliteSession {
            mutation: boundary.mutation().clone(),
            successor_head: boundary.mutation().successor_head().clone(),
            catalog_entry,
            compaction_intents,
        })
    }

    fn validate_prepared_recovery_sqlite_session(
        runtime_id: &LogicalRuntimeId,
        session_commit: &meerkat_core::lifecycle::core_executor::BoundSessionCommit,
        prepared: &PreparedHeadCanonicalSqliteSession,
        boundary: &CommittedRecoveryBoundary,
    ) -> Result<(), RuntimeStoreError> {
        let evidence = boundary.evidence();
        evidence.verify_head_canonical_boundary(session_commit, boundary.receipt())?;
        if evidence.session_id() != prepared.mutation.session_id() {
            return Err(session_authority_conflict(
                runtime_id,
                "prepared recovery head mutation and sealed session identity differ",
            ));
        }
        let (
            _committed_store_revision,
            _committed_head_token,
            _physical_store_revision,
            physical_head_token,
            recovered_head_token,
        ) = evidence
            .head_canonical_authority_transition()
            .ok_or_else(|| {
                session_authority_conflict(
                    runtime_id,
                    "prepared recovery evidence carries no HeadCanonical authority transition",
                )
            })?;
        if prepared.mutation.predecessor_head().is_none()
            || prepared.mutation.predecessor_head_token() != Some(physical_head_token)
            || prepared.mutation.successor_head_token() != recovered_head_token
        {
            return Err(session_authority_conflict(
                runtime_id,
                "prepared recovery mutation does not bind the sealed physical/recovered store tokens",
            ));
        }
        Ok(())
    }

    fn validate_prepared_recovery_request_binding(
        session: &meerkat_core::lifecycle::core_executor::BoundSessionCommit,
        evidence: &crate::store::PreparedRecoveryEvidence,
        input_updates: &[InputStatePersistenceRecord],
        receipt: &RunBoundaryReceipt,
        lifecycle: &MachineLifecycleCommit,
    ) -> Result<(), RuntimeStoreError> {
        evidence.verify_head_canonical_boundary(session, receipt)?;
        evidence.verify_input_updates(input_updates)?;
        evidence.verify_request_effects(receipt, lifecycle)?;
        Ok(())
    }

    fn encode_u64(value: u64) -> [u8; 8] {
        value.to_be_bytes()
    }

    fn decode_u64(bytes: Vec<u8>, label: &str) -> Result<u64, RuntimeStoreError> {
        let encoded: [u8; 8] = bytes.try_into().map_err(|bytes: Vec<u8>| {
            RuntimeStoreError::ReadFailed(format!(
                "{label} must be an 8-byte unsigned integer, found {} bytes",
                bytes.len()
            ))
        })?;
        Ok(u64::from_be_bytes(encoded))
    }

    /// Deserialize a persisted session-snapshot row through typed serde.
    /// `Session::deserialize` validates the mandatory envelope version against
    /// the generated persistence version authority, so a missing or
    /// non-current (v0/v1) row fails closed here instead of silently
    /// defaulting or upgrading on read.
    fn deserialize_persisted_session(
        bytes: &[u8],
    ) -> Result<meerkat_core::Session, RuntimeStoreError> {
        meerkat_core::Session::from_persisted_bytes(bytes)
            .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))
    }

    /// Deserialize a persisted `StoredInputState` row through typed serde.
    /// `StoredInputState::deserialize` validates the mandatory
    /// `stored_input_state_version` byte against the generated persistence
    /// version authority, so a missing or non-current row fails closed.
    fn deserialize_persisted_input_state(
        bytes: &[u8],
    ) -> Result<StoredInputState, RuntimeStoreError> {
        serde_json::from_slice(bytes).map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))
    }

    /// Encode a `u64` boundary-receipt sequence into the durable `INTEGER`
    /// column as the reinterpreted two's-complement `i64` bit pattern.
    ///
    /// This is a total, injective bijection over the full `u64` domain: every
    /// distinct sequence maps to a distinct stored key (values above
    /// `i64::MAX` wrap into the negative `i64` range rather than saturating to
    /// a single alias). The `sequence` column participates only in exact-match
    /// equality lookups against a composite primary key — never in ordered
    /// range scans — so the lack of monotonic ORDER BY ordering for the
    /// wrapped high half is irrelevant to identity. Decode symmetrically with
    /// [`decode_receipt_sequence`].
    fn encode_receipt_sequence(sequence: u64) -> i64 {
        i64::from_ne_bytes(sequence.to_ne_bytes())
    }

    /// Inverse of [`encode_receipt_sequence`]: reinterpret the stored `i64`
    /// bit pattern back into the original `u64` sequence.
    #[cfg(test)]
    fn decode_receipt_sequence(stored: i64) -> u64 {
        u64::from_ne_bytes(stored.to_ne_bytes())
    }

    const AUTH_OAUTH_FLOW_STATE_ID: &str = "auth_oauth_flow_state";

    fn load_whole_blob_store_authority(
        conn: &Connection,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<WholeBlobStoreAuthority>, RuntimeStoreError> {
        let row = conn
            .query_row(
                r"
                SELECT authority_version, session_id, store_revision, blob_sha256
                FROM runtime_whole_blob_authority
                WHERE runtime_id = ?1
                ",
                params![runtime_id_text(runtime_id)],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
        let Some((version, session_id, revision, blob_sha256)) = row else {
            return Ok(None);
        };
        if version != i64::from(WholeBlobStoreAuthority::VERSION) || revision <= 0 {
            return Err(session_authority_conflict(
                runtime_id,
                "WholeBlob authority ledger has an unsupported version or revision",
            ));
        }
        let session_id = meerkat_core::types::SessionId::parse(&session_id).map_err(|error| {
            session_authority_conflict(
                runtime_id,
                format!("WholeBlob authority session id is invalid: {error}"),
            )
        })?;
        if &LogicalRuntimeId::for_session(&session_id) != runtime_id {
            return Err(session_authority_conflict(
                runtime_id,
                "WholeBlob authority session id does not own this runtime",
            ));
        }
        WholeBlobStoreAuthority::issued(session_id, revision as u64, blob_sha256).map(Some)
    }

    fn whole_blob_body_sha256(bytes: &[u8]) -> String {
        use sha2::Digest as _;
        format!("row-sha256:{:x}", sha2::Sha256::digest(bytes))
    }

    fn catalog_system_time_millis(time: SystemTime, field: &str) -> Result<i64, RuntimeStoreError> {
        let millis = time
            .duration_since(UNIX_EPOCH)
            .map_err(|error| {
                RuntimeStoreError::WriteFailed(format!(
                    "runtime session catalog {field} predates unix epoch: {error}"
                ))
            })?
            .as_millis();
        i64::try_from(millis).map_err(|_| {
            RuntimeStoreError::WriteFailed(format!(
                "runtime session catalog {field} exceeds SQLite INTEGER"
            ))
        })
    }

    fn upsert_runtime_session_catalog_entry_in_txn(
        tx: &Transaction<'_>,
        runtime_id: &LogicalRuntimeId,
        entry: &crate::store::RuntimeSessionCatalogEntry,
    ) -> Result<(), RuntimeStoreError> {
        if &LogicalRuntimeId::for_session(entry.session_id()) != runtime_id {
            return Err(session_authority_conflict(
                runtime_id,
                "runtime session catalog entry does not bind this runtime",
            ));
        }
        let encoded = serde_json::to_vec(entry)
            .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
        tx.execute(
            r"
            INSERT INTO runtime_session_catalog
                (runtime_id, session_id, created_at_ms, updated_at_ms, entry_json)
            VALUES (?1, ?2, ?3, ?4, ?5)
            ON CONFLICT(runtime_id) DO UPDATE SET
                session_id = excluded.session_id,
                created_at_ms = excluded.created_at_ms,
                updated_at_ms = excluded.updated_at_ms,
                entry_json = excluded.entry_json
            ",
            params![
                runtime_id_text(runtime_id),
                entry.session_id().to_string(),
                catalog_system_time_millis(entry.created_at(), "created_at")?,
                catalog_system_time_millis(entry.updated_at(), "updated_at")?,
                encoded,
            ],
        )
        .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
        Ok(())
    }

    fn decode_runtime_session_catalog_entry(
        runtime_id: &LogicalRuntimeId,
        stored_session_id: &str,
        created_at_ms: i64,
        updated_at_ms: i64,
        encoded: &[u8],
    ) -> Result<crate::store::RuntimeSessionCatalogEntry, RuntimeStoreError> {
        let entry: crate::store::RuntimeSessionCatalogEntry = serde_json::from_slice(encoded)
            .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
        if entry.session_id().to_string() != stored_session_id
            || &LogicalRuntimeId::for_session(entry.session_id()) != runtime_id
            || catalog_system_time_millis(entry.created_at(), "created_at")? != created_at_ms
            || catalog_system_time_millis(entry.updated_at(), "updated_at")? != updated_at_ms
        {
            return Err(session_authority_conflict(
                runtime_id,
                "runtime session catalog indexed facts differ from encoded entry",
            ));
        }
        Ok(entry)
    }

    fn load_runtime_session_catalog_entry_in_txn(
        conn: &Connection,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<crate::store::RuntimeSessionCatalogEntry>, RuntimeStoreError> {
        let row = conn
            .query_row(
                r"
                SELECT session_id, created_at_ms, updated_at_ms, entry_json
                FROM runtime_session_catalog
                WHERE runtime_id = ?1
                ",
                params![runtime_id_text(runtime_id)],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, JsonColumnBytes>(3)?.into_bytes(),
                    ))
                },
            )
            .optional()
            .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
        row.map(|(session_id, created_at_ms, updated_at_ms, encoded)| {
            decode_runtime_session_catalog_entry(
                runtime_id,
                &session_id,
                created_at_ms,
                updated_at_ms,
                &encoded,
            )
        })
        .transpose()
    }

    fn list_runtime_session_catalog_entries_in_conn(
        conn: &Connection,
        filter: meerkat_core::SessionFilter,
    ) -> Result<Vec<crate::store::RuntimeSessionCatalogEntry>, RuntimeStoreError> {
        let created_after_ms = filter
            .created_after
            .map(|time| catalog_system_time_millis(time, "created_after"))
            .transpose()?;
        let updated_after_ms = filter
            .updated_after
            .map(|time| catalog_system_time_millis(time, "updated_after"))
            .transpose()?;
        let limit = filter
            .limit
            .map(|value| {
                i64::try_from(value).map_err(|_| {
                    RuntimeStoreError::ReadFailed(
                        "runtime session catalog limit exceeds SQLite INTEGER".to_string(),
                    )
                })
            })
            .transpose()?
            .unwrap_or(-1);
        let offset = i64::try_from(filter.offset.unwrap_or(0)).map_err(|_| {
            RuntimeStoreError::ReadFailed(
                "runtime session catalog offset exceeds SQLite INTEGER".to_string(),
            )
        })?;
        let mut statement = conn
            .prepare(
                r"
                SELECT runtime_id, session_id, created_at_ms, updated_at_ms, entry_json
                FROM runtime_session_catalog
                WHERE (?1 IS NULL OR created_at_ms >= ?1)
                  AND (?2 IS NULL OR updated_at_ms >= ?2)
                ORDER BY updated_at_ms DESC, session_id ASC
                LIMIT ?3 OFFSET ?4
                ",
            )
            .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
        let rows = statement
            .query_map(
                params![created_after_ms, updated_after_ms, limit, offset],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, JsonColumnBytes>(4)?.into_bytes(),
                    ))
                },
            )
            .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
        rows.map(|row| {
            let (runtime_id, session_id, created_at_ms, updated_at_ms, encoded) =
                row.map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
            decode_runtime_session_catalog_entry(
                &LogicalRuntimeId(runtime_id),
                &session_id,
                created_at_ms,
                updated_at_ms,
                &encoded,
            )
        })
        .collect()
    }

    #[derive(Debug, Clone)]
    struct StoredWholeBlobProvisionalMetadata {
        authority: WholeBlobProvisionalTailAuthority,
        conversation_digest: String,
        message_count: u64,
        catalog_entry: crate::store::RuntimeSessionCatalogEntry,
        compaction_projection_intents: Vec<meerkat_core::CompactionProjectionIntent>,
    }

    fn load_whole_blob_provisional_metadata(
        conn: &Connection,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<StoredWholeBlobProvisionalMetadata>, RuntimeStoreError> {
        let row = conn
            .query_row(
                r"
                SELECT authority_version, session_id, base_store_revision,
                       base_blob_sha256, run_id, candidate_sequence,
                       candidate_blob_sha256,
                       EXISTS (
                           SELECT 1 FROM runtime_whole_blob_bodies AS bodies
                           WHERE bodies.blob_sha256 =
                                 provisional.candidate_blob_sha256
                       )
                       , conversation_digest, message_count,
                       catalog_json, compaction_intents_json
                FROM runtime_whole_blob_provisional_tails AS provisional
                WHERE provisional.runtime_id = ?1
                ",
                params![runtime_id_text(runtime_id)],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, i64>(7)?,
                        row.get::<_, String>(8)?,
                        row.get::<_, i64>(9)?,
                        row.get::<_, JsonColumnBytes>(10)?.into_bytes(),
                        row.get::<_, JsonColumnBytes>(11)?.into_bytes(),
                    ))
                },
            )
            .optional()
            .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
        let Some((
            version,
            session_id,
            base_revision,
            base_blob_sha256,
            run_id,
            candidate_sequence,
            candidate_blob_sha256,
            candidate_body_exists,
            conversation_digest,
            message_count,
            catalog_json,
            compaction_intents_json,
        )) = row
        else {
            return Ok(None);
        };
        if version != 1
            || base_revision <= 0
            || candidate_sequence <= 0
            || candidate_body_exists != 1
            || conversation_digest.is_empty()
            || message_count < 0
        {
            return Err(session_authority_conflict(
                runtime_id,
                "WholeBlob provisional row has unsupported identity or a missing candidate body",
            ));
        }
        let session_id = meerkat_core::types::SessionId::parse(&session_id).map_err(|error| {
            session_authority_conflict(
                runtime_id,
                format!("WholeBlob provisional session id is invalid: {error}"),
            )
        })?;
        let run_id = uuid::Uuid::parse_str(&run_id)
            .map(RunId::from_uuid)
            .map_err(|error| {
                session_authority_conflict(
                    runtime_id,
                    format!("WholeBlob provisional run id is invalid: {error}"),
                )
            })?;
        let authority = WholeBlobProvisionalTailAuthority::issued(
            session_id,
            base_revision as u64,
            base_blob_sha256,
            run_id,
            candidate_blob_sha256,
            candidate_sequence as u64,
        )
        .map_err(|error| session_authority_conflict(runtime_id, error.to_string()))?;
        let catalog_entry: crate::store::RuntimeSessionCatalogEntry =
            serde_json::from_slice(&catalog_json)
                .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
        let compaction_projection_intents: Vec<meerkat_core::CompactionProjectionIntent> =
            serde_json::from_slice(&compaction_intents_json)
                .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
        if catalog_entry.session_id() != authority.session_id()
            || catalog_entry.persistence_profile() != RuntimeSessionPersistenceProfile::WholeBlobV1
            || u64::try_from(catalog_entry.message_count()).ok() != Some(message_count as u64)
            || compaction_projection_intents
                .iter()
                .any(|intent| intent.projection.session_id() != authority.session_id())
        {
            return Err(session_authority_conflict(
                runtime_id,
                "WholeBlob provisional catalog/compaction facts do not bind candidate session",
            ));
        }
        Ok(Some(StoredWholeBlobProvisionalMetadata {
            authority,
            conversation_digest,
            message_count: message_count as u64,
            catalog_entry,
            compaction_projection_intents,
        }))
    }

    fn load_whole_blob_provisional_authority(
        conn: &Connection,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<WholeBlobProvisionalTailAuthority>, RuntimeStoreError> {
        let row = conn
            .query_row(
                r"
                SELECT authority_version, session_id, base_store_revision,
                       base_blob_sha256, run_id, candidate_sequence,
                       candidate_blob_sha256,
                       EXISTS (
                           SELECT 1 FROM runtime_whole_blob_bodies AS bodies
                           WHERE bodies.blob_sha256 =
                                 provisional.candidate_blob_sha256
                       )
                FROM runtime_whole_blob_provisional_tails AS provisional
                WHERE provisional.runtime_id = ?1
                ",
                params![runtime_id_text(runtime_id)],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, i64>(7)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
        let Some((
            version,
            session_id,
            base_revision,
            base_blob_sha256,
            run_id,
            candidate_sequence,
            candidate_blob_sha256,
            candidate_body_exists,
        )) = row
        else {
            return Ok(None);
        };
        if version != 1
            || base_revision <= 0
            || candidate_sequence <= 0
            || candidate_body_exists != 1
        {
            return Err(session_authority_conflict(
                runtime_id,
                "WholeBlob provisional authority has unsupported identity or a missing candidate body",
            ));
        }
        let session_id = meerkat_core::types::SessionId::parse(&session_id).map_err(|error| {
            session_authority_conflict(
                runtime_id,
                format!("WholeBlob provisional session id is invalid: {error}"),
            )
        })?;
        let run_id = uuid::Uuid::parse_str(&run_id)
            .map(RunId::from_uuid)
            .map_err(|error| {
                session_authority_conflict(
                    runtime_id,
                    format!("WholeBlob provisional run id is invalid: {error}"),
                )
            })?;
        WholeBlobProvisionalTailAuthority::issued(
            session_id,
            base_revision as u64,
            base_blob_sha256,
            run_id,
            candidate_blob_sha256,
            candidate_sequence as u64,
        )
        .map(Some)
        .map_err(|error| session_authority_conflict(runtime_id, error.to_string()))
    }

    fn load_whole_blob_provisional_tail(
        conn: &Connection,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<CommittedWholeBlobProvisionalTail>, RuntimeStoreError> {
        let Some(authority) = load_whole_blob_provisional_authority(conn, runtime_id)? else {
            return Ok(None);
        };
        let candidate_bytes = conn
            .query_row(
                r"
                SELECT session_snapshot
                FROM runtime_whole_blob_bodies
                WHERE blob_sha256 = ?1
                ",
                params![authority.candidate_blob_sha256()],
                |row| Ok(row.get::<_, JsonColumnBytes>(0)?.into_bytes()),
            )
            .optional()
            .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?
            .ok_or_else(|| {
                session_authority_conflict(
                    runtime_id,
                    "WholeBlob provisional authority references a missing candidate body",
                )
            })?;
        if whole_blob_body_sha256(&candidate_bytes) != authority.candidate_blob_sha256() {
            return Err(session_authority_conflict(
                runtime_id,
                "WholeBlob provisional body digest differs from store authority",
            ));
        }
        Ok(Some(CommittedWholeBlobProvisionalTail::new(
            authority,
            Arc::new(candidate_bytes),
        )))
    }

    fn upsert_runtime_snapshot_issued(
        tx: &Transaction<'_>,
        runtime_id: &LogicalRuntimeId,
        snapshot: &[u8],
        session_id: &meerkat_core::types::SessionId,
        blob_sha256: &str,
        catalog_entry: &crate::store::RuntimeSessionCatalogEntry,
    ) -> Result<WholeBlobStoreAuthority, RuntimeStoreError> {
        if &LogicalRuntimeId::for_session(session_id) != runtime_id
            || catalog_entry.session_id() != session_id
            || catalog_entry.persistence_profile() != RuntimeSessionPersistenceProfile::WholeBlobV1
        {
            return Err(session_authority_conflict(
                runtime_id,
                format!("WholeBlob payload session {session_id} does not own this runtime"),
            ));
        }
        let current = load_whole_blob_store_authority(tx, runtime_id)?;
        if let Some(current) = current.as_ref()
            && current.session_id() == session_id
            && current.blob_sha256() == blob_sha256
        {
            let body_exists = tx
                .query_row(
                    "SELECT 1 FROM runtime_whole_blob_bodies WHERE blob_sha256 = ?1",
                    params![blob_sha256],
                    |_row| Ok(()),
                )
                .optional()
                .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?
                .is_some();
            if !body_exists {
                return Err(session_authority_conflict(
                    runtime_id,
                    "WholeBlob authority references a missing body",
                ));
            }
            upsert_runtime_session_catalog_entry_in_txn(tx, runtime_id, catalog_entry)?;
            clear_runtime_projection_quarantine(tx, runtime_id)?;
            return Ok(current.clone());
        }
        if load_whole_blob_provisional_authority(tx, runtime_id)?.is_some() {
            return Err(session_authority_conflict(
                runtime_id,
                "ordinary WholeBlob write cannot bypass a store-owned provisional candidate",
            ));
        }
        if let Some(current) = current.as_ref() {
            let body_exists = tx
                .query_row(
                    "SELECT 1 FROM runtime_whole_blob_bodies WHERE blob_sha256 = ?1",
                    params![current.blob_sha256()],
                    |_row| Ok(()),
                )
                .optional()
                .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?
                .is_some();
            if !body_exists {
                return Err(session_authority_conflict(
                    runtime_id,
                    "WholeBlob authority references a missing predecessor body",
                ));
            }
        }
        let next_revision = current
            .as_ref()
            .map(WholeBlobStoreAuthority::store_revision)
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| {
                RuntimeStoreError::WriteFailed(format!(
                    "WholeBlob store revision exhausted for runtime {runtime_id}"
                ))
            })?;
        let next_revision_i64 = i64::try_from(next_revision).map_err(|_| {
            RuntimeStoreError::WriteFailed(format!(
                "WholeBlob store revision exceeds SQLite INTEGER for runtime {runtime_id}"
            ))
        })?;
        tx.execute(
            r"
            INSERT INTO runtime_whole_blob_bodies (blob_sha256, session_snapshot)
            VALUES (?1, ?2)
            ON CONFLICT(blob_sha256) DO NOTHING
            ",
            params![blob_sha256, snapshot],
        )
        .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
        upsert_runtime_session_catalog_entry_in_txn(tx, runtime_id, catalog_entry)?;
        tx.execute(
            r"
            INSERT INTO runtime_whole_blob_authority
                (runtime_id, authority_version, session_id, store_revision, blob_sha256)
            VALUES (?1, 1, ?2, ?3, ?4)
            ON CONFLICT(runtime_id) DO UPDATE SET
                authority_version = excluded.authority_version,
                session_id = excluded.session_id,
                store_revision = excluded.store_revision,
                blob_sha256 = excluded.blob_sha256
            ",
            params![
                runtime_id_text(runtime_id),
                session_id.to_string(),
                next_revision_i64,
                blob_sha256
            ],
        )
        .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
        tx.execute(
            "DELETE FROM runtime_session_snapshots WHERE runtime_id = ?1",
            params![runtime_id_text(runtime_id)],
        )
        .map_err(|error| map_runtime_snapshot_mutation_error(runtime_id, error))?;
        if let Some(previous) = current.as_ref()
            && previous.blob_sha256() != blob_sha256
        {
            tx.execute(
                r"
                DELETE FROM runtime_whole_blob_bodies
                WHERE blob_sha256 = ?1
                  AND NOT EXISTS (
                      SELECT 1 FROM runtime_whole_blob_authority
                      WHERE blob_sha256 = ?1
                  )
                  AND NOT EXISTS (
                      SELECT 1 FROM runtime_whole_blob_provisional_tails
                      WHERE candidate_blob_sha256 = ?1
                         OR base_blob_sha256 = ?1
                  )
                ",
                params![previous.blob_sha256()],
            )
            .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
        }
        clear_runtime_projection_quarantine(tx, runtime_id)?;
        WholeBlobStoreAuthority::issued(session_id.clone(), next_revision, blob_sha256.to_string())
    }

    fn upsert_runtime_snapshot(
        tx: &Transaction<'_>,
        runtime_id: &LogicalRuntimeId,
        snapshot: &[u8],
    ) -> Result<(), RuntimeStoreError> {
        let session = meerkat_core::Session::from_persisted_bytes(snapshot)
            .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
        let runtime_state = load_runtime_session_catalog_entry_in_txn(tx, runtime_id)?
            .and_then(|entry| entry.runtime_state());
        let catalog_entry = crate::store::RuntimeSessionCatalogEntry::from_session(
            &session,
            RuntimeSessionPersistenceProfile::WholeBlobV1,
            runtime_state,
        )?;
        use sha2::Digest as _;
        let blob_sha256 = format!("row-sha256:{:x}", sha2::Sha256::digest(snapshot));
        upsert_runtime_snapshot_issued(
            tx,
            runtime_id,
            snapshot,
            session.id(),
            &blob_sha256,
            &catalog_entry,
        )
        .map(|_| ())
    }

    fn delete_whole_blob_state(
        tx: &Transaction<'_>,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<(), RuntimeStoreError> {
        let mut tokens = Vec::new();
        if let Some(authority) = load_whole_blob_store_authority(tx, runtime_id)? {
            tokens.push(authority.blob_sha256().to_string());
        }
        if let Some(provisional) = load_whole_blob_provisional_authority(tx, runtime_id)? {
            tokens.push(provisional.candidate_blob_sha256().to_string());
        }
        tx.execute(
            "DELETE FROM runtime_whole_blob_provisional_tails WHERE runtime_id = ?1",
            params![runtime_id_text(runtime_id)],
        )
        .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
        tx.execute(
            "DELETE FROM runtime_whole_blob_authority WHERE runtime_id = ?1",
            params![runtime_id_text(runtime_id)],
        )
        .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
        tx.execute(
            "DELETE FROM runtime_session_snapshots WHERE runtime_id = ?1",
            params![runtime_id_text(runtime_id)],
        )
        .map_err(|error| map_runtime_snapshot_mutation_error(runtime_id, error))?;
        tx.execute(
            "DELETE FROM runtime_session_catalog WHERE runtime_id = ?1",
            params![runtime_id_text(runtime_id)],
        )
        .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
        for token in tokens {
            tx.execute(
                r"
                DELETE FROM runtime_whole_blob_bodies
                WHERE blob_sha256 = ?1
                  AND NOT EXISTS (
                      SELECT 1 FROM runtime_whole_blob_authority
                      WHERE blob_sha256 = ?1
                  )
                  AND NOT EXISTS (
                      SELECT 1 FROM runtime_whole_blob_provisional_tails
                      WHERE candidate_blob_sha256 = ?1
                  )
                ",
                params![token],
            )
            .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
        }
        Ok(())
    }

    fn promote_whole_blob_provisional_in_txn(
        tx: &Transaction<'_>,
        runtime_id: &LogicalRuntimeId,
        expected: &WholeBlobProvisionalTailAuthority,
        receipt: &RunBoundaryReceipt,
        checkpoint_conversation_digest: &str,
        checkpoint_message_count: u64,
        runtime_state: Option<RuntimeState>,
    ) -> Result<WholeBlobStoreAuthority, RuntimeStoreError> {
        let stored = load_whole_blob_provisional_metadata(tx, runtime_id)?.ok_or_else(|| {
            session_authority_conflict(
                runtime_id,
                "WholeBlob provisional promotion candidate is absent",
            )
        })?;
        let current = load_whole_blob_store_authority(tx, runtime_id)?.ok_or_else(|| {
            session_authority_conflict(
                runtime_id,
                "WholeBlob provisional promotion has no committed base",
            )
        })?;
        if &stored.authority != expected
            || expected.run_id() != &receipt.run_id
            || current.session_id() != expected.session_id()
            || current.store_revision() != expected.base_store_revision()
            || current.blob_sha256() != expected.base_blob_sha256()
        {
            return Err(session_authority_conflict(
                runtime_id,
                "WholeBlob provisional promotion does not exactly match stored base/run/candidate",
            ));
        }
        if stored.conversation_digest != checkpoint_conversation_digest
            || stored.message_count != checkpoint_message_count
            || receipt.conversation_digest.as_deref() != Some(stored.conversation_digest.as_str())
            || u64::try_from(receipt.message_count).ok() != Some(stored.message_count)
        {
            return Err(session_authority_conflict(
                runtime_id,
                "WholeBlob final receipt does not bind the stored checkpoint count/digest",
            ));
        }
        reject_finalized_compaction_projection_replays(
            tx,
            runtime_id,
            &stored.compaction_projection_intents,
        )?;
        let mut catalog_entry = stored.catalog_entry;
        if let Some(runtime_state) = runtime_state {
            catalog_entry.set_runtime_state(Some(runtime_state));
        } else if let Some(current_catalog) =
            load_runtime_session_catalog_entry_in_txn(tx, runtime_id)?
        {
            catalog_entry.set_runtime_state(current_catalog.runtime_state());
        }
        let next_revision = current.store_revision().checked_add(1).ok_or_else(|| {
            RuntimeStoreError::WriteFailed(format!(
                "WholeBlob store revision exhausted for runtime {runtime_id}"
            ))
        })?;
        let next_revision_i64 = i64::try_from(next_revision).map_err(|_| {
            RuntimeStoreError::WriteFailed(format!(
                "WholeBlob store revision exceeds SQLite INTEGER for runtime {runtime_id}"
            ))
        })?;
        // The candidate body already lives in `runtime_whole_blob_bodies`.
        // Promotion performs metadata-only indexed writes; it never reads,
        // hashes, encodes, copies, or rewrites the accumulated document.
        let updated = tx
            .execute(
                r"
                UPDATE runtime_whole_blob_authority
                SET session_id = ?2, store_revision = ?3, blob_sha256 = ?4
                WHERE runtime_id = ?1
                  AND session_id = ?2
                  AND store_revision = ?5
                  AND blob_sha256 = ?6
                ",
                params![
                    runtime_id_text(runtime_id),
                    expected.session_id().to_string(),
                    next_revision_i64,
                    expected.candidate_blob_sha256(),
                    i64::try_from(current.store_revision()).map_err(|_| {
                        RuntimeStoreError::WriteFailed(
                            "WholeBlob current revision exceeds SQLite INTEGER".to_string(),
                        )
                    })?,
                    current.blob_sha256(),
                ],
            )
            .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
        if updated != 1 {
            return Err(session_authority_conflict(
                runtime_id,
                "WholeBlob provisional base changed during promotion",
            ));
        }
        let deleted = tx
            .execute(
                r"
                DELETE FROM runtime_whole_blob_provisional_tails
                WHERE runtime_id = ?1
                  AND session_id = ?2
                  AND base_store_revision = ?3
                  AND base_blob_sha256 = ?4
                  AND run_id = ?5
                  AND candidate_blob_sha256 = ?6
                  AND candidate_sequence = ?7
                ",
                params![
                    runtime_id_text(runtime_id),
                    expected.session_id().to_string(),
                    i64::try_from(expected.base_store_revision()).map_err(|_| {
                        RuntimeStoreError::WriteFailed(
                            "WholeBlob provisional base revision exceeds SQLite INTEGER"
                                .to_string(),
                        )
                    })?,
                    expected.base_blob_sha256(),
                    expected.run_id().0.to_string(),
                    expected.candidate_blob_sha256(),
                    i64::try_from(expected.candidate_sequence()).map_err(|_| {
                        RuntimeStoreError::WriteFailed(
                            "WholeBlob provisional sequence exceeds SQLite INTEGER".to_string(),
                        )
                    })?,
                ],
            )
            .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
        if deleted != 1 {
            return Err(session_authority_conflict(
                runtime_id,
                "WholeBlob provisional row changed during promotion",
            ));
        }
        insert_compaction_projection_outbox_intents(
            tx,
            runtime_id,
            &stored.compaction_projection_intents,
        )?;
        upsert_runtime_session_catalog_entry_in_txn(tx, runtime_id, &catalog_entry)?;
        tx.execute(
            "DELETE FROM runtime_session_snapshots WHERE runtime_id = ?1",
            params![runtime_id_text(runtime_id)],
        )
        .map_err(|error| map_runtime_snapshot_mutation_error(runtime_id, error))?;
        tx.execute(
            r"
            DELETE FROM runtime_whole_blob_bodies
            WHERE blob_sha256 = ?1
              AND NOT EXISTS (
                  SELECT 1 FROM runtime_whole_blob_authority
                  WHERE blob_sha256 = ?1
              )
              AND NOT EXISTS (
                  SELECT 1 FROM runtime_whole_blob_provisional_tails
                  WHERE candidate_blob_sha256 = ?1
              )
            ",
            params![current.blob_sha256()],
        )
        .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
        clear_runtime_projection_quarantine(tx, runtime_id)?;
        WholeBlobStoreAuthority::issued(
            expected.session_id().clone(),
            next_revision,
            expected.candidate_blob_sha256().to_string(),
        )
    }

    fn install_repaired_whole_blob_recovery_in_txn(
        tx: &Transaction<'_>,
        runtime_id: &LogicalRuntimeId,
        expected: &WholeBlobProvisionalTailAuthority,
        repaired_bytes: &[u8],
        recovered_blob_sha256: &str,
        catalog_entry: &crate::store::RuntimeSessionCatalogEntry,
        compaction_intents: &[meerkat_core::CompactionProjectionIntent],
    ) -> Result<WholeBlobStoreAuthority, RuntimeStoreError> {
        let stored = load_whole_blob_provisional_metadata(tx, runtime_id)?.ok_or_else(|| {
            session_authority_conflict(
                runtime_id,
                "WholeBlob repaired recovery candidate is absent",
            )
        })?;
        let current = load_whole_blob_store_authority(tx, runtime_id)?.ok_or_else(|| {
            session_authority_conflict(
                runtime_id,
                "WholeBlob repaired recovery has no committed base",
            )
        })?;
        if stored.authority != *expected
            || current.session_id() != expected.session_id()
            || current.store_revision() != expected.base_store_revision()
            || current.blob_sha256() != expected.base_blob_sha256()
            || recovered_blob_sha256 == expected.candidate_blob_sha256()
            || catalog_entry.session_id() != expected.session_id()
            || catalog_entry.persistence_profile() != RuntimeSessionPersistenceProfile::WholeBlobV1
            || compaction_intents
                .iter()
                .any(|intent| intent.projection.session_id() != expected.session_id())
        {
            return Err(session_authority_conflict(
                runtime_id,
                "WholeBlob repaired recovery does not exactly bind its base/candidate/successor",
            ));
        }
        reject_finalized_compaction_projection_replays(tx, runtime_id, compaction_intents)?;
        let next_revision = current.store_revision().checked_add(1).ok_or_else(|| {
            RuntimeStoreError::WriteFailed(format!(
                "WholeBlob store revision exhausted for runtime {runtime_id}"
            ))
        })?;
        let next_revision_i64 = i64::try_from(next_revision).map_err(|_| {
            RuntimeStoreError::WriteFailed(format!(
                "WholeBlob store revision exceeds SQLite INTEGER for runtime {runtime_id}"
            ))
        })?;
        tx.execute(
            r"
            INSERT INTO runtime_whole_blob_bodies (blob_sha256, session_snapshot)
            VALUES (?1, ?2)
            ON CONFLICT(blob_sha256) DO NOTHING
            ",
            params![recovered_blob_sha256, repaired_bytes],
        )
        .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
        let repaired_body_matches = tx
            .query_row(
                r"
                SELECT 1
                FROM runtime_whole_blob_bodies
                WHERE blob_sha256 = ?1 AND session_snapshot = ?2
                ",
                params![recovered_blob_sha256, repaired_bytes],
                |_row| Ok(()),
            )
            .optional()
            .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?
            .is_some();
        if !repaired_body_matches {
            return Err(session_authority_conflict(
                runtime_id,
                "WholeBlob repaired digest already names different stored bytes",
            ));
        }
        let updated = tx
            .execute(
                r"
                UPDATE runtime_whole_blob_authority
                SET session_id = ?2, store_revision = ?3, blob_sha256 = ?4
                WHERE runtime_id = ?1
                  AND session_id = ?2
                  AND store_revision = ?5
                  AND blob_sha256 = ?6
                ",
                params![
                    runtime_id_text(runtime_id),
                    expected.session_id().to_string(),
                    next_revision_i64,
                    recovered_blob_sha256,
                    i64::try_from(current.store_revision()).map_err(|_| {
                        RuntimeStoreError::WriteFailed(
                            "WholeBlob current revision exceeds SQLite INTEGER".to_string(),
                        )
                    })?,
                    current.blob_sha256(),
                ],
            )
            .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
        if updated != 1 {
            return Err(session_authority_conflict(
                runtime_id,
                "WholeBlob repaired recovery base changed during commit",
            ));
        }
        let deleted = tx
            .execute(
                r"
                DELETE FROM runtime_whole_blob_provisional_tails
                WHERE runtime_id = ?1
                  AND session_id = ?2
                  AND base_store_revision = ?3
                  AND base_blob_sha256 = ?4
                  AND run_id = ?5
                  AND candidate_blob_sha256 = ?6
                  AND candidate_sequence = ?7
                ",
                params![
                    runtime_id_text(runtime_id),
                    expected.session_id().to_string(),
                    i64::try_from(expected.base_store_revision()).map_err(|_| {
                        RuntimeStoreError::WriteFailed(
                            "WholeBlob provisional base revision exceeds SQLite INTEGER"
                                .to_string(),
                        )
                    })?,
                    expected.base_blob_sha256(),
                    expected.run_id().0.to_string(),
                    expected.candidate_blob_sha256(),
                    i64::try_from(expected.candidate_sequence()).map_err(|_| {
                        RuntimeStoreError::WriteFailed(
                            "WholeBlob provisional sequence exceeds SQLite INTEGER".to_string(),
                        )
                    })?,
                ],
            )
            .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
        if deleted != 1 {
            return Err(session_authority_conflict(
                runtime_id,
                "WholeBlob repaired recovery provisional row changed during commit",
            ));
        }
        insert_compaction_projection_outbox_intents(tx, runtime_id, compaction_intents)?;
        upsert_runtime_session_catalog_entry_in_txn(tx, runtime_id, catalog_entry)?;
        tx.execute(
            "DELETE FROM runtime_session_snapshots WHERE runtime_id = ?1",
            params![runtime_id_text(runtime_id)],
        )
        .map_err(|error| map_runtime_snapshot_mutation_error(runtime_id, error))?;
        for obsolete_blob_sha256 in [current.blob_sha256(), expected.candidate_blob_sha256()] {
            tx.execute(
                r"
                DELETE FROM runtime_whole_blob_bodies
                WHERE blob_sha256 = ?1
                  AND NOT EXISTS (
                      SELECT 1 FROM runtime_whole_blob_authority
                      WHERE blob_sha256 = ?1
                  )
                  AND NOT EXISTS (
                      SELECT 1 FROM runtime_whole_blob_provisional_tails
                      WHERE candidate_blob_sha256 = ?1
                  )
                ",
                params![obsolete_blob_sha256],
            )
            .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
        }
        clear_runtime_projection_quarantine(tx, runtime_id)?;
        WholeBlobStoreAuthority::issued(
            expected.session_id().clone(),
            next_revision,
            recovered_blob_sha256.to_string(),
        )
    }

    fn promote_head_canonical_provisional_in_txn(
        tx: &Transaction<'_>,
        runtime_id: &LogicalRuntimeId,
        expected: &HeadCanonicalProvisionalTailAuthority,
        receipt: &RunBoundaryReceipt,
        runtime_state: Option<RuntimeState>,
    ) -> Result<HeadCanonicalStoreAuthority, RuntimeStoreError> {
        let stored = load_head_canonical_provisional_tail(tx, runtime_id)?.ok_or_else(|| {
            session_authority_conflict(
                runtime_id,
                "HeadCanonical provisional promotion candidate is absent",
            )
        })?;
        let current = load_head_canonical_authority(tx, runtime_id)?
            .and_then(|authority| authority.head_canonical().cloned())
            .ok_or_else(|| {
                session_authority_conflict(
                    runtime_id,
                    "HeadCanonical provisional promotion has no committed base",
                )
            })?;
        if &stored.authority != expected
            || expected.run_id() != &receipt.run_id
            || current.session_id() != expected.session_id()
            || current.store_revision() != expected.base_store_revision()
            || current.committed_head_token() != expected.base_committed_head_token()
        {
            return Err(session_authority_conflict(
                runtime_id,
                "HeadCanonical provisional promotion does not exactly match stored base/run/physical target",
            ));
        }
        if receipt.message_count != stored.candidate_message_count
            || receipt.conversation_digest.as_deref()
                != Some(stored.candidate_conversation_digest.as_str())
        {
            return Err(session_authority_conflict(
                runtime_id,
                "HeadCanonical final receipt does not bind the provisional candidate count/digest",
            ));
        }
        let (physical_head, physical_head_token) =
            meerkat_store::sqlite_store::load_head_canonical_for_runtime_in_txn(
                tx,
                expected.session_id(),
            )
            .map_err(|error| map_head_canonical_session_store_error(runtime_id, error))?
            .ok_or_else(|| {
                session_authority_conflict(
                    runtime_id,
                    "HeadCanonical provisional promotion has no physical target head",
                )
            })?;
        if physical_head_token != expected.physical_head_token()
            || physical_head.head_revision != stored.candidate_conversation_digest
            || physical_head.message_count
                != u64::try_from(stored.candidate_message_count).map_err(|_| {
                    session_authority_conflict(
                        runtime_id,
                        "HeadCanonical provisional candidate message count exceeds u64",
                    )
                })?
        {
            return Err(session_authority_conflict(
                runtime_id,
                "HeadCanonical provisional promotion physical head differs from the exact candidate",
            ));
        }
        validate_exact_head_prefix_authority(
            runtime_id,
            &physical_head,
            "HeadCanonical provisional promotion physical head",
        )?;
        meerkat_store::sqlite_store::verify_head_rewrite_prefix_descent_in_txn(
            tx,
            current.boundary_head(),
            &physical_head,
        )
        .map_err(|error| map_head_canonical_session_store_error(runtime_id, error))?;
        reject_finalized_compaction_projection_replays(
            tx,
            runtime_id,
            &stored.compaction_projection_intents,
        )?;
        let final_revision = expected
            .physical_store_revision()
            .checked_add(1)
            .ok_or_else(|| {
                session_authority_conflict(
                    runtime_id,
                    "HeadCanonical final promotion revision overflow",
                )
            })?;
        let committed = HeadCanonicalStoreAuthority::issued(
            expected.session_id().clone(),
            final_revision,
            physical_head,
            physical_head_token,
        )?;
        write_head_canonical_authority_in_txn(
            tx,
            runtime_id,
            &RuntimeSessionAuthority::HeadCanonical(committed.clone()),
        )?;
        insert_compaction_projection_outbox_intents(
            tx,
            runtime_id,
            &stored.compaction_projection_intents,
        )?;
        let mut catalog_entry = stored.catalog_entry;
        let runtime_state = match runtime_state {
            Some(runtime_state) => Some(runtime_state),
            None => load_runtime_session_catalog_entry_in_txn(tx, runtime_id)?
                .and_then(|entry| entry.runtime_state()),
        };
        catalog_entry.set_runtime_state(runtime_state);
        upsert_runtime_session_catalog_entry_in_txn(tx, runtime_id, &catalog_entry)?;
        clear_runtime_projection_quarantine(tx, runtime_id)?;
        Ok(committed)
    }

    fn commit_prepared_whole_blob_snapshot_in_txn(
        tx: &Transaction<'_>,
        runtime_id: &LogicalRuntimeId,
        prepared: crate::store::PreparedWholeBlobSnapshot,
    ) -> Result<WholeBlobStoreAuthority, RuntimeStoreError> {
        let (session, serialized, candidate_blob_sha256) = prepared.into_parts();
        if load_whole_blob_provisional_authority(tx, runtime_id)?.is_some() {
            return Err(session_authority_conflict(
                runtime_id,
                "ordinary WholeBlob write cannot bypass or re-encode a store-owned provisional candidate; use exact metadata-only promotion",
            ));
        }
        let runtime_state = load_runtime_session_catalog_entry_in_txn(tx, runtime_id)?
            .and_then(|entry| entry.runtime_state());
        let catalog_entry = crate::store::RuntimeSessionCatalogEntry::from_session(
            session.as_ref(),
            RuntimeSessionPersistenceProfile::WholeBlobV1,
            runtime_state,
        )?;
        upsert_runtime_snapshot_issued(
            tx,
            runtime_id,
            serialized.session_snapshot.as_ref(),
            session.id(),
            &candidate_blob_sha256,
            &catalog_entry,
        )
    }

    fn set_runtime_projection_quarantine(
        tx: &Transaction<'_>,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<(), RuntimeStoreError> {
        tx.execute(
            r"
            INSERT OR REPLACE INTO runtime_projection_quarantine (runtime_id)
            VALUES (?1)
            ",
            params![runtime_id_text(runtime_id)],
        )
        .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
        Ok(())
    }

    fn clear_runtime_projection_quarantine(
        tx: &Transaction<'_>,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<(), RuntimeStoreError> {
        tx.execute(
            "DELETE FROM runtime_projection_quarantine WHERE runtime_id = ?1",
            params![runtime_id_text(runtime_id)],
        )
        .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
        Ok(())
    }

    fn insert_compaction_projection_outbox_intents(
        tx: &Transaction<'_>,
        runtime_id: &LogicalRuntimeId,
        intents: &[meerkat_core::CompactionProjectionIntent],
    ) -> Result<(), RuntimeStoreError> {
        for intent in intents {
            let encoded = serde_json::to_vec(intent)
                .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
            tx.execute(
                r"
                INSERT OR IGNORE INTO runtime_compaction_projection_outbox
                    (runtime_id, session_id, parent_revision, revision, commit_fingerprint, intent_json, state)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending')
                ",
                params![
                    runtime_id_text(runtime_id),
                    intent.projection.session_id().to_string(),
                    intent.projection.parent_revision(),
                    intent.projection.revision(),
                    intent.projection.commit_fingerprint(),
                    encoded,
                ],
            )
            .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
            let existing = tx
                .query_row(
                    r"
                    SELECT intent_json
                    FROM runtime_compaction_projection_outbox
                    WHERE runtime_id = ?1 AND session_id = ?2
                      AND parent_revision = ?3 AND revision = ?4
                      AND commit_fingerprint = ?5
                    ",
                    params![
                        runtime_id_text(runtime_id),
                        intent.projection.session_id().to_string(),
                        intent.projection.parent_revision(),
                        intent.projection.revision(),
                        intent.projection.commit_fingerprint(),
                    ],
                    |row| Ok(row.get::<_, JsonColumnBytes>(0)?.into_bytes()),
                )
                .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
            let existing: meerkat_core::CompactionProjectionIntent =
                serde_json::from_slice(&existing)
                    .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
            if existing != *intent {
                return Err(RuntimeStoreError::WriteFailed(format!(
                    "conflicting compaction outbox intent for rewrite {}",
                    intent.projection.revision()
                )));
            }
        }
        Ok(())
    }

    fn ensure_compaction_intents_already_outboxed(
        tx: &Transaction<'_>,
        runtime_id: &LogicalRuntimeId,
        session: &meerkat_core::Session,
    ) -> Result<(), RuntimeStoreError> {
        let intents = crate::store::validated_compaction_projection_intents(session)?;
        ensure_compaction_intents_already_outboxed_list(tx, runtime_id, &intents)
    }

    fn ensure_compaction_intents_already_outboxed_list(
        tx: &Transaction<'_>,
        runtime_id: &LogicalRuntimeId,
        intents: &[meerkat_core::CompactionProjectionIntent],
    ) -> Result<(), RuntimeStoreError> {
        for intent in intents {
            let (encoded, state) = tx
                .query_row(
                    r"
                    SELECT intent_json, state
                    FROM runtime_compaction_projection_outbox
                    WHERE runtime_id = ?1 AND session_id = ?2
                      AND parent_revision = ?3 AND revision = ?4
                      AND commit_fingerprint = ?5
                    ",
                    params![
                        runtime_id_text(runtime_id),
                        intent.projection.session_id().to_string(),
                        intent.projection.parent_revision(),
                        intent.projection.revision(),
                        intent.projection.commit_fingerprint(),
                    ],
                    |row| {
                        Ok((
                            row.get::<_, JsonColumnBytes>(0)?.into_bytes(),
                            row.get::<_, String>(1)?,
                        ))
                    },
                )
                .optional()
                .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?
                .ok_or_else(|| {
                    RuntimeStoreError::WriteFailed(format!(
                        "non-boundary snapshot introduces compaction intent {} without atomic outbox authority",
                        intent.projection.revision()
                    ))
                })?;
            if state == "finalized" {
                return Err(RuntimeStoreError::WriteFailed(format!(
                    "non-boundary snapshot replays finalized compaction intent {}",
                    intent.projection.revision()
                )));
            }
            let existing: meerkat_core::CompactionProjectionIntent =
                serde_json::from_slice(&encoded)
                    .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
            if existing != *intent {
                return Err(RuntimeStoreError::WriteFailed(format!(
                    "non-boundary snapshot conflicts with compaction outbox rewrite {}",
                    intent.projection.revision()
                )));
            }
        }
        Ok(())
    }

    fn reject_finalized_compaction_projection_replays(
        tx: &Transaction<'_>,
        runtime_id: &LogicalRuntimeId,
        intents: &[meerkat_core::CompactionProjectionIntent],
    ) -> Result<(), RuntimeStoreError> {
        for intent in intents {
            let state = tx
                .query_row(
                    r"
                    SELECT state
                    FROM runtime_compaction_projection_outbox
                    WHERE runtime_id = ?1 AND session_id = ?2
                      AND parent_revision = ?3 AND revision = ?4
                      AND commit_fingerprint = ?5
                    ",
                    params![
                        runtime_id_text(runtime_id),
                        intent.projection.session_id().to_string(),
                        intent.projection.parent_revision(),
                        intent.projection.revision(),
                        intent.projection.commit_fingerprint(),
                    ],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
            if state.as_deref() == Some("finalized") {
                return Err(RuntimeStoreError::WriteFailed(format!(
                    "atomic session snapshot replays finalized compaction intent {}",
                    intent.projection.revision()
                )));
            }
        }
        Ok(())
    }

    fn insert_receipt(
        tx: &Transaction<'_>,
        runtime_id: &LogicalRuntimeId,
        receipt: &RunBoundaryReceipt,
    ) -> Result<(), RuntimeStoreError> {
        let receipt_json = serde_json::to_vec(receipt)
            .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
        tx.execute(
            r"
            INSERT INTO runtime_boundary_receipts (runtime_id, run_id, sequence, receipt_json)
            VALUES (?1, ?2, ?3, ?4)
            ",
            params![
                runtime_id_text(runtime_id),
                receipt.run_id.0.to_string(),
                encode_receipt_sequence(receipt.sequence),
                receipt_json,
            ],
        )
        .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
        Ok(())
    }

    /// Exact target-local compare token for one stored input row's bytes.
    /// Same construction as `MachineLifecycleObservationVersion`.
    fn input_row_version_digest(bytes: &[u8]) -> String {
        use sha2::Digest as _;
        format!("sha256:{:x}", sha2::Sha256::digest(bytes))
    }

    fn sqlite_text_evidence(
        value: rusqlite::types::ValueRef<'_>,
    ) -> (String, Option<&'static str>) {
        match value {
            rusqlite::types::ValueRef::Text(bytes) => match std::str::from_utf8(bytes) {
                Ok(value) => (value.to_string(), None),
                Err(_) => (
                    format!("<non-UTF-8 TEXT {}>", input_row_version_digest(bytes)),
                    Some("non-UTF-8 TEXT"),
                ),
            },
            rusqlite::types::ValueRef::Blob(bytes) => (
                format!("<BLOB {}>", input_row_version_digest(bytes)),
                Some("BLOB"),
            ),
            rusqlite::types::ValueRef::Integer(value) => (value.to_string(), Some("INTEGER")),
            rusqlite::types::ValueRef::Real(value) => (value.to_string(), Some("REAL")),
            rusqlite::types::ValueRef::Null => ("<NULL>".to_string(), Some("NULL")),
        }
    }

    fn load_recovery_input_set_revision(
        conn: &Connection,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<RecoveryInputSetRevision, RuntimeStoreError> {
        let revision = conn
            .query_row(
                r"
                SELECT revision
                FROM runtime_input_set_revisions
                WHERE runtime_id = ?1
                ",
                params![runtime_id_text(runtime_id)],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
        let generation = match revision {
            Some(revision) => u64::try_from(revision).map_err(|_| {
                RuntimeStoreError::ReadFailed(format!(
                    "input-set revision for runtime {runtime_id} is negative"
                ))
            })?,
            None => 0,
        };
        Ok(RecoveryInputSetRevision::from_store_generation(generation))
    }

    fn load_recovery_nonterminal_input_snapshot(
        conn: &Connection,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<PreparedRecoveryInputSnapshot, RuntimeStoreError> {
        let revision = load_recovery_input_set_revision(conn, runtime_id)?;
        let query = format!(
            r"
            SELECT input_id, state_json
            FROM runtime_input_states
                INDEXED BY idx_runtime_input_states_recovery_nonterminal
            WHERE runtime_id = ?1
              AND {RECOVERY_NONTERMINAL_INPUT_PREDICATE_SQL}
            ORDER BY input_id ASC
            "
        );
        let mut stmt = conn
            .prepare(&query)
            .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
        let raw_rows = stmt
            .query_map(params![runtime_id_text(runtime_id)], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, JsonColumnBytes>(1)?.into_bytes(),
                ))
            })
            .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
        let mut rows = Vec::with_capacity(raw_rows.len());
        for (stored_input_id, bytes) in raw_rows {
            let state = match deserialize_persisted_input_state(&bytes) {
                Ok(state) => state,
                Err(error) => {
                    tracing::error!(
                        runtime_id = %runtime_id,
                        input_id = %stored_input_id,
                        detail = %error,
                        "exact durable-tail input observation found a forensic corrupt row; classifying the input evidence as unfenceable"
                    );
                    // `observe_candidate_run_inputs` maps Unsupported to the
                    // generated machine's stable Unfenceable evidence class.
                    // ReadFailed would instead enter RecoveryBackoff forever,
                    // while excluding this row would mint false set absence.
                    return Err(RuntimeStoreError::Unsupported(format!(
                        "exact recovery input set for runtime {runtime_id} contains forensic corrupt row `{stored_input_id}`"
                    )));
                }
            };
            if state.state.input_id.to_string() != stored_input_id {
                tracing::error!(
                    runtime_id = %runtime_id,
                    stored_input_id = %stored_input_id,
                    decoded_input_id = %state.state.input_id,
                    "exact durable-tail input observation found a row-key identity mismatch; classifying the input evidence as unfenceable"
                );
                return Err(RuntimeStoreError::Unsupported(format!(
                    "exact recovery input set for runtime {runtime_id} contains row-key mismatch `{stored_input_id}`"
                )));
            }
            rows.push((state, input_row_version_digest(&bytes)));
        }
        PreparedRecoveryInputSnapshot::from_exact_nonterminal_rows(
            runtime_id.clone(),
            revision,
            rows,
        )
    }

    fn enforce_recovery_input_set_authority(
        tx: &Transaction<'_>,
        runtime_id: &LogicalRuntimeId,
        expected_revision: RecoveryInputSetRevision,
        expected_exact_set_token: &str,
    ) -> Result<(), RuntimeStoreError> {
        let current = load_recovery_nonterminal_input_snapshot(tx, runtime_id)?;
        if current.input_set_revision() != expected_revision
            || current.exact_set_token() != expected_exact_set_token
        {
            return Err(RuntimeStoreError::RecoveryInputSetConflict {
                runtime_id: runtime_id.to_string(),
            });
        }
        Ok(())
    }

    enum SqliteRecoveryInputMutation {
        Upsert {
            replacement: StoredInputState,
            target_bytes: Vec<u8>,
        },
        Delete {
            input_id: InputId,
        },
    }

    impl SqliteRecoveryInputMutation {
        fn input_id(&self) -> &InputId {
            match self {
                Self::Upsert { replacement, .. } => &replacement.state.input_id,
                Self::Delete { input_id } => input_id,
            }
        }
    }

    /// Release every target row's old idempotency mapping before applying the
    /// first sibling mutation. SQLite enforces UNIQUE constraints immediately,
    /// so updating A(x)->A(y) while B still owns y rejects a valid final-image
    /// swap unless the complete target set relinquishes its prior mappings
    /// first. The enclosing transaction keeps this release invisible and rolls
    /// it back with any later write failure.
    fn release_input_idempotency_keys_for_mutation_set(
        tx: &Transaction<'_>,
        runtime_id: &LogicalRuntimeId,
        target_input_ids: impl IntoIterator<Item = String>,
    ) -> Result<(), RuntimeStoreError> {
        let mut unique_targets = HashSet::new();
        for input_id in target_input_ids {
            if !unique_targets.insert(input_id.clone()) {
                return Err(RuntimeStoreError::WriteFailed(format!(
                    "atomic input-state mutation set repeats input {input_id} in runtime {runtime_id}"
                )));
            }
        }
        for input_id in unique_targets {
            tx.execute(
                r"
                DELETE FROM runtime_input_idempotency_keys
                WHERE runtime_id = ?1 AND input_id = ?2
                ",
                params![runtime_id_text(runtime_id), input_id],
            )
            .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
        }
        Ok(())
    }

    fn prepare_current_sqlite_recovery_input_mutations(
        tx: &Transaction<'_>,
        runtime_id: &LogicalRuntimeId,
        expected_revision: RecoveryInputSetRevision,
        prepared: &[PreparedRecoveryInputStateMutation],
    ) -> Result<Option<Vec<SqliteRecoveryInputMutation>>, RuntimeStoreError> {
        if load_recovery_input_set_revision(tx, runtime_id)? != expected_revision {
            return Ok(None);
        }
        let mut changed = Vec::new();
        for mutation in prepared {
            let current = tx
                .query_row(
                    r"
                    SELECT state_json
                    FROM runtime_input_states
                    WHERE runtime_id = ?1 AND input_id = ?2
                    ",
                    params![runtime_id_text(runtime_id), mutation.input_id().to_string()],
                    |row| Ok(row.get::<_, JsonColumnBytes>(0)?.into_bytes()),
                )
                .optional()
                .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
            let Some(current) = current else {
                return Ok(None);
            };
            if input_row_version_digest(&current) != mutation.expected_row_digest() {
                return Ok(None);
            }
            match mutation {
                PreparedRecoveryInputStateMutation::Upsert { replacement, .. } => {
                    let target_bytes = serde_json::to_vec(replacement)
                        .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
                    if current != target_bytes {
                        changed.push(SqliteRecoveryInputMutation::Upsert {
                            replacement: replacement.clone(),
                            target_bytes,
                        });
                    }
                }
                PreparedRecoveryInputStateMutation::Delete { input_id, .. } => {
                    changed.push(SqliteRecoveryInputMutation::Delete {
                        input_id: input_id.clone(),
                    });
                }
            }
        }
        Ok(Some(changed))
    }

    fn apply_sqlite_recovery_input_mutations(
        tx: &Transaction<'_>,
        runtime_id: &LogicalRuntimeId,
        mutations: &[SqliteRecoveryInputMutation],
    ) -> Result<(), RuntimeStoreError> {
        release_input_idempotency_keys_for_mutation_set(
            tx,
            runtime_id,
            mutations
                .iter()
                .map(|mutation| mutation.input_id().to_string()),
        )?;
        for mutation in mutations {
            match mutation {
                SqliteRecoveryInputMutation::Upsert {
                    replacement,
                    target_bytes,
                } => {
                    tx.execute(
                        r"
                        INSERT INTO runtime_input_states (runtime_id, input_id, state_json)
                        VALUES (?1, ?2, ?3)
                        ON CONFLICT(runtime_id, input_id) DO UPDATE
                        SET state_json = excluded.state_json
                        ",
                        params![
                            runtime_id_text(runtime_id),
                            replacement.state.input_id.to_string(),
                            target_bytes,
                        ],
                    )
                    .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
                    update_pending_terminal_owner_index(tx, runtime_id, replacement)?;
                }
                SqliteRecoveryInputMutation::Delete { input_id } => {
                    let deleted = tx
                        .execute(
                            r"
                            DELETE FROM runtime_input_states
                            WHERE runtime_id = ?1 AND input_id = ?2
                            ",
                            params![runtime_id_text(runtime_id), input_id.to_string()],
                        )
                        .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
                    if deleted != 1 {
                        return Err(RuntimeStoreError::InputRowVersionConflict {
                            input_id: input_id.to_string(),
                        });
                    }
                    tx.execute(
                        r"
                        DELETE FROM runtime_pending_terminal_owners
                        WHERE runtime_id = ?1 AND owner_input_id = ?2
                        ",
                        params![runtime_id_text(runtime_id), input_id.to_string()],
                    )
                    .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
                }
            }
        }
        Ok(())
    }

    /// Enforce a machine-lifecycle commit's expected prior row version inside
    /// the writing transaction. `Missing` demands the row still be absent; a
    /// concrete version demands the stored bytes still hash to it.
    fn enforce_machine_lifecycle_expected_version(
        tx: &Transaction<'_>,
        runtime_id: &LogicalRuntimeId,
        expected: &MachineLifecycleExpectedVersion,
    ) -> Result<(), RuntimeStoreError> {
        let existing = tx
            .query_row(
                "SELECT runtime_state_json FROM runtime_states WHERE runtime_id = ?1",
                params![runtime_id_text(runtime_id)],
                |row| Ok(row.get::<_, JsonColumnBytes>(0)?.into_bytes()),
            )
            .optional()
            .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))?;
        let matches = match expected {
            MachineLifecycleExpectedVersion::Missing => existing.is_none(),
            MachineLifecycleExpectedVersion::Version(version) => {
                existing.as_deref().is_some_and(|bytes| {
                    MachineLifecycleObservationVersion::from_raw_record(bytes) == *version
                })
            }
        };
        if !matches {
            return Err(RuntimeStoreError::MachineLifecycleVersionConflict {
                runtime_id: runtime_id_text(runtime_id).to_owned(),
            });
        }
        Ok(())
    }

    fn upsert_input_states(
        tx: &Transaction<'_>,
        runtime_id: &LogicalRuntimeId,
        input_states: &[(StoredInputState, Option<String>)],
    ) -> Result<(), RuntimeStoreError> {
        // Prove every fenced predecessor before releasing any old index key.
        // The transaction would roll a failed proof back either way, but
        // separating observation from realization gives this batch the same
        // final-image semantics as the in-memory implementation.
        for (bundle, expected_row_digest) in input_states {
            if let Some(expected) = expected_row_digest {
                let existing = tx
                    .query_row(
                        r"
                        SELECT state_json FROM runtime_input_states
                        WHERE runtime_id = ?1 AND input_id = ?2
                        ",
                        params![
                            runtime_id_text(runtime_id),
                            bundle.state.input_id.0.to_string()
                        ],
                        |row| Ok(row.get::<_, JsonColumnBytes>(0)?.into_bytes()),
                    )
                    .optional()
                    .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))?;
                let matches = existing
                    .as_deref()
                    .is_some_and(|bytes| input_row_version_digest(bytes) == *expected);
                if !matches {
                    return Err(RuntimeStoreError::InputRowVersionConflict {
                        input_id: bundle.state.input_id.0.to_string(),
                    });
                }
            }
        }
        release_input_idempotency_keys_for_mutation_set(
            tx,
            runtime_id,
            input_states
                .iter()
                .map(|(bundle, _)| bundle.state.input_id.to_string()),
        )?;
        for (bundle, _) in input_states {
            let state_json = serde_json::to_vec(bundle)
                .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
            tx.execute(
                r"
                INSERT INTO runtime_input_states (runtime_id, input_id, state_json)
                VALUES (?1, ?2, ?3)
                ON CONFLICT(runtime_id, input_id) DO UPDATE SET state_json = excluded.state_json
                ",
                params![
                    runtime_id_text(runtime_id),
                    bundle.state.input_id.0.to_string(),
                    state_json
                ],
            )
            .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
            update_pending_terminal_owner_index(tx, runtime_id, bundle)?;
        }
        Ok(())
    }

    fn update_pending_terminal_owner_index(
        tx: &Transaction<'_>,
        runtime_id: &LogicalRuntimeId,
        bundle: &StoredInputState,
    ) -> Result<(), RuntimeStoreError> {
        if crate::store::input_state_is_pending_terminal_owner(&bundle.state) {
            tx.execute(
                r"
                INSERT INTO runtime_pending_terminal_owners
                    (runtime_id, owner_input_id)
                VALUES (?1, ?2)
                ON CONFLICT(runtime_id, owner_input_id) DO NOTHING
                ",
                params![
                    runtime_id_text(runtime_id),
                    bundle.state.input_id.0.to_string()
                ],
            )
        } else {
            tx.execute(
                r"
                DELETE FROM runtime_pending_terminal_owners
                WHERE runtime_id = ?1 AND owner_input_id = ?2
                ",
                params![
                    runtime_id_text(runtime_id),
                    bundle.state.input_id.0.to_string()
                ],
            )
        }
        .map(|_| ())
        .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))
    }

    fn upsert_machine_lifecycle_snapshot(
        tx: &Transaction<'_>,
        runtime_id: &LogicalRuntimeId,
        snapshot: &MachineLifecycleSnapshot,
    ) -> Result<(), RuntimeStoreError> {
        let state_json = MachineLifecycleStoreRecord::from_snapshot(snapshot).encode()?;
        tx.execute(
            r"
            INSERT INTO runtime_states (runtime_id, runtime_state_json)
            VALUES (?1, ?2)
            ON CONFLICT(runtime_id) DO UPDATE SET runtime_state_json = excluded.runtime_state_json
            ",
            params![runtime_id_text(runtime_id), state_json],
        )
        .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
        Ok(())
    }

    #[derive(Debug, PartialEq, Eq)]
    struct UnregisterFinalizationObservation {
        lifecycle_record: Option<Vec<u8>>,
        input_state_records: Vec<(String, Option<Vec<u8>>)>,
        ops_record: Option<Vec<u8>>,
        retired_ops_epoch_present: bool,
    }

    fn observe_unregister_finalization(
        conn: &Connection,
        runtime_id: &LogicalRuntimeId,
        input_ids: &[String],
        retired_ops_epoch: &meerkat_core::RuntimeEpochId,
    ) -> Result<UnregisterFinalizationObservation, RuntimeStoreError> {
        let lifecycle_record = conn
            .query_row(
                "SELECT runtime_state_json FROM runtime_states WHERE runtime_id = ?1",
                params![runtime_id_text(runtime_id)],
                |row| Ok(row.get::<_, JsonColumnBytes>(0)?.into_bytes()),
            )
            .optional()
            .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))?;
        let input_state_records = input_ids
            .iter()
            .map(|input_id| {
                conn.query_row(
                    r"
                    SELECT state_json
                    FROM runtime_input_states
                    WHERE runtime_id = ?1 AND input_id = ?2
                    ",
                    params![runtime_id_text(runtime_id), input_id],
                    |row| Ok(row.get::<_, JsonColumnBytes>(0)?.into_bytes()),
                )
                .optional()
                .map(|record| (input_id.clone(), record))
                .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let ops_record = conn
            .query_row(
                "SELECT state_json FROM runtime_ops_lifecycle WHERE runtime_id = ?1",
                params![runtime_id_text(runtime_id)],
                |row| Ok(row.get::<_, JsonColumnBytes>(0)?.into_bytes()),
            )
            .optional()
            .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))?;
        let retired_ops_epoch_present = conn
            .query_row(
                r"
                SELECT 1
                FROM runtime_retired_ops_epochs
                WHERE runtime_id = ?1 AND epoch_id = ?2
                ",
                params![runtime_id_text(runtime_id), retired_ops_epoch.to_string()],
                |_row| Ok(()),
            )
            .optional()
            .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))?
            .is_some();
        Ok(UnregisterFinalizationObservation {
            lifecycle_record,
            input_state_records,
            ops_record,
            retired_ops_epoch_present,
        })
    }

    /// SQLite-backed runtime store sharing the same sqlite file as `SqliteSessionStore`.
    pub struct SqliteRuntimeStore {
        path: PathBuf,
        session_persistence_profile: RuntimeSessionPersistenceProfile,
        #[cfg(test)]
        unregister_finalization_fault: AtomicU8,
        /// Candidate bytes shipped into the snapshot byte-equality probe.
        /// Observability seam for the length-gate regression tests only.
        #[cfg(test)]
        snapshot_byte_probe_bytes: std::sync::Arc<std::sync::atomic::AtomicU64>,
    }

    impl SqliteRuntimeStore {
        /// Open the compatibility whole-BLOB runtime store.
        ///
        /// New SQLite realm composition should choose
        /// [`Self::new_head_canonical`] explicitly. This constructor preserves
        /// the established public default for independent RuntimeStore files.
        pub fn new(path: impl Into<PathBuf>) -> Result<Self, RuntimeStoreError> {
            Self::new_whole_blob(path)
        }

        /// Open an explicit whole-BLOB runtime authority.
        pub fn new_whole_blob(path: impl Into<PathBuf>) -> Result<Self, RuntimeStoreError> {
            let path = path.into();
            let conn = open_runtime_connection(&path)?;
            if head_canonical_profile_has_durable_claim(&conn)? {
                return Err(RuntimeStoreError::Unsupported(
                    "SQLite runtime database is irreversibly pinned to head_canonical_v1"
                        .to_string(),
                ));
            }
            drop(conn);
            Ok(Self {
                path,
                session_persistence_profile: RuntimeSessionPersistenceProfile::WholeBlobV1,
                #[cfg(test)]
                unregister_finalization_fault: AtomicU8::new(0),
                #[cfg(test)]
                snapshot_byte_probe_bytes: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(
                    0,
                )),
            })
        }

        /// Open a head-canonical runtime authority co-located with the SQLite
        /// session store in the same database file.
        ///
        /// This constructor is the explicit profile-activation boundary. It
        /// synchronously converts every durable whole-BLOB predecessor before
        /// returning; an `in_progress` activation is retried, and any
        /// unverifiable source refuses construction rather than leaking an
        /// O(document) migration into the first ordinary service boundary.
        pub fn new_head_canonical(path: impl Into<PathBuf>) -> Result<Self, RuntimeStoreError> {
            let path = path.into();
            let mut conn = open_head_canonical_runtime_connection(&path)?;
            pin_head_canonical_profile(&mut conn)?;
            activate_head_canonical_profiles(&mut conn)?;
            drop(conn);
            Ok(Self {
                path,
                session_persistence_profile: RuntimeSessionPersistenceProfile::HeadCanonicalV1,
                #[cfg(test)]
                unregister_finalization_fault: AtomicU8::new(0),
                #[cfg(test)]
                snapshot_byte_probe_bytes: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(
                    0,
                )),
            })
        }

        pub fn path(&self) -> &Path {
            &self.path
        }

        fn require_whole_blob_session_operation(
            &self,
            runtime_id: &LogicalRuntimeId,
            operation: &str,
        ) -> Result<(), RuntimeStoreError> {
            if self.session_persistence_profile == RuntimeSessionPersistenceProfile::WholeBlobV1 {
                return Ok(());
            }
            Err(session_authority_conflict(
                runtime_id,
                format!("{operation} is a whole-BLOB operation but this store is head-canonical"),
            ))
        }

        fn require_head_canonical_session_operation(
            &self,
            runtime_id: &LogicalRuntimeId,
            operation: &str,
        ) -> Result<(), RuntimeStoreError> {
            if self.session_persistence_profile == RuntimeSessionPersistenceProfile::HeadCanonicalV1
            {
                return Ok(());
            }
            Err(session_authority_conflict(
                runtime_id,
                format!("{operation} is a head-canonical operation but this store is whole-BLOB"),
            ))
        }

        #[cfg(test)]
        fn inject_unregister_finalization_fault(&self, fault: u8) {
            self.unregister_finalization_fault
                .store(fault, Ordering::SeqCst);
        }

        /// Total candidate bytes this store has shipped into the snapshot
        /// byte-equality probe. Length-gate regression tests only.
        #[cfg(test)]
        fn snapshot_byte_probe_bytes(&self) -> u64 {
            self.snapshot_byte_probe_bytes
                .load(std::sync::atomic::Ordering::Relaxed)
        }

        /// Whole-BLOB snapshot commit with the ordinary continuity guard.
        async fn commit_session_snapshot_checked(
            &self,
            runtime_id: &LogicalRuntimeId,
            session_delta: SerializedSessionSnapshot,
        ) -> Result<(), RuntimeStoreError> {
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            #[cfg(test)]
            let snapshot_byte_probe_bytes = std::sync::Arc::clone(&self.snapshot_byte_probe_bytes);
            tokio::task::spawn_blocking(move || {
                let incoming: meerkat_core::Session =
                    meerkat_core::Session::from_persisted_bytes(&session_delta.session_snapshot)
                        .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                let mut conn = open_runtime_connection(&path)?;
                let tx = begin_runtime_transaction(&mut conn)?;
                refuse_whole_blob_write_under_head_authority(
                    &tx,
                    &runtime_id,
                    "whole-BLOB session snapshot commit",
                )?;
                ensure_compaction_intents_already_outboxed(&tx, &runtime_id, &incoming)?;
                // Byte equality requires equal lengths, and `length()` on a
                // BLOB reads the record header, not the payload. Ordinary
                // turns grow the document, so the unchanged-snapshot probe
                // usually costs one integer compare instead of shipping the
                // whole candidate blob into SQLite to hear "no".
                let candidate_len = i64::try_from(session_delta.session_snapshot.len()).ok();
                let stored_len = tx
                    .query_row(
                        "SELECT length(session_snapshot) FROM runtime_session_snapshots WHERE runtime_id = ?1",
                        params![runtime_id_text(&runtime_id)],
                        |row| row.get::<_, Option<i64>>(0),
                    )
                    .optional()
                    .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))?
                    .flatten();
                let snapshot_is_unchanged = candidate_len.is_some()
                    && stored_len == candidate_len
                    && {
                        #[cfg(test)]
                        snapshot_byte_probe_bytes.fetch_add(
                            session_delta.session_snapshot.len() as u64,
                            std::sync::atomic::Ordering::Relaxed,
                        );
                        tx.query_row(
                            "SELECT 1 FROM runtime_session_snapshots WHERE runtime_id = ?1 AND session_snapshot = ?2",
                            params![
                                runtime_id_text(&runtime_id),
                                session_delta.session_snapshot.as_ref()
                            ],
                            |row| row.get::<_, i64>(0),
                        )
                        .optional()
                        .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))?
                        .is_some()
                    };
                if snapshot_is_unchanged {
                    // Incoming bytes already crossed typed Session validation
                    // and compaction-intent authority. Exact identity means
                    // there is no prior BLOB to allocate/parse and no snapshot
                    // write. The self-guard preserves live-head coherence and
                    // every fail-closed save invariant before the fast return.
                    meerkat_core::session_store::run_boundary_snapshot_head_coherence_guard(
                        &incoming,
                    )
                    .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                    clear_runtime_projection_quarantine(&tx, &runtime_id)?;
                    tx.commit()
                        .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                    return Ok(());
                }
                let previous = tx
                    .query_row(
                        "SELECT session_snapshot FROM runtime_session_snapshots WHERE runtime_id = ?1",
                        params![runtime_id_text(&runtime_id)],
                        |row| Ok(row.get::<_, JsonColumnBytes>(0)?.into_bytes()),
                    )
                    .optional()
                    .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))?
                    .map(|bytes| deserialize_persisted_session(&bytes))
                    .transpose()?;
                meerkat_core::session_store::run_boundary_snapshot_save_guard(
                    &incoming,
                    previous.as_ref(),
                )
                .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                upsert_runtime_snapshot(&tx, &runtime_id, &session_delta.session_snapshot)?;
                tx.commit()
                    .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                Ok(())
            })
            .await
            .map_err(|err| RuntimeStoreError::Internal(format!("Task join failed: {err}")))?
        }

        async fn commit_prepared_whole_blob_boundary(
            &self,
            runtime_id: &LogicalRuntimeId,
            prepared_session: Option<crate::store::PreparedWholeBlobSnapshot>,
            receipt: Option<RunBoundaryReceipt>,
            machine_lifecycle: Option<MachineLifecycleCommit>,
            input_updates: Vec<InputStatePersistenceRecord>,
            session_store_key: Option<meerkat_core::types::SessionId>,
        ) -> Result<Option<WholeBlobStoreAuthority>, RuntimeStoreError> {
            self.require_whole_blob_session_operation(
                runtime_id,
                "commit_prepared_whole_blob_boundary",
            )?;
            let compaction_projection_intents = prepared_session
                .as_ref()
                .map(|prepared| {
                    crate::store::validated_compaction_projection_intents(prepared.session())
                })
                .transpose()?
                .unwrap_or_default();
            if let Some(prepared) = prepared_session.as_ref() {
                if &LogicalRuntimeId::for_session(prepared.session().id()) != runtime_id {
                    return Err(session_authority_conflict(
                        runtime_id,
                        "prepared WholeBlob boundary does not bind this runtime/session",
                    ));
                }
                if let Some(session_store_key) = session_store_key.as_ref()
                    && prepared.session().id() != session_store_key
                {
                    return Err(RuntimeStoreError::SessionKeyMismatch {
                        expected: session_store_key.clone(),
                        actual: prepared.session().id().clone(),
                    });
                }
            } else if let Some(session_store_key) = session_store_key.as_ref()
                && &LogicalRuntimeId::for_session(session_store_key) != runtime_id
            {
                return Err(session_authority_conflict(
                    runtime_id,
                    format!(
                        "receipt-only WholeBlob session key {session_store_key} does not own this runtime"
                    ),
                ));
            }

            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            let lifecycle_expected = machine_lifecycle
                .as_ref()
                .and_then(|commit| commit.expected_version().cloned());
            let lifecycle_snapshot = machine_lifecycle.map(MachineLifecycleCommit::into_snapshot);
            let input_updates = input_updates
                .into_iter()
                .map(InputStatePersistenceRecord::into_stored_and_expected)
                .collect::<Vec<_>>();
            tokio::task::spawn_blocking(move || {
                let mut conn = open_runtime_connection(&path)?;
                let tx = begin_runtime_transaction(&mut conn)?;
                refuse_whole_blob_write_under_head_authority(
                    &tx,
                    &runtime_id,
                    "prepared WholeBlob boundary",
                )?;
                if let Some(expected) = lifecycle_expected.as_ref() {
                    enforce_machine_lifecycle_expected_version(&tx, &runtime_id, expected)?;
                }
                reject_finalized_compaction_projection_replays(
                    &tx,
                    &runtime_id,
                    &compaction_projection_intents,
                )?;
                let authority = match prepared_session {
                    Some(prepared) => Some(commit_prepared_whole_blob_snapshot_in_txn(
                        &tx,
                        &runtime_id,
                        prepared,
                    )?),
                    None => {
                        if load_whole_blob_provisional_authority(&tx, &runtime_id)?.is_some() {
                            return Err(session_authority_conflict(
                                &runtime_id,
                                "receipt-only boundary cannot bypass a store-owned WholeBlob candidate",
                            ));
                        }
                        None
                    }
                };
                insert_compaction_projection_outbox_intents(
                    &tx,
                    &runtime_id,
                    &compaction_projection_intents,
                )?;
                if let Some(snapshot) = lifecycle_snapshot.as_ref() {
                    upsert_machine_lifecycle_snapshot(&tx, &runtime_id, snapshot)?;
                }
                if let Some(receipt) = receipt.as_ref() {
                    insert_receipt(&tx, &runtime_id, receipt)?;
                }
                upsert_input_states(&tx, &runtime_id, &input_updates)?;
                tx.commit()
                    .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
                Ok(authority)
            })
            .await
            .map_err(|error| RuntimeStoreError::Internal(format!("Task join failed: {error}")))?
        }

        async fn commit_whole_blob_provisional_promotion(
            &self,
            runtime_id: &LogicalRuntimeId,
            promotion: crate::store::PreparedWholeBlobProvisionalPromotion,
            receipt: RunBoundaryReceipt,
            machine_lifecycle: Option<MachineLifecycleCommit>,
            input_updates: Vec<InputStatePersistenceRecord>,
            session_store_key: meerkat_core::types::SessionId,
        ) -> Result<WholeBlobStoreAuthority, RuntimeStoreError> {
            self.require_whole_blob_session_operation(
                runtime_id,
                "commit_whole_blob_provisional_promotion",
            )?;
            let (authority, checkpoint_conversation_digest, checkpoint_message_count) =
                promotion.into_parts();
            if authority.session_id() != &session_store_key {
                return Err(RuntimeStoreError::SessionKeyMismatch {
                    expected: authority.session_id().clone(),
                    actual: session_store_key,
                });
            }
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            let input_updates = input_updates
                .into_iter()
                .map(InputStatePersistenceRecord::into_stored_and_expected)
                .collect::<Vec<_>>();
            tokio::task::spawn_blocking(move || {
                let lifecycle_expected = machine_lifecycle
                    .as_ref()
                    .and_then(|commit| commit.expected_version().cloned());
                let runtime_state = machine_lifecycle
                    .as_ref()
                    .map(MachineLifecycleCommit::runtime_state);
                let lifecycle_snapshot =
                    machine_lifecycle.map(MachineLifecycleCommit::into_snapshot);
                let mut conn = open_runtime_connection(&path)?;
                let tx = begin_runtime_transaction(&mut conn)?;
                refuse_whole_blob_write_under_head_authority(
                    &tx,
                    &runtime_id,
                    "whole-BLOB provisional promotion",
                )?;
                if let Some(expected) = &lifecycle_expected {
                    enforce_machine_lifecycle_expected_version(&tx, &runtime_id, expected)?;
                }
                let committed = promote_whole_blob_provisional_in_txn(
                    &tx,
                    &runtime_id,
                    &authority,
                    &receipt,
                    &checkpoint_conversation_digest,
                    checkpoint_message_count,
                    runtime_state,
                )?;
                if let Some(snapshot) = lifecycle_snapshot.as_ref() {
                    upsert_machine_lifecycle_snapshot(&tx, &runtime_id, snapshot)?;
                }
                insert_receipt(&tx, &runtime_id, &receipt)?;
                upsert_input_states(&tx, &runtime_id, &input_updates)?;
                tx.commit()
                    .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
                Ok(committed)
            })
            .await
            .map_err(|error| RuntimeStoreError::Internal(format!("Task join failed: {error}")))?
        }

        // This implements the sealed RuntimeStore recovery verb; the arity is
        // the trait's explicit set of fenced boundary components.
        #[allow(clippy::too_many_arguments)]
        async fn commit_whole_blob_recovery(
            &self,
            runtime_id: &LogicalRuntimeId,
            promotion: crate::store::PreparedWholeBlobRecoveryPromotion,
            evidence: crate::store::PreparedRecoveryEvidence,
            receipt: RunBoundaryReceipt,
            machine_lifecycle: MachineLifecycleCommit,
            input_updates: Vec<InputStatePersistenceRecord>,
            session_store_key: meerkat_core::types::SessionId,
        ) -> Result<PreparedRuntimeSessionCommitResult, RuntimeStoreError> {
            self.require_whole_blob_session_operation(runtime_id, "commit_whole_blob_recovery")?;
            let (expected, repaired_snapshot) = promotion.into_parts();
            if expected.session_id() != &session_store_key {
                return Err(RuntimeStoreError::SessionKeyMismatch {
                    expected: expected.session_id().clone(),
                    actual: session_store_key,
                });
            }
            evidence.verify_input_updates(&input_updates)?;
            evidence.verify_request_effects(&receipt, &machine_lifecycle)?;
            let recovery = CommittedRecoveryBoundary::from_prepared(&evidence, &receipt);
            let (
                _base_store_revision,
                _base_blob_sha256,
                candidate_blob_sha256,
                _candidate_sequence,
                recovered_blob_sha256,
            ) = evidence.whole_blob_authority_transition().ok_or_else(|| {
                session_authority_conflict(
                    runtime_id,
                    "WholeBlob recovery evidence has no WholeBlob authority transition",
                )
            })?;
            if candidate_blob_sha256 != expected.candidate_blob_sha256() {
                return Err(session_authority_conflict(
                    runtime_id,
                    "WholeBlob recovery candidate digest differs from its store authority",
                ));
            }
            let recovered_blob_sha256 = recovered_blob_sha256.to_string();
            let runtime_state = machine_lifecycle.runtime_state();
            let lifecycle_expected =
                machine_lifecycle
                    .expected_version()
                    .cloned()
                    .ok_or_else(|| {
                        session_authority_conflict(
                            runtime_id,
                            "WholeBlob recovery lifecycle has no exact predecessor fence",
                        )
                    })?;
            let lifecycle_snapshot = machine_lifecycle.into_snapshot();
            let lifecycle_target =
                MachineLifecycleStoreRecord::from_snapshot(&lifecycle_snapshot).encode()?;
            let repaired = repaired_snapshot
                .map(|prepared| {
                    let (session, serialized, blob_sha256) = prepared.into_parts();
                    if session.id() != expected.session_id() || blob_sha256 != recovered_blob_sha256
                    {
                        return Err(session_authority_conflict(
                            runtime_id,
                            "WholeBlob repaired artifact differs from recovery evidence",
                        ));
                    }
                    let mut catalog_entry = crate::store::RuntimeSessionCatalogEntry::from_session(
                        session.as_ref(),
                        RuntimeSessionPersistenceProfile::WholeBlobV1,
                        Some(runtime_state),
                    )?;
                    catalog_entry.set_runtime_state(Some(runtime_state));
                    let compaction_intents =
                        crate::store::validated_compaction_projection_intents(session.as_ref())?;
                    Ok((
                        serialized.session_snapshot,
                        catalog_entry,
                        compaction_intents,
                    ))
                })
                .transpose()?;
            let input_updates = input_updates
                .into_iter()
                .map(InputStatePersistenceRecord::into_stored_and_expected)
                .collect::<Vec<_>>();
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            tokio::task::spawn_blocking(move || {
                let mut conn = open_runtime_connection(&path)?;
                let tx = begin_runtime_transaction(&mut conn)?;
                refuse_whole_blob_write_under_head_authority(
                    &tx,
                    &runtime_id,
                    "whole-BLOB recovery",
                )?;
                if let Some(stored) =
                    load_recovery_boundary(&tx, &runtime_id, evidence.candidate_id())?
                {
                    if stored != recovery {
                        return Err(session_authority_conflict(
                            &runtime_id,
                            "a divergent recovery boundary already exists for this candidate",
                        ));
                    }
                    let expected_current = WholeBlobStoreAuthority::issued(
                        evidence.session_id().clone(),
                        expected
                            .base_store_revision()
                            .checked_add(1)
                            .ok_or_else(|| {
                                session_authority_conflict(
                                    &runtime_id,
                                    "committed WholeBlob recovery revision overflow",
                                )
                            })?,
                        recovered_blob_sha256.clone(),
                    )?;
                    if load_whole_blob_store_authority(&tx, &runtime_id)?
                        != Some(expected_current.clone())
                        || load_whole_blob_provisional_authority(&tx, &runtime_id)?.is_some()
                    {
                        return Err(session_authority_conflict(
                            &runtime_id,
                            "committed WholeBlob recovery authority was superseded",
                        ));
                    }
                    let body_exists = tx
                        .query_row(
                            "SELECT 1 FROM runtime_whole_blob_bodies WHERE blob_sha256 = ?1",
                            params![expected_current.blob_sha256()],
                            |_row| Ok(()),
                        )
                        .optional()
                        .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?
                        .is_some();
                    if !body_exists {
                        return Err(session_authority_conflict(
                            &runtime_id,
                            "committed WholeBlob recovery body is absent",
                        ));
                    }
                    let current_lifecycle = tx
                        .query_row(
                            r"
                            SELECT runtime_state_json
                            FROM runtime_states
                            WHERE runtime_id = ?1
                            ",
                            params![runtime_id_text(&runtime_id)],
                            |row| Ok(row.get::<_, JsonColumnBytes>(0)?.into_bytes()),
                        )
                        .optional()
                        .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
                    if current_lifecycle.as_deref() != Some(lifecycle_target.as_slice())
                        || load_boundary_receipt(&tx, &runtime_id, &receipt)?
                            != Some(receipt.clone())
                    {
                        return Err(session_authority_conflict(
                            &runtime_id,
                            "committed WholeBlob recovery effects were superseded",
                        ));
                    }
                    for (target, _) in &input_updates {
                        let target_bytes = serde_json::to_vec(target)
                            .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
                        let current = tx
                            .query_row(
                                r"
                                SELECT state_json
                                FROM runtime_input_states
                                WHERE runtime_id = ?1 AND input_id = ?2
                                ",
                                params![
                                    runtime_id_text(&runtime_id),
                                    target.state.input_id.0.to_string(),
                                ],
                                |row| Ok(row.get::<_, JsonColumnBytes>(0)?.into_bytes()),
                            )
                            .optional()
                            .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
                        if current.as_deref() != Some(target_bytes.as_slice()) {
                            return Err(session_authority_conflict(
                                &runtime_id,
                                format!(
                                    "committed recovery input {} was superseded",
                                    target.state.input_id
                                ),
                            ));
                        }
                    }
                    verify_recovery_receipt_digest_enrichments_in_txn(&tx, &runtime_id, &recovery)?;
                    tx.commit()
                        .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
                    return Ok(PreparedRuntimeSessionCommitResult::recovery(
                        RuntimeSessionAuthority::WholeBlob(expected_current),
                        RecoveryCommitStatus::AlreadyCommittedExact,
                    ));
                }

                enforce_recovery_input_set_authority(
                    &tx,
                    &runtime_id,
                    evidence.predecessor_nonterminal_input_set_revision(),
                    evidence.predecessor_nonterminal_input_set_token(),
                )?;
                enforce_machine_lifecycle_expected_version(&tx, &runtime_id, &lifecycle_expected)?;
                let committed = if let Some((repaired_bytes, catalog_entry, compaction_intents)) =
                    repaired.as_ref()
                {
                    install_repaired_whole_blob_recovery_in_txn(
                        &tx,
                        &runtime_id,
                        &expected,
                        repaired_bytes.as_ref(),
                        &recovered_blob_sha256,
                        catalog_entry,
                        compaction_intents,
                    )?
                } else {
                    let conversation_digest =
                        receipt.conversation_digest.as_deref().ok_or_else(|| {
                            session_authority_conflict(
                                &runtime_id,
                                "completed WholeBlob recovery receipt has no conversation digest",
                            )
                        })?;
                    let message_count = u64::try_from(receipt.message_count).map_err(|_| {
                        session_authority_conflict(
                            &runtime_id,
                            "completed WholeBlob recovery message count exceeds durable range",
                        )
                    })?;
                    promote_whole_blob_provisional_in_txn(
                        &tx,
                        &runtime_id,
                        &expected,
                        &receipt,
                        conversation_digest,
                        message_count,
                        Some(runtime_state),
                    )?
                };
                upsert_machine_lifecycle_snapshot(&tx, &runtime_id, &lifecycle_snapshot)?;
                apply_recovery_receipt_digest_enrichments_in_txn(&tx, &runtime_id, &recovery)?;
                insert_receipt(&tx, &runtime_id, &receipt)?;
                upsert_input_states(&tx, &runtime_id, &input_updates)?;
                insert_recovery_boundary_in_txn(&tx, &runtime_id, &recovery)?;
                tx.commit()
                    .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
                Ok(PreparedRuntimeSessionCommitResult::recovery(
                    RuntimeSessionAuthority::WholeBlob(committed),
                    RecoveryCommitStatus::Committed,
                ))
            })
            .await
            .map_err(|error| RuntimeStoreError::Internal(format!("Task join failed: {error}")))?
        }

        async fn commit_head_canonical_provisional_promotion(
            &self,
            runtime_id: &LogicalRuntimeId,
            promotion: crate::store::PreparedHeadCanonicalProvisionalPromotion,
            receipt: RunBoundaryReceipt,
            machine_lifecycle: Option<MachineLifecycleCommit>,
            input_updates: Vec<InputStatePersistenceRecord>,
            session_store_key: meerkat_core::types::SessionId,
        ) -> Result<(HeadCanonicalStoreAuthority, bool), RuntimeStoreError> {
            self.require_head_canonical_session_operation(
                runtime_id,
                "commit_head_canonical_provisional_promotion",
            )?;
            let (checkpoint, authority) = promotion.into_parts();
            if authority.session_id() != &session_store_key {
                return Err(RuntimeStoreError::SessionKeyMismatch {
                    expected: authority.session_id().clone(),
                    actual: session_store_key,
                });
            }
            if receipt.conversation_digest.as_deref() != Some(checkpoint.conversation_digest())
                || u64::try_from(receipt.message_count).ok() != Some(checkpoint.message_count())
            {
                return Err(session_authority_conflict(
                    runtime_id,
                    "HeadCanonical terminal receipt differs from its exact checkpoint digest/count",
                ));
            }
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            let input_updates = input_updates
                .into_iter()
                .map(InputStatePersistenceRecord::into_stored_and_expected)
                .collect::<Vec<_>>();
            tokio::task::spawn_blocking(move || {
                let lifecycle_expected = machine_lifecycle
                    .as_ref()
                    .and_then(|commit| commit.expected_version().cloned());
                let runtime_state = machine_lifecycle
                    .as_ref()
                    .map(MachineLifecycleCommit::runtime_state);
                let lifecycle_snapshot =
                    machine_lifecycle.map(MachineLifecycleCommit::into_snapshot);
                let mut conn = open_head_canonical_runtime_connection(&path)?;
                let tx = begin_runtime_transaction(&mut conn)?;
                let witness = prepare_head_canonical_promotion_witness(
                    &runtime_id,
                    &authority,
                    &receipt,
                    lifecycle_expected.as_ref(),
                    lifecycle_snapshot.as_ref(),
                    &input_updates,
                )?;
                if let Some(stored_digest) =
                    load_ordinary_boundary_witness(&tx, &runtime_id, &witness.boundary_key)?
                {
                    if stored_digest != witness.request_digest {
                        return Err(session_authority_conflict(
                            &runtime_id,
                            "HeadCanonical promotion boundary identity belongs to a divergent request",
                        ));
                    }
                    match load_boundary_receipt(&tx, &runtime_id, &receipt)? {
                        Some(stored) if stored == receipt => {}
                        _ => {
                            return Err(session_authority_conflict(
                                &runtime_id,
                                "HeadCanonical promotion witness has no exact durable receipt",
                            ));
                        }
                    }
                    let current = load_head_canonical_authority(&tx, &runtime_id)?
                        .and_then(|current| current.head_canonical().cloned())
                        .ok_or_else(|| {
                            session_authority_conflict(
                                &runtime_id,
                                "HeadCanonical promotion witness has no committed authority",
                            )
                        })?;
                    let final_revision = authority
                        .physical_store_revision()
                        .checked_add(1)
                        .ok_or_else(|| {
                            session_authority_conflict(
                                &runtime_id,
                                "HeadCanonical promotion retry revision overflow",
                            )
                        })?;
                    if current.session_id() != authority.session_id()
                        || current.store_revision() != final_revision
                        || current.committed_head_token() != authority.physical_head_token()
                        || load_head_canonical_provisional_tail_authority(&tx, &runtime_id)?
                            .is_some()
                    {
                        return Err(session_authority_conflict(
                            &runtime_id,
                            "HeadCanonical promotion witness has been superseded",
                        ));
                    }
                    let (physical_head, physical_token) =
                        meerkat_store::sqlite_store::load_head_canonical_for_runtime_in_txn(
                            &tx,
                            authority.session_id(),
                        )
                        .map_err(|error| {
                            map_head_canonical_session_store_error(&runtime_id, error)
                        })?
                        .ok_or_else(|| {
                            session_authority_conflict(
                                &runtime_id,
                                "HeadCanonical promotion retry has no physical head",
                            )
                        })?;
                    if physical_token != authority.physical_head_token()
                        || &physical_head != current.boundary_head()
                    {
                        return Err(session_authority_conflict(
                            &runtime_id,
                            "HeadCanonical promotion retry physical head was superseded",
                        ));
                    }
                    verify_current_ordinary_boundary_effects(
                        &tx,
                        &runtime_id,
                        None,
                        lifecycle_snapshot.as_ref(),
                        &input_updates,
                    )?;
                    let current_catalog =
                        load_runtime_session_catalog_entry_in_txn(&tx, &runtime_id)?;
                    let expected_runtime_state = runtime_state.or_else(|| {
                        current_catalog
                            .as_ref()
                            .and_then(|entry| entry.runtime_state())
                    });
                    let expected_catalog = crate::store::RuntimeSessionCatalogEntry::from_head(
                        &physical_head,
                        RuntimeSessionPersistenceProfile::HeadCanonicalV1,
                        expected_runtime_state,
                    )?;
                    if current_catalog != Some(expected_catalog) {
                        return Err(session_authority_conflict(
                            &runtime_id,
                            "HeadCanonical promotion retry catalog was superseded",
                        ));
                    }
                    tx.commit()
                        .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
                    return Ok((current, true));
                }
                if let Some(expected) = &lifecycle_expected {
                    enforce_machine_lifecycle_expected_version(&tx, &runtime_id, expected)?;
                }
                let committed = promote_head_canonical_provisional_in_txn(
                    &tx,
                    &runtime_id,
                    &authority,
                    &receipt,
                    runtime_state,
                )?;
                if let Some(snapshot) = lifecycle_snapshot.as_ref() {
                    upsert_machine_lifecycle_snapshot(&tx, &runtime_id, snapshot)?;
                }
                insert_receipt(&tx, &runtime_id, &receipt)?;
                upsert_input_states(&tx, &runtime_id, &input_updates)?;
                insert_ordinary_boundary_witness_in_txn(&tx, &runtime_id, &witness)?;
                tx.commit()
                    .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
                Ok((committed, false))
            })
            .await
            .map_err(|error| RuntimeStoreError::Internal(format!("Task join failed: {error}")))?
        }
    }

    #[async_trait::async_trait]
    impl RuntimeStore for SqliteRuntimeStore {
        fn session_persistence_profile(&self) -> RuntimeSessionPersistenceProfile {
            self.session_persistence_profile
        }

        fn session_boundary_authority_read_cost(&self) -> RuntimeSessionAuthorityReadCost {
            RuntimeSessionAuthorityReadCost::Bounded
        }

        fn input_state_batch_cas_implementation_profile(
            &self,
        ) -> InputStateBatchCasImplementationProfile {
            InputStateBatchCasImplementationProfile::MultiWriter
        }

        async fn commit_prepared_session_boundary(
            &self,
            runtime_id: &LogicalRuntimeId,
            request: PreparedRuntimeSessionCommit,
        ) -> Result<PreparedRuntimeSessionCommitResult, RuntimeStoreError> {
            if self.session_persistence_profile == RuntimeSessionPersistenceProfile::WholeBlobV1 {
                let authority = match request.into_payload() {
                    PreparedRuntimeSessionCommitPayload::SnapshotOnly { session } => {
                        self.commit_prepared_whole_blob_boundary(
                            runtime_id,
                            Some(crate::store::prepared_whole_blob_snapshot(&session)?),
                            None,
                            None,
                            Vec::new(),
                            None,
                        )
                        .await?
                    }
                    PreparedRuntimeSessionCommitPayload::Success {
                        session,
                        receipt,
                        input_updates,
                        session_store_key,
                    } => {
                        let prepared = session
                            .as_ref()
                            .map(crate::store::prepared_whole_blob_snapshot)
                            .transpose()?;
                        self.commit_prepared_whole_blob_boundary(
                            runtime_id,
                            prepared,
                            Some(receipt),
                            None,
                            input_updates,
                            session_store_key,
                        )
                        .await?
                    }
                    PreparedRuntimeSessionCommitPayload::PromoteWholeBlobSuccess {
                        promotion,
                        receipt,
                        input_updates,
                        session_store_key,
                    } => Some(
                        self.commit_whole_blob_provisional_promotion(
                            runtime_id,
                            promotion,
                            receipt,
                            None,
                            input_updates,
                            session_store_key,
                        )
                        .await?,
                    ),
                    PreparedRuntimeSessionCommitPayload::ServiceTurnTerminal {
                        session,
                        receipt,
                        machine_lifecycle,
                        session_store_key,
                    } => self
                        .commit_prepared_whole_blob_boundary(
                            runtime_id,
                            Some(crate::store::prepared_whole_blob_snapshot(&session)?),
                            Some(receipt),
                            Some(machine_lifecycle),
                            Vec::new(),
                            Some(session_store_key),
                        )
                        .await?,
                    PreparedRuntimeSessionCommitPayload::PromoteWholeBlobServiceTurnTerminal {
                        promotion,
                        receipt,
                        machine_lifecycle,
                        session_store_key,
                    } => Some(
                        self.commit_whole_blob_provisional_promotion(
                            runtime_id,
                            promotion,
                            receipt,
                            Some(machine_lifecycle),
                            Vec::new(),
                            session_store_key,
                        )
                        .await?,
                    ),
                    PreparedRuntimeSessionCommitPayload::MachineTerminal {
                        session,
                        receipt,
                        machine_lifecycle,
                        input_updates,
                        session_store_key,
                    } => self
                        .commit_prepared_whole_blob_boundary(
                            runtime_id,
                            Some(crate::store::prepared_whole_blob_snapshot(&session)?),
                            Some(receipt),
                            Some(machine_lifecycle),
                            input_updates,
                            Some(session_store_key),
                        )
                        .await?,
                    PreparedRuntimeSessionCommitPayload::PromoteWholeBlobMachineTerminal {
                        promotion,
                        receipt,
                        machine_lifecycle,
                        input_updates,
                        session_store_key,
                    } => Some(
                        self.commit_whole_blob_provisional_promotion(
                            runtime_id,
                            promotion,
                            receipt,
                            Some(machine_lifecycle),
                            input_updates,
                            session_store_key,
                        )
                        .await?,
                    ),
                    PreparedRuntimeSessionCommitPayload::PromoteWholeBlobRecovery {
                        promotion,
                        evidence,
                        receipt,
                        machine_lifecycle,
                        input_updates,
                        session_store_key,
                    } => {
                        return self
                            .commit_whole_blob_recovery(
                                runtime_id,
                                promotion,
                                evidence,
                                receipt,
                                machine_lifecycle,
                                input_updates,
                                session_store_key,
                            )
                            .await;
                    }
                    PreparedRuntimeSessionCommitPayload::PromoteHeadCanonicalSuccess { .. }
                    | PreparedRuntimeSessionCommitPayload::PromoteHeadCanonicalServiceTurnTerminal {
                        ..
                    }
                    | PreparedRuntimeSessionCommitPayload::PromoteHeadCanonicalMachineTerminal {
                        ..
                    } => {
                        return Err(session_authority_conflict(
                            runtime_id,
                            "HeadCanonical promotion cannot commit through a WholeBlob store",
                        ));
                    }
                    PreparedRuntimeSessionCommitPayload::Recovery { .. } => {
                        return Err(
                            RuntimeStoreError::PreparedRecoveryRequiresAtomicPhysicalHeadCas {
                                profile: RuntimeSessionPersistenceProfile::WholeBlobV1,
                            },
                        );
                    }
                };
                return Ok(match authority {
                    Some(authority) => PreparedRuntimeSessionCommitResult::committed(
                        RuntimeSessionAuthority::WholeBlob(authority),
                    ),
                    None => PreparedRuntimeSessionCommitResult::receipt_only(
                        RuntimeSessionPersistenceProfile::WholeBlobV1,
                    ),
                });
            }

            let payload = match request.into_payload() {
                PreparedRuntimeSessionCommitPayload::PromoteHeadCanonicalSuccess {
                    promotion,
                    receipt,
                    input_updates,
                    session_store_key,
                } => {
                    let (authority, already_applied) = self
                        .commit_head_canonical_provisional_promotion(
                            runtime_id,
                            promotion,
                            receipt,
                            None,
                            input_updates,
                            session_store_key,
                        )
                        .await?;
                    let result = PreparedRuntimeSessionCommitResult::committed(
                        RuntimeSessionAuthority::HeadCanonical(authority),
                    );
                    return Ok(if already_applied {
                        result.already_applied_exact()
                    } else {
                        result
                    });
                }
                PreparedRuntimeSessionCommitPayload::PromoteHeadCanonicalServiceTurnTerminal {
                    promotion,
                    receipt,
                    machine_lifecycle,
                    session_store_key,
                } => {
                    let (authority, already_applied) = self
                        .commit_head_canonical_provisional_promotion(
                            runtime_id,
                            promotion,
                            receipt,
                            Some(machine_lifecycle),
                            Vec::new(),
                            session_store_key,
                        )
                        .await?;
                    let result = PreparedRuntimeSessionCommitResult::committed(
                        RuntimeSessionAuthority::HeadCanonical(authority),
                    );
                    return Ok(if already_applied {
                        result.already_applied_exact()
                    } else {
                        result
                    });
                }
                PreparedRuntimeSessionCommitPayload::PromoteHeadCanonicalMachineTerminal {
                    promotion,
                    receipt,
                    machine_lifecycle,
                    input_updates,
                    session_store_key,
                } => {
                    let (authority, already_applied) = self
                        .commit_head_canonical_provisional_promotion(
                            runtime_id,
                            promotion,
                            receipt,
                            Some(machine_lifecycle),
                            input_updates,
                            session_store_key,
                        )
                        .await?;
                    let result = PreparedRuntimeSessionCommitResult::committed(
                        RuntimeSessionAuthority::HeadCanonical(authority),
                    );
                    return Ok(if already_applied {
                        result.already_applied_exact()
                    } else {
                        result
                    });
                }
                PreparedRuntimeSessionCommitPayload::PromoteWholeBlobSuccess { .. }
                | PreparedRuntimeSessionCommitPayload::PromoteWholeBlobServiceTurnTerminal {
                    ..
                }
                | PreparedRuntimeSessionCommitPayload::PromoteWholeBlobMachineTerminal { .. }
                | PreparedRuntimeSessionCommitPayload::PromoteWholeBlobRecovery { .. } => {
                    return Err(session_authority_conflict(
                        runtime_id,
                        "WholeBlob promotion cannot commit through a HeadCanonical store",
                    ));
                }
                payload => payload,
            };

            let (
                prepared_session,
                receipt,
                input_updates,
                lifecycle_expected,
                lifecycle_snapshot,
                recovery_boundary,
            ) = match payload {
                PreparedRuntimeSessionCommitPayload::SnapshotOnly { session } => (
                    Some(prepare_head_canonical_sqlite_session(
                        runtime_id, &session, None,
                    )?),
                    None,
                    Vec::new(),
                    None,
                    None,
                    None,
                ),
                PreparedRuntimeSessionCommitPayload::Success {
                    session,
                    receipt,
                    input_updates,
                    session_store_key,
                } => (
                    session
                        .as_ref()
                        .map(|session| {
                            prepare_head_canonical_sqlite_session(
                                runtime_id,
                                session,
                                session_store_key.as_ref(),
                            )
                        })
                        .transpose()?,
                    Some(receipt),
                    input_updates
                        .into_iter()
                        .map(InputStatePersistenceRecord::into_stored_and_expected)
                        .collect(),
                    None,
                    None,
                    None,
                ),
                PreparedRuntimeSessionCommitPayload::ServiceTurnTerminal {
                    session,
                    receipt,
                    machine_lifecycle,
                    session_store_key,
                } => {
                    let expected = machine_lifecycle.expected_version().cloned();
                    let snapshot = machine_lifecycle.into_snapshot();
                    (
                        Some(prepare_head_canonical_sqlite_session(
                            runtime_id,
                            &session,
                            Some(&session_store_key),
                        )?),
                        Some(receipt),
                        Vec::new(),
                        expected,
                        Some(snapshot),
                        None,
                    )
                }
                PreparedRuntimeSessionCommitPayload::MachineTerminal {
                    session,
                    receipt,
                    machine_lifecycle,
                    input_updates,
                    session_store_key,
                } => {
                    let expected = machine_lifecycle.expected_version().cloned();
                    let snapshot = machine_lifecycle.into_snapshot();
                    (
                        Some(prepare_head_canonical_sqlite_session(
                            runtime_id,
                            &session,
                            Some(&session_store_key),
                        )?),
                        Some(receipt),
                        input_updates
                            .into_iter()
                            .map(InputStatePersistenceRecord::into_stored_and_expected)
                            .collect(),
                        expected,
                        Some(snapshot),
                        None,
                    )
                }
                PreparedRuntimeSessionCommitPayload::Recovery {
                    session,
                    evidence,
                    receipt,
                    machine_lifecycle,
                    input_updates,
                    session_store_key,
                } => {
                    let prepared = prepare_head_canonical_sqlite_session(
                        runtime_id,
                        &session,
                        Some(&session_store_key),
                    )?;
                    validate_prepared_recovery_request_binding(
                        &session,
                        &evidence,
                        &input_updates,
                        &receipt,
                        &machine_lifecycle,
                    )?;
                    let recovery = CommittedRecoveryBoundary::from_prepared(&evidence, &receipt);
                    validate_prepared_recovery_sqlite_session(
                        runtime_id, &session, &prepared, &recovery,
                    )?;
                    let expected = machine_lifecycle.expected_version().cloned();
                    let snapshot = machine_lifecycle.into_snapshot();
                    (
                        Some(prepared),
                        Some(receipt),
                        input_updates
                            .into_iter()
                            .map(InputStatePersistenceRecord::into_stored_and_expected)
                            .collect(),
                        expected,
                        Some(snapshot),
                        Some(recovery),
                    )
                }
                PreparedRuntimeSessionCommitPayload::PromoteWholeBlobSuccess { .. }
                | PreparedRuntimeSessionCommitPayload::PromoteHeadCanonicalSuccess { .. }
                | PreparedRuntimeSessionCommitPayload::PromoteWholeBlobServiceTurnTerminal {
                    ..
                }
                | PreparedRuntimeSessionCommitPayload::PromoteHeadCanonicalServiceTurnTerminal {
                    ..
                }
                | PreparedRuntimeSessionCommitPayload::PromoteWholeBlobMachineTerminal { .. }
                | PreparedRuntimeSessionCommitPayload::PromoteHeadCanonicalMachineTerminal {
                    ..
                }
                | PreparedRuntimeSessionCommitPayload::PromoteWholeBlobRecovery { .. } => {
                    unreachable!(
                        "profile-specific promotion payloads return from the dispatch match above"
                    )
                }
            };

            let ordinary_witness = if recovery_boundary.is_none() {
                Some(prepare_ordinary_boundary_witness(
                    runtime_id,
                    prepared_session.as_ref(),
                    receipt.as_ref(),
                    lifecycle_expected.as_ref(),
                    lifecycle_snapshot.as_ref(),
                    &input_updates,
                )?)
            } else {
                None
            };
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            tokio::task::spawn_blocking(move || {
                let mut conn = open_head_canonical_runtime_connection(&path)?;
                let tx = begin_runtime_transaction(&mut conn)?;
                let mut newly_installed_head_authority = None;
                let successor_authority = prepared_session
                    .as_ref()
                    .map(|prepared| {
                        issue_head_canonical_authority_in_txn(
                            &tx,
                            &runtime_id,
                            prepared.successor_head.clone(),
                        )
                    })
                    .transpose()?;
                let result = match successor_authority.as_ref() {
                    Some(authority) => {
                        PreparedRuntimeSessionCommitResult::committed(authority.clone())
                    }
                    None => PreparedRuntimeSessionCommitResult::receipt_only(
                        RuntimeSessionPersistenceProfile::HeadCanonicalV1,
                    ),
                };

                if let Some(recovery) = recovery_boundary.as_ref() {
                    let prepared = prepared_session.as_ref().ok_or_else(|| {
                        session_authority_conflict(
                            &runtime_id,
                            "prepared recovery has no head-canonical session mutation",
                        )
                    })?;
                    let successor_authority = successor_authority.as_ref().ok_or_else(|| {
                        session_authority_conflict(
                            &runtime_id,
                            "prepared recovery has no issued HeadCanonical successor authority",
                        )
                    })?;
                    let receipt = receipt.as_ref().ok_or_else(|| {
                        session_authority_conflict(
                            &runtime_id,
                            "prepared recovery has no exact boundary receipt",
                        )
                    })?;
                    let lifecycle_expected =
                        lifecycle_expected.as_ref().ok_or_else(|| {
                            session_authority_conflict(
                                &runtime_id,
                                "prepared recovery lifecycle is not fenced on an exact observed row",
                            )
                        })?;
                    let lifecycle_snapshot =
                        lifecycle_snapshot.as_ref().ok_or_else(|| {
                            session_authority_conflict(
                                &runtime_id,
                                "prepared recovery has no machine lifecycle target",
                            )
                        })?;
                    let evidence = recovery.evidence();

                    if let Some(stored) = load_recovery_boundary(
                        &tx,
                        &runtime_id,
                        evidence.candidate_id(),
                    )? {
                        if stored != *recovery {
                            return Err(session_authority_conflict(
                                &runtime_id,
                                "a divergent recovery boundary already exists for this candidate",
                            ));
                        }
                        let current_authority =
                            load_head_canonical_authority(&tx, &runtime_id)?
                                .ok_or_else(|| {
                                    session_authority_conflict(
                                        &runtime_id,
                                        "committed recovery witness has no current runtime authority",
                                    )
                                })?;
                        if &current_authority != successor_authority {
                            return Err(session_authority_conflict(
                                &runtime_id,
                                "committed recovery boundary has been superseded by newer runtime authority",
                            ));
                        }
                        let (physical_head, physical_token) =
                            meerkat_store::sqlite_store::load_head_canonical_for_runtime_in_txn(
                                &tx,
                                prepared.mutation.session_id(),
                            )
                            .map_err(|error| {
                                map_head_canonical_session_store_error(
                                    &runtime_id,
                                    error,
                                )
                            })?
                            .ok_or_else(|| {
                                session_authority_conflict(
                                    &runtime_id,
                                    "committed recovery witness has no current physical head",
                                )
                            })?;
                        if &physical_head
                            != prepared.mutation.successor_head()
                            || physical_token
                                != prepared
                                    .mutation
                                    .successor_head_token()
                        {
                            return Err(session_authority_conflict(
                                &runtime_id,
                                "committed recovery boundary has been superseded by a newer physical head",
                            ));
                        }
                        verify_prepared_suffix_rows_for_exact_retry(
                            &tx,
                            &runtime_id,
                            prepared,
                        )?;
                        let lifecycle_target =
                            MachineLifecycleStoreRecord::from_snapshot(
                                lifecycle_snapshot,
                            )
                            .encode()?;
                        let current_lifecycle = tx
                            .query_row(
                                r"
                                SELECT runtime_state_json
                                FROM runtime_states
                                WHERE runtime_id = ?1
                                ",
                                params![runtime_id_text(&runtime_id)],
                                |row| {
                                    Ok(row
                                        .get::<_, JsonColumnBytes>(0)?
                                        .into_bytes())
                                },
                            )
                            .optional()
                            .map_err(|error| {
                                RuntimeStoreError::ReadFailed(
                                    error.to_string(),
                                )
                            })?;
                        if current_lifecycle.as_deref()
                            != Some(lifecycle_target.as_slice())
                        {
                            return Err(session_authority_conflict(
                                &runtime_id,
                                "committed recovery lifecycle target has been superseded",
                            ));
                        }
                        for (input, _) in &input_updates {
                            let target =
                                serde_json::to_vec(input).map_err(|error| {
                                    RuntimeStoreError::WriteFailed(
                                        error.to_string(),
                                    )
                                })?;
                            let current = tx
                                .query_row(
                                    r"
                                    SELECT state_json
                                    FROM runtime_input_states
                                    WHERE runtime_id = ?1 AND input_id = ?2
                                    ",
                                    params![
                                        runtime_id_text(&runtime_id),
                                        input.state.input_id.0.to_string(),
                                    ],
                                    |row| {
                                        Ok(row
                                            .get::<_, JsonColumnBytes>(0)?
                                            .into_bytes())
                                    },
                                )
                                .optional()
                                .map_err(|error| {
                                    RuntimeStoreError::ReadFailed(
                                        error.to_string(),
                                    )
                                })?;
                            if current.as_deref()
                                != Some(target.as_slice())
                            {
                                return Err(session_authority_conflict(
                                    &runtime_id,
                                    format!(
                                        "committed recovery input {} has been superseded",
                                        input.state.input_id
                                    ),
                                ));
                            }
                        }
                        verify_recovery_receipt_digest_enrichments_in_txn(
                            &tx,
                            &runtime_id,
                            recovery,
                        )?;
                        tx.commit().map_err(|error| {
                            RuntimeStoreError::WriteFailed(error.to_string())
                        })?;
                        return Ok(PreparedRuntimeSessionCommitResult::recovery(
                            successor_authority.clone(),
                            RecoveryCommitStatus::AlreadyCommittedExact,
                        ));
                    }

                    let current_authority =
                        load_head_canonical_authority(&tx, &runtime_id)?
                            .ok_or_else(|| {
                                session_authority_conflict(
                                    &runtime_id,
                                    "prepared recovery requires existing head-canonical runtime authority",
                                )
                            })?;
                    let (
                        committed_store_revision,
                        committed_head_token,
                        physical_store_revision,
                        physical_head_token,
                        recovered_head_token,
                    ) = evidence.head_canonical_authority_transition().ok_or_else(|| {
                        session_authority_conflict(
                            &runtime_id,
                            "HeadCanonical recovery evidence carries no HeadCanonical authority transition",
                        )
                    })?;
                    let committed_authority =
                        current_authority.head_canonical().ok_or_else(|| {
                            session_authority_conflict(
                                &runtime_id,
                                "current recovery authority is not HeadCanonical",
                            )
                        })?;
                    if committed_authority.session_id() != evidence.session_id()
                        || committed_authority.store_revision() != committed_store_revision
                        || committed_authority.committed_head_token() != committed_head_token
                    {
                        return Err(session_authority_conflict(
                            &runtime_id,
                            "current runtime authority does not match the sealed recovery predecessor",
                        ));
                    }
                    // Fence the small, indexed classification surface before
                    // re-materializing either session document. BEGIN
                    // IMMEDIATE already excludes a concurrent writer, so one
                    // exact check here remains valid through commit.
                    enforce_recovery_input_set_authority(
                        &tx,
                        &runtime_id,
                        evidence.predecessor_nonterminal_input_set_revision(),
                        evidence.predecessor_nonterminal_input_set_token(),
                    )?;
                    verify_materialized_head_authority_in_txn(
                        &tx,
                        &runtime_id,
                        &current_authority,
                    )?;
                    let provisional =
                        load_head_canonical_provisional_tail_authority(&tx, &runtime_id)?
                            .ok_or_else(|| {
                                session_authority_conflict(
                                    &runtime_id,
                                    "prepared recovery has no provisional HeadCanonical authority",
                                )
                            })?;
                    if provisional.session_id() != committed_authority.session_id()
                        || provisional.base_store_revision() != committed_store_revision
                        || provisional.base_committed_head_token() != committed_head_token
                        || provisional.physical_store_revision() != physical_store_revision
                        || provisional.physical_head_token() != physical_head_token
                        || provisional.run_id() != evidence.candidate_run_id()
                    {
                        return Err(session_authority_conflict(
                            &runtime_id,
                            "provisional HeadCanonical authority differs from sealed recovery evidence",
                        ));
                    }

                    let (physical_head, physical_token) =
                        meerkat_store::sqlite_store::load_head_canonical_for_runtime_in_txn(
                            &tx,
                            evidence.session_id(),
                        )
                        .map_err(|error| {
                            map_head_canonical_session_store_error(
                                &runtime_id,
                                error,
                            )
                        })?
                        .ok_or_else(|| {
                            session_authority_conflict(
                                &runtime_id,
                                "prepared recovery physical head is absent",
                            )
                        })?;
                    if Some(&physical_head)
                        != prepared.mutation.predecessor_head()
                        || physical_token != physical_head_token
                    {
                        return Err(session_authority_conflict(
                            &runtime_id,
                            "current physical head differs from sealed recovery evidence",
                        ));
                    }
                    let runtime_head = committed_authority.boundary_head();
                    meerkat_store::sqlite_store::verify_head_rewrite_prefix_descent_in_txn(
                        &tx,
                        runtime_head,
                        &physical_head,
                    )
                    .map_err(|error| {
                        map_head_canonical_session_store_error(&runtime_id, error)
                    })?;
                    let successor = successor_authority.head_canonical().ok_or_else(|| {
                        session_authority_conflict(
                            &runtime_id,
                            "prepared recovery successor is not HeadCanonical",
                        )
                    })?;
                    if successor.store_revision()
                        != physical_store_revision.checked_add(1).ok_or_else(|| {
                            session_authority_conflict(
                                &runtime_id,
                                "physical HeadCanonical store revision overflow",
                            )
                        })?
                        || successor.committed_head_token() != recovered_head_token
                    {
                        return Err(session_authority_conflict(
                            &runtime_id,
                            "prepared recovery successor differs from sealed HeadCanonical authority evidence",
                        ));
                    }

                    enforce_machine_lifecycle_expected_version(
                        &tx,
                        &runtime_id,
                        lifecycle_expected,
                    )?;
                    reject_finalized_compaction_projection_replays(
                        &tx,
                        &runtime_id,
                        &prepared.compaction_intents,
                    )?;
                    apply_prepared_head_canonical_physical_mutation_in_txn(
                        &tx,
                        &runtime_id,
                        prepared,
                    )?;
                    write_head_canonical_authority_in_txn(
                        &tx,
                        &runtime_id,
                        successor_authority,
                    )?;
                    clear_runtime_projection_quarantine(&tx, &runtime_id)?;
                    insert_compaction_projection_outbox_intents(
                        &tx,
                        &runtime_id,
                        &prepared.compaction_intents,
                    )?;
                    let mut catalog_entry = prepared.catalog_entry.clone();
                    catalog_entry.set_runtime_state(Some(lifecycle_snapshot.runtime_state()));
                    upsert_runtime_session_catalog_entry_in_txn(
                        &tx,
                        &runtime_id,
                        &catalog_entry,
                    )?;
                    upsert_machine_lifecycle_snapshot(
                        &tx,
                        &runtime_id,
                        lifecycle_snapshot,
                    )?;
                    apply_recovery_receipt_digest_enrichments_in_txn(
                        &tx,
                        &runtime_id,
                        recovery,
                    )?;
                    insert_receipt(&tx, &runtime_id, receipt)?;
                    upsert_input_states(&tx, &runtime_id, &input_updates)?;
                    insert_recovery_boundary_in_txn(
                        &tx,
                        &runtime_id,
                        recovery,
                    )?;
                    tx.commit().map_err(|error| {
                        RuntimeStoreError::WriteFailed(error.to_string())
                    })?;
                    return Ok(PreparedRuntimeSessionCommitResult::recovery(
                        successor_authority.clone(),
                        RecoveryCommitStatus::Committed,
                    ));
                }

                let witness = ordinary_witness.as_ref().ok_or_else(|| {
                    session_authority_conflict(
                        &runtime_id,
                        "ordinary boundary has no exact request witness",
                    )
                })?;
                if let Some(stored_digest) = load_ordinary_boundary_witness(
                    &tx,
                    &runtime_id,
                    &witness.boundary_key,
                )? {
                    if stored_digest != witness.request_digest {
                        return Err(session_authority_conflict(
                            &runtime_id,
                            "ordinary boundary identity already belongs to a divergent request",
                        ));
                    }
                    if let Some(receipt) = receipt.as_ref() {
                        match load_boundary_receipt(&tx, &runtime_id, receipt)? {
                            Some(stored) if stored == *receipt => {}
                            Some(_) => {
                                return Err(session_authority_conflict(
                                    &runtime_id,
                                    "ordinary boundary witness and durable receipt differ",
                                ));
                            }
                            None => {
                                return Err(session_authority_conflict(
                                    &runtime_id,
                                    "ordinary boundary witness has no atomic durable receipt",
                                ));
                            }
                        }
                    }
                    if let Some(prepared) = prepared_session.as_ref() {
                        let prepared_successor =
                            successor_authority.as_ref().ok_or_else(|| {
                                session_authority_conflict(
                                    &runtime_id,
                                    "ordinary boundary has no issued HeadCanonical successor",
                                )
                            })?;
                        let current =
                            load_head_canonical_authority(&tx, &runtime_id)?
                                .ok_or_else(|| {
                                    session_authority_conflict(
                                        &runtime_id,
                                        "ordinary boundary witness has no current runtime session authority",
                                    )
                                })?;
                        if current.session_id() != prepared_successor.session_id() {
                            return Err(session_authority_conflict(
                                &runtime_id,
                                "ordinary boundary witness belongs to a different current session",
                            ));
                        }
                        if &current == prepared_successor {
                            // While this boundary remains current, recheck
                            // only its delta rows. After a later rewrite, the
                            // session store may legally prune the superseded
                            // strand; the atomic durable witness then remains
                            // the historical proof that this exact request
                            // committed.
                            verify_prepared_suffix_rows_for_exact_retry(
                                &tx,
                                &runtime_id,
                                prepared,
                            )?;
                        }
                    }
                    tx.commit().map_err(|error| {
                        RuntimeStoreError::WriteFailed(error.to_string())
                    })?;
                    return Ok(result.already_applied_exact());
                }

                // Supported-floor adoption: exact 0.8.10 could atomically
                // commit receipt/session/lifecycle/input effects before the
                // current request-witness table existed. Only a receipt copied
                // into the migration allowlist may enter this path. Current
                // stores missing a witness fail closed instead of being
                // mistaken for legacy state.
                if let Some(receipt) = receipt.as_ref() {
                    match load_boundary_receipt(&tx, &runtime_id, receipt)? {
                        Some(stored) if stored == *receipt => {
                            let marker = load_released_0810_boundary_receipt_marker(
                                &tx,
                                &runtime_id,
                                receipt,
                            )?
                            .ok_or_else(|| {
                                session_authority_conflict(
                                    &runtime_id,
                                    "boundary receipt has no request witness and no exact released 0.8.10 migration marker",
                                )
                            })?;
                            if let Some(prepared) = prepared_session.as_ref() {
                                let prepared_successor =
                                    successor_authority.as_ref().ok_or_else(|| {
                                        session_authority_conflict(
                                            &runtime_id,
                                            "released boundary has no issued HeadCanonical successor",
                                        )
                                    })?;
                                let current =
                                    load_head_canonical_authority(&tx, &runtime_id)?
                                        .ok_or_else(|| {
                                            session_authority_conflict(
                                                &runtime_id,
                                                "released boundary has no current runtime session authority",
                                            )
                                        })?;
                                if &current != prepared_successor {
                                    return Err(session_authority_conflict(
                                        &runtime_id,
                                        "released boundary cannot prove the prepared session successor is still current",
                                    ));
                                }
                                verify_prepared_suffix_rows_for_exact_retry(
                                    &tx,
                                    &runtime_id,
                                    prepared,
                                )?;
                            }
                            // Released receipts bind the committed boundary but
                            // do not retain prior lifecycle/input CAS tokens.
                            // Reject unprovable fences, then verify every
                            // surviving effect exactly; the migration marker
                            // authorizes selecting this request as the one
                            // current witnessed form.
                            verify_released_0810_boundary_effects_for_adoption(
                                &tx,
                                &runtime_id,
                                prepared_session.as_ref(),
                                lifecycle_expected.as_ref(),
                                lifecycle_snapshot.as_ref(),
                                &input_updates,
                            )?;
                            insert_ordinary_boundary_witness_in_txn(
                                &tx,
                                &runtime_id,
                                witness,
                            )?;
                            consume_released_0810_boundary_receipt_marker(
                                &tx,
                                &runtime_id,
                                receipt,
                                &marker,
                            )?;
                            tx.commit().map_err(|error| {
                                RuntimeStoreError::WriteFailed(error.to_string())
                            })?;
                            return Ok(result.already_applied_released_equivalent());
                        }
                        Some(_) => {
                            return Err(session_authority_conflict(
                                &runtime_id,
                                "boundary receipt conflicts with the prepared request",
                            ));
                        }
                        None => {}
                    }
                }

                if let Some(prepared) = prepared_session.as_ref() {
                    let prepared_successor =
                        successor_authority.as_ref().ok_or_else(|| {
                            session_authority_conflict(
                                &runtime_id,
                                "prepared ordinary boundary has no issued HeadCanonical successor",
                            )
                        })?;
                    let current_authority =
                        load_head_canonical_authority(&tx, &runtime_id)?;
                    if current_authority.as_ref() == Some(prepared_successor) {
                        verify_prepared_suffix_rows_for_exact_retry(
                            &tx,
                            &runtime_id,
                            prepared,
                        )?;
                        if let Some(receipt) = receipt.as_ref() {
                            match load_boundary_receipt(
                                &tx,
                                &runtime_id,
                                receipt,
                            )? {
                                Some(stored) if stored == *receipt => {
                                    return Err(session_authority_conflict(
                                        &runtime_id,
                                        "a boundary receipt exists without its exact request witness",
                                    ));
                                }
                                Some(_) => {
                                    return Err(session_authority_conflict(
                                        &runtime_id,
                                        "successor authority exists but its boundary receipt conflicts",
                                    ));
                                }
                                None => {
                                    return Err(session_authority_conflict(
                                        &runtime_id,
                                        "successor authority exists without its atomic boundary receipt",
                                    ));
                                }
                            }
                        }
                        insert_ordinary_boundary_witness_in_txn(
                            &tx,
                            &runtime_id,
                            witness,
                        )?;
                        tx.commit()
                            .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
                        return Ok(result.already_applied_exact());
                    }

                    match current_authority.as_ref() {
                        Some(current) => {
                            if prepared.mutation.predecessor_head().is_none() {
                                return Err(session_authority_conflict(
                                    &runtime_id,
                                    "existing head-canonical authority cannot be replaced by a root mutation",
                                ));
                            }
                            let current = current.head_canonical().ok_or_else(|| {
                                session_authority_conflict(
                                    &runtime_id,
                                    "stored authority is not HeadCanonical",
                                )
                            })?;
                            let runtime_head = current.boundary_head();
                            let physical_predecessor =
                                prepared.mutation.predecessor_head().ok_or_else(|| {
                                    session_authority_conflict(
                                        &runtime_id,
                                        "successor mutation has no physical predecessor head",
                                    )
                                })?;
                            if current.session_id() != prepared_successor.session_id() {
                                return Err(session_authority_conflict(
                                    &runtime_id,
                                    "stored runtime authority belongs to a different prepared session",
                                ));
                            }
                            meerkat_store::sqlite_store::verify_head_rewrite_prefix_descent_in_txn(
                                &tx,
                                runtime_head,
                                physical_predecessor,
                            )
                            .map_err(|error| {
                                map_head_canonical_session_store_error(&runtime_id, error)
                            })?;
                        }
                        None => {
                            let whole_blob_authority_exists =
                                load_whole_blob_store_authority(&tx, &runtime_id)?.is_some();
                            let activation_marker =
                                load_head_canonical_activation_marker(&tx, &runtime_id)?;
                            if whole_blob_authority_exists
                                || prepared
                                    .mutation
                                    .predecessor_head()
                                    .is_some()
                                || activation_marker.is_some()
                            {
                                return Err(
                                    RuntimeStoreError::HeadCanonicalActivationRequired {
                                        runtime_id: runtime_id_text(&runtime_id).to_owned(),
                                        state: if let Some(marker) = activation_marker {
                                            match marker.state {
                                                HeadCanonicalActivationState::InProgress => {
                                                    "in_progress".to_string()
                                                }
                                                HeadCanonicalActivationState::Complete => {
                                                    "complete marker is missing runtime authority"
                                                        .to_string()
                                                }
                                            }
                                        } else if whole_blob_authority_exists {
                                            "not_started: store-owned WholeBlob predecessor exists"
                                                .to_string()
                                        } else {
                                            "not_started: prepared boundary names a legacy predecessor"
                                                .to_string()
                                        },
                                    },
                                );
                            }
                            tracing::info!(
                                runtime_id = %runtime_id,
                                session_id = %prepared.mutation.session_id(),
                                "starting new head-canonical runtime authority installation"
                            );
                            newly_installed_head_authority =
                                Some(prepared.mutation.session_id().clone());
                        }
                    }

                    reject_finalized_compaction_projection_replays(
                        &tx,
                        &runtime_id,
                        &prepared.compaction_intents,
                    )?;
                    apply_prepared_head_canonical_physical_mutation_in_txn(
                        &tx,
                        &runtime_id,
                        prepared,
                    )?;
                    write_head_canonical_authority_in_txn(
                        &tx,
                        &runtime_id,
                        prepared_successor,
                    )?;
                    clear_runtime_projection_quarantine(&tx, &runtime_id)?;
                    insert_compaction_projection_outbox_intents(
                        &tx,
                        &runtime_id,
                        &prepared.compaction_intents,
                    )?;
                    let mut catalog_entry = prepared.catalog_entry.clone();
                    let runtime_state = match lifecycle_snapshot.as_ref() {
                        Some(snapshot) => Some(snapshot.runtime_state()),
                        None => load_runtime_session_catalog_entry_in_txn(&tx, &runtime_id)?
                            .and_then(|entry| entry.runtime_state()),
                    };
                    catalog_entry.set_runtime_state(runtime_state);
                    upsert_runtime_session_catalog_entry_in_txn(
                        &tx,
                        &runtime_id,
                        &catalog_entry,
                    )?;
                }

                if let Some(expected) = lifecycle_expected.as_ref() {
                    enforce_machine_lifecycle_expected_version(
                        &tx,
                        &runtime_id,
                        expected,
                    )?;
                }
                if let Some(snapshot) = lifecycle_snapshot.as_ref() {
                    upsert_machine_lifecycle_snapshot(
                        &tx,
                        &runtime_id,
                        snapshot,
                    )?;
                }
                if let Some(receipt) = receipt.as_ref() {
                    insert_receipt(&tx, &runtime_id, receipt)?;
                }
                upsert_input_states(&tx, &runtime_id, &input_updates)?;
                insert_ordinary_boundary_witness_in_txn(
                    &tx,
                    &runtime_id,
                    witness,
                )?;
                tx.commit().map_err(|error| {
                    RuntimeStoreError::WriteFailed(error.to_string())
                })?;
                if let Some(session_id) = newly_installed_head_authority {
                    tracing::info!(
                        runtime_id = %runtime_id,
                        %session_id,
                        "committed new head-canonical runtime authority installation"
                    );
                }
                Ok(result)
            })
            .await
            .map_err(|error| {
                RuntimeStoreError::Internal(format!("Task join failed: {error}"))
            })?
        }

        async fn load_session_boundary_authority(
            &self,
            runtime_id: &LogicalRuntimeId,
        ) -> Result<Option<RuntimeSessionAuthority>, RuntimeStoreError> {
            if self.session_persistence_profile == RuntimeSessionPersistenceProfile::WholeBlobV1 {
                return self
                    .load_whole_blob_store_authority(runtime_id)
                    .await
                    .map(|authority| authority.map(RuntimeSessionAuthority::WholeBlob));
            }

            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            tokio::task::spawn_blocking(move || {
                let conn = open_runtime_connection(&path)?;
                load_head_canonical_authority(&conn, &runtime_id)
            })
            .await
            .map_err(|error| RuntimeStoreError::Internal(format!("Task join failed: {error}")))?
        }

        async fn load_durable_tail_recovery_source(
            &self,
            runtime_id: &LogicalRuntimeId,
        ) -> Result<Option<PreparedDurableTailRecoverySource>, RuntimeStoreError> {
            if self.session_persistence_profile != RuntimeSessionPersistenceProfile::HeadCanonicalV1
            {
                return Err(
                    RuntimeStoreError::PreparedRecoveryRequiresAtomicPhysicalHeadCas {
                        profile: self.session_persistence_profile,
                    },
                );
            }
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            tokio::task::spawn_blocking(move || {
                let mut conn = open_head_canonical_runtime_connection(&path)?;
                let tx = conn
                    .transaction()
                    .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
                let Some(authority) =
                    load_head_canonical_authority(&tx, &runtime_id)?
                else {
                    if load_whole_blob_store_authority(&tx, &runtime_id)?.is_some() {
                        return Err(session_authority_conflict(
                            &runtime_id,
                            "durable-tail recovery requires activation of the store-owned WholeBlob predecessor",
                        ));
                    }
                    if let Some(session_id) = runtime_id.session_id()
                        && meerkat_store::sqlite_store::load_head_canonical_for_runtime_in_txn(
                            &tx,
                            &session_id,
                        )
                        .map_err(|error| {
                            map_head_canonical_session_store_error(
                                &runtime_id,
                                error,
                            )
                        })?
                        .is_some()
                    {
                        return Err(session_authority_conflict(
                            &runtime_id,
                            "physical canonical head exists without runtime session authority",
                        ));
                    }
                    tx.commit().map_err(|error| {
                        RuntimeStoreError::ReadFailed(error.to_string())
                    })?;
                    return Ok(None);
                };
                let committed_authority = authority.head_canonical().ok_or_else(|| {
                    session_authority_conflict(
                        &runtime_id,
                        "durable-tail recovery source requires HeadCanonical authority",
                    )
                })?;
                let boundary_head = committed_authority.boundary_head();
                let provisional =
                    load_head_canonical_provisional_tail_authority(&tx, &runtime_id)?;
                let committed =
                    meerkat_store::sqlite_store::verify_runtime_boundary_head_canonical_in_txn(
                        &tx,
                        boundary_head,
                    )
                    .map_err(|error| {
                        map_head_canonical_session_store_error(
                            &runtime_id,
                            error,
                        )
                    })?;
                let (physical_head, physical_token) =
                    meerkat_store::sqlite_store::load_head_canonical_for_runtime_in_txn(
                        &tx,
                        authority.session_id(),
                    )
                    .map_err(|error| {
                        map_head_canonical_session_store_error(
                            &runtime_id,
                            error,
                        )
                    })?
                    .ok_or_else(|| {
                        session_authority_conflict(
                            &runtime_id,
                            "runtime authority exists without a physical canonical head",
                        )
                    })?;
                if &physical_head == boundary_head {
                    if committed_authority.committed_head_token() != physical_token {
                        return Err(session_authority_conflict(
                            &runtime_id,
                            "aligned runtime and physical heads carry divergent committed authority",
                        ));
                    }
                    let source = PreparedDurableTailRecoverySource::new(
                        authority,
                        provisional,
                        committed.clone(),
                        committed,
                    )?;
                    tx.commit().map_err(|error| {
                        RuntimeStoreError::ReadFailed(error.to_string())
                    })?;
                    return Ok(Some(source));
                }
                let physical =
                    meerkat_store::sqlite_store::verify_physical_head_retains_boundary_prefix_for_runtime_in_txn(
                        &tx,
                        boundary_head,
                        &physical_head,
                    )
                    .map_err(|error| {
                        map_head_canonical_session_store_error(
                            &runtime_id,
                            error,
                        )
                    })?;
                let source =
                    PreparedDurableTailRecoverySource::new(
                        authority,
                        provisional,
                        committed,
                        physical,
                    )?;
                tx.commit().map_err(|error| {
                    RuntimeStoreError::ReadFailed(error.to_string())
                })?;
                Ok(Some(source))
            })
            .await
            .map_err(|error| {
                RuntimeStoreError::Internal(format!("Task join failed: {error}"))
            })?
        }

        async fn load_durable_tail_recovery_receipts(
            &self,
            runtime_id: &LogicalRuntimeId,
            run_id: &RunId,
        ) -> Result<Vec<PreparedRecoveryReceiptSource>, RuntimeStoreError> {
            let profile = self.session_persistence_profile;
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            let run_id = run_id.clone();
            tokio::task::spawn_blocking(move || {
                let mut conn = match profile {
                    RuntimeSessionPersistenceProfile::WholeBlobV1 => {
                        open_runtime_connection(&path)?
                    }
                    RuntimeSessionPersistenceProfile::HeadCanonicalV1 => {
                        open_head_canonical_runtime_connection(&path)?
                    }
                };
                let tx = conn
                    .transaction()
                    .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
                let mut statement = tx
                    .prepare(
                        r"
                        SELECT sequence, receipt_json
                        FROM runtime_boundary_receipts
                        WHERE runtime_id = ?1 AND run_id = ?2
                        ",
                    )
                    .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
                let rows = statement
                    .query_map(
                        params![runtime_id_text(&runtime_id), run_id.0.to_string(),],
                        |row| {
                            Ok((
                                row.get::<_, i64>(0)?,
                                row.get::<_, JsonColumnBytes>(1)?.into_bytes(),
                            ))
                        },
                    )
                    .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
                let mut receipts = Vec::new();
                for row in rows {
                    let (stored_sequence, bytes) =
                        row.map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
                    let source = PreparedRecoveryReceiptSource::from_serialized_row(&bytes)?;
                    if source.receipt().run_id != run_id
                        || encode_receipt_sequence(source.receipt().sequence) != stored_sequence
                    {
                        return Err(session_authority_conflict(
                            &runtime_id,
                            "recovery receipt row key differs from its exact encoded identity",
                        ));
                    }
                    receipts.push(source);
                }
                drop(statement);
                receipts.sort_by_key(|source| source.receipt().sequence);
                tx.commit()
                    .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
                Ok(receipts)
            })
            .await
            .map_err(|error| RuntimeStoreError::Internal(format!("Task join failed: {error}")))?
        }

        async fn load_committed_recovery_boundary(
            &self,
            runtime_id: &LogicalRuntimeId,
            candidate_id: &str,
        ) -> Result<Option<CommittedRecoveryBoundary>, RuntimeStoreError> {
            if candidate_id.is_empty() {
                return Err(session_authority_conflict(
                    runtime_id,
                    "recovery candidate id is empty",
                ));
            }
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            let candidate_id = candidate_id.to_string();
            tokio::task::spawn_blocking(move || {
                let conn = open_runtime_connection(&path)?;
                load_recovery_boundary(&conn, &runtime_id, &candidate_id)
            })
            .await
            .map_err(|error| RuntimeStoreError::Internal(format!("Task join failed: {error}")))?
        }

        fn supports_compaction_projection_outbox(&self) -> bool {
            true
        }

        async fn load_runtime_delivery_authority(
            &self,
            runtime_id: &LogicalRuntimeId,
        ) -> Result<Option<RuntimeDeliveryAuthorityRecord>, RuntimeStoreError> {
            let Some(conn) = open_runtime_delivery_read_connection(&self.path)? else {
                return Ok(None);
            };
            conn.query_row(
                r"
                SELECT revision, state_json
                  FROM runtime_delivery_authority
                 WHERE runtime_id = ?1
                ",
                params![runtime_id_text(runtime_id)],
                |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?)),
            )
            .optional()
            .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?
            .map(|(revision, state_json)| {
                Ok(RuntimeDeliveryAuthorityRecord::from_parts(
                    decode_u64(revision, "runtime delivery authority revision")?,
                    state_json,
                ))
            })
            .transpose()
        }

        async fn load_runtime_delivery_record(
            &self,
            runtime_id: &LogicalRuntimeId,
            delivery_id: &str,
        ) -> Result<Option<RuntimeDeliveryStoreRecord>, RuntimeStoreError> {
            let Some(conn) = open_runtime_delivery_read_connection(&self.path)? else {
                return Ok(None);
            };
            conn.query_row(
                r"
                SELECT sequence, submission_json
                  FROM runtime_delivery_inbox
                 WHERE runtime_id = ?1 AND delivery_id = ?2
                ",
                params![runtime_id_text(runtime_id), delivery_id],
                |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?)),
            )
            .optional()
            .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?
            .map(|(sequence, submission_json)| {
                Ok(RuntimeDeliveryStoreRecord::from_parts(
                    delivery_id,
                    decode_u64(sequence, "runtime delivery sequence")?,
                    submission_json,
                ))
            })
            .transpose()
        }

        async fn compare_and_swap_runtime_delivery_authority(
            &self,
            runtime_id: &LogicalRuntimeId,
            expected_revision: Option<u64>,
            replacement: RuntimeDeliveryAuthorityRecord,
            inserted_delivery: Option<RuntimeDeliveryStoreRecord>,
        ) -> Result<RuntimeDeliveryAuthorityCasOutcome, RuntimeStoreError> {
            let mut conn = open_runtime_delivery_write_connection(&self.path)?;
            let tx = begin_runtime_transaction(&mut conn)?;
            let current = tx
                .query_row(
                    r"
                    SELECT revision, state_json
                      FROM runtime_delivery_authority
                     WHERE runtime_id = ?1
                    ",
                    params![runtime_id_text(runtime_id)],
                    |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?)),
                )
                .optional()
                .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?
                .map(|(revision, state_json)| {
                    Ok(RuntimeDeliveryAuthorityRecord::from_parts(
                        decode_u64(revision, "runtime delivery authority revision")?,
                        state_json,
                    ))
                })
                .transpose()?;
            if current
                .as_ref()
                .map(RuntimeDeliveryAuthorityRecord::revision)
                != expected_revision
            {
                return Ok(RuntimeDeliveryAuthorityCasOutcome::Conflict(current));
            }
            let required_revision = expected_revision
                .map_or(Some(1), |revision| revision.checked_add(1))
                .ok_or_else(|| {
                    RuntimeStoreError::WriteFailed(
                        "runtime delivery authority revision exhausted u64".into(),
                    )
                })?;
            if replacement.revision() != required_revision {
                return Err(RuntimeStoreError::WriteFailed(format!(
                    "runtime delivery replacement revision {} is not required successor {required_revision}",
                    replacement.revision()
                )));
            }

            if let Some(record) = inserted_delivery.as_ref() {
                tx.execute(
                    r"
                    INSERT INTO runtime_delivery_inbox
                        (runtime_id, delivery_id, sequence, submission_json)
                    VALUES (?1, ?2, ?3, ?4)
                    ",
                    params![
                        runtime_id_text(runtime_id),
                        record.delivery_id(),
                        encode_u64(record.sequence()).as_slice(),
                        record.submission_json(),
                    ],
                )
                .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
            }

            tx.execute(
                r"
                INSERT INTO runtime_delivery_authority (runtime_id, revision, state_json)
                VALUES (?1, ?2, ?3)
                ON CONFLICT(runtime_id) DO UPDATE SET
                    revision = excluded.revision,
                    state_json = excluded.state_json
                ",
                params![
                    runtime_id_text(runtime_id),
                    encode_u64(replacement.revision()).as_slice(),
                    replacement.state_json(),
                ],
            )
            .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
            tx.commit()
                .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
            Ok(RuntimeDeliveryAuthorityCasOutcome::Applied(replacement))
        }

        async fn list_runtime_delivery_records(
            &self,
            runtime_id: &LogicalRuntimeId,
            after_sequence: u64,
            limit: usize,
        ) -> Result<Vec<RuntimeDeliveryStoreRecord>, RuntimeStoreError> {
            if limit == 0 {
                return Ok(Vec::new());
            }
            let Some(conn) = open_runtime_delivery_read_connection(&self.path)? else {
                return Ok(Vec::new());
            };
            let mut statement = conn
                .prepare(
                    r"
                    SELECT delivery_id, sequence, submission_json
                      FROM runtime_delivery_inbox
                     WHERE runtime_id = ?1 AND sequence > ?2
                     ORDER BY sequence ASC
                     LIMIT ?3
                    ",
                )
                .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
            let rows = statement
                .query_map(
                    params![
                        runtime_id_text(runtime_id),
                        encode_u64(after_sequence).as_slice(),
                        i64::try_from(limit).unwrap_or(i64::MAX),
                    ],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, Vec<u8>>(1)?,
                            row.get::<_, Vec<u8>>(2)?,
                        ))
                    },
                )
                .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
            let mut records = Vec::new();
            for row in rows {
                let (delivery_id, sequence, submission_json) =
                    row.map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
                records.push(RuntimeDeliveryStoreRecord::from_parts(
                    delivery_id,
                    decode_u64(sequence, "runtime delivery sequence")?,
                    submission_json,
                ));
            }
            Ok(records)
        }

        fn auth_authority_key(&self) -> Option<String> {
            let path = std::fs::canonicalize(&self.path).unwrap_or_else(|_| self.path.clone());
            Some(format!("sqlite:{}", path.display()))
        }

        fn persist_auth_oauth_flow_snapshot(
            &self,
            snapshot_json: &[u8],
        ) -> Result<(), RuntimeStoreError> {
            let mut conn = open_runtime_connection(&self.path)?;
            let tx = begin_runtime_transaction(&mut conn)?;
            tx.execute(
                r"
                INSERT INTO runtime_auth_oauth_flow_state (id, state_json)
                VALUES (?1, ?2)
                ON CONFLICT(id) DO UPDATE SET state_json = excluded.state_json
                ",
                params![AUTH_OAUTH_FLOW_STATE_ID, snapshot_json],
            )
            .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
            tx.commit()
                .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
            Ok(())
        }

        fn load_auth_oauth_flow_snapshot(&self) -> Result<Option<Vec<u8>>, RuntimeStoreError> {
            let conn = open_runtime_connection(&self.path)?;
            conn.query_row(
                r"
                SELECT state_json
                FROM runtime_auth_oauth_flow_state
                WHERE id = ?1
                ",
                params![AUTH_OAUTH_FLOW_STATE_ID],
                |row| Ok(row.get::<_, JsonColumnBytes>(0)?.into_bytes()),
            )
            .optional()
            .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))
        }

        fn update_auth_oauth_flow_snapshot(
            &self,
            update: &mut AuthOAuthFlowSnapshotUpdate<'_>,
        ) -> Result<(), RuntimeStoreError> {
            let mut conn = open_runtime_connection(&self.path)?;
            let tx = begin_runtime_transaction(&mut conn)?;
            let current = tx
                .query_row(
                    r"
                    SELECT state_json
                    FROM runtime_auth_oauth_flow_state
                    WHERE id = ?1
                    ",
                    params![AUTH_OAUTH_FLOW_STATE_ID],
                    |row| Ok(row.get::<_, JsonColumnBytes>(0)?.into_bytes()),
                )
                .optional()
                .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))?;
            let next = update(current.as_deref())?;
            tx.execute(
                r"
                INSERT INTO runtime_auth_oauth_flow_state (id, state_json)
                VALUES (?1, ?2)
                ON CONFLICT(id) DO UPDATE SET state_json = excluded.state_json
                ",
                params![AUTH_OAUTH_FLOW_STATE_ID, next],
            )
            .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
            tx.commit()
                .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
            Ok(())
        }

        async fn commit_session_snapshot(
            &self,
            runtime_id: &LogicalRuntimeId,
            session_delta: SerializedSessionSnapshot,
        ) -> Result<(), RuntimeStoreError> {
            self.require_whole_blob_session_operation(runtime_id, "commit_session_snapshot")?;
            self.commit_session_snapshot_checked(runtime_id, session_delta)
                .await
        }

        async fn commit_prepared_whole_blob_rewrite_boundary(
            &self,
            runtime_id: &LogicalRuntimeId,
            boundary: PreparedWholeBlobRewriteStoreParts,
        ) -> Result<WholeBlobStoreAuthority, RuntimeStoreError> {
            self.require_whole_blob_session_operation(
                runtime_id,
                "commit_prepared_whole_blob_rewrite_boundary",
            )?;
            let (
                expected,
                successor_session_id,
                successor_blob_sha256,
                successor_bytes,
                mut successor_catalog_entry,
                compaction_projection_intents,
            ) = boundary.into_tuple();
            if expected.session_id() != &successor_session_id
                || &LogicalRuntimeId::for_session(&successor_session_id) != runtime_id
            {
                return Err(session_authority_conflict(
                    runtime_id,
                    "prepared WholeBlob rewrite authorities do not bind this runtime/session",
                ));
            }
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            tokio::task::spawn_blocking(move || {
                let mut conn = open_runtime_connection(&path)?;
                let tx = begin_runtime_transaction(&mut conn)?;
                refuse_whole_blob_write_under_head_authority(
                    &tx,
                    &runtime_id,
                    "whole-BLOB transcript rewrite commit",
                )?;
                let current =
                    load_whole_blob_store_authority(&tx, &runtime_id)?.ok_or_else(|| {
                        session_authority_conflict(
                            &runtime_id,
                            "prepared WholeBlob predecessor authority is absent",
                        )
                    })?;
                ensure_compaction_intents_already_outboxed_list(
                    &tx,
                    &runtime_id,
                    &compaction_projection_intents,
                )?;
                let successor_revision =
                    expected.store_revision().checked_add(1).ok_or_else(|| {
                        RuntimeStoreError::WriteFailed(format!(
                            "WholeBlob store revision exhausted for runtime {runtime_id}"
                        ))
                    })?;
                let exact_idempotent_successor = current.session_id() == &successor_session_id
                    && current.blob_sha256() == successor_blob_sha256
                    && ((current == expected
                        && expected.blob_sha256() == successor_blob_sha256)
                        || current.store_revision() == successor_revision);
                if exact_idempotent_successor {
                    return Ok(current);
                }
                if current != expected {
                    return Err(session_authority_conflict(
                        &runtime_id,
                        "prepared WholeBlob predecessor revision/token does not match current authority",
                    ));
                }
                // The non-constructible prepared carrier already paired this
                // exact shared byte slice with `successor_blob_sha256` using
                // the one final-document hash. Re-hashing or decoding it here
                // would add O(document) work; this transaction revalidates only
                // the store-owned predecessor authority.
                let runtime_state =
                    load_runtime_session_catalog_entry_in_txn(&tx, &runtime_id)?
                        .ok_or_else(|| {
                        session_authority_conflict(
                            &runtime_id,
                            "prepared WholeBlob predecessor has no catalog projection",
                        )
                    })?
                    .runtime_state();
                successor_catalog_entry.set_runtime_state(runtime_state);
                let successor = upsert_runtime_snapshot_issued(
                    &tx,
                    &runtime_id,
                    successor_bytes.as_ref(),
                    &successor_session_id,
                    &successor_blob_sha256,
                    &successor_catalog_entry,
                )?;
                tx.commit()
                    .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                Ok(successor)
            })
            .await
            .map_err(|err| RuntimeStoreError::Internal(format!("Task join failed: {err}")))?
        }

        async fn atomic_apply(
            &self,
            runtime_id: &LogicalRuntimeId,
            session_delta: Option<SerializedSessionSnapshot>,
            receipt: RunBoundaryReceipt,
            input_updates: Vec<InputStatePersistenceRecord>,
            session_store_key: Option<meerkat_core::types::SessionId>,
        ) -> Result<(), RuntimeStoreError> {
            self.require_whole_blob_session_operation(runtime_id, "atomic_apply")?;
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            let input_updates = input_updates
                .into_iter()
                .map(InputStatePersistenceRecord::into_stored_and_expected)
                .collect::<Vec<_>>();
            tokio::task::spawn_blocking(move || {
                let prepared_session = session_delta.map(parsed_whole_blob_snapshot).transpose()?;
                let compaction_intents = prepared_session
                    .as_ref()
                    .map(|prepared| {
                        crate::store::validated_compaction_projection_intents(prepared.session())
                    })
                    .transpose()?
                    .unwrap_or_default();
                if let (Some(prepared), Some(session_store_key)) =
                    (prepared_session.as_ref(), session_store_key.as_ref())
                    && prepared.session().id() != session_store_key
                {
                    return Err(RuntimeStoreError::SessionKeyMismatch {
                        expected: session_store_key.clone(),
                        actual: prepared.session().id().clone(),
                    });
                }

                let mut conn = open_runtime_connection(&path)?;
                let tx = begin_runtime_transaction(&mut conn)?;
                refuse_whole_blob_write_under_head_authority(
                    &tx,
                    &runtime_id,
                    "whole-BLOB atomic run boundary",
                )?;
                reject_finalized_compaction_projection_replays(
                    &tx,
                    &runtime_id,
                    &compaction_intents,
                )?;

                if let Some(prepared) = prepared_session {
                    commit_prepared_whole_blob_snapshot_in_txn(&tx, &runtime_id, prepared)?;
                } else if load_whole_blob_provisional_authority(&tx, &runtime_id)?.is_some() {
                    return Err(session_authority_conflict(
                        &runtime_id,
                        "receipt-only boundary cannot bypass a store-owned WholeBlob candidate",
                    ));
                }

                insert_compaction_projection_outbox_intents(&tx, &runtime_id, &compaction_intents)?;
                insert_receipt(&tx, &runtime_id, &receipt)?;
                upsert_input_states(&tx, &runtime_id, &input_updates)?;
                tx.commit()
                    .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                Ok(())
            })
            .await
            .map_err(|err| RuntimeStoreError::Internal(format!("Task join failed: {err}")))?
        }

        async fn atomic_apply_with_machine_lifecycle(
            &self,
            runtime_id: &LogicalRuntimeId,
            session_delta: SerializedSessionSnapshot,
            receipt: RunBoundaryReceipt,
            machine_lifecycle: MachineLifecycleCommit,
            input_updates: Vec<InputStatePersistenceRecord>,
            session_store_key: meerkat_core::types::SessionId,
        ) -> Result<(), RuntimeStoreError> {
            self.require_whole_blob_session_operation(
                runtime_id,
                "atomic_apply_with_machine_lifecycle",
            )?;
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            let lifecycle_expected = machine_lifecycle.expected_version().cloned();
            let lifecycle_snapshot = machine_lifecycle.into_snapshot();
            let input_updates = input_updates
                .into_iter()
                .map(InputStatePersistenceRecord::into_stored_and_expected)
                .collect::<Vec<_>>();
            tokio::task::spawn_blocking(move || {
                let prepared = parsed_whole_blob_snapshot(session_delta)?;
                let session = prepared.session();
                let compaction_intents =
                    crate::store::validated_compaction_projection_intents(session)?;
                if session.id() != &session_store_key {
                    return Err(RuntimeStoreError::SessionKeyMismatch {
                        expected: session_store_key,
                        actual: session.id().clone(),
                    });
                }

                let mut conn = open_runtime_connection(&path)?;
                let tx = begin_runtime_transaction(&mut conn)?;
                refuse_whole_blob_write_under_head_authority(
                    &tx,
                    &runtime_id,
                    "whole-BLOB machine-terminal boundary",
                )?;
                reject_finalized_compaction_projection_replays(
                    &tx,
                    &runtime_id,
                    &compaction_intents,
                )?;
                commit_prepared_whole_blob_snapshot_in_txn(&tx, &runtime_id, prepared)?;
                if let Some(expected) = &lifecycle_expected {
                    enforce_machine_lifecycle_expected_version(&tx, &runtime_id, expected)?;
                }
                upsert_machine_lifecycle_snapshot(&tx, &runtime_id, &lifecycle_snapshot)?;
                insert_compaction_projection_outbox_intents(&tx, &runtime_id, &compaction_intents)?;
                insert_receipt(&tx, &runtime_id, &receipt)?;
                upsert_input_states(&tx, &runtime_id, &input_updates)?;
                tx.commit()
                    .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                Ok(())
            })
            .await
            .map_err(|err| RuntimeStoreError::Internal(format!("Task join failed: {err}")))?
        }

        async fn load_pending_compaction_projections(
            &self,
            runtime_id: &LogicalRuntimeId,
        ) -> Result<Vec<meerkat_core::CompactionProjectionIntent>, RuntimeStoreError> {
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            tokio::task::spawn_blocking(move || {
                let conn = open_runtime_connection(&path)?;
                let mut statement = conn
                    .prepare(
                        r"
                        SELECT intent_json
                        FROM runtime_compaction_projection_outbox
                        WHERE runtime_id = ?1 AND state = 'pending'
                        ORDER BY session_id, parent_revision, revision, commit_fingerprint
                        ",
                    )
                    .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
                let rows = statement
                    .query_map(params![runtime_id_text(&runtime_id)], |row| {
                        Ok(row.get::<_, JsonColumnBytes>(0)?.into_bytes())
                    })
                    .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
                rows.map(|row| {
                    let encoded =
                        row.map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
                    serde_json::from_slice(&encoded)
                        .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))
                })
                .collect()
            })
            .await
            .map_err(|error| RuntimeStoreError::Internal(format!("Task join failed: {error}")))?
        }

        async fn mark_compaction_projection_finalized(
            &self,
            runtime_id: &LogicalRuntimeId,
            projection: &meerkat_core::CompactionProjectionId,
        ) -> Result<(), RuntimeStoreError> {
            if self.session_persistence_profile == RuntimeSessionPersistenceProfile::HeadCanonicalV1
            {
                let path = self.path.clone();
                let runtime_id = runtime_id.clone();
                let projection = projection.clone();
                return tokio::task::spawn_blocking(move || {
                    let mut conn =
                        open_head_canonical_runtime_connection(&path)?;
                    let tx = begin_runtime_transaction(&mut conn)?;
                    let outbox = tx
                        .query_row(
                            r"
                            SELECT intent_json, state
                            FROM runtime_compaction_projection_outbox
                            WHERE runtime_id = ?1 AND session_id = ?2
                              AND parent_revision = ?3 AND revision = ?4
                              AND commit_fingerprint = ?5
                            ",
                            params![
                                runtime_id_text(&runtime_id),
                                projection.session_id().to_string(),
                                projection.parent_revision(),
                                projection.revision(),
                                projection.commit_fingerprint(),
                            ],
                            |row| {
                                Ok((
                                    row.get::<_, JsonColumnBytes>(0)?
                                        .into_bytes(),
                                    row.get::<_, String>(1)?,
                                ))
                            },
                        )
                        .optional()
                        .map_err(|error| {
                            RuntimeStoreError::ReadFailed(error.to_string())
                        })?
                        .ok_or_else(|| {
                            RuntimeStoreError::NotFound(format!(
                                "compaction outbox rewrite {}",
                                projection.revision()
                            ))
                        })?;
                    let stored_intent: meerkat_core::CompactionProjectionIntent =
                        serde_json::from_slice(&outbox.0).map_err(|error| {
                            RuntimeStoreError::ReadFailed(error.to_string())
                        })?;
                    if stored_intent.projection != projection {
                        return Err(session_authority_conflict(
                            &runtime_id,
                            "compaction outbox key and encoded projection identity differ",
                        ));
                    }
                    if outbox.1 == "finalized" {
                        tx.commit().map_err(|error| {
                            RuntimeStoreError::WriteFailed(
                                error.to_string(),
                            )
                        })?;
                        return Ok(());
                    }
                    if outbox.1 != "pending" {
                        return Err(session_authority_conflict(
                            &runtime_id,
                            format!(
                                "compaction outbox has unsupported state '{}'",
                                outbox.1
                            ),
                        ));
                    }

                    let authority =
                        load_head_canonical_authority(&tx, &runtime_id)?
                            .ok_or_else(|| {
                                session_authority_conflict(
                                    &runtime_id,
                                    "pending compaction projection has no head-canonical runtime authority",
                                )
                            })?;
                    if authority.session_id() != projection.session_id() {
                        return Err(session_authority_conflict(
                            &runtime_id,
                            "pending compaction projection belongs to a different runtime session",
                        ));
                    }
                    let committed_authority = authority.head_canonical().ok_or_else(|| {
                        session_authority_conflict(
                            &runtime_id,
                            "compaction projection authority is not HeadCanonical",
                        )
                    })?;
                    let boundary_head = committed_authority.boundary_head();
                    let (physical_head, physical_token) =
                        meerkat_store::sqlite_store::load_head_canonical_for_runtime_in_txn(
                            &tx,
                            authority.session_id(),
                        )
                        .map_err(|error| {
                            map_head_canonical_session_store_error(
                                &runtime_id,
                                error,
                            )
                        })?
                        .ok_or_else(|| {
                            session_authority_conflict(
                                &runtime_id,
                                "pending compaction projection has no physical canonical head",
                            )
                        })?;
                    if &physical_head != boundary_head
                        || committed_authority.committed_head_token() != physical_token
                    {
                        return Err(session_authority_conflict(
                            &runtime_id,
                            "compaction finalization requires exact runtime and physical head equality",
                        ));
                    }

                    tracing::info!(
                        runtime_id = %runtime_id,
                        session_id = %authority.session_id(),
                        rewrite = %projection.revision(),
                        "starting head-canonical compaction projection finalization"
                    );
                    let verified =
                        meerkat_store::sqlite_store::verify_runtime_boundary_head_canonical_in_txn(
                            &tx,
                            boundary_head,
                        )
                        .map_err(|error| {
                            map_head_canonical_session_store_error(
                                &runtime_id,
                                error,
                            )
                        })?;
                    let mut session = verified.session().as_ref().clone();
                    let current_intents =
                        crate::store::validated_compaction_projection_intents(
                            &session,
                        )?;
                    let current_intent = current_intents
                        .iter()
                        .find(|intent| intent.projection == projection)
                        .ok_or_else(|| {
                            session_authority_conflict(
                                &runtime_id,
                                "pending outbox projection is absent from the authoritative session checkpoint",
                            )
                        })?;
                    if current_intent != &stored_intent {
                        return Err(session_authority_conflict(
                            &runtime_id,
                            "pending outbox projection differs from the authoritative session intent",
                        ));
                    }
                    complete_compaction_projection_intent(
                        &mut session,
                        &projection,
                    )?;
                    if crate::store::validated_compaction_projection_intents(
                        &session,
                    )?
                    .iter()
                    .any(|intent| intent.projection == projection)
                    {
                        return Err(session_authority_conflict(
                            &runtime_id,
                            "compaction finalization did not clear the authoritative session intent",
                        ));
                    }
                    let mutation =
                        meerkat_core::session_store::PreparedHeadCanonicalMutation::prepare(
                            &session,
                            Some(physical_head.clone()),
                        )
                        .map_err(|error| {
                            map_head_canonical_session_store_error(
                                &runtime_id,
                                error,
                            )
                        })?;
                    if !mutation.serialized_suffix().is_empty()
                        || mutation.base_seq()
                            != physical_head.message_count
                        || mutation.successor_head().message_count
                            != physical_head.message_count
                        || mutation.successor_head().strand
                            != physical_head.strand
                        || mutation.successor_head().rewrite_count
                            != physical_head.rewrite_count
                        || mutation.successor_head().rewrite_prefix
                            != physical_head.rewrite_prefix
                    {
                        return Err(session_authority_conflict(
                            &runtime_id,
                            "compaction finalization prepared a non-metadata canonical mutation",
                        ));
                    }
                    validate_exact_head_prefix_authority(
                        &runtime_id,
                        mutation.successor_head(),
                        "compaction-finalized successor head",
                    )?;
                    let successor_authority = issue_head_canonical_authority_in_txn(
                        &tx,
                        &runtime_id,
                        mutation.successor_head().clone(),
                    )?;
                    meerkat_store::sqlite_store::apply_prepared_head_canonical_mutation_in_txn(
                        &tx,
                        &mutation,
                    )
                    .map(|_outcome| ())
                    .map_err(|error| {
                        map_head_canonical_session_store_error(
                            &runtime_id,
                            error,
                        )
                    })?;
                    write_head_canonical_authority_in_txn(
                        &tx,
                        &runtime_id,
                        &successor_authority,
                    )?;
                    let updated = tx
                        .execute(
                            r"
                            UPDATE runtime_compaction_projection_outbox
                            SET state = 'finalized'
                            WHERE runtime_id = ?1 AND session_id = ?2
                              AND parent_revision = ?3 AND revision = ?4
                              AND commit_fingerprint = ?5
                              AND state = 'pending'
                            ",
                            params![
                                runtime_id_text(&runtime_id),
                                projection.session_id().to_string(),
                                projection.parent_revision(),
                                projection.revision(),
                                projection.commit_fingerprint(),
                            ],
                        )
                        .map_err(|error| {
                            RuntimeStoreError::WriteFailed(error.to_string())
                        })?;
                    if updated != 1 {
                        return Err(session_authority_conflict(
                            &runtime_id,
                            "pending compaction outbox row changed during finalization",
                        ));
                    }
                    tx.commit().map_err(|error| {
                        RuntimeStoreError::WriteFailed(error.to_string())
                    })?;
                    tracing::info!(
                        runtime_id = %runtime_id,
                        session_id = %successor_authority.session_id(),
                        rewrite = %projection.revision(),
                        "committed head-canonical compaction projection finalization"
                    );
                    Ok(())
                })
                .await
                .map_err(|error| {
                    RuntimeStoreError::Internal(format!(
                        "Task join failed: {error}"
                    ))
                })?;
            }

            self.require_whole_blob_session_operation(
                runtime_id,
                "mark_compaction_projection_finalized",
            )?;
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            let projection = projection.clone();
            tokio::task::spawn_blocking(move || {
                let mut conn = open_runtime_connection(&path)?;
                let tx = begin_runtime_transaction(&mut conn)?;
                refuse_whole_blob_write_under_head_authority(
                    &tx,
                    &runtime_id,
                    "whole-BLOB compaction finalization",
                )?;
                let exists = tx
                    .query_row(
                        r"
                        SELECT 1 FROM runtime_compaction_projection_outbox
                        WHERE runtime_id = ?1 AND session_id = ?2
                          AND parent_revision = ?3 AND revision = ?4
                          AND commit_fingerprint = ?5
                        ",
                        params![
                            runtime_id_text(&runtime_id),
                            projection.session_id().to_string(),
                            projection.parent_revision(),
                            projection.revision(),
                            projection.commit_fingerprint(),
                        ],
                        |_row| Ok(()),
                    )
                    .optional()
                    .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?
                    .is_some();
                if !exists {
                    return Err(RuntimeStoreError::NotFound(format!(
                        "compaction outbox rewrite {}",
                        projection.revision()
                    )));
                }
                if let Some(snapshot) = tx
                    .query_row(
                        "SELECT session_snapshot FROM runtime_session_snapshots WHERE runtime_id = ?1",
                        params![runtime_id_text(&runtime_id)],
                        |row| Ok(row.get::<_, JsonColumnBytes>(0)?.into_bytes()),
                    )
                    .optional()
                    .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?
                {
                    let mut session = deserialize_persisted_session(&snapshot)?;
                    complete_compaction_projection_intent(&mut session, &projection)?;
                    let cleaned = serde_json::to_vec(&session)
                        .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
                    upsert_runtime_snapshot(&tx, &runtime_id, &cleaned)?;
                }
                tx
                    .execute(
                        r"
                        UPDATE runtime_compaction_projection_outbox
                        SET state = 'finalized'
                        WHERE runtime_id = ?1 AND session_id = ?2
                          AND parent_revision = ?3 AND revision = ?4
                          AND commit_fingerprint = ?5
                        ",
                        params![
                            runtime_id_text(&runtime_id),
                            projection.session_id().to_string(),
                            projection.parent_revision(),
                            projection.revision(),
                            projection.commit_fingerprint(),
                        ],
                    )
                    .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
                tx.commit()
                    .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
                Ok(())
            })
            .await
            .map_err(|error| RuntimeStoreError::Internal(format!("Task join failed: {error}")))?
        }

        async fn load_input_states(
            &self,
            runtime_id: &LogicalRuntimeId,
        ) -> Result<Vec<InputStateRow>, RuntimeStoreError> {
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            tokio::task::spawn_blocking(move || {
                let conn = open_runtime_connection(&path)?;
                let mut stmt = conn
                    .prepare(
                        r"
                        SELECT input_id, state_json
                        FROM runtime_input_states
                        WHERE runtime_id = ?1
                        ORDER BY input_id ASC
                        ",
                    )
                    .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))?;
                let rows = stmt
                    .query_map(params![runtime_id_text(&runtime_id)], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, JsonColumnBytes>(1)?.into_bytes(),
                        ))
                    })
                    .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))?;
                rows.map(|row| {
                    let (input_id, bytes) =
                        row.map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))?;
                    // Decode failure is a per-row fact: report it typed under
                    // the row's key instead of poisoning the whole load.
                    Ok(match deserialize_persisted_input_state(&bytes) {
                        Ok(state) => InputStateRow::Decoded(Box::new(state)),
                        Err(err) => InputStateRow::Corrupt {
                            input_id,
                            detail: err.to_string(),
                        },
                    })
                })
                .collect()
            })
            .await
            .map_err(|err| RuntimeStoreError::Internal(format!("Task join failed: {err}")))?
        }

        async fn load_input_states_with_versions(
            &self,
            runtime_id: &LogicalRuntimeId,
        ) -> Result<PreparedRecoveryInputSnapshot, RuntimeStoreError> {
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            tokio::task::spawn_blocking(move || {
                let mut conn = open_runtime_connection(&path)?;
                let tx = conn
                    .transaction()
                    .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
                let snapshot = load_recovery_nonterminal_input_snapshot(&tx, &runtime_id)?;
                tx.commit()
                    .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
                Ok(snapshot)
            })
            .await
            .map_err(|err| RuntimeStoreError::Internal(format!("Task join failed: {err}")))?
        }

        async fn load_boundary_receipt(
            &self,
            runtime_id: &LogicalRuntimeId,
            run_id: &RunId,
            sequence: u64,
        ) -> Result<Option<RunBoundaryReceipt>, RuntimeStoreError> {
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            let run_id = run_id.clone();
            tokio::task::spawn_blocking(move || {
                let conn = open_runtime_connection(&path)?;
                conn.query_row(
                    r"
                    SELECT receipt_json
                    FROM runtime_boundary_receipts
                    WHERE runtime_id = ?1 AND run_id = ?2 AND sequence = ?3
                    ",
                    params![
                        runtime_id_text(&runtime_id),
                        run_id.0.to_string(),
                        encode_receipt_sequence(sequence)
                    ],
                    |row| Ok(row.get::<_, JsonColumnBytes>(0)?.into_bytes()),
                )
                .optional()
                .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))?
                .map(|bytes| {
                    serde_json::from_slice(&bytes)
                        .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))
                })
                .transpose()
            })
            .await
            .map_err(|err| RuntimeStoreError::Internal(format!("Task join failed: {err}")))?
        }

        async fn load_committed_boundary_receipts(
            &self,
            runtime_id: &LogicalRuntimeId,
            run_id: &RunId,
        ) -> Result<Vec<RunBoundaryReceipt>, RuntimeStoreError> {
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            let run_id = run_id.clone();
            tokio::task::spawn_blocking(move || {
                let conn = open_runtime_connection(&path)?;
                let mut stmt = conn
                    .prepare(
                        r"
                        SELECT receipt_json
                        FROM runtime_boundary_receipts
                        WHERE runtime_id = ?1 AND run_id = ?2
                        ",
                    )
                    .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))?;
                let rows = stmt
                    .query_map(
                        params![runtime_id_text(&runtime_id), run_id.0.to_string()],
                        |row| Ok(row.get::<_, JsonColumnBytes>(0)?.into_bytes()),
                    )
                    .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))?;
                let mut receipts = rows
                    .map(|row| {
                        let bytes =
                            row.map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))?;
                        serde_json::from_slice::<RunBoundaryReceipt>(&bytes)
                            .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                // Sequences are stored as a bit-cast i64, so SQL-side ORDER BY
                // is not faithful across the full u64 range; sort decoded.
                receipts.sort_by_key(|receipt| receipt.sequence);
                Ok(receipts)
            })
            .await
            .map_err(|err| RuntimeStoreError::Internal(format!("Task join failed: {err}")))?
        }

        async fn load_session_snapshot(
            &self,
            runtime_id: &LogicalRuntimeId,
        ) -> Result<Option<Arc<Vec<u8>>>, RuntimeStoreError> {
            if self.session_persistence_profile == RuntimeSessionPersistenceProfile::WholeBlobV1 {
                return self
                    .load_committed_whole_blob_snapshot(runtime_id)
                    .await
                    .map(|snapshot| snapshot.map(|snapshot| snapshot.bytes_arc()));
            }
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            let snapshot = tokio::task::spawn_blocking(move || {
                let conn = open_runtime_connection(&path)?;
                if load_head_canonical_authority(&conn, &runtime_id)?.is_some() {
                    return Err(session_authority_conflict(
                        &runtime_id,
                        "whole-BLOB load refused because the row is a frozen migration predecessor",
                    ));
                }
                conn.query_row(
                    "SELECT session_snapshot FROM runtime_session_snapshots WHERE runtime_id = ?1",
                    params![runtime_id_text(&runtime_id)],
                    |row| Ok(row.get::<_, JsonColumnBytes>(0)?.into_bytes()),
                )
                .optional()
                .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))
            })
            .await
            .map_err(|err| RuntimeStoreError::Internal(format!("Task join failed: {err}")))??;
            Ok(snapshot.map(Arc::new))
        }

        async fn load_whole_blob_store_authority(
            &self,
            runtime_id: &LogicalRuntimeId,
        ) -> Result<Option<WholeBlobStoreAuthority>, RuntimeStoreError> {
            self.require_whole_blob_session_operation(
                runtime_id,
                "load_whole_blob_store_authority",
            )?;
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            tokio::task::spawn_blocking(move || {
                let conn = open_runtime_connection(&path)?;
                load_whole_blob_store_authority(&conn, &runtime_id)
            })
            .await
            .map_err(|error| RuntimeStoreError::Internal(format!("Task join failed: {error}")))?
        }

        async fn delete_runtime_session_catalog_entry(
            &self,
            runtime_id: &LogicalRuntimeId,
        ) -> Result<(), RuntimeStoreError> {
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            tokio::task::spawn_blocking(move || {
                let conn = open_runtime_connection(&path)?;
                conn.execute(
                    "DELETE FROM runtime_session_catalog WHERE runtime_id = ?1",
                    params![runtime_id_text(&runtime_id)],
                )
                .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
                Ok(())
            })
            .await
            .map_err(|error| RuntimeStoreError::Internal(format!("Task join failed: {error}")))?
        }

        async fn load_runtime_session_catalog_entry(
            &self,
            runtime_id: &LogicalRuntimeId,
        ) -> Result<Option<crate::store::RuntimeSessionCatalogEntry>, RuntimeStoreError> {
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            tokio::task::spawn_blocking(move || {
                let conn = open_runtime_connection(&path)?;
                load_runtime_session_catalog_entry_in_txn(&conn, &runtime_id)
            })
            .await
            .map_err(|error| RuntimeStoreError::Internal(format!("Task join failed: {error}")))?
        }

        async fn list_runtime_session_catalog_entries(
            &self,
            filter: meerkat_core::SessionFilter,
        ) -> Result<Vec<crate::store::RuntimeSessionCatalogEntry>, RuntimeStoreError> {
            let path = self.path.clone();
            tokio::task::spawn_blocking(move || {
                let conn = open_runtime_connection(&path)?;
                list_runtime_session_catalog_entries_in_conn(&conn, filter)
            })
            .await
            .map_err(|error| RuntimeStoreError::Internal(format!("Task join failed: {error}")))?
        }

        async fn load_committed_whole_blob_snapshot(
            &self,
            runtime_id: &LogicalRuntimeId,
        ) -> Result<Option<CommittedWholeBlobSnapshot>, RuntimeStoreError> {
            self.require_whole_blob_session_operation(
                runtime_id,
                "load_committed_whole_blob_snapshot",
            )?;
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            tokio::task::spawn_blocking(move || {
                let mut conn = open_runtime_connection(&path)?;
                let tx = begin_runtime_transaction(&mut conn)?;
                let authority = load_whole_blob_store_authority(&tx, &runtime_id)?;
                let observed = match authority {
                    None => None,
                    Some(authority) => {
                        let bytes = tx
                            .query_row(
                                "SELECT session_snapshot FROM runtime_whole_blob_bodies WHERE blob_sha256 = ?1",
                                params![authority.blob_sha256()],
                                |row| Ok(row.get::<_, JsonColumnBytes>(0)?.into_bytes()),
                            )
                            .optional()
                            .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?
                            .ok_or_else(|| {
                                session_authority_conflict(
                                    &runtime_id,
                                    "WholeBlob authority references a missing body",
                                )
                            })?;
                        Some(CommittedWholeBlobSnapshot::new(
                            Arc::new(bytes),
                            authority,
                        )?)
                    }
                };
                tx.commit()
                    .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
                Ok(observed)
            })
            .await
            .map_err(|error| RuntimeStoreError::Internal(format!("Task join failed: {error}")))?
        }

        async fn commit_prepared_whole_blob_snapshot_cas(
            &self,
            runtime_id: &LogicalRuntimeId,
            prepared: PreparedWholeBlobSnapshotCas,
        ) -> Result<WholeBlobSnapshotCasOutcome, RuntimeStoreError> {
            self.require_whole_blob_session_operation(
                runtime_id,
                "commit_prepared_whole_blob_snapshot_cas",
            )?;
            let (expected, candidate_session, candidate_bytes, candidate_blob_sha256) =
                prepared.into_parts();
            if &LogicalRuntimeId::for_session(candidate_session.id()) != runtime_id
                || candidate_session.id() != expected.session_id()
            {
                return Err(session_authority_conflict(
                    runtime_id,
                    "prepared WholeBlob snapshot CAS does not bind this runtime/session",
                ));
            }
            let compaction_projection_intents =
                crate::store::validated_compaction_projection_intents(candidate_session.as_ref())?;
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            tokio::task::spawn_blocking(move || {
                let mut conn = open_runtime_connection(&path)?;
                let tx = begin_runtime_transaction(&mut conn)?;
                refuse_whole_blob_write_under_head_authority(
                    &tx,
                    &runtime_id,
                    "typed WholeBlob snapshot CAS",
                )?;
                let Some(current) = load_whole_blob_store_authority(&tx, &runtime_id)? else {
                    return Ok(WholeBlobSnapshotCasOutcome::Conflict);
                };
                if current != expected {
                    return Ok(WholeBlobSnapshotCasOutcome::Conflict);
                }
                if current.blob_sha256() == candidate_blob_sha256 {
                    return Ok(WholeBlobSnapshotCasOutcome::Committed(current));
                }
                if load_whole_blob_provisional_authority(&tx, &runtime_id)?.is_some() {
                    return Err(session_authority_conflict(
                        &runtime_id,
                        "snapshot CAS cannot bypass a store-owned WholeBlob provisional candidate",
                    ));
                }
                ensure_compaction_intents_already_outboxed_list(
                    &tx,
                    &runtime_id,
                    &compaction_projection_intents,
                )?;
                let runtime_state = load_runtime_session_catalog_entry_in_txn(&tx, &runtime_id)?
                    .and_then(|entry| entry.runtime_state());
                let catalog_entry = crate::store::RuntimeSessionCatalogEntry::from_session(
                    candidate_session.as_ref(),
                    RuntimeSessionPersistenceProfile::WholeBlobV1,
                    runtime_state,
                )?;
                let authority = upsert_runtime_snapshot_issued(
                    &tx,
                    &runtime_id,
                    candidate_bytes.as_ref(),
                    candidate_session.id(),
                    &candidate_blob_sha256,
                    &catalog_entry,
                )?;
                tx.commit()
                    .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
                Ok(WholeBlobSnapshotCasOutcome::Committed(authority))
            })
            .await
            .map_err(|error| RuntimeStoreError::Internal(format!("Task join failed: {error}")))?
        }

        async fn write_prepared_whole_blob_provisional_tail(
            &self,
            runtime_id: &LogicalRuntimeId,
            prepared: PreparedWholeBlobProvisionalTail,
        ) -> Result<WholeBlobProvisionalTailAuthority, RuntimeStoreError> {
            self.require_whole_blob_session_operation(
                runtime_id,
                "write_prepared_whole_blob_provisional_tail",
            )?;
            let (
                authority,
                candidate_artifact,
                conversation_digest,
                message_count,
                catalog_entry,
                compaction_projection_intents,
            ) = prepared.into_parts();
            let candidate_bytes = candidate_artifact.bytes_arc();
            if &LogicalRuntimeId::for_session(authority.session_id()) != runtime_id
                || catalog_entry.session_id() != authority.session_id()
                || catalog_entry.persistence_profile()
                    != RuntimeSessionPersistenceProfile::WholeBlobV1
                || u64::try_from(catalog_entry.message_count()).ok() != Some(message_count)
                || candidate_artifact.row_sha256_token() != authority.candidate_blob_sha256()
            {
                return Err(session_authority_conflict(
                    runtime_id,
                    "WholeBlob provisional artifact/catalog does not bind this runtime/session authority",
                ));
            }
            let catalog_json = serde_json::to_vec(&catalog_entry)
                .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
            let compaction_intents_json = serde_json::to_vec(&compaction_projection_intents)
                .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            tokio::task::spawn_blocking(move || {
                let mut conn = open_runtime_connection(&path)?;
                let tx = begin_runtime_transaction(&mut conn)?;
                let current =
                    load_whole_blob_store_authority(&tx, &runtime_id)?.ok_or_else(|| {
                        session_authority_conflict(
                            &runtime_id,
                            "WholeBlob provisional candidate has no committed base",
                        )
                    })?;
                if current.session_id() != authority.session_id()
                    || current.store_revision() != authority.base_store_revision()
                    || current.blob_sha256() != authority.base_blob_sha256()
                {
                    return Err(session_authority_conflict(
                        &runtime_id,
                        "WholeBlob provisional candidate base is stale",
                    ));
                }
                let existing = load_whole_blob_provisional_metadata(&tx, &runtime_id)?;
                if let Some(existing) = existing.as_ref() {
                    if existing.authority == authority {
                        if existing.conversation_digest != conversation_digest
                            || existing.message_count != message_count
                        {
                            return Err(session_authority_conflict(
                                &runtime_id,
                                "WholeBlob provisional retry changes bounded candidate facts",
                            ));
                        }
                        return Ok(authority);
                    }
                    let required_sequence = existing
                        .authority
                        .candidate_sequence()
                        .checked_add(1)
                        .ok_or_else(|| {
                            RuntimeStoreError::WriteFailed(
                                "WholeBlob provisional candidate sequence exhausted".to_string(),
                            )
                        })?;
                    if existing.authority.session_id() != authority.session_id()
                        || existing.authority.base_store_revision()
                            != authority.base_store_revision()
                        || existing.authority.base_blob_sha256()
                            != authority.base_blob_sha256()
                        || existing.authority.run_id() != authority.run_id()
                        || authority.candidate_sequence() != required_sequence
                    {
                        return Err(session_authority_conflict(
                            &runtime_id,
                            "WholeBlob provisional replacement is stale or skips sequence",
                        ));
                    }
                } else if authority.candidate_sequence() != 1 {
                    return Err(session_authority_conflict(
                        &runtime_id,
                        "first WholeBlob provisional candidate sequence must be one",
                    ));
                }
                let base_revision = i64::try_from(authority.base_store_revision()).map_err(|_| {
                    RuntimeStoreError::WriteFailed(format!(
                        "WholeBlob provisional base revision exceeds SQLite INTEGER for runtime {runtime_id}"
                    ))
                })?;
                let message_count = i64::try_from(message_count).map_err(|_| {
                    RuntimeStoreError::WriteFailed(format!(
                        "WholeBlob provisional message count exceeds SQLite INTEGER for runtime {runtime_id}"
                    ))
                })?;
                tx.execute(
                    r"
                    INSERT INTO runtime_whole_blob_bodies (blob_sha256, session_snapshot)
                    VALUES (?1, ?2)
                    ON CONFLICT(blob_sha256) DO NOTHING
                    ",
                    params![authority.candidate_blob_sha256(), candidate_bytes.as_ref()],
                )
                .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
                if let Some(existing) = existing {
                    let updated = tx
                        .execute(
                            r"
                            UPDATE runtime_whole_blob_provisional_tails
                            SET candidate_sequence = ?7,
                                candidate_blob_sha256 = ?8,
                                conversation_digest = ?9,
                                message_count = ?10,
                                catalog_json = ?11,
                                compaction_intents_json = ?12
                            WHERE runtime_id = ?1
                              AND session_id = ?2
                              AND base_store_revision = ?3
                              AND base_blob_sha256 = ?4
                              AND run_id = ?5
                              AND candidate_sequence = ?6
                            ",
                            params![
                                runtime_id_text(&runtime_id),
                                authority.session_id().to_string(),
                                base_revision,
                                authority.base_blob_sha256(),
                                authority.run_id().0.to_string(),
                                i64::try_from(existing.authority.candidate_sequence()).map_err(
                                    |_| RuntimeStoreError::WriteFailed(
                                        "WholeBlob provisional sequence exceeds SQLite INTEGER"
                                            .to_string(),
                                    ),
                                )?,
                                i64::try_from(authority.candidate_sequence()).map_err(|_| {
                                    RuntimeStoreError::WriteFailed(
                                        "WholeBlob provisional sequence exceeds SQLite INTEGER"
                                            .to_string(),
                                    )
                                })?,
                                authority.candidate_blob_sha256(),
                                conversation_digest,
                                message_count,
                                catalog_json,
                                compaction_intents_json,
                            ],
                        )
                        .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
                    if updated != 1 {
                        return Err(session_authority_conflict(
                            &runtime_id,
                            "WholeBlob provisional candidate changed during replacement",
                        ));
                    }
                    if existing.authority.candidate_blob_sha256()
                        != authority.candidate_blob_sha256()
                    {
                        tx.execute(
                            r"
                            DELETE FROM runtime_whole_blob_bodies
                            WHERE blob_sha256 = ?1
                              AND NOT EXISTS (
                                  SELECT 1 FROM runtime_whole_blob_authority
                                  WHERE blob_sha256 = ?1
                              )
                              AND NOT EXISTS (
                                  SELECT 1 FROM runtime_whole_blob_provisional_tails
                                  WHERE candidate_blob_sha256 = ?1
                              )
                            ",
                            params![existing.authority.candidate_blob_sha256()],
                        )
                        .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
                    }
                } else {
                    tx.execute(
                        r"
                        INSERT INTO runtime_whole_blob_provisional_tails
                            (runtime_id, authority_version, session_id, base_store_revision,
                             base_blob_sha256, run_id, candidate_sequence,
                             candidate_blob_sha256, conversation_digest, message_count,
                             catalog_json, compaction_intents_json)
                        VALUES (?1, 1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                        ",
                        params![
                            runtime_id_text(&runtime_id),
                            authority.session_id().to_string(),
                            base_revision,
                            authority.base_blob_sha256(),
                            authority.run_id().0.to_string(),
                            i64::try_from(authority.candidate_sequence()).map_err(|_| {
                                RuntimeStoreError::WriteFailed(
                                    "WholeBlob provisional sequence exceeds SQLite INTEGER"
                                        .to_string(),
                                )
                            })?,
                            authority.candidate_blob_sha256(),
                            conversation_digest,
                            message_count,
                            catalog_json,
                            compaction_intents_json,
                        ],
                    )
                    .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
                }
                tx.commit()
                    .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
                Ok(authority)
            })
            .await
            .map_err(|error| RuntimeStoreError::Internal(format!("Task join failed: {error}")))?
        }

        async fn load_whole_blob_provisional_tail(
            &self,
            runtime_id: &LogicalRuntimeId,
        ) -> Result<Option<CommittedWholeBlobProvisionalTail>, RuntimeStoreError> {
            self.require_whole_blob_session_operation(
                runtime_id,
                "load_whole_blob_provisional_tail",
            )?;
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            tokio::task::spawn_blocking(move || {
                let mut conn = open_runtime_connection(&path)?;
                let tx = conn
                    .transaction()
                    .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
                let observed = load_whole_blob_provisional_tail(&tx, &runtime_id)?;
                tx.commit()
                    .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
                Ok(observed)
            })
            .await
            .map_err(|error| RuntimeStoreError::Internal(format!("Task join failed: {error}")))?
        }

        async fn discard_whole_blob_provisional_tail(
            &self,
            runtime_id: &LogicalRuntimeId,
            expected: &WholeBlobProvisionalTailAuthority,
        ) -> Result<bool, RuntimeStoreError> {
            self.require_whole_blob_session_operation(
                runtime_id,
                "discard_whole_blob_provisional_tail",
            )?;
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            let expected = expected.clone();
            tokio::task::spawn_blocking(move || {
                let mut conn = open_runtime_connection(&path)?;
                let tx = begin_runtime_transaction(&mut conn)?;
                let deleted = tx
                    .execute(
                        r"
                        DELETE FROM runtime_whole_blob_provisional_tails
                        WHERE runtime_id = ?1
                          AND session_id = ?2
                          AND base_store_revision = ?3
                          AND base_blob_sha256 = ?4
                          AND run_id = ?5
                          AND candidate_blob_sha256 = ?6
                          AND candidate_sequence = ?7
                        ",
                        params![
                            runtime_id_text(&runtime_id),
                            expected.session_id().to_string(),
                            i64::try_from(expected.base_store_revision()).map_err(|_| {
                                RuntimeStoreError::WriteFailed(
                                    "WholeBlob provisional base revision exceeds SQLite INTEGER"
                                        .to_string(),
                                )
                            })?,
                            expected.base_blob_sha256(),
                            expected.run_id().0.to_string(),
                            expected.candidate_blob_sha256(),
                            i64::try_from(expected.candidate_sequence()).map_err(|_| {
                                RuntimeStoreError::WriteFailed(
                                    "WholeBlob provisional sequence exceeds SQLite INTEGER"
                                        .to_string(),
                                )
                            })?,
                        ],
                    )
                    .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
                if deleted == 1 {
                    tx.execute(
                        r"
                        DELETE FROM runtime_whole_blob_bodies
                        WHERE blob_sha256 = ?1
                          AND NOT EXISTS (
                              SELECT 1 FROM runtime_whole_blob_authority
                              WHERE blob_sha256 = ?1
                          )
                          AND NOT EXISTS (
                              SELECT 1 FROM runtime_whole_blob_provisional_tails
                              WHERE candidate_blob_sha256 = ?1
                          )
                        ",
                        params![expected.candidate_blob_sha256()],
                    )
                    .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
                }
                tx.commit()
                    .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
                Ok(deleted == 1)
            })
            .await
            .map_err(|error| RuntimeStoreError::Internal(format!("Task join failed: {error}")))?
        }

        async fn write_prepared_head_canonical_provisional_tail(
            &self,
            runtime_id: &LogicalRuntimeId,
            prepared: PreparedHeadCanonicalProvisionalTail,
        ) -> Result<HeadCanonicalProvisionalTailAuthority, RuntimeStoreError> {
            self.require_head_canonical_session_operation(
                runtime_id,
                "write_prepared_head_canonical_provisional_tail",
            )?;
            if &LogicalRuntimeId::for_session(prepared.committed().session_id()) != runtime_id
                || &prepared.successor_head().id != prepared.committed().session_id()
            {
                return Err(session_authority_conflict(
                    runtime_id,
                    "HeadCanonical provisional intent does not bind this runtime/session",
                ));
            }
            let derived_successor_token = meerkat_core::session_head_cas_token(
                prepared.successor_head(),
            )
            .map_err(|error| {
                session_authority_conflict(
                    runtime_id,
                    format!("HeadCanonical provisional successor is invalid: {error}"),
                )
            })?;
            if derived_successor_token.as_str() != prepared.successor_head_token() {
                return Err(session_authority_conflict(
                    runtime_id,
                    "HeadCanonical provisional successor token differs from its exact head",
                ));
            }
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            tokio::task::spawn_blocking(move || {
                let mut conn = open_head_canonical_runtime_connection(&path)?;
                let tx = begin_runtime_transaction(&mut conn)?;
                let authority = issue_head_canonical_provisional_tail_authority_in_txn(
                    &tx,
                    &runtime_id,
                    prepared.committed(),
                    prepared.run_id(),
                    prepared.successor_head(),
                    prepared.successor_head_token(),
                    prepared.candidate_message_count(),
                    prepared.candidate_conversation_digest(),
                    prepared.catalog_entry(),
                    prepared.compaction_projection_intents(),
                )?;
                tx.commit()
                    .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
                Ok(authority)
            })
            .await
            .map_err(|error| RuntimeStoreError::Internal(format!("Task join failed: {error}")))?
        }

        async fn load_head_canonical_provisional_tail(
            &self,
            runtime_id: &LogicalRuntimeId,
        ) -> Result<Option<HeadCanonicalProvisionalTailAuthority>, RuntimeStoreError> {
            self.require_head_canonical_session_operation(
                runtime_id,
                "load_head_canonical_provisional_tail",
            )?;
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            tokio::task::spawn_blocking(move || {
                let conn = open_head_canonical_runtime_connection(&path)?;
                load_head_canonical_provisional_tail_authority(&conn, &runtime_id)
            })
            .await
            .map_err(|error| RuntimeStoreError::Internal(format!("Task join failed: {error}")))?
        }

        async fn discard_head_canonical_provisional_tail(
            &self,
            runtime_id: &LogicalRuntimeId,
            expected: &HeadCanonicalProvisionalTailAuthority,
        ) -> Result<bool, RuntimeStoreError> {
            self.require_head_canonical_session_operation(
                runtime_id,
                "discard_head_canonical_provisional_tail",
            )?;
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            let expected = expected.clone();
            tokio::task::spawn_blocking(move || {
                let mut conn = open_head_canonical_runtime_connection(&path)?;
                let tx = begin_runtime_transaction(&mut conn)?;
                let discarded = discard_head_canonical_provisional_tail_authority_in_txn(
                    &tx,
                    &runtime_id,
                    &expected,
                )?;
                tx.commit()
                    .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
                Ok(discarded)
            })
            .await
            .map_err(|error| RuntimeStoreError::Internal(format!("Task join failed: {error}")))?
        }

        async fn clear_session_snapshot(
            &self,
            runtime_id: &LogicalRuntimeId,
        ) -> Result<(), RuntimeStoreError> {
            self.require_whole_blob_session_operation(runtime_id, "clear_session_snapshot")?;
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            tokio::task::spawn_blocking(move || {
                let mut conn = open_runtime_connection(&path)?;
                let tx = begin_runtime_transaction(&mut conn)?;
                if load_head_canonical_authority(&tx, &runtime_id)?.is_some() {
                    return Err(session_authority_conflict(
                        &runtime_id,
                        "whole-BLOB clear refused because head-canonical authority is installed",
                    ));
                }
                delete_whole_blob_state(&tx, &runtime_id)?;
                tx.commit()
                    .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                Ok(())
            })
            .await
            .map_err(|err| RuntimeStoreError::Internal(format!("Task join failed: {err}")))?
        }

        async fn replace_session_snapshot_if_current(
            &self,
            runtime_id: &LogicalRuntimeId,
            expected_current: &[u8],
            replacement: Vec<u8>,
        ) -> Result<bool, RuntimeStoreError> {
            self.require_whole_blob_session_operation(
                runtime_id,
                "replace_session_snapshot_if_current",
            )?;
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            let expected_current = expected_current.to_vec();
            tokio::task::spawn_blocking(move || {
                let mut conn = open_runtime_connection(&path)?;
                let tx = begin_runtime_transaction(&mut conn)?;
                if load_head_canonical_authority(&tx, &runtime_id)?.is_some() {
                    return Err(session_authority_conflict(
                        &runtime_id,
                        "whole-BLOB replacement refused because head-canonical authority is installed",
                    ));
                }
                let replacement_session: meerkat_core::Session =
                    serde_json::from_slice(&replacement)
                        .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                let current = tx
                    .query_row(
                        "SELECT session_snapshot FROM runtime_session_snapshots WHERE runtime_id = ?1",
                        params![runtime_id_text(&runtime_id)],
                        |row| Ok(row.get::<_, JsonColumnBytes>(0)?.into_bytes()),
                    )
                    .optional()
                    .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))?;
                if current.as_deref() != Some(expected_current.as_slice()) {
                    return Ok(false);
                }
                ensure_compaction_intents_already_outboxed(
                    &tx,
                    &runtime_id,
                    &replacement_session,
                )?;
                upsert_runtime_snapshot(&tx, &runtime_id, &replacement)?;
                tx.commit()
                    .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                Ok(true)
            })
            .await
            .map_err(|err| RuntimeStoreError::Internal(format!("Task join failed: {err}")))?
        }

        async fn clear_session_snapshot_if_current(
            &self,
            runtime_id: &LogicalRuntimeId,
            expected_current: &[u8],
        ) -> Result<bool, RuntimeStoreError> {
            self.require_whole_blob_session_operation(
                runtime_id,
                "clear_session_snapshot_if_current",
            )?;
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            let expected_current = expected_current.to_vec();
            tokio::task::spawn_blocking(move || {
                let mut conn = open_runtime_connection(&path)?;
                let tx = begin_runtime_transaction(&mut conn)?;
                if load_head_canonical_authority(&tx, &runtime_id)?.is_some() {
                    return Err(session_authority_conflict(
                        &runtime_id,
                        "whole-BLOB conditional clear refused because head-canonical authority is installed",
                    ));
                }
                let current = tx
                    .query_row(
                        "SELECT session_snapshot FROM runtime_session_snapshots WHERE runtime_id = ?1",
                        params![runtime_id_text(&runtime_id)],
                        |row| Ok(row.get::<_, JsonColumnBytes>(0)?.into_bytes()),
                    )
                    .optional()
                    .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))?;
                if current.as_deref() != Some(expected_current.as_slice()) {
                    return Ok(false);
                }
                tx.execute(
                    "DELETE FROM runtime_session_snapshots WHERE runtime_id = ?1",
                    params![runtime_id_text(&runtime_id)],
                )
                .map_err(|error| map_runtime_snapshot_mutation_error(&runtime_id, error))?;
                // Record the durable quarantine marker in the SAME transaction
                // that deletes the rejected runtime snapshot, so the fact
                // survives a process restart.
                set_runtime_projection_quarantine(&tx, &runtime_id)?;
                tx.commit()
                    .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                Ok(true)
            })
            .await
            .map_err(|err| RuntimeStoreError::Internal(format!("Task join failed: {err}")))?
        }

        async fn is_runtime_projection_quarantined(
            &self,
            runtime_id: &LogicalRuntimeId,
        ) -> Result<bool, RuntimeStoreError> {
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            tokio::task::spawn_blocking(move || {
                let conn = open_runtime_connection(&path)?;
                conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM runtime_projection_quarantine WHERE runtime_id = ?1)",
                    params![runtime_id_text(&runtime_id)],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))
            })
            .await
            .map_err(|err| RuntimeStoreError::Internal(format!("Task join failed: {err}")))?
        }

        async fn persist_input_state(
            &self,
            runtime_id: &LogicalRuntimeId,
            state: &InputStatePersistenceRecord,
        ) -> Result<(), RuntimeStoreError> {
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            let state = (
                state.clone_stored(),
                state.expected_row_digest().map(str::to_owned),
            );
            tokio::task::spawn_blocking(move || {
                let mut conn = open_runtime_connection(&path)?;
                let tx = begin_runtime_transaction(&mut conn)?;
                upsert_input_states(&tx, &runtime_id, &[state])?;
                tx.commit()
                    .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                Ok(())
            })
            .await
            .map_err(|err| RuntimeStoreError::Internal(format!("Task join failed: {err}")))?
        }

        async fn persist_input_states_atomically(
            &self,
            runtime_id: &LogicalRuntimeId,
            records: &[InputStatePersistenceRecord],
        ) -> Result<(), RuntimeStoreError> {
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            let states: Vec<_> = records
                .iter()
                .map(|record| {
                    (
                        record.clone_stored(),
                        record.expected_row_digest().map(str::to_owned),
                    )
                })
                .collect();
            tokio::task::spawn_blocking(move || {
                let mut conn = open_runtime_connection(&path)?;
                let tx = begin_runtime_transaction(&mut conn)?;
                upsert_input_states(&tx, &runtime_id, &states)?;
                tx.commit()
                    .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                Ok(())
            })
            .await
            .map_err(|err| RuntimeStoreError::Internal(format!("Task join failed: {err}")))?
        }

        async fn compare_and_swap_input_states_atomically(
            &self,
            runtime_id: &LogicalRuntimeId,
            expected: &[StoredInputState],
            replacements: &[InputStatePersistenceRecord],
        ) -> Result<InputStateBatchCasOutcome, RuntimeStoreError> {
            // Validate keys/counts and serialize both sides before opening the
            // transaction. The transaction then contains only exact reads,
            // deterministic writes, and commit.
            let prepared = prepare_input_state_batch_cas(expected, replacements)?;
            if prepared.is_empty() {
                return Ok(InputStateBatchCasOutcome::Swapped);
            }
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            tokio::task::spawn_blocking(move || {
                let mut conn = open_runtime_connection(&path)?;
                let tx = begin_runtime_transaction(&mut conn)?;
                let mut all_expected = true;
                let mut all_replacements = true;
                for row in &prepared {
                    let current = tx
                        .query_row(
                            r"
                            SELECT state_json
                            FROM runtime_input_states
                            WHERE runtime_id = ?1 AND input_id = ?2
                            ",
                            params![runtime_id_text(&runtime_id), row.input_id.0.to_string()],
                            |sql_row| Ok(sql_row.get::<_, JsonColumnBytes>(0)?.into_bytes()),
                        )
                        .optional()
                        .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
                    let Some(current) = current else {
                        return Ok(InputStateBatchCasOutcome::Stale);
                    };
                    if current != row.expected_json {
                        all_expected = false;
                    }
                    if current != row.replacement_json {
                        all_replacements = false;
                    }
                }

                if all_replacements {
                    return Ok(InputStateBatchCasOutcome::Swapped);
                }
                if !all_expected {
                    return Ok(InputStateBatchCasOutcome::Stale);
                }

                release_input_idempotency_keys_for_mutation_set(
                    &tx,
                    &runtime_id,
                    prepared.iter().map(|row| row.input_id.to_string()),
                )?;
                for row in &prepared {
                    tx.execute(
                        r"
                        INSERT INTO runtime_input_states (runtime_id, input_id, state_json)
                        VALUES (?1, ?2, ?3)
                        ON CONFLICT(runtime_id, input_id) DO UPDATE
                        SET state_json = excluded.state_json
                        ",
                        params![
                            runtime_id_text(&runtime_id),
                            row.input_id.0.to_string(),
                            &row.replacement_json,
                        ],
                    )
                    .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
                    update_pending_terminal_owner_index(&tx, &runtime_id, &row.replacement)?;
                }
                tx.commit()
                    .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
                Ok(InputStateBatchCasOutcome::Swapped)
            })
            .await
            .map_err(|err| RuntimeStoreError::Internal(format!("Task join failed: {err}")))?
        }

        async fn compare_and_swap_input_states_atomically_with_fence(
            &self,
            runtime_id: &LogicalRuntimeId,
            expected: &[StoredInputState],
            replacements: &[InputStatePersistenceRecord],
            write_fence: Arc<dyn RuntimeStoreWriteFence>,
        ) -> Result<FencedInputStateBatchCasOutcome, RuntimeStoreError> {
            let prepared = prepare_input_state_batch_cas(expected, replacements)?;
            if prepared.is_empty() {
                return Ok(FencedInputStateBatchCasOutcome::Swapped);
            }
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            tokio::task::spawn_blocking(move || {
                let mut conn = open_runtime_connection(&path)?;
                let tx = begin_runtime_transaction(&mut conn)?;
                let mut all_expected = true;
                let mut all_replacements = true;
                for row in &prepared {
                    let current = tx
                        .query_row(
                            r"
                            SELECT state_json
                            FROM runtime_input_states
                            WHERE runtime_id = ?1 AND input_id = ?2
                            ",
                            params![runtime_id_text(&runtime_id), row.input_id.0.to_string()],
                            |sql_row| Ok(sql_row.get::<_, JsonColumnBytes>(0)?.into_bytes()),
                        )
                        .optional()
                        .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
                    let Some(current) = current else {
                        return Ok(FencedInputStateBatchCasOutcome::Stale);
                    };
                    if current != row.expected_json {
                        all_expected = false;
                    }
                    if current != row.replacement_json {
                        all_replacements = false;
                    }
                }
                if !all_replacements && !all_expected {
                    return Ok(FencedInputStateBatchCasOutcome::Stale);
                }

                let mut tx_slot = Some(tx);
                let fence_outcome =
                    execute_runtime_store_write_fence(write_fence.as_ref(), || {
                        let tx = tx_slot.as_ref().ok_or_else(|| {
                            RuntimeStoreError::Internal(
                                "SQLite input recovery fence lost its transaction".to_string(),
                            )
                        })?;
                        if !all_replacements {
                            release_input_idempotency_keys_for_mutation_set(
                                tx,
                                &runtime_id,
                                prepared.iter().map(|row| row.input_id.to_string()),
                            )?;
                            for row in &prepared {
                                tx.execute(
                                    r"
                                    INSERT INTO runtime_input_states
                                        (runtime_id, input_id, state_json)
                                    VALUES (?1, ?2, ?3)
                                    ON CONFLICT(runtime_id, input_id) DO UPDATE
                                    SET state_json = excluded.state_json
                                    ",
                                    params![
                                        runtime_id_text(&runtime_id),
                                        row.input_id.0.to_string(),
                                        &row.replacement_json,
                                    ],
                                )
                                .map_err(|error| {
                                    RuntimeStoreError::WriteFailed(error.to_string())
                                })?;
                                update_pending_terminal_owner_index(
                                    tx,
                                    &runtime_id,
                                    &row.replacement,
                                )?;
                            }
                        }
                        tx_slot
                            .take()
                            .ok_or_else(|| {
                                RuntimeStoreError::Internal(
                                    "SQLite input recovery fence consumed its transaction twice"
                                        .to_string(),
                                )
                            })?
                            .commit()
                            .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))
                    })?;
                match fence_outcome {
                    RuntimeStoreWriteFenceOutcome::Applied => {
                        Ok(FencedInputStateBatchCasOutcome::Swapped)
                    }
                    RuntimeStoreWriteFenceOutcome::Conflict { reason } => {
                        Ok(FencedInputStateBatchCasOutcome::FenceConflict { reason })
                    }
                    RuntimeStoreWriteFenceOutcome::Backoff { reason } => {
                        Ok(FencedInputStateBatchCasOutcome::FenceBackoff { reason })
                    }
                }
            })
            .await
            .map_err(|err| RuntimeStoreError::Internal(format!("Task join failed: {err}")))?
        }

        async fn compare_and_swap_recovery_input_states_atomically(
            &self,
            runtime_id: &LogicalRuntimeId,
            expected_revision: RecoveryInputSetRevision,
            mutations: &[RecoveryInputStateMutation],
        ) -> Result<InputStateBatchCasOutcome, RuntimeStoreError> {
            let prepared = prepare_recovery_input_state_mutations(mutations)?;
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            tokio::task::spawn_blocking(move || {
                let mut conn = open_runtime_connection(&path)?;
                let tx = begin_runtime_transaction(&mut conn)?;
                let Some(changed) = prepare_current_sqlite_recovery_input_mutations(
                    &tx,
                    &runtime_id,
                    expected_revision,
                    &prepared,
                )?
                else {
                    return Ok(InputStateBatchCasOutcome::Stale);
                };
                apply_sqlite_recovery_input_mutations(&tx, &runtime_id, &changed)?;
                tx.commit()
                    .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
                Ok(InputStateBatchCasOutcome::Swapped)
            })
            .await
            .map_err(|err| RuntimeStoreError::Internal(format!("Task join failed: {err}")))?
        }

        async fn compare_and_swap_recovery_input_states_atomically_with_fence(
            &self,
            runtime_id: &LogicalRuntimeId,
            expected_revision: RecoveryInputSetRevision,
            mutations: &[RecoveryInputStateMutation],
            write_fence: Arc<dyn RuntimeStoreWriteFence>,
        ) -> Result<FencedInputStateBatchCasOutcome, RuntimeStoreError> {
            let prepared = prepare_recovery_input_state_mutations(mutations)?;
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            tokio::task::spawn_blocking(move || {
                let mut conn = open_runtime_connection(&path)?;
                let tx = begin_runtime_transaction(&mut conn)?;
                let Some(changed) = prepare_current_sqlite_recovery_input_mutations(
                    &tx,
                    &runtime_id,
                    expected_revision,
                    &prepared,
                )?
                else {
                    return Ok(FencedInputStateBatchCasOutcome::Stale);
                };

                let mut tx_slot = Some(tx);
                let fence_outcome =
                    execute_runtime_store_write_fence(write_fence.as_ref(), || {
                        let tx = tx_slot.as_ref().ok_or_else(|| {
                            RuntimeStoreError::Internal(
                                "SQLite recovery input fence lost its transaction".to_string(),
                            )
                        })?;
                        apply_sqlite_recovery_input_mutations(tx, &runtime_id, &changed)?;
                        tx_slot
                            .take()
                            .ok_or_else(|| {
                                RuntimeStoreError::Internal(
                                    "SQLite recovery input fence consumed its transaction twice"
                                        .to_string(),
                                )
                            })?
                            .commit()
                            .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))
                    })?;
                match fence_outcome {
                    RuntimeStoreWriteFenceOutcome::Applied => {
                        Ok(FencedInputStateBatchCasOutcome::Swapped)
                    }
                    RuntimeStoreWriteFenceOutcome::Conflict { reason } => {
                        Ok(FencedInputStateBatchCasOutcome::FenceConflict { reason })
                    }
                    RuntimeStoreWriteFenceOutcome::Backoff { reason } => {
                        Ok(FencedInputStateBatchCasOutcome::FenceBackoff { reason })
                    }
                }
            })
            .await
            .map_err(|err| RuntimeStoreError::Internal(format!("Task join failed: {err}")))?
        }

        async fn load_input_state(
            &self,
            runtime_id: &LogicalRuntimeId,
            input_id: &InputId,
        ) -> Result<Option<StoredInputState>, RuntimeStoreError> {
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            let input_id = input_id.clone();
            tokio::task::spawn_blocking(move || {
                let conn = open_runtime_connection(&path)?;
                conn.query_row(
                    r"
                    SELECT state_json
                    FROM runtime_input_states
                    WHERE runtime_id = ?1 AND input_id = ?2
                    ",
                    params![runtime_id_text(&runtime_id), input_id.0.to_string()],
                    |row| Ok(row.get::<_, JsonColumnBytes>(0)?.into_bytes()),
                )
                .optional()
                .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))?
                .map(|bytes| deserialize_persisted_input_state(&bytes))
                .transpose()
            })
            .await
            .map_err(|err| RuntimeStoreError::Internal(format!("Task join failed: {err}")))?
        }

        async fn load_input_state_by_idempotency_key(
            &self,
            runtime_id: &LogicalRuntimeId,
            key: &IdempotencyKey,
        ) -> Result<Option<ExactInputStateObservation>, RuntimeStoreError> {
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            let key = key.clone();
            tokio::task::spawn_blocking(move || {
                let mut conn = open_runtime_connection(&path)?;
                // Completeness evidence and the keyed row must come from one
                // SQLite snapshot. Separate autocommit reads can interleave
                // with a corrupt-row repair and manufacture a miss that no
                // single database state ever authorized.
                let tx = conn
                    .transaction()
                    .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
                let uncertain = |evidence_input_id: String, reason: String| {
                    RuntimeStoreError::InputIdempotencyIndexUncertain {
                        runtime_id: runtime_id.to_string(),
                        key: key.to_string(),
                        evidence_input_id,
                        reason,
                    }
                };
                let unindexable_row = tx
                    .query_row(
                        r"
                        SELECT input_id, reason
                        FROM runtime_input_idempotency_unindexable_rows
                        WHERE runtime_id = ?1
                        ORDER BY input_id
                        LIMIT 1
                        ",
                        params![runtime_id_text(&runtime_id)],
                        |row| {
                            let (input_id, invalid_input_id_storage) =
                                sqlite_text_evidence(row.get_ref(0)?);
                            let (reason, invalid_reason_storage) =
                                sqlite_text_evidence(row.get_ref(1)?);
                            Ok((
                                input_id,
                                invalid_input_id_storage,
                                reason,
                                invalid_reason_storage,
                            ))
                        },
                    )
                    .optional()
                    .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
                if let Some((
                    evidence_input_id,
                    invalid_input_id_storage,
                    reason,
                    invalid_reason_storage,
                )) = unindexable_row
                {
                    if let Some(storage_class) = invalid_input_id_storage {
                        return Err(uncertain(
                            evidence_input_id,
                            format!(
                                "unindexable-row evidence input_id has invalid SQLite \
                                 representation {storage_class}"
                            ),
                        ));
                    }
                    if let Some(storage_class) = invalid_reason_storage {
                        return Err(uncertain(
                            evidence_input_id,
                            format!(
                                "unindexable-row evidence reason has invalid SQLite representation \
                                 {storage_class}"
                            ),
                        ));
                    }
                    return Err(uncertain(evidence_input_id, reason));
                }

                let row = tx
                    .query_row(
                        r"
                        SELECT indexed.input_id, states.state_json
                        FROM runtime_input_idempotency_keys AS indexed
                        LEFT JOIN runtime_input_states AS states
                          ON states.runtime_id = indexed.runtime_id
                         AND states.input_id = indexed.input_id
                        WHERE indexed.runtime_id = ?1
                          AND indexed.idempotency_key = ?2
                        ",
                        params![runtime_id_text(&runtime_id), &key.0],
                        |row| {
                            let (indexed_input_id, invalid_owner_storage_class) =
                                sqlite_text_evidence(row.get_ref(0)?);
                            let (bytes, invalid_storage_class) = match row.get_ref(1)? {
                                rusqlite::types::ValueRef::Null => (None, None),
                                rusqlite::types::ValueRef::Text(bytes)
                                | rusqlite::types::ValueRef::Blob(bytes) => {
                                    (Some(bytes.to_vec()), None)
                                }
                                rusqlite::types::ValueRef::Integer(_) => (None, Some("INTEGER")),
                                rusqlite::types::ValueRef::Real(_) => (None, Some("REAL")),
                            };
                            Ok((
                                indexed_input_id,
                                invalid_owner_storage_class,
                                bytes,
                                invalid_storage_class,
                            ))
                        },
                    )
                    .optional()
                    .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
                let Some((
                    indexed_input_id,
                    invalid_owner_storage_class,
                    bytes,
                    invalid_storage_class,
                )) = row
                else {
                    tx.commit()
                        .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
                    return Ok(None);
                };
                if let Some(storage_class) = invalid_owner_storage_class {
                    return Err(uncertain(
                        indexed_input_id,
                        format!(
                            "indexed owner input_id has invalid SQLite representation \
                             {storage_class}"
                        ),
                    ));
                }
                if let Some(storage_class) = invalid_storage_class {
                    return Err(uncertain(
                        indexed_input_id,
                        format!(
                            "indexed source state_json has non-JSON SQLite storage class \
                             {storage_class}"
                        ),
                    ));
                }
                let bytes = bytes.ok_or_else(|| {
                    uncertain(
                        indexed_input_id.clone(),
                        "index names a missing source input row".to_string(),
                    )
                })?;
                let state = deserialize_persisted_input_state(&bytes).map_err(|error| {
                    uncertain(
                        indexed_input_id.clone(),
                        format!("indexed source input row is not a valid stored state: {error}"),
                    )
                })?;
                if state.state.input_id.to_string() != indexed_input_id
                    || state.state.idempotency_key.as_ref() != Some(&key)
                {
                    return Err(uncertain(
                        indexed_input_id,
                        format!(
                            "index owner differs from decoded source identity/key \
                             (decoded input {}, decoded key {:?})",
                            state.state.input_id, state.state.idempotency_key
                        ),
                    ));
                }
                crate::meerkat_machine::authorize_stored_input_state_seed(
                    &state.state.input_id,
                    &state.seed,
                )
                .map_err(|error| {
                    uncertain(
                        indexed_input_id.clone(),
                        format!(
                            "indexed source input row has a non-authoritative machine seed: {error}"
                        ),
                    )
                })?;
                let observation = ExactInputStateObservation::from_exact_stored_row(
                    state,
                    input_row_version_digest(&bytes),
                )
                .map_err(|error| {
                    uncertain(
                        indexed_input_id,
                        format!(
                            "indexed source row could not produce an exact observation: {error}"
                        ),
                    )
                })?;
                tx.commit()
                    .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
                Ok(Some(observation))
            })
            .await
            .map_err(|err| RuntimeStoreError::Internal(format!("Task join failed: {err}")))?
        }

        async fn load_input_states_by_ids(
            &self,
            runtime_id: &LogicalRuntimeId,
            input_ids: &[InputId],
        ) -> Result<Vec<Option<StoredInputState>>, RuntimeStoreError> {
            validate_input_state_batch_read_ids(input_ids)?;
            if input_ids.is_empty() {
                return Ok(Vec::new());
            }
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            let input_ids = input_ids.to_vec();
            tokio::task::spawn_blocking(move || {
                let conn = open_runtime_connection(&path)?;
                let placeholders = (0..input_ids.len())
                    .map(|index| format!("?{}", index + 2))
                    .collect::<Vec<_>>()
                    .join(", ");
                let sql = format!(
                    "SELECT input_id, state_json \
                     FROM runtime_input_states \
                     WHERE runtime_id = ?1 AND input_id IN ({placeholders})"
                );
                let mut values = Vec::with_capacity(input_ids.len() + 1);
                values.push(rusqlite::types::Value::Text(
                    runtime_id_text(&runtime_id).to_owned(),
                ));
                values.extend(
                    input_ids
                        .iter()
                        .map(|input_id| rusqlite::types::Value::Text(input_id.0.to_string())),
                );
                let mut statement = conn
                    .prepare(&sql)
                    .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
                let rows = statement
                    .query_map(rusqlite::params_from_iter(values.iter()), |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, JsonColumnBytes>(1)?.into_bytes(),
                        ))
                    })
                    .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
                let mut by_id = HashMap::with_capacity(input_ids.len());
                for row in rows {
                    let (stored_key, bytes) =
                        row.map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
                    let decoded = deserialize_persisted_input_state(&bytes)?;
                    if decoded.state.input_id.0.to_string() != stored_key {
                        return Err(RuntimeStoreError::ReadFailed(format!(
                            "input state row key `{stored_key}` differs from its encoded identity"
                        )));
                    }
                    if by_id.insert(stored_key.clone(), decoded).is_some() {
                        return Err(RuntimeStoreError::ReadFailed(format!(
                            "duplicate input state row `{stored_key}`"
                        )));
                    }
                }
                Ok(input_ids
                    .iter()
                    .map(|input_id| by_id.remove(&input_id.0.to_string()))
                    .collect())
            })
            .await
            .map_err(|err| RuntimeStoreError::Internal(format!("Task join failed: {err}")))?
        }

        async fn load_pending_terminal_owner_ids_page(
            &self,
            runtime_id: &LogicalRuntimeId,
            after: Option<&InputId>,
            limit: usize,
        ) -> Result<Vec<InputId>, RuntimeStoreError> {
            crate::store::validate_pending_terminal_owner_page(after, limit, &[])?;
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            let after = after.map(|input_id| input_id.0.to_string());
            tokio::task::spawn_blocking(move || {
                let conn = open_runtime_connection(&path)?;
                let sql_limit = i64::try_from(limit).map_err(|_| {
                    RuntimeStoreError::ReadFailed(
                        "pending-terminal owner page limit exceeds SQLite INTEGER".to_string(),
                    )
                })?;
                let encoded_owner_input_ids = if let Some(after) = after.as_deref() {
                    let mut statement = conn
                        .prepare(LOAD_PENDING_TERMINAL_OWNER_CONTINUATION_PAGE_SQL)
                        .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
                    statement
                        .query_map(
                            params![runtime_id_text(&runtime_id), after, sql_limit],
                            |row| row.get::<_, String>(0),
                        )
                        .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?
                } else {
                    let mut statement = conn
                        .prepare(LOAD_PENDING_TERMINAL_OWNER_FIRST_PAGE_SQL)
                        .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
                    statement
                        .query_map(params![runtime_id_text(&runtime_id), sql_limit], |row| {
                            row.get::<_, String>(0)
                        })
                        .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?
                };
                let mut owner_input_ids = Vec::new();
                for encoded in encoded_owner_input_ids {
                    let input_id = encoded.parse::<uuid::Uuid>().map_err(|error| {
                        RuntimeStoreError::ReadFailed(format!(
                            "pending-terminal owner id `{encoded}` is malformed: {error}"
                        ))
                    })?;
                    owner_input_ids.push(InputId::from_uuid(input_id));
                }
                let after = after
                    .as_deref()
                    .and_then(|encoded| encoded.parse::<uuid::Uuid>().ok())
                    .map(InputId::from_uuid);
                crate::store::validate_pending_terminal_owner_page(
                    after.as_ref(),
                    limit,
                    &owner_input_ids,
                )?;
                Ok(owner_input_ids)
            })
            .await
            .map_err(|err| RuntimeStoreError::Internal(format!("Task join failed: {err}")))?
        }

        async fn observe_machine_lifecycle(
            &self,
            runtime_id: &LogicalRuntimeId,
        ) -> Result<MachineLifecycleObservation, RuntimeStoreError> {
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            tokio::task::spawn_blocking(move || {
                let conn = open_runtime_connection(&path)?;
                let raw = conn
                    .query_row(
                        "SELECT runtime_state_json FROM runtime_states WHERE runtime_id = ?1",
                        params![runtime_id_text(&runtime_id)],
                        |row| Ok(row.get::<_, JsonColumnBytes>(0)?.into_bytes()),
                    )
                    .optional()
                    .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))?;
                Ok(raw.as_deref().map_or(
                    MachineLifecycleObservation::Missing,
                    classify_machine_lifecycle_record,
                ))
            })
            .await
            .map_err(|err| RuntimeStoreError::Internal(format!("Task join failed: {err}")))?
        }

        async fn compare_and_swap_machine_lifecycle(
            &self,
            runtime_id: &LogicalRuntimeId,
            expected: MachineLifecycleExpectedVersion,
            replacement: MachineLifecycleCommit,
        ) -> Result<MachineLifecycleCasOutcome, RuntimeStoreError> {
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            let replacement = prepare_machine_lifecycle_replacement(replacement)?;
            tokio::task::spawn_blocking(move || {
                let mut conn = open_runtime_connection(&path)?;
                let tx = begin_runtime_transaction(&mut conn)?;
                let current_raw = tx
                    .query_row(
                        "SELECT runtime_state_json FROM runtime_states WHERE runtime_id = ?1",
                        params![runtime_id_text(&runtime_id)],
                        |row| Ok(row.get::<_, JsonColumnBytes>(0)?.into_bytes()),
                    )
                    .optional()
                    .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))?;
                let current = current_raw.as_deref().map_or(
                    MachineLifecycleObservation::Missing,
                    classify_machine_lifecycle_record,
                );
                let matches = match (&expected, &current) {
                    (
                        MachineLifecycleExpectedVersion::Missing,
                        MachineLifecycleObservation::Missing,
                    ) => true,
                    (MachineLifecycleExpectedVersion::Version(expected), current) => {
                        current.version().is_some_and(|actual| actual == expected)
                    }
                    _ => false,
                };
                if !matches {
                    return Ok(MachineLifecycleCasOutcome::Conflict { current });
                }
                let replacement = replacement.preserve_observed_custody(&current)?;
                validate_machine_lifecycle_replacement(
                    &current,
                    current_raw.as_deref(),
                    &replacement.snapshot,
                )?;
                tx.execute(
                    r"
                    INSERT INTO runtime_states (runtime_id, runtime_state_json)
                    VALUES (?1, ?2)
                    ON CONFLICT(runtime_id) DO UPDATE SET
                        runtime_state_json = excluded.runtime_state_json
                    ",
                    params![runtime_id_text(&runtime_id), replacement.bytes],
                )
                .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                tx.commit()
                    .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                Ok(MachineLifecycleCasOutcome::Applied {
                    version: replacement.version,
                })
            })
            .await
            .map_err(|err| RuntimeStoreError::Internal(format!("Task join failed: {err}")))?
        }

        async fn compare_and_swap_machine_lifecycle_with_fence(
            &self,
            runtime_id: &LogicalRuntimeId,
            expected: MachineLifecycleExpectedVersion,
            replacement: MachineLifecycleCommit,
            write_fence: Arc<dyn RuntimeStoreWriteFence>,
        ) -> Result<FencedMachineLifecycleCasOutcome, RuntimeStoreError> {
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            let replacement = prepare_machine_lifecycle_replacement(replacement)?;
            tokio::task::spawn_blocking(move || {
                let mut conn = open_runtime_connection(&path)?;
                let tx = begin_runtime_transaction(&mut conn)?;
                let current_raw = tx
                    .query_row(
                        "SELECT runtime_state_json FROM runtime_states WHERE runtime_id = ?1",
                        params![runtime_id_text(&runtime_id)],
                        |row| Ok(row.get::<_, JsonColumnBytes>(0)?.into_bytes()),
                    )
                    .optional()
                    .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))?;
                let current = current_raw.as_deref().map_or(
                    MachineLifecycleObservation::Missing,
                    classify_machine_lifecycle_record,
                );
                let matches = match (&expected, &current) {
                    (
                        MachineLifecycleExpectedVersion::Missing,
                        MachineLifecycleObservation::Missing,
                    ) => true,
                    (MachineLifecycleExpectedVersion::Version(expected), current) => {
                        current.version().is_some_and(|actual| actual == expected)
                    }
                    _ => false,
                };
                if !matches {
                    return Ok(FencedMachineLifecycleCasOutcome::Conflict { current });
                }
                let replacement = replacement.preserve_observed_custody(&current)?;
                validate_machine_lifecycle_replacement(
                    &current,
                    current_raw.as_deref(),
                    &replacement.snapshot,
                )?;
                let already_exact = current_raw.as_deref() == Some(replacement.bytes.as_slice());
                let record = decoded_prepared_machine_lifecycle_replacement(&replacement)?;
                let version = replacement.version.clone();
                let replacement_bytes = replacement.bytes;
                let mut tx_slot = Some(tx);
                let fence_outcome =
                    execute_runtime_store_write_fence(write_fence.as_ref(), || {
                        let tx = tx_slot.as_ref().ok_or_else(|| {
                            RuntimeStoreError::Internal(
                                "SQLite lifecycle fence lost its transaction".to_string(),
                            )
                        })?;
                        if !already_exact {
                            tx.execute(
                                r"
                                INSERT INTO runtime_states (runtime_id, runtime_state_json)
                                VALUES (?1, ?2)
                                ON CONFLICT(runtime_id) DO UPDATE SET
                                    runtime_state_json = excluded.runtime_state_json
                                ",
                                params![runtime_id_text(&runtime_id), &replacement_bytes],
                            )
                            .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                        }
                        tx_slot
                            .take()
                            .ok_or_else(|| {
                                RuntimeStoreError::Internal(
                                    "SQLite lifecycle fence consumed its transaction twice"
                                        .to_string(),
                                )
                            })?
                            .commit()
                            .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))
                    })?;
                match fence_outcome {
                    RuntimeStoreWriteFenceOutcome::Applied if already_exact => {
                        Ok(FencedMachineLifecycleCasOutcome::AlreadyExact { record, version })
                    }
                    RuntimeStoreWriteFenceOutcome::Applied => {
                        Ok(FencedMachineLifecycleCasOutcome::Applied { record, version })
                    }
                    RuntimeStoreWriteFenceOutcome::Conflict { reason } => {
                        Ok(FencedMachineLifecycleCasOutcome::FenceConflict { reason })
                    }
                    RuntimeStoreWriteFenceOutcome::Backoff { reason } => {
                        Ok(FencedMachineLifecycleCasOutcome::FenceBackoff { reason })
                    }
                }
            })
            .await
            .map_err(|err| RuntimeStoreError::Internal(format!("Task join failed: {err}")))?
        }

        async fn load_machine_lifecycle_record(
            &self,
            runtime_id: &LogicalRuntimeId,
        ) -> Result<Option<Vec<u8>>, RuntimeStoreError> {
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            tokio::task::spawn_blocking(move || {
                let conn = open_runtime_connection(&path)?;
                conn.query_row(
                    "SELECT runtime_state_json FROM runtime_states WHERE runtime_id = ?1",
                    params![runtime_id_text(&runtime_id)],
                    |row| Ok(row.get::<_, JsonColumnBytes>(0)?.into_bytes()),
                )
                .optional()
                .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))
            })
            .await
            .map_err(|err| RuntimeStoreError::Internal(format!("Task join failed: {err}")))?
        }

        async fn commit_machine_lifecycle(
            &self,
            runtime_id: &LogicalRuntimeId,
            commit: MachineLifecycleCommit,
            input_states: &[InputStatePersistenceRecord],
        ) -> Result<(), RuntimeStoreError> {
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            let snapshot = commit.into_snapshot();
            let input_states = input_states
                .iter()
                .map(|record| {
                    (
                        record.clone_stored(),
                        record.expected_row_digest().map(str::to_owned),
                    )
                })
                .collect::<Vec<_>>();
            tokio::task::spawn_blocking(move || {
                let mut conn = open_runtime_connection(&path)?;
                let tx = begin_runtime_transaction(&mut conn)?;
                upsert_machine_lifecycle_snapshot(&tx, &runtime_id, &snapshot)?;
                upsert_input_states(&tx, &runtime_id, &input_states)?;
                tx.commit()
                    .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                Ok(())
            })
            .await
            .map_err(|err| RuntimeStoreError::Internal(format!("Task join failed: {err}")))?
        }

        async fn commit_unregister_finalization(
            &self,
            runtime_id: &LogicalRuntimeId,
            finalization: crate::store::UnregisterFinalizationCommit,
        ) -> Result<(), RuntimeStoreError> {
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            let (snapshot, input_states, retired_ops_epoch) = finalization.into_parts();
            let input_states = input_states
                .into_iter()
                .map(|record| record.clone_stored())
                .collect::<Vec<_>>();
            #[cfg(test)]
            let fault = self.unregister_finalization_fault.swap(0, Ordering::SeqCst);
            #[cfg(not(test))]
            let fault = 0_u8;
            // Complete this rare finalization synchronously in the future's
            // first poll. A detached blocking task could outlive cancellation
            // and cross a same-runtime-ID replacement.
            {
                let mut conn = open_runtime_connection(&path)?;
                let final_lifecycle_record =
                    MachineLifecycleStoreRecord::from_snapshot(&snapshot).encode()?;
                let final_input_state_records = input_states
                    .iter()
                    .map(|state| {
                        serde_json::to_vec(state)
                            .map(|record| (state.state.input_id.0.to_string(), Some(record)))
                            .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let input_ids = final_input_state_records
                    .iter()
                    .map(|(input_id, _)| input_id.clone())
                    .collect::<Vec<_>>();
                let tx = begin_runtime_transaction(&mut conn)?;
                let before_observation = observe_unregister_finalization(
                    &tx,
                    &runtime_id,
                    &input_ids,
                    &retired_ops_epoch,
                )?;
                let final_ops_record = match before_observation.ops_record.as_ref() {
                    Some(bytes) => {
                        let persisted: crate::ops_lifecycle::PersistedOpsSnapshot =
                            serde_json::from_slice(bytes).map_err(|error| {
                                RuntimeStoreError::ReadFailed(format!(
                                    "failed to decode ops epoch before unregister finalization: {error}"
                                ))
                            })?;
                        if persisted.epoch_id == retired_ops_epoch {
                            None
                        } else {
                            Some(bytes.clone())
                        }
                    }
                    None => None,
                };
                let final_observation = UnregisterFinalizationObservation {
                    lifecycle_record: Some(final_lifecycle_record),
                    input_state_records: final_input_state_records,
                    ops_record: final_ops_record.clone(),
                    retired_ops_epoch_present: true,
                };
                upsert_machine_lifecycle_snapshot(&tx, &runtime_id, &snapshot)?;
                let unfenced_input_states = input_states
                    .iter()
                    .map(|state| (state.clone(), None))
                    .collect::<Vec<_>>();
                upsert_input_states(&tx, &runtime_id, &unfenced_input_states)?;
                if fault == 1 {
                    return Err(RuntimeStoreError::WriteFailed(
                        "synthetic power cut after unregister lifecycle write".to_string(),
                    ));
                }
                tx.execute(
                    r"
                    INSERT INTO runtime_retired_ops_epochs (runtime_id, epoch_id)
                    VALUES (?1, ?2)
                    ON CONFLICT(runtime_id, epoch_id) DO NOTHING
                    ",
                    params![runtime_id_text(&runtime_id), retired_ops_epoch.to_string()],
                )
                .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                if before_observation.ops_record.is_some() && final_ops_record.is_none() {
                    tx.execute(
                        "DELETE FROM runtime_ops_lifecycle WHERE runtime_id = ?1",
                        params![runtime_id_text(&runtime_id)],
                    )
                    .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                }
                if fault == 2 {
                    return Err(RuntimeStoreError::WriteFailed(
                        "synthetic power cut after unregister ops deletion".to_string(),
                    ));
                }
                let commit_error = match tx.commit() {
                    Ok(()) if fault != 3 => return Ok(()),
                    Ok(()) => "synthetic lost acknowledgement after unregister commit".to_string(),
                    Err(err) => err.to_string(),
                };

                // COMMIT acknowledgement can be uncertain on some I/O
                // failures. Reopen the database and classify exact durable
                // bytes before returning: final is success, exact pre-state
                // is a safe ordinary error, and no other state may trigger a
                // compensating lifecycle rollback.
                drop(conn);
                let observed = open_runtime_connection(&path)
                    .and_then(|conn| {
                        observe_unregister_finalization(
                            &conn,
                            &runtime_id,
                            &input_ids,
                            &retired_ops_epoch,
                        )
                    })
                    .map_err(|observation_error| {
                        RuntimeStoreError::UnregisterFinalizationOutcomeUnknown(format!(
                            "commit acknowledgement failed ({commit_error}); durable outcome read failed: {observation_error}"
                        ))
                    })?;
                if observed == final_observation {
                    return Ok(());
                }
                if observed == before_observation {
                    return Err(RuntimeStoreError::WriteFailed(commit_error));
                }
                Err(RuntimeStoreError::UnregisterFinalizationOutcomeUnknown(
                    format!(
                        "commit acknowledgement failed ({commit_error}); reopened lifecycle/input/ops bytes match neither final nor pre-transaction authority"
                    ),
                ))
            }
        }

        async fn persist_ops_lifecycle(
            &self,
            runtime_id: &LogicalRuntimeId,
            snapshot: &crate::ops_lifecycle::PersistedOpsSnapshot,
        ) -> Result<(), RuntimeStoreError> {
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            let snapshot = snapshot.clone();
            tokio::task::spawn_blocking(move || {
                let state_json = serde_json::to_vec(&snapshot)
                    .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                let mut conn = open_runtime_connection(&path)?;
                let tx = begin_runtime_transaction(&mut conn)?;
                let retired = tx
                    .query_row(
                        r"
                        SELECT 1
                        FROM runtime_retired_ops_epochs
                        WHERE runtime_id = ?1 AND epoch_id = ?2
                        ",
                        params![runtime_id_text(&runtime_id), snapshot.epoch_id.to_string()],
                        |_row| Ok(()),
                    )
                    .optional()
                    .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))?
                    .is_some();
                if retired {
                    return Err(RuntimeStoreError::OpsLifecycleEpochRetired {
                        runtime_id: runtime_id.0.clone(),
                        epoch_id: snapshot.epoch_id,
                    });
                }
                tx.execute(
                    r"
                    INSERT INTO runtime_ops_lifecycle (runtime_id, state_json)
                    VALUES (?1, ?2)
                    ON CONFLICT(runtime_id) DO UPDATE SET state_json = excluded.state_json
                    ",
                    params![runtime_id_text(&runtime_id), state_json],
                )
                .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                tx.commit()
                    .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                Ok(())
            })
            .await
            .map_err(|err| RuntimeStoreError::Internal(format!("Task join failed: {err}")))?
        }

        async fn initialize_ops_lifecycle_if_absent(
            &self,
            runtime_id: &LogicalRuntimeId,
            candidate: &crate::ops_lifecycle::PersistedOpsSnapshot,
        ) -> Result<crate::ops_lifecycle::PersistedOpsSnapshot, RuntimeStoreError> {
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            let candidate = candidate.clone();
            tokio::task::spawn_blocking(move || {
                let state_json = serde_json::to_vec(&candidate)
                    .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                let mut conn = open_runtime_connection(&path)?;
                let tx = begin_runtime_transaction(&mut conn)?;
                let retired = tx
                    .query_row(
                        r"
                        SELECT 1
                        FROM runtime_retired_ops_epochs
                        WHERE runtime_id = ?1 AND epoch_id = ?2
                        ",
                        params![runtime_id_text(&runtime_id), candidate.epoch_id.to_string()],
                        |_row| Ok(()),
                    )
                    .optional()
                    .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))?
                    .is_some();
                if retired {
                    return Err(RuntimeStoreError::OpsLifecycleEpochRetired {
                        runtime_id: runtime_id.0.clone(),
                        epoch_id: candidate.epoch_id,
                    });
                }
                tx.execute(
                    r"
                    INSERT INTO runtime_ops_lifecycle (runtime_id, state_json)
                    VALUES (?1, ?2)
                    ON CONFLICT(runtime_id) DO NOTHING
                    ",
                    params![runtime_id_text(&runtime_id), state_json],
                )
                .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                let canonical_json = tx
                    .query_row(
                        "SELECT state_json FROM runtime_ops_lifecycle WHERE runtime_id = ?1",
                        params![runtime_id_text(&runtime_id)],
                        |row| Ok(row.get::<_, JsonColumnBytes>(0)?.into_bytes()),
                    )
                    .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))?;
                let canonical: crate::ops_lifecycle::PersistedOpsSnapshot =
                    serde_json::from_slice(&canonical_json)
                        .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))?;
                let canonical_retired = tx
                    .query_row(
                        r"
                        SELECT 1
                        FROM runtime_retired_ops_epochs
                        WHERE runtime_id = ?1 AND epoch_id = ?2
                        ",
                        params![runtime_id_text(&runtime_id), canonical.epoch_id.to_string()],
                        |_row| Ok(()),
                    )
                    .optional()
                    .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))?
                    .is_some();
                if canonical_retired {
                    return Err(RuntimeStoreError::OpsLifecycleEpochRetired {
                        runtime_id: runtime_id.0,
                        epoch_id: canonical.epoch_id,
                    });
                }
                tx.commit()
                    .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                Ok(canonical)
            })
            .await
            .map_err(|err| RuntimeStoreError::Internal(format!("Task join failed: {err}")))?
        }

        async fn load_ops_lifecycle(
            &self,
            runtime_id: &LogicalRuntimeId,
        ) -> Result<Option<crate::ops_lifecycle::PersistedOpsSnapshot>, RuntimeStoreError> {
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            tokio::task::spawn_blocking(move || {
                let conn = open_runtime_connection(&path)?;
                conn.query_row(
                    "SELECT state_json FROM runtime_ops_lifecycle WHERE runtime_id = ?1",
                    params![runtime_id_text(&runtime_id)],
                    |row| Ok(row.get::<_, JsonColumnBytes>(0)?.into_bytes()),
                )
                .optional()
                .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))?
                .map(|bytes| {
                    serde_json::from_slice(&bytes)
                        .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))
                })
                .transpose()
            })
            .await
            .map_err(|err| RuntimeStoreError::Internal(format!("Task join failed: {err}")))?
        }

        async fn delete_ops_lifecycle(
            &self,
            runtime_id: &LogicalRuntimeId,
        ) -> Result<(), RuntimeStoreError> {
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            tokio::task::spawn_blocking(move || {
                let mut conn = open_runtime_connection(&path)?;
                let tx = begin_runtime_transaction(&mut conn)?;
                tx.execute(
                    "DELETE FROM runtime_ops_lifecycle WHERE runtime_id = ?1",
                    params![runtime_id_text(&runtime_id)],
                )
                .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                tx.commit()
                    .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                Ok(())
            })
            .await
            .map_err(|err| RuntimeStoreError::Internal(format!("Task join failed: {err}")))?
        }

        async fn load_mob_host_binding(
            &self,
            mob_id: &str,
        ) -> Result<Option<Vec<u8>>, RuntimeStoreError> {
            let path = self.path.clone();
            let mob_id = mob_id.to_string();
            tokio::task::spawn_blocking(move || {
                let conn = open_runtime_connection(&path)?;
                conn.query_row(
                    "SELECT record_json FROM runtime_mob_host_bindings WHERE mob_id = ?1",
                    params![mob_id],
                    |row| Ok(row.get::<_, JsonColumnBytes>(0)?.into_bytes()),
                )
                .optional()
                .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))
            })
            .await
            .map_err(|err| RuntimeStoreError::Internal(format!("Task join failed: {err}")))?
        }

        async fn list_mob_host_bindings(
            &self,
        ) -> Result<Vec<(String, Vec<u8>)>, RuntimeStoreError> {
            let path = self.path.clone();
            tokio::task::spawn_blocking(move || {
                let conn = open_runtime_connection(&path)?;
                let mut stmt = conn
                    .prepare(
                        "SELECT mob_id, record_json FROM runtime_mob_host_bindings ORDER BY mob_id",
                    )
                    .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))?;
                let rows = stmt
                    .query_map([], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, JsonColumnBytes>(1)?.into_bytes(),
                        ))
                    })
                    .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))?;
                rows.collect::<Result<Vec<_>, _>>()
                    .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))
            })
            .await
            .map_err(|err| RuntimeStoreError::Internal(format!("Task join failed: {err}")))?
        }

        async fn put_mob_host_binding_if_absent(
            &self,
            mob_id: &str,
            record_json: &[u8],
        ) -> Result<bool, RuntimeStoreError> {
            let path = self.path.clone();
            let mob_id = mob_id.to_string();
            let record_json = record_json.to_vec();
            tokio::task::spawn_blocking(move || {
                let mut conn = open_runtime_connection(&path)?;
                let tx = begin_runtime_transaction(&mut conn)?;
                let changed = tx
                    .execute(
                        "INSERT OR IGNORE INTO runtime_mob_host_bindings (mob_id, record_json) \
                         VALUES (?1, ?2)",
                        params![mob_id, record_json],
                    )
                    .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                if changed > 0 {
                    // A fresh replacement binding supersedes the prior
                    // controller's revoke receipt at the same atomic
                    // boundary. A delayed old RevokeHost can therefore
                    // never replay across the replacement ceremony.
                    tx.execute(
                        "DELETE FROM runtime_mob_host_revocations WHERE mob_id = ?1",
                        params![mob_id],
                    )
                    .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                }
                tx.commit()
                    .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                Ok(changed > 0)
            })
            .await
            .map_err(|err| RuntimeStoreError::Internal(format!("Task join failed: {err}")))?
        }

        async fn compare_and_put_mob_host_binding(
            &self,
            mob_id: &str,
            expected_json: &[u8],
            next_json: &[u8],
        ) -> Result<bool, RuntimeStoreError> {
            let path = self.path.clone();
            let mob_id = mob_id.to_string();
            let expected_json = expected_json.to_vec();
            let next_json = next_json.to_vec();
            tokio::task::spawn_blocking(move || {
                let mut conn = open_runtime_connection(&path)?;
                let tx = begin_runtime_transaction(&mut conn)?;
                let changed = tx
                    .execute(
                        "UPDATE runtime_mob_host_bindings \
                         SET record_json = ?2 \
                         WHERE mob_id = ?1 AND record_json = ?3",
                        params![mob_id, next_json, expected_json],
                    )
                    .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                tx.commit()
                    .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                Ok(changed > 0)
            })
            .await
            .map_err(|err| RuntimeStoreError::Internal(format!("Task join failed: {err}")))?
        }

        async fn delete_mob_host_binding(
            &self,
            mob_id: &str,
            expected_json: &[u8],
        ) -> Result<bool, RuntimeStoreError> {
            let path = self.path.clone();
            let mob_id = mob_id.to_string();
            let expected_json = expected_json.to_vec();
            tokio::task::spawn_blocking(move || {
                let mut conn = open_runtime_connection(&path)?;
                let tx = begin_runtime_transaction(&mut conn)?;
                let changed = tx
                    .execute(
                        "DELETE FROM runtime_mob_host_bindings \
                         WHERE mob_id = ?1 AND record_json = ?2",
                        params![mob_id, expected_json],
                    )
                    .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                tx.commit()
                    .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                Ok(changed > 0)
            })
            .await
            .map_err(|err| RuntimeStoreError::Internal(format!("Task join failed: {err}")))?
        }

        async fn load_mob_host_revocation(
            &self,
            mob_id: &str,
        ) -> Result<Option<Vec<u8>>, RuntimeStoreError> {
            let path = self.path.clone();
            let mob_id = mob_id.to_string();
            tokio::task::spawn_blocking(move || {
                let conn = open_runtime_connection(&path)?;
                conn.query_row(
                    "SELECT receipt_json FROM runtime_mob_host_revocations WHERE mob_id = ?1",
                    params![mob_id],
                    |row| Ok(row.get::<_, JsonColumnBytes>(0)?.into_bytes()),
                )
                .optional()
                .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))
            })
            .await
            .map_err(|err| RuntimeStoreError::Internal(format!("Task join failed: {err}")))?
        }

        async fn list_mob_host_revocations(
            &self,
        ) -> Result<Vec<(String, Vec<u8>)>, RuntimeStoreError> {
            let path = self.path.clone();
            tokio::task::spawn_blocking(move || {
                let conn = open_runtime_connection(&path)?;
                let mut stmt = conn
                    .prepare(
                        "SELECT mob_id, receipt_json FROM runtime_mob_host_revocations ORDER BY mob_id",
                    )
                    .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))?;
                let rows = stmt
                    .query_map([], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, JsonColumnBytes>(1)?.into_bytes(),
                        ))
                    })
                    .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))?;
                rows.collect::<Result<Vec<_>, _>>()
                    .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))
            })
            .await
            .map_err(|err| RuntimeStoreError::Internal(format!("Task join failed: {err}")))?
        }

        async fn revoke_mob_host_binding(
            &self,
            mob_id: &str,
            expected_binding_json: &[u8],
            receipt_json: &[u8],
        ) -> Result<bool, RuntimeStoreError> {
            let path = self.path.clone();
            let mob_id = mob_id.to_string();
            let expected_binding_json = expected_binding_json.to_vec();
            let receipt_json = receipt_json.to_vec();
            tokio::task::spawn_blocking(move || {
                let mut conn = open_runtime_connection(&path)?;
                let tx = begin_runtime_transaction(&mut conn)?;
                let changed = tx
                    .execute(
                        "DELETE FROM runtime_mob_host_bindings \
                         WHERE mob_id = ?1 AND record_json = ?2",
                        params![mob_id, expected_binding_json],
                    )
                    .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                if changed == 0 {
                    tx.rollback()
                        .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                    return Ok(false);
                }
                tx.execute(
                    "INSERT INTO runtime_mob_host_revocations (mob_id, receipt_json) \
                     VALUES (?1, ?2) \
                     ON CONFLICT(mob_id) DO UPDATE SET receipt_json = excluded.receipt_json",
                    params![mob_id, receipt_json],
                )
                .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                tx.commit()
                    .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                Ok(true)
            })
            .await
            .map_err(|err| RuntimeStoreError::Internal(format!("Task join failed: {err}")))?
        }
    }

    #[cfg(test)]
    #[allow(clippy::expect_used, clippy::unwrap_used)]
    mod tests {
        use tempfile::TempDir;

        use super::*;

        #[tokio::test]
        async fn pending_terminal_owner_index_satisfies_store_contract() {
            let tempdir = tempfile::TempDir::new().unwrap();
            let store = SqliteRuntimeStore::new(tempdir.path().join("runtime.sqlite3")).unwrap();
            crate::store::assert_pending_terminal_owner_index_contract(&store).await;
        }

        #[tokio::test]
        async fn recovery_input_revision_rejects_a_phantom_insert_after_empty_observation() {
            let tempdir = tempfile::TempDir::new().unwrap();
            let store = SqliteRuntimeStore::new(tempdir.path().join("runtime.sqlite3")).unwrap();
            let runtime_id = LogicalRuntimeId::new("recovery-input-set-race");
            let observed = store
                .load_input_states_with_versions(&runtime_id)
                .await
                .unwrap();
            assert!(observed.exact_set_token().starts_with("sha256:"));
            let observed_revision = observed.input_set_revision();
            let observed_token = observed.exact_set_token().to_string();

            store
                .persist_input_state(
                    &runtime_id,
                    &InputStatePersistenceRecord::from_machine_snapshot(
                        StoredInputState::new_accepted(InputId::new()),
                    )
                    .unwrap(),
                )
                .await
                .unwrap();

            let mut conn = open_runtime_connection(store.path()).unwrap();
            let tx = begin_runtime_transaction(&mut conn).unwrap();
            let error = enforce_recovery_input_set_authority(
                &tx,
                &runtime_id,
                observed_revision,
                &observed_token,
            )
            .unwrap_err();
            assert!(matches!(
                error,
                RuntimeStoreError::RecoveryInputSetConflict {
                    runtime_id: conflicted
                } if conflicted == runtime_id.to_string()
            ));
        }

        #[tokio::test]
        async fn recovery_input_authority_rejects_token_mismatch_at_same_revision() {
            let tempdir = tempfile::TempDir::new().unwrap();
            let store = SqliteRuntimeStore::new(tempdir.path().join("runtime.sqlite3")).unwrap();
            let runtime_id = LogicalRuntimeId::new("recovery-input-set-token-mismatch");
            let observed = store
                .load_input_states_with_versions(&runtime_id)
                .await
                .unwrap();

            let mut conn = open_runtime_connection(store.path()).unwrap();
            let tx = begin_runtime_transaction(&mut conn).unwrap();
            let error = enforce_recovery_input_set_authority(
                &tx,
                &runtime_id,
                observed.input_set_revision(),
                "sha256:wrong-input-set-token",
            )
            .unwrap_err();
            assert!(matches!(
                error,
                RuntimeStoreError::RecoveryInputSetConflict {
                    runtime_id: conflicted
                } if conflicted == runtime_id.to_string()
            ));
        }

        #[test]
        fn runtime_v2_migrates_published_v1_rows_to_current_schema() {
            let mut conn = Connection::open_in_memory().unwrap();
            let tx = conn.transaction().unwrap();
            migration_0001_runtime_schema(&tx).unwrap();
            let released_session = include_bytes!(
                "../../../meerkat-core/tests/fixtures/v0_8_10_ob3_recovery_migration_session.json"
            );
            let imported = meerkat_core::import_released_0810_session(released_session).unwrap();
            let runtime_id = LogicalRuntimeId::for_session(imported.receipt().session_id());
            let expected_session = imported.session().clone();
            let input_id = InputId::new();
            let (mut stored, _) =
                crate::store::pending_terminal_owner_fixture(input_id.clone(), false);
            stored.state.persisted_input = Some(crate::input::Input::Prompt(
                crate::input::PromptInput::new("pending directed payload", None),
            ));
            stored.seed.phase = crate::input_state::InputLifecycleState::Consumed;
            stored.seed.terminal_outcome = Some(crate::input_state::InputTerminalOutcome::Consumed);
            stored.seed.recovery_lane = None;
            let mut pending_json = serde_json::to_value(&stored).unwrap();
            pending_json["stored_input_state_version"] = serde_json::json!(4);
            let state_json = serde_json::to_vec(&pending_json).unwrap();
            tx.execute(
                r"
                INSERT INTO runtime_input_states (runtime_id, input_id, state_json)
                VALUES (?1, ?2, ?3)
                ",
                params![
                    runtime_id_text(&runtime_id),
                    input_id.0.to_string(),
                    state_json
                ],
            )
            .unwrap();
            let insert_released_context_row =
                |tx: &rusqlite::Transaction<'_>,
                 runtime_id: &LogicalRuntimeId,
                 input: crate::input::Input,
                 semantics: crate::ingress_types::RuntimeInputSemantics,
                 context_key: &str,
                 context_text: &str| {
                    let input_id = input.id().clone();
                    let mut stored = StoredInputState::new_accepted(input_id.clone());
                    stored.state.runtime_semantics = Some(semantics);
                    stored.state.persisted_input = Some(input);
                    let mut encoded = serde_json::to_value(&stored).unwrap();
                    encoded["stored_input_state_version"] = serde_json::json!(4);
                    encoded["persisted_input"]["context_append"] = serde_json::json!({
                        "key": context_key,
                        "content": {
                            "type": "text",
                            "text": context_text,
                        },
                    });
                    let bytes = serde_json::to_vec(&encoded).unwrap();
                    tx.execute(
                        "INSERT INTO runtime_input_states (runtime_id, input_id, state_json)
                         VALUES (?1, ?2, ?3)",
                        params![runtime_id_text(runtime_id), input_id.0.to_string(), bytes],
                    )
                    .unwrap();
                    input_id
                };
            let steer_continuation =
                crate::input::Input::Continuation(crate::input::ContinuationInput {
                    header: crate::input::InputHeader {
                        id: InputId::new(),
                        timestamp: chrono::Utc::now(),
                        source: crate::input::InputOrigin::System,
                        durability: crate::input::InputDurability::Durable,
                        visibility: crate::input::InputVisibility::default(),
                        idempotency_key: None,
                        supersession_key: None,
                        correlation_id: None,
                    },
                    reason: "released active steer".to_string(),
                    continuation_kind: crate::input::ContinuationKind::Ordinary,
                    handling_mode: meerkat_core::types::HandlingMode::Steer,
                    request_id: None,
                    turn_tool_overlay: None,
                    turn_append: None,
                });
            let steer_semantics = crate::ingress_types::RuntimeInputSemantics {
                boundary: RunApplyBoundary::RunCheckpoint,
                execution_kind: meerkat_core::lifecycle::RuntimeExecutionKind::ResumePending,
                execution_handling_mode: None,
                peer_response_terminal_apply_intent: None,
                live_interrupt_required: true,
            };
            let released_steer_id = insert_released_context_row(
                &tx,
                &runtime_id,
                steer_continuation,
                steer_semantics,
                "released:steer:one",
                "  exact steer bytes  ",
            );
            let instruction_continuation =
                crate::input::Input::Continuation(crate::input::ContinuationInput {
                    header: crate::input::InputHeader {
                        id: InputId::new(),
                        timestamp: chrono::Utc::now(),
                        source: crate::input::InputOrigin::System,
                        durability: crate::input::InputDurability::Durable,
                        visibility: crate::input::InputVisibility::default(),
                        idempotency_key: None,
                        supersession_key: None,
                        correlation_id: None,
                    },
                    reason: "released instruction continuation".to_string(),
                    continuation_kind: crate::input::ContinuationKind::WorkgraphAttention,
                    handling_mode: meerkat_core::types::HandlingMode::Queue,
                    request_id: None,
                    turn_tool_overlay: None,
                    turn_append: None,
                });
            let instruction_semantics = crate::ingress_types::RuntimeInputSemantics {
                boundary: RunApplyBoundary::RunStart,
                execution_kind: meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn,
                execution_handling_mode: None,
                peer_response_terminal_apply_intent: None,
                live_interrupt_required: false,
            };
            let released_instruction_id = insert_released_context_row(
                &tx,
                &runtime_id,
                instruction_continuation,
                instruction_semantics,
                "released:instruction:one",
                "  legacy instruction  ",
            );
            let terminal_input = crate::input::Input::Prompt(crate::input::PromptInput::new(
                "released terminal payload",
                None,
            ));
            let terminal_input_id = terminal_input.id().clone();
            let mut terminal = StoredInputState::new_accepted(terminal_input_id.clone());
            terminal.state.persisted_input = Some(terminal_input);
            terminal.seed.phase = crate::input_state::InputLifecycleState::Consumed;
            terminal.seed.terminal_outcome =
                Some(crate::input_state::InputTerminalOutcome::Consumed);
            terminal.seed.recovery_lane = None;
            let mut terminal_json = serde_json::to_value(&terminal).unwrap();
            terminal_json["stored_input_state_version"] = serde_json::json!(4);
            let terminal_state_json = serde_json::to_vec(&terminal_json).unwrap();
            tx.execute(
                r"
                INSERT INTO runtime_input_states (runtime_id, input_id, state_json)
                VALUES (?1, ?2, ?3)
                ",
                params![
                    runtime_id_text(&runtime_id),
                    terminal_input_id.0.to_string(),
                    terminal_state_json
                ],
            )
            .unwrap();
            tx.execute(
                r#"
                INSERT INTO runtime_input_states (runtime_id, input_id, state_json)
                VALUES (
                    'unindexable-runtime',
                    'duplicate-key-row',
                    '{"idempotency_key":"first-key","idempotency_key":"second-key"}'
                )
                "#,
                [],
            )
            .unwrap();
            tx.execute(
                r"
                INSERT INTO runtime_session_snapshots (runtime_id, session_snapshot)
                VALUES (?1, ?2)
                ",
                params![runtime_id_text(&runtime_id), released_session.as_slice()],
            )
            .unwrap();
            let released_receipt = RunBoundaryReceipt {
                run_id: RunId::new(),
                boundary: RunApplyBoundary::Immediate,
                contributing_input_ids: vec![input_id.clone()],
                conversation_digest: None,
                message_count: expected_session.messages().len(),
                sequence: 7,
            };
            let released_receipt_json = serde_json::to_vec(&released_receipt).unwrap();
            tx.execute(
                r"
                INSERT INTO runtime_boundary_receipts (
                    runtime_id, run_id, sequence, receipt_json
                ) VALUES (?1, ?2, ?3, ?4)
                ",
                params![
                    runtime_id_text(&runtime_id),
                    released_receipt.run_id.0.to_string(),
                    encode_receipt_sequence(released_receipt.sequence),
                    &released_receipt_json,
                ],
            )
            .unwrap();

            migration_0002_current_runtime_schema(&tx).unwrap();

            let (authority_session_id, store_revision, blob_sha256, body) = tx
                .query_row(
                    r"
                    SELECT authority.session_id, authority.store_revision,
                           authority.blob_sha256, body.session_snapshot
                    FROM runtime_whole_blob_authority AS authority
                    JOIN runtime_whole_blob_bodies AS body
                      ON body.blob_sha256 = authority.blob_sha256
                    WHERE authority.runtime_id = ?1
                    ",
                    params![runtime_id_text(&runtime_id)],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, JsonColumnBytes>(3)?.into_bytes(),
                        ))
                    },
                )
                .unwrap();
            assert_eq!(authority_session_id, expected_session.id().to_string());
            assert_eq!(store_revision, 1);
            assert_eq!(blob_sha256, whole_blob_body_sha256(&body));
            assert_eq!(body, expected_session.to_persisted_bytes().unwrap());
            let decoded = meerkat_core::Session::from_persisted_bytes(&body).unwrap();
            assert_eq!(decoded.id(), expected_session.id());
            assert_eq!(decoded.messages(), expected_session.messages());
            assert_eq!(
                tx.query_row(
                    r"
                    SELECT receipt_sha256
                    FROM runtime_released_0810_boundary_receipts
                    WHERE runtime_id = ?1 AND run_id = ?2 AND sequence = ?3
                    ",
                    params![
                        runtime_id_text(&runtime_id),
                        released_receipt.run_id.0.to_string(),
                        encode_receipt_sequence(released_receipt.sequence),
                    ],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
                input_row_version_digest(&released_receipt_json),
                "v1 -> v2 activation must bind exact released receipt bytes in its one-time allowlist"
            );

            let catalog = load_runtime_session_catalog_entry_in_txn(&tx, &runtime_id)
                .unwrap()
                .expect("migrated WholeBlob session is cataloged");
            assert_eq!(catalog.session_id(), expected_session.id());
            assert_eq!(
                catalog.persistence_profile(),
                RuntimeSessionPersistenceProfile::WholeBlobV1
            );
            assert_eq!(catalog.created_at(), expected_session.created_at());
            assert_eq!(catalog.updated_at(), expected_session.updated_at());
            assert_eq!(catalog.message_count(), expected_session.messages().len());
            assert_eq!(catalog.total_tokens(), expected_session.total_tokens());
            assert_eq!(catalog.runtime_state(), None);
            assert_eq!(
                list_runtime_session_catalog_entries_in_conn(
                    &tx,
                    meerkat_core::SessionFilter::default(),
                )
                .unwrap(),
                vec![catalog],
                "migrated WholeBlob state must be immediately listable"
            );

            assert_eq!(
                tx.query_row(
                    r"
                    SELECT owner_input_id
                    FROM runtime_pending_terminal_owners
                    WHERE runtime_id = ?1
                    ",
                    params![runtime_id_text(&runtime_id)],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
                input_id.0.to_string()
            );
            let migrated_pending = tx
                .query_row(
                    "SELECT state_json
                     FROM runtime_input_states
                     WHERE runtime_id = ?1 AND input_id = ?2",
                    params![runtime_id_text(&runtime_id), input_id.0.to_string()],
                    |row| row.get::<_, JsonColumnBytes>(0),
                )
                .map(JsonColumnBytes::into_bytes)
                .map(|bytes| deserialize_persisted_input_state(&bytes))
                .unwrap()
                .unwrap();
            assert!(
                migrated_pending.state.persisted_input.is_some(),
                "pending terminal outbox retains its exact retry payload during activation"
            );
            let migrated_terminal = tx
                .query_row(
                    "SELECT state_json
                     FROM runtime_input_states
                     WHERE runtime_id = ?1 AND input_id = ?2",
                    params![
                        runtime_id_text(&runtime_id),
                        terminal_input_id.0.to_string()
                    ],
                    |row| row.get::<_, JsonColumnBytes>(0),
                )
                .map(JsonColumnBytes::into_bytes)
                .map(|bytes| deserialize_persisted_input_state(&bytes))
                .unwrap()
                .unwrap();
            assert_eq!(
                migrated_terminal.seed.phase,
                crate::input_state::InputLifecycleState::Consumed
            );
            assert_eq!(
                migrated_terminal.seed.terminal_outcome,
                Some(crate::input_state::InputTerminalOutcome::Consumed)
            );
            assert!(
                migrated_terminal.state.persisted_input.is_none(),
                "exact v1 -> v2 activation retires closed terminal payload bytes"
            );
            for (input_id, expected_role, expected_text, expected_key) in [
                (
                    released_steer_id,
                    meerkat_core::lifecycle::run_primitive::ConversationAppendRole::User,
                    "  exact steer bytes  ",
                    None,
                ),
                (
                    released_instruction_id,
                    meerkat_core::lifecycle::run_primitive::ConversationAppendRole::System,
                    "[Runtime System Context]\nsource: released:instruction:one\n\nlegacy instruction",
                    Some("released:instruction:one"),
                ),
            ] {
                let (bytes, migrated) = tx
                    .query_row(
                        "SELECT state_json
                         FROM runtime_input_states
                         WHERE runtime_id = ?1 AND input_id = ?2",
                        params![runtime_id_text(&runtime_id), input_id.0.to_string()],
                        |row| row.get::<_, JsonColumnBytes>(0),
                    )
                    .map(JsonColumnBytes::into_bytes)
                    .map(|bytes| {
                        let migrated = deserialize_persisted_input_state(&bytes).unwrap();
                        (bytes, migrated)
                    })
                    .unwrap();
                assert!(
                    !String::from_utf8_lossy(&bytes).contains("context_append"),
                    "current row must not retain the released sidecar field"
                );
                let Some(crate::input::Input::Continuation(continuation)) =
                    migrated.state.persisted_input
                else {
                    panic!("migrated context row must retain its continuation input");
                };
                let append = continuation
                    .turn_append
                    .expect("released context must become an ordinary turn append");
                assert_eq!(append.role, expected_role);
                assert_eq!(append.content.render_text(), expected_text);
                assert_eq!(
                    append
                        .identity
                        .as_ref()
                        .and_then(|identity| identity.source.as_deref()),
                    expected_key
                );
                assert_eq!(
                    append
                        .identity
                        .as_ref()
                        .and_then(|identity| identity.idempotency_key.as_deref()),
                    expected_key
                );
            }
            assert_eq!(
                tx.query_row(
                    r"
                    SELECT revision
                    FROM runtime_input_set_revisions
                    WHERE runtime_id = ?1
                    ",
                    params![runtime_id_text(&runtime_id)],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
                1,
                "published v1 input rows must receive one exact initial set revision"
            );
            assert_eq!(
                tx.query_row(
                    r"
                    SELECT reason
                    FROM runtime_input_idempotency_unindexable_rows
                    WHERE runtime_id = 'unindexable-runtime'
                      AND input_id = 'duplicate-key-row'
                    ",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
                "idempotency_key appears more than once"
            );
            assert_eq!(
                tx.query_row(
                    "SELECT runtime_id FROM runtime_head_canonical_activation_queue",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
                runtime_id_text(&runtime_id)
            );
            assert!(
                !LOAD_HEAD_CANONICAL_ACTIVATION_CANDIDATES_SQL
                    .contains("runtime_session_snapshots"),
                "ordinary reopen must not rescan retained whole-BLOB history"
            );
        }

        use crate::identifiers::LogicalRuntimeId;
        use crate::runtime_state::RuntimeState;
        use crate::traits::RuntimeDriver as _;
        use meerkat_core::lifecycle::core_executor::BoundSessionCommit;
        use meerkat_core::lifecycle::run_primitive::RunApplyBoundary;
        use meerkat_core::lifecycle::{InputId, RunBoundaryReceipt, RunId};
        use meerkat_core::session_store::{
            IncrementalSessionStore as _, PreparedHeadCanonicalMutation, SessionStore as _,
        };
        use meerkat_core::types::{
            AssistantBlock, BlockAssistantMessage, Message, StopReason, UserMessage,
        };
        use meerkat_core::{Session, TranscriptRewriteReason, TranscriptRewriteSelection};
        use meerkat_store::SqliteSessionStore;

        fn temp_store() -> (TempDir, SqliteRuntimeStore) {
            let dir = TempDir::new().unwrap();
            let path = dir.path().join("sessions.sqlite3");
            let store = SqliteRuntimeStore::new(path).unwrap();
            (dir, store)
        }

        fn raw_fixture_row(path: &Path, table: &str, column: &str, runtime_id: &str) -> Vec<u8> {
            let conn = Connection::open(path).unwrap();
            conn.query_row(
                &format!("SELECT {column} FROM {table} WHERE runtime_id = ?1"),
                params![runtime_id],
                |row| Ok(row.get::<_, JsonColumnBytes>(0)?.into_bytes()),
            )
            .unwrap()
        }

        fn runtime_id() -> LogicalRuntimeId {
            LogicalRuntimeId("runtime-1".to_string())
        }

        fn lifecycle_commit(
            runtime_id: &LogicalRuntimeId,
            state: RuntimeState,
            fence_token: u64,
            runtime_generation: u64,
        ) -> MachineLifecycleCommit {
            MachineLifecycleCommit::new_with_binding(
                state,
                crate::store::MachineLifecycleBindingFacts::new(
                    Some(runtime_id.0.clone()),
                    Some(fence_token),
                    Some(runtime_generation),
                    Some(format!("epoch-{runtime_generation}")),
                ),
                crate::store::SupervisorAuthoritySnapshot::UnboundNoReceipt,
            )
        }

        fn input_state() -> InputStatePersistenceRecord {
            InputStatePersistenceRecord::from_machine_snapshot(StoredInputState::new_accepted(
                InputId::new(),
            ))
            .expect("accepted test input state seed must be machine-authorized")
        }

        fn persistable(bundle: StoredInputState) -> InputStatePersistenceRecord {
            InputStatePersistenceRecord::from_machine_snapshot(bundle)
                .expect("test input state seed must be machine-authorized")
        }

        fn replacement_records(
            expected: &[StoredInputState],
            recovery_count: u32,
        ) -> Vec<InputStatePersistenceRecord> {
            expected
                .iter()
                .cloned()
                .map(|mut row| {
                    row.state.recovery_count = recovery_count;
                    persistable(row)
                })
                .collect()
        }

        fn session_with_one_turn() -> Session {
            let mut session = Session::new();
            session.push(Message::User(UserMessage::text("hello".to_string())));
            session.push(Message::BlockAssistant(BlockAssistantMessage {
                blocks: vec![AssistantBlock::Text {
                    text: "verbose answer".to_string(),
                    meta: None,
                }],
                stop_reason: StopReason::EndTurn,
                identity: meerkat_core::types::TranscriptMessageIdentity::default(),
                created_at: meerkat_core::types::message_timestamp_now(),
            }));
            session
        }

        fn session_with_user(content: &str) -> Session {
            let mut session = Session::new();
            session.push(Message::User(UserMessage::text(content.to_string())));
            session
        }

        fn input_state_with_idempotency_key(input_id: InputId, key: &str) -> StoredInputState {
            let mut state = StoredInputState::new_accepted(input_id);
            state.state.idempotency_key = Some(IdempotencyKey::new(key));
            state
        }

        #[tokio::test]
        async fn sqlite_input_idempotency_mutations_use_complete_final_image() {
            let (_dir, store) = temp_store();
            crate::store::assert_input_idempotency_final_image_contract(&store).await;
        }

        #[tokio::test]
        async fn unindexable_input_row_refuses_idempotency_presence_absence_and_exact_recovery() {
            let (_dir, store) = temp_store();
            let runtime_id = LogicalRuntimeId::new("unindexable-input-row");
            let corrupt_input_id = "corrupt-input-row";
            let indexed = input_state_with_idempotency_key(InputId::new(), "visible-key");
            store
                .persist_input_state(&runtime_id, &persistable(indexed))
                .await
                .unwrap();
            let conn = Connection::open(store.path()).unwrap();
            conn.execute(
                r"
                INSERT INTO runtime_input_states (runtime_id, input_id, state_json)
                VALUES (?1, ?2, ?3)
                ",
                params![
                    runtime_id_text(&runtime_id),
                    corrupt_input_id,
                    b"{not-json".as_slice()
                ],
            )
            .unwrap();
            assert_eq!(
                conn.query_row(
                    r"
                    SELECT reason
                    FROM runtime_input_idempotency_unindexable_rows
                    WHERE runtime_id = ?1 AND input_id = ?2
                    ",
                    params![runtime_id_text(&runtime_id), corrupt_input_id],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
                "state_json is not valid JSON"
            );
            drop(conn);

            assert!(matches!(
                store
                    .load_input_state_by_idempotency_key(
                        &runtime_id,
                        &IdempotencyKey::new("possibly-hidden-key"),
                    )
                    .await,
                Err(RuntimeStoreError::InputIdempotencyIndexUncertain {
                    evidence_input_id,
                    ..
                }) if evidence_input_id == corrupt_input_id
            ));
            assert!(matches!(
                store
                    .load_input_state_by_idempotency_key(
                        &runtime_id,
                        &IdempotencyKey::new("visible-key"),
                    )
                    .await,
                Err(RuntimeStoreError::InputIdempotencyIndexUncertain {
                    evidence_input_id,
                    ..
                }) if evidence_input_id == corrupt_input_id
            ));
            assert!(matches!(
                store.load_input_states_with_versions(&runtime_id).await,
                Err(RuntimeStoreError::Unsupported(detail))
                    if detail.contains(corrupt_input_id)
            ));
            let recoverable = crate::store::load_input_states_for_recovery(&store, &runtime_id)
                .await
                .unwrap();
            assert_eq!(
                recoverable.len(),
                1,
                "ordinary compatibility recovery preserves and skips only the forensic corrupt row"
            );
            assert_eq!(
                recoverable[0]
                    .state
                    .idempotency_key
                    .as_ref()
                    .map(|key| key.0.as_str()),
                Some("visible-key"),
                "the independent decodable row remains recoverable"
            );
            let conn = Connection::open(store.path()).unwrap();
            assert_eq!(
                conn.query_row(
                    r"
                    SELECT state_json
                    FROM runtime_input_states
                    WHERE runtime_id = ?1 AND input_id = ?2
                    ",
                    params![runtime_id_text(&runtime_id), corrupt_input_id],
                    |row| Ok(row.get::<_, JsonColumnBytes>(0)?.into_bytes()),
                )
                .unwrap(),
                b"{not-json"
            );
        }

        #[tokio::test]
        async fn duplicate_idempotency_members_refuse_both_extracted_keys() {
            let (_dir, store) = temp_store();
            let runtime_id = LogicalRuntimeId::new("duplicate-idempotency-members");
            let duplicate_input_id = InputId::new();
            let stored = input_state_with_idempotency_key(duplicate_input_id.clone(), "first-key");
            let canonical_json = serde_json::to_string(&stored).unwrap();
            let duplicate_json = canonical_json.replacen(
                r#""idempotency_key":"first-key""#,
                r#""idempotency_key":"first-key","idempotency_key":"second-key""#,
                1,
            );
            assert_ne!(
                duplicate_json, canonical_json,
                "fixture must insert a duplicate top-level idempotency member"
            );
            let conn = Connection::open(store.path()).unwrap();
            conn.execute(
                r"
                INSERT INTO runtime_input_states (runtime_id, input_id, state_json)
                VALUES (?1, ?2, ?3)
                ",
                params![
                    runtime_id_text(&runtime_id),
                    duplicate_input_id.to_string(),
                    duplicate_json.as_bytes(),
                ],
            )
            .unwrap();
            assert_eq!(
                conn.query_row(
                    r"
                    SELECT reason
                    FROM runtime_input_idempotency_unindexable_rows
                    WHERE runtime_id = ?1 AND input_id = ?2
                    ",
                    params![runtime_id_text(&runtime_id), duplicate_input_id.to_string()],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
                "idempotency_key appears more than once"
            );
            drop(conn);

            for key in ["first-key", "second-key"] {
                assert!(matches!(
                    store
                        .load_input_state_by_idempotency_key(
                            &runtime_id,
                            &IdempotencyKey::new(key),
                        )
                        .await,
                    Err(RuntimeStoreError::InputIdempotencyIndexUncertain {
                        evidence_input_id,
                        ..
                    }) if evidence_input_id == duplicate_input_id.to_string()
                ));
            }
        }

        #[tokio::test]
        async fn corrupt_indexed_hits_are_typed_non_authoritative_evidence() {
            let (_dir, store) = temp_store();
            let conn = Connection::open(store.path()).unwrap();
            let mut cases = Vec::new();

            let dangling_runtime = LogicalRuntimeId::new("dangling-index-owner");
            let dangling_input_id = InputId::new().to_string();
            conn.execute(
                r"
                INSERT INTO runtime_input_idempotency_keys
                    (runtime_id, idempotency_key, input_id)
                VALUES (?1, 'dangling-key', ?2)
                ",
                params![runtime_id_text(&dangling_runtime), &dangling_input_id],
            )
            .unwrap();
            cases.push((
                dangling_runtime,
                IdempotencyKey::new("dangling-key"),
                dangling_input_id,
                "missing source input row",
            ));

            let invalid_sentinel_input_runtime =
                LogicalRuntimeId::new("invalid-sentinel-input-storage");
            let invalid_sentinel_input_bytes = b"blob-sentinel-owner";
            conn.execute(
                r"
                INSERT INTO runtime_input_idempotency_unindexable_rows
                    (runtime_id, input_id, reason)
                VALUES (?1, ?2, 'forged evidence')
                ",
                params![
                    runtime_id_text(&invalid_sentinel_input_runtime),
                    invalid_sentinel_input_bytes.as_slice()
                ],
            )
            .unwrap();
            cases.push((
                invalid_sentinel_input_runtime,
                IdempotencyKey::new("sentinel-probe"),
                format!(
                    "<BLOB {}>",
                    input_row_version_digest(invalid_sentinel_input_bytes.as_slice())
                ),
                "evidence input_id has invalid SQLite representation BLOB",
            ));

            let invalid_sentinel_reason_runtime =
                LogicalRuntimeId::new("invalid-sentinel-reason-storage");
            let invalid_sentinel_reason_input_id = InputId::new().to_string();
            conn.execute(
                r"
                INSERT INTO runtime_input_idempotency_unindexable_rows
                    (runtime_id, input_id, reason)
                VALUES (?1, ?2, ?3)
                ",
                params![
                    runtime_id_text(&invalid_sentinel_reason_runtime),
                    &invalid_sentinel_reason_input_id,
                    b"blob-sentinel-reason".as_slice()
                ],
            )
            .unwrap();
            cases.push((
                invalid_sentinel_reason_runtime,
                IdempotencyKey::new("sentinel-probe"),
                invalid_sentinel_reason_input_id,
                "evidence reason has invalid SQLite representation BLOB",
            ));

            let invalid_owner_runtime = LogicalRuntimeId::new("invalid-index-owner-storage");
            let invalid_owner_bytes = b"blob-index-owner";
            conn.execute(
                r"
                INSERT INTO runtime_input_idempotency_keys
                    (runtime_id, idempotency_key, input_id)
                VALUES (?1, 'invalid-owner-key', ?2)
                ",
                params![
                    runtime_id_text(&invalid_owner_runtime),
                    invalid_owner_bytes.as_slice()
                ],
            )
            .unwrap();
            cases.push((
                invalid_owner_runtime,
                IdempotencyKey::new("invalid-owner-key"),
                format!(
                    "<BLOB {}>",
                    input_row_version_digest(invalid_owner_bytes.as_slice())
                ),
                "invalid SQLite representation BLOB",
            ));

            let invalid_storage_runtime = LogicalRuntimeId::new("invalid-indexed-storage");
            let invalid_storage_input_id = InputId::new().to_string();
            conn.execute(
                r"
                INSERT INTO runtime_input_states (runtime_id, input_id, state_json)
                VALUES (?1, ?2, 42)
                ",
                params![
                    runtime_id_text(&invalid_storage_runtime),
                    &invalid_storage_input_id
                ],
            )
            .unwrap();
            conn.execute(
                r"
                INSERT INTO runtime_input_idempotency_keys
                    (runtime_id, idempotency_key, input_id)
                VALUES (?1, 'invalid-storage-key', ?2)
                ",
                params![
                    runtime_id_text(&invalid_storage_runtime),
                    &invalid_storage_input_id
                ],
            )
            .unwrap();
            cases.push((
                invalid_storage_runtime,
                IdempotencyKey::new("invalid-storage-key"),
                invalid_storage_input_id,
                "storage class INTEGER",
            ));

            let invalid_shape_runtime = LogicalRuntimeId::new("invalid-indexed-state-shape");
            let invalid_shape_input_id = InputId::new();
            let mut invalid_shape_json = serde_json::to_value(input_state_with_idempotency_key(
                invalid_shape_input_id.clone(),
                "invalid-shape-key",
            ))
            .unwrap();
            invalid_shape_json
                .as_object_mut()
                .unwrap()
                .remove("stored_input_state_version");
            conn.execute(
                r"
                INSERT INTO runtime_input_states (runtime_id, input_id, state_json)
                VALUES (?1, ?2, ?3)
                ",
                params![
                    runtime_id_text(&invalid_shape_runtime),
                    invalid_shape_input_id.to_string(),
                    serde_json::to_vec(&invalid_shape_json).unwrap(),
                ],
            )
            .unwrap();
            cases.push((
                invalid_shape_runtime,
                IdempotencyKey::new("invalid-shape-key"),
                invalid_shape_input_id.to_string(),
                "not a valid stored state",
            ));

            let invalid_seed_runtime = LogicalRuntimeId::new("invalid-indexed-machine-seed");
            let invalid_seed_input_id = InputId::new();
            let mut invalid_seed =
                input_state_with_idempotency_key(invalid_seed_input_id.clone(), "invalid-seed-key");
            invalid_seed.seed.terminal_outcome =
                Some(crate::input_state::InputTerminalOutcome::Consumed);
            conn.execute(
                r"
                INSERT INTO runtime_input_states (runtime_id, input_id, state_json)
                VALUES (?1, ?2, ?3)
                ",
                params![
                    runtime_id_text(&invalid_seed_runtime),
                    invalid_seed_input_id.to_string(),
                    serde_json::to_vec(&invalid_seed).unwrap(),
                ],
            )
            .unwrap();
            cases.push((
                invalid_seed_runtime,
                IdempotencyKey::new("invalid-seed-key"),
                invalid_seed_input_id.to_string(),
                "non-authoritative machine seed",
            ));

            let mismatched_identity_runtime =
                LogicalRuntimeId::new("mismatched-indexed-input-identity");
            let indexed_input_id = InputId::new();
            let encoded_input_id = InputId::new();
            let mismatched_identity_json = serde_json::to_vec(&input_state_with_idempotency_key(
                encoded_input_id,
                "mismatched-identity-key",
            ))
            .unwrap();
            conn.execute(
                r"
                INSERT INTO runtime_input_states (runtime_id, input_id, state_json)
                VALUES (?1, ?2, ?3)
                ",
                params![
                    runtime_id_text(&mismatched_identity_runtime),
                    indexed_input_id.to_string(),
                    mismatched_identity_json,
                ],
            )
            .unwrap();
            cases.push((
                mismatched_identity_runtime,
                IdempotencyKey::new("mismatched-identity-key"),
                indexed_input_id.to_string(),
                "differs from decoded source identity/key",
            ));

            let mismatched_key_runtime = LogicalRuntimeId::new("mismatched-indexed-key");
            let mismatched_key_input_id = InputId::new();
            let mismatched_key_json = serde_json::to_vec(&input_state_with_idempotency_key(
                mismatched_key_input_id.clone(),
                "encoded-key",
            ))
            .unwrap();
            conn.execute(
                r"
                INSERT INTO runtime_input_states (runtime_id, input_id, state_json)
                VALUES (?1, ?2, ?3)
                ",
                params![
                    runtime_id_text(&mismatched_key_runtime),
                    mismatched_key_input_id.to_string(),
                    mismatched_key_json,
                ],
            )
            .unwrap();
            conn.execute(
                r"
                UPDATE runtime_input_idempotency_keys
                SET idempotency_key = 'forged-index-key'
                WHERE runtime_id = ?1 AND input_id = ?2
                ",
                params![
                    runtime_id_text(&mismatched_key_runtime),
                    mismatched_key_input_id.to_string()
                ],
            )
            .unwrap();
            cases.push((
                mismatched_key_runtime,
                IdempotencyKey::new("forged-index-key"),
                mismatched_key_input_id.to_string(),
                "differs from decoded source identity/key",
            ));
            drop(conn);

            for (runtime_id, key, expected_input_id, expected_reason) in cases {
                match store
                    .load_input_state_by_idempotency_key(&runtime_id, &key)
                    .await
                {
                    Err(RuntimeStoreError::InputIdempotencyIndexUncertain {
                        evidence_input_id,
                        reason,
                        ..
                    }) => {
                        assert_eq!(evidence_input_id, expected_input_id);
                        assert!(
                            reason.contains(expected_reason),
                            "unexpected typed corruption reason for {runtime_id}/{key}: {reason}"
                        );
                    }
                    other => panic!(
                        "corrupt indexed hit for {runtime_id}/{key} must be typed uncertainty, got \
                         {other:?}"
                    ),
                }
            }
        }

        #[test]
        fn activation_refuses_noncurrent_authority_without_candidate_repair() {
            let (_dir, store) = temp_store();
            let session = session_with_user("noncurrent authority");
            let session_id = session.id().clone();
            let runtime_id = LogicalRuntimeId::for_session(&session_id);
            let mut boundary_head = meerkat_core::session_store::SessionHead::from_session(
                &session,
                meerkat_core::session_store::TranscriptStrandId::root(),
                0,
            )
            .unwrap();
            boundary_head.message_row_prefix = None;
            let boundary_head_token = meerkat_core::session_head_cas_token(&boundary_head).unwrap();
            let boundary_head_json = serde_json::to_vec(&boundary_head).unwrap();
            let session_snapshot = serde_json::to_vec(&session).unwrap();
            let catalog_entry = crate::store::RuntimeSessionCatalogEntry::from_session(
                &session,
                RuntimeSessionPersistenceProfile::WholeBlobV1,
                None,
            )
            .unwrap();

            let mut conn = open_runtime_connection(store.path()).unwrap();
            let tx = begin_runtime_transaction(&mut conn).unwrap();
            upsert_runtime_snapshot_issued(
                &tx,
                &runtime_id,
                &session_snapshot,
                &session_id,
                &format!(
                    "row-sha256:{:x}",
                    <sha2::Sha256 as sha2::Digest>::digest(&session_snapshot)
                ),
                &catalog_entry,
            )
            .unwrap();
            tx.execute(
                r"
                INSERT INTO runtime_session_authority (
                    runtime_id, authority_version, session_id, store_revision,
                    boundary_head_json, committed_head_token
                ) VALUES (?1, 1, ?2, 1, ?3, ?4)
                ",
                params![
                    runtime_id_text(&runtime_id),
                    session_id.to_string(),
                    boundary_head_json,
                    boundary_head_token,
                ],
            )
            .unwrap();
            tx.execute(
                r"
                INSERT INTO runtime_head_canonical_activations (
                    runtime_id, activation_version, state, session_id,
                    started_at_ms, updated_at_ms
                ) VALUES (?1, 1, 'in_progress', ?2, 1, 1)
                ",
                params![runtime_id_text(&runtime_id), session_id.to_string()],
            )
            .unwrap();
            tx.commit().unwrap();

            assert!(matches!(
                activate_head_canonical_profiles(&mut conn),
                Err(RuntimeStoreError::SessionPersistenceAuthorityConflict { detail, .. })
                    if detail.contains("has no exact message-row prefix authority")
            ));
        }

        #[tokio::test]
        async fn marker_first_activation_resumes_current_shape() {
            let dir = TempDir::new().unwrap();
            let path = dir.path().join("marker-first-activation.sqlite3");
            let whole_blob = SqliteRuntimeStore::new_whole_blob(&path).unwrap();
            let physical_store = SqliteSessionStore::open(path.clone()).unwrap();
            let session = session_with_user("marker-first activation");
            let session_id = session.id().clone();
            let runtime_id = LogicalRuntimeId::for_session(&session_id);
            physical_store.save(&session).await.unwrap();
            whole_blob
                .commit_session_snapshot(
                    &runtime_id,
                    SerializedSessionSnapshot {
                        session_snapshot: serde_json::to_vec(&session).unwrap().into(),
                    },
                )
                .await
                .unwrap();
            drop(physical_store);
            drop(whole_blob);

            let mut conn = open_runtime_connection(&path).unwrap();
            let tx = begin_runtime_transaction(&mut conn).unwrap();
            tx.execute(
                r"
                INSERT INTO runtime_head_canonical_activations (
                    runtime_id, activation_version, state, session_id,
                    started_at_ms, updated_at_ms
                ) VALUES (?1, 1, 'in_progress', ?2, 1, 1)
                ",
                params![runtime_id_text(&runtime_id), session_id.to_string()],
            )
            .unwrap();
            tx.commit().unwrap();
            drop(conn);

            let activated = SqliteRuntimeStore::new_head_canonical(&path).unwrap();
            let conn = Connection::open(activated.path()).unwrap();
            assert_eq!(
                conn.query_row(
                    r"
                    SELECT state
                    FROM runtime_head_canonical_activations
                    WHERE runtime_id = ?1
                    ",
                    params![runtime_id_text(&runtime_id)],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
                "complete"
            );
            assert!(
                load_head_canonical_authority(&conn, &runtime_id)
                    .unwrap()
                    .is_some(),
                "marker-first retry must install the exact current authority"
            );
        }

        #[test]
        fn completed_activation_receipt_is_not_a_current_authority_candidate() {
            let (_dir, store) = temp_store();
            let session_id = Session::new().id().clone();
            let runtime_id = LogicalRuntimeId::for_session(&session_id);
            let mut conn = open_runtime_connection(store.path()).unwrap();
            let tx = begin_runtime_transaction(&mut conn).unwrap();
            tx.execute(
                r"
                INSERT INTO runtime_session_snapshots (runtime_id, session_snapshot)
                VALUES (?1, X'7B7D')
                ",
                params![runtime_id_text(&runtime_id)],
            )
            .unwrap();
            tx.execute(
                r"
                INSERT INTO runtime_session_authority (
                    runtime_id, authority_version, session_id, store_revision,
                    boundary_head_json, committed_head_token
                ) VALUES (?1, 1, ?2, 1, X'7B7D', 'current-authority-token')
                ",
                params![runtime_id_text(&runtime_id), session_id.to_string()],
            )
            .unwrap();
            tx.execute(
                r"
                INSERT INTO runtime_head_canonical_activations (
                    runtime_id, activation_version, state, session_id,
                    source_snapshot_token, source_snapshot_bytes,
                    source_message_count, started_at_ms, updated_at_ms,
                    completed_at_ms, elapsed_ms, boundary_message_count,
                    physical_message_count, boundary_head_cas_token,
                    physical_head_cas_token
                ) VALUES (
                    ?1, 1, 'complete', ?2, 'source-token', 2, 1, 1, 2,
                    2, 1, 1, 1, 'historical-boundary-token',
                    'historical-physical-token'
                )
                ",
                params![runtime_id_text(&runtime_id), session_id.to_string()],
            )
            .unwrap();
            tx.commit().unwrap();

            assert_eq!(
                conn.query_row(
                    "SELECT COUNT(*) FROM runtime_head_canonical_activation_queue",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
                0,
                "installing current authority must retire the one-time activation work item"
            );
            assert!(
                head_canonical_activation_candidate_ids(&conn)
                    .unwrap()
                    .is_empty(),
                "completed migration receipts and current authorities are historical, not startup work"
            );
            activate_head_canonical_profiles(&mut conn).unwrap();
        }

        #[tokio::test]
        async fn ordinary_append_after_activation_reopens_past_historical_receipt() {
            let dir = TempDir::new().unwrap();
            let path = dir.path().join("activation-then-append.sqlite3");
            let whole_blob = SqliteRuntimeStore::new_whole_blob(&path).unwrap();
            let physical_store = SqliteSessionStore::open(path.clone()).unwrap();
            let session = session_with_user("legacy boundary");
            let session_id = session.id().clone();
            let runtime_id = LogicalRuntimeId::for_session(&session_id);
            physical_store.save(&session).await.unwrap();
            whole_blob
                .commit_session_snapshot(
                    &runtime_id,
                    SerializedSessionSnapshot {
                        session_snapshot: serde_json::to_vec(&session).unwrap().into(),
                    },
                )
                .await
                .unwrap();
            drop(physical_store);
            drop(whole_blob);

            let activated = SqliteRuntimeStore::new_head_canonical(&path).unwrap();
            let conn = Connection::open(&path).unwrap();
            let activated_authority = load_head_canonical_authority(&conn, &runtime_id)
                .unwrap()
                .expect("activation authority");
            let activated_authority = activated_authority
                .head_canonical()
                .expect("HeadCanonical activation authority");
            let activated_store_revision = activated_authority.store_revision();
            drop(conn);

            let physical_store = SqliteSessionStore::open(path.clone()).unwrap();
            let observed_head = physical_store
                .load_head(&session_id)
                .await
                .unwrap()
                .expect("activated physical head");
            let mut resumed_session = physical_store
                .load(&session_id)
                .await
                .unwrap()
                .expect("activated physical session");
            resumed_session.push(Message::User(UserMessage::text("ordinary append")));
            let mutation =
                PreparedHeadCanonicalMutation::prepare(&resumed_session, Some(observed_head))
                    .unwrap();
            let appended_head = mutation.successor_head().clone();
            let appended_head_token = mutation.successor_head_token().to_string();
            let boundary =
                BoundSessionCommit::head_canonical_from_session(&resumed_session, mutation)
                    .unwrap();
            activated
                .commit_prepared_session_boundary(
                    &runtime_id,
                    PreparedRuntimeSessionCommit::snapshot_only(boundary),
                )
                .await
                .unwrap();
            drop(physical_store);
            drop(activated);

            let reopened = SqliteRuntimeStore::new_head_canonical(&path)
                .expect("historical activation receipt must not block reopen");
            let conn = Connection::open(reopened.path()).unwrap();
            let current = load_head_canonical_authority(&conn, &runtime_id)
                .unwrap()
                .expect("current runtime authority");
            let current = current
                .head_canonical()
                .expect("current HeadCanonical authority");
            assert_eq!(
                current.store_revision(),
                activated_store_revision + 1,
                "ordinary append must issue exactly one successor store revision"
            );
            assert_eq!(
                current.boundary_head(),
                &appended_head,
                "reopen must preserve the ordinary append head, not revalidate the historical activation boundary"
            );
            assert_eq!(
                current.committed_head_token(),
                appended_head_token,
                "reopen must preserve the store-issued append token"
            );
            assert!(
                head_canonical_activation_candidate_ids(&conn)
                    .unwrap()
                    .is_empty(),
                "completed activation plus valid append must leave no startup work"
            );
        }

        #[tokio::test]
        async fn head_canonical_provisional_chain_advances_each_physical_checkpoint_then_commits() {
            let dir = TempDir::new().unwrap();
            let path = dir.path().join("head-authority-revisions.sqlite3");
            let runtime_store = SqliteRuntimeStore::new_head_canonical(&path).unwrap();
            let physical_store = SqliteSessionStore::open(path.clone()).unwrap();
            let session = session_with_user("committed base");
            let session_id = session.id().clone();
            let runtime_id = LogicalRuntimeId::for_session(&session_id);

            let root = PreparedHeadCanonicalMutation::prepare_root(&session).unwrap();
            physical_store
                .apply_prepared_head_canonical_mutation(&root)
                .await
                .unwrap();
            let committed = {
                let mut conn = open_head_canonical_runtime_connection(&path).unwrap();
                let tx = begin_runtime_transaction(&mut conn).unwrap();
                let issued = issue_head_canonical_authority_in_txn(
                    &tx,
                    &runtime_id,
                    root.successor_head().clone(),
                )
                .unwrap();
                write_head_canonical_authority_in_txn(&tx, &runtime_id, &issued).unwrap();
                tx.commit().unwrap();
                issued
                    .head_canonical()
                    .expect("root HeadCanonical authority")
                    .clone()
            };

            let observed_head = physical_store
                .load_head(&session_id)
                .await
                .unwrap()
                .expect("root physical head");
            let mut resumed = physical_store
                .load(&session_id)
                .await
                .unwrap()
                .expect("root physical session");
            resumed.push(Message::User(UserMessage::text("provisional tail")));
            let tail =
                PreparedHeadCanonicalMutation::prepare(&resumed, Some(observed_head)).unwrap();
            let run_id = RunId::new();
            let prepared = PreparedHeadCanonicalProvisionalTail::prepare(
                committed.clone(),
                run_id.clone(),
                tail.successor_head(),
                tail.successor_head_token(),
                &resumed,
            )
            .unwrap();
            let provisional = runtime_store
                .write_prepared_head_canonical_provisional_tail(&runtime_id, prepared)
                .await
                .unwrap();
            assert_eq!(
                provisional.base_store_revision(),
                committed.store_revision()
            );
            assert_eq!(
                provisional.physical_store_revision(),
                committed.store_revision() + 1
            );
            assert_eq!(provisional.candidate_sequence(), 1);
            assert_eq!(
                runtime_store
                    .load_head_canonical_provisional_tail(&runtime_id)
                    .await
                    .unwrap()
                    .as_ref(),
                Some(&provisional)
            );

            physical_store
                .apply_prepared_head_canonical_mutation(&tail)
                .await
                .unwrap();
            let observed_head = physical_store
                .load_head(&session_id)
                .await
                .unwrap()
                .expect("first provisional physical head");
            let mut resumed = physical_store
                .load(&session_id)
                .await
                .unwrap()
                .expect("first provisional physical session");
            resumed.push(Message::User(UserMessage::text("second provisional tail")));
            let second_tail =
                PreparedHeadCanonicalMutation::prepare(&resumed, Some(observed_head)).unwrap();
            let second_candidate = resumed.clone();
            let second_prepared = PreparedHeadCanonicalProvisionalTail::prepare(
                committed.clone(),
                run_id.clone(),
                second_tail.successor_head(),
                second_tail.successor_head_token(),
                &resumed,
            )
            .unwrap();
            let second_provisional = runtime_store
                .write_prepared_head_canonical_provisional_tail(&runtime_id, second_prepared)
                .await
                .unwrap();
            assert_eq!(
                second_provisional.physical_store_revision(),
                provisional.physical_store_revision() + 1
            );
            assert_eq!(second_provisional.candidate_sequence(), 2);
            assert_eq!(
                runtime_store
                    .write_prepared_head_canonical_provisional_tail(
                        &runtime_id,
                        PreparedHeadCanonicalProvisionalTail::prepare(
                            committed.clone(),
                            run_id.clone(),
                            second_tail.successor_head(),
                            second_tail.successor_head_token(),
                            &resumed,
                        )
                        .unwrap(),
                    )
                    .await
                    .unwrap(),
                second_provisional,
                "same-run same-head retry must return the exact latest provisional authority"
            );
            physical_store
                .apply_prepared_head_canonical_mutation(&second_tail)
                .await
                .unwrap();
            let observed_head = physical_store
                .load_head(&session_id)
                .await
                .unwrap()
                .expect("second provisional physical head");
            let mut resumed = physical_store
                .load(&session_id)
                .await
                .unwrap()
                .expect("second provisional physical session");
            resumed.push(Message::User(UserMessage::text("incomplete third intent")));
            let incomplete_tail =
                PreparedHeadCanonicalMutation::prepare(&resumed, Some(observed_head)).unwrap();
            let incomplete = runtime_store
                .write_prepared_head_canonical_provisional_tail(
                    &runtime_id,
                    PreparedHeadCanonicalProvisionalTail::prepare(
                        committed.clone(),
                        run_id.clone(),
                        incomplete_tail.successor_head(),
                        incomplete_tail.successor_head_token(),
                        &resumed,
                    )
                    .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                incomplete.physical_store_revision(),
                second_provisional.physical_store_revision() + 1
            );
            assert_eq!(incomplete.candidate_sequence(), 3);
            assert!(
                runtime_store
                    .discard_head_canonical_provisional_tail(&runtime_id, &incomplete)
                    .await
                    .unwrap(),
                "discarding an incomplete later intent must roll back one provisional revision"
            );
            assert_eq!(
                runtime_store
                    .load_head_canonical_provisional_tail(&runtime_id)
                    .await
                    .unwrap()
                    .as_ref(),
                Some(&second_provisional),
                "incomplete later intent rollback must retain the last applied physical authority"
            );
            let receipt = RunBoundaryReceipt {
                run_id: run_id.clone(),
                boundary: RunApplyBoundary::Immediate,
                contributing_input_ids: Vec::new(),
                conversation_digest: Some(second_candidate.transcript_content_digest().unwrap()),
                message_count: second_candidate.messages().len(),
                sequence: 1,
            };
            let checkpoint = meerkat_core::RunCheckpointReceipt::issued(
                meerkat_core::RunCheckpointAuthority::HeadCanonical(second_provisional.clone()),
                second_candidate.transcript_content_digest().unwrap(),
                second_candidate.messages().len() as u64,
            )
            .unwrap();
            let mut divergent_receipt = receipt.clone();
            divergent_receipt.conversation_digest = Some("sha256:divergent".to_string());
            assert!(
                PreparedRuntimeSessionCommit::promote_head_canonical_success(
                    crate::store::PreparedHeadCanonicalProvisionalPromotion::prepare(
                        checkpoint.clone(),
                        &run_id,
                    )
                    .unwrap(),
                    divergent_receipt,
                    Vec::new(),
                    session_id.clone(),
                )
                .is_err(),
                "HeadCanonical promotion preparation must bind the terminal receipt to the exact checkpoint digest/count"
            );
            let promotion = crate::store::PreparedHeadCanonicalProvisionalPromotion::prepare(
                checkpoint, &run_id,
            )
            .unwrap();
            let successor = runtime_store
                .commit_prepared_session_boundary(
                    &runtime_id,
                    PreparedRuntimeSessionCommit::promote_head_canonical_success(
                        promotion,
                        receipt.clone(),
                        Vec::new(),
                        session_id.clone(),
                    )
                    .unwrap(),
                )
                .await
                .unwrap();
            let successor = successor
                .authority()
                .and_then(RuntimeSessionAuthority::head_canonical)
                .expect("successor HeadCanonical authority")
                .clone();
            assert_eq!(
                successor.store_revision(),
                second_provisional.physical_store_revision() + 1
            );
            assert_eq!(
                successor.committed_head_token(),
                second_provisional.physical_head_token()
            );
            let retry_checkpoint = meerkat_core::RunCheckpointReceipt::issued(
                meerkat_core::RunCheckpointAuthority::HeadCanonical(second_provisional.clone()),
                second_candidate.transcript_content_digest().unwrap(),
                second_candidate.messages().len() as u64,
            )
            .unwrap();
            let retry = runtime_store
                .commit_prepared_session_boundary(
                    &runtime_id,
                    PreparedRuntimeSessionCommit::promote_head_canonical_success(
                        crate::store::PreparedHeadCanonicalProvisionalPromotion::prepare(
                            retry_checkpoint,
                            &run_id,
                        )
                        .unwrap(),
                        receipt,
                        Vec::new(),
                        session_id.clone(),
                    )
                    .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                retry.outcome(),
                crate::store::PreparedRuntimeSessionCommitOutcome::AlreadyAppliedExact
            );
            assert!(
                runtime_store
                    .load_head_canonical_provisional_tail(&runtime_id)
                    .await
                    .unwrap()
                    .is_none(),
                "final authority must atomically clear the promoted provisional tail"
            );
            let catalog = runtime_store
                .load_runtime_session_catalog_entry(&runtime_id)
                .await
                .unwrap()
                .expect("promoted HeadCanonical catalog");
            assert_eq!(
                catalog.persistence_profile(),
                RuntimeSessionPersistenceProfile::HeadCanonicalV1
            );
            assert_eq!(catalog.message_count(), second_candidate.messages().len());

            let observed_head = physical_store
                .load_head(&session_id)
                .await
                .unwrap()
                .expect("promoted physical head");
            let mut next_run_candidate = physical_store
                .load(&session_id)
                .await
                .unwrap()
                .expect("promoted physical session");
            next_run_candidate.push(Message::User(UserMessage::text("next run checkpoint")));
            let next_run_tail =
                PreparedHeadCanonicalMutation::prepare(&next_run_candidate, Some(observed_head))
                    .unwrap();
            let next_run = runtime_store
                .write_prepared_head_canonical_provisional_tail(
                    &runtime_id,
                    PreparedHeadCanonicalProvisionalTail::prepare(
                        successor.clone(),
                        RunId::new(),
                        next_run_tail.successor_head(),
                        next_run_tail.successor_head_token(),
                        &next_run_candidate,
                    )
                    .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                next_run.physical_store_revision(),
                successor.store_revision() + 1,
                "the first checkpoint of the next run must consume the revision immediately after final promotion"
            );
            assert_eq!(next_run.candidate_sequence(), 1);
        }

        #[test]
        fn head_canonical_provisional_schema_rejects_partial_predecessor_projection() {
            let (_dir, store) = temp_store();
            let session_id = Session::new().id().clone();
            let runtime_id = LogicalRuntimeId::for_session(&session_id);
            let conn = open_runtime_connection(store.path()).unwrap();
            let result = conn.execute(
                r"
                INSERT INTO runtime_head_canonical_provisional_tails (
                    runtime_id, authority_version, session_id,
                    base_store_revision, base_committed_head_token,
                    physical_store_revision, physical_head_token,
                    run_id, candidate_sequence,
                    candidate_message_count, candidate_conversation_digest,
                    catalog_json, compaction_intents_json,
                    predecessor_candidate_message_count
                ) VALUES (
                    ?1, 1, ?2, 1, 'base-token',
                    2, 'physical-token', ?3, 1,
                    0, 'sha256:candidate', X'7B7D', X'5B5D', 0
                )
                ",
                params![
                    runtime_id_text(&runtime_id),
                    session_id.to_string(),
                    RunId::new().to_string(),
                ],
            );
            assert!(
                result.is_err(),
                "SQLite must reject every partially populated predecessor projection bundle"
            );
        }

        #[test]
        fn pending_owner_continuation_query_exposes_composite_key_range() {
            assert!(!LOAD_PENDING_TERMINAL_OWNER_CONTINUATION_PAGE_SQL.contains(" IS NULL OR "));
            assert!(
                LOAD_PENDING_TERMINAL_OWNER_CONTINUATION_PAGE_SQL.contains("owner_input_id > ?2")
            );
            let (_dir, store) = temp_store();
            let conn = open_runtime_connection(store.path()).unwrap();
            let explain =
                format!("EXPLAIN QUERY PLAN {LOAD_PENDING_TERMINAL_OWNER_CONTINUATION_PAGE_SQL}");
            let detail = conn
                .query_row(&explain, params!["runtime", "after", 16_i64], |row| {
                    row.get::<_, String>(3)
                })
                .unwrap();
            assert!(
                detail.contains("runtime_id=? AND owner_input_id>?"),
                "continuation must use the composite primary-key range: {detail}"
            );
        }

        #[tokio::test]
        async fn only_migration_marked_released_receipt_can_adopt_missing_witness() {
            let dir = TempDir::new().unwrap();
            let path = dir.path().join("released-receipt-ack-loss.sqlite3");
            let store = SqliteRuntimeStore::new_head_canonical(&path).unwrap();

            let session = session_with_user("supported-floor boundary");
            let session_id = session.id().clone();
            let runtime_id = LogicalRuntimeId::for_session(&session_id);
            let mutation = PreparedHeadCanonicalMutation::prepare(&session, None).unwrap();
            let document = || {
                BoundSessionCommit::sealed(Arc::new(session.clone()))
                    .unwrap()
                    .with_head_canonical_mutation(mutation.clone())
                    .unwrap()
            };

            let input_id = InputId::new();
            let mut input = StoredInputState::new_accepted(input_id.clone());
            input.state.idempotency_key =
                Some(crate::identifiers::IdempotencyKey::new("floor-ack-loss"));
            let input = persistable(input);
            let lifecycle = lifecycle_commit(&runtime_id, RuntimeState::Idle, 41, 9);
            let receipt = RunBoundaryReceipt {
                run_id: RunId::new(),
                boundary: RunApplyBoundary::Immediate,
                contributing_input_ids: vec![input_id],
                conversation_digest: Some(session.transcript_content_digest().unwrap()),
                message_count: session.messages().len(),
                sequence: 1,
            };

            let first = store
                .commit_prepared_session_boundary(
                    &runtime_id,
                    PreparedRuntimeSessionCommit::machine_terminal(
                        document(),
                        receipt.clone(),
                        lifecycle.clone(),
                        vec![input.clone()],
                        session_id.clone(),
                    ),
                )
                .await
                .unwrap();
            assert_eq!(
                first.outcome(),
                crate::store::PreparedRuntimeSessionCommitOutcome::Applied
            );

            // A missing witness on current state is corruption, not evidence
            // that this receipt came from the released v1 schema.
            let conn = Connection::open(&path).unwrap();
            assert_eq!(
                conn.execute(
                    "DELETE FROM runtime_session_boundary_witnesses \
                     WHERE runtime_id = ?1",
                    params![runtime_id_text(&runtime_id)],
                )
                .unwrap(),
                1
            );
            drop(conn);

            let unmarked = store
                .commit_prepared_session_boundary(
                    &runtime_id,
                    PreparedRuntimeSessionCommit::machine_terminal(
                        document(),
                        receipt.clone(),
                        lifecycle.clone(),
                        vec![input.clone()],
                        session_id.clone(),
                    ),
                )
                .await
                .unwrap_err();
            assert_eq!(
                unmarked
                    .to_string()
                    .contains("no request witness and no exact released 0.8.10 migration marker"),
                true,
                "current missing-witness state must fail closed: {unmarked}"
            );

            // Model the exact marker created only by v1 -> v2 activation.
            let conn = Connection::open(&path).unwrap();
            let receipt_json = serde_json::to_vec(&receipt).unwrap();
            assert_eq!(
                conn.execute(
                    r"
                    INSERT INTO runtime_released_0810_boundary_receipts (
                        runtime_id, run_id, sequence, receipt_sha256
                    )
                    VALUES (?1, ?2, ?3, ?4)
                    ",
                    params![
                        runtime_id_text(&runtime_id),
                        receipt.run_id.0.to_string(),
                        encode_receipt_sequence(receipt.sequence),
                        input_row_version_digest(&receipt_json),
                    ],
                )
                .unwrap(),
                1
            );
            drop(conn);

            let fenced_input = input
                .clone()
                .with_expected_row_digest("sha256:unprovable-released-precondition".to_string());
            let fenced = store
                .commit_prepared_session_boundary(
                    &runtime_id,
                    PreparedRuntimeSessionCommit::machine_terminal(
                        document(),
                        receipt.clone(),
                        lifecycle.clone(),
                        vec![fenced_input],
                        session_id.clone(),
                    ),
                )
                .await
                .unwrap_err();
            assert!(
                fenced
                    .to_string()
                    .contains("cannot prove current lifecycle/input CAS preconditions"),
                "released receipt must not certify an unretained prior-row fence: {fenced}"
            );

            let adopted = store
                .commit_prepared_session_boundary(
                    &runtime_id,
                    PreparedRuntimeSessionCommit::machine_terminal(
                        document(),
                        receipt.clone(),
                        lifecycle.clone(),
                        vec![input.clone()],
                        session_id.clone(),
                    ),
                )
                .await
                .unwrap();
            assert_eq!(
                adopted.outcome(),
                crate::store::PreparedRuntimeSessionCommitOutcome::AlreadyAppliedReleasedEquivalent
            );
            let conn = Connection::open(&path).unwrap();
            assert_eq!(
                conn.query_row(
                    "SELECT COUNT(*) FROM runtime_session_boundary_witnesses \
                     WHERE runtime_id = ?1",
                    params![runtime_id_text(&runtime_id)],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
                1,
                "adoption must install the exact request witness atomically"
            );
            assert_eq!(
                conn.query_row(
                    "SELECT COUNT(*) FROM runtime_released_0810_boundary_receipts \
                     WHERE runtime_id = ?1",
                    params![runtime_id_text(&runtime_id)],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
                0,
                "adoption must consume the released migration marker atomically"
            );
            drop(conn);

            let exact_retry = store
                .commit_prepared_session_boundary(
                    &runtime_id,
                    PreparedRuntimeSessionCommit::machine_terminal(
                        document(),
                        receipt,
                        lifecycle,
                        vec![input],
                        session_id,
                    ),
                )
                .await
                .unwrap();
            assert_eq!(
                exact_retry.outcome(),
                crate::store::PreparedRuntimeSessionCommitOutcome::AlreadyAppliedExact
            );
        }

        fn session_with_compaction_intent() -> (Session, meerkat_core::CompactionProjectionIntent) {
            let mut session = session_with_user("verbose context one");
            session.push(Message::User(UserMessage::text("verbose context two")));
            let parent = session.transcript_revision().unwrap();
            session
                .commit_transcript_rewrite(
                    TranscriptRewriteSelection::MessageRange { start: 0, end: 2 },
                    vec![Message::User(UserMessage::compaction_summary(
                        "compacted context",
                    ))],
                    TranscriptRewriteReason::new("compaction"),
                    Some("sqlite-outbox-test".to_string()),
                    Some(parent),
                )
                .unwrap();
            let mut encoded = serde_json::to_value(&session).unwrap();
            encoded["metadata"][meerkat_core::SESSION_TRANSCRIPT_HISTORY_STATE_KEY]["commits"][0]
                ["selection"] = serde_json::json!({
                "type": "compaction_message_range",
                "range": { "start": 0, "end": 2 }
            });
            let mut session: Session = serde_json::from_value(encoded).unwrap();
            let commit = session
                .transcript_history_state()
                .unwrap()
                .unwrap()
                .commits()
                .last()
                .unwrap()
                .clone();
            let intent = meerkat_core::CompactionProjectionIntent {
                projection: serde_json::from_value(serde_json::json!({
                    "session_id": session.id(),
                    "parent_revision": &commit.parent_revision,
                    "revision": &commit.revision,
                    "commit_fingerprint": "sha256:aee1fea2386a630969f33a58068390400ed9c0e5964a1838269ae2eeab2761da",
                }))
                .unwrap(),
                summary_tokens: 5,
                messages_before: 2,
                messages_after: 1,
            };
            session
                .add_compaction_projection_intent(intent.clone())
                .unwrap();
            (session, intent)
        }

        fn snapshot_with_raw_intents(
            session: &Session,
            intents: &[meerkat_core::CompactionProjectionIntent],
        ) -> Vec<u8> {
            let mut value = serde_json::to_value(session).unwrap();
            value["metadata"][meerkat_core::memory::SESSION_COMPACTION_PROJECTION_INTENTS_KEY] =
                serde_json::to_value(intents).unwrap();
            serde_json::to_vec(&value).unwrap()
        }

        fn unbacked_intent(
            session_id: &meerkat_core::types::SessionId,
        ) -> meerkat_core::CompactionProjectionIntent {
            meerkat_core::CompactionProjectionIntent {
                projection: serde_json::from_value(serde_json::json!({
                    "session_id": session_id,
                    "parent_revision": "missing-parent",
                    "revision": "missing-revision",
                    "commit_fingerprint": "sha256:unbacked-persisted-fixture",
                }))
                .unwrap(),
                summary_tokens: 1,
                messages_before: 2,
                messages_after: 1,
            }
        }

        #[tokio::test]
        async fn compaction_outbox_is_atomic_durable_and_finalize_ack_is_idempotent() {
            let dir = TempDir::new().unwrap();
            let path = dir.path().join("runtime.sqlite3");
            let runtime_id = runtime_id();
            let (session, intent) = session_with_compaction_intent();
            let snapshot = serde_json::to_vec(&session).unwrap();
            {
                let store = SqliteRuntimeStore::new(&path).unwrap();
                store
                    .atomic_apply(
                        &runtime_id,
                        Some(SerializedSessionSnapshot {
                            session_snapshot: snapshot.clone().into(),
                        }),
                        RunBoundaryReceipt {
                            run_id: RunId::new(),
                            boundary: RunApplyBoundary::RunStart,
                            contributing_input_ids: vec![],
                            conversation_digest: None,
                            message_count: 1,
                            sequence: 41,
                        },
                        vec![],
                        Some(session.id().clone()),
                    )
                    .await
                    .unwrap();
                assert_eq!(
                    store.load_session_snapshot(&runtime_id).await.unwrap(),
                    Some(Arc::new(snapshot))
                );
            }

            let reopened = SqliteRuntimeStore::new(&path).unwrap();
            assert_eq!(
                reopened
                    .load_pending_compaction_projections(&runtime_id)
                    .await
                    .unwrap(),
                vec![intent.clone()]
            );
            reopened
                .mark_compaction_projection_finalized(&runtime_id, &intent.projection)
                .await
                .unwrap();
            reopened
                .mark_compaction_projection_finalized(&runtime_id, &intent.projection)
                .await
                .unwrap();
            assert!(
                reopened
                    .load_pending_compaction_projections(&runtime_id)
                    .await
                    .unwrap()
                    .is_empty()
            );
            drop(reopened);
            let after_ack_reopen = SqliteRuntimeStore::new(&path).unwrap();
            let persisted: Session = serde_json::from_slice(
                &after_ack_reopen
                    .load_session_snapshot(&runtime_id)
                    .await
                    .unwrap()
                    .unwrap(),
            )
            .unwrap();
            assert!(
                persisted
                    .compaction_projection_intents()
                    .unwrap()
                    .is_empty()
            );
            assert!(
                after_ack_reopen
                    .load_pending_compaction_projections(&runtime_id)
                    .await
                    .unwrap()
                    .is_empty()
            );
        }

        #[tokio::test]
        async fn finalized_sqlite_outbox_tombstone_rejects_all_snapshot_replay_paths() {
            let (_dir, store) = temp_store();
            let runtime_id = runtime_id();
            let (session, intent) = session_with_compaction_intent();
            let replay_snapshot = serde_json::to_vec(&session).unwrap();
            let receipt = |run_id, sequence| RunBoundaryReceipt {
                run_id,
                boundary: RunApplyBoundary::RunStart,
                contributing_input_ids: vec![],
                conversation_digest: None,
                message_count: 1,
                sequence,
            };
            store
                .atomic_apply(
                    &runtime_id,
                    Some(SerializedSessionSnapshot {
                        session_snapshot: replay_snapshot.clone().into(),
                    }),
                    receipt(RunId::new(), 80),
                    vec![],
                    Some(session.id().clone()),
                )
                .await
                .unwrap();
            store
                .mark_compaction_projection_finalized(&runtime_id, &intent.projection)
                .await
                .unwrap();
            let cleaned_snapshot = store
                .load_session_snapshot(&runtime_id)
                .await
                .unwrap()
                .unwrap();

            let replay_run_id = RunId::new();
            let error = store
                .atomic_apply(
                    &runtime_id,
                    Some(SerializedSessionSnapshot {
                        session_snapshot: replay_snapshot.clone().into(),
                    }),
                    receipt(replay_run_id.clone(), 81),
                    vec![],
                    Some(session.id().clone()),
                )
                .await
                .unwrap_err();
            assert!(error.to_string().contains("finalized compaction intent"));
            assert!(
                store
                    .load_boundary_receipt(&runtime_id, &replay_run_id, 81)
                    .await
                    .unwrap()
                    .is_none(),
                "finalized replay rejection must roll back the whole SQLite transaction"
            );

            let error = store
                .commit_session_snapshot(
                    &runtime_id,
                    SerializedSessionSnapshot {
                        session_snapshot: replay_snapshot.clone().into(),
                    },
                )
                .await
                .unwrap_err();
            assert!(error.to_string().contains("finalized compaction intent"));
            let error = store
                .replace_session_snapshot_if_current(
                    &runtime_id,
                    &cleaned_snapshot,
                    replay_snapshot,
                )
                .await
                .unwrap_err();
            assert!(error.to_string().contains("finalized compaction intent"));

            assert_eq!(
                store.load_session_snapshot(&runtime_id).await.unwrap(),
                Some(cleaned_snapshot)
            );
            assert!(
                store
                    .load_pending_compaction_projections(&runtime_id)
                    .await
                    .unwrap()
                    .is_empty(),
                "a finalized SQLite tombstone must never be silently revived or left untracked"
            );
        }

        #[tokio::test]
        async fn invalid_compaction_outbox_intent_rolls_back_snapshot_and_outbox() {
            let (_dir, store) = temp_store();
            let runtime_id = runtime_id();
            let (session, mut conflicting) = session_with_compaction_intent();
            let original = session.compaction_projection_intents().unwrap()[0].clone();
            conflicting.summary_tokens += 1;
            let error = store
                .atomic_apply(
                    &runtime_id,
                    Some(SerializedSessionSnapshot {
                        session_snapshot: snapshot_with_raw_intents(
                            &session,
                            &[original, conflicting],
                        )
                        .into(),
                    }),
                    RunBoundaryReceipt {
                        run_id: RunId::new(),
                        boundary: RunApplyBoundary::RunStart,
                        contributing_input_ids: vec![],
                        conversation_digest: None,
                        message_count: 1,
                        sequence: 42,
                    },
                    vec![],
                    Some(session.id().clone()),
                )
                .await
                .unwrap_err();
            assert!(matches!(error, RuntimeStoreError::WriteFailed(_)));
            assert_eq!(
                store.load_session_snapshot(&runtime_id).await.unwrap(),
                None
            );
            assert!(
                store
                    .load_pending_compaction_projections(&runtime_id)
                    .await
                    .unwrap()
                    .is_empty()
            );

            let foreign = session_with_compaction_intent().1;
            for (sequence, invalid) in [foreign, unbacked_intent(session.id())]
                .into_iter()
                .enumerate()
            {
                let error = store
                    .atomic_apply(
                        &runtime_id,
                        Some(SerializedSessionSnapshot {
                            session_snapshot: snapshot_with_raw_intents(&session, &[invalid])
                                .into(),
                        }),
                        RunBoundaryReceipt {
                            run_id: RunId::new(),
                            boundary: RunApplyBoundary::RunStart,
                            contributing_input_ids: vec![],
                            conversation_digest: None,
                            message_count: 1,
                            sequence: 50 + sequence as u64,
                        },
                        vec![],
                        Some(session.id().clone()),
                    )
                    .await
                    .unwrap_err();
                assert!(matches!(error, RuntimeStoreError::WriteFailed(_)));
                assert_eq!(
                    store.load_session_snapshot(&runtime_id).await.unwrap(),
                    None
                );
                assert!(
                    store
                        .load_pending_compaction_projections(&runtime_id)
                        .await
                        .unwrap()
                        .is_empty()
                );
            }
        }

        #[tokio::test]
        async fn superseded_snapshot_rejects_without_advancing_sqlite_compaction_outbox() {
            let (_dir, store) = temp_store();
            let runtime_id = runtime_id();
            let (incoming, intent) = session_with_compaction_intent();
            let mut current = incoming.clone();
            current
                .complete_compaction_projection_intent(&intent.projection)
                .unwrap();
            current.push(Message::User(UserMessage::text("already advanced")));
            let current_snapshot = serde_json::to_vec(&current).unwrap();
            store
                .commit_session_snapshot(
                    &runtime_id,
                    SerializedSessionSnapshot {
                        session_snapshot: current_snapshot.clone().into(),
                    },
                )
                .await
                .unwrap();
            let error = store
                .atomic_apply(
                    &runtime_id,
                    Some(SerializedSessionSnapshot {
                        session_snapshot: serde_json::to_vec(&incoming).unwrap().into(),
                    }),
                    RunBoundaryReceipt {
                        run_id: RunId::new(),
                        boundary: RunApplyBoundary::RunStart,
                        contributing_input_ids: vec![],
                        conversation_digest: None,
                        message_count: 1,
                        sequence: 43,
                    },
                    vec![],
                    Some(incoming.id().clone()),
                )
                .await
                .expect_err("superseded compaction boundary must be explicitly rejected");
            assert!(matches!(
                error,
                RuntimeStoreError::SessionSnapshotSuperseded { .. }
            ));
            assert_eq!(
                store.load_session_snapshot(&runtime_id).await.unwrap(),
                Some(Arc::new(current_snapshot))
            );
            assert!(
                store
                    .load_pending_compaction_projections(&runtime_id)
                    .await
                    .unwrap()
                    .is_empty()
            );
        }

        #[tokio::test]
        async fn existing_sqlite_outbox_rejects_changed_intent_without_advancing_snapshot() {
            let (_dir, store) = temp_store();
            let runtime_id = runtime_id();
            let (session, intent) = session_with_compaction_intent();
            let original_snapshot = serde_json::to_vec(&session).unwrap();
            let receipt = |sequence| RunBoundaryReceipt {
                run_id: RunId::new(),
                boundary: RunApplyBoundary::RunStart,
                contributing_input_ids: vec![],
                conversation_digest: None,
                message_count: 1,
                sequence,
            };
            store
                .atomic_apply(
                    &runtime_id,
                    Some(SerializedSessionSnapshot {
                        session_snapshot: original_snapshot.clone().into(),
                    }),
                    receipt(70),
                    vec![],
                    Some(session.id().clone()),
                )
                .await
                .unwrap();
            let mut advanced = session.clone();
            advanced.push(Message::User(UserMessage::text("later turn")));
            let mut conflicting = intent.clone();
            conflicting.summary_tokens += 1;
            let error = store
                .atomic_apply(
                    &runtime_id,
                    Some(SerializedSessionSnapshot {
                        session_snapshot: snapshot_with_raw_intents(&advanced, &[conflicting])
                            .into(),
                    }),
                    receipt(71),
                    vec![],
                    Some(session.id().clone()),
                )
                .await
                .unwrap_err();
            assert!(matches!(error, RuntimeStoreError::WriteFailed(_)));
            assert_eq!(
                store.load_session_snapshot(&runtime_id).await.unwrap(),
                Some(Arc::new(original_snapshot))
            );
            assert_eq!(
                store
                    .load_pending_compaction_projections(&runtime_id)
                    .await
                    .unwrap(),
                vec![intent]
            );
        }

        #[tokio::test]
        async fn sqlite_non_boundary_snapshot_apis_cannot_bypass_compaction_outbox() {
            let (_dir, store) = temp_store();
            let runtime_id = runtime_id();
            let (session, _intent) = session_with_compaction_intent();
            let snapshot = serde_json::to_vec(&session).unwrap();
            assert!(
                store
                    .commit_session_snapshot(
                        &runtime_id,
                        SerializedSessionSnapshot {
                            session_snapshot: snapshot.clone().into(),
                        },
                    )
                    .await
                    .is_err()
            );
            let clean = Session::with_id(session.id().clone());
            let clean_snapshot = serde_json::to_vec(&clean).unwrap();
            store
                .commit_session_snapshot(
                    &runtime_id,
                    SerializedSessionSnapshot {
                        session_snapshot: clean_snapshot.clone().into(),
                    },
                )
                .await
                .unwrap();
            assert!(
                store
                    .replace_session_snapshot_if_current(&runtime_id, &clean_snapshot, snapshot,)
                    .await
                    .is_err()
            );
            assert_eq!(
                store.load_session_snapshot(&runtime_id).await.unwrap(),
                Some(Arc::new(clean_snapshot))
            );
            assert!(
                store
                    .load_pending_compaction_projections(&runtime_id)
                    .await
                    .unwrap()
                    .is_empty()
            );
        }

        fn receipt_row_count(store: &SqliteRuntimeStore) -> usize {
            let conn = open_runtime_connection(store.path()).unwrap();
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM runtime_boundary_receipts",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            usize::try_from(count).unwrap()
        }

        #[tokio::test]
        async fn atomic_apply_roundtrip() {
            let (_dir, store) = temp_store();
            let runtime_id = runtime_id();
            let session = serde_json::to_vec(&meerkat_core::Session::new()).unwrap();
            let receipt = RunBoundaryReceipt {
                run_id: RunId(uuid::Uuid::new_v4()),
                boundary: RunApplyBoundary::RunStart,
                contributing_input_ids: vec![],
                conversation_digest: Some("machine-owned-digest".to_string()),
                message_count: 42,
                sequence: 5,
            };
            store
                .atomic_apply(
                    &runtime_id,
                    Some(SerializedSessionSnapshot {
                        session_snapshot: session.clone().into(),
                    }),
                    receipt.clone(),
                    vec![input_state()],
                    None,
                )
                .await
                .unwrap();

            assert!(
                store
                    .load_session_snapshot(&runtime_id)
                    .await
                    .unwrap()
                    .is_some()
            );
            assert_eq!(
                store
                    .load_input_states_strict(&runtime_id)
                    .await
                    .unwrap()
                    .len(),
                1
            );
        }

        #[tokio::test]
        async fn input_state_batch_cas_two_handles_exactly_one_adopter_wins() {
            let dir = TempDir::new().unwrap();
            let path = dir.path().join("input-cas-race.sqlite3");
            let runtime_id = LogicalRuntimeId::new("input-cas-race");
            let expected: Vec<_> = (0..8)
                .map(|_| StoredInputState::new_accepted(InputId::new()))
                .collect();
            let initial: Vec<_> = expected.iter().cloned().map(persistable).collect();
            SqliteRuntimeStore::new(path.clone())
                .unwrap()
                .persist_input_states_atomically(&runtime_id, &initial)
                .await
                .unwrap();

            let expected_a = expected.clone();
            let expected_b = expected.clone();
            let replacements_a = replacement_records(&expected, 1);
            let replacements_b = replacement_records(&expected, 2);
            let runtime_a = runtime_id.clone();
            let runtime_b = runtime_id.clone();
            let path_a = path.clone();
            let path_b = path.clone();
            let adopter_a = tokio::spawn(async move {
                SqliteRuntimeStore::new(path_a)
                    .unwrap()
                    .compare_and_swap_input_states_atomically(
                        &runtime_a,
                        &expected_a,
                        &replacements_a,
                    )
                    .await
            });
            let adopter_b = tokio::spawn(async move {
                SqliteRuntimeStore::new(path_b)
                    .unwrap()
                    .compare_and_swap_input_states_atomically(
                        &runtime_b,
                        &expected_b,
                        &replacements_b,
                    )
                    .await
            });
            let outcome_a = adopter_a.await.unwrap().unwrap();
            let outcome_b = adopter_b.await.unwrap().unwrap();
            assert!(matches!(
                (outcome_a, outcome_b),
                (
                    InputStateBatchCasOutcome::Swapped,
                    InputStateBatchCasOutcome::Stale
                ) | (
                    InputStateBatchCasOutcome::Stale,
                    InputStateBatchCasOutcome::Swapped
                )
            ));

            let winner_count = if outcome_a == InputStateBatchCasOutcome::Swapped {
                1
            } else {
                2
            };
            let rows = SqliteRuntimeStore::new(path)
                .unwrap()
                .load_input_states_strict(&runtime_id)
                .await
                .unwrap();
            assert_eq!(rows.len(), expected.len());
            assert!(
                rows.iter()
                    .all(|row| row.state.recovery_count == winner_count),
                "the stale adopter must change no row"
            );
        }

        #[tokio::test]
        async fn input_state_batch_cas_sqlite_write_fault_rolls_back_every_row() {
            let (_dir, store) = temp_store();
            let runtime_id = LogicalRuntimeId::new("input-cas-fault");
            let expected: Vec<_> = (0..3)
                .map(|_| StoredInputState::new_accepted(InputId::new()))
                .collect();
            let initial: Vec<_> = expected.iter().cloned().map(persistable).collect();
            store
                .persist_input_states_atomically(&runtime_id, &initial)
                .await
                .unwrap();
            let replacements = replacement_records(&expected, 9);

            let fault_input_id = expected[1].state.input_id.0.to_string();
            let conn = open_runtime_connection(store.path()).unwrap();
            conn.execute_batch(&format!(
                r"
                CREATE TRIGGER fail_exact_input_batch_cas
                BEFORE UPDATE ON runtime_input_states
                WHEN NEW.input_id = '{fault_input_id}'
                BEGIN
                    SELECT RAISE(ABORT, 'synthetic exact input batch CAS fault');
                END;
                "
            ))
            .unwrap();
            drop(conn);

            let error = store
                .compare_and_swap_input_states_atomically(&runtime_id, &expected, &replacements)
                .await
                .expect_err("trigger must abort the replacement transaction");
            assert!(matches!(error, RuntimeStoreError::WriteFailed(_)));
            let rows = store.load_input_states_strict(&runtime_id).await.unwrap();
            assert_eq!(rows.len(), expected.len());
            assert!(
                rows.iter().all(|row| row.state.recovery_count == 0),
                "a mid-batch write fault must roll back earlier updates"
            );
        }

        #[tokio::test]
        async fn input_state_batch_cas_sqlite_accepts_256_rows_atomically() {
            let (_dir, store) = temp_store();
            let runtime_id = LogicalRuntimeId::new("input-cas-256");
            let expected: Vec<_> = (0..crate::store::MAX_INPUT_STATE_BATCH_CAS)
                .map(|_| StoredInputState::new_accepted(InputId::new()))
                .collect();
            let initial: Vec<_> = expected.iter().cloned().map(persistable).collect();
            store
                .persist_input_states_atomically(&runtime_id, &initial)
                .await
                .unwrap();
            let replacements = replacement_records(&expected, 7);

            assert_eq!(
                store
                    .compare_and_swap_input_states_atomically(
                        &runtime_id,
                        &expected,
                        &replacements,
                    )
                    .await
                    .unwrap(),
                InputStateBatchCasOutcome::Swapped
            );
            assert_eq!(
                store
                    .compare_and_swap_input_states_atomically(
                        &runtime_id,
                        &expected,
                        &replacements,
                    )
                    .await
                    .unwrap(),
                InputStateBatchCasOutcome::Swapped,
                "retry after a lost CAS acknowledgement must observe the exact replacement as success"
            );
            let rows = store.load_input_states_strict(&runtime_id).await.unwrap();
            assert_eq!(rows.len(), crate::store::MAX_INPUT_STATE_BATCH_CAS);
            assert!(rows.iter().all(|row| row.state.recovery_count == 7));
        }

        #[tokio::test]
        async fn commit_session_snapshot_does_not_write_boundary_receipt() {
            let (_dir, store) = temp_store();
            let runtime_id = runtime_id();
            let session = serde_json::to_vec(&meerkat_core::Session::new()).unwrap();

            store
                .commit_session_snapshot(
                    &runtime_id,
                    SerializedSessionSnapshot {
                        session_snapshot: session.into(),
                    },
                )
                .await
                .unwrap();

            assert!(
                store
                    .load_session_snapshot(&runtime_id)
                    .await
                    .unwrap()
                    .is_some()
            );
            assert_eq!(receipt_row_count(&store), 0);
            assert!(
                store
                    .load_input_states_strict(&runtime_id)
                    .await
                    .unwrap()
                    .is_empty()
            );
        }

        #[tokio::test]
        async fn commit_session_snapshot_identical_bytes_does_not_update_snapshot_row() {
            let (_dir, store) = temp_store();
            let runtime_id = runtime_id();
            let mut session = Session::new();
            session.push(Message::User(UserMessage::text("before".to_string())));
            session
                .commit_transcript_rewrite(
                    TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
                    vec![Message::User(UserMessage::text("after".to_string()))],
                    TranscriptRewriteReason::new("unit-test-edit"),
                    Some("unit-test".to_string()),
                    None,
                )
                .unwrap();
            let snapshot = serde_json::to_vec(&session).unwrap();

            store
                .commit_session_snapshot(
                    &runtime_id,
                    SerializedSessionSnapshot {
                        session_snapshot: snapshot.clone().into(),
                    },
                )
                .await
                .unwrap();

            let conn = open_runtime_connection(store.path()).unwrap();
            conn.execute_batch(
                r"
                CREATE TRIGGER reject_runtime_snapshot_update
                BEFORE UPDATE ON runtime_session_snapshots
                BEGIN
                    SELECT RAISE(ABORT, 'identical runtime snapshot was rewritten');
                END;
                ",
            )
            .unwrap();
            drop(conn);

            store
                .commit_session_snapshot(
                    &runtime_id,
                    SerializedSessionSnapshot {
                        session_snapshot: snapshot.clone().into(),
                    },
                )
                .await
                .expect("identical validated snapshot should bypass the UPDATE path");

            assert_eq!(
                store.load_session_snapshot(&runtime_id).await.unwrap(),
                Some(Arc::new(snapshot))
            );
        }

        /// Replace one occurrence of `needle` so the fixture differs in
        /// content but not in serialized length (same-session document,
        /// deterministic byte-for-byte otherwise).
        fn splice_bytes(bytes: &[u8], needle: &[u8], replacement: &[u8]) -> Vec<u8> {
            assert_eq!(needle.len(), replacement.len());
            let position = bytes
                .windows(needle.len())
                .position(|window| window == needle)
                .expect("fixture needle present");
            let mut out = bytes.to_vec();
            out[position..position + needle.len()].copy_from_slice(replacement);
            out
        }

        #[tokio::test]
        async fn commit_session_snapshot_growth_ships_zero_probe_bytes() {
            let (_dir, store) = temp_store();
            let runtime_id = runtime_id();
            let mut session = Session::new();
            session.push(Message::User(UserMessage::text("first turn".to_string())));
            store
                .commit_session_snapshot(
                    &runtime_id,
                    SerializedSessionSnapshot {
                        session_snapshot: serde_json::to_vec(&session).unwrap().into(),
                    },
                )
                .await
                .unwrap();
            let baseline = store.snapshot_byte_probe_bytes();

            session.push(Message::User(UserMessage::text(
                "second turn grows the document".to_string(),
            )));
            let grown = serde_json::to_vec(&session).unwrap();
            store
                .commit_session_snapshot(
                    &runtime_id,
                    SerializedSessionSnapshot {
                        session_snapshot: grown.clone().into(),
                    },
                )
                .await
                .unwrap();

            assert_eq!(
                store.snapshot_byte_probe_bytes(),
                baseline,
                "a length-changing save must answer the unchanged-snapshot \
                 question from the stored length alone, not ship the whole \
                 candidate blob into SQLite for a byte compare that answers no"
            );
            assert_eq!(
                store.load_session_snapshot(&runtime_id).await.unwrap(),
                Some(Arc::new(grown))
            );
        }

        #[tokio::test]
        async fn commit_session_snapshot_equal_length_different_bytes_still_byte_compares() {
            let (_dir, store) = temp_store();
            let runtime_id = runtime_id();
            let mut session = Session::new();
            session.push(Message::User(UserMessage::text("probe".to_string())));
            session.set_metadata(
                "probe_slot",
                serde_json::Value::String("probe-fixture-aaaa".to_string()),
            );
            let first = serde_json::to_vec(&session).unwrap();
            let second = splice_bytes(&first, b"probe-fixture-aaaa", b"probe-fixture-bbbb");
            assert_eq!(first.len(), second.len());
            assert_ne!(first, second);

            store
                .commit_session_snapshot(
                    &runtime_id,
                    SerializedSessionSnapshot {
                        session_snapshot: first.into(),
                    },
                )
                .await
                .unwrap();
            let before = store.snapshot_byte_probe_bytes();
            store
                .commit_session_snapshot(
                    &runtime_id,
                    SerializedSessionSnapshot {
                        session_snapshot: second.clone().into(),
                    },
                )
                .await
                .unwrap();

            assert_eq!(
                store.snapshot_byte_probe_bytes() - before,
                second.len() as u64,
                "length-equal candidates must still run the byte probe"
            );
            assert_eq!(
                store.load_session_snapshot(&runtime_id).await.unwrap(),
                Some(Arc::new(second)),
                "length-equal but different content must be written, not \
                 treated as unchanged"
            );
        }

        #[tokio::test]
        async fn commit_session_snapshot_does_not_write_session_projection_row() {
            let dir = TempDir::new().unwrap();
            let path = dir.path().join("sessions.sqlite3");
            let store = SqliteRuntimeStore::new(path.clone()).unwrap();
            let runtime_id = runtime_id();
            let session = meerkat_core::Session::new();
            let session_id = session.id().clone();

            store
                .commit_session_snapshot(
                    &runtime_id,
                    SerializedSessionSnapshot {
                        session_snapshot: serde_json::to_vec(&session).unwrap().into(),
                    },
                )
                .await
                .unwrap();

            let session_store = SqliteSessionStore::open(path).unwrap();
            assert!(
                session_store.load(&session_id).await.unwrap().is_none(),
                "runtime snapshot commits must not contaminate the SessionStore projection row before checkpoint continuity validation"
            );
            assert!(
                store
                    .load_session_snapshot(&runtime_id)
                    .await
                    .unwrap()
                    .is_some()
            );
        }

        #[tokio::test]
        async fn commit_session_snapshot_rejects_stale_runtime_parent() {
            let (_dir, store) = temp_store();
            let runtime_id = runtime_id();
            let accepted = session_with_user("accepted runtime turn");
            let mut stale = Session::with_id(accepted.id().clone());
            stale.push(Message::User(UserMessage::text(
                "stale runtime turn".to_string(),
            )));
            let accepted_snapshot = serde_json::to_vec(&accepted).unwrap();

            store
                .commit_session_snapshot(
                    &runtime_id,
                    SerializedSessionSnapshot {
                        session_snapshot: accepted_snapshot.clone().into(),
                    },
                )
                .await
                .unwrap();

            let err = store
                .commit_session_snapshot(
                    &runtime_id,
                    SerializedSessionSnapshot {
                        session_snapshot: serde_json::to_vec(&stale).unwrap().into(),
                    },
                )
                .await
                .expect_err("stale non-continuation must not overwrite runtime snapshot");

            assert!(matches!(err, RuntimeStoreError::WriteFailed(_)));
            assert_eq!(
                store.load_session_snapshot(&runtime_id).await.unwrap(),
                Some(Arc::new(accepted_snapshot))
            );
        }

        #[tokio::test]
        async fn atomic_apply_keeps_current_snapshot_when_incoming_is_superseded() {
            let (_dir, store) = temp_store();
            let runtime_id = runtime_id();
            let incoming = session_with_user("turn input");
            let mut current = incoming.clone();
            current.push(Message::BlockAssistant(BlockAssistantMessage {
                blocks: vec![AssistantBlock::Text {
                    text: "peer response already applied".to_string(),
                    meta: None,
                }],
                stop_reason: StopReason::EndTurn,
                identity: meerkat_core::types::TranscriptMessageIdentity::default(),
                created_at: meerkat_core::types::message_timestamp_now(),
            }));
            let current_snapshot = serde_json::to_vec(&current).unwrap();
            let receipt = RunBoundaryReceipt {
                run_id: RunId(uuid::Uuid::new_v4()),
                boundary: RunApplyBoundary::RunStart,
                contributing_input_ids: vec![],
                conversation_digest: Some("machine-owned-digest".to_string()),
                message_count: 2,
                sequence: 11,
            };

            store
                .commit_session_snapshot(
                    &runtime_id,
                    SerializedSessionSnapshot {
                        session_snapshot: current_snapshot.clone().into(),
                    },
                )
                .await
                .unwrap();

            let error = store
                .atomic_apply(
                    &runtime_id,
                    Some(SerializedSessionSnapshot {
                        session_snapshot: serde_json::to_vec(&incoming).unwrap().into(),
                    }),
                    receipt.clone(),
                    vec![input_state()],
                    Some(incoming.id().clone()),
                )
                .await
                .expect_err("superseded atomic commit must be explicitly rejected");
            assert!(matches!(
                error,
                RuntimeStoreError::SessionSnapshotSuperseded { .. }
            ));

            assert_eq!(
                store.load_session_snapshot(&runtime_id).await.unwrap(),
                Some(Arc::new(current_snapshot))
            );
            // The session snapshot was classified superseded and skipped, so the
            // boundary receipt + input-state writes must NOT advance against the
            // retained (more-advanced) session snapshot.
            assert_eq!(
                store
                    .load_boundary_receipt(&runtime_id, &receipt.run_id, receipt.sequence)
                    .await
                    .unwrap(),
                None
            );
            assert!(
                store
                    .load_input_states_strict(&runtime_id)
                    .await
                    .unwrap()
                    .is_empty()
            );
        }

        #[tokio::test]
        async fn atomic_apply_allows_first_generated_snapshot_after_placeholder() {
            let (_dir, store) = temp_store();
            let runtime_id = runtime_id();
            let mut placeholder = Session::new();
            placeholder.append_system_message("base system".to_string());
            let mut incoming = Session::with_id(placeholder.id().clone());
            incoming.append_system_message("base system".to_string());
            incoming.push(Message::User(UserMessage::text(
                "verbose first turn".to_string(),
            )));
            let parent_revision = incoming.transcript_revision().unwrap();
            incoming
                .commit_transcript_rewrite(
                    TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                    vec![Message::User(UserMessage::compaction_summary(
                        "[Context compacted] first turn",
                    ))],
                    TranscriptRewriteReason::new("compaction"),
                    Some("meerkat-core".to_string()),
                    Some(parent_revision),
                )
                .unwrap();
            let incoming_snapshot = serde_json::to_vec(&incoming).unwrap();
            let receipt = RunBoundaryReceipt {
                run_id: RunId(uuid::Uuid::new_v4()),
                boundary: RunApplyBoundary::RunStart,
                contributing_input_ids: vec![],
                conversation_digest: Some("machine-owned-digest".to_string()),
                message_count: incoming.messages().len(),
                sequence: 12,
            };

            store
                .commit_session_snapshot(
                    &runtime_id,
                    SerializedSessionSnapshot {
                        session_snapshot: serde_json::to_vec(&placeholder).unwrap().into(),
                    },
                )
                .await
                .unwrap();

            store
                .atomic_apply(
                    &runtime_id,
                    Some(SerializedSessionSnapshot {
                        session_snapshot: incoming_snapshot.clone().into(),
                    }),
                    receipt.clone(),
                    vec![],
                    Some(incoming.id().clone()),
                )
                .await
                .unwrap();

            assert_eq!(
                store.load_session_snapshot(&runtime_id).await.unwrap(),
                Some(Arc::new(incoming_snapshot))
            );
            assert_eq!(
                store
                    .load_boundary_receipt(&runtime_id, &receipt.run_id, receipt.sequence)
                    .await
                    .unwrap(),
                Some(receipt)
            );
        }

        #[tokio::test]
        async fn atomic_apply_allows_generated_compaction_before_retained_tail() {
            let (_dir, store) = temp_store();
            let runtime_id = runtime_id();
            let mut previous = Session::new();
            previous.append_system_message("runtime system before context refresh".to_string());
            previous.push(Message::User(UserMessage::text(
                "Turn 1 request".to_string(),
            )));
            previous.push(Message::BlockAssistant(BlockAssistantMessage {
                blocks: vec![AssistantBlock::Text {
                    text: "Turn 1 answer".to_string(),
                    meta: None,
                }],
                stop_reason: StopReason::EndTurn,
                identity: meerkat_core::types::TranscriptMessageIdentity::default(),
                created_at: meerkat_core::types::message_timestamp_now(),
            }));

            let mut incoming = Session::with_id(previous.id().clone());
            incoming.append_system_message("runtime system after context refresh".to_string());
            incoming.push(Message::User(UserMessage::text(
                "Verbose context that will be compacted".to_string(),
            )));
            for message in previous.messages()[1..].iter().cloned() {
                incoming.push(message);
            }
            incoming.push(Message::BlockAssistant(BlockAssistantMessage {
                blocks: vec![AssistantBlock::Text {
                    text: "Turn 2 generated answer".to_string(),
                    meta: None,
                }],
                stop_reason: StopReason::EndTurn,
                identity: meerkat_core::types::TranscriptMessageIdentity::default(),
                created_at: meerkat_core::types::message_timestamp_now(),
            }));
            let parent_revision = incoming.transcript_revision().unwrap();
            incoming
                .commit_transcript_rewrite(
                    TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                    vec![Message::User(UserMessage::compaction_summary(
                        "[Context compacted] Earlier runtime context".to_string(),
                    ))],
                    TranscriptRewriteReason::new("compaction"),
                    Some("meerkat-core".to_string()),
                    Some(parent_revision),
                )
                .unwrap();
            let incoming_snapshot = serde_json::to_vec(&incoming).unwrap();
            let receipt = RunBoundaryReceipt {
                run_id: RunId(uuid::Uuid::new_v4()),
                boundary: RunApplyBoundary::RunStart,
                contributing_input_ids: vec![],
                conversation_digest: Some("machine-owned-digest".to_string()),
                message_count: incoming.messages().len(),
                sequence: 13,
            };

            store
                .commit_session_snapshot(
                    &runtime_id,
                    SerializedSessionSnapshot {
                        session_snapshot: serde_json::to_vec(&previous).unwrap().into(),
                    },
                )
                .await
                .unwrap();

            store
                .atomic_apply(
                    &runtime_id,
                    Some(SerializedSessionSnapshot {
                        session_snapshot: incoming_snapshot.clone().into(),
                    }),
                    receipt.clone(),
                    vec![],
                    Some(incoming.id().clone()),
                )
                .await
                .unwrap();

            assert_eq!(
                store.load_session_snapshot(&runtime_id).await.unwrap(),
                Some(Arc::new(incoming_snapshot))
            );
            assert_eq!(
                store
                    .load_boundary_receipt(&runtime_id, &receipt.run_id, receipt.sequence)
                    .await
                    .unwrap(),
                Some(receipt)
            );
        }

        #[tokio::test]
        async fn prepared_transcript_rewrite_boundary_rejects_stale_runtime_parent() {
            let (_dir, store) = temp_store();
            let original = session_with_one_turn();
            let runtime_id = LogicalRuntimeId::for_session(original.id());
            let original_snapshot = serde_json::to_vec(&original).unwrap();
            store
                .commit_session_snapshot(
                    &runtime_id,
                    SerializedSessionSnapshot {
                        session_snapshot: original_snapshot.clone().into(),
                    },
                )
                .await
                .unwrap();

            let parent_revision = original.transcript_revision().unwrap();
            let mut first_rewrite = original.clone();
            let first_commit = first_rewrite
                .commit_transcript_rewrite(
                    TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                    vec![Message::BlockAssistant(BlockAssistantMessage {
                        blocks: vec![AssistantBlock::Text {
                            text: "first compact answer".to_string(),
                            meta: None,
                        }],
                        stop_reason: StopReason::EndTurn,
                        identity: meerkat_core::types::TranscriptMessageIdentity::default(),
                        created_at: meerkat_core::types::message_timestamp_now(),
                    })],
                    TranscriptRewriteReason::new("compaction"),
                    Some("sqlite-test".to_string()),
                    Some(parent_revision.clone()),
                )
                .unwrap();

            let mut stale_rewrite = original.clone();
            let stale_commit = stale_rewrite
                .commit_transcript_rewrite(
                    TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                    vec![Message::BlockAssistant(BlockAssistantMessage {
                        blocks: vec![AssistantBlock::Text {
                            text: "stale compact answer".to_string(),
                            meta: None,
                        }],
                        stop_reason: StopReason::EndTurn,
                        identity: meerkat_core::types::TranscriptMessageIdentity::default(),
                        created_at: meerkat_core::types::message_timestamp_now(),
                    })],
                    TranscriptRewriteReason::new("compaction"),
                    Some("sqlite-test".to_string()),
                    Some(parent_revision),
                )
                .unwrap();

            let committed = store
                .load_committed_whole_blob_snapshot(&runtime_id)
                .await
                .unwrap()
                .unwrap();
            let expected = crate::store::VerifiedCommittedWholeBlobPayload::from_committed(
                original.id(),
                committed,
            )
            .unwrap();
            let first_boundary = crate::store::PreparedWholeBlobRewriteBoundary::prepare(
                expected.clone(),
                first_rewrite,
                std::slice::from_ref(&first_commit),
            )
            .unwrap();
            let stale_boundary = crate::store::PreparedWholeBlobRewriteBoundary::prepare(
                expected,
                stale_rewrite,
                std::slice::from_ref(&stale_commit),
            )
            .unwrap();

            store
                .commit_prepared_whole_blob_rewrite_boundary(
                    &runtime_id,
                    first_boundary.store_parts(),
                )
                .await
                .unwrap();
            let err = store
                .commit_prepared_whole_blob_rewrite_boundary(
                    &runtime_id,
                    stale_boundary.store_parts(),
                )
                .await
                .expect_err("stale rewrite parent should be rejected atomically");
            assert!(matches!(
                err,
                RuntimeStoreError::SessionPersistenceAuthorityConflict { .. }
            ));

            let stored = store
                .load_session_snapshot(&runtime_id)
                .await
                .unwrap()
                .unwrap();
            let stored: Session = serde_json::from_slice(&stored).unwrap();
            assert_eq!(stored.transcript_revision().unwrap(), first_commit.revision);
            let catalog = store
                .load_runtime_session_catalog_entry(&runtime_id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(catalog.updated_at(), stored.updated_at());
            assert_eq!(catalog.message_count(), stored.messages().len());
        }

        #[tokio::test]
        async fn atomic_apply_is_atomic_on_receipt_conflict() {
            let (_dir, store) = temp_store();
            let runtime_id = runtime_id();
            let receipt = RunBoundaryReceipt {
                run_id: RunId(uuid::Uuid::new_v4()),
                boundary: RunApplyBoundary::RunStart,
                contributing_input_ids: vec![],
                conversation_digest: None,
                message_count: 0,
                sequence: 0,
            };

            store
                .atomic_apply(
                    &runtime_id,
                    None,
                    receipt.clone(),
                    vec![input_state()],
                    None,
                )
                .await
                .unwrap();

            let session = serde_json::to_vec(&meerkat_core::Session::new()).unwrap();
            let err = store
                .atomic_apply(
                    &runtime_id,
                    Some(SerializedSessionSnapshot {
                        session_snapshot: session.into(),
                    }),
                    receipt,
                    vec![input_state()],
                    None,
                )
                .await
                .expect_err("duplicate receipt should fail");

            assert!(matches!(err, RuntimeStoreError::WriteFailed(_)));
            assert!(
                store
                    .load_session_snapshot(&runtime_id)
                    .await
                    .unwrap()
                    .is_none()
            );
            let states = store.load_input_states_strict(&runtime_id).await.unwrap();
            assert_eq!(states.len(), 1);
        }

        #[tokio::test]
        async fn machine_terminal_atomic_apply_rolls_back_all_tables_on_receipt_conflict() {
            let (_dir, store) = temp_store();
            let runtime_id = runtime_id();
            let receipt = RunBoundaryReceipt {
                run_id: RunId(uuid::Uuid::new_v4()),
                boundary: RunApplyBoundary::RunStart,
                contributing_input_ids: vec![],
                conversation_digest: None,
                message_count: 0,
                sequence: 0,
            };
            store
                .atomic_apply(
                    &runtime_id,
                    None,
                    receipt.clone(),
                    vec![input_state()],
                    None,
                )
                .await
                .unwrap();

            let session = meerkat_core::Session::new();
            let error = store
                .atomic_apply_with_machine_lifecycle(
                    &runtime_id,
                    SerializedSessionSnapshot {
                        session_snapshot: serde_json::to_vec(&session).unwrap().into(),
                    },
                    receipt,
                    MachineLifecycleCommit::new_with_binding(
                        RuntimeState::Idle,
                        crate::store::MachineLifecycleBindingFacts::default(),
                        crate::store::SupervisorAuthoritySnapshot::UnboundNoReceipt,
                    ),
                    vec![input_state()],
                    session.id().clone(),
                )
                .await
                .expect_err("duplicate receipt should roll back the terminal transaction");
            assert!(matches!(error, RuntimeStoreError::WriteFailed(_)));
            assert!(
                store
                    .load_session_snapshot(&runtime_id)
                    .await
                    .unwrap()
                    .is_none(),
                "failed terminal transaction must roll back the session snapshot"
            );
            assert_eq!(
                crate::store::load_runtime_state(&store, &runtime_id)
                    .await
                    .unwrap(),
                None,
                "failed terminal transaction must roll back machine lifecycle"
            );
            assert_eq!(
                store
                    .load_input_states_strict(&runtime_id)
                    .await
                    .unwrap()
                    .len(),
                1,
                "failed terminal transaction must retain only the seeded input row"
            );
        }

        #[tokio::test]
        async fn machine_terminal_atomic_apply_tracks_and_tombstones_compaction_intents() {
            let (_dir, store) = temp_store();
            let runtime_id = runtime_id();
            let (session, intent) = session_with_compaction_intent();
            let encoded = serde_json::to_vec(&session).unwrap();
            let receipt = |sequence| RunBoundaryReceipt {
                run_id: RunId(uuid::Uuid::new_v4()),
                boundary: RunApplyBoundary::RunStart,
                contributing_input_ids: vec![],
                conversation_digest: None,
                message_count: 0,
                sequence,
            };

            store
                .atomic_apply_with_machine_lifecycle(
                    &runtime_id,
                    SerializedSessionSnapshot {
                        session_snapshot: encoded.clone().into(),
                    },
                    receipt(0),
                    MachineLifecycleCommit::new_with_binding(
                        RuntimeState::Idle,
                        crate::store::MachineLifecycleBindingFacts::default(),
                        crate::store::SupervisorAuthoritySnapshot::UnboundNoReceipt,
                    ),
                    Vec::new(),
                    session.id().clone(),
                )
                .await
                .unwrap();
            assert_eq!(
                store
                    .load_pending_compaction_projections(&runtime_id)
                    .await
                    .unwrap(),
                vec![intent.clone()]
            );

            store
                .mark_compaction_projection_finalized(&runtime_id, &intent.projection)
                .await
                .unwrap();
            let error = store
                .atomic_apply_with_machine_lifecycle(
                    &runtime_id,
                    SerializedSessionSnapshot {
                        session_snapshot: encoded.into(),
                    },
                    receipt(1),
                    MachineLifecycleCommit::new_with_binding(
                        RuntimeState::Idle,
                        crate::store::MachineLifecycleBindingFacts::default(),
                        crate::store::SupervisorAuthoritySnapshot::UnboundNoReceipt,
                    ),
                    Vec::new(),
                    session.id().clone(),
                )
                .await
                .expect_err("a finalized compaction tombstone must reject stale terminal replay");
            assert!(
                error
                    .to_string()
                    .contains("replays finalized compaction intent")
            );
        }

        #[tokio::test]
        async fn machine_terminal_atomic_apply_rejects_superseded_snapshot_without_publication() {
            let (_dir, store) = temp_store();
            let runtime_id = runtime_id();
            let incoming = session_with_user("failed turn input");
            let mut durable_head = incoming.clone();
            durable_head.push(Message::User(meerkat_core::types::UserMessage::text(
                "already advanced",
            )));
            let durable_snapshot = serde_json::to_vec(&durable_head).unwrap();
            store
                .commit_session_snapshot(
                    &runtime_id,
                    SerializedSessionSnapshot {
                        session_snapshot: durable_snapshot.clone().into(),
                    },
                )
                .await
                .unwrap();

            let receipt = RunBoundaryReceipt {
                run_id: RunId(uuid::Uuid::new_v4()),
                boundary: RunApplyBoundary::RunStart,
                contributing_input_ids: vec![],
                conversation_digest: None,
                message_count: 0,
                sequence: 0,
            };
            let error = store
                .atomic_apply_with_machine_lifecycle(
                    &runtime_id,
                    SerializedSessionSnapshot {
                        session_snapshot: serde_json::to_vec(&incoming).unwrap().into(),
                    },
                    receipt.clone(),
                    MachineLifecycleCommit::new_with_binding(
                        RuntimeState::Idle,
                        crate::store::MachineLifecycleBindingFacts::default(),
                        crate::store::SupervisorAuthoritySnapshot::UnboundNoReceipt,
                    ),
                    vec![input_state()],
                    incoming.id().clone(),
                )
                .await
                .expect_err("superseded terminal snapshot must reject the entire transaction");
            assert!(matches!(
                error,
                RuntimeStoreError::SessionSnapshotSuperseded { .. }
            ));
            assert_eq!(
                store.load_session_snapshot(&runtime_id).await.unwrap(),
                Some(Arc::new(durable_snapshot))
            );
            assert_eq!(
                crate::store::load_runtime_state(&store, &runtime_id)
                    .await
                    .unwrap(),
                None
            );
            assert!(
                store
                    .load_input_states_strict(&runtime_id)
                    .await
                    .unwrap()
                    .is_empty()
            );
            assert!(
                store
                    .load_boundary_receipt(&runtime_id, &receipt.run_id, receipt.sequence)
                    .await
                    .unwrap()
                    .is_none()
            );
        }

        #[tokio::test]
        async fn atomic_apply_rejects_mismatched_session_store_key() {
            let (_dir, store) = temp_store();
            let runtime_id = runtime_id();
            let session = meerkat_core::Session::new();
            let wrong_session_id = meerkat_core::Session::new().id().clone();
            let snapshot = serde_json::to_vec(&session).unwrap();

            let err = store
                .atomic_apply(
                    &runtime_id,
                    Some(SerializedSessionSnapshot {
                        session_snapshot: snapshot.into(),
                    }),
                    RunBoundaryReceipt {
                        run_id: RunId(uuid::Uuid::new_v4()),
                        boundary: RunApplyBoundary::RunStart,
                        contributing_input_ids: vec![],
                        conversation_digest: None,
                        message_count: 0,
                        sequence: 0,
                    },
                    vec![input_state()],
                    Some(wrong_session_id),
                )
                .await
                .expect_err("mismatched session_store_key should fail");

            assert!(matches!(err, RuntimeStoreError::SessionKeyMismatch { .. }));
            assert!(
                store
                    .load_session_snapshot(&runtime_id)
                    .await
                    .unwrap()
                    .is_none()
            );
        }

        #[tokio::test]
        async fn commit_machine_lifecycle_persists_both_parts() {
            let (_dir, store) = temp_store();
            let runtime_id = runtime_id();
            let runtime_state = RuntimeState::Stopped;
            let binding = crate::store::MachineLifecycleBindingFacts::new(
                Some("rt:session:sqlite".to_string()),
                Some(11),
                None,
                Some("epoch-sqlite".to_string()),
            );
            store
                .commit_machine_lifecycle(
                    &runtime_id,
                    MachineLifecycleCommit::new_with_binding(
                        runtime_state,
                        binding.clone(),
                        crate::store::SupervisorAuthoritySnapshot::UnboundNoReceipt,
                    ),
                    &[input_state()],
                )
                .await
                .unwrap();

            assert!(
                crate::store::load_runtime_state(&store, &runtime_id)
                    .await
                    .unwrap()
                    .is_some()
            );
            let lifecycle = crate::store::load_machine_lifecycle(&store, &runtime_id)
                .await
                .unwrap()
                .expect("machine lifecycle snapshot");
            assert_eq!(lifecycle.runtime_state(), runtime_state);
            assert_eq!(lifecycle.binding(), &binding);
            assert_eq!(
                store
                    .load_input_states_strict(&runtime_id)
                    .await
                    .unwrap()
                    .len(),
                1
            );
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn concurrent_ops_initializers_return_one_canonical_snapshot() {
            let (_dir, first_store) = temp_store();
            let second_store = SqliteRuntimeStore::new(first_store.path().to_owned()).unwrap();
            let runtime_id = runtime_id();
            let registry = crate::ops_lifecycle::RuntimeOpsLifecycleRegistry::new();
            let first_candidate = registry
                .capture_persistence_snapshot(
                    meerkat_core::RuntimeEpochId::new(),
                    &meerkat_core::EpochCursorState::new(),
                )
                .unwrap();
            let second_candidate = registry
                .capture_persistence_snapshot(
                    meerkat_core::RuntimeEpochId::new(),
                    &meerkat_core::EpochCursorState::new(),
                )
                .unwrap();
            assert_ne!(first_candidate.epoch_id, second_candidate.epoch_id);

            let (first, second) = tokio::join!(
                first_store.initialize_ops_lifecycle_if_absent(&runtime_id, &first_candidate),
                second_store.initialize_ops_lifecycle_if_absent(&runtime_id, &second_candidate),
            );
            let first = first.unwrap();
            let second = second.unwrap();

            assert_eq!(first.epoch_id, second.epoch_id);
            assert_eq!(
                first_store
                    .load_ops_lifecycle(&runtime_id)
                    .await
                    .unwrap()
                    .expect("canonical snapshot")
                    .epoch_id,
                first.epoch_id
            );
        }

        #[tokio::test]
        async fn unregister_finalization_power_cuts_reopen_without_split_epoch_truth() {
            for (fault, commit_was_durable) in [(1_u8, false), (2_u8, false), (3_u8, true)] {
                let dir = TempDir::new().unwrap();
                let path = dir.path().join(format!("unregister-fault-{fault}.sqlite3"));
                let runtime_id = LogicalRuntimeId::new(format!("runtime-fault-{fault}"));
                let store = SqliteRuntimeStore::new(path.clone()).unwrap();

                store
                    .commit_machine_lifecycle(
                        &runtime_id,
                        MachineLifecycleCommit::new_with_binding(
                            RuntimeState::Idle,
                            crate::store::MachineLifecycleBindingFacts::new(
                                Some(format!("rt:session:fault-{fault}")),
                                Some(1),
                                Some(1),
                                Some(format!("epoch-{fault}")),
                            ),
                            crate::store::SupervisorAuthoritySnapshot::UnboundNoReceipt,
                        ),
                        &[],
                    )
                    .await
                    .unwrap();
                let stale_ops = crate::ops_lifecycle::RuntimeOpsLifecycleRegistry::new()
                    .capture_persistence_snapshot(
                        meerkat_core::RuntimeEpochId::new(),
                        &meerkat_core::EpochCursorState::new(),
                    )
                    .unwrap();
                store
                    .persist_ops_lifecycle(&runtime_id, &stale_ops)
                    .await
                    .unwrap();
                let retired_ops_epoch = stale_ops.epoch_id.clone();

                store.inject_unregister_finalization_fault(fault);
                let result = store
                    .commit_unregister_finalization(
                        &runtime_id,
                        crate::store::UnregisterFinalizationCommit::new(
                            MachineLifecycleCommit::new_with_binding(
                                RuntimeState::Stopped,
                                crate::store::MachineLifecycleBindingFacts::new(
                                    None, None, None, None,
                                ),
                                crate::store::SupervisorAuthoritySnapshot::UnboundNoReceipt,
                            ),
                            vec![],
                            retired_ops_epoch.clone(),
                            crate::meerkat_machine::DeleteOpsFinalizationAuthority::for_store_test(
                            ),
                        ),
                    )
                    .await;
                if commit_was_durable {
                    // The final former kill window is after COMMIT and before
                    // the caller acts on the acknowledgement. Discarding the
                    // successful result and reopening models that process
                    // death: no shell rollback gets to run.
                    result.expect("post-commit kill window has durable finalization");
                } else {
                    let error = result
                        .expect_err("pre-commit unregister finalization interruption must surface");
                    assert!(error.to_string().contains("synthetic"));
                }
                drop(store);

                let reopened = SqliteRuntimeStore::new(path.clone()).unwrap();
                let recovered_state = crate::store::load_runtime_state(&reopened, &runtime_id)
                    .await
                    .unwrap();
                let recovered_ops = reopened.load_ops_lifecycle(&runtime_id).await.unwrap();
                if commit_was_durable {
                    assert_eq!(recovered_state, Some(RuntimeState::Stopped));
                    assert!(recovered_ops.is_none());
                } else {
                    assert_eq!(recovered_state, Some(RuntimeState::Idle));
                    assert!(recovered_ops.is_some());
                }
                assert!(
                    recovered_state != Some(RuntimeState::Stopped) || recovered_ops.is_none(),
                    "reopen must never expose terminal lifecycle with the stale ops epoch"
                );

                // Both rollback-before-commit and crash-after-commit reopen
                // states converge under the same idempotent retry.
                reopened
                    .commit_unregister_finalization(
                        &runtime_id,
                        crate::store::UnregisterFinalizationCommit::new(
                            MachineLifecycleCommit::new_with_binding(
                                RuntimeState::Stopped,
                                crate::store::MachineLifecycleBindingFacts::new(
                                    None, None, None, None,
                                ),
                                crate::store::SupervisorAuthoritySnapshot::UnboundNoReceipt,
                            ),
                            vec![],
                            retired_ops_epoch.clone(),
                            crate::meerkat_machine::DeleteOpsFinalizationAuthority::for_store_test(
                            ),
                        ),
                    )
                    .await
                    .unwrap();
                drop(reopened);

                let reopened_after_retry = SqliteRuntimeStore::new(path).unwrap();
                assert_eq!(
                    crate::store::load_runtime_state(&reopened_after_retry, &runtime_id)
                        .await
                        .unwrap(),
                    Some(RuntimeState::Stopped)
                );
                assert!(
                    reopened_after_retry
                        .load_ops_lifecycle(&runtime_id)
                        .await
                        .unwrap()
                        .is_none()
                );
                let late_error = reopened_after_retry
                    .persist_ops_lifecycle(&runtime_id, &stale_ops)
                    .await
                    .expect_err("reopen must retain the exact retired-epoch fence");
                assert!(matches!(
                    late_error,
                    RuntimeStoreError::OpsLifecycleEpochRetired { epoch_id, .. }
                        if epoch_id == retired_ops_epoch
                ));
                assert!(matches!(
                    reopened_after_retry
                        .initialize_ops_lifecycle_if_absent(&runtime_id, &stale_ops)
                        .await
                        .expect_err("reopen initialization must honor the retired-epoch fence"),
                    RuntimeStoreError::OpsLifecycleEpochRetired { epoch_id, .. }
                        if epoch_id == retired_ops_epoch
                ));
                assert!(
                    reopened_after_retry
                        .load_ops_lifecycle(&runtime_id)
                        .await
                        .unwrap()
                        .is_none()
                );
            }
        }

        #[tokio::test]
        async fn delayed_old_epoch_finalizer_preserves_new_epoch_across_reopen() {
            let dir = TempDir::new().unwrap();
            let path = dir.path().join("old-finalizer-new-epoch.sqlite3");
            let runtime_id = LogicalRuntimeId::new("runtime-old-finalizer-new-epoch");
            let store = SqliteRuntimeStore::new(path.clone()).unwrap();
            let registry = crate::ops_lifecycle::RuntimeOpsLifecycleRegistry::new();
            let old_ops = registry
                .capture_persistence_snapshot(
                    meerkat_core::RuntimeEpochId::new(),
                    &meerkat_core::EpochCursorState::new(),
                )
                .unwrap();
            let new_ops = registry
                .capture_persistence_snapshot(
                    meerkat_core::RuntimeEpochId::new(),
                    &meerkat_core::EpochCursorState::new(),
                )
                .unwrap();
            store
                .persist_ops_lifecycle(&runtime_id, &old_ops)
                .await
                .unwrap();
            store
                .persist_ops_lifecycle(&runtime_id, &new_ops)
                .await
                .unwrap();

            store.inject_unregister_finalization_fault(3);
            store
                .commit_unregister_finalization(
                    &runtime_id,
                    crate::store::UnregisterFinalizationCommit::new(
                        MachineLifecycleCommit::new_with_binding(
                            RuntimeState::Stopped,
                            crate::store::MachineLifecycleBindingFacts::new(None, None, None, None),
                            crate::store::SupervisorAuthoritySnapshot::UnboundNoReceipt,
                        ),
                        vec![],
                        old_ops.epoch_id.clone(),
                        crate::meerkat_machine::DeleteOpsFinalizationAuthority::for_store_test(),
                    ),
                )
                .await
                .unwrap();
            drop(store);

            let reopened = SqliteRuntimeStore::new(path).unwrap();
            assert_eq!(
                reopened
                    .load_ops_lifecycle(&runtime_id)
                    .await
                    .unwrap()
                    .expect("new epoch row must survive delayed old finalization")
                    .epoch_id,
                new_ops.epoch_id
            );
            assert!(matches!(
                reopened
                    .persist_ops_lifecycle(&runtime_id, &old_ops)
                    .await
                    .expect_err("old epoch tombstone must survive reopen"),
                RuntimeStoreError::OpsLifecycleEpochRetired { .. }
            ));
            reopened
                .persist_ops_lifecycle(&runtime_id, &new_ops)
                .await
                .unwrap();
        }

        #[tokio::test]
        async fn unregister_finalization_commits_lifecycle_inputs_and_ops_delete_together() {
            let (_dir, store) = temp_store();
            let runtime_id = runtime_id();
            let ops_snapshot = crate::ops_lifecycle::RuntimeOpsLifecycleRegistry::new()
                .capture_persistence_snapshot(
                    meerkat_core::RuntimeEpochId::new(),
                    &meerkat_core::EpochCursorState::new(),
                )
                .unwrap();
            store
                .persist_ops_lifecycle(&runtime_id, &ops_snapshot)
                .await
                .unwrap();
            let finalization = crate::store::UnregisterFinalizationCommit::new(
                MachineLifecycleCommit::new_with_binding(
                    RuntimeState::Idle,
                    crate::store::MachineLifecycleBindingFacts::default(),
                    crate::store::SupervisorAuthoritySnapshot::UnboundNoReceipt,
                ),
                vec![input_state()],
                ops_snapshot.epoch_id.clone(),
                crate::meerkat_machine::DeleteOpsFinalizationAuthority::for_store_test(),
            );

            store
                .commit_unregister_finalization(&runtime_id, finalization)
                .await
                .unwrap();

            let lifecycle = crate::store::load_machine_lifecycle(&store, &runtime_id)
                .await
                .unwrap()
                .expect("terminal lifecycle");
            assert_eq!(lifecycle.runtime_state(), RuntimeState::Idle);
            assert_eq!(
                lifecycle.binding(),
                &crate::store::MachineLifecycleBindingFacts::default()
            );
            assert_eq!(
                store
                    .load_input_states_strict(&runtime_id)
                    .await
                    .unwrap()
                    .len(),
                1
            );
            assert!(
                store
                    .load_ops_lifecycle(&runtime_id)
                    .await
                    .unwrap()
                    .is_none()
            );
        }

        #[tokio::test]
        async fn unregister_finalization_rolls_back_lifecycle_and_inputs_when_delete_fails() {
            let (_dir, store) = temp_store();
            let runtime_id = runtime_id();
            let ops_snapshot = crate::ops_lifecycle::RuntimeOpsLifecycleRegistry::new()
                .capture_persistence_snapshot(
                    meerkat_core::RuntimeEpochId::new(),
                    &meerkat_core::EpochCursorState::new(),
                )
                .unwrap();
            store
                .persist_ops_lifecycle(&runtime_id, &ops_snapshot)
                .await
                .unwrap();
            let conn = open_runtime_connection(&store.path).unwrap();
            conn.execute_batch(
                r"
                CREATE TRIGGER reject_unregister_ops_delete
                BEFORE DELETE ON runtime_ops_lifecycle
                BEGIN
                    SELECT RAISE(ABORT, 'synthetic unregister delete failure');
                END;
                ",
            )
            .unwrap();
            let finalization = crate::store::UnregisterFinalizationCommit::new(
                MachineLifecycleCommit::new_with_binding(
                    RuntimeState::Idle,
                    crate::store::MachineLifecycleBindingFacts::default(),
                    crate::store::SupervisorAuthoritySnapshot::UnboundNoReceipt,
                ),
                vec![input_state()],
                ops_snapshot.epoch_id.clone(),
                crate::meerkat_machine::DeleteOpsFinalizationAuthority::for_store_test(),
            );

            store
                .commit_unregister_finalization(&runtime_id, finalization)
                .await
                .expect_err("delete failure must abort the whole finalization transaction");

            assert!(
                crate::store::load_machine_lifecycle(&store, &runtime_id)
                    .await
                    .unwrap()
                    .is_none(),
                "failed delete must roll back terminal lifecycle publication"
            );
            assert!(
                store
                    .load_input_states_strict(&runtime_id)
                    .await
                    .unwrap()
                    .is_empty()
            );
            assert!(
                store
                    .load_ops_lifecycle(&runtime_id)
                    .await
                    .unwrap()
                    .is_some(),
                "failed delete must retain the prior ops snapshot"
            );
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn cancelled_unregister_finalization_has_no_detached_sqlite_write() {
            let (_dir, store) = temp_store();
            let store = std::sync::Arc::new(store);
            let runtime_id = runtime_id();
            let ops_snapshot = crate::ops_lifecycle::RuntimeOpsLifecycleRegistry::new()
                .capture_persistence_snapshot(
                    meerkat_core::RuntimeEpochId::new(),
                    &meerkat_core::EpochCursorState::new(),
                )
                .unwrap();
            store
                .persist_ops_lifecycle(&runtime_id, &ops_snapshot)
                .await
                .unwrap();
            let finalization = crate::store::UnregisterFinalizationCommit::new(
                MachineLifecycleCommit::new_with_binding(
                    RuntimeState::Idle,
                    crate::store::MachineLifecycleBindingFacts::default(),
                    crate::store::SupervisorAuthoritySnapshot::UnboundNoReceipt,
                ),
                vec![input_state()],
                ops_snapshot.epoch_id.clone(),
                crate::meerkat_machine::DeleteOpsFinalizationAuthority::for_store_test(),
            );

            // Hold SQLite's write reservation on a dedicated thread so the
            // finalizer is known to be inside its first, non-yielding poll when
            // cancellation arrives.
            let (locked_tx, locked_rx) = std::sync::mpsc::sync_channel(1);
            let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
            let blocker_path = store.path.clone();
            let blocker = std::thread::spawn(move || {
                let mut blocker_conn = open_runtime_connection(&blocker_path).unwrap();
                let blocker_tx = begin_runtime_transaction(&mut blocker_conn).unwrap();
                locked_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                drop(blocker_tx);
            });
            locked_rx
                .recv_timeout(std::time::Duration::from_secs(1))
                .expect("blocking writer should acquire SQLite reservation");
            let entered = std::sync::Arc::new(tokio::sync::Notify::new());
            let mut finalizer = tokio::spawn({
                let store = std::sync::Arc::clone(&store);
                let runtime_id = runtime_id.clone();
                let entered = std::sync::Arc::clone(&entered);
                async move {
                    entered.notify_one();
                    store
                        .commit_unregister_finalization(&runtime_id, finalization)
                        .await
                }
            });
            tokio::time::timeout(std::time::Duration::from_secs(1), entered.notified())
                .await
                .expect("finalizer task should start");
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;

            finalizer.abort();
            assert!(
                tokio::time::timeout(std::time::Duration::from_millis(100), &mut finalizer,)
                    .await
                    .is_err(),
                "cancellation must not return while a SQLite finalization poll can still mutate; a detached spawn_blocking write violates this fence"
            );

            release_tx.send(()).unwrap();
            blocker.join().unwrap();
            let joined = tokio::time::timeout(std::time::Duration::from_secs(2), finalizer)
                .await
                .expect("finalizer should finish after the competing writer releases");
            match joined {
                Ok(Ok(())) => {}
                Err(error) if error.is_cancelled() => {}
                Ok(Err(error)) => panic!("finalizer failed after lock release: {error}"),
                Err(error) => panic!("finalizer task failed unexpectedly: {error}"),
            }

            let lifecycle = crate::store::load_machine_lifecycle(store.as_ref(), &runtime_id)
                .await
                .unwrap()
                .expect("the in-progress atomic poll must finish before cancellation returns");
            assert_eq!(lifecycle.runtime_state(), RuntimeState::Idle);
            assert_eq!(
                store
                    .load_input_states_strict(&runtime_id)
                    .await
                    .unwrap()
                    .len(),
                1
            );
            assert!(
                store
                    .load_ops_lifecycle(&runtime_id)
                    .await
                    .unwrap()
                    .is_none(),
                "cancellation may observe the complete transaction, never a delayed or split write"
            );
        }

        #[tokio::test]
        async fn legacy_runtime_state_row_is_not_lifecycle_authority() {
            let (_dir, store) = temp_store();
            let runtime_id = runtime_id();
            let legacy_state_json = serde_json::to_vec(&RuntimeState::Retired).unwrap();
            let conn = open_runtime_connection(&store.path).unwrap();
            conn.execute(
                r"
                INSERT INTO runtime_states (runtime_id, runtime_state_json)
                VALUES (?1, ?2)
                ",
                params![runtime_id_text(&runtime_id), legacy_state_json],
            )
            .unwrap();

            assert!(matches!(
                crate::store::load_runtime_state(&store, &runtime_id).await,
                Err(RuntimeStoreError::ReadFailed(_))
            ));
            assert!(matches!(
                crate::store::load_machine_lifecycle(&store, &runtime_id).await,
                Err(RuntimeStoreError::ReadFailed(_))
            ));
        }

        #[tokio::test]
        async fn legacy_machine_lifecycle_snapshot_row_is_not_lifecycle_authority() {
            let (_dir, store) = temp_store();
            let runtime_id = runtime_id();
            let runtime_state = RuntimeState::Retired;
            let legacy_snapshot_json = serde_json::to_vec(&serde_json::json!({
                "runtime_state": runtime_state,
                "binding": {
                    "agent_runtime_id": "rt:session:legacy",
                    "fence_token": 23,
                    "runtime_generation": 7,
                    "runtime_epoch_id": "epoch-legacy"
                }
            }))
            .unwrap();
            let conn = open_runtime_connection(&store.path).unwrap();
            conn.execute(
                r"
                INSERT INTO runtime_states (runtime_id, runtime_state_json)
                VALUES (?1, ?2)
                ",
                params![runtime_id_text(&runtime_id), legacy_snapshot_json],
            )
            .unwrap();

            assert!(matches!(
                crate::store::load_runtime_state(&store, &runtime_id).await,
                Err(RuntimeStoreError::ReadFailed(_))
            ));
            assert!(matches!(
                crate::store::load_machine_lifecycle(&store, &runtime_id).await,
                Err(RuntimeStoreError::ReadFailed(_))
            ));
        }

        #[test]
        fn receipt_sequence_encoding_is_injective_across_i64_boundary() {
            // Distinct u64 sequences straddling the i64::MAX boundary must map
            // to distinct durable keys (no saturation aliasing) and decode
            // symmetrically.
            let probes: [u64; 6] = [
                0,
                1,
                i64::MAX as u64 - 1,
                i64::MAX as u64,
                i64::MAX as u64 + 1,
                u64::MAX,
            ];
            let mut seen = std::collections::HashSet::new();
            for sequence in probes {
                let encoded = encode_receipt_sequence(sequence);
                assert!(
                    seen.insert(encoded),
                    "sequence {sequence} aliased an already-stored key {encoded}"
                );
                assert_eq!(
                    decode_receipt_sequence(encoded),
                    sequence,
                    "round-trip failed for sequence {sequence}"
                );
            }
        }

        fn receipt_with_sequence(run_id: RunId, sequence: u64) -> RunBoundaryReceipt {
            RunBoundaryReceipt {
                run_id,
                boundary: RunApplyBoundary::RunStart,
                contributing_input_ids: vec![],
                conversation_digest: Some("machine-owned-digest".to_string()),
                message_count: 1,
                sequence,
            }
        }

        #[tokio::test]
        async fn boundary_receipts_straddling_i64_max_persist_and_read_distinctly() {
            let (_dir, store) = temp_store();
            let runtime_id = runtime_id();
            let run_id = RunId(uuid::Uuid::new_v4());
            let low = receipt_with_sequence(run_id.clone(), i64::MAX as u64);
            let high = receipt_with_sequence(run_id.clone(), i64::MAX as u64 + 1);

            store
                .atomic_apply(&runtime_id, None, low.clone(), vec![], None)
                .await
                .unwrap();
            store
                .atomic_apply(&runtime_id, None, high.clone(), vec![], None)
                .await
                .unwrap();

            // Two distinct sequences must produce two distinct durable rows,
            // not collapse onto one i64::MAX key.
            assert_eq!(receipt_row_count(&store), 2);
            assert_eq!(
                store
                    .load_boundary_receipt(&runtime_id, &run_id, low.sequence)
                    .await
                    .unwrap(),
                Some(low)
            );
            assert_eq!(
                store
                    .load_boundary_receipt(&runtime_id, &run_id, high.sequence)
                    .await
                    .unwrap(),
                Some(high)
            );
        }

        /// A session blob without the mandatory envelope `version` byte (the
        /// pre-typed-owner v0 shape) must FAIL CLOSED through the runtime-store
        /// read helper — it never silently defaults or upgrades on read.
        #[test]
        fn deserialize_persisted_session_rejects_missing_version_row() {
            let v0_blob = serde_json::json!({
                "id": "00000000-0000-0000-0000-000000000012",
                "messages": [],
                "created_at": { "secs_since_epoch": 1727784000, "nanos_since_epoch": 0 },
                "updated_at": { "secs_since_epoch": 1727784000, "nanos_since_epoch": 0 },
                "metadata": {}
            });
            let bytes = serde_json::to_vec(&v0_blob).unwrap();

            let err = deserialize_persisted_session(&bytes)
                .expect_err("missing-version session row must fail closed");
            assert!(
                err.to_string().contains("version"),
                "unexpected error: {err}"
            );
        }

        /// A session blob carrying the retired legacy envelope version (v1)
        /// must FAIL CLOSED with the typed generated-authority rejection.
        #[test]
        fn deserialize_persisted_session_rejects_legacy_v1_version_row() {
            let v1_blob = serde_json::json!({
                "version": 1,
                "id": "00000000-0000-0000-0000-000000000012",
                "messages": [],
                "created_at": { "secs_since_epoch": 1727784000, "nanos_since_epoch": 0 },
                "updated_at": { "secs_since_epoch": 1727784000, "nanos_since_epoch": 0 },
                "metadata": {}
            });
            let bytes = serde_json::to_vec(&v1_blob).unwrap();

            let err = deserialize_persisted_session(&bytes)
                .expect_err("legacy v1 session row must fail closed");
            assert!(
                err.to_string()
                    .contains("generated session persistence version authority rejected"),
                "unexpected error: {err}"
            );
        }

        /// End-to-end: a raw v0 session row written directly into the
        /// `runtime_session_snapshots` table must FAIL the runtime-store
        /// `previous`-snapshot read path (here exercised by
        /// `commit_session_snapshot`) — the read path never silently accepts a
        /// pre-version row.
        #[tokio::test]
        async fn runtime_store_read_path_rejects_v0_session_row() {
            let (_dir, store) = temp_store();
            let runtime_id = runtime_id();

            // Write a v0-shaped row (no envelope `version`) directly into the
            // durable table.
            let v0_blob = serde_json::json!({
                "id": "00000000-0000-0000-0000-000000000012",
                "messages": [],
                "created_at": { "secs_since_epoch": 1727784000, "nanos_since_epoch": 0 },
                "updated_at": { "secs_since_epoch": 1727784000, "nanos_since_epoch": 0 },
                "metadata": {}
            });
            let v0_bytes = serde_json::to_vec(&v0_blob).unwrap();
            {
                let mut conn = open_runtime_connection(store.path()).unwrap();
                let tx = begin_runtime_transaction(&mut conn).unwrap();
                upsert_runtime_snapshot(&tx, &runtime_id, &v0_bytes).unwrap();
                tx.commit().unwrap();
            }

            // A subsequent boundary commit reads the persisted previous row;
            // the v0 row must surface a read failure, not a silent default.
            let mut incoming = Session::new();
            incoming.push(Message::User(UserMessage::text("hello".to_string())));
            let err = store
                .commit_session_snapshot(
                    &runtime_id,
                    SerializedSessionSnapshot {
                        session_snapshot: serde_json::to_vec(&incoming).unwrap().into(),
                    },
                )
                .await
                .expect_err("v0 previous row must fail the read path closed");
            assert!(
                err.to_string().contains("version"),
                "unexpected error: {err}"
            );
        }

        /// A `StoredInputState` row without the mandatory
        /// `stored_input_state_version` byte (the pre-version v0 shape) written
        /// directly into `runtime_input_states` must FAIL CLOSED on read.
        #[tokio::test]
        async fn runtime_store_read_path_rejects_v0_input_state_row() {
            let (_dir, store) = temp_store();
            let runtime_id = runtime_id();

            let input_id = InputId::new();
            let bundle = StoredInputState::new_accepted(input_id.clone());
            let mut row = serde_json::to_value(&bundle).unwrap();
            // Drop the version byte to simulate a pre-version (v0) row.
            row.as_object_mut()
                .unwrap()
                .remove("stored_input_state_version");
            let row_bytes = serde_json::to_vec(&row).unwrap();
            {
                let mut conn = open_runtime_connection(store.path()).unwrap();
                let tx = begin_runtime_transaction(&mut conn).unwrap();
                tx.execute(
                    r"
                    INSERT INTO runtime_input_states (runtime_id, input_id, state_json)
                    VALUES (?1, ?2, ?3)
                    ",
                    params![
                        runtime_id_text(&runtime_id),
                        input_id.0.to_string(),
                        row_bytes
                    ],
                )
                .unwrap();
                tx.commit().unwrap();
            }

            let err = store
                .load_input_state(&runtime_id, &input_id)
                .await
                .expect_err("v0 input-state row must fail the read path closed");
            assert!(
                err.to_string().contains("stored_input_state_version"),
                "unexpected error: {err}"
            );

            let err = store
                .load_input_states_strict(&runtime_id)
                .await
                .expect_err("v0 input-state row must fail the bulk read path closed");
            assert!(
                err.to_string().contains("stored_input_state_version"),
                "unexpected error: {err}"
            );
        }

        /// The projection-quarantine marker recorded by
        /// `clear_session_snapshot_if_current` is durable: a FRESH store opened
        /// on the same path (simulating a process restart) still reports the
        /// runtime as quarantined. A subsequent live snapshot write clears it.
        #[tokio::test]
        async fn projection_quarantine_marker_survives_restart_and_clears_on_write() {
            let dir = TempDir::new().unwrap();
            let path = dir.path().join("sessions.sqlite3");
            let runtime_id = runtime_id();

            let rejected = session_with_user("rejected runtime turn");
            let rejected_snapshot = serde_json::to_vec(&rejected).unwrap();

            // Commit the snapshot, then conditionally clear it: this is the
            // fail-closed quarantine path.
            {
                let store = SqliteRuntimeStore::new(path.clone()).unwrap();
                assert!(
                    !store
                        .is_runtime_projection_quarantined(&runtime_id)
                        .await
                        .unwrap(),
                    "a fresh runtime must not start quarantined"
                );
                store
                    .commit_session_snapshot(
                        &runtime_id,
                        SerializedSessionSnapshot {
                            session_snapshot: rejected_snapshot.clone().into(),
                        },
                    )
                    .await
                    .unwrap();
                assert!(
                    store
                        .clear_session_snapshot_if_current(&runtime_id, &rejected_snapshot)
                        .await
                        .unwrap(),
                    "matching snapshot must be cleared"
                );
                assert!(
                    store
                        .is_runtime_projection_quarantined(&runtime_id)
                        .await
                        .unwrap(),
                    "clearing the rejected snapshot must record the quarantine marker"
                );
            }

            // Reopen on the same path: the durable marker must still be present.
            {
                let restarted = SqliteRuntimeStore::new(path.clone()).unwrap();
                assert!(
                    restarted
                        .is_runtime_projection_quarantined(&runtime_id)
                        .await
                        .unwrap(),
                    "quarantine marker must survive a simulated process restart"
                );

                // A live snapshot write reclaims runtime authority and clears
                // the marker atomically.
                let revived = session_with_user("revived runtime turn");
                restarted
                    .commit_session_snapshot(
                        &runtime_id,
                        SerializedSessionSnapshot {
                            session_snapshot: serde_json::to_vec(&revived).unwrap().into(),
                        },
                    )
                    .await
                    .unwrap();
                assert!(
                    !restarted
                        .is_runtime_projection_quarantined(&runtime_id)
                        .await
                        .unwrap(),
                    "a live snapshot write must clear the quarantine marker"
                );
            }

            // And the cleared state is itself durable across another restart.
            {
                let restarted_again = SqliteRuntimeStore::new(path).unwrap();
                assert!(
                    !restarted_again
                        .is_runtime_projection_quarantined(&runtime_id)
                        .await
                        .unwrap(),
                    "cleared quarantine marker must stay cleared across restart"
                );
            }
        }

        /// Upgrade-carry: JSON payload columns written as TEXT by an external
        /// host (SQLite affinity keeps whatever type a writer bound) must
        /// still read; one legacy row must not fail every load. Same contract
        /// as the meerkat-store `JsonColumnBytes` boundary this store reuses.
        #[tokio::test]
        async fn legacy_text_json_columns_still_read() {
            let (_dir, store) = temp_store();
            let runtime_id = runtime_id();
            let session = session_with_one_turn();
            let snapshot = serde_json::to_vec(&session).unwrap();
            store
                .commit_session_snapshot(
                    &runtime_id,
                    SerializedSessionSnapshot {
                        session_snapshot: snapshot.clone().into(),
                    },
                )
                .await
                .unwrap();

            {
                let conn = open_runtime_connection(store.path()).unwrap();
                let changed = conn
                    .execute(
                        "UPDATE runtime_session_snapshots                          SET session_snapshot = CAST(session_snapshot AS TEXT)",
                        [],
                    )
                    .unwrap();
                assert!(changed > 0, "expected snapshot rows to degrade");
            }

            let carried = store
                .load_session_snapshot(&runtime_id)
                .await
                .expect("load over TEXT snapshot must not fail")
                .expect("snapshot present");
            assert_eq!(
                carried.as_ref(),
                &snapshot,
                "TEXT snapshot bytes must round-trip"
            );
        }

        #[tokio::test]
        async fn mob_host_binding_rows_insert_cas_delete_and_list() {
            let (_dir, store) = temp_store();

            // Absent row: load None, list empty, CAS/delete no-ops.
            assert!(
                store
                    .load_mob_host_binding("mob-a")
                    .await
                    .unwrap()
                    .is_none()
            );
            assert!(store.list_mob_host_bindings().await.unwrap().is_empty());
            assert!(
                !store
                    .compare_and_put_mob_host_binding("mob-a", b"old", b"new")
                    .await
                    .unwrap()
            );
            assert!(
                !store
                    .delete_mob_host_binding("mob-a", b"old")
                    .await
                    .unwrap()
            );

            // Insert-if-absent: first wins, second is refused.
            assert!(
                store
                    .put_mob_host_binding_if_absent("mob-a", b"record-1")
                    .await
                    .unwrap()
            );
            assert!(
                !store
                    .put_mob_host_binding_if_absent("mob-a", b"record-other")
                    .await
                    .unwrap()
            );
            assert_eq!(
                store
                    .load_mob_host_binding("mob-a")
                    .await
                    .unwrap()
                    .as_deref(),
                Some(b"record-1".as_slice())
            );

            // CAS honours the expected blob.
            assert!(
                !store
                    .compare_and_put_mob_host_binding("mob-a", b"stale", b"record-2")
                    .await
                    .unwrap()
            );
            assert!(
                store
                    .compare_and_put_mob_host_binding("mob-a", b"record-1", b"record-2")
                    .await
                    .unwrap()
            );

            // List is keyed and ordered.
            assert!(
                store
                    .put_mob_host_binding_if_absent("mob-b", b"record-b")
                    .await
                    .unwrap()
            );
            let rows = store.list_mob_host_bindings().await.unwrap();
            assert_eq!(
                rows,
                vec![
                    ("mob-a".to_string(), b"record-2".to_vec()),
                    ("mob-b".to_string(), b"record-b".to_vec()),
                ]
            );

            // Delete honours the expected blob.
            assert!(
                !store
                    .delete_mob_host_binding("mob-a", b"stale")
                    .await
                    .unwrap()
            );
            assert!(
                store
                    .delete_mob_host_binding("mob-a", b"record-2")
                    .await
                    .unwrap()
            );
            assert!(
                store
                    .load_mob_host_binding("mob-a")
                    .await
                    .unwrap()
                    .is_none()
            );
        }

        #[tokio::test]
        async fn mob_host_revoke_is_atomic_durable_and_fresh_bind_supersedes_receipt() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("runtime.sqlite3");

            {
                let store = SqliteRuntimeStore::new(path.clone()).unwrap();
                assert!(
                    store
                        .put_mob_host_binding_if_absent("mob-r", b"binding-v1")
                        .await
                        .unwrap()
                );

                // A stale expected binding changes neither region.
                assert!(
                    !store
                        .revoke_mob_host_binding("mob-r", b"stale", b"receipt-v1")
                        .await
                        .unwrap()
                );
                assert_eq!(
                    store
                        .load_mob_host_binding("mob-r")
                        .await
                        .unwrap()
                        .as_deref(),
                    Some(b"binding-v1".as_slice())
                );
                assert!(
                    store
                        .load_mob_host_revocation("mob-r")
                        .await
                        .unwrap()
                        .is_none(),
                    "a failed revoke CAS must not publish a success receipt"
                );

                assert!(
                    store
                        .revoke_mob_host_binding("mob-r", b"binding-v1", b"receipt-v1")
                        .await
                        .unwrap()
                );
                assert!(
                    store
                        .load_mob_host_binding("mob-r")
                        .await
                        .unwrap()
                        .is_none(),
                    "the durable terminal contains no active binding"
                );
                assert_eq!(
                    store.list_mob_host_revocations().await.unwrap(),
                    vec![("mob-r".to_string(), b"receipt-v1".to_vec())]
                );
            }

            // Both halves survive reopen: no binding can revive member rows,
            // while the exact retry receipt remains available.
            {
                let restarted = SqliteRuntimeStore::new(path.clone()).unwrap();
                assert!(
                    restarted
                        .load_mob_host_binding("mob-r")
                        .await
                        .unwrap()
                        .is_none()
                );
                assert_eq!(
                    restarted
                        .load_mob_host_revocation("mob-r")
                        .await
                        .unwrap()
                        .as_deref(),
                    Some(b"receipt-v1".as_slice())
                );

                // Replacement bind and old-receipt removal share one
                // transaction, so a delayed old revoke cannot replay across
                // a successful replacement ceremony.
                assert!(
                    restarted
                        .put_mob_host_binding_if_absent("mob-r", b"binding-v2")
                        .await
                        .unwrap()
                );
                assert!(
                    restarted
                        .load_mob_host_revocation("mob-r")
                        .await
                        .unwrap()
                        .is_none()
                );
                assert_eq!(
                    restarted
                        .load_mob_host_binding("mob-r")
                        .await
                        .unwrap()
                        .as_deref(),
                    Some(b"binding-v2".as_slice())
                );
            }
        }

        #[tokio::test]
        async fn lifecycle_observation_and_cas_survive_sqlite_reopen() {
            let (dir, store) = temp_store();
            let path = dir.path().join("sessions.sqlite3");
            let runtime_id = LogicalRuntimeId::new("sqlite-lifecycle-cas");
            assert_eq!(
                store.observe_machine_lifecycle(&runtime_id).await.unwrap(),
                MachineLifecycleObservation::Missing
            );

            let MachineLifecycleCasOutcome::Applied { version } = store
                .compare_and_swap_machine_lifecycle(
                    &runtime_id,
                    MachineLifecycleExpectedVersion::Missing,
                    lifecycle_commit(&runtime_id, RuntimeState::Idle, 11, 4),
                )
                .await
                .unwrap()
            else {
                panic!("missing SQLite lifecycle row must be inserted");
            };
            drop(store);

            let reopened = SqliteRuntimeStore::new(&path).unwrap();
            let observed = reopened
                .observe_machine_lifecycle(&runtime_id)
                .await
                .unwrap();
            let MachineLifecycleObservation::Decoded {
                record,
                version: observed_version,
            } = &observed
            else {
                panic!("reopened lifecycle row must decode");
            };
            assert_eq!(observed_version, &version);
            assert_eq!(record.runtime_state(), Some(RuntimeState::Idle));
            assert_eq!(record.binding().fence_token(), Some(11));

            assert_eq!(
                reopened
                    .compare_and_swap_machine_lifecycle(
                        &runtime_id,
                        MachineLifecycleExpectedVersion::Missing,
                        lifecycle_commit(&runtime_id, RuntimeState::Stopped, 12, 5),
                    )
                    .await
                    .unwrap(),
                MachineLifecycleCasOutcome::Conflict { current: observed }
            );
        }

        #[tokio::test]
        async fn sqlite_malformed_lifecycle_repair_is_always_blocked() {
            let (dir, store) = temp_store();
            let path = dir.path().join("sessions.sqlite3");
            let runtime_id = LogicalRuntimeId::new("sqlite-malformed-lifecycle");
            let raw = serde_json::to_vec(&serde_json::json!({
                "record_version": crate::store::MACHINE_LIFECYCLE_STORE_RECORD_VERSION,
                "runtime_state": "idle",
                "binding": {
                    "agent_runtime_id": runtime_id.0.clone(),
                    "fence_token": 17,
                    "runtime_generation": 8,
                    "runtime_epoch_id": "epoch-8"
                },
                "current_run_id": null,
                "pre_run_phase": null,
                "unregister_progress": null
            }))
            .unwrap();
            let conn = Connection::open(&path).unwrap();
            conn.execute(
                "INSERT INTO runtime_states (runtime_id, runtime_state_json) VALUES (?1, ?2)",
                params![runtime_id.0.clone(), raw.clone()],
            )
            .unwrap();
            drop(conn);

            let observed = store.observe_machine_lifecycle(&runtime_id).await.unwrap();
            let MachineLifecycleObservation::Malformed { version, .. } = observed else {
                panic!("incomplete SQLite lifecycle row must remain malformed evidence");
            };
            assert!(matches!(
                store
                    .compare_and_swap_machine_lifecycle(
                        &runtime_id,
                        MachineLifecycleExpectedVersion::Version(version.clone()),
                        lifecycle_commit(&runtime_id, RuntimeState::Idle, 16, 8),
                    )
                    .await
                    .expect_err("repair must not lower SQLite lifecycle fencing"),
                RuntimeStoreError::MachineLifecycleRepairBlocked { .. }
            ));
            assert_eq!(
                raw_fixture_row(&path, "runtime_states", "runtime_state_json", &runtime_id.0),
                raw
            );

            assert!(matches!(
                store
                    .compare_and_swap_machine_lifecycle(
                        &runtime_id,
                        MachineLifecycleExpectedVersion::Version(version),
                        lifecycle_commit(&runtime_id, RuntimeState::Idle, 18, 9),
                    )
                    .await
                    .expect_err("malformed SQLite bytes are not repair authority"),
                RuntimeStoreError::MachineLifecycleRepairBlocked { .. }
            ));
            drop(store);
            assert_eq!(
                raw_fixture_row(&path, "runtime_states", "runtime_state_json", &runtime_id.0),
                raw
            );
        }

        // ── upgrade/rollback ledger contract for the delivery domain ──────
        //
        // Head-canonical runtime authority intentionally advances the released
        // v1 runtime domain to v2: older binaries do not understand the frozen-BLOB
        // ownership split and must refuse rather than resume writing it.
        // Durable delivery remains a separate lazily provisioned domain;
        // merely opening or reading a realm never stamps that domain.

        #[test]
        fn opening_the_runtime_store_stamps_current_runtime_store_only() {
            let (_dir, store) = temp_store();
            let conn = Connection::open(store.path()).unwrap();
            assert_eq!(
                meerkat_sqlite::domain_version(&conn, "runtime-store").unwrap(),
                Some(2),
                "runtime-store must install the complete head-canonical authority contract"
            );
            assert_eq!(
                meerkat_sqlite::domain_version(&conn, "runtime-delivery").unwrap(),
                None,
                "opening a realm must not stamp the delivery domain"
            );
            for (object_type, object_name) in [
                ("table", "runtime_session_authority"),
                ("table", "runtime_head_canonical_provisional_tails"),
                ("trigger", "runtime_session_authority_after_delete"),
                ("table", "runtime_head_canonical_profile_pin"),
                ("table", "runtime_head_canonical_activations"),
                ("table", "runtime_head_canonical_activation_queue"),
                ("trigger", "runtime_head_canonical_profile_pin_no_update"),
                ("trigger", "runtime_head_canonical_profile_pin_no_delete"),
                (
                    "trigger",
                    "runtime_session_snapshots_hc_activation_no_insert",
                ),
                (
                    "trigger",
                    "runtime_session_snapshots_hc_activation_no_update",
                ),
                (
                    "trigger",
                    "runtime_session_snapshots_hc_activation_no_delete",
                ),
                ("index", "idx_runtime_input_states_recovery_nonterminal"),
                ("table", "runtime_input_set_revisions"),
                ("table", "runtime_input_idempotency_keys"),
                ("table", "runtime_input_idempotency_unindexable_rows"),
                ("trigger", "runtime_input_states_revision_after_insert"),
                ("trigger", "runtime_input_states_revision_after_update_old"),
                ("trigger", "runtime_input_states_revision_after_update_new"),
                ("trigger", "runtime_input_states_revision_after_delete"),
                ("trigger", "runtime_input_idempotency_after_insert"),
                ("trigger", "runtime_input_idempotency_after_update"),
                ("trigger", "runtime_input_idempotency_after_delete"),
                (
                    "trigger",
                    "runtime_input_idempotency_unindexable_after_insert",
                ),
                (
                    "trigger",
                    "runtime_input_idempotency_unindexable_after_update",
                ),
                (
                    "trigger",
                    "runtime_input_idempotency_unindexable_after_delete",
                ),
                (
                    "trigger",
                    "runtime_whole_blob_authority_activation_queue_after_insert",
                ),
                (
                    "trigger",
                    "runtime_whole_blob_authority_activation_queue_after_update",
                ),
                (
                    "trigger",
                    "runtime_whole_blob_authority_activation_queue_after_delete",
                ),
                (
                    "trigger",
                    "runtime_session_authority_activation_queue_after_insert",
                ),
                (
                    "trigger",
                    "runtime_session_authority_activation_queue_after_update",
                ),
                (
                    "trigger",
                    "runtime_head_canonical_activations_queue_after_complete_insert",
                ),
                (
                    "trigger",
                    "runtime_head_canonical_activations_queue_after_complete_update",
                ),
            ] {
                assert_eq!(
                    conn.query_row(
                        "SELECT COUNT(*) FROM sqlite_master WHERE type = ?1 AND name = ?2",
                        params![object_type, object_name],
                        |row| row.get::<_, i64>(0),
                    )
                    .unwrap(),
                    1,
                    "current runtime-store schema must install {object_type} {object_name}"
                );
            }
        }

        #[test]
        fn head_canonical_constructor_pins_the_database_against_profile_mixing() {
            let dir = TempDir::new().unwrap();
            let path = dir.path().join("sessions.sqlite3");
            let store = SqliteRuntimeStore::new_head_canonical(&path).unwrap();
            drop(store);

            let conn = Connection::open(&path).unwrap();
            assert_eq!(
                conn.query_row(
                    "SELECT persistence_profile FROM runtime_head_canonical_profile_pin WHERE singleton = 1",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
                RuntimeSessionPersistenceProfile::HeadCanonicalV1.to_string()
            );
            drop(conn);

            assert!(matches!(
                SqliteRuntimeStore::new_whole_blob(&path),
                Err(RuntimeStoreError::Unsupported(detail))
                    if detail.contains("irreversibly pinned")
            ));
        }

        #[tokio::test]
        async fn delivery_reads_do_not_provision_the_delivery_domain() {
            let (_dir, store) = temp_store();
            let rid = runtime_id();
            assert!(
                store
                    .load_runtime_delivery_authority(&rid)
                    .await
                    .unwrap()
                    .is_none()
            );
            assert!(
                store
                    .load_runtime_delivery_record(&rid, "job:job_1:terminal:1")
                    .await
                    .unwrap()
                    .is_none()
            );
            assert!(
                store
                    .list_runtime_delivery_records(&rid, 0, 16)
                    .await
                    .unwrap()
                    .is_empty()
            );
            let conn = Connection::open(store.path()).unwrap();
            assert_eq!(
                meerkat_sqlite::domain_version(&conn, "runtime-delivery").unwrap(),
                None,
                "delivery reads must not stamp the delivery domain"
            );
        }

        #[tokio::test]
        async fn first_delivery_write_provisions_the_delivery_domain_lazily() {
            let (_dir, store) = temp_store();
            let rid = runtime_id();
            let outcome = store
                .compare_and_swap_runtime_delivery_authority(
                    &rid,
                    None,
                    RuntimeDeliveryAuthorityRecord::from_parts(1, b"state".to_vec()),
                    Some(RuntimeDeliveryStoreRecord::from_parts(
                        "job:job_1:terminal:1",
                        1,
                        b"payload".to_vec(),
                    )),
                )
                .await
                .unwrap();
            assert!(matches!(
                outcome,
                RuntimeDeliveryAuthorityCasOutcome::Applied(_)
            ));
            let conn = Connection::open(store.path()).unwrap();
            assert_eq!(
                meerkat_sqlite::domain_version(&conn, "runtime-delivery").unwrap(),
                Some(1),
                "the first delivery write provisions the delivery domain"
            );
            assert_eq!(
                meerkat_sqlite::domain_version(&conn, "runtime-store").unwrap(),
                Some(2),
                "delivery use must not move the runtime-store domain"
            );
            drop(conn);
            assert!(
                store
                    .load_runtime_delivery_authority(&rid)
                    .await
                    .unwrap()
                    .is_some()
            );
            assert_eq!(
                store
                    .list_runtime_delivery_records(&rid, 0, 16)
                    .await
                    .unwrap()
                    .len(),
                1
            );
        }

        // ── per-row corruption containment for durable inputs ─────────────

        #[tokio::test]
        async fn one_corrupt_input_row_does_not_poison_the_load() {
            let (_dir, store) = temp_store();
            let rid = runtime_id();
            let good = input_state();
            store.persist_input_state(&rid, &good).await.unwrap();
            let conn = Connection::open(store.path()).unwrap();
            conn.execute(
                "INSERT INTO runtime_input_states (runtime_id, input_id, state_json)
                 VALUES (?1, 'zz-corrupt-row', ?2)",
                params![rid.0.clone(), b"{\"not\":\"an input state\"}".as_slice()],
            )
            .unwrap();
            drop(conn);

            let rows = store.load_input_states(&rid).await.unwrap();
            assert_eq!(rows.len(), 2);
            assert!(matches!(&rows[0], InputStateRow::Decoded(state)
                if state.state.input_id == good.as_stored().state.input_id));
            let InputStateRow::Corrupt { input_id, detail } = &rows[1] else {
                panic!("damaged row must surface as a typed per-row corruption witness");
            };
            assert_eq!(input_id, "zz-corrupt-row");
            assert!(!detail.is_empty());

            let strict = store.load_input_states_strict(&rid).await;
            let Err(RuntimeStoreError::ReadFailed(message)) = strict else {
                panic!("strict projection must fail on the corrupt row");
            };
            assert!(message.contains("zz-corrupt-row"));

            let recovered = crate::store::load_input_states_for_recovery(&store, &rid)
                .await
                .unwrap();
            assert_eq!(
                recovered.len(),
                1,
                "recovery proceeds with the decodable rows"
            );
        }
    }
}

#[cfg(feature = "sqlite-store")]
pub use inner::SqliteRuntimeStore;
