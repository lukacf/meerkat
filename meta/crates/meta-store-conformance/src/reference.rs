//! In-crate reference store implementations.
//!
//! These exist for two reasons, mirroring the upstream harness's
//! `EmulatedCasSessionStore`: they prove every chapter is satisfiable (a
//! chapter no store can pass pins nothing), and they document the minimal
//! semantics a remote backend must implement.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use meerkat_core::session_store::session_projection_cas_token;
use meerkat_core::types::SessionId;
use meerkat_core::{DurabilityDeclaration, DurabilityResolution};
use meerkat_meta::identity_first::{
    AgentIdentity, AgentMemoryError, AgentMemoryForgetResult, AgentMemoryProvider,
    AgentMemoryRecallRequest, AgentMemoryRecord, ContinuityGeneration, ContinuityRecord,
    ContinuityResolveState, ContinuityStore, ContinuityStoreError, FencingToken, NewAgentMemory,
    SessionSnapshot,
};
use meerkat_meta::storage_provider::{
    MetaLeaseAuthority, MetaRealmOpenContext, MetaRealmStoreSet, MetaStorageProvider,
    MetaStorageProviderError, enforce_fail_closed_store_set,
};
use meerkat_meta::unified_runtime::{EventLogError, EventLogStore, EventQuery, PersistedEvent};

// ---------------------------------------------------------------------------
// CompatRollbackContinuityStore
// ---------------------------------------------------------------------------

struct StoredRecord {
    record: ContinuityRecord,
    fence: u64,
}

struct StoredSnapshot {
    identity: AgentIdentity,
    generation: ContinuityGeneration,
    data: Vec<u8>,
}

#[derive(Default)]
struct ContinuityInner {
    records: BTreeMap<String, StoredRecord>,
    snapshots: BTreeMap<String, StoredSnapshot>,
}

/// Minimal in-memory `ContinuityStore` that deliberately does NOT override
/// `rollback_continuity_record`, so the trait's non-atomic compatibility
/// default is what executes. Its required methods follow the same CAS
/// discipline `LocalContinuityStore` enforces: fence compare-and-set
/// (monotonic `>=`), strictly-increasing checkpoint versions per save, and
/// generation-monotonic upserts.
#[derive(Default)]
pub struct CompatRollbackContinuityStore {
    inner: Mutex<ContinuityInner>,
}

impl CompatRollbackContinuityStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ContinuityInner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[async_trait]
impl ContinuityStore for CompatRollbackContinuityStore {
    /// Reverse lookup: the record whose session matches, with its fence and
    /// checkpoint version.
    ///
    /// meerkat/meta made this REQUIRED rather than defaulting to `Ok(None)`,
    /// because a store that silently answers "no record" for a session it is
    /// in fact holding makes a resume look like a first boot. A default is
    /// exactly how a conformance reference would inherit that lie.
    ///
    /// DELIBERATE DIVERGENCE FROM `LocalContinuityStore`, stated rather than
    /// hidden: the SQLite store returns the SUBSTRATE'S CURRENT checkpoint
    /// version - the max across session snapshots and continuity heads -
    /// because the record's own stamp trails it whenever writes landed after
    /// the last checkpoint. This in-memory compat store keeps no per-snapshot
    /// checkpoint version, so it cannot compute that maximum and returns the
    /// record's own stamp. Callers that need substrate-current semantics must
    /// not treat this reference as equivalent on that axis.
    async fn resolve_record_by_session(
        &self,
        session_id: &SessionId,
    ) -> Result<
        Option<(
            ContinuityRecord,
            FencingToken,
            meerkat_meta::identity_first::CheckpointVersion,
        )>,
        ContinuityStoreError,
    > {
        let inner = self.lock();
        Ok(inner
            .records
            .values()
            .find(|stored| &stored.record.session_id == session_id)
            .map(|stored| {
                (
                    stored.record.clone(),
                    FencingToken::new(stored.fence),
                    stored.record.checkpoint_version,
                )
            }))
    }

    async fn resolve_many(
        &self,
        identities: &[AgentIdentity],
    ) -> Result<BTreeMap<AgentIdentity, ContinuityResolveState>, ContinuityStoreError> {
        let inner = self.lock();
        Ok(identities
            .iter()
            .map(|identity| {
                let state = inner.records.get(identity.as_str()).map_or(
                    ContinuityResolveState::Uninitialized,
                    |stored| ContinuityResolveState::Ready {
                        record: stored.record.clone(),
                    },
                );
                (identity.clone(), state)
            })
            .collect())
    }

    async fn load_session_snapshot(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<SessionSnapshot>, ContinuityStoreError> {
        let inner = self.lock();
        Ok(inner
            .snapshots
            .get(&session_id.to_string())
            .map(|snapshot| SessionSnapshot {
                data: snapshot.data.clone(),
            }))
    }

    async fn delete_session_snapshot_if_current_revision(
        &self,
        session_id: &SessionId,
        expected_current_revision: &str,
    ) -> Result<bool, ContinuityStoreError> {
        let mut inner = self.lock();
        let key = session_id.to_string();
        let Some(snapshot) = inner.snapshots.get(&key) else {
            return Ok(false);
        };
        let session: meerkat_core::Session = serde_json::from_slice(&snapshot.data)
            .map_err(|error| ContinuityStoreError::Io(format!("deserialize snapshot: {error}")))?;
        let current = session_projection_cas_token(&session)
            .map_err(|error| ContinuityStoreError::Io(format!("cas token: {error}")))?;
        if current != expected_current_revision {
            return Ok(false);
        }
        inner.snapshots.remove(&key);
        Ok(true)
    }

    async fn save_session_snapshot(
        &self,
        identity: &AgentIdentity,
        session_id: &SessionId,
        generation: ContinuityGeneration,
        version: meerkat_meta::identity_first::CheckpointVersion,
        fencing_token: FencingToken,
        snapshot: &SessionSnapshot,
    ) -> Result<(), ContinuityStoreError> {
        let mut inner = self.lock();
        if let Some(stored) = inner.records.get(identity.as_str()) {
            if stored.record.session_id != *session_id || stored.record.generation != generation {
                return Err(ContinuityStoreError::NotFound {
                    identity: identity.clone(),
                });
            }
            if fencing_token.get() < stored.fence {
                return Err(ContinuityStoreError::StaleFencingToken {
                    identity: identity.clone(),
                    presented: fencing_token,
                    current: FencingToken::new(stored.fence),
                });
            }
            if version.get() <= stored.record.checkpoint_version.get() {
                return Err(ContinuityStoreError::StaleCheckpointVersion {
                    identity: identity.clone(),
                    presented: version,
                    current: stored.record.checkpoint_version,
                });
            }
        }
        let key = session_id.to_string();
        if let Some(existing) = inner.snapshots.get(&key)
            && (existing.identity != *identity || existing.generation != generation)
        {
            return Err(ContinuityStoreError::Corruption(format!(
                "snapshot {session_id} already owned by {}/generation {}",
                existing.identity, existing.generation
            )));
        }
        inner.snapshots.insert(
            key,
            StoredSnapshot {
                identity: identity.clone(),
                generation,
                data: snapshot.data.clone(),
            },
        );
        if let Some(stored) = inner.records.get_mut(identity.as_str()) {
            stored.record.checkpoint_version = version;
            stored.fence = fencing_token.get();
        }
        Ok(())
    }

    async fn upsert_continuity_record(
        &self,
        record: &ContinuityRecord,
        fencing_token: FencingToken,
    ) -> Result<(), ContinuityStoreError> {
        let mut inner = self.lock();
        let key = record.identity.as_str().to_string();
        if let Some(stored) = inner.records.get(&key) {
            if fencing_token.get() < stored.fence {
                return Err(ContinuityStoreError::StaleFencingToken {
                    identity: record.identity.clone(),
                    presented: fencing_token,
                    current: FencingToken::new(stored.fence),
                });
            }
            if record.generation.get() < stored.record.generation.get() {
                return Err(ContinuityStoreError::StaleContinuityGeneration {
                    identity: record.identity.clone(),
                    presented: record.generation,
                    current: stored.record.generation,
                });
            }
            // A same-generation rebind must not rewind the version stream; a
            // generation advance resets it (the excluded version wins).
            let mut next = record.clone();
            if record.generation == stored.record.generation {
                next.checkpoint_version = stored
                    .record
                    .checkpoint_version
                    .max(record.checkpoint_version);
            }
            inner.records.insert(
                key,
                StoredRecord {
                    record: next,
                    fence: fencing_token.get(),
                },
            );
        } else {
            inner.records.insert(
                key,
                StoredRecord {
                    record: record.clone(),
                    fence: fencing_token.get(),
                },
            );
        }
        Ok(())
    }

    async fn delete_continuity_record(
        &self,
        identity: &AgentIdentity,
        fencing_token: FencingToken,
    ) -> Result<(), ContinuityStoreError> {
        let mut inner = self.lock();
        if let Some(stored) = inner.records.get(identity.as_str())
            && fencing_token.get() < stored.fence
        {
            return Err(ContinuityStoreError::StaleFencingToken {
                identity: identity.clone(),
                presented: fencing_token,
                current: FencingToken::new(stored.fence),
            });
        }
        inner.records.remove(identity.as_str());
        inner
            .snapshots
            .retain(|_, snapshot| snapshot.identity != *identity);
        Ok(())
    }
    /// `None`, stated: this in-memory compatibility reference keeps whole
    /// snapshots only.
    fn as_incremental_sessions(
        &self,
    ) -> Option<
        std::sync::Arc<dyn meerkat_meta::identity_first::contracts::ContinuityIncrementalSessions>,
    > {
        None
    }
}

// ---------------------------------------------------------------------------
// ReferenceEventLogStore
// ---------------------------------------------------------------------------

/// Minimal id-idempotent in-memory `EventLogStore`.
///
/// Meta bundles no durable production `EventLogStore` (the built-in default
/// is a private null store that drops every event), so this reference
/// implementation both proves the [`crate::chapters::event_log`] chapter is
/// satisfiable and documents the store-side redelivery contract the Meta
/// flusher's retry loop relies on.
#[derive(Default)]
pub struct ReferenceEventLogStore {
    events: Mutex<BTreeMap<String, PersistedEvent>>,
}

impl ReferenceEventLogStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl EventLogStore for ReferenceEventLogStore {
    fn append_batch(
        &self,
        events: Vec<PersistedEvent>,
    ) -> Pin<Box<dyn Future<Output = Result<(), EventLogError>> + Send + '_>> {
        Box::pin(async move {
            let mut map = self
                .events
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for event in events {
                // Documented contract: duplicate events (same `id`) are
                // ignored, keeping redelivery exactly-once.
                map.entry(event.id.clone()).or_insert(event);
            }
            Ok(())
        })
    }

    fn query(
        &self,
        query: EventQuery,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<PersistedEvent>, EventLogError>> + Send + '_>> {
        Box::pin(async move {
            let map = self
                .events
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut rows: Vec<PersistedEvent> = map.values().cloned().collect();
            rows.sort_by_key(|event| event.seq);
            if let Some(after) = query.after_seq {
                rows.retain(|event| event.seq > after);
            }
            if let Some(limit) = query.limit {
                rows.truncate(limit);
            }
            Ok(rows)
        })
    }
}

// ---------------------------------------------------------------------------
// ReferenceInMemoryAgentMemoryStore
// ---------------------------------------------------------------------------

/// Minimal in-memory `AgentMemoryProvider` for the reference bundle: the
/// recall/remember/forget shape with every other capability honestly refused via the trait's
/// `Unsupported` defaults.
#[derive(Default)]
pub struct ReferenceInMemoryAgentMemoryStore {
    records: Mutex<BTreeMap<(String, String), Vec<AgentMemoryRecord>>>,
    next_id: Mutex<u64>,
}

impl ReferenceInMemoryAgentMemoryStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl AgentMemoryProvider for ReferenceInMemoryAgentMemoryStore {
    async fn recall(
        &self,
        request: AgentMemoryRecallRequest,
    ) -> Result<Vec<AgentMemoryRecord>, AgentMemoryError> {
        let records = self
            .records
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut rows = records
            .get(&(request.realm.clone(), request.identity.as_str().to_string()))
            .cloned()
            .unwrap_or_default();
        rows.truncate(request.max_entries);
        Ok(rows)
    }

    async fn remember(
        &self,
        realm: &str,
        identity: &AgentIdentity,
        memory: NewAgentMemory,
    ) -> Result<AgentMemoryRecord, AgentMemoryError> {
        let memory_id = {
            let mut next = self
                .next_id
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *next += 1;
            format!("reference-memory-{next}")
        };
        let record = AgentMemoryRecord {
            memory_id,
            title: memory.title,
            body: memory.body,
            tags: memory.tags,
            created_at_ms: 0,
            updated_at_ms: 0,
        };
        self.records
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry((realm.to_string(), identity.as_str().to_string()))
            .or_default()
            .push(record.clone());
        Ok(record)
    }

    fn supports_remember(&self) -> bool {
        true
    }

    async fn forget(
        &self,
        realm: &str,
        identity: &AgentIdentity,
        memory_id: &str,
    ) -> Result<AgentMemoryForgetResult, AgentMemoryError> {
        let mut records = self
            .records
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let deleted = records
            .get_mut(&(realm.to_string(), identity.as_str().to_string()))
            .map(|rows| {
                let before = rows.len();
                rows.retain(|row| row.memory_id != memory_id);
                rows.len() != before
            })
            .unwrap_or(false);
        Ok(AgentMemoryForgetResult {
            memory_id: memory_id.to_string(),
            deleted,
        })
    }

    fn supports_forget(&self) -> bool {
        true
    }
}

// ---------------------------------------------------------------------------
// ReferenceMemoryBundleProvider — the one-remote-bundle demonstration
// ---------------------------------------------------------------------------

/// Meerkat-level half of the reference bundle: a non-disk
/// [`meerkat::storage_provider::RealmStorageProvider`] serving declared
/// in-memory implementations of every meerkat-shared store.
#[derive(Debug, Clone, Copy, Default)]
pub struct ReferenceMemoryRealmProvider;

#[async_trait]
impl meerkat::storage_provider::RealmStorageProvider for ReferenceMemoryRealmProvider {
    fn name(&self) -> &'static str {
        "reference-memory"
    }

    async fn open(
        &self,
        ctx: &meerkat::storage_provider::RealmOpenContext,
    ) -> Result<meerkat::storage_provider::RealmStoreSet, meerkat::PersistenceError> {
        let session_store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let blob_store: Arc<dyn meerkat_core::BlobStore> =
            Arc::new(meerkat_store::MemoryBlobStore::new());
        Ok(meerkat::storage_provider::RealmStoreSet {
            session_store,
            runtime_store: Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
            // Durable jobs (meerkat 0.8.8) added a job store to the realm set.
            job_store: Arc::new(meerkat::MemoryDetachedJobStore::new()),
            schedule_store: Arc::new(meerkat::MemoryScheduleStore::new()),
            workgraph_store: Arc::new(meerkat::MemoryWorkGraphStore::new()),
            blob_store,
            artifact_store: Arc::new(meerkat_store::MemoryArtifactStore::new()),
            store_path: ctx.paths.root.clone(),
            projection_root: None,
            durability: [
                "sessions",
                "runtime",
                "jobs",
                "schedule",
                "workgraph",
                "blobs",
                "artifacts",
            ]
            .iter()
            .map(|domain| {
                DurabilityDeclaration::durable(domain, DurabilityResolution::DeclaredEphemeral)
            })
            .collect(),
        })
    }
}

/// The in-crate reference [`MetaStorageProvider`]: a **non-disk**,
/// in-memory-declared bundle proving that a downstream implements ONE
/// provider and gets sessions, continuity, events, blobs, and memory
/// through the single seam — without touching Meta internals. Every open
/// mints a fresh realm (in-memory stores share state per handle, which is
/// the correct restart model for deliberately non-persistent backends).
#[derive(Default)]
pub struct ReferenceMemoryBundleProvider {
    meerkat: ReferenceMemoryRealmProvider,
}

impl ReferenceMemoryBundleProvider {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl MetaStorageProvider for ReferenceMemoryBundleProvider {
    fn name(&self) -> &'static str {
        "reference-memory"
    }

    async fn open_realm(
        &self,
        ctx: &MetaRealmOpenContext,
    ) -> Result<MetaRealmStoreSet, MetaStorageProviderError> {
        let set = MetaRealmStoreSet {
            continuity_store: Arc::new(CompatRollbackContinuityStore::new()),
            lease_authority: MetaLeaseAuthority::FencingFloor(0),
            event_log_store: Some(Box::new(ReferenceEventLogStore::new())),
            console_log_store: Arc::new(
                meerkat_meta::console_aggregator::InMemoryConsoleLogStore::new(),
            ),
            metadata_store: Arc::new(meerkat_meta::runtime::InMemoryMetadataStore::new()),
            blob_store: Arc::new(meerkat_meta::blob_store::ObjectStoreBlobStore::memory()),
            agent_memory_provider: Some(Arc::new(ReferenceInMemoryAgentMemoryStore::new())),
            schedule_store: Arc::new(meerkat::MemoryScheduleStore::new()),
            durability: [
                "continuity",
                "event_log",
                "console",
                "metadata",
                "blobs",
                "agent_memory",
                "schedule",
            ]
            .iter()
            .map(|domain| {
                DurabilityDeclaration::durable(domain, DurabilityResolution::DeclaredEphemeral)
            })
            .collect(),
        };
        enforce_fail_closed_store_set(&set, ctx)?;
        Ok(set)
    }

    fn meerkat_provider(&self) -> &dyn meerkat::storage_provider::RealmStorageProvider {
        &self.meerkat
    }
}
