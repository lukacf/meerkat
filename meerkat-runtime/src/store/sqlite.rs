//! SQLite-backed RuntimeStore with atomic cross-table commits.

#[cfg(feature = "sqlite-store")]
mod inner {
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};
    #[cfg(test)]
    use std::sync::atomic::{AtomicU8, Ordering};
    use std::time::Duration;

    use chrono::{DateTime, Utc};
    use meerkat_core::lifecycle::{InputId, RunBoundaryReceipt, RunId};
    use meerkat_store::json_column::JsonColumnBytes;
    use meerkat_store::sqlite_store::{begin_immediate_transaction, open_connection};
    use rusqlite::{
        Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior, params,
    };
    use serde::{Deserialize, Serialize};

    use crate::identifiers::LogicalRuntimeId;
    use crate::input_state::{
        InputLifecycleState, InputState, InputStateHistoryEntry, InputStatePersistenceRecord,
        InputStateSeed, InputTerminalOutcome, StoredInputState,
    };
    use crate::store::{
        AuthOAuthFlowSnapshotUpdate, InputStateBatchCasOutcome, MachineLifecycleCommit,
        MachineLifecycleSnapshot, MachineLifecycleStoreRecord, RuntimeStore, RuntimeStoreError,
        SessionDelta, prepare_input_state_batch_cas,
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

    fn ensure_runtime_schema(conn: &Connection) -> Result<(), RuntimeStoreError> {
        conn.execute_batch(CREATE_RUNTIME_SCHEMA_SQL)
            .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))
    }

    fn open_runtime_connection(path: &Path) -> Result<Connection, RuntimeStoreError> {
        let conn =
            open_connection(path).map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
        ensure_runtime_schema(&conn)?;
        Ok(conn)
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

    /// Deserialize a persisted session-snapshot row through typed serde.
    /// `Session::deserialize` validates the mandatory envelope version against
    /// the generated persistence version authority, so a missing or
    /// non-current (v0/v1) row fails closed here instead of silently
    /// defaulting or upgrading on read.
    fn deserialize_persisted_session(
        bytes: &[u8],
    ) -> Result<meerkat_core::Session, RuntimeStoreError> {
        serde_json::from_slice(bytes).map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))
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

    /// Outcome for one runtime row inspected by the explicit v0.6.34
    /// completed-idle migrator.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
    #[serde(rename_all = "snake_case")]
    pub enum LegacyV0_6_34MigrationDisposition {
        WouldMigrate,
        Migrated,
        Current,
        Blocked,
    }

    /// One deterministic row in a legacy-migration report.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize)]
    pub struct LegacyV0_6_34MigrationItem {
        pub runtime_id: String,
        pub disposition: LegacyV0_6_34MigrationDisposition,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub detail: Option<String>,
    }

    /// Report returned by `rkat session migrate`. Dry-run is the default and
    /// never creates or modifies SQLite state.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize)]
    pub struct LegacyV0_6_34MigrationReport {
        pub applied: bool,
        pub items: Vec<LegacyV0_6_34MigrationItem>,
    }

    const CREATE_LEGACY_V0_6_34_AUDIT_SQL: &str = r"
CREATE TABLE IF NOT EXISTS runtime_legacy_v0_6_34_audit (
    runtime_id TEXT NOT NULL,
    table_name TEXT NOT NULL,
    row_key TEXT NOT NULL,
    original_record BLOB NOT NULL,
    migrated_record BLOB,
    action TEXT NOT NULL CHECK (action IN ('replace', 'delete')),
    PRIMARY KEY (runtime_id, table_name, row_key)
);
CREATE TRIGGER IF NOT EXISTS runtime_legacy_v0_6_34_audit_no_update
BEFORE UPDATE ON runtime_legacy_v0_6_34_audit
BEGIN SELECT RAISE(ABORT, 'legacy migration audit rows are append-only'); END;
CREATE TRIGGER IF NOT EXISTS runtime_legacy_v0_6_34_audit_no_delete
BEFORE DELETE ON runtime_legacy_v0_6_34_audit
BEGIN SELECT RAISE(ABORT, 'legacy migration audit rows are append-only'); END;
";

    impl LegacyV0_6_34MigrationReport {
        #[must_use]
        pub fn migration_count(&self) -> usize {
            self.items
                .iter()
                .filter(|item| {
                    matches!(
                        item.disposition,
                        LegacyV0_6_34MigrationDisposition::WouldMigrate
                            | LegacyV0_6_34MigrationDisposition::Migrated
                    )
                })
                .count()
        }

        #[must_use]
        pub fn blocked_count(&self) -> usize {
            self.items
                .iter()
                .filter(|item| item.disposition == LegacyV0_6_34MigrationDisposition::Blocked)
                .count()
        }
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct LegacyV0_6_34InputState {
        input_id: InputId,
        current_state: InputLifecycleState,
        #[serde(default, rename = "policy")]
        _policy: Option<serde_json::Value>,
        #[serde(default, rename = "runtime_semantics")]
        _runtime_semantics: Option<serde_json::Value>,
        #[serde(default)]
        terminal_outcome: Option<InputTerminalOutcome>,
        #[serde(default)]
        durability: Option<crate::input::InputDurability>,
        #[serde(default)]
        idempotency_key: Option<crate::identifiers::IdempotencyKey>,
        #[serde(default)]
        attempt_count: u32,
        #[serde(default)]
        recovery_count: u32,
        #[serde(default)]
        history: Vec<InputStateHistoryEntry>,
        #[serde(default, rename = "reconstruction_source")]
        _reconstruction_source: Option<serde_json::Value>,
        #[serde(default, rename = "persisted_input")]
        _persisted_input: Option<serde_json::Value>,
        #[serde(default)]
        last_run_id: Option<RunId>,
        #[serde(default)]
        last_boundary_sequence: Option<u64>,
        created_at: DateTime<Utc>,
        updated_at: DateTime<Utc>,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct LegacyV0_6_34OpsSnapshot {
        #[serde(rename = "epoch_id")]
        _epoch_id: meerkat_core::RuntimeEpochId,
        authority_state: LegacyV0_6_34OpsAuthority,
        operation_specs: BTreeMap<String, serde_json::Value>,
        completion_entries: Vec<serde_json::Value>,
        cursors: LegacyV0_6_34OpsCursors,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct LegacyV0_6_34OpsAuthority {
        operations: BTreeMap<String, serde_json::Value>,
        completed_order: Vec<serde_json::Value>,
        max_completed: usize,
        max_concurrent: serde_json::Value,
        active_count: usize,
        wait_request_id: serde_json::Value,
        wait_operation_ids: Vec<serde_json::Value>,
        next_completion_seq: u64,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct LegacyV0_6_34OpsCursors {
        agent_applied_cursor: u64,
        runtime_observed_seq: u64,
        runtime_last_injected_seq: u64,
    }

    struct PreparedLegacyInput {
        input_id: String,
        original_record: Vec<u8>,
        migrated_record: Vec<u8>,
    }

    struct PreparedLegacyRuntime {
        runtime_id: String,
        original_lifecycle: Vec<u8>,
        migrated_lifecycle: Vec<u8>,
        inputs: Vec<PreparedLegacyInput>,
        zero_ops_record: Option<Vec<u8>>,
    }

    struct LegacyMigrationPreflight {
        items: Vec<LegacyV0_6_34MigrationItem>,
        runtimes: Vec<PreparedLegacyRuntime>,
    }

    fn sqlite_table_exists(conn: &Connection, table_name: &str) -> Result<bool, RuntimeStoreError> {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
            params![table_name],
            |row| row.get(0),
        )
        .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))
    }

    fn legacy_v0_6_34_terminal_input(bytes: &[u8]) -> Result<StoredInputState, String> {
        let legacy: LegacyV0_6_34InputState = serde_json::from_slice(bytes)
            .map_err(|error| format!("not a v0.6.34 input-state row: {error}"))?;
        let terminal_pair = matches!(
            (legacy.current_state, legacy.terminal_outcome.as_ref()),
            (
                InputLifecycleState::Consumed,
                Some(InputTerminalOutcome::Consumed)
            ) | (
                InputLifecycleState::Superseded,
                Some(InputTerminalOutcome::Superseded { .. })
            ) | (
                InputLifecycleState::Coalesced,
                Some(InputTerminalOutcome::Coalesced { .. })
            ) | (
                InputLifecycleState::Abandoned,
                Some(InputTerminalOutcome::Abandoned { .. })
            )
        );
        if !terminal_pair {
            return Err(format!(
                "only terminal phase/outcome pairs are safe to migrate (observed {:?})",
                legacy.current_state
            ));
        }
        Ok(StoredInputState {
            state: InputState {
                input_id: legacy.input_id,
                history: legacy.history,
                updated_at: legacy.updated_at,
                policy: None,
                runtime_semantics: None,
                durability: legacy.durability,
                idempotency_key: legacy.idempotency_key,
                recovery_count: legacy.recovery_count,
                reconstruction_source: None,
                interaction_terminal_outbox: None,
                persisted_input: None,
                created_at: legacy.created_at,
            },
            seed: InputStateSeed {
                phase: legacy.current_state,
                last_run_id: legacy.last_run_id,
                last_boundary_sequence: legacy.last_boundary_sequence,
                admission_sequence: None,
                terminal_outcome: legacy.terminal_outcome,
                attempt_count: legacy.attempt_count,
                recovery_lane: None,
            },
        })
    }

    fn validate_zero_v0_6_34_ops(bytes: &[u8]) -> Result<(), String> {
        let snapshot: LegacyV0_6_34OpsSnapshot = serde_json::from_slice(bytes)
            .map_err(|error| format!("not a v0.6.34 ops snapshot: {error}"))?;
        let authority = snapshot.authority_state;
        let zero = authority.operations.is_empty()
            && snapshot.operation_specs.is_empty()
            && authority.completed_order.is_empty()
            && snapshot.completion_entries.is_empty()
            && authority.active_count == 0
            && authority.wait_request_id.is_null()
            && authority.wait_operation_ids.is_empty()
            && authority.next_completion_seq == 0
            && snapshot.cursors.agent_applied_cursor == 0
            && snapshot.cursors.runtime_observed_seq == 0
            && snapshot.cursors.runtime_last_injected_seq == 0;
        if !zero {
            return Err("ops snapshot contains live or completed operation authority".into());
        }
        if authority.max_completed != meerkat_core::ops_lifecycle::DEFAULT_MAX_COMPLETED
            || !authority.max_concurrent.is_null()
        {
            return Err("ops snapshot contains nondefault capacity policy".into());
        }
        Ok(())
    }

    fn validate_legacy_session_snapshot(
        conn: &Connection,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<(), String> {
        if !sqlite_table_exists(conn, "runtime_session_snapshots").map_err(|e| e.to_string())? {
            return Ok(());
        }
        let bytes = conn
            .query_row(
                "SELECT session_snapshot FROM runtime_session_snapshots WHERE runtime_id = ?1",
                params![runtime_id_text(runtime_id)],
                |row| Ok(row.get::<_, JsonColumnBytes>(0)?.into_bytes()),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        let Some(bytes) = bytes else {
            return Ok(());
        };
        let session: meerkat_core::Session =
            serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
        let owner = LogicalRuntimeId::for_session(session.id());
        if owner != *runtime_id {
            return Err(format!(
                "session {} belongs to runtime {owner}, not {runtime_id}",
                session.id()
            ));
        }
        Ok(())
    }

    fn validate_legacy_receipts(
        conn: &Connection,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<(), String> {
        if !sqlite_table_exists(conn, "runtime_boundary_receipts").map_err(|e| e.to_string())? {
            return Ok(());
        }
        let mut statement = conn
            .prepare(
                "SELECT run_id, sequence, receipt_json FROM runtime_boundary_receipts WHERE runtime_id = ?1 ORDER BY run_id, sequence",
            )
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map(params![runtime_id_text(runtime_id)], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, JsonColumnBytes>(2)?.into_bytes(),
                ))
            })
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        for (run_id, sequence, bytes) in rows {
            let receipt: RunBoundaryReceipt =
                serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
            let payload_sequence = i64::try_from(receipt.sequence)
                .map_err(|_| format!("receipt sequence {} is out of range", receipt.sequence))?;
            if receipt.run_id.0.to_string() != run_id || payload_sequence != sequence {
                return Err(format!(
                    "receipt payload identity {}:{} does not match row {run_id}:{sequence}",
                    receipt.run_id.0, payload_sequence
                ));
            }
        }
        Ok(())
    }

    fn prepare_legacy_companions(
        conn: &Connection,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<(Vec<PreparedLegacyInput>, Option<Vec<u8>>), String> {
        validate_legacy_session_snapshot(conn, runtime_id)?;
        validate_legacy_receipts(conn, runtime_id)?;

        let input_rows = if sqlite_table_exists(conn, "runtime_input_states")
            .map_err(|e| e.to_string())?
        {
            let mut statement = conn
                .prepare(
                    "SELECT input_id, state_json FROM runtime_input_states WHERE runtime_id = ?1 ORDER BY input_id",
                )
                .map_err(|error| error.to_string())?;
            statement
                .query_map(params![runtime_id_text(runtime_id)], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, JsonColumnBytes>(1)?.into_bytes(),
                    ))
                })
                .map_err(|error| error.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| error.to_string())?
        } else {
            Vec::new()
        };

        let mut driver = crate::driver::EphemeralRuntimeDriver::new(runtime_id.clone());
        let mut inputs = Vec::new();
        for (input_id, bytes) in input_rows {
            if deserialize_persisted_input_state(&bytes).is_ok() {
                return Err(format!(
                    "input {input_id}: mixed current-format companion under a bare v0.6.34 lifecycle is unsupported"
                ));
            }
            let bundle = legacy_v0_6_34_terminal_input(&bytes)
                .map_err(|detail| format!("input {input_id}: {detail}"))?;
            if bundle.state.input_id.0.to_string() != input_id {
                return Err(format!(
                    "input {input_id}: payload id does not match row key"
                ));
            }
            let record = driver
                .recover_input_state_persistence_record(bundle)
                .map_err(|error| {
                    format!("input {input_id}: generated recovery rejected it: {error}")
                })?;
            let migrated_record = serde_json::to_vec(record.as_stored())
                .map_err(|error| format!("input {input_id}: encode failed: {error}"))?;
            deserialize_persisted_input_state(&migrated_record).map_err(|error| {
                format!("input {input_id}: migrated record is invalid: {error}")
            })?;
            inputs.push(PreparedLegacyInput {
                input_id,
                original_record: bytes,
                migrated_record,
            });
        }

        let ops =
            if sqlite_table_exists(conn, "runtime_ops_lifecycle").map_err(|e| e.to_string())? {
                conn.query_row(
                    "SELECT state_json FROM runtime_ops_lifecycle WHERE runtime_id = ?1",
                    params![runtime_id_text(runtime_id)],
                    |row| Ok(row.get::<_, JsonColumnBytes>(0)?.into_bytes()),
                )
                .optional()
                .map_err(|error| error.to_string())?
            } else {
                None
            };
        let zero_ops_record = if let Some(bytes) = ops {
            if serde_json::from_slice::<crate::ops_lifecycle::PersistedOpsSnapshot>(&bytes).is_ok()
            {
                return Err(
                    "mixed current-format ops authority under a bare v0.6.34 lifecycle is unsupported"
                        .into(),
                );
            }
            validate_zero_v0_6_34_ops(&bytes)?;
            Some(bytes)
        } else {
            None
        };
        Ok((inputs, zero_ops_record))
    }

    fn preflight_legacy_v0_6_34(
        conn: &Connection,
    ) -> Result<LegacyMigrationPreflight, RuntimeStoreError> {
        if !sqlite_table_exists(conn, "runtime_states")? {
            return Ok(LegacyMigrationPreflight {
                items: Vec::new(),
                runtimes: Vec::new(),
            });
        }
        let rows = {
            let mut statement = conn
                .prepare(
                    "SELECT runtime_id, runtime_state_json FROM runtime_states ORDER BY runtime_id",
                )
                .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
            statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, JsonColumnBytes>(1)?.into_bytes(),
                    ))
                })
                .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?
        };

        let mut items = Vec::with_capacity(rows.len());
        let mut runtimes = Vec::new();
        for (runtime_id, bytes) in rows {
            if crate::store::decode_machine_lifecycle_store_record(&bytes).is_ok() {
                items.push(LegacyV0_6_34MigrationItem {
                    runtime_id,
                    disposition: LegacyV0_6_34MigrationDisposition::Current,
                    detail: None,
                });
                continue;
            }
            if bytes != b"\"idle\"" {
                items.push(LegacyV0_6_34MigrationItem {
                    runtime_id,
                    disposition: LegacyV0_6_34MigrationDisposition::Blocked,
                    detail: Some("not the exact v0.6.34 bare \"idle\" record".into()),
                });
                continue;
            }

            let logical_runtime_id = LogicalRuntimeId(runtime_id.clone());
            let companions = prepare_legacy_companions(conn, &logical_runtime_id);
            let (inputs, zero_ops_record) = match companions {
                Ok(prepared) => prepared,
                Err(detail) => {
                    items.push(LegacyV0_6_34MigrationItem {
                        runtime_id,
                        disposition: LegacyV0_6_34MigrationDisposition::Blocked,
                        detail: Some(detail),
                    });
                    continue;
                }
            };
            let mut lifecycle_driver =
                crate::driver::EphemeralRuntimeDriver::new(logical_runtime_id.clone());
            let migrated_lifecycle = lifecycle_driver
                .recover_v0_6_34_completed_idle_lifecycle_record()
                .map_err(|error| RuntimeStoreError::Internal(error.to_string()))?
                .encode()?;
            items.push(LegacyV0_6_34MigrationItem {
                runtime_id: runtime_id.clone(),
                disposition: LegacyV0_6_34MigrationDisposition::WouldMigrate,
                detail: None,
            });
            runtimes.push(PreparedLegacyRuntime {
                runtime_id,
                original_lifecycle: bytes,
                migrated_lifecycle,
                inputs,
                zero_ops_record,
            });
        }
        Ok(LegacyMigrationPreflight { items, runtimes })
    }

    fn open_existing_legacy_database(
        path: &Path,
        apply: bool,
    ) -> Result<Connection, RuntimeStoreError> {
        if !path.is_file() {
            return Err(RuntimeStoreError::ReadFailed(format!(
                "runtime database does not exist: {}",
                path.display()
            )));
        }
        let flags = if apply {
            OpenFlags::SQLITE_OPEN_READ_WRITE
        } else {
            OpenFlags::SQLITE_OPEN_READ_ONLY
        };
        let conn = Connection::open_with_flags(path, flags).map_err(|error| {
            if apply {
                RuntimeStoreError::WriteFailed(error.to_string())
            } else {
                RuntimeStoreError::ReadFailed(error.to_string())
            }
        })?;
        conn.busy_timeout(Duration::ZERO)
            .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
        Ok(conn)
    }

    fn apply_legacy_v0_6_34(
        path: &Path,
    ) -> Result<LegacyV0_6_34MigrationReport, RuntimeStoreError> {
        let mut conn = open_existing_legacy_database(path, true)?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Exclusive)
            .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
        let mut preflight = preflight_legacy_v0_6_34(&tx)?;
        let blocked = preflight
            .items
            .iter()
            .filter(|item| item.disposition == LegacyV0_6_34MigrationDisposition::Blocked)
            .map(|item| item.runtime_id.as_str())
            .collect::<Vec<_>>();
        if !blocked.is_empty() {
            return Err(RuntimeStoreError::ReadFailed(format!(
                "legacy migration blocked for runtime(s): {}",
                blocked.join(", ")
            )));
        }
        if !preflight.runtimes.is_empty() {
            tx.execute_batch(CREATE_LEGACY_V0_6_34_AUDIT_SQL)
                .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
        }
        for runtime in &preflight.runtimes {
            tx.execute(
                "INSERT INTO runtime_legacy_v0_6_34_audit (runtime_id, table_name, row_key, original_record, migrated_record, action) VALUES (?1, 'runtime_states', 'singleton', ?2, ?3, 'replace')",
                params![
                    runtime.runtime_id,
                    runtime.original_lifecycle,
                    runtime.migrated_lifecycle,
                ],
            )
            .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
            tx.execute(
                "UPDATE runtime_states SET runtime_state_json = ?1 WHERE runtime_id = ?2",
                params![runtime.migrated_lifecycle, runtime.runtime_id],
            )
            .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
            for input in &runtime.inputs {
                tx.execute(
                    "INSERT INTO runtime_legacy_v0_6_34_audit (runtime_id, table_name, row_key, original_record, migrated_record, action) VALUES (?1, 'runtime_input_states', ?2, ?3, ?4, 'replace')",
                    params![
                        runtime.runtime_id,
                        input.input_id,
                        input.original_record,
                        input.migrated_record,
                    ],
                )
                .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
                tx.execute(
                    "UPDATE runtime_input_states SET state_json = ?1 WHERE runtime_id = ?2 AND input_id = ?3",
                    params![input.migrated_record, runtime.runtime_id, input.input_id],
                )
                .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
            }
            if let Some(original_ops) = runtime.zero_ops_record.as_ref() {
                tx.execute(
                    "INSERT INTO runtime_legacy_v0_6_34_audit (runtime_id, table_name, row_key, original_record, migrated_record, action) VALUES (?1, 'runtime_ops_lifecycle', 'singleton', ?2, NULL, 'delete')",
                    params![runtime.runtime_id, original_ops],
                )
                .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
                tx.execute(
                    "DELETE FROM runtime_ops_lifecycle WHERE runtime_id = ?1",
                    params![runtime.runtime_id],
                )
                .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
            }
        }
        tx.commit()
            .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
        for item in &mut preflight.items {
            if item.disposition == LegacyV0_6_34MigrationDisposition::WouldMigrate {
                item.disposition = LegacyV0_6_34MigrationDisposition::Migrated;
            }
        }
        Ok(LegacyV0_6_34MigrationReport {
            applied: true,
            items: preflight.items,
        })
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

    fn is_runtime_placeholder_session(session: &meerkat_core::Session) -> bool {
        session.transcript_history_state().ok().flatten().is_none()
            && matches!(
                session.messages(),
                [] | [meerkat_core::types::Message::System(_)]
            )
    }

    const AUTH_OAUTH_FLOW_STATE_ID: &str = "auth_oauth_flow_state";

    fn upsert_runtime_snapshot(
        tx: &Transaction<'_>,
        runtime_id: &LogicalRuntimeId,
        snapshot: &[u8],
    ) -> Result<(), RuntimeStoreError> {
        tx.execute(
            r"
            INSERT INTO runtime_session_snapshots (runtime_id, session_snapshot)
            VALUES (?1, ?2)
            ON CONFLICT(runtime_id) DO UPDATE SET session_snapshot = excluded.session_snapshot
            ",
            params![runtime_id_text(runtime_id), snapshot],
        )
        .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
        // A live snapshot write clears any standing projection-quarantine marker
        // in the same atomic boundary: the runtime snapshot is authoritative
        // again, so a store-only projection fallback is no longer permitted.
        clear_runtime_projection_quarantine(tx, runtime_id)?;
        Ok(())
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
        for intent in crate::store::validated_compaction_projection_intents(session)? {
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
            if existing != intent {
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

    fn upsert_input_states(
        tx: &Transaction<'_>,
        runtime_id: &LogicalRuntimeId,
        input_states: &[StoredInputState],
    ) -> Result<(), RuntimeStoreError> {
        for bundle in input_states {
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
        }
        Ok(())
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
        #[cfg(test)]
        unregister_finalization_fault: AtomicU8,
    }

    impl SqliteRuntimeStore {
        pub fn new(path: impl Into<PathBuf>) -> Result<Self, RuntimeStoreError> {
            let path = path.into();
            let conn = open_runtime_connection(&path)?;
            drop(conn);
            Ok(Self {
                path,
                #[cfg(test)]
                unregister_finalization_fault: AtomicU8::new(0),
            })
        }

        /// Inspect or atomically migrate the exact completed-idle persistence
        /// shape emitted by v0.6.34. This is deliberately a concrete SQLite
        /// maintenance API: ordinary RuntimeStore reads remain fail-closed and
        /// other backends do not inherit a compatibility contract.
        ///
        /// `apply = true` requires the caller to stop every process that can
        /// access this database. The transaction is atomic, but it cannot
        /// fence a pre-v0.8 process from writing legacy rows after commit.
        pub fn migrate_v0_6_34_completed_idle(
            path: impl AsRef<Path>,
            apply: bool,
        ) -> Result<LegacyV0_6_34MigrationReport, RuntimeStoreError> {
            let path = path.as_ref();
            if apply {
                return apply_legacy_v0_6_34(path);
            }
            let conn = open_existing_legacy_database(path, false)?;
            let preflight = preflight_legacy_v0_6_34(&conn)?;
            Ok(LegacyV0_6_34MigrationReport {
                applied: false,
                items: preflight.items,
            })
        }

        pub fn path(&self) -> &Path {
            &self.path
        }

        #[cfg(test)]
        fn inject_unregister_finalization_fault(&self, fault: u8) {
            self.unregister_finalization_fault
                .store(fault, Ordering::SeqCst);
        }
    }

    #[async_trait::async_trait]
    impl RuntimeStore for SqliteRuntimeStore {
        fn supports_compaction_projection_outbox(&self) -> bool {
            true
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
            session_delta: SessionDelta,
        ) -> Result<(), RuntimeStoreError> {
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            tokio::task::spawn_blocking(move || {
                let incoming =
                    serde_json::from_slice(&session_delta.session_snapshot)
                        .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                let mut conn = open_runtime_connection(&path)?;
                let tx = begin_runtime_transaction(&mut conn)?;
                ensure_compaction_intents_already_outboxed(&tx, &runtime_id, &incoming)?;
                let snapshot_is_unchanged = tx
                    .query_row(
                        "SELECT 1 FROM runtime_session_snapshots WHERE runtime_id = ?1 AND session_snapshot = ?2",
                        params![
                            runtime_id_text(&runtime_id),
                            session_delta.session_snapshot.as_slice()
                        ],
                        |row| row.get::<_, i64>(0),
                    )
                    .optional()
                    .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))?
                    .is_some();
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

        async fn commit_session_transcript_rewrite_snapshot(
            &self,
            runtime_id: &LogicalRuntimeId,
            session_delta: SessionDelta,
            commit: &meerkat_core::TranscriptRewriteCommit,
        ) -> Result<(), RuntimeStoreError> {
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            let commit = commit.clone();
            tokio::task::spawn_blocking(move || {
                let incoming = serde_json::from_slice::<meerkat_core::Session>(
                    &session_delta.session_snapshot,
                )
                .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                let mut conn = open_runtime_connection(&path)?;
                let tx = begin_runtime_transaction(&mut conn)?;
                ensure_compaction_intents_already_outboxed(&tx, &runtime_id, &incoming)?;
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
                meerkat_core::session_store::transcript_rewrite_save_guard(
                    &incoming,
                    previous.as_ref(),
                    &commit,
                )
                .map_err(|err| match err {
                    meerkat_core::SessionStoreError::TranscriptRevisionConflict {
                        expected, actual, ..
                    } => RuntimeStoreError::TranscriptRevisionConflict { expected, actual },
                    other => RuntimeStoreError::WriteFailed(other.to_string()),
                })?;
                upsert_runtime_snapshot(&tx, &runtime_id, &session_delta.session_snapshot)?;
                tx.commit()
                    .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                Ok(())
            })
            .await
            .map_err(|err| RuntimeStoreError::Internal(format!("Task join failed: {err}")))?
        }

        async fn atomic_apply(
            &self,
            runtime_id: &LogicalRuntimeId,
            session_delta: Option<SessionDelta>,
            receipt: RunBoundaryReceipt,
            input_updates: Vec<InputStatePersistenceRecord>,
            session_store_key: Option<meerkat_core::types::SessionId>,
        ) -> Result<(), RuntimeStoreError> {
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            let input_updates = input_updates
                .into_iter()
                .map(InputStatePersistenceRecord::into_stored)
                .collect::<Vec<_>>();
            tokio::task::spawn_blocking(move || {
                let session_snapshot = session_delta
                    .as_ref()
                    .map(|delta| {
                        serde_json::from_slice::<meerkat_core::Session>(&delta.session_snapshot)
                            .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))
                    })
                    .transpose()?;
                let compaction_intents = session_snapshot
                    .as_ref()
                    .map(crate::store::validated_compaction_projection_intents)
                    .transpose()?
                    .unwrap_or_default();
                if let (Some(session), Some(session_store_key)) =
                    (session_snapshot.as_ref(), session_store_key.as_ref())
                    && session.id() != session_store_key
                {
                    return Err(RuntimeStoreError::SessionKeyMismatch {
                        expected: session_store_key.clone(),
                        actual: session.id().clone(),
                    });
                }

                let mut conn = open_runtime_connection(&path)?;
                let tx = begin_runtime_transaction(&mut conn)?;
                reject_finalized_compaction_projection_replays(
                    &tx,
                    &runtime_id,
                    &compaction_intents,
                )?;

                // The supersession verdict keys the entire commit: if the
                // incoming session snapshot is classified as superseded (the
                // persisted head is already a valid append-extension of it),
                // the snapshot write is skipped AND so are the receipt + input
                // writes, so receipt/input ordering identity never advances
                // past the retained session truth.
                let mut session_snapshot_superseded = false;
                if let Some(session) = session_snapshot.as_ref() {
                    let mut persist_session_snapshot = true;
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
                    if let Err(err) = meerkat_core::session_store::run_boundary_snapshot_save_guard(
                        session,
                        previous.as_ref(),
                    ) {
                        if previous.as_ref().is_some_and(is_runtime_placeholder_session) {
                            persist_session_snapshot = true;
                        } else if previous.as_ref().is_some_and(|previous| {
                            meerkat_core::session_store::run_boundary_snapshot_save_guard(
                                previous,
                                Some(session),
                            )
                            .is_ok()
                        }) {
                            persist_session_snapshot = false;
                            session_snapshot_superseded = true;
                        } else {
                            return Err(RuntimeStoreError::WriteFailed(err.to_string()));
                        }
                    }
                    if persist_session_snapshot
                        && let Some(delta) = session_delta.as_ref()
                    {
                        upsert_runtime_snapshot(&tx, &runtime_id, &delta.session_snapshot)?;
                    }
                }

                // When the session snapshot was superseded and skipped, the
                // boundary receipt and input-state updates must also be skipped:
                // advancing them against a retained (older) session snapshot
                // would split receipt/input ordering identity from session
                // truth. Rejecting here drops and rolls back the untouched
                // transaction, releasing its lock without publishing a false
                // success to the stale writer.
                if session_snapshot_superseded {
                    return Err(RuntimeStoreError::SessionSnapshotSuperseded {
                        runtime_id: runtime_id_text(&runtime_id).to_owned(),
                    });
                }

                insert_compaction_projection_outbox_intents(
                    &tx,
                    &runtime_id,
                    &compaction_intents,
                )?;
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
            session_delta: SessionDelta,
            receipt: RunBoundaryReceipt,
            machine_lifecycle: MachineLifecycleCommit,
            input_updates: Vec<InputStatePersistenceRecord>,
            session_store_key: meerkat_core::types::SessionId,
        ) -> Result<(), RuntimeStoreError> {
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            let lifecycle_snapshot = machine_lifecycle.into_snapshot();
            let input_updates = input_updates
                .into_iter()
                .map(InputStatePersistenceRecord::into_stored)
                .collect::<Vec<_>>();
            tokio::task::spawn_blocking(move || {
                let session = serde_json::from_slice::<meerkat_core::Session>(
                    &session_delta.session_snapshot,
                )
                .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                let compaction_intents =
                    crate::store::validated_compaction_projection_intents(&session)?;
                if session.id() != &session_store_key {
                    return Err(RuntimeStoreError::SessionKeyMismatch {
                        expected: session_store_key,
                        actual: session.id().clone(),
                    });
                }

                let mut conn = open_runtime_connection(&path)?;
                let tx = begin_runtime_transaction(&mut conn)?;
                reject_finalized_compaction_projection_replays(
                    &tx,
                    &runtime_id,
                    &compaction_intents,
                )?;
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
                if let Err(err) = meerkat_core::session_store::run_boundary_snapshot_save_guard(
                    &session,
                    previous.as_ref(),
                ) {
                    if previous.as_ref().is_some_and(is_runtime_placeholder_session) {
                        // The generated snapshot replaces the placeholder in
                        // the same terminal transaction.
                    } else if previous.as_ref().is_some_and(|previous| {
                        meerkat_core::session_store::run_boundary_snapshot_save_guard(
                            previous,
                            Some(&session),
                        )
                        .is_ok()
                    }) {
                        return Err(RuntimeStoreError::SessionSnapshotSuperseded {
                            runtime_id: runtime_id_text(&runtime_id).to_owned(),
                        });
                    } else {
                        return Err(RuntimeStoreError::WriteFailed(err.to_string()));
                    }
                }

                upsert_runtime_snapshot(&tx, &runtime_id, &session_delta.session_snapshot)?;
                upsert_machine_lifecycle_snapshot(&tx, &runtime_id, &lifecycle_snapshot)?;
                insert_compaction_projection_outbox_intents(
                    &tx,
                    &runtime_id,
                    &compaction_intents,
                )?;
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
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            let projection = projection.clone();
            tokio::task::spawn_blocking(move || {
                let mut conn = open_runtime_connection(&path)?;
                let tx = begin_runtime_transaction(&mut conn)?;
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
                    session
                        .complete_compaction_projection_intent(&projection)
                        .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
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
        ) -> Result<Vec<StoredInputState>, RuntimeStoreError> {
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            tokio::task::spawn_blocking(move || {
                let conn = open_runtime_connection(&path)?;
                let mut stmt = conn
                    .prepare(
                        r"
                        SELECT state_json
                        FROM runtime_input_states
                        WHERE runtime_id = ?1
                        ORDER BY input_id ASC
                        ",
                    )
                    .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))?;
                let rows = stmt
                    .query_map(params![runtime_id_text(&runtime_id)], |row| {
                        row.get::<_, JsonColumnBytes>(0)
                            .map(JsonColumnBytes::into_bytes)
                    })
                    .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))?;
                rows.map(|row| {
                    let bytes =
                        row.map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))?;
                    deserialize_persisted_input_state(&bytes)
                })
                .collect()
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

        async fn load_session_snapshot(
            &self,
            runtime_id: &LogicalRuntimeId,
        ) -> Result<Option<Vec<u8>>, RuntimeStoreError> {
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            tokio::task::spawn_blocking(move || {
                let conn = open_runtime_connection(&path)?;
                conn.query_row(
                    "SELECT session_snapshot FROM runtime_session_snapshots WHERE runtime_id = ?1",
                    params![runtime_id_text(&runtime_id)],
                    |row| Ok(row.get::<_, JsonColumnBytes>(0)?.into_bytes()),
                )
                .optional()
                .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))
            })
            .await
            .map_err(|err| RuntimeStoreError::Internal(format!("Task join failed: {err}")))?
        }

        async fn clear_session_snapshot(
            &self,
            runtime_id: &LogicalRuntimeId,
        ) -> Result<(), RuntimeStoreError> {
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            tokio::task::spawn_blocking(move || {
                let mut conn = open_runtime_connection(&path)?;
                let tx = begin_runtime_transaction(&mut conn)?;
                tx.execute(
                    "DELETE FROM runtime_session_snapshots WHERE runtime_id = ?1",
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

        async fn replace_session_snapshot_if_current(
            &self,
            runtime_id: &LogicalRuntimeId,
            expected_current: &[u8],
            replacement: Vec<u8>,
        ) -> Result<bool, RuntimeStoreError> {
            let replacement_session: meerkat_core::Session =
                serde_json::from_slice(&replacement)
                    .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            let expected_current = expected_current.to_vec();
            tokio::task::spawn_blocking(move || {
                let mut conn = open_runtime_connection(&path)?;
                let tx = begin_runtime_transaction(&mut conn)?;
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
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            let expected_current = expected_current.to_vec();
            tokio::task::spawn_blocking(move || {
                let mut conn = open_runtime_connection(&path)?;
                let tx = begin_runtime_transaction(&mut conn)?;
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
                .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
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
            let state = state.clone_stored();
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
                .map(InputStatePersistenceRecord::clone_stored)
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
                }
                tx.commit()
                    .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
                Ok(InputStateBatchCasOutcome::Swapped)
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
                .map(InputStatePersistenceRecord::clone_stored)
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
                upsert_input_states(&tx, &runtime_id, &input_states)?;
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
        use crate::identifiers::LogicalRuntimeId;
        use crate::runtime_state::RuntimeState;
        use crate::traits::RuntimeDriver as _;
        use meerkat_core::lifecycle::run_primitive::RunApplyBoundary;
        use meerkat_core::lifecycle::{InputId, RunBoundaryReceipt, RunId};
        use meerkat_core::session_store::SessionStore as _;
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

        fn runtime_id() -> LogicalRuntimeId {
            LogicalRuntimeId("runtime-1".to_string())
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
                .commits
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
                        Some(SessionDelta {
                            session_snapshot: snapshot.clone(),
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
                    Some(snapshot)
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
            let commit = session
                .transcript_history_state()
                .unwrap()
                .unwrap()
                .commits
                .last()
                .unwrap()
                .clone();
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
                    Some(SessionDelta {
                        session_snapshot: replay_snapshot.clone(),
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
                    Some(SessionDelta {
                        session_snapshot: replay_snapshot.clone(),
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
                    SessionDelta {
                        session_snapshot: replay_snapshot.clone(),
                    },
                )
                .await
                .unwrap_err();
            assert!(error.to_string().contains("finalized compaction intent"));
            let error = store
                .commit_session_transcript_rewrite_snapshot(
                    &runtime_id,
                    SessionDelta {
                        session_snapshot: replay_snapshot.clone(),
                    },
                    &commit,
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
                    Some(SessionDelta {
                        session_snapshot: snapshot_with_raw_intents(
                            &session,
                            &[original, conflicting],
                        ),
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
                        Some(SessionDelta {
                            session_snapshot: snapshot_with_raw_intents(&session, &[invalid]),
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
                    SessionDelta {
                        session_snapshot: current_snapshot.clone(),
                    },
                )
                .await
                .unwrap();
            let error = store
                .atomic_apply(
                    &runtime_id,
                    Some(SessionDelta {
                        session_snapshot: serde_json::to_vec(&incoming).unwrap(),
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
                Some(current_snapshot)
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
                    Some(SessionDelta {
                        session_snapshot: original_snapshot.clone(),
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
                    Some(SessionDelta {
                        session_snapshot: snapshot_with_raw_intents(&advanced, &[conflicting]),
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
                Some(original_snapshot)
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
            let commit = session
                .transcript_history_state()
                .unwrap()
                .unwrap()
                .commits
                .last()
                .unwrap()
                .clone();
            assert!(
                store
                    .commit_session_snapshot(
                        &runtime_id,
                        SessionDelta {
                            session_snapshot: snapshot.clone(),
                        },
                    )
                    .await
                    .is_err()
            );
            assert!(
                store
                    .commit_session_transcript_rewrite_snapshot(
                        &runtime_id,
                        SessionDelta {
                            session_snapshot: snapshot.clone(),
                        },
                        &commit,
                    )
                    .await
                    .is_err()
            );
            let clean = Session::with_id(session.id().clone());
            let clean_snapshot = serde_json::to_vec(&clean).unwrap();
            store
                .commit_session_snapshot(
                    &runtime_id,
                    SessionDelta {
                        session_snapshot: clean_snapshot.clone(),
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
                Some(clean_snapshot)
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
                    Some(SessionDelta {
                        session_snapshot: session.clone(),
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
            assert_eq!(store.load_input_states(&runtime_id).await.unwrap().len(), 1);
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
                .load_input_states(&runtime_id)
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
            let rows = store.load_input_states(&runtime_id).await.unwrap();
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
            let rows = store.load_input_states(&runtime_id).await.unwrap();
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
                    SessionDelta {
                        session_snapshot: session,
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
                    .load_input_states(&runtime_id)
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
                    SessionDelta {
                        session_snapshot: snapshot.clone(),
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
                    SessionDelta {
                        session_snapshot: snapshot.clone(),
                    },
                )
                .await
                .expect("identical validated snapshot should bypass the UPDATE path");

            assert_eq!(
                store.load_session_snapshot(&runtime_id).await.unwrap(),
                Some(snapshot)
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
                    SessionDelta {
                        session_snapshot: serde_json::to_vec(&session).unwrap(),
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
                    SessionDelta {
                        session_snapshot: accepted_snapshot.clone(),
                    },
                )
                .await
                .unwrap();

            let err = store
                .commit_session_snapshot(
                    &runtime_id,
                    SessionDelta {
                        session_snapshot: serde_json::to_vec(&stale).unwrap(),
                    },
                )
                .await
                .expect_err("stale non-continuation must not overwrite runtime snapshot");

            assert!(matches!(err, RuntimeStoreError::WriteFailed(_)));
            assert_eq!(
                store.load_session_snapshot(&runtime_id).await.unwrap(),
                Some(accepted_snapshot)
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
                    SessionDelta {
                        session_snapshot: current_snapshot.clone(),
                    },
                )
                .await
                .unwrap();

            let error = store
                .atomic_apply(
                    &runtime_id,
                    Some(SessionDelta {
                        session_snapshot: serde_json::to_vec(&incoming).unwrap(),
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
                Some(current_snapshot)
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
                    .load_input_states(&runtime_id)
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
            placeholder.set_system_prompt("base system".to_string());
            let mut incoming = Session::with_id(placeholder.id().clone());
            incoming.set_system_prompt("base system".to_string());
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
                    SessionDelta {
                        session_snapshot: serde_json::to_vec(&placeholder).unwrap(),
                    },
                )
                .await
                .unwrap();

            store
                .atomic_apply(
                    &runtime_id,
                    Some(SessionDelta {
                        session_snapshot: incoming_snapshot.clone(),
                    }),
                    receipt.clone(),
                    vec![],
                    Some(incoming.id().clone()),
                )
                .await
                .unwrap();

            assert_eq!(
                store.load_session_snapshot(&runtime_id).await.unwrap(),
                Some(incoming_snapshot)
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
            previous.set_system_prompt("runtime system before context refresh".to_string());
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
            incoming.set_system_prompt("runtime system after context refresh".to_string());
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
                    SessionDelta {
                        session_snapshot: serde_json::to_vec(&previous).unwrap(),
                    },
                )
                .await
                .unwrap();

            store
                .atomic_apply(
                    &runtime_id,
                    Some(SessionDelta {
                        session_snapshot: incoming_snapshot.clone(),
                    }),
                    receipt.clone(),
                    vec![],
                    Some(incoming.id().clone()),
                )
                .await
                .unwrap();

            assert_eq!(
                store.load_session_snapshot(&runtime_id).await.unwrap(),
                Some(incoming_snapshot)
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
        async fn transcript_rewrite_snapshot_rejects_stale_runtime_parent() {
            let (_dir, store) = temp_store();
            let runtime_id = runtime_id();
            let original = session_with_one_turn();
            store
                .commit_session_snapshot(
                    &runtime_id,
                    SessionDelta {
                        session_snapshot: serde_json::to_vec(&original).unwrap(),
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
            store
                .commit_session_transcript_rewrite_snapshot(
                    &runtime_id,
                    SessionDelta {
                        session_snapshot: serde_json::to_vec(&first_rewrite).unwrap(),
                    },
                    &first_commit,
                )
                .await
                .unwrap();

            let mut stale_rewrite = original;
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
            let err = store
                .commit_session_transcript_rewrite_snapshot(
                    &runtime_id,
                    SessionDelta {
                        session_snapshot: serde_json::to_vec(&stale_rewrite).unwrap(),
                    },
                    &stale_commit,
                )
                .await
                .expect_err("stale rewrite parent should be rejected atomically");
            assert!(matches!(
                err,
                RuntimeStoreError::TranscriptRevisionConflict { .. }
            ));

            let stored = store
                .load_session_snapshot(&runtime_id)
                .await
                .unwrap()
                .unwrap();
            let stored: Session = serde_json::from_slice(&stored).unwrap();
            assert_eq!(stored.transcript_revision().unwrap(), first_commit.revision);
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
                    Some(SessionDelta {
                        session_snapshot: session,
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
            let states = store.load_input_states(&runtime_id).await.unwrap();
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
                    SessionDelta {
                        session_snapshot: serde_json::to_vec(&session).unwrap(),
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
                store.load_input_states(&runtime_id).await.unwrap().len(),
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
                    SessionDelta {
                        session_snapshot: encoded.clone(),
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
                    SessionDelta {
                        session_snapshot: encoded,
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
                    SessionDelta {
                        session_snapshot: durable_snapshot.clone(),
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
                    SessionDelta {
                        session_snapshot: serde_json::to_vec(&incoming).unwrap(),
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
                Some(durable_snapshot)
            );
            assert_eq!(
                crate::store::load_runtime_state(&store, &runtime_id)
                    .await
                    .unwrap(),
                None
            );
            assert!(
                store
                    .load_input_states(&runtime_id)
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
                    Some(SessionDelta {
                        session_snapshot: snapshot,
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
            assert_eq!(store.load_input_states(&runtime_id).await.unwrap().len(), 1);
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
            assert_eq!(store.load_input_states(&runtime_id).await.unwrap().len(), 1);
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
                    .load_input_states(&runtime_id)
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
            assert_eq!(store.load_input_states(&runtime_id).await.unwrap().len(), 1);
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
                    SessionDelta {
                        session_snapshot: serde_json::to_vec(&incoming).unwrap(),
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
                .load_input_states(&runtime_id)
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
                        SessionDelta {
                            session_snapshot: rejected_snapshot.clone(),
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
                        SessionDelta {
                            session_snapshot: serde_json::to_vec(&revived).unwrap(),
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
                    SessionDelta {
                        session_snapshot: snapshot.clone(),
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
            assert_eq!(carried, snapshot, "TEXT snapshot bytes must round-trip");
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

        fn legacy_fixture(name: &str) -> Vec<u8> {
            let path = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/v0_6_34_completed_idle")
                .join(name);
            let mut bytes = std::fs::read(path).unwrap();
            // Repository text fixtures end with LF, while v0.6.34 persisted
            // the exact compact bytes emitted by `serde_json::to_vec`.
            if bytes.last() == Some(&b'\n') {
                bytes.pop();
            }
            bytes
        }

        #[derive(Deserialize)]
        struct LegacySessionRow {
            session_id: String,
            created_at_ms: i64,
            updated_at_ms: i64,
            message_count: i64,
            total_tokens: i64,
            metadata_json: String,
        }

        fn create_complete_v0_6_34_realm_database(path: &Path) -> LogicalRuntimeId {
            let conn = Connection::open(path).unwrap();
            conn.execute_batch(include_str!(
                "../../tests/fixtures/v0_6_34_completed_idle/schema.sql"
            ))
            .unwrap();
            let session: LegacySessionRow =
                serde_json::from_slice(&legacy_fixture("session_row.json")).unwrap();
            let runtime_id = LogicalRuntimeId(format!("rt:session:{}", session.session_id));
            conn.execute(
                "INSERT INTO sessions (session_id, created_at_ms, updated_at_ms, message_count, total_tokens, metadata_json, session_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    session.session_id,
                    session.created_at_ms,
                    session.updated_at_ms,
                    session.message_count,
                    session.total_tokens,
                    session.metadata_json,
                    legacy_fixture("session_snapshot.json"),
                ],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO runtime_states (runtime_id, runtime_state_json) VALUES (?1, ?2)",
                params![runtime_id.0, legacy_fixture("runtime_state.json")],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO runtime_session_snapshots (runtime_id, session_snapshot) VALUES (?1, ?2)",
                params![runtime_id.0, legacy_fixture("session_snapshot.json")],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO runtime_input_states (runtime_id, input_id, state_json) VALUES (?1, ?2, ?3)",
                params![
                    runtime_id.0,
                    "018f0000-0000-7000-8000-000000000002",
                    legacy_fixture("input_state_consumed.json"),
                ],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO runtime_boundary_receipts (runtime_id, run_id, sequence, receipt_json) VALUES (?1, ?2, 1, ?3)",
                params![
                    runtime_id.0,
                    "018f0000-0000-7000-8000-000000000003",
                    legacy_fixture("boundary_receipt.json"),
                ],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO runtime_ops_lifecycle (runtime_id, state_json) VALUES (?1, ?2)",
                params![runtime_id.0, legacy_fixture("ops_lifecycle_empty.json")],
            )
            .unwrap();
            drop(conn);
            runtime_id
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

        fn raw_audit_row(
            path: &Path,
            runtime_id: &str,
            table_name: &str,
            row_key: &str,
        ) -> Vec<u8> {
            let conn = Connection::open(path).unwrap();
            conn.query_row(
                "SELECT original_record FROM runtime_legacy_v0_6_34_audit WHERE runtime_id = ?1 AND table_name = ?2 AND row_key = ?3",
                params![runtime_id, table_name, row_key],
                |row| row.get(0),
            )
            .unwrap()
        }

        fn raw_session_store_row(path: &Path, session_id: &str) -> Vec<u8> {
            let conn = Connection::open(path).unwrap();
            conn.query_row(
                "SELECT session_json FROM sessions WHERE session_id = ?1",
                params![session_id],
                |row| row.get(0),
            )
            .unwrap()
        }

        #[tokio::test]
        async fn explicit_v0_6_34_completed_idle_migration_is_atomic_and_idempotent() {
            let dir = TempDir::new().unwrap();
            let path = dir.path().join("sessions.sqlite3");
            let runtime_id = create_complete_v0_6_34_realm_database(&path);
            let original_session = raw_fixture_row(
                &path,
                "runtime_session_snapshots",
                "session_snapshot",
                &runtime_id.0,
            );
            let original_session_store =
                raw_session_store_row(&path, "018f0000-0000-7000-8000-000000000001");
            let original_receipt = raw_fixture_row(
                &path,
                "runtime_boundary_receipts",
                "receipt_json",
                &runtime_id.0,
            );

            let ordinary = SqliteRuntimeStore::new(&path).unwrap();
            assert!(
                crate::store::load_runtime_state(&ordinary, &runtime_id)
                    .await
                    .is_err()
            );
            drop(ordinary);

            let dry_run = SqliteRuntimeStore::migrate_v0_6_34_completed_idle(&path, false).unwrap();
            assert!(!dry_run.applied);
            assert_eq!(dry_run.migration_count(), 1, "{:?}", dry_run.items);
            assert_eq!(
                raw_fixture_row(&path, "runtime_states", "runtime_state_json", &runtime_id.0,),
                b"\"idle\""
            );

            let applied = SqliteRuntimeStore::migrate_v0_6_34_completed_idle(&path, true).unwrap();
            assert!(applied.applied);
            assert_eq!(applied.migration_count(), 1);
            assert_eq!(
                raw_audit_row(
                    &path,
                    &runtime_id.0,
                    "runtime_input_states",
                    "018f0000-0000-7000-8000-000000000002",
                ),
                legacy_fixture("input_state_consumed.json")
            );
            assert_eq!(
                raw_audit_row(&path, &runtime_id.0, "runtime_states", "singleton"),
                legacy_fixture("runtime_state.json")
            );
            assert_eq!(
                raw_audit_row(&path, &runtime_id.0, "runtime_ops_lifecycle", "singleton",),
                legacy_fixture("ops_lifecycle_empty.json")
            );
            let audit_conn = Connection::open(&path).unwrap();
            assert!(
                audit_conn
                    .execute(
                        "DELETE FROM runtime_legacy_v0_6_34_audit WHERE runtime_id = ?1",
                        params![runtime_id.0],
                    )
                    .is_err(),
                "migration audit rows must reject deletion"
            );
            drop(audit_conn);
            assert_eq!(
                raw_session_store_row(&path, "018f0000-0000-7000-8000-000000000001",),
                original_session_store
            );
            assert_eq!(
                raw_fixture_row(
                    &path,
                    "runtime_session_snapshots",
                    "session_snapshot",
                    &runtime_id.0,
                ),
                original_session
            );
            assert_eq!(
                raw_fixture_row(
                    &path,
                    "runtime_boundary_receipts",
                    "receipt_json",
                    &runtime_id.0,
                ),
                original_receipt
            );
            let reopened = SqliteRuntimeStore::new(&path).unwrap();
            assert_eq!(
                crate::store::load_runtime_state(&reopened, &runtime_id)
                    .await
                    .unwrap(),
                Some(RuntimeState::Idle)
            );
            assert_eq!(
                reopened.load_input_states(&runtime_id).await.unwrap().len(),
                1
            );
            assert!(
                reopened
                    .load_ops_lifecycle(&runtime_id)
                    .await
                    .unwrap()
                    .is_none()
            );
            drop(reopened);
            let runtime_store: std::sync::Arc<dyn RuntimeStore> =
                std::sync::Arc::new(SqliteRuntimeStore::new(&path).unwrap());
            let blob_store: std::sync::Arc<dyn meerkat_core::BlobStore> =
                std::sync::Arc::new(meerkat_store::MemoryBlobStore::new());
            let mut recovered_driver = crate::driver::PersistentRuntimeDriver::new(
                runtime_id.clone(),
                runtime_store,
                blob_store,
            );
            recovered_driver
                .recover()
                .await
                .expect("migrated image must pass the real persistent recovery boundary");
            assert_eq!(recovered_driver.runtime_state(), RuntimeState::Idle);

            let session_store = SqliteSessionStore::open(&path).unwrap();
            let sessions = session_store.list(Default::default()).await.unwrap();
            assert_eq!(
                sessions.len(),
                1,
                "the complete legacy sessions row survives"
            );

            let repeated = SqliteRuntimeStore::migrate_v0_6_34_completed_idle(&path, true).unwrap();
            assert_eq!(repeated.migration_count(), 0);
            assert_eq!(
                repeated.items[0].disposition,
                LegacyV0_6_34MigrationDisposition::Current
            );
        }

        #[test]
        fn legacy_migration_refuses_nonterminal_input_without_writes() {
            let dir = TempDir::new().unwrap();
            let path = dir.path().join("sessions.sqlite3");
            let runtime_id = create_complete_v0_6_34_realm_database(&path);
            let conn = Connection::open(&path).unwrap();
            let mut value: serde_json::Value =
                serde_json::from_slice(&legacy_fixture("input_state_consumed.json")).unwrap();
            value["current_state"] = serde_json::json!("accepted");
            value["terminal_outcome"] = serde_json::Value::Null;
            conn.execute(
                "UPDATE runtime_input_states SET state_json = ?1 WHERE runtime_id = ?2",
                params![serde_json::to_vec(&value).unwrap(), runtime_id.0],
            )
            .unwrap();
            drop(conn);

            let before =
                raw_fixture_row(&path, "runtime_states", "runtime_state_json", &runtime_id.0);
            let dry_run = SqliteRuntimeStore::migrate_v0_6_34_completed_idle(&path, false).unwrap();
            assert_eq!(dry_run.blocked_count(), 1);
            assert!(SqliteRuntimeStore::migrate_v0_6_34_completed_idle(&path, true).is_err());
            assert_eq!(
                raw_fixture_row(&path, "runtime_states", "runtime_state_json", &runtime_id.0,),
                before
            );
        }

        #[test]
        fn legacy_migration_refuses_mixed_current_input_without_writes() {
            let dir = TempDir::new().unwrap();
            let path = dir.path().join("sessions.sqlite3");
            let runtime_id = create_complete_v0_6_34_realm_database(&path);
            let current = StoredInputState::new_accepted(InputId::new());
            let conn = Connection::open(&path).unwrap();
            conn.execute(
                "UPDATE runtime_input_states SET state_json = ?1 WHERE runtime_id = ?2",
                params![serde_json::to_vec(&current).unwrap(), runtime_id.0],
            )
            .unwrap();
            drop(conn);

            let before =
                raw_fixture_row(&path, "runtime_states", "runtime_state_json", &runtime_id.0);
            let dry_run = SqliteRuntimeStore::migrate_v0_6_34_completed_idle(&path, false).unwrap();
            assert_eq!(dry_run.blocked_count(), 1);
            assert!(
                dry_run.items[0]
                    .detail
                    .as_deref()
                    .is_some_and(|detail| detail.contains("mixed current-format companion"))
            );
            assert!(SqliteRuntimeStore::migrate_v0_6_34_completed_idle(&path, true).is_err());
            assert_eq!(
                raw_fixture_row(&path, "runtime_states", "runtime_state_json", &runtime_id.0,),
                before
            );
        }
    }
}

#[cfg(feature = "sqlite-store")]
pub use inner::{
    LegacyV0_6_34MigrationDisposition, LegacyV0_6_34MigrationItem, LegacyV0_6_34MigrationReport,
    SqliteRuntimeStore,
};
