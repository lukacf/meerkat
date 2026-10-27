use crate::*;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::de::DeserializeOwned;
use std::fs::{File, OpenOptions};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Durable local backend. Opening independent handles to the same database is
/// supported. Network filesystems must satisfy SQLite's locking/WAL contract;
/// copying the database does not extend its executor fence to another copy.
#[derive(Clone)]
pub struct SqliteStore {
    connection: Arc<Mutex<OwnedConnection>>,
    store_id: String,
    record_publication: Option<Arc<dyn RecordChangePublication>>,
}

// Entered blocking work retains this same Arc, including physical custody,
// after its awaiting caller or last public store handle has been dropped.
struct OwnedConnection {
    connection: Connection,
    _physical_owner: File,
}

// Darwin's whole-file flock conflicts with SQLite's own POSIX locks. Keep
// custody on the same inode, but reserve one byte outside SQLite's main-file
// lock range (0x40000000 through 0x400001ff). OFD locks remain bound to this
// open description across unrelated SQLite closes and until the last retained
// File is dropped; ordinary process-associated F_SETLK would not suffice.
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
fn lock_physical_owner(file: &File, exclusive: bool) -> StoreResult<()> {
    use std::os::fd::AsRawFd;
    let lock = libc::flock {
        l_start: 0,
        l_len: 1,
        l_pid: 0,
        l_type: if exclusive { libc::F_WRLCK } else { libc::F_RDLCK },
        l_whence: libc::SEEK_SET as i16,
    };
    // SAFETY: file retains a valid descriptor for this synchronous call. The
    // initialized flock lives through the call and F_OFD_SETLK reads it without
    // retaining the pointer. Byte zero, length one is a valid nonnegative range;
    // the real libc constant selects open-description custody, not process locks.
    let result = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_OFD_SETLK, &lock) };
    if result == -1 { Err(StoreError::OwnerUnavailable) } else { Ok(()) }
}

#[cfg(not(target_os = "macos"))]
fn lock_physical_owner(file: &File, exclusive: bool) -> StoreResult<()> {
    if exclusive {
        file.try_lock().map_err(|_| StoreError::OwnerUnavailable)
    } else {
        file.try_lock_shared().map_err(|_| StoreError::OwnerUnavailable)
    }
}

impl SqliteStore {
    pub fn open(path: impl AsRef<Path>) -> StoreResult<Self> {
        Self::open_mode(path.as_ref(), true, None)
    }

    /// Open an initialized store without creating a missing database or schema.
    /// This is not a read-only connection; ordinary WAL coordination still
    /// applies. It permits observation tools to refuse a mistaken database path.
    pub fn open_existing(path: impl AsRef<Path>) -> StoreResult<Self> {
        Self::open_mode(path.as_ref(), false, None)
    }

    /// Open this installed application's existing database with exclusive
    /// physical custody and its actual owner's publication. The guard spans all
    /// clones and entered blocking work. A competing supported handle refuses
    /// to open while this owner exists. Raw SQL is outside this host contract.
    pub fn open_existing_published(
        path: impl AsRef<Path>,
        publication: Arc<dyn RecordChangePublication>,
    ) -> StoreResult<Self> {
        Self::open_mode(path.as_ref(), false, Some(publication))
    }

    /// Create a fresh store with the same exclusive publication custody used
    /// by ordinary installed restore. The caller must already own and validate
    /// the absent native store pair; this method is not a replacement policy.
    pub fn open_published(
        path: impl AsRef<Path>,
        publication: Arc<dyn RecordChangePublication>,
    ) -> StoreResult<Self> {
        Self::open_mode(path.as_ref(), true, Some(publication))
    }

    pub fn record_publication_is(&self, publication: &Arc<dyn RecordChangePublication>) -> bool {
        self.record_publication.as_ref().is_some_and(|bound| Arc::ptr_eq(bound, publication))
    }

    fn open_mode(path: &Path, create: bool, publication: Option<Arc<dyn RecordChangePublication>>) -> StoreResult<Self> {
        if path.as_os_str().is_empty() || path == Path::new(":memory:") {
            return Err(StoreError::Invalid(
                "SqliteStore requires a persistent filesystem path".into(),
            ));
        }
        let physical_owner = OpenOptions::new().read(true).write(true).create(create)
            .open(path).map_err(|_| StoreError::OwnerUnavailable)?;
        lock_physical_owner(&physical_owner, publication.is_some())?;
        check_database_identity(path, &physical_owner)?;
        let mut flags = rusqlite::OpenFlags::default();
        if !create {
            flags.remove(rusqlite::OpenFlags::SQLITE_OPEN_CREATE);
        }
        let mut connection = Connection::open_with_flags(path, flags)?;
        check_database_identity(path, &physical_owner)?;
        connection.busy_timeout(Duration::from_secs(10))?;
        if !create {
            let version: i64 = connection.pragma_query_value(None, "user_version", |r| r.get(0))?;
            if version != 1 {
                return Err(StoreError::UnsupportedFormat(version));
            }
        }
        connection.pragma_update(None, "foreign_keys", "ON")?;
        let journal: String = connection.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))?;
        if journal != "wal" {
            return Err(StoreError::Invalid("SQLite WAL mode is required".into()));
        }
        connection.pragma_update(None, "synchronous", "FULL")?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let version: i64 = tx.pragma_query_value(None, "user_version", |r| r.get(0))?;
        match version {
            0 if create => {
                tx.execute_batch(
                    "CREATE TABLE toolkit_metadata (singleton INTEGER PRIMARY KEY CHECK(singleton=1), store_id TEXT NOT NULL);
                     CREATE TABLE toolkit_records (
                        realm TEXT NOT NULL, owner TEXT NOT NULL, id TEXT NOT NULL,
                        revision INTEGER NOT NULL CHECK(revision>0), digest TEXT NOT NULL,
                        data TEXT NOT NULL, updated_at_ms INTEGER NOT NULL,
                        claim_fence INTEGER NOT NULL DEFAULT 0,
                        claim_worker TEXT, claim_until_ms INTEGER,
                        PRIMARY KEY(realm,owner,id));
                     CREATE TABLE toolkit_admissions (
                        realm TEXT NOT NULL, owner TEXT NOT NULL, admission_key TEXT NOT NULL,
                        request_digest TEXT NOT NULL, record_id TEXT NOT NULL,
                        PRIMARY KEY(realm,owner,admission_key),
                        FOREIGN KEY(realm,owner,record_id) REFERENCES toolkit_records(realm,owner,id));
                     CREATE TABLE toolkit_events (
                        realm TEXT NOT NULL, owner TEXT NOT NULL, record_id TEXT NOT NULL,
                        sequence INTEGER NOT NULL, record_revision INTEGER NOT NULL,
                        kind TEXT NOT NULL, data TEXT NOT NULL, recorded_at_ms INTEGER NOT NULL,
                        PRIMARY KEY(realm,owner,record_id,sequence),
                        FOREIGN KEY(realm,owner,record_id) REFERENCES toolkit_records(realm,owner,id));
                     CREATE TABLE toolkit_effects (
                        realm TEXT NOT NULL, owner TEXT NOT NULL, record_id TEXT NOT NULL,
                        id TEXT NOT NULL, idempotency_key TEXT NOT NULL,
                        data TEXT NOT NULL, fence INTEGER NOT NULL DEFAULT 0,
                        PRIMARY KEY(realm,owner,record_id,id),
                        UNIQUE(realm,owner,idempotency_key),
                        FOREIGN KEY(realm,owner,record_id) REFERENCES toolkit_records(realm,owner,id));",
                )?;
                tx.execute(
                    "INSERT INTO toolkit_metadata(singleton,store_id) VALUES(1,?1)",
                    [uuid::Uuid::new_v4().to_string()],
                )?;
                tx.pragma_update(None, "user_version", 1)?;
            }
            1 => {}
            value => return Err(StoreError::UnsupportedFormat(value)),
        }
        let store_id = tx.query_row(
            "SELECT store_id FROM toolkit_metadata WHERE singleton=1",
            [],
            |r| r.get(0),
        )?;
        tx.commit()?;
        Ok(Self {
            connection: Arc::new(Mutex::new(OwnedConnection { connection, _physical_owner: physical_owner })),
            store_id,
            record_publication: publication,
        })
    }

    /// Blocking SQLite work never holds an async runtime thread. Cancellation of
    /// an awaiting caller does not roll back a transaction that already committed;
    /// callers reconcile by admission key, revision and effect ID after loss.
    async fn with<T: Send + 'static>(
        &self,
        action: impl FnOnce(&mut Connection, &str) -> StoreResult<T> + Send + 'static,
    ) -> StoreResult<T> {
        let connection = self.connection.clone();
        let store_id = self.store_id.clone();
        tokio::task::spawn_blocking(move || {
            let mut connection = connection.lock().map_err(|_| StoreError::Synchronization)?;
            action(&mut connection.connection, &store_id)
        })
        .await
        .map_err(|_| StoreError::Worker)?
    }
}

fn check_database_identity(path: &Path, file: &File) -> StoreResult<()> {
    let named = std::fs::metadata(path).map_err(|_| StoreError::OwnerUnavailable)?;
    let opened = file.metadata().map_err(|_| StoreError::OwnerUnavailable)?;
    if !named.is_file() || !opened.is_file() {
        return Err(StoreError::OwnerUnavailable);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if named.dev() != opened.dev() || named.ino() != opened.ino() {
            return Err(StoreError::OwnerUnavailable);
        }
    }
    Ok(())
}

fn now(connection: &Connection) -> StoreResult<i64> {
    Ok(connection.query_row(
        "SELECT CAST(unixepoch('subsec') * 1000 AS INTEGER)",
        [],
        |row| row.get(0),
    )?)
}

fn encode(value: &impl Serialize) -> StoreResult<String> {
    Ok(serde_json::to_string(value)?)
}

fn decode<T: DeserializeOwned>(value: String) -> StoreResult<T> {
    Ok(serde_json::from_str(&value)?)
}

fn integer(value: u64) -> StoreResult<i64> {
    i64::try_from(value).map_err(|_| StoreError::Invalid("integer exceeds storage range".into()))
}

fn read_record(connection: &Connection, key: &StoreKey) -> StoreResult<Option<StoredRecord>> {
    let row = connection
        .query_row(
            "SELECT revision,digest,data,updated_at_ms FROM toolkit_records WHERE realm=?1 AND owner=?2 AND id=?3",
            params![key.realm,key.owner,key.id],
            |r| Ok((r.get::<_, u64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, i64>(3)?)),
        )
        .optional()?;
    row.map(|(revision, content_digest, data, updated_at_ms)| {
        Ok(StoredRecord {
            key: key.clone(),
            revision,
            content_digest,
            data: decode(data)?,
            updated_at_ms,
        })
    })
    .transpose()
}

fn append_event(
    tx: &Transaction<'_>,
    key: &StoreKey,
    revision: u64,
    event: &EventWrite,
    time: i64,
) -> StoreResult<()> {
    tx.execute(
        "INSERT INTO toolkit_events(realm,owner,record_id,sequence,record_revision,kind,data,recorded_at_ms)
         SELECT ?1,?2,?3,COALESCE(MAX(sequence),0)+1,?4,?5,?6,?7 FROM toolkit_events
         WHERE realm=?1 AND owner=?2 AND record_id=?3",
        params![key.realm,key.owner,key.id,integer(revision)?,event.kind,encode(&event.data)?,time],
    )?;
    Ok(())
}

fn append_batch(
    tx: &Transaction<'_>,
    record: &StoredRecord,
    events: &[EventWrite],
    effects: &[EffectIntent],
) -> StoreResult<()> {
    for event in events {
        append_event(
            tx,
            &record.key,
            record.revision,
            event,
            record.updated_at_ms,
        )?;
    }
    for intent in effects {
        let effect = StoredEffect {
            key: EffectKey {
                record: record.key.clone(),
                id: intent.id.clone(),
            },
            record_revision: record.revision,
            intent: intent.clone(),
            request_digest: intent.digest()?,
            status: EffectStatus::Pending,
            attempt: 0,
            lease: None,
            outcome: None,
            updated_at_ms: record.updated_at_ms,
        };
        let result = tx.execute(
            "INSERT INTO toolkit_effects(realm,owner,record_id,id,idempotency_key,data) VALUES(?1,?2,?3,?4,?5,?6)",
            params![record.key.realm,record.key.owner,record.key.id,intent.id,intent.idempotency_key,encode(&effect)?],
        );
        if let Err(error) = result {
            if error.sqlite_error_code() == Some(rusqlite::ErrorCode::ConstraintViolation) {
                return Err(StoreError::EffectConflict);
            }
            return Err(error.into());
        }
    }
    Ok(())
}

fn check_record_lease(
    connection: &Connection,
    store_id: &str,
    key: &StoreKey,
    lease: &RecordLease,
    time: i64,
) -> StoreResult<()> {
    if lease.store_id != store_id || &lease.key != key {
        return Err(StoreError::Fenced);
    }
    let valid: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM toolkit_records WHERE realm=?1 AND owner=?2 AND id=?3
         AND claim_worker=?4 AND claim_fence=?5 AND claim_until_ms=?6 AND claim_until_ms>?7)",
        params![
            key.realm,
            key.owner,
            key.id,
            lease.worker,
            integer(lease.fence)?,
            lease.expires_at_ms,
            time
        ],
        |r| r.get(0),
    )?;
    if !valid {
        return Err(StoreError::Fenced);
    }
    Ok(())
}

fn read_effect(connection: &Connection, key: &EffectKey) -> StoreResult<Option<StoredEffect>> {
    let row = connection
        .query_row(
            "SELECT data FROM toolkit_effects WHERE realm=?1 AND owner=?2 AND record_id=?3 AND id=?4",
            params![key.record.realm,key.record.owner,key.record.id,key.id],
            |r| r.get::<_,String>(0),
        )
        .optional()?;
    row.map(decode).transpose()
}

fn save_effect(tx: &Transaction<'_>, effect: &StoredEffect) -> StoreResult<()> {
    tx.execute(
        "UPDATE toolkit_effects SET data=?5 WHERE realm=?1 AND owner=?2 AND record_id=?3 AND id=?4",
        params![
            effect.key.record.realm,
            effect.key.record.owner,
            effect.key.record.id,
            effect.key.id,
            encode(effect)?
        ],
    )?;
    Ok(())
}

fn effect_event(
    tx: &Transaction<'_>,
    effect: &StoredEffect,
    kind: &str,
    evidence: Value,
) -> StoreResult<()> {
    let record = read_record(tx, &effect.key.record)?.ok_or(StoreError::NotFound)?;
    append_event(
        tx,
        &effect.key.record,
        record.revision,
        &EventWrite {
            kind: format!("toolkit.store.effect.{kind}"),
            data: RecordData {
                schema: "toolkit.store.effect-event".into(),
                version: 1,
                body: serde_json::json!({
                    "effect_id": effect.key.id,
                    "attempt": effect.attempt,
                    "request_digest": effect.request_digest,
                    "status": effect.status,
                    "evidence": evidence,
                }),
            },
        },
        effect.updated_at_ms,
    )
}

fn allocate_effect_lease(
    tx: &Transaction<'_>,
    store_id: &str,
    effect: &mut StoredEffect,
    worker: String,
    expiry: i64,
    kind: EffectClaimKind,
) -> StoreResult<()> {
    let fence: u64 = tx.query_row(
        "UPDATE toolkit_effects SET fence=fence+1 WHERE realm=?1 AND owner=?2 AND record_id=?3 AND id=?4 RETURNING fence",
        params![effect.key.record.realm,effect.key.record.owner,effect.key.record.id,effect.key.id],
        |r| r.get(0),
    )?;
    effect.lease = Some(EffectLease {
        store_id: store_id.into(),
        key: effect.key.clone(),
        worker,
        fence,
        expires_at_ms: expiry,
        kind,
    });
    Ok(())
}

fn validate_effect_lease(
    effect: &StoredEffect,
    lease: &EffectLease,
    store_id: &str,
    kind: EffectClaimKind,
    time: i64,
) -> StoreResult<()> {
    if lease.store_id != store_id
        || lease.key != effect.key
        || lease.kind != kind
        || effect.lease.as_ref() != Some(lease)
        || lease.expires_at_ms <= time
    {
        return Err(StoreError::Fenced);
    }
    Ok(())
}

fn validate_outcome(outcome: &EffectOutcome) -> StoreResult<()> {
    match outcome {
        EffectOutcome::Succeeded { receipt } => receipt.validate(),
        EffectOutcome::Failed { evidence } | EffectOutcome::Uncertain { evidence } => {
            evidence.validate()
        }
    }
}

fn validate_admission(request: &AdmissionRequest) -> StoreResult<()> {
    request.key.validate()?;
    identifier(&request.admission_key)?;
    request.data.validate()?;
    validate_effects_and_events(&request.effects, &request.events)
}

fn existing_admission(
    tx: &Transaction<'_>,
    request: &AdmissionRequest,
) -> StoreResult<Option<StoredRecord>> {
    let request_digest = digest(request)?;
    let prior = tx
        .query_row(
            "SELECT request_digest,record_id FROM toolkit_admissions WHERE realm=?1 AND owner=?2 AND admission_key=?3",
            params![request.key.realm,request.key.owner,request.admission_key],
            |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)),
        )
        .optional()?;
    if let Some((prior_digest, id)) = prior {
        if prior_digest != request_digest || id != request.key.id {
            return Err(StoreError::AdmissionConflict);
        }
        return read_record(tx, &request.key)?
            .ok_or(StoreError::NotFound)
            .map(Some);
    }
    Ok(None)
}

fn admit_in_transaction(
    tx: &Transaction<'_>,
    request: AdmissionRequest,
) -> StoreResult<AdmissionOutcome> {
    if let Some(record) = existing_admission(tx, &request)? {
        return Ok(AdmissionOutcome::Existing(record));
    }
    if read_record(tx, &request.key)?.is_some() {
        return Err(StoreError::AlreadyExists);
    }
    let request_digest = digest(&request)?;
    let record = StoredRecord {
        key: request.key,
        revision: 1,
        content_digest: request.data.digest()?,
        data: request.data,
        updated_at_ms: now(tx)?,
    };
    tx.execute(
        "INSERT INTO toolkit_records(realm,owner,id,revision,digest,data,updated_at_ms) VALUES(?1,?2,?3,?4,?5,?6,?7)",
        params![record.key.realm,record.key.owner,record.key.id,integer(record.revision)?,record.content_digest,encode(&record.data)?,record.updated_at_ms],
    )?;
    tx.execute(
        "INSERT INTO toolkit_admissions(realm,owner,admission_key,request_digest,record_id) VALUES(?1,?2,?3,?4,?5)",
        params![record.key.realm,record.key.owner,request.admission_key,request_digest,record.key.id],
    )?;
    append_batch(tx, &record, &request.events, &request.effects)?;
    Ok(AdmissionOutcome::Created(record))
}

fn validate_conditions(
    target: &StoreKey, conditions: &[RecordCondition], before_ms: i64,
) -> StoreResult<()> {
    if conditions.is_empty() || conditions.len() > 64 || before_ms <= 0 {
        return Err(StoreError::Invalid(
            "conditional admission requires 1-64 conditions and a positive deadline".into(),
        ));
    }
    let mut seen = std::collections::BTreeSet::new();
    for condition in conditions {
        condition.key.validate()?;
        integer(condition.revision)?;
        if condition.revision == 0
            || condition.key.realm != target.realm
            || &condition.key == target
            || !seen.insert(&condition.key)
            || condition.content_digest.len() != 64
            || !condition
                .content_digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(StoreError::Invalid(
                "invalid conditional owner record".into(),
            ));
        }
        if let Some(lease) = &condition.claim {
            lease.key.validate()?;
            identifier(&lease.worker)?;
            integer(lease.fence)?;
            if lease.key != condition.key {
                return Err(StoreError::Invalid(
                    "record claim does not identify the conditional owner".into(),
                ));
            }
        }
        if let Some(lease) = &condition.dispatch_claim {
            lease.key.validate()?;
            identifier(&lease.worker)?;
            integer(lease.fence)?;
            if lease.key.record != condition.key || lease.kind != EffectClaimKind::Dispatch {
                return Err(StoreError::Invalid(
                    "effect claim must identify a dispatch from the conditional owner".into(),
                ));
            }
        }
    }
    Ok(())
}

fn check_conditions(
    tx: &Transaction<'_>,
    store_id: &str,
    conditions: &[RecordCondition],
    before_ms: i64,
) -> StoreResult<()> {
    let time = now(tx)?;
    if time >= before_ms {
        return Err(StoreError::AdmissionExpired);
    }
    for condition in conditions {
        let record = read_record(tx, &condition.key)?.ok_or(StoreError::Conflict)?;
        if record.revision != condition.revision
            || record.content_digest != condition.content_digest
        {
            return Err(StoreError::Conflict);
        }
        if let Some(lease) = &condition.claim {
            check_record_lease(tx, store_id, &condition.key, lease, time)?;
        }
        if let Some(lease) = &condition.dispatch_claim {
            let effect = read_effect(tx, &lease.key)?.ok_or(StoreError::Fenced)?;
            if effect.status != EffectStatus::InFlight {
                return Err(StoreError::Fenced);
            }
            validate_effect_lease(&effect, lease, store_id, EffectClaimKind::Dispatch, time)?;
        }
    }
    Ok(())
}

// The writer transaction already pins all condition identities. Sample time
// after their final scan so decoding large records cannot reuse an earlier
// timestamp. This is the final admission decision before COMMIT, not a promise
// that disk synchronization or the caller response precedes these deadlines.
fn check_final_condition_time(
    tx: &Transaction<'_>,
    conditions: &[RecordCondition],
    before_ms: i64,
    target_claim: Option<&RecordLease>,
) -> StoreResult<()> {
    let time = now(tx)?;
    if time >= before_ms {
        return Err(StoreError::AdmissionExpired);
    }
    if target_claim.is_some_and(|lease| lease.expires_at_ms <= time)
        || conditions.iter().any(|condition| {
        condition
            .claim
            .as_ref()
            .is_some_and(|lease| lease.expires_at_ms <= time)
            || condition
                .dispatch_claim
                .as_ref()
                .is_some_and(|lease| lease.expires_at_ms <= time)
    }) {
        return Err(StoreError::Fenced);
    }
    Ok(())
}

#[async_trait]
impl ConditionalAdmissionStore for SqliteStore {
    async fn admit_conditionally(
        &self,
        request: ConditionalAdmission,
    ) -> StoreResult<AdmissionOutcome> {
        validate_admission(&request.admission)?;
        validate_conditions(&request.admission.key, &request.conditions, request.before_ms)?;
        self.with(move |connection, store_id| {
            let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if let Some(record) = existing_admission(&tx, &request.admission)? {
                // Replay proves the initial admission identity exists and
                // returns its current projection. It does not attest that this
                // API or these predicates created the original admission.
                return Ok(AdmissionOutcome::Existing(record));
            }
            check_conditions(&tx, store_id, &request.conditions, request.before_ms)?;
            let outcome = admit_in_transaction(&tx, request.admission)?;
            // Include time spent writing the event/effect batch. Transaction
            // custody already excludes competing writes to owner records.
            check_conditions(&tx, store_id, &request.conditions, request.before_ms)?;
            check_final_condition_time(&tx, &request.conditions, request.before_ms, None)?;
            tx.commit()?;
            Ok(outcome)
        })
        .await
    }

    async fn compare_and_swap_conditionally(
        &self,
        request: ConditionalTransition,
    ) -> StoreResult<StoredRecord> {
        let transition = &request.transition;
        transition.key.validate()?;
        transition.data.validate()?;
        integer(transition.expected_revision)?;
        validate_effects_and_events(&transition.effects, &transition.events)?;
        if transition.expected_revision == 0 || !transition.expected_digest.as_ref().is_some_and(|digest| {
            digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        }) {
            return Err(StoreError::Invalid("conditional transition requires exact target revision and digest".into()));
        }
        validate_conditions(&transition.key, &request.conditions, request.before_ms)?;
        let publication = self.record_publication.clone();
        self.with(move |connection, store_id| {
            let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let transition = request.transition;
            let prior = read_record(&tx, &transition.key)?.ok_or(StoreError::NotFound)?;
            if prior.revision != transition.expected_revision
                || transition.expected_digest.as_ref() != Some(&prior.content_digest)
            {
                return Err(StoreError::Conflict);
            }
            check_conditions(&tx, store_id, &request.conditions, request.before_ms)?;
            let time = now(&tx)?;
            if let Some(claim) = &transition.claim {
                check_record_lease(&tx, store_id, &transition.key, claim, time)?;
            }
            let revision = prior.revision.checked_add(1)
                .ok_or_else(|| StoreError::Invalid("revision exhausted".into()))?;
            let record = StoredRecord {
                key: transition.key, revision,
                content_digest: transition.data.digest()?, data: transition.data,
                updated_at_ms: time,
            };
            // The transaction moves inside this guard. Rollback on every
            // error and COMMIT both finish before publishing a new even stamp.
            let _publication = publication.as_ref()
                .map(|owner| owner.begin_record_change()).transpose()?;
            (move || {
                tx.execute(
                    "UPDATE toolkit_records SET revision=?4,digest=?5,data=?6,updated_at_ms=?7 WHERE realm=?1 AND owner=?2 AND id=?3",
                    params![record.key.realm,record.key.owner,record.key.id,integer(revision)?,record.content_digest,encode(&record.data)?,time],
                )?;
                append_batch(&tx, &record, &transition.events, &transition.effects)?;
                check_conditions(&tx, store_id, &request.conditions, request.before_ms)?;
                // The target claim's exact identity was checked above under
                // this same transaction. Its expiry and all owner deadlines
                // participate in the same final backend-time decision.
                check_final_condition_time(&tx, &request.conditions, request.before_ms,
                    transition.claim.as_ref())?;
                tx.commit()?;
                Ok(record)
            })()
        }).await
    }
}

#[async_trait]
impl ToolkitStore for SqliteStore {
    fn capabilities(&self) -> StoreCapabilities {
        StoreCapabilities {
            durability: Durability::Durable,
            revision_cas: true,
            unique_admission: true,
            atomic_record_events_effects: true,
            cross_process_fencing: true,
            store_clock: true,
        }
    }

    async fn now_ms(&self) -> StoreResult<i64> {
        self.with(|connection, _| now(connection)).await
    }

    async fn admit(&self, request: AdmissionRequest) -> StoreResult<AdmissionOutcome> {
        validate_admission(&request)?;
        self.with(move |connection, _| {
            let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let outcome = admit_in_transaction(&tx, request)?;
            tx.commit()?;
            Ok(outcome)
        })
        .await
    }

    async fn get(&self, key: &StoreKey) -> StoreResult<Option<StoredRecord>> {
        key.validate()?;
        let key = key.clone();
        self.with(move |connection, _| read_record(connection, &key))
            .await
    }

    async fn list_records(
        &self,
        scope: &StoreScope,
        after_id: Option<&str>,
        limit: u32,
    ) -> StoreResult<Vec<StoredRecord>> {
        scope.validate()?;
        validate_limit(limit)?;
        let scope = scope.clone();
        let after_id = after_id.unwrap_or("").to_string();
        self.with(move |connection, _| {
            let mut statement = connection.prepare(
                "SELECT id,revision,digest,data,updated_at_ms FROM toolkit_records WHERE realm=?1 AND owner=?2 AND id>?3 ORDER BY id LIMIT ?4",
            )?;
            let rows = statement.query_map(params![scope.realm,scope.owner,after_id,limit], |r| {
                Ok((r.get::<_,String>(0)?,r.get::<_,u64>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,i64>(4)?))
            })?;
            let mut records = Vec::new();
            for row in rows {
                let (id,revision,content_digest,data,updated_at_ms) = row?;
                records.push(StoredRecord {
                    key: StoreKey { realm: scope.realm.clone(), owner: scope.owner.clone(), id },
                    revision, content_digest, data: decode(data)?, updated_at_ms,
                });
            }
            Ok(records)
        }).await
    }

    async fn compare_and_swap(&self, transition: Transition) -> StoreResult<StoredRecord> {
        transition.key.validate()?;
        transition.data.validate()?;
        integer(transition.expected_revision)?;
        validate_effects_and_events(&transition.effects, &transition.events)?;
        let publication = self.record_publication.clone();
        self.with(move |connection, store_id| {
            let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let prior = read_record(&tx, &transition.key)?.ok_or(StoreError::NotFound)?;
            if prior.revision != transition.expected_revision
                || transition.expected_digest.as_ref().is_some_and(|expected| expected != &prior.content_digest)
            {
                return Err(StoreError::Conflict);
            }
            let time = now(&tx)?;
            if let Some(claim) = &transition.claim {
                check_record_lease(&tx, store_id, &transition.key, claim, time)?;
            }
            let revision = prior.revision.checked_add(1).ok_or_else(|| StoreError::Invalid("revision exhausted".into()))?;
            let record = StoredRecord {
                key: transition.key,
                revision,
                content_digest: transition.data.digest()?,
                data: transition.data,
                updated_at_ms: time,
            };
            // The inner closure owns tx: all error rollback and successful
            // commit finish before this publication guard is released. Nothing
            // awaits or exposes a new even stamp while old rows are visible.
            let _publication = publication.as_ref()
                .map(|owner| owner.begin_record_change()).transpose()?;
            (move || {
            tx.execute(
                "UPDATE toolkit_records SET revision=?4,digest=?5,data=?6,updated_at_ms=?7 WHERE realm=?1 AND owner=?2 AND id=?3",
                params![record.key.realm,record.key.owner,record.key.id,integer(revision)?,record.content_digest,encode(&record.data)?,time],
            )?;
            append_batch(&tx, &record, &transition.events, &transition.effects)?;
            tx.commit()?;
            Ok(record)
            })()
        }).await
    }

    async fn claim_record(
        &self,
        key: &StoreKey,
        expected_revision: u64,
        worker: &str,
        ttl_ms: u64,
    ) -> StoreResult<RecordLease> {
        key.validate()?;
        identifier(worker)?;
        integer(expected_revision)?;
        let ttl = validate_ttl(ttl_ms)?;
        let key = key.clone();
        let worker = worker.to_string();
        self.with(move |connection, store_id| {
            let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let record = read_record(&tx, &key)?.ok_or(StoreError::NotFound)?;
            if record.revision != expected_revision {
                return Err(StoreError::Conflict);
            }
            let time = now(&tx)?;
            let lease_until: Option<i64> = tx.query_row(
                "SELECT claim_until_ms FROM toolkit_records WHERE realm=?1 AND owner=?2 AND id=?3",
                params![key.realm,key.owner,key.id], |r| r.get(0),
            )?;
            if lease_until.is_some_and(|until| until > time) {
                return Err(StoreError::Claimed);
            }
            let expires_at_ms = time + ttl;
            let fence = tx.query_row(
                "UPDATE toolkit_records SET claim_fence=claim_fence+1,claim_worker=?4,claim_until_ms=?5 WHERE realm=?1 AND owner=?2 AND id=?3 RETURNING claim_fence",
                params![key.realm,key.owner,key.id,worker,expires_at_ms], |r| r.get(0),
            )?;
            tx.commit()?;
            Ok(RecordLease { store_id: store_id.into(), key, worker, fence, expires_at_ms })
        }).await
    }

    async fn renew_record_claim(
        &self,
        lease: &RecordLease,
        ttl_ms: u64,
    ) -> StoreResult<RecordLease> {
        lease.key.validate()?;
        let ttl = validate_ttl(ttl_ms)?;
        let lease = lease.clone();
        self.with(move |connection, store_id| {
            let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let time = now(&tx)?;
            check_record_lease(&tx, store_id, &lease.key, &lease, time)?;
            let renewed = RecordLease { expires_at_ms: time + ttl, ..lease };
            tx.execute(
                "UPDATE toolkit_records SET claim_until_ms=?4 WHERE realm=?1 AND owner=?2 AND id=?3",
                params![renewed.key.realm,renewed.key.owner,renewed.key.id,renewed.expires_at_ms],
            )?;
            tx.commit()?;
            Ok(renewed)
        }).await
    }

    async fn release_record_claim(&self, lease: &RecordLease) -> StoreResult<()> {
        lease.key.validate()?;
        let lease = lease.clone();
        self.with(move |connection, store_id| {
            let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            check_record_lease(&tx, store_id, &lease.key, &lease, now(&tx)?)?;
            tx.execute(
                "UPDATE toolkit_records SET claim_worker=NULL,claim_until_ms=NULL WHERE realm=?1 AND owner=?2 AND id=?3",
                params![lease.key.realm,lease.key.owner,lease.key.id],
            )?;
            tx.commit()?;
            Ok(())
        }).await
    }

    async fn recover_record_claim(&self, prior: &RecordLease) -> StoreResult<RecordLease> {
        prior.key.validate()?;
        identifier(&prior.worker)?;
        integer(prior.fence)?;
        let prior = prior.clone();
        self.with(move |connection, store_id| {
            if prior.store_id != store_id {
                return Err(StoreError::Fenced);
            }
            // Observe one live claim under the original executor generation.
            // The expired prior token is evidence of which claim to recover,
            // not permission to replace a newer worker or bypass mutation CAS.
            let expires_at_ms = connection
                .query_row(
                    "SELECT claim_until_ms FROM toolkit_records
                     WHERE realm=?1 AND owner=?2 AND id=?3 AND claim_worker=?4 AND claim_fence=?5
                     AND claim_until_ms>CAST(unixepoch('subsec') * 1000 AS INTEGER)",
                    params![
                        prior.key.realm,
                        prior.key.owner,
                        prior.key.id,
                        prior.worker,
                        integer(prior.fence)?
                    ],
                    |r| r.get(0),
                )
                .optional()?
                .ok_or(StoreError::Fenced)?;
            Ok(RecordLease {
                expires_at_ms,
                ..prior
            })
        })
        .await
    }

    async fn read_events(
        &self,
        key: &StoreKey,
        after_sequence: u64,
        limit: u32,
    ) -> StoreResult<Vec<StoredEvent>> {
        key.validate()?;
        validate_limit(limit)?;
        let sequence = integer(after_sequence)?;
        let key = key.clone();
        self.with(move |connection, _| {
            let mut statement = connection.prepare(
                "SELECT sequence,record_revision,kind,data,recorded_at_ms FROM toolkit_events WHERE realm=?1 AND owner=?2 AND record_id=?3 AND sequence>?4 ORDER BY sequence LIMIT ?5",
            )?;
            let rows = statement.query_map(params![key.realm,key.owner,key.id,sequence,limit], |r| {
                Ok((r.get::<_,u64>(0)?,r.get::<_,u64>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,i64>(4)?))
            })?;
            let mut events = Vec::new();
            for row in rows {
                let (sequence,record_revision,kind,data,recorded_at_ms) = row?;
                events.push(StoredEvent { sequence,key:key.clone(),record_revision,kind,data:decode(data)?,recorded_at_ms });
            }
            Ok(events)
        }).await
    }

    async fn get_effect(&self, key: &EffectKey) -> StoreResult<Option<StoredEffect>> {
        key.validate()?;
        let key = key.clone();
        self.with(move |connection, _| read_effect(connection, &key))
            .await
    }

    async fn list_effects(
        &self,
        key: &StoreKey,
        after_id: Option<&str>,
        limit: u32,
    ) -> StoreResult<Vec<StoredEffect>> {
        key.validate()?;
        validate_limit(limit)?;
        let key = key.clone();
        let after_id = after_id.unwrap_or("").to_string();
        self.with(move |connection, _| {
            let mut statement = connection.prepare(
                "SELECT data FROM toolkit_effects WHERE realm=?1 AND owner=?2 AND record_id=?3 AND id>?4 ORDER BY id LIMIT ?5",
            )?;
            let rows = statement.query_map(params![key.realm,key.owner,key.id,after_id,limit], |r| r.get::<_,String>(0))?;
            rows.map(|row| decode(row?)).collect()
        }).await
    }

    async fn claim_effect(
        &self,
        key: &EffectKey,
        expected_record_revision: u64,
        worker: &str,
        ttl_ms: u64,
    ) -> StoreResult<EffectClaimOutcome> {
        key.validate()?;
        identifier(worker)?;
        let ttl = validate_ttl(ttl_ms)?;
        let key = key.clone();
        let worker = worker.to_string();
        self.with(move |connection, store_id| {
            let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let record = read_record(&tx, &key.record)?.ok_or(StoreError::NotFound)?;
            if record.revision != expected_record_revision {
                return Err(StoreError::Conflict);
            }
            let mut effect = read_effect(&tx, &key)?.ok_or(StoreError::NotFound)?;
            let time = now(&tx)?;
            if matches!(
                effect.status,
                EffectStatus::Succeeded | EffectStatus::Failed
            ) {
                return Err(StoreError::EffectState);
            }
            if effect
                .lease
                .as_ref()
                .is_some_and(|lease| lease.expires_at_ms > time)
            {
                return Err(StoreError::Claimed);
            }
            if effect.status == EffectStatus::Uncertain
                || (effect.status == EffectStatus::InFlight
                    && effect.intent.replay_safety == ReplaySafety::ReconcileBeforeRetry)
            {
                let changed = effect.status != EffectStatus::Uncertain || effect.lease.is_some();
                effect.status = EffectStatus::Uncertain;
                effect.lease = None;
                if changed {
                    effect.updated_at_ms = time;
                    save_effect(&tx, &effect)?;
                    effect_event(&tx, &effect, "reconciliation-required", Value::Null)?;
                }
                tx.commit()?;
                return Ok(EffectClaimOutcome::NeedsReconciliation(effect));
            }
            allocate_effect_lease(
                &tx,
                store_id,
                &mut effect,
                worker,
                time + ttl,
                EffectClaimKind::Dispatch,
            )?;
            effect.attempt = effect
                .attempt
                .checked_add(1)
                .ok_or_else(|| StoreError::Invalid("effect attempts exhausted".into()))?;
            effect.status = EffectStatus::InFlight;
            effect.updated_at_ms = time;
            save_effect(&tx, &effect)?;
            effect_event(&tx, &effect, "claimed", Value::Null)?;
            tx.commit()?;
            Ok(EffectClaimOutcome::Claimed(effect))
        })
        .await
    }

    async fn acknowledge_effect(
        &self,
        lease: &EffectLease,
        outcome: EffectOutcome,
    ) -> StoreResult<StoredEffect> {
        lease.key.validate()?;
        validate_outcome(&outcome)?;
        let lease = lease.clone();
        self.with(move |connection, store_id| {
            let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let mut effect = read_effect(&tx, &lease.key)?.ok_or(StoreError::NotFound)?;
            let time = now(&tx)?;
            validate_effect_lease(&effect, &lease, store_id, EffectClaimKind::Dispatch, time)?;
            if effect.status != EffectStatus::InFlight {
                return Err(StoreError::EffectState);
            }
            effect.status = match &outcome {
                EffectOutcome::Succeeded { .. } => EffectStatus::Succeeded,
                EffectOutcome::Failed { .. } => EffectStatus::Failed,
                EffectOutcome::Uncertain { .. } => EffectStatus::Uncertain,
            };
            effect.outcome = Some(outcome);
            effect.lease = None;
            effect.updated_at_ms = time;
            save_effect(&tx, &effect)?;
            effect_event(
                &tx,
                &effect,
                "acknowledged",
                serde_json::to_value(&effect.outcome)?,
            )?;
            tx.commit()?;
            Ok(effect)
        })
        .await
    }

    async fn claim_reconciliation(
        &self,
        key: &EffectKey,
        worker: &str,
        ttl_ms: u64,
    ) -> StoreResult<StoredEffect> {
        key.validate()?;
        identifier(worker)?;
        let ttl = validate_ttl(ttl_ms)?;
        let key = key.clone();
        let worker = worker.to_string();
        self.with(move |connection, store_id| {
            let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let mut effect = read_effect(&tx, &key)?.ok_or(StoreError::NotFound)?;
            let time = now(&tx)?;
            if effect
                .lease
                .as_ref()
                .is_some_and(|lease| lease.expires_at_ms > time)
            {
                return Err(StoreError::Claimed);
            }
            if !matches!(
                effect.status,
                EffectStatus::Uncertain | EffectStatus::InFlight
            ) && !(effect.status == EffectStatus::Pending && effect.attempt > 0)
            {
                return Err(StoreError::EffectState);
            }
            effect.status = EffectStatus::Uncertain;
            effect.updated_at_ms = time;
            allocate_effect_lease(
                &tx,
                store_id,
                &mut effect,
                worker,
                time + ttl,
                EffectClaimKind::Reconcile,
            )?;
            save_effect(&tx, &effect)?;
            effect_event(&tx, &effect, "reconciliation-claimed", Value::Null)?;
            tx.commit()?;
            Ok(effect)
        })
        .await
    }

    async fn reconcile_effect(
        &self,
        lease: &EffectLease,
        resolution: Reconciliation,
    ) -> StoreResult<StoredEffect> {
        lease.key.validate()?;
        match &resolution {
            Reconciliation::Applied { receipt } => receipt.validate()?,
            Reconciliation::NotApplied { evidence }
            | Reconciliation::RetrySafe { evidence }
            | Reconciliation::Pending { evidence }
            | Reconciliation::Failed { evidence } => evidence.validate()?,
        }
        let lease = lease.clone();
        self.with(move |connection, store_id| {
            let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let mut effect = read_effect(&tx, &lease.key)?.ok_or(StoreError::NotFound)?;
            let time = now(&tx)?;
            validate_effect_lease(&effect, &lease, store_id, EffectClaimKind::Reconcile, time)?;
            if effect.status != EffectStatus::Uncertain {
                return Err(StoreError::EffectState);
            }
            match &resolution {
                Reconciliation::Applied { receipt } => {
                    effect.status = EffectStatus::Succeeded;
                    effect.outcome = Some(EffectOutcome::Succeeded {
                        receipt: receipt.clone(),
                    });
                }
                Reconciliation::NotApplied { .. } | Reconciliation::RetrySafe { .. } => {
                    effect.status = EffectStatus::Pending;
                    effect.outcome = None;
                }
                Reconciliation::Pending { evidence } => {
                    effect.status = EffectStatus::Uncertain;
                    effect.outcome = Some(EffectOutcome::Uncertain {
                        evidence: evidence.clone(),
                    });
                }
                Reconciliation::Failed { evidence } => {
                    effect.status = EffectStatus::Failed;
                    effect.outcome = Some(EffectOutcome::Failed {
                        evidence: evidence.clone(),
                    });
                }
            }
            effect.lease = None;
            effect.updated_at_ms = time;
            save_effect(&tx, &effect)?;
            effect_event(
                &tx,
                &effect,
                "reconciled",
                serde_json::to_value(&resolution)?,
            )?;
            tx.commit()?;
            Ok(effect)
        })
        .await
    }
}

#[cfg(test)]
#[path = "conditional_clock_tests.rs"]
mod conditional_clock_tests;

#[cfg(test)]
#[path = "sqlite_publication_tests.rs"]
mod publication_tests;

#[cfg(test)]
#[path = "conditional_transition_tests.rs"]
mod conditional_transition_tests;
