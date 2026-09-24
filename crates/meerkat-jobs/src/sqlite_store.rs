use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use meerkat_core::{AuthBindingRef, SessionId, ToolCredentialContextRef};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};

use crate::machines::detached_job::{
    DetachedJobMachineState, DetachedJobPhase, DetachedJobRestartClass, DetachedJobTerminalKind,
};
use crate::store::{
    PredicateDeliveryCommitOutcome, ensure_same_predicate_delivery_identity, next_revision,
    validate_job_replacement, validate_predicate_delivery_receipt, validate_stored_job,
};
use crate::{
    CanonicalArgumentsHash, DetachedJobError, DetachedJobStore, ExecutionIntentId,
    InsertJobOutcome, InteractionLineageId, JobId, JobOutboxEntry, JobOutboxPayload, JobProgress,
    JobSpec, JobSubmissionKey, JobSubscription, JobTerminalResult, OriginMemberId,
    PredicateDeliveryCommit, PredicateDeliveryIdentity, PredicateDeliveryReceipt, RunnerIdentity,
    RunnerSpecificationRef, StoredJob, ToolIdentity,
};

const STORED_JOB_FORMAT_VERSION: u32 = 1;
const PREDICATE_DELIVERY_RECEIPT_FORMAT_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
struct StoredJobEnvelope {
    format_version: u32,
    job: PersistedStoredJob,
}

#[derive(Debug, Serialize, Deserialize)]
struct PredicateDeliveryReceiptEnvelope {
    format_version: u32,
    receipt: PredicateDeliveryReceipt,
}

#[derive(Debug, Serialize, Deserialize)]
struct PersistedStoredJob {
    job_id: JobId,
    spec: PersistedJobSpec,
    revision: u64,
    machine_state: PersistedMachineState,
    progress: Option<JobProgress>,
    terminal_result: Option<JobTerminalResult>,
    #[serde(default)]
    subscriptions: Vec<JobSubscription>,
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
    #[serde(default)]
    runner_specification_ref: Option<RunnerSpecificationRef>,
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
    #[serde(default)]
    delivery_sequence: u64,
    #[serde(default)]
    notification_ids: BTreeSet<String>,
    #[serde(default)]
    notification_idempotency_keys: BTreeSet<String>,
    #[serde(default)]
    notification_id_by_key: BTreeMap<String, String>,
    #[serde(default)]
    notification_delivery_ids: BTreeMap<String, String>,
    #[serde(default)]
    notification_sequences: BTreeMap<String, u64>,
    #[serde(default)]
    notification_applied: BTreeSet<String>,
    terminal_kind: Option<PersistedTerminalKind>,
    terminal_delivery_sequence: u64,
    terminal_delivery_applied: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct PersistedJobOutboxEntry {
    job_id: JobId,
    #[serde(default)]
    delivery_id: Option<String>,
    delivery_sequence: u64,
    #[serde(default)]
    payload: Option<JobOutboxPayload>,
    #[serde(default)]
    terminal_kind: Option<PersistedTerminalKind>,
    #[serde(default)]
    terminal_result: Option<JobTerminalResult>,
    #[serde(default)]
    targets: Vec<JobSubscription>,
    applied: bool,
}

pub const JOBS_DOMAIN: meerkat_sqlite::SchemaDomain = meerkat_sqlite::SchemaDomain {
    name: "jobs",
    migrations: &[
        meerkat_sqlite::Migration {
            version: 1,
            name: "base-schema",
            apply: migration_0001_jobs_schema,
        },
        meerkat_sqlite::Migration {
            version: 2,
            name: "notification-outbox-and-subscriptions",
            apply: migration_0002_notification_outbox_and_subscriptions,
        },
        meerkat_sqlite::Migration {
            version: 3,
            name: "predicate-delivery-ledger",
            apply: migration_0003_predicate_delivery_ledger,
        },
        meerkat_sqlite::Migration {
            version: 4,
            name: "census-live-projection",
            apply: migration_0004_census_live_projection,
        },
    ],
    initialize_current: initialize_current_jobs_schema,
    allowed_existing_versions: &[2, 3, 4],
    bridge_recoverable_versions: &[1],
    released_predecessors: &[
        meerkat_sqlite::SchemaPredecessor {
            version: 2,
            verify: verify_released_jobs_v2_schema,
        },
        meerkat_sqlite::SchemaPredecessor {
            version: 3,
            verify: verify_released_jobs_v3_schema,
        },
    ],
    owned_objects: &[
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Table,
            name: "detached_jobs",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Index,
            name: "idx_detached_jobs_pending_outbox",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Index,
            name: "idx_detached_jobs_census_live",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Table,
            name: "detached_job_predicate_deliveries",
        },
    ],
    retired_objects: &[],
};

fn initialize_current_jobs_schema(tx: &Transaction<'_>) -> Result<(), rusqlite::Error> {
    migration_0001_jobs_schema(tx)?;
    migration_0002_notification_outbox_and_subscriptions(tx)?;
    migration_0003_predicate_delivery_ledger(tx)?;
    migration_0004_census_live_projection(tx)
}

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

fn migration_0002_notification_outbox_and_subscriptions(
    _tx: &Transaction<'_>,
) -> Result<(), rusqlite::Error> {
    // The durable document remains in the existing job_json envelope, but
    // v2 stamps the first writer that may persist notification/subscription
    // fields. Older binaries therefore refuse the database before attempting
    // to decode a row they cannot understand.
    Ok(())
}

const RELEASED_JOBS_V2_OBJECTS: &[meerkat_sqlite::SchemaObject] = &[
    meerkat_sqlite::SchemaObject {
        kind: meerkat_sqlite::SchemaObjectKind::Table,
        name: "detached_jobs",
    },
    meerkat_sqlite::SchemaObject {
        kind: meerkat_sqlite::SchemaObjectKind::Index,
        name: "idx_detached_jobs_pending_outbox",
    },
];

fn verify_released_jobs_v2_schema(conn: &Connection) -> Result<(), String> {
    meerkat_sqlite::verify_released_schema_fingerprint(
        conn,
        &JOBS_DOMAIN,
        RELEASED_JOBS_V2_OBJECTS,
        migration_0001_jobs_schema,
    )
}

const RELEASED_JOBS_V3_OBJECTS: &[meerkat_sqlite::SchemaObject] = &[
    meerkat_sqlite::SchemaObject {
        kind: meerkat_sqlite::SchemaObjectKind::Table,
        name: "detached_jobs",
    },
    meerkat_sqlite::SchemaObject {
        kind: meerkat_sqlite::SchemaObjectKind::Index,
        name: "idx_detached_jobs_pending_outbox",
    },
    meerkat_sqlite::SchemaObject {
        kind: meerkat_sqlite::SchemaObjectKind::Table,
        name: "detached_job_predicate_deliveries",
    },
];

/// Frozen v3 catalog: the released shape this binary may migrate FROM.
///
/// Deliberately its own builder rather than a call into
/// [`initialize_current_jobs_schema`]. That function tracks the current
/// version and would drift into "whatever we ship today", which verifies
/// nothing.
fn build_released_jobs_v3_schema(tx: &Transaction<'_>) -> Result<(), rusqlite::Error> {
    migration_0001_jobs_schema(tx)?;
    migration_0002_notification_outbox_and_subscriptions(tx)?;
    migration_0003_predicate_delivery_ledger(tx)
}

fn verify_released_jobs_v3_schema(conn: &Connection) -> Result<(), String> {
    meerkat_sqlite::verify_released_schema_fingerprint(
        conn,
        &JOBS_DOMAIN,
        RELEASED_JOBS_V3_OBJECTS,
        build_released_jobs_v3_schema,
    )
}

fn migration_0003_predicate_delivery_ledger(tx: &Transaction<'_>) -> Result<(), rusqlite::Error> {
    tx.execute_batch(
        r"
        CREATE TABLE detached_job_predicate_deliveries (
            job_id TEXT NOT NULL,
            delivery_idempotency_key TEXT NOT NULL,
            occurrence_id TEXT NOT NULL,
            runnable TEXT NOT NULL,
            committed_revision BLOB NOT NULL CHECK (length(committed_revision) = 8),
            receipt_json BLOB NOT NULL,
            PRIMARY KEY (job_id, delivery_idempotency_key)
        );
        ",
    )
}

/// Project census relevance out of the job document into an indexed column.
///
/// Without it the operational census has no way to ask for live rows: phase
/// lives inside `job_json`, so the only available window was "the first N rows
/// by primary key", and job ids are time-ordered - the window filled with the
/// oldest, most settled rows first and went blind as retention grew.
///
/// The column is named for the question it answers, not for machine
/// terminality: [`crate::store::phase_is_census_live`] keeps `NeedsAttention`
/// live even though the machine calls it terminal, because a job parked for a
/// human is one of the three conditions that degrade the census.
fn migration_0004_census_live_projection(tx: &Transaction<'_>) -> Result<(), rusqlite::Error> {
    tx.execute_batch(
        r"
        ALTER TABLE detached_jobs
            ADD COLUMN census_live INTEGER NOT NULL DEFAULT 1
                CHECK (census_live IN (0, 1));
        CREATE INDEX idx_detached_jobs_census_live
            ON detached_jobs (realm_id, job_id)
            WHERE census_live = 1;
        ",
    )?;
    backfill_census_live(tx)
}

/// Classify every existing row.
///
/// This is the load-bearing half of the migration. Settled rows are never
/// rewritten again, so a backfill that classified nothing would leave every
/// pre-existing store exactly as blind as before while reporting a successful
/// migration - the schema would move and the defect would not.
///
/// A row this cannot decode stays LIVE (the column default). That is the safe
/// direction and the reason the probe below is a frozen, minimal shape rather
/// than the full document decoder: mis-marking a settled row as live costs
/// window space, while mis-marking a live row as settled hides a wedged job,
/// which is the defect itself.
fn backfill_census_live(tx: &Transaction<'_>) -> Result<(), rusqlite::Error> {
    let settled = {
        let mut statement = tx.prepare("SELECT job_id, job_json FROM detached_jobs")?;
        let mut rows = statement.query([])?;
        let mut settled: Vec<String> = Vec::new();
        while let Some(row) = rows.next()? {
            let job_id: String = row.get(0)?;
            let encoded = row
                .get::<_, meerkat_sqlite::JsonColumnBytes>(1)?
                .into_bytes();
            if probe_settled_phase(&encoded) {
                settled.push(job_id);
            }
        }
        settled
    };
    let mut update = tx.prepare("UPDATE detached_jobs SET census_live = 0 WHERE job_id = ?1")?;
    for job_id in settled {
        update.execute([job_id])?;
    }
    Ok(())
}

/// Frozen migration-local view of the stored document: just the phase.
///
/// Pinned here rather than reusing [`StoredJobEnvelope`] so that later changes
/// to the document type cannot change what this already-released migration
/// does to a database. If the shape ever stops matching, decoding fails and
/// every row stays live - loud in cost, silent in nothing.
fn probe_settled_phase(encoded: &[u8]) -> bool {
    #[derive(Deserialize)]
    struct PhaseProbeEnvelope {
        job: PhaseProbeJob,
    }
    #[derive(Deserialize)]
    struct PhaseProbeJob {
        machine_state: PhaseProbeState,
    }
    #[derive(Deserialize)]
    struct PhaseProbeState {
        lifecycle_phase: PersistedPhase,
    }

    serde_json::from_slice::<PhaseProbeEnvelope>(encoded).is_ok_and(|envelope| {
        !crate::store::phase_is_census_live(DetachedJobPhase::from(
            envelope.job.machine_state.lifecycle_phase,
        ))
    })
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

    pub const fn is_persistent(&self) -> bool {
        true
    }

    pub async fn list_for_origin(
        &self,
        realm_id: &str,
        origin_session_id: &SessionId,
        limit: usize,
    ) -> Result<Vec<StoredJob>, DetachedJobError> {
        DetachedJobStore::list_for_origin(self, realm_id, origin_session_id, limit).await
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
            let census_live = crate::store::job_is_census_live(&job);
            let revision = revision_bytes(job.revision);
            tx.execute(
                "INSERT INTO detached_jobs
                    (job_id, realm_id, submission_key, revision, has_pending_outbox,
                     census_live, job_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    job.job_id.as_str(),
                    job.spec.realm_id,
                    job.spec.submission_key.as_str(),
                    revision.as_slice(),
                    pending,
                    census_live,
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
            let census_live = crate::store::job_is_census_live(&replacement);
            let replacement_revision = revision_bytes(replacement.revision);
            let expected_revision_bytes = revision_bytes(expected_revision);
            let changed = tx
                .execute(
                    "UPDATE detached_jobs
                        SET revision = ?2, has_pending_outbox = ?3, job_json = ?4,
                            census_live = ?6
                      WHERE job_id = ?1 AND revision = ?5",
                    params![
                        replacement.job_id.as_str(),
                        replacement_revision.as_slice(),
                        pending,
                        encoded,
                        expected_revision_bytes.as_slice(),
                        census_live,
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

    async fn predicate_delivery_receipt(
        &self,
        job_id: &JobId,
        identity: &PredicateDeliveryIdentity,
    ) -> Result<Option<PredicateDeliveryReceipt>, DetachedJobError> {
        self.with_connection(|conn| {
            let tx = conn.transaction().map_err(raw_sqlite_error)?;
            let Some(job) = select_by_id(&tx, job_id)? else {
                return Err(DetachedJobError::NotFound(job_id.clone()));
            };
            let receipt = select_predicate_delivery_receipt(
                &tx,
                job_id,
                identity.idempotency_key().as_str(),
            )?;
            if let Some(receipt) = &receipt {
                ensure_same_predicate_delivery_identity(job_id, &receipt.identity, identity)?;
                validate_predicate_delivery_receipt(&job, identity.idempotency_key(), receipt)?;
            }
            tx.commit().map_err(raw_sqlite_error)?;
            Ok(receipt)
        })
    }

    async fn commit_predicate_delivery(
        &self,
        expected_revision: u64,
        mut replacement: StoredJob,
        commit: PredicateDeliveryCommit,
    ) -> Result<PredicateDeliveryCommitOutcome, DetachedJobError> {
        self.with_connection(|conn| {
            let tx = meerkat_sqlite::begin_immediate(conn).map_err(sqlite_store_error)?;
            let current = select_by_id(&tx, &replacement.job_id)?
                .ok_or_else(|| DetachedJobError::NotFound(replacement.job_id.clone()))?;
            if let Some(receipt) = select_predicate_delivery_receipt(
                &tx,
                &replacement.job_id,
                commit.identity.idempotency_key().as_str(),
            )? {
                ensure_same_predicate_delivery_identity(
                    &replacement.job_id,
                    &receipt.identity,
                    &commit.identity,
                )?;
                validate_predicate_delivery_receipt(
                    &current,
                    commit.identity.idempotency_key(),
                    &receipt,
                )?;
                tx.commit().map_err(raw_sqlite_error)?;
                return Ok(PredicateDeliveryCommitOutcome::Deduplicated {
                    job: current,
                    receipt,
                });
            }
            validate_job_replacement(&current, expected_revision, &replacement)?;
            replacement.revision = next_revision(expected_revision)?;
            let receipt = PredicateDeliveryReceipt {
                job_id: replacement.job_id.clone(),
                identity: commit.identity,
                committed_revision: replacement.revision,
                evaluation: commit.evaluation,
                notification: commit.notification,
            };
            validate_predicate_delivery_receipt(
                &replacement,
                receipt.identity.idempotency_key(),
                &receipt,
            )?;

            let encoded_job = encode_job(&replacement)?;
            let pending = has_pending_outbox(&replacement);
            let census_live = crate::store::job_is_census_live(&replacement);
            let replacement_revision = revision_bytes(replacement.revision);
            let expected_revision_bytes = revision_bytes(expected_revision);
            let changed = tx
                .execute(
                    "UPDATE detached_jobs
                        SET revision = ?2, has_pending_outbox = ?3, job_json = ?4,
                            census_live = ?6
                      WHERE job_id = ?1 AND revision = ?5",
                    params![
                        replacement.job_id.as_str(),
                        replacement_revision.as_slice(),
                        pending,
                        encoded_job,
                        expected_revision_bytes.as_slice(),
                        census_live,
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
            let encoded_receipt = encode_predicate_delivery_receipt(&receipt)?;
            tx.execute(
                "INSERT INTO detached_job_predicate_deliveries
                    (job_id, delivery_idempotency_key, occurrence_id, runnable,
                     committed_revision, receipt_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    replacement.job_id.as_str(),
                    receipt.identity.idempotency_key().as_str(),
                    receipt.identity.occurrence_id(),
                    receipt.identity.runnable(),
                    replacement_revision.as_slice(),
                    encoded_receipt,
                ],
            )
            .map_err(raw_sqlite_error)?;
            tx.commit().map_err(raw_sqlite_error)?;
            Ok(PredicateDeliveryCommitOutcome::Committed {
                job: replacement,
                receipt,
            })
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
                            has_pending_outbox, census_live, job_json
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

    async fn list_for_origin(
        &self,
        realm_id: &str,
        origin_session_id: &SessionId,
        limit: usize,
    ) -> Result<Vec<StoredJob>, DetachedJobError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        self.with_connection(|conn| {
            let mut statement = conn
                .prepare(
                    "SELECT job_id, realm_id, submission_key, revision,
                            has_pending_outbox, census_live, job_json
                       FROM detached_jobs
                      WHERE realm_id = ?1
                      ORDER BY job_id",
                )
                .map_err(raw_sqlite_error)?;
            let mut rows = statement.query([realm_id]).map_err(raw_sqlite_error)?;
            // Recovery deliberately requests an unbounded origin view. Avoid
            // turning that semantic limit into an impossible eager allocation.
            let mut jobs = Vec::new();
            while jobs.len() < limit {
                let Some(row) = rows.next().map_err(raw_sqlite_error)? else {
                    break;
                };
                let job = decode_job_row(row)?;
                if &job.spec.origin_session_id == origin_session_id {
                    jobs.push(job);
                }
            }
            Ok(jobs)
        })
    }

    async fn list_all(&self, limit: usize) -> Result<Vec<StoredJob>, DetachedJobError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        self.with_connection(|conn| {
            let limit = i64::try_from(limit).unwrap_or(i64::MAX);
            let mut statement = conn
                .prepare(
                    "SELECT job_id, realm_id, submission_key, revision,
                            has_pending_outbox, census_live, job_json
                       FROM detached_jobs
                      ORDER BY job_id
                      LIMIT ?1",
                )
                .map_err(raw_sqlite_error)?;
            let mut rows = statement.query([limit]).map_err(raw_sqlite_error)?;
            let mut jobs = Vec::new();
            while let Some(row) = rows.next().map_err(raw_sqlite_error)? {
                jobs.push(decode_job_row(row)?);
            }
            Ok(jobs)
        })
    }

    /// Live rows for this realm, off the partial index.
    ///
    /// `census_live = 1` is written verbatim so SQLite may use
    /// `idx_detached_jobs_census_live`, which indexes only live rows. The
    /// index therefore stays proportional to outstanding work rather than to
    /// history, which is the property that keeps this query from degrading the
    /// way the old whole-table window did.
    async fn list_census_candidates(
        &self,
        realm_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<StoredJob>, DetachedJobError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        self.with_connection(|conn| {
            let bounded = i64::try_from(limit).unwrap_or(i64::MAX);
            let mut jobs = Vec::new();
            match realm_id {
                Some(realm_id) => {
                    let mut statement = conn
                        .prepare(
                            "SELECT job_id, realm_id, submission_key, revision,
                                    has_pending_outbox, census_live, job_json
                               FROM detached_jobs
                              WHERE census_live = 1 AND realm_id = ?1
                              ORDER BY job_id
                              LIMIT ?2",
                        )
                        .map_err(raw_sqlite_error)?;
                    let mut rows = statement
                        .query(params![realm_id, bounded])
                        .map_err(raw_sqlite_error)?;
                    while let Some(row) = rows.next().map_err(raw_sqlite_error)? {
                        jobs.push(decode_job_row(row)?);
                    }
                }
                None => {
                    let mut statement = conn
                        .prepare(
                            "SELECT job_id, realm_id, submission_key, revision,
                                    has_pending_outbox, census_live, job_json
                               FROM detached_jobs
                              WHERE census_live = 1
                              ORDER BY job_id
                              LIMIT ?1",
                        )
                        .map_err(raw_sqlite_error)?;
                    let mut rows = statement.query([bounded]).map_err(raw_sqlite_error)?;
                    while let Some(row) = rows.next().map_err(raw_sqlite_error)? {
                        jobs.push(decode_job_row(row)?);
                    }
                }
            }
            Ok(jobs)
        })
    }

    /// One indexed aggregate over `idx_detached_jobs_pending_outbox`.
    ///
    /// `has_pending_outbox` leads the index, so the `= 1` predicate is a range
    /// scan over pending rows only and `realm_id` filters from the row. No
    /// document is decoded, no window is applied, and no lifecycle phase is
    /// consulted - a terminal job holding an unapplied terminal delivery is
    /// precisely what this must count.
    async fn count_pending_outbox_jobs(
        &self,
        realm_id: Option<&str>,
    ) -> Result<u64, DetachedJobError> {
        self.with_connection(|conn| {
            let count: i64 = match realm_id {
                Some(realm_id) => conn.query_row(
                    "SELECT COUNT(*) FROM detached_jobs
                      WHERE has_pending_outbox = 1 AND realm_id = ?1",
                    [realm_id],
                    |row| row.get(0),
                ),
                None => conn.query_row(
                    "SELECT COUNT(*) FROM detached_jobs WHERE has_pending_outbox = 1",
                    [],
                    |row| row.get(0),
                ),
            }
            .map_err(raw_sqlite_error)?;
            u64::try_from(count).map_err(|_| {
                DetachedJobError::Store(format!(
                    "pending outbox count {count} is not a valid population"
                ))
            })
        })
    }

    fn is_persistent(&self) -> bool {
        true
    }
}

fn select_by_id(conn: &Connection, job_id: &JobId) -> Result<Option<StoredJob>, DetachedJobError> {
    conn.query_row(
        "SELECT job_id, realm_id, submission_key, revision, has_pending_outbox, census_live,
                job_json
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
        "SELECT job_id, realm_id, submission_key, revision, has_pending_outbox, census_live,
                job_json
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

fn select_predicate_delivery_receipt(
    conn: &Connection,
    job_id: &JobId,
    idempotency_key: &str,
) -> Result<Option<PredicateDeliveryReceipt>, DetachedJobError> {
    let encoded = conn
        .query_row(
            "SELECT job_id, delivery_idempotency_key, occurrence_id, runnable,
                    committed_revision, receipt_json
               FROM detached_job_predicate_deliveries
              WHERE job_id = ?1 AND delivery_idempotency_key = ?2",
            params![job_id.as_str(), idempotency_key],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Vec<u8>>(4)?,
                    row.get::<_, meerkat_sqlite::JsonColumnBytes>(5)?
                        .into_bytes(),
                ))
            },
        )
        .optional()
        .map_err(raw_sqlite_error)?;
    let Some((stored_job_id, stored_key, occurrence_id, runnable, revision, encoded)) = encoded
    else {
        return Ok(None);
    };
    let envelope: PredicateDeliveryReceiptEnvelope =
        serde_json::from_slice(&encoded).map_err(|error| {
            DetachedJobError::Store(format!(
                "stored predicate delivery receipt JSON is invalid: {error}"
            ))
        })?;
    if envelope.format_version != PREDICATE_DELIVERY_RECEIPT_FORMAT_VERSION {
        return Err(DetachedJobError::Store(format!(
            "stored predicate delivery receipt format version {} is unsupported; this binary supports {}",
            envelope.format_version, PREDICATE_DELIVERY_RECEIPT_FORMAT_VERSION
        )));
    }
    let receipt = envelope.receipt;
    if stored_job_id != job_id.as_str()
        || &receipt.job_id != job_id
        || stored_key != idempotency_key
        || occurrence_id != receipt.identity.occurrence_id()
        || runnable != receipt.identity.runnable()
        || revision_from_bytes(&revision)? != receipt.committed_revision
        || receipt.identity.idempotency_key().as_str() != idempotency_key
    {
        return Err(DetachedJobError::Store(format!(
            "predicate delivery row columns disagree with encoded receipt for job {job_id}"
        )));
    }
    Ok(Some(receipt))
}

type EncodedJobRow = (String, String, String, Vec<u8>, bool, bool, Vec<u8>);

fn decode_job_row_sql(row: &rusqlite::Row<'_>) -> Result<EncodedJobRow, rusqlite::Error> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get::<_, meerkat_sqlite::JsonColumnBytes>(6)?
            .into_bytes(),
    ))
}

fn decode_job_row(row: &rusqlite::Row<'_>) -> Result<StoredJob, DetachedJobError> {
    decode_checked_row(decode_job_row_sql(row).map_err(raw_sqlite_error)?)
}

fn decode_checked_row(
    (job_id, realm_id, submission_key, encoded_revision, pending, census_live, encoded): EncodedJobRow,
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
        // Fail closed on a stale census projection. A row whose column says
        // "settled" while its document says otherwise is invisible to the
        // health window, which is the exact silence this projection exists to
        // end - so it must surface as an error on any read that touches the
        // row rather than as a quietly missing census entry.
        || crate::store::job_is_census_live(&job) != census_live
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

fn encode_predicate_delivery_receipt(
    receipt: &PredicateDeliveryReceipt,
) -> Result<Vec<u8>, DetachedJobError> {
    serde_json::to_vec(&PredicateDeliveryReceiptEnvelope {
        format_version: PREDICATE_DELIVERY_RECEIPT_FORMAT_VERSION,
        receipt: receipt.clone(),
    })
    .map_err(|error| {
        DetachedJobError::Store(format!("cannot encode predicate delivery receipt: {error}"))
    })
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
            subscriptions: job.subscriptions.clone(),
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
            subscriptions: job.subscriptions,
            outbox: job
                .outbox
                .into_iter()
                .map(JobOutboxEntry::try_from)
                .collect::<Result<Vec<_>, _>>()?,
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
            runner_specification_ref: spec.runner_specification_ref.clone(),
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
            runner_specification_ref: spec.runner_specification_ref,
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
            delivery_sequence: state.delivery_sequence,
            notification_ids: state.notification_ids.clone(),
            notification_idempotency_keys: state.notification_idempotency_keys.clone(),
            notification_id_by_key: state.notification_id_by_key.clone(),
            notification_delivery_ids: state.notification_delivery_ids.clone(),
            notification_sequences: state.notification_sequences.clone(),
            notification_applied: state.notification_applied.clone(),
            terminal_kind: state.terminal_kind.map(PersistedTerminalKind::from),
            terminal_delivery_sequence: state.terminal_delivery_sequence,
            terminal_delivery_applied: state.terminal_delivery_applied,
        }
    }
}

impl From<PersistedMachineState> for DetachedJobMachineState {
    fn from(mut state: PersistedMachineState) -> Self {
        if state.delivery_sequence == 0 && state.terminal_delivery_sequence > 0 {
            state.delivery_sequence = state.terminal_delivery_sequence;
        }
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
            delivery_sequence: state.delivery_sequence,
            notification_ids: state.notification_ids,
            notification_idempotency_keys: state.notification_idempotency_keys,
            notification_id_by_key: state.notification_id_by_key,
            notification_delivery_ids: state.notification_delivery_ids,
            notification_sequences: state.notification_sequences,
            notification_applied: state.notification_applied,
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
            delivery_id: Some(entry.delivery_id.clone()),
            delivery_sequence: entry.delivery_sequence,
            payload: Some(entry.payload.clone()),
            terminal_kind: None,
            terminal_result: None,
            targets: entry.targets.clone(),
            applied: entry.applied,
        }
    }
}

impl TryFrom<PersistedJobOutboxEntry> for JobOutboxEntry {
    type Error = DetachedJobError;

    fn try_from(entry: PersistedJobOutboxEntry) -> Result<Self, Self::Error> {
        let payload = match (entry.payload, entry.terminal_result) {
            (Some(payload), None) => payload,
            (None, Some(result)) => {
                if entry
                    .terminal_kind
                    .map(DetachedJobTerminalKind::from)
                    .is_some_and(|kind| kind != result.kind())
                {
                    return Err(DetachedJobError::Store(
                        "legacy terminal outbox kind disagrees with result".into(),
                    ));
                }
                JobOutboxPayload::Terminal(result)
            }
            (Some(_), Some(_)) => {
                return Err(DetachedJobError::Store(
                    "stored outbox entry contains both current and legacy payloads".into(),
                ));
            }
            (None, None) => {
                return Err(DetachedJobError::Store(
                    "stored outbox entry has no payload".into(),
                ));
            }
        };
        let delivery_id = entry.delivery_id.unwrap_or_else(|| match &payload {
            JobOutboxPayload::Terminal(_) => "terminal".to_string(),
            JobOutboxPayload::Notification(notification) => {
                notification.notification_id().as_str().to_string()
            }
        });
        Ok(Self {
            job_id: entry.job_id,
            delivery_id,
            delivery_sequence: entry.delivery_sequence,
            payload,
            targets: entry.targets,
            applied: entry.applied,
        })
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::{JOBS_DOMAIN, PersistedPhase};
    use crate::machines::detached_job::DetachedJobPhase;

    #[test]
    fn revision_encoding_round_trips_the_full_u64_domain() {
        for revision in [1, i64::MAX as u64, i64::MAX as u64 + 1, u64::MAX] {
            assert_eq!(
                super::revision_from_bytes(&super::revision_bytes(revision)).expect("decode"),
                revision
            );
        }
    }

    /// Build a v3 file: the released shape a deployed store is actually in.
    ///
    /// Hand-rolled rather than produced by opening the store, because opening
    /// it with this binary would migrate it to v4 and there would be nothing
    /// left to test.
    fn released_v3_database(rows: &[(&str, &str, PersistedPhase)]) -> rusqlite::Connection {
        let mut conn = rusqlite::Connection::open_in_memory().expect("open");
        let tx = conn.transaction().expect("tx");
        super::build_released_jobs_v3_schema(&tx).expect("v3 catalog");
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS main.meerkat_schema (
                 domain TEXT PRIMARY KEY,
                 version INTEGER NOT NULL
             )",
        )
        .expect("ledger table");
        tx.execute(
            "INSERT INTO meerkat_schema (domain, version) VALUES (?1, 3)",
            [JOBS_DOMAIN.name],
        )
        .expect("stamp v3");
        for (job_id, realm_id, phase) in rows {
            let document = serde_json::json!({
                "format_version": 1,
                "job": { "machine_state": { "lifecycle_phase": phase } },
            });
            tx.execute(
                "INSERT INTO detached_jobs
                    (job_id, realm_id, submission_key, revision, has_pending_outbox, job_json)
                 VALUES (?1, ?2, ?1, ?3, 0, ?4)",
                rusqlite::params![
                    job_id,
                    realm_id,
                    super::revision_bytes(1).as_slice(),
                    serde_json::to_vec(&document).expect("encode"),
                ],
            )
            .expect("insert v3 row");
        }
        tx.commit().expect("commit");
        conn
    }

    fn census_live_column(conn: &rusqlite::Connection, job_id: &str) -> i64 {
        conn.query_row(
            "SELECT census_live FROM detached_jobs WHERE job_id = ?1",
            [job_id],
            |row| row.get(0),
        )
        .expect("census_live")
    }

    /// The migration classifies rows it already had, and the DENOMINATOR is
    /// part of the evidence.
    ///
    /// A backfill that scanned zero rows would leave every deployed store as
    /// blind as before while reporting a clean migration, so the assertions
    /// below pin how many rows existed, how many were marked settled, and how
    /// many stayed live. Settled rows are never rewritten again - if this pass
    /// does not classify them, nothing ever will.
    #[test]
    fn migrating_a_released_v3_store_classifies_every_existing_row() {
        let mut conn = released_v3_database(&[
            ("job_a_succeeded", "realm-a", PersistedPhase::Succeeded),
            ("job_b_failed", "realm-a", PersistedPhase::Failed),
            ("job_c_cancelled", "realm-a", PersistedPhase::Cancelled),
            ("job_d_worker_lost", "realm-a", PersistedPhase::WorkerLost),
            ("job_e_running", "realm-a", PersistedPhase::Running),
            (
                "job_f_needs_attention",
                "realm-a",
                PersistedPhase::NeedsAttention,
            ),
            ("job_g_queued", "realm-b", PersistedPhase::Queued),
        ]);
        let scanned: i64 = conn
            .query_row("SELECT COUNT(*) FROM detached_jobs", [], |row| row.get(0))
            .expect("row denominator");
        assert_eq!(scanned, 7, "denominator: the rows the backfill must read");

        let report =
            meerkat_sqlite::apply_domain_migrations(&mut conn, &JOBS_DOMAIN).expect("migrate v3");
        assert_eq!(report.from_version, 3);
        assert_eq!(report.to_version, 4);

        let settled: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM detached_jobs WHERE census_live = 0",
                [],
                |row| row.get(0),
            )
            .expect("settled count");
        let live: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM detached_jobs WHERE census_live = 1",
                [],
                |row| row.get(0),
            )
            .expect("live count");
        assert_eq!(settled, 4, "the four settled phases must be marked");
        assert_eq!(live, 3, "and only those four");

        // Named, not just counted: NeedsAttention is the row a machine
        // terminality check would have wrongly hidden.
        assert_eq!(census_live_column(&conn, "job_a_succeeded"), 0);
        assert_eq!(census_live_column(&conn, "job_d_worker_lost"), 0);
        assert_eq!(census_live_column(&conn, "job_e_running"), 1);
        assert_eq!(census_live_column(&conn, "job_f_needs_attention"), 1);
        assert_eq!(census_live_column(&conn, "job_g_queued"), 1);
    }

    /// A v3 file whose owned catalog was tampered with is refused.
    ///
    /// The predecessor verifier is only worth registering if it can say no; a
    /// verifier that passes everything is the generation theater this repo
    /// keeps finding.
    #[test]
    fn a_tampered_v3_catalog_is_refused_rather_than_migrated() {
        let mut conn = released_v3_database(&[]);
        conn.execute_batch("DROP INDEX idx_detached_jobs_pending_outbox")
            .expect("tamper");
        let error = meerkat_sqlite::apply_domain_migrations(&mut conn, &JOBS_DOMAIN)
            .expect_err("a v3 catalog missing an owned index is not a v3 catalog");
        assert!(
            format!("{error}").contains("jobs"),
            "the refusal must name the domain: {error}"
        );
    }

    /// The partial index is actually used by the census query.
    ///
    /// An index the planner ignores is decoration: the whole point of
    /// `WHERE census_live = 1` is that the scan is proportional to outstanding
    /// work rather than to retained history.
    #[test]
    fn the_census_query_uses_the_live_partial_index() {
        let mut conn = released_v3_database(&[]);
        meerkat_sqlite::apply_domain_migrations(&mut conn, &JOBS_DOMAIN).expect("migrate");
        let plan: String = conn
            .query_row(
                "EXPLAIN QUERY PLAN
                 SELECT job_id, realm_id, submission_key, revision,
                        has_pending_outbox, census_live, job_json
                   FROM detached_jobs
                  WHERE census_live = 1 AND realm_id = ?1
                  ORDER BY job_id
                  LIMIT ?2",
                rusqlite::params!["realm-a", 10_i64],
                |row| row.get(3),
            )
            .expect("query plan");
        assert!(
            plan.contains("idx_detached_jobs_census_live"),
            "census query must ride the live partial index, got: {plan}"
        );
    }

    /// The column projection and the classification cannot disagree.
    ///
    /// Both directions are pinned so a future phase cannot be added to one
    /// side only: the `match` here is exhaustive, so a new variant fails to
    /// compile until someone classifies it.
    #[test]
    fn every_phase_has_one_census_classification() {
        for phase in [
            DetachedJobPhase::Unsubmitted,
            DetachedJobPhase::Queued,
            DetachedJobPhase::Claimed,
            DetachedJobPhase::Running,
            DetachedJobPhase::WaitingExternal,
            DetachedJobPhase::LossObserved,
            DetachedJobPhase::RetryScheduled,
            DetachedJobPhase::Succeeded,
            DetachedJobPhase::Failed,
            DetachedJobPhase::Cancelled,
            DetachedJobPhase::WorkerLost,
            DetachedJobPhase::NeedsAttention,
        ] {
            let expected = match phase {
                DetachedJobPhase::Succeeded
                | DetachedJobPhase::Failed
                | DetachedJobPhase::Cancelled
                | DetachedJobPhase::WorkerLost => false,
                DetachedJobPhase::Unsubmitted
                | DetachedJobPhase::Queued
                | DetachedJobPhase::Claimed
                | DetachedJobPhase::Running
                | DetachedJobPhase::WaitingExternal
                | DetachedJobPhase::LossObserved
                | DetachedJobPhase::RetryScheduled
                | DetachedJobPhase::NeedsAttention => true,
            };
            assert_eq!(
                crate::store::phase_is_census_live(phase),
                expected,
                "{phase:?} is classified inconsistently"
            );
            let document = serde_json::json!({
                "format_version": 1,
                "job": { "machine_state": { "lifecycle_phase": PersistedPhase::from(phase) } },
            });
            assert_eq!(
                super::probe_settled_phase(&serde_json::to_vec(&document).expect("encode")),
                !expected,
                "the migration probe disagrees with the classification for {phase:?}"
            );
        }
    }

    /// A document the probe cannot read leaves the row LIVE.
    ///
    /// The safe direction, stated as a test: an unreadable row costs window
    /// space, while guessing "settled" would hide a job that may be wedged.
    #[test]
    fn an_undecodable_document_is_left_live() {
        assert!(!super::probe_settled_phase(b"not json at all"));
        assert!(!super::probe_settled_phase(
            br#"{"format_version":1,"job":{}}"#
        ));
        assert!(!super::probe_settled_phase(
            br#"{"format_version":1,"job":{"machine_state":{"lifecycle_phase":"invented"}}}"#
        ));
    }
}
