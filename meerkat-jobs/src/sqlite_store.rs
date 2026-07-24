use std::path::{Path, PathBuf};

use async_trait::async_trait;
use meerkat_core::{AuthBindingRef, SessionId, ToolCredentialContextRef};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};

use crate::machines::detached_job::{
    DetachedJobMachineState, DetachedJobPhase, DetachedJobRestartClass, DetachedJobTerminalKind,
};
use crate::store::{next_revision, validate_stored_job};
use crate::{
    CanonicalArgumentsHash, DetachedJobError, DetachedJobStore, ExecutionIntentId,
    InsertJobOutcome, InteractionLineageId, JobId, JobOutboxEntry, JobProgress, JobSpec,
    JobSubmissionKey, JobTerminalResult, OriginMemberId, RunnerIdentity, StoredJob, ToolIdentity,
};

const STORED_JOB_FORMAT_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
struct StoredJobEnvelope {
    format_version: u32,
    job: PersistedStoredJob,
}

#[derive(Debug, Serialize, Deserialize)]
struct PersistedStoredJob {
    job_id: JobId,
    spec: PersistedJobSpec,
    revision: u64,
    machine_state: PersistedMachineState,
    progress: Option<JobProgress>,
    terminal_result: Option<JobTerminalResult>,
    outbox: Vec<PersistedJobOutboxEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
struct PersistedJobSpec {
    realm_id: String,
    origin_session_id: SessionId,
    origin_member_id: Option<OriginMemberId>,
    execution_intent_id: ExecutionIntentId,
    interaction_lineage_id: InteractionLineageId,
    tool: ToolIdentity,
    runner: RunnerIdentity,
    restart_class: PersistedRestartClass,
    canonical_arguments_hash: CanonicalArgumentsHash,
    credential_context_refs: Vec<PersistedCredentialContextRef>,
    submission_key: JobSubmissionKey,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum PersistedCredentialContextRef {
    OwningProfile {
        required_scopes: std::collections::BTreeSet<String>,
    },
    AuthBinding {
        auth_binding: AuthBindingRef,
        required_scopes: std::collections::BTreeSet<String>,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PersistedRestartClass {
    Adoptable,
    CheckpointResumable,
    Replayable,
    NonResumable,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PersistedPhase {
    Unsubmitted,
    Queued,
    Claimed,
    Running,
    WaitingExternal,
    LossObserved,
    RetryScheduled,
    Succeeded,
    Failed,
    Cancelled,
    WorkerLost,
    NeedsAttention,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PersistedTerminalKind {
    Succeeded,
    Failed,
    Cancelled,
    WorkerLost,
    NeedsAttention,
}

#[derive(Debug, Serialize, Deserialize)]
struct PersistedMachineState {
    lifecycle_phase: PersistedPhase,
    job_id: String,
    restart_class: PersistedRestartClass,
    attempt_count: u64,
    current_attempt_id: Option<String>,
    current_fence: u64,
    current_worker_id: Option<String>,
    lease_expires_at_ms: Option<u64>,
    heartbeat_at_ms: Option<u64>,
    checkpoint_ref: Option<String>,
    runner_handle: Option<String>,
    progress_cursor: u64,
    lease_expired: bool,
    retry_due_at_ms: Option<u64>,
    cancel_requested: bool,
    terminal_kind: Option<PersistedTerminalKind>,
    terminal_delivery_sequence: u64,
    terminal_delivery_applied: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct PersistedJobOutboxEntry {
    job_id: JobId,
    delivery_sequence: u64,
    terminal_kind: PersistedTerminalKind,
    terminal_result: JobTerminalResult,
    applied: bool,
}

pub const JOBS_DOMAIN: meerkat_sqlite::SchemaDomain = meerkat_sqlite::SchemaDomain {
    name: "jobs",
    migrations: &[meerkat_sqlite::Migration {
        version: 1,
        name: "base-schema",
        apply: migration_0001_jobs_schema,
    }],
};

fn migration_0001_jobs_schema(tx: &Transaction<'_>) -> Result<(), rusqlite::Error> {
    tx.execute_batch(
        r"
        CREATE TABLE detached_jobs (
            job_id TEXT PRIMARY KEY,
            realm_id TEXT NOT NULL,
            submission_key TEXT NOT NULL,
            revision BLOB NOT NULL CHECK (length(revision) = 8),
            has_pending_outbox INTEGER NOT NULL CHECK (has_pending_outbox IN (0, 1)),
            job_json BLOB NOT NULL,
            UNIQUE (realm_id, submission_key)
        );
        CREATE INDEX idx_detached_jobs_pending_outbox
            ON detached_jobs (has_pending_outbox, job_id);
        ",
    )
}

#[derive(Debug, Clone)]
pub struct SqliteDetachedJobStore {
    path: PathBuf,
}

impl SqliteDetachedJobStore {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, DetachedJobError> {
        let store = Self { path: path.into() };
        store.with_connection(|_| Ok(()))?;
        Ok(store)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn with_connection<T>(
        &self,
        f: impl FnOnce(&mut Connection) -> Result<T, DetachedJobError>,
    ) -> Result<T, DetachedJobError> {
        let _guard =
            meerkat_sqlite::OperationGuard::for_database(&self.path).map_err(sqlite_store_error)?;
        let mut conn = meerkat_sqlite::open_with(
            &self.path,
            meerkat_sqlite::ConnectionProfile::PRIMARY,
            meerkat_sqlite::OpenOptions {
                schema_preflight: &[&JOBS_DOMAIN],
                ..Default::default()
            },
        )
        .map_err(sqlite_store_error)?;
        meerkat_sqlite::apply_domain_migrations(&mut conn, &JOBS_DOMAIN)
            .map_err(sqlite_store_error)?;
        f(&mut conn)
    }
}

#[async_trait]
impl DetachedJobStore for SqliteDetachedJobStore {
    async fn insert_deduplicated(
        &self,
        job: StoredJob,
    ) -> Result<InsertJobOutcome, DetachedJobError> {
        validate_stored_job(&job)?;
        self.with_connection(|conn| {
            let tx = meerkat_sqlite::begin_immediate(conn).map_err(sqlite_store_error)?;
            let existing = select_by_submission(&tx, &job.spec.realm_id, &job.spec.submission_key)?;
            if let Some(existing) = existing {
                if !existing.spec.equivalent_submission(&job.spec) {
                    return Err(DetachedJobError::SubmissionConflict);
                }
                tx.commit().map_err(raw_sqlite_error)?;
                return Ok(InsertJobOutcome::Existing(existing));
            }
            let encoded = encode_job(&job)?;
            let pending = has_pending_outbox(&job);
            let revision = revision_bytes(job.revision);
            tx.execute(
                "INSERT INTO detached_jobs
                    (job_id, realm_id, submission_key, revision, has_pending_outbox, job_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    job.job_id.as_str(),
                    job.spec.realm_id,
                    job.spec.submission_key.as_str(),
                    revision.as_slice(),
                    pending,
                    encoded,
                ],
            )
            .map_err(raw_sqlite_error)?;
            tx.commit().map_err(raw_sqlite_error)?;
            Ok(InsertJobOutcome::Inserted(job))
        })
    }

    async fn get(&self, job_id: &JobId) -> Result<Option<StoredJob>, DetachedJobError> {
        self.with_connection(|conn| select_by_id(conn, job_id))
    }

    async fn compare_and_swap(
        &self,
        expected_revision: u64,
        mut replacement: StoredJob,
    ) -> Result<StoredJob, DetachedJobError> {
        self.with_connection(|conn| {
            let tx = meerkat_sqlite::begin_immediate(conn).map_err(sqlite_store_error)?;
            let current = select_by_id(&tx, &replacement.job_id)?
                .ok_or_else(|| DetachedJobError::NotFound(replacement.job_id.clone()))?;
            if current.revision != expected_revision {
                return Err(DetachedJobError::StaleRevision {
                    job_id: replacement.job_id.clone(),
                    expected: expected_revision,
                    actual: current.revision,
                });
            }
            if current.spec.submission_key != replacement.spec.submission_key
                || !current.spec.equivalent_submission(&replacement.spec)
            {
                return Err(DetachedJobError::Store(
                    "compare-and-swap cannot change the submitted job specification".into(),
                ));
            }
            validate_stored_job(&replacement)?;
            replacement.revision = next_revision(expected_revision)?;
            let encoded = encode_job(&replacement)?;
            let pending = has_pending_outbox(&replacement);
            let replacement_revision = revision_bytes(replacement.revision);
            let expected_revision_bytes = revision_bytes(expected_revision);
            let changed = tx
                .execute(
                    "UPDATE detached_jobs
                        SET revision = ?2, has_pending_outbox = ?3, job_json = ?4
                      WHERE job_id = ?1 AND revision = ?5",
                    params![
                        replacement.job_id.as_str(),
                        replacement_revision.as_slice(),
                        pending,
                        encoded,
                        expected_revision_bytes.as_slice(),
                    ],
                )
                .map_err(raw_sqlite_error)?;
            if changed != 1 {
                let actual = current_revision(&tx, &replacement.job_id)?.unwrap_or_default();
                return Err(DetachedJobError::StaleRevision {
                    job_id: replacement.job_id.clone(),
                    expected: expected_revision,
                    actual,
                });
            }
            tx.commit().map_err(raw_sqlite_error)?;
            Ok(replacement)
        })
    }

    async fn list_pending_outbox(
        &self,
        limit: usize,
    ) -> Result<Vec<JobOutboxEntry>, DetachedJobError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        self.with_connection(|conn| {
            let mut statement = conn
                .prepare(
                    "SELECT job_id, realm_id, submission_key, revision,
                            has_pending_outbox, job_json
                       FROM detached_jobs
                      WHERE has_pending_outbox = 1
                      ORDER BY job_id",
                )
                .map_err(raw_sqlite_error)?;
            let mut rows = statement.query([]).map_err(raw_sqlite_error)?;
            let mut pending = Vec::with_capacity(limit);
            while pending.len() < limit {
                let Some(row) = rows.next().map_err(raw_sqlite_error)? else {
                    break;
                };
                let job = decode_job_row(row)?;
                for entry in job.outbox.into_iter().filter(|entry| !entry.applied) {
                    pending.push(entry);
                    if pending.len() == limit {
                        break;
                    }
                }
            }
            Ok(pending)
        })
    }
}

fn select_by_id(conn: &Connection, job_id: &JobId) -> Result<Option<StoredJob>, DetachedJobError> {
    conn.query_row(
        "SELECT job_id, realm_id, submission_key, revision, has_pending_outbox, job_json
           FROM detached_jobs
          WHERE job_id = ?1",
        [job_id.as_str()],
        decode_job_row_sql,
    )
    .optional()
    .map_err(raw_sqlite_error)?
    .map(decode_checked_row)
    .transpose()
}

fn select_by_submission(
    conn: &Connection,
    realm_id: &str,
    submission_key: &crate::JobSubmissionKey,
) -> Result<Option<StoredJob>, DetachedJobError> {
    conn.query_row(
        "SELECT job_id, realm_id, submission_key, revision, has_pending_outbox, job_json
           FROM detached_jobs
          WHERE realm_id = ?1 AND submission_key = ?2",
        params![realm_id, submission_key.as_str()],
        decode_job_row_sql,
    )
    .optional()
    .map_err(raw_sqlite_error)?
    .map(decode_checked_row)
    .transpose()
}

type EncodedJobRow = (String, String, String, Vec<u8>, bool, Vec<u8>);

fn decode_job_row_sql(row: &rusqlite::Row<'_>) -> Result<EncodedJobRow, rusqlite::Error> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get::<_, meerkat_sqlite::JsonColumnBytes>(5)?
            .into_bytes(),
    ))
}

fn decode_job_row(row: &rusqlite::Row<'_>) -> Result<StoredJob, DetachedJobError> {
    decode_checked_row(decode_job_row_sql(row).map_err(raw_sqlite_error)?)
}

fn decode_checked_row(
    (job_id, realm_id, submission_key, encoded_revision, pending, encoded): EncodedJobRow,
) -> Result<StoredJob, DetachedJobError> {
    let revision = revision_from_bytes(&encoded_revision)?;
    let envelope: StoredJobEnvelope = serde_json::from_slice(&encoded)
        .map_err(|error| DetachedJobError::Store(format!("stored job JSON is invalid: {error}")))?;
    if envelope.format_version != STORED_JOB_FORMAT_VERSION {
        return Err(DetachedJobError::Store(format!(
            "stored job format version {} is unsupported; this binary supports {}",
            envelope.format_version, STORED_JOB_FORMAT_VERSION
        )));
    }
    let job = StoredJob::try_from(envelope.job)?;
    if job.job_id.as_str() != job_id
        || job.spec.realm_id != realm_id
        || job.spec.submission_key.as_str() != submission_key
        || job.revision != revision
        || has_pending_outbox(&job) != pending
    {
        return Err(DetachedJobError::Store(format!(
            "detached job row columns disagree with encoded job {job_id}"
        )));
    }
    validate_stored_job(&job)?;
    Ok(job)
}

fn current_revision(conn: &Connection, job_id: &JobId) -> Result<Option<u64>, DetachedJobError> {
    let encoded = conn
        .query_row(
            "SELECT revision FROM detached_jobs WHERE job_id = ?1",
            [job_id.as_str()],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional()
        .map_err(raw_sqlite_error)?;
    encoded.map(|bytes| revision_from_bytes(&bytes)).transpose()
}

fn encode_job(job: &StoredJob) -> Result<Vec<u8>, DetachedJobError> {
    serde_json::to_vec(&StoredJobEnvelope {
        format_version: STORED_JOB_FORMAT_VERSION,
        job: PersistedStoredJob::from(job),
    })
    .map_err(|error| DetachedJobError::Store(format!("cannot encode stored job: {error}")))
}

fn has_pending_outbox(job: &StoredJob) -> bool {
    job.outbox.iter().any(|entry| !entry.applied)
}

fn sqlite_store_error(error: meerkat_sqlite::SqliteStoreError) -> DetachedJobError {
    DetachedJobError::Sqlite(error)
}

fn raw_sqlite_error(error: rusqlite::Error) -> DetachedJobError {
    DetachedJobError::Sqlite(error.into())
}

fn revision_bytes(revision: u64) -> [u8; 8] {
    revision.to_be_bytes()
}

fn revision_from_bytes(bytes: &[u8]) -> Result<u64, DetachedJobError> {
    let encoded: [u8; 8] = bytes.try_into().map_err(|_| {
        DetachedJobError::Store(format!(
            "stored detached job revision has {} bytes; expected 8",
            bytes.len()
        ))
    })?;
    Ok(u64::from_be_bytes(encoded))
}

impl From<&StoredJob> for PersistedStoredJob {
    fn from(job: &StoredJob) -> Self {
        Self {
            job_id: job.job_id.clone(),
            spec: PersistedJobSpec::from(&job.spec),
            revision: job.revision,
            machine_state: PersistedMachineState::from(&job.machine_state),
            progress: job.progress.clone(),
            terminal_result: job.terminal_result.clone(),
            outbox: job
                .outbox
                .iter()
                .map(PersistedJobOutboxEntry::from)
                .collect(),
        }
    }
}

impl TryFrom<PersistedStoredJob> for StoredJob {
    type Error = DetachedJobError;

    fn try_from(job: PersistedStoredJob) -> Result<Self, Self::Error> {
        Ok(Self {
            job_id: job.job_id,
            spec: JobSpec::from(job.spec),
            revision: job.revision,
            machine_state: DetachedJobMachineState::from(job.machine_state),
            progress: job.progress,
            terminal_result: job.terminal_result,
            outbox: job.outbox.into_iter().map(JobOutboxEntry::from).collect(),
        })
    }
}

impl From<&JobSpec> for PersistedJobSpec {
    fn from(spec: &JobSpec) -> Self {
        Self {
            realm_id: spec.realm_id.clone(),
            origin_session_id: spec.origin_session_id.clone(),
            origin_member_id: spec.origin_member_id.clone(),
            execution_intent_id: spec.execution_intent_id.clone(),
            interaction_lineage_id: spec.interaction_lineage_id.clone(),
            tool: spec.tool.clone(),
            runner: spec.runner.clone(),
            restart_class: PersistedRestartClass::from(spec.restart_class),
            canonical_arguments_hash: spec.canonical_arguments_hash.clone(),
            credential_context_refs: spec
                .credential_context_refs
                .iter()
                .map(PersistedCredentialContextRef::from)
                .collect(),
            submission_key: spec.submission_key.clone(),
        }
    }
}

impl From<PersistedJobSpec> for JobSpec {
    fn from(spec: PersistedJobSpec) -> Self {
        Self {
            realm_id: spec.realm_id,
            origin_session_id: spec.origin_session_id,
            origin_member_id: spec.origin_member_id,
            execution_intent_id: spec.execution_intent_id,
            interaction_lineage_id: spec.interaction_lineage_id,
            tool: spec.tool,
            runner: spec.runner,
            restart_class: DetachedJobRestartClass::from(spec.restart_class),
            canonical_arguments_hash: spec.canonical_arguments_hash,
            credential_context_refs: spec
                .credential_context_refs
                .into_iter()
                .map(ToolCredentialContextRef::from)
                .collect(),
            submission_key: spec.submission_key,
        }
    }
}

impl From<&ToolCredentialContextRef> for PersistedCredentialContextRef {
    fn from(reference: &ToolCredentialContextRef) -> Self {
        match reference {
            ToolCredentialContextRef::OwningProfile { required_scopes } => Self::OwningProfile {
                required_scopes: required_scopes.clone(),
            },
            ToolCredentialContextRef::AuthBinding {
                auth_binding,
                required_scopes,
            } => Self::AuthBinding {
                auth_binding: auth_binding.clone(),
                required_scopes: required_scopes.clone(),
            },
        }
    }
}

impl From<PersistedCredentialContextRef> for ToolCredentialContextRef {
    fn from(reference: PersistedCredentialContextRef) -> Self {
        match reference {
            PersistedCredentialContextRef::OwningProfile { required_scopes } => {
                Self::OwningProfile { required_scopes }
            }
            PersistedCredentialContextRef::AuthBinding {
                auth_binding,
                required_scopes,
            } => Self::AuthBinding {
                auth_binding,
                required_scopes,
            },
        }
    }
}

impl From<DetachedJobRestartClass> for PersistedRestartClass {
    fn from(value: DetachedJobRestartClass) -> Self {
        match value {
            DetachedJobRestartClass::Adoptable => Self::Adoptable,
            DetachedJobRestartClass::CheckpointResumable => Self::CheckpointResumable,
            DetachedJobRestartClass::Replayable => Self::Replayable,
            DetachedJobRestartClass::NonResumable => Self::NonResumable,
        }
    }
}

impl From<PersistedRestartClass> for DetachedJobRestartClass {
    fn from(value: PersistedRestartClass) -> Self {
        match value {
            PersistedRestartClass::Adoptable => Self::Adoptable,
            PersistedRestartClass::CheckpointResumable => Self::CheckpointResumable,
            PersistedRestartClass::Replayable => Self::Replayable,
            PersistedRestartClass::NonResumable => Self::NonResumable,
        }
    }
}

impl From<DetachedJobPhase> for PersistedPhase {
    fn from(value: DetachedJobPhase) -> Self {
        match value {
            DetachedJobPhase::Unsubmitted => Self::Unsubmitted,
            DetachedJobPhase::Queued => Self::Queued,
            DetachedJobPhase::Claimed => Self::Claimed,
            DetachedJobPhase::Running => Self::Running,
            DetachedJobPhase::WaitingExternal => Self::WaitingExternal,
            DetachedJobPhase::LossObserved => Self::LossObserved,
            DetachedJobPhase::RetryScheduled => Self::RetryScheduled,
            DetachedJobPhase::Succeeded => Self::Succeeded,
            DetachedJobPhase::Failed => Self::Failed,
            DetachedJobPhase::Cancelled => Self::Cancelled,
            DetachedJobPhase::WorkerLost => Self::WorkerLost,
            DetachedJobPhase::NeedsAttention => Self::NeedsAttention,
        }
    }
}

impl From<PersistedPhase> for DetachedJobPhase {
    fn from(value: PersistedPhase) -> Self {
        match value {
            PersistedPhase::Unsubmitted => Self::Unsubmitted,
            PersistedPhase::Queued => Self::Queued,
            PersistedPhase::Claimed => Self::Claimed,
            PersistedPhase::Running => Self::Running,
            PersistedPhase::WaitingExternal => Self::WaitingExternal,
            PersistedPhase::LossObserved => Self::LossObserved,
            PersistedPhase::RetryScheduled => Self::RetryScheduled,
            PersistedPhase::Succeeded => Self::Succeeded,
            PersistedPhase::Failed => Self::Failed,
            PersistedPhase::Cancelled => Self::Cancelled,
            PersistedPhase::WorkerLost => Self::WorkerLost,
            PersistedPhase::NeedsAttention => Self::NeedsAttention,
        }
    }
}

impl From<DetachedJobTerminalKind> for PersistedTerminalKind {
    fn from(value: DetachedJobTerminalKind) -> Self {
        match value {
            DetachedJobTerminalKind::Succeeded => Self::Succeeded,
            DetachedJobTerminalKind::Failed => Self::Failed,
            DetachedJobTerminalKind::Cancelled => Self::Cancelled,
            DetachedJobTerminalKind::WorkerLost => Self::WorkerLost,
            DetachedJobTerminalKind::NeedsAttention => Self::NeedsAttention,
        }
    }
}

impl From<PersistedTerminalKind> for DetachedJobTerminalKind {
    fn from(value: PersistedTerminalKind) -> Self {
        match value {
            PersistedTerminalKind::Succeeded => Self::Succeeded,
            PersistedTerminalKind::Failed => Self::Failed,
            PersistedTerminalKind::Cancelled => Self::Cancelled,
            PersistedTerminalKind::WorkerLost => Self::WorkerLost,
            PersistedTerminalKind::NeedsAttention => Self::NeedsAttention,
        }
    }
}

impl From<&DetachedJobMachineState> for PersistedMachineState {
    fn from(state: &DetachedJobMachineState) -> Self {
        Self {
            lifecycle_phase: PersistedPhase::from(state.lifecycle_phase),
            job_id: state.job_id.clone(),
            restart_class: PersistedRestartClass::from(state.restart_class),
            attempt_count: state.attempt_count,
            current_attempt_id: state.current_attempt_id.clone(),
            current_fence: state.current_fence,
            current_worker_id: state.current_worker_id.clone(),
            lease_expires_at_ms: state.lease_expires_at_ms,
            heartbeat_at_ms: state.heartbeat_at_ms,
            checkpoint_ref: state.checkpoint_ref.clone(),
            runner_handle: state.runner_handle.clone(),
            progress_cursor: state.progress_cursor,
            lease_expired: state.lease_expired,
            retry_due_at_ms: state.retry_due_at_ms,
            cancel_requested: state.cancel_requested,
            terminal_kind: state.terminal_kind.map(PersistedTerminalKind::from),
            terminal_delivery_sequence: state.terminal_delivery_sequence,
            terminal_delivery_applied: state.terminal_delivery_applied,
        }
    }
}

impl From<PersistedMachineState> for DetachedJobMachineState {
    fn from(state: PersistedMachineState) -> Self {
        Self {
            lifecycle_phase: DetachedJobPhase::from(state.lifecycle_phase),
            job_id: state.job_id,
            restart_class: DetachedJobRestartClass::from(state.restart_class),
            attempt_count: state.attempt_count,
            current_attempt_id: state.current_attempt_id,
            current_fence: state.current_fence,
            current_worker_id: state.current_worker_id,
            lease_expires_at_ms: state.lease_expires_at_ms,
            heartbeat_at_ms: state.heartbeat_at_ms,
            checkpoint_ref: state.checkpoint_ref,
            runner_handle: state.runner_handle,
            progress_cursor: state.progress_cursor,
            lease_expired: state.lease_expired,
            retry_due_at_ms: state.retry_due_at_ms,
            cancel_requested: state.cancel_requested,
            terminal_kind: state.terminal_kind.map(DetachedJobTerminalKind::from),
            terminal_delivery_sequence: state.terminal_delivery_sequence,
            terminal_delivery_applied: state.terminal_delivery_applied,
        }
    }
}

impl From<&JobOutboxEntry> for PersistedJobOutboxEntry {
    fn from(entry: &JobOutboxEntry) -> Self {
        Self {
            job_id: entry.job_id.clone(),
            delivery_sequence: entry.delivery_sequence,
            terminal_kind: PersistedTerminalKind::from(entry.terminal_kind),
            terminal_result: entry.terminal_result.clone(),
            applied: entry.applied,
        }
    }
}

impl From<PersistedJobOutboxEntry> for JobOutboxEntry {
    fn from(entry: PersistedJobOutboxEntry) -> Self {
        Self {
            job_id: entry.job_id,
            delivery_sequence: entry.delivery_sequence,
            terminal_kind: DetachedJobTerminalKind::from(entry.terminal_kind),
            terminal_result: entry.terminal_result,
            applied: entry.applied,
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    #[test]
    fn revision_encoding_round_trips_the_full_u64_domain() {
        for revision in [1, i64::MAX as u64, i64::MAX as u64 + 1, u64::MAX] {
            assert_eq!(
                super::revision_from_bytes(&super::revision_bytes(revision)).expect("decode"),
                revision
            );
        }
    }
}
