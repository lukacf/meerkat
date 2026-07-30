//! InMemoryRuntimeStore — in-memory implementation for testing/ephemeral.
//!
//! Uses `tokio::sync::Mutex` per the in-memory concurrency rule.
//! All mutations complete inside one lock acquisition (no lock held across .await).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};

use indexmap::IndexMap;
use meerkat_core::lifecycle::{InputId, RunBoundaryReceipt, RunId};
#[cfg(not(target_arch = "wasm32"))]
use tokio::sync::Mutex;
#[cfg(target_arch = "wasm32")]
use tokio_with_wasm::alias::sync::Mutex;

use super::{
    AuthOAuthFlowSnapshotUpdate, CommittedWholeBlobProvisionalTail, CommittedWholeBlobSnapshot,
    ExactInputStateObservation, FencedInputStateBatchCasOutcome, FencedMachineLifecycleCasOutcome,
    InputStateBatchCasImplementationProfile, InputStateBatchCasOutcome, InputStateRow,
    MachineLifecycleCasOutcome, MachineLifecycleCommit, MachineLifecycleExpectedVersion,
    MachineLifecycleObservation, MachineLifecycleStoreRecord, PreparedRecoveryInputSnapshot,
    PreparedRecoveryInputStateMutation, PreparedRuntimeSessionCommitResult,
    PreparedWholeBlobProvisionalTail, PreparedWholeBlobRewriteStoreParts,
    PreparedWholeBlobSnapshot, PreparedWholeBlobSnapshotCas, RecoveryInputSetRevision,
    RecoveryInputStateMutation, RuntimeDeliveryAuthorityCasOutcome, RuntimeDeliveryAuthorityRecord,
    RuntimeDeliveryStoreRecord, RuntimeSessionAuthority, RuntimeSessionAuthorityReadCost,
    RuntimeSessionPersistenceProfile, RuntimeStore, RuntimeStoreError, RuntimeStoreWriteFence,
    RuntimeStoreWriteFenceOutcome, SerializedSessionSnapshot, WholeBlobProvisionalTailAuthority,
    WholeBlobSnapshotCasOutcome, WholeBlobStoreAuthority, classify_machine_lifecycle_record,
    complete_compaction_projection_intent, decoded_prepared_machine_lifecycle_replacement,
    execute_runtime_store_write_fence, parsed_whole_blob_snapshot, prepare_input_state_batch_cas,
    prepare_machine_lifecycle_replacement, prepare_recovery_input_state_mutations,
    validate_input_state_batch_read_ids, validate_machine_lifecycle_replacement,
};
use crate::identifiers::{IdempotencyKey, LogicalRuntimeId};
use crate::input_state::{InputStatePersistenceRecord, StoredInputState};
use crate::ops_lifecycle::PersistedOpsSnapshot;

/// Receipt key: (runtime_id, run_id, sequence).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ReceiptKey {
    runtime_id: String,
    run_id: RunId,
    sequence: u64,
}

#[derive(Debug, Clone)]
struct CompactionOutboxEntry {
    intent: meerkat_core::CompactionProjectionIntent,
    finalized: bool,
}

#[derive(Debug, Clone)]
struct StoredWholeBlobProvisionalTail {
    authority: WholeBlobProvisionalTailAuthority,
    candidate_bytes: Arc<Vec<u8>>,
    conversation_digest: String,
    message_count: u64,
    catalog_entry: super::RuntimeSessionCatalogEntry,
    compaction_projection_intents: Vec<meerkat_core::CompactionProjectionIntent>,
}

#[cfg(test)]
type InputStateBatchCasTestBlock = (
    Arc<crate::tokio::sync::Notify>,
    Arc<crate::tokio::sync::Notify>,
);

/// Inner state protected by the mutex.
#[derive(Debug, Default)]
struct Inner {
    /// runtime_id → (input_id → StoredInputState). IndexMap for deterministic iteration order.
    input_states: HashMap<String, IndexMap<InputId, StoredInputState>>,
    /// Runtime id → canonical owners of unfinished terminal work.
    pending_terminal_owners: HashMap<String, BTreeSet<uuid::Uuid>>,
    /// Runtime id → canonical complete set of nonterminal input ids.
    recovery_nonterminal_inputs: HashMap<String, BTreeSet<uuid::Uuid>>,
    /// Runtime id → store-owned revision bumped for every input-row mutation.
    ///
    /// Missing is canonical generation zero and remains a real absence fence.
    recovery_input_set_revisions: HashMap<String, u64>,
    /// Runtime id → exact idempotency key → input id.
    input_idempotency_index: HashMap<String, HashMap<String, InputId>>,
    /// Receipt storage.
    receipts: HashMap<ReceiptKey, RunBoundaryReceipt>,
    /// Exact machine-authorized recovery witness by runtime/candidate.
    recovery_boundaries: HashMap<(String, String), super::CommittedRecoveryBoundary>,
    /// Runtime session snapshots keyed by canonical runtime id.
    sessions: HashMap<String, Arc<Vec<u8>>>,
    /// Fixed-size authority paired atomically with every WholeBlob snapshot.
    session_authorities: HashMap<String, WholeBlobStoreAuthority>,
    /// Body-free session listing/lifecycle projection.
    session_catalog: HashMap<String, super::RuntimeSessionCatalogEntry>,
    /// Store-owned provisional candidate body and exact base/run identity.
    whole_blob_provisional_tails: HashMap<String, StoredWholeBlobProvisionalTail>,
    /// Canonical runtime ids whose projection fallback is quarantined.
    ///
    /// Mirrors the durable SQLite `runtime_projection_quarantine` table: set
    /// when a rejected runtime snapshot is cleared via
    /// `clear_session_snapshot_if_current`, cleared whenever a live snapshot is
    /// written for the runtime.
    projection_quarantine: HashSet<String>,
    /// Exact persisted machine-lifecycle bytes. Raw storage is required so
    /// malformed and unsupported rows remain observable instead of being
    /// normalized by an eager typed decode.
    runtime_lifecycle: HashMap<String, Vec<u8>>,
    /// Persisted ops lifecycle snapshots.
    ops_lifecycle_snapshots: HashMap<String, PersistedOpsSnapshot>,
    /// Exact ops epochs retired by atomic unregister finalization. Tombstones
    /// outlive row deletion so detached callbacks cannot resurrect them.
    retired_ops_epochs: HashSet<(String, meerkat_core::RuntimeEpochId)>,
    /// Runtime id -> transcript-rewrite-keyed compaction projection outbox.
    compaction_projection_outbox:
        HashMap<String, HashMap<meerkat_core::CompactionProjectionId, CompactionOutboxEntry>>,
    /// Exact generated runtime-delivery authority by logical runtime.
    runtime_delivery_authority: HashMap<String, RuntimeDeliveryAuthorityRecord>,
    /// Durable runtime-delivery rows ordered by generated sequence.
    runtime_delivery_records: HashMap<String, BTreeMap<u64, RuntimeDeliveryStoreRecord>>,
}

fn sync_runtime_session_catalog_lifecycle(
    inner: &mut Inner,
    runtime_id: &str,
    runtime_state: crate::RuntimeState,
) {
    if let Some(entry) = inner.session_catalog.get_mut(runtime_id) {
        entry.set_runtime_state(Some(runtime_state));
    }
}

fn store_input_state_prechecked(
    inner: &mut Inner,
    runtime_id: &str,
    bundle: StoredInputState,
    next_revision: u64,
) {
    let input_id = bundle.state.input_id.clone();
    let new_idempotency_key = bundle
        .state
        .idempotency_key
        .as_ref()
        .map(|key| key.0.clone());
    let old_idempotency_key = inner
        .input_states
        .get(runtime_id)
        .and_then(|states| states.get(&input_id))
        .and_then(|state| state.state.idempotency_key.as_ref())
        .map(|key| key.0.clone());
    let owners_empty = {
        let owners = inner
            .pending_terminal_owners
            .entry(runtime_id.to_string())
            .or_default();
        if super::input_state_is_pending_terminal_owner(&bundle.state) {
            owners.insert(input_id.0);
        } else {
            owners.remove(&input_id.0);
        }
        owners.is_empty()
    };
    if owners_empty {
        inner.pending_terminal_owners.remove(runtime_id);
    }
    let nonterminal_empty = {
        let nonterminal = inner
            .recovery_nonterminal_inputs
            .entry(runtime_id.to_string())
            .or_default();
        if super::input_state_is_recovery_nonterminal(&bundle) {
            nonterminal.insert(input_id.0);
        } else {
            nonterminal.remove(&input_id.0);
        }
        nonterminal.is_empty()
    };
    if nonterminal_empty {
        inner.recovery_nonterminal_inputs.remove(runtime_id);
    }
    inner
        .input_states
        .entry(runtime_id.to_string())
        .or_default()
        .insert(input_id.clone(), bundle);
    if old_idempotency_key != new_idempotency_key {
        let remove_empty_index = if let Some(old_key) = old_idempotency_key
            && let Some(index) = inner.input_idempotency_index.get_mut(runtime_id)
        {
            if index.get(&old_key) == Some(&input_id) {
                index.remove(&old_key);
            }
            index.is_empty()
        } else {
            false
        };
        if remove_empty_index {
            inner.input_idempotency_index.remove(runtime_id);
        }
        if let Some(new_key) = new_idempotency_key {
            inner
                .input_idempotency_index
                .entry(runtime_id.to_string())
                .or_default()
                .insert(new_key, input_id);
        }
    }
    inner
        .recovery_input_set_revisions
        .insert(runtime_id.to_string(), next_revision);
}

fn store_input_state(
    inner: &mut Inner,
    runtime_id: &str,
    bundle: StoredInputState,
) -> Result<(), RuntimeStoreError> {
    let next_revision = inner
        .recovery_input_set_revisions
        .get(runtime_id)
        .copied()
        .unwrap_or(0)
        .checked_add(1)
        .ok_or_else(|| {
            RuntimeStoreError::WriteFailed(format!(
                "input-set revision exhausted for runtime {runtime_id}"
            ))
        })?;
    let input_id = &bundle.state.input_id;
    if let Some(key) = bundle.state.idempotency_key.as_ref()
        && inner
            .input_idempotency_index
            .get(runtime_id)
            .and_then(|index| index.get(&key.0))
            .is_some_and(|indexed_input_id| indexed_input_id != input_id)
    {
        return Err(RuntimeStoreError::WriteFailed(format!(
            "idempotency key `{key}` already belongs to another input in runtime {runtime_id}"
        )));
    }
    store_input_state_prechecked(inner, runtime_id, bundle, next_revision);
    Ok(())
}

fn delete_input_state_prechecked(
    inner: &mut Inner,
    runtime_id: &str,
    input_id: &InputId,
    next_revision: u64,
) {
    let removed = inner
        .input_states
        .get_mut(runtime_id)
        .and_then(|states| states.shift_remove(input_id));
    let Some(removed) = removed else {
        // Prepared batches prove row presence while holding the same store
        // mutex. Treat an impossible internal mismatch as a no-op instead of
        // introducing a fallible boundary after sibling effects are visible.
        return;
    };
    if inner
        .input_states
        .get(runtime_id)
        .is_some_and(IndexMap::is_empty)
    {
        inner.input_states.remove(runtime_id);
    }
    if let Some(owners) = inner.pending_terminal_owners.get_mut(runtime_id) {
        owners.remove(&input_id.0);
        if owners.is_empty() {
            inner.pending_terminal_owners.remove(runtime_id);
        }
    }
    if let Some(nonterminal) = inner.recovery_nonterminal_inputs.get_mut(runtime_id) {
        nonterminal.remove(&input_id.0);
        if nonterminal.is_empty() {
            inner.recovery_nonterminal_inputs.remove(runtime_id);
        }
    }
    let remove_empty_index = if let Some(key) = removed.state.idempotency_key.as_ref()
        && let Some(index) = inner.input_idempotency_index.get_mut(runtime_id)
    {
        if index.get(&key.0) == Some(input_id) {
            index.remove(&key.0);
        }
        index.is_empty()
    } else {
        false
    };
    if remove_empty_index {
        inner.input_idempotency_index.remove(runtime_id);
    }
    inner
        .recovery_input_set_revisions
        .insert(runtime_id.to_string(), next_revision);
}

enum MemoryInputStateMutation {
    Upsert(StoredInputState),
    Delete(InputId),
}

impl MemoryInputStateMutation {
    fn input_id(&self) -> &InputId {
        match self {
            Self::Upsert(bundle) => &bundle.state.input_id,
            Self::Delete(input_id) => input_id,
        }
    }

    fn target_idempotency_key(&self) -> Option<&str> {
        match self {
            Self::Upsert(bundle) => bundle
                .state
                .idempotency_key
                .as_ref()
                .map(|key| key.0.as_str()),
            Self::Delete(_) => None,
        }
    }
}

struct PreparedMemoryInputStateMutation {
    mutation: MemoryInputStateMutation,
    next_revision: u64,
}

/// Validate a complete in-memory input mutation set before any sibling effect
/// becomes visible.
///
/// Besides revision exhaustion and duplicate input ids, this releases and
/// reclaims idempotency keys as one logical batch. That permits valid key swaps
/// while rejecting a collision with any input outside the mutation set.
fn prepare_memory_input_state_mutations(
    inner: &Inner,
    runtime_id: &str,
    mutations: Vec<MemoryInputStateMutation>,
) -> Result<Vec<PreparedMemoryInputStateMutation>, RuntimeStoreError> {
    let mut target_ids = HashSet::with_capacity(mutations.len());
    for mutation in &mutations {
        if !target_ids.insert(mutation.input_id().clone()) {
            return Err(RuntimeStoreError::WriteFailed(format!(
                "atomic input-state mutation set repeats input {} in runtime {runtime_id}",
                mutation.input_id()
            )));
        }
        if matches!(mutation, MemoryInputStateMutation::Delete(_))
            && !inner
                .input_states
                .get(runtime_id)
                .is_some_and(|states| states.contains_key(mutation.input_id()))
        {
            return Err(RuntimeStoreError::InputRowVersionConflict {
                input_id: mutation.input_id().to_string(),
            });
        }
    }

    let mut target_keys: HashMap<String, InputId> = HashMap::new();
    for mutation in &mutations {
        let Some(key) = mutation.target_idempotency_key() else {
            continue;
        };
        if let Some(other_input_id) =
            target_keys.insert(key.to_string(), mutation.input_id().clone())
            && other_input_id != *mutation.input_id()
        {
            return Err(RuntimeStoreError::WriteFailed(format!(
                "idempotency key `{key}` is claimed by both {other_input_id} and {} in runtime {runtime_id}",
                mutation.input_id()
            )));
        }
        if let Some(existing_input_id) = inner
            .input_idempotency_index
            .get(runtime_id)
            .and_then(|index| index.get(key))
            && existing_input_id != mutation.input_id()
            && !target_ids.contains(existing_input_id)
        {
            return Err(RuntimeStoreError::WriteFailed(format!(
                "idempotency key `{key}` already belongs to input {existing_input_id} outside the atomic mutation set for runtime {runtime_id}"
            )));
        }
    }

    let current_revision = inner
        .recovery_input_set_revisions
        .get(runtime_id)
        .copied()
        .unwrap_or(0);
    let mutation_count = u64::try_from(mutations.len()).map_err(|_| {
        RuntimeStoreError::WriteFailed(format!(
            "input-set mutation count does not fit the revision for runtime {runtime_id}"
        ))
    })?;
    current_revision
        .checked_add(mutation_count)
        .ok_or_else(|| {
            RuntimeStoreError::WriteFailed(format!(
                "input-set revision exhausted for runtime {runtime_id}"
            ))
        })?;

    mutations
        .into_iter()
        .enumerate()
        .map(|(index, mutation)| {
            let offset = u64::try_from(index + 1).map_err(|_| {
                RuntimeStoreError::WriteFailed(format!(
                    "input-set mutation ordinal does not fit the revision for runtime {runtime_id}"
                ))
            })?;
            Ok(PreparedMemoryInputStateMutation {
                mutation,
                next_revision: current_revision + offset,
            })
        })
        .collect()
}

/// Apply a previously validated input batch while retaining the same store
/// mutex. No fallible work remains here, so a larger boundary can safely write
/// its other effects before or after this call.
fn apply_prepared_memory_input_state_mutations(
    inner: &mut Inner,
    runtime_id: &str,
    prepared: Vec<PreparedMemoryInputStateMutation>,
) {
    let remove_empty_index = if let Some(index) = inner.input_idempotency_index.get_mut(runtime_id)
    {
        for mutation in &prepared {
            let input_id = mutation.mutation.input_id();
            let old_key = inner
                .input_states
                .get(runtime_id)
                .and_then(|states| states.get(input_id))
                .and_then(|bundle| bundle.state.idempotency_key.as_ref())
                .map(|key| key.0.as_str());
            if let Some(old_key) = old_key
                && Some(old_key) != mutation.mutation.target_idempotency_key()
                && index.get(old_key) == Some(input_id)
            {
                index.remove(old_key);
            }
        }
        index.is_empty()
    } else {
        false
    };
    if remove_empty_index {
        inner.input_idempotency_index.remove(runtime_id);
    }

    for prepared in prepared {
        match prepared.mutation {
            MemoryInputStateMutation::Upsert(bundle) => {
                store_input_state_prechecked(inner, runtime_id, bundle, prepared.next_revision);
            }
            MemoryInputStateMutation::Delete(input_id) => {
                delete_input_state_prechecked(inner, runtime_id, &input_id, prepared.next_revision);
            }
        }
    }
}

/// In-memory runtime store. Thread-safe via `tokio::sync::Mutex`.
#[derive(Debug, Clone)]
pub struct InMemoryRuntimeStore {
    inner: Arc<Mutex<Inner>>,
    auth_oauth_flow_snapshot: Arc<StdMutex<Option<Vec<u8>>>>,
    #[cfg(test)]
    input_state_batch_cas_before: Arc<StdMutex<Option<InputStateBatchCasTestBlock>>>,
    #[cfg(test)]
    input_state_batch_cas_after_commit: Arc<StdMutex<Option<InputStateBatchCasTestBlock>>>,
    #[cfg(test)]
    machine_lifecycle_cas_conflicts_remaining: Arc<AtomicUsize>,
    #[cfg(test)]
    machine_lifecycle_observe_errors_remaining: Arc<AtomicUsize>,
    /// Candidate bytes shipped into the snapshot byte-equality compare.
    /// Observability seam for the length-gate regression tests only.
    #[cfg(test)]
    snapshot_byte_probe_bytes: Arc<std::sync::atomic::AtomicU64>,
}

impl InMemoryRuntimeStore {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner::default())),
            auth_oauth_flow_snapshot: Arc::new(StdMutex::new(None)),
            #[cfg(test)]
            input_state_batch_cas_before: Arc::new(StdMutex::new(None)),
            #[cfg(test)]
            input_state_batch_cas_after_commit: Arc::new(StdMutex::new(None)),
            #[cfg(test)]
            machine_lifecycle_cas_conflicts_remaining: Arc::new(AtomicUsize::new(0)),
            #[cfg(test)]
            machine_lifecycle_observe_errors_remaining: Arc::new(AtomicUsize::new(0)),
            #[cfg(test)]
            snapshot_byte_probe_bytes: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        }
    }

    /// Total candidate bytes this store has shipped into the snapshot
    /// byte-equality compare. Length-gate regression tests only.
    #[cfg(test)]
    pub(crate) fn snapshot_byte_probe_bytes(&self) -> u64 {
        self.snapshot_byte_probe_bytes
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    #[cfg(test)]
    pub(crate) fn block_next_input_state_batch_cas_before_mutation(
        &self,
        entered: Arc<crate::tokio::sync::Notify>,
        release: Arc<crate::tokio::sync::Notify>,
    ) {
        *self
            .input_state_batch_cas_before
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some((entered, release));
    }

    #[cfg(test)]
    pub(crate) fn block_next_input_state_batch_cas_after_commit(
        &self,
        entered: Arc<crate::tokio::sync::Notify>,
        release: Arc<crate::tokio::sync::Notify>,
    ) {
        *self
            .input_state_batch_cas_after_commit
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some((entered, release));
    }

    #[cfg(test)]
    pub(crate) async fn seed_machine_lifecycle_raw(
        &self,
        runtime_id: &LogicalRuntimeId,
        bytes: Vec<u8>,
    ) {
        self.inner
            .lock()
            .await
            .runtime_lifecycle
            .insert(runtime_id.0.clone(), bytes);
    }

    #[cfg(test)]
    pub(crate) fn conflict_next_machine_lifecycle_cas(&self) {
        self.machine_lifecycle_cas_conflicts_remaining
            .fetch_add(1, Ordering::SeqCst);
    }

    #[cfg(test)]
    pub(crate) fn fail_next_machine_lifecycle_observation(&self) {
        self.machine_lifecycle_observe_errors_remaining
            .fetch_add(1, Ordering::SeqCst);
    }

    async fn commit_session_snapshot_inner(
        &self,
        runtime_id: &LogicalRuntimeId,
        prepared: PreparedWholeBlobSnapshot,
    ) -> Result<WholeBlobStoreAuthority, RuntimeStoreError> {
        if &LogicalRuntimeId::for_session(prepared.session().id()) != runtime_id {
            return Err(RuntimeStoreError::SessionPersistenceAuthorityConflict {
                runtime_id: runtime_id.to_string(),
                detail: format!(
                    "WholeBlob payload session {} does not bind this runtime",
                    prepared.session().id()
                ),
            });
        }
        let mut inner = self.inner.lock().await;
        ensure_compaction_intents_already_outboxed(&inner, runtime_id, prepared.session())?;
        commit_prepared_whole_blob_snapshot_locked(&mut inner, runtime_id, prepared)
    }

    async fn atomic_apply_prepared_whole_blob(
        &self,
        runtime_id: &LogicalRuntimeId,
        prepared_session: Option<PreparedWholeBlobSnapshot>,
        receipt: RunBoundaryReceipt,
        input_updates: Vec<InputStatePersistenceRecord>,
        session_store_key: Option<meerkat_core::types::SessionId>,
    ) -> Result<Option<WholeBlobStoreAuthority>, RuntimeStoreError> {
        let rid = runtime_id.0.clone();
        let compaction_intents = prepared_session
            .as_ref()
            .map(|prepared| super::validated_compaction_projection_intents(prepared.session()))
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

        let input_updates = input_updates
            .into_iter()
            .map(InputStatePersistenceRecord::into_stored_and_expected)
            .collect::<Vec<_>>();
        let key = ReceiptKey {
            runtime_id: rid.clone(),
            run_id: receipt.run_id.clone(),
            sequence: receipt.sequence,
        };
        let mut inner = self.inner.lock().await;
        if prepared_session.is_none() && inner.whole_blob_provisional_tails.contains_key(&rid) {
            return Err(RuntimeStoreError::SessionPersistenceAuthorityConflict {
                runtime_id: rid,
                detail: "receipt-only boundary cannot bypass a store-owned WholeBlob candidate"
                    .to_string(),
            });
        }
        if let Some(existing) = inner.compaction_projection_outbox.get(&rid) {
            for intent in &compaction_intents {
                if let Some(entry) = existing.get(&intent.projection) {
                    if entry.finalized {
                        return Err(RuntimeStoreError::WriteFailed(format!(
                            "atomic session snapshot replays finalized compaction intent {}",
                            intent.projection.revision()
                        )));
                    }
                    if entry.intent != *intent {
                        return Err(RuntimeStoreError::WriteFailed(format!(
                            "conflicting compaction outbox intent for rewrite {}",
                            intent.projection.revision()
                        )));
                    }
                }
            }
        }
        if inner.receipts.contains_key(&key) {
            return Err(RuntimeStoreError::WriteFailed(format!(
                "boundary receipt already exists for runtime '{}' run {} sequence {}",
                runtime_id, receipt.run_id, receipt.sequence
            )));
        }
        precheck_fenced_input_updates(inner.input_states.get(&rid), &input_updates)?;
        let prepared_input_mutations = prepare_memory_input_state_mutations(
            &inner,
            &rid,
            input_updates
                .into_iter()
                .map(|(bundle, _expected)| MemoryInputStateMutation::Upsert(bundle))
                .collect(),
        )?;

        // The body/authority promotion is the final fallible step before any
        // other mutation. Everything below is infallible under this lock, so a
        // stale base/run/candidate cannot leave a partial receipt or outbox.
        let authority = prepared_session
            .map(|prepared| {
                commit_prepared_whole_blob_snapshot_locked(&mut inner, runtime_id, prepared)
            })
            .transpose()?;
        let outbox = inner
            .compaction_projection_outbox
            .entry(rid.clone())
            .or_default();
        for intent in compaction_intents {
            outbox
                .entry(intent.projection.clone())
                .or_insert(CompactionOutboxEntry {
                    intent,
                    finalized: false,
                });
        }
        inner.receipts.insert(key, receipt);
        apply_prepared_memory_input_state_mutations(&mut inner, &rid, prepared_input_mutations);
        Ok(authority)
    }

    async fn atomic_apply_prepared_whole_blob_with_machine_lifecycle(
        &self,
        runtime_id: &LogicalRuntimeId,
        prepared: PreparedWholeBlobSnapshot,
        receipt: RunBoundaryReceipt,
        machine_lifecycle: MachineLifecycleCommit,
        input_updates: Vec<InputStatePersistenceRecord>,
        session_store_key: meerkat_core::types::SessionId,
    ) -> Result<WholeBlobStoreAuthority, RuntimeStoreError> {
        if prepared.session().id() != &session_store_key {
            return Err(RuntimeStoreError::SessionKeyMismatch {
                expected: session_store_key,
                actual: prepared.session().id().clone(),
            });
        }
        let rid = runtime_id.0.clone();
        let compaction_intents =
            super::validated_compaction_projection_intents(prepared.session())?;
        let machine_lifecycle_record = machine_lifecycle.store_record().encode()?;
        let lifecycle_expected = machine_lifecycle.expected_version().cloned();
        let input_updates = input_updates
            .into_iter()
            .map(InputStatePersistenceRecord::into_stored_and_expected)
            .collect::<Vec<_>>();
        let key = ReceiptKey {
            runtime_id: rid.clone(),
            run_id: receipt.run_id.clone(),
            sequence: receipt.sequence,
        };

        let mut inner = self.inner.lock().await;
        if let Some(existing) = inner.compaction_projection_outbox.get(&rid) {
            for intent in &compaction_intents {
                if let Some(entry) = existing.get(&intent.projection) {
                    if entry.finalized {
                        return Err(RuntimeStoreError::WriteFailed(format!(
                            "atomic session snapshot replays finalized compaction intent {}",
                            intent.projection.revision()
                        )));
                    }
                    if entry.intent != *intent {
                        return Err(RuntimeStoreError::WriteFailed(format!(
                            "conflicting compaction outbox intent for rewrite {}",
                            intent.projection.revision()
                        )));
                    }
                }
            }
        }
        if inner.receipts.contains_key(&key) {
            return Err(RuntimeStoreError::WriteFailed(format!(
                "boundary receipt already exists for runtime '{}' run {} sequence {}",
                runtime_id, receipt.run_id, receipt.sequence
            )));
        }
        precheck_fenced_input_updates(inner.input_states.get(&rid), &input_updates)?;
        let prepared_input_mutations = prepare_memory_input_state_mutations(
            &inner,
            &rid,
            input_updates
                .into_iter()
                .map(|(bundle, _expected)| MemoryInputStateMutation::Upsert(bundle))
                .collect(),
        )?;
        if let Some(expected) = &lifecycle_expected {
            let existing = inner.runtime_lifecycle.get(&rid);
            let matches = match expected {
                MachineLifecycleExpectedVersion::Missing => existing.is_none(),
                MachineLifecycleExpectedVersion::Version(version) => {
                    existing.is_some_and(|bytes| {
                        super::MachineLifecycleObservationVersion::from_raw_record(bytes)
                            == *version
                    })
                }
            };
            if !matches {
                return Err(RuntimeStoreError::MachineLifecycleVersionConflict { runtime_id: rid });
            }
        }

        let authority =
            commit_prepared_whole_blob_snapshot_locked(&mut inner, runtime_id, prepared)?;
        let outbox = inner
            .compaction_projection_outbox
            .entry(rid.clone())
            .or_default();
        for intent in compaction_intents {
            outbox
                .entry(intent.projection.clone())
                .or_insert(CompactionOutboxEntry {
                    intent,
                    finalized: false,
                });
        }
        inner
            .runtime_lifecycle
            .insert(rid.clone(), machine_lifecycle_record);
        inner.receipts.insert(key, receipt);
        apply_prepared_memory_input_state_mutations(&mut inner, &rid, prepared_input_mutations);
        Ok(authority)
    }

    async fn atomic_promote_whole_blob(
        &self,
        runtime_id: &LogicalRuntimeId,
        promotion: super::PreparedWholeBlobProvisionalPromotion,
        receipt: RunBoundaryReceipt,
        input_updates: Vec<InputStatePersistenceRecord>,
        session_store_key: meerkat_core::types::SessionId,
    ) -> Result<WholeBlobStoreAuthority, RuntimeStoreError> {
        let (authority, checkpoint_conversation_digest, checkpoint_message_count) =
            promotion.into_parts();
        if authority.session_id() != &session_store_key {
            return Err(RuntimeStoreError::SessionKeyMismatch {
                expected: authority.session_id().clone(),
                actual: session_store_key,
            });
        }
        let rid = runtime_id.0.clone();
        let key = ReceiptKey {
            runtime_id: rid.clone(),
            run_id: receipt.run_id.clone(),
            sequence: receipt.sequence,
        };
        let input_updates = input_updates
            .into_iter()
            .map(InputStatePersistenceRecord::into_stored_and_expected)
            .collect::<Vec<_>>();
        let mut inner = self.inner.lock().await;
        if inner.receipts.contains_key(&key) {
            return Err(RuntimeStoreError::WriteFailed(format!(
                "boundary receipt already exists for runtime '{}' run {} sequence {}",
                runtime_id, receipt.run_id, receipt.sequence
            )));
        }
        precheck_fenced_input_updates(inner.input_states.get(&rid), &input_updates)?;
        let prepared_input_mutations = prepare_memory_input_state_mutations(
            &inner,
            &rid,
            input_updates
                .into_iter()
                .map(|(bundle, _expected)| MemoryInputStateMutation::Upsert(bundle))
                .collect(),
        )?;
        let committed = promote_whole_blob_provisional_locked(
            &mut inner,
            runtime_id,
            &authority,
            &receipt,
            &checkpoint_conversation_digest,
            checkpoint_message_count,
            None,
        )?;
        inner.receipts.insert(key, receipt);
        apply_prepared_memory_input_state_mutations(&mut inner, &rid, prepared_input_mutations);
        Ok(committed)
    }

    async fn atomic_promote_whole_blob_with_machine_lifecycle(
        &self,
        runtime_id: &LogicalRuntimeId,
        promotion: super::PreparedWholeBlobProvisionalPromotion,
        receipt: RunBoundaryReceipt,
        machine_lifecycle: MachineLifecycleCommit,
        input_updates: Vec<InputStatePersistenceRecord>,
        session_store_key: meerkat_core::types::SessionId,
    ) -> Result<WholeBlobStoreAuthority, RuntimeStoreError> {
        let (authority, checkpoint_conversation_digest, checkpoint_message_count) =
            promotion.into_parts();
        if authority.session_id() != &session_store_key {
            return Err(RuntimeStoreError::SessionKeyMismatch {
                expected: authority.session_id().clone(),
                actual: session_store_key,
            });
        }
        let rid = runtime_id.0.clone();
        let key = ReceiptKey {
            runtime_id: rid.clone(),
            run_id: receipt.run_id.clone(),
            sequence: receipt.sequence,
        };
        let machine_lifecycle_record = machine_lifecycle.store_record().encode()?;
        let lifecycle_expected = machine_lifecycle.expected_version().cloned();
        let runtime_state = machine_lifecycle.runtime_state();
        let input_updates = input_updates
            .into_iter()
            .map(InputStatePersistenceRecord::into_stored_and_expected)
            .collect::<Vec<_>>();
        let mut inner = self.inner.lock().await;
        if inner.receipts.contains_key(&key) {
            return Err(RuntimeStoreError::WriteFailed(format!(
                "boundary receipt already exists for runtime '{}' run {} sequence {}",
                runtime_id, receipt.run_id, receipt.sequence
            )));
        }
        precheck_fenced_input_updates(inner.input_states.get(&rid), &input_updates)?;
        let prepared_input_mutations = prepare_memory_input_state_mutations(
            &inner,
            &rid,
            input_updates
                .into_iter()
                .map(|(bundle, _expected)| MemoryInputStateMutation::Upsert(bundle))
                .collect(),
        )?;
        if let Some(expected) = &lifecycle_expected {
            let existing = inner.runtime_lifecycle.get(&rid);
            let matches = match expected {
                MachineLifecycleExpectedVersion::Missing => existing.is_none(),
                MachineLifecycleExpectedVersion::Version(version) => {
                    existing.is_some_and(|bytes| {
                        super::MachineLifecycleObservationVersion::from_raw_record(bytes)
                            == *version
                    })
                }
            };
            if !matches {
                return Err(RuntimeStoreError::MachineLifecycleVersionConflict { runtime_id: rid });
            }
        }
        let committed = promote_whole_blob_provisional_locked(
            &mut inner,
            runtime_id,
            &authority,
            &receipt,
            &checkpoint_conversation_digest,
            checkpoint_message_count,
            Some(runtime_state),
        )?;
        inner
            .runtime_lifecycle
            .insert(rid.clone(), machine_lifecycle_record);
        inner.receipts.insert(key, receipt);
        apply_prepared_memory_input_state_mutations(&mut inner, &rid, prepared_input_mutations);
        Ok(committed)
    }

    // RuntimeStore's sealed recovery verb intentionally carries each fenced
    // boundary component as a separate typed argument.
    #[allow(clippy::too_many_arguments)]
    async fn atomic_recover_whole_blob(
        &self,
        runtime_id: &LogicalRuntimeId,
        promotion: super::PreparedWholeBlobRecoveryPromotion,
        evidence: super::PreparedRecoveryEvidence,
        receipt: RunBoundaryReceipt,
        machine_lifecycle: MachineLifecycleCommit,
        input_updates: Vec<InputStatePersistenceRecord>,
        session_store_key: meerkat_core::types::SessionId,
    ) -> Result<PreparedRuntimeSessionCommitResult, RuntimeStoreError> {
        let (expected, repaired_snapshot) = promotion.into_parts();
        if expected.session_id() != &session_store_key {
            return Err(RuntimeStoreError::SessionKeyMismatch {
                expected: expected.session_id().clone(),
                actual: session_store_key,
            });
        }
        evidence.verify_input_updates(&input_updates)?;
        evidence.verify_request_effects(&receipt, &machine_lifecycle)?;
        let recovery = super::CommittedRecoveryBoundary::from_prepared(&evidence, &receipt);
        let receipt_key = ReceiptKey {
            runtime_id: runtime_id.0.clone(),
            run_id: receipt.run_id.clone(),
            sequence: receipt.sequence,
        };
        let lifecycle_target = machine_lifecycle.store_record().encode()?;
        let lifecycle_expected =
            machine_lifecycle
                .expected_version()
                .cloned()
                .ok_or_else(|| RuntimeStoreError::SessionPersistenceAuthorityConflict {
                    runtime_id: runtime_id.to_string(),
                    detail: "WholeBlob recovery lifecycle has no exact predecessor fence"
                        .to_string(),
                })?;
        let runtime_state = machine_lifecycle.runtime_state();
        let input_updates = input_updates
            .into_iter()
            .map(InputStatePersistenceRecord::into_stored_and_expected)
            .collect::<Vec<_>>();
        let mut inner = self.inner.lock().await;

        let recovery_key = (runtime_id.0.clone(), evidence.candidate_id().to_string());
        if let Some(stored) = inner.recovery_boundaries.get(&recovery_key) {
            if stored != &recovery {
                return Err(RuntimeStoreError::SessionPersistenceAuthorityConflict {
                    runtime_id: runtime_id.to_string(),
                    detail: "a divergent recovery boundary already exists for this candidate"
                        .to_string(),
                });
            }
            let (_, _, _, _, recovered_blob_sha256) = evidence
                .whole_blob_authority_transition()
                .ok_or_else(|| RuntimeStoreError::SessionPersistenceAuthorityConflict {
                    runtime_id: runtime_id.to_string(),
                    detail: "committed WholeBlob recovery lost its authority transition"
                        .to_string(),
                })?;
            let expected_current = WholeBlobStoreAuthority::issued(
                evidence.session_id().clone(),
                expected
                    .base_store_revision()
                    .checked_add(1)
                    .ok_or_else(|| RuntimeStoreError::SessionPersistenceAuthorityConflict {
                        runtime_id: runtime_id.to_string(),
                        detail: "committed WholeBlob recovery revision overflow".to_string(),
                    })?,
                recovered_blob_sha256.to_string(),
            )?;
            if inner.session_authorities.get(&runtime_id.0) != Some(&expected_current)
                || inner
                    .whole_blob_provisional_tails
                    .contains_key(&runtime_id.0)
                || inner.runtime_lifecycle.get(&runtime_id.0) != Some(&lifecycle_target)
                || inner.receipts.get(&receipt_key) != Some(&receipt)
            {
                return Err(RuntimeStoreError::SessionPersistenceAuthorityConflict {
                    runtime_id: runtime_id.to_string(),
                    detail: "committed WholeBlob recovery effects were superseded".to_string(),
                });
            }
            for (target, _) in &input_updates {
                let current = inner
                    .input_states
                    .get(&runtime_id.0)
                    .and_then(|states| states.get(&target.state.input_id));
                let current_digest = current.map(memory_input_row_version_digest).transpose()?;
                let target_digest = memory_input_row_version_digest(target)?;
                if current_digest.as_deref() != Some(target_digest.as_str()) {
                    return Err(RuntimeStoreError::SessionPersistenceAuthorityConflict {
                        runtime_id: runtime_id.to_string(),
                        detail: format!(
                            "committed recovery input {} was superseded",
                            target.state.input_id
                        ),
                    });
                }
            }
            for enrichment in evidence.receipt_digest_enrichments() {
                let enriched = enrichment.enriched_receipt();
                let key = ReceiptKey {
                    runtime_id: runtime_id.0.clone(),
                    run_id: enriched.run_id.clone(),
                    sequence: enriched.sequence,
                };
                if inner.receipts.get(&key) != Some(&enriched) {
                    return Err(RuntimeStoreError::SessionPersistenceAuthorityConflict {
                        runtime_id: runtime_id.to_string(),
                        detail: format!(
                            "committed recovery receipt enrichment {}:{} was superseded",
                            enriched.run_id, enriched.sequence
                        ),
                    });
                }
            }
            return Ok(PreparedRuntimeSessionCommitResult::recovery(
                super::RuntimeSessionAuthority::WholeBlob(expected_current),
                super::RecoveryCommitStatus::AlreadyCommittedExact,
            ));
        }

        let input_snapshot = prepared_memory_recovery_input_snapshot(&inner, runtime_id)?;
        if input_snapshot.input_set_revision()
            != evidence.predecessor_nonterminal_input_set_revision()
            || input_snapshot.exact_set_token()
                != evidence.predecessor_nonterminal_input_set_token()
        {
            return Err(RuntimeStoreError::RecoveryInputSetConflict {
                runtime_id: runtime_id.to_string(),
            });
        }
        let current = inner
            .session_authorities
            .get(&runtime_id.0)
            .ok_or_else(|| RuntimeStoreError::SessionPersistenceAuthorityConflict {
                runtime_id: runtime_id.to_string(),
                detail: "WholeBlob recovery has no committed base authority".to_string(),
            })?;
        let stored = inner
            .whole_blob_provisional_tails
            .get(&runtime_id.0)
            .ok_or_else(|| RuntimeStoreError::SessionPersistenceAuthorityConflict {
                runtime_id: runtime_id.to_string(),
                detail: "WholeBlob recovery has no provisional candidate".to_string(),
            })?;
        if current.session_id() != expected.session_id()
            || current.store_revision() != expected.base_store_revision()
            || current.blob_sha256() != expected.base_blob_sha256()
            || stored.authority != expected
            || expected.run_id() != &receipt.run_id
        {
            return Err(RuntimeStoreError::SessionPersistenceAuthorityConflict {
                runtime_id: runtime_id.to_string(),
                detail: "WholeBlob recovery does not exactly match stored base/run/candidate"
                    .to_string(),
            });
        }
        let (_, _, candidate_blob_sha256, _, recovered_blob_sha256) = evidence
            .whole_blob_authority_transition()
            .ok_or_else(|| RuntimeStoreError::SessionPersistenceAuthorityConflict {
                runtime_id: runtime_id.to_string(),
                detail: "WholeBlob recovery evidence has no WholeBlob transition".to_string(),
            })?;
        if candidate_blob_sha256 != expected.candidate_blob_sha256() {
            return Err(RuntimeStoreError::SessionPersistenceAuthorityConflict {
                runtime_id: runtime_id.to_string(),
                detail: "WholeBlob recovery candidate digest changed after classification"
                    .to_string(),
            });
        }
        if inner.receipts.contains_key(&receipt_key) {
            return Err(RuntimeStoreError::SessionPersistenceAuthorityConflict {
                runtime_id: runtime_id.to_string(),
                detail: "WholeBlob recovery receipt identity already exists".to_string(),
            });
        }
        let lifecycle_matches = match &lifecycle_expected {
            MachineLifecycleExpectedVersion::Missing => {
                !inner.runtime_lifecycle.contains_key(&runtime_id.0)
            }
            MachineLifecycleExpectedVersion::Version(version) => inner
                .runtime_lifecycle
                .get(&runtime_id.0)
                .is_some_and(|bytes| {
                    super::MachineLifecycleObservationVersion::from_raw_record(bytes) == *version
                }),
        };
        if !lifecycle_matches {
            return Err(RuntimeStoreError::MachineLifecycleVersionConflict {
                runtime_id: runtime_id.to_string(),
            });
        }
        precheck_fenced_input_updates(inner.input_states.get(&runtime_id.0), &input_updates)?;
        let prepared_input_mutations = prepare_memory_input_state_mutations(
            &inner,
            &runtime_id.0,
            input_updates
                .iter()
                .map(|(target, _)| MemoryInputStateMutation::Upsert(target.clone()))
                .collect(),
        )?;
        let mut enriched_receipts = Vec::new();
        for enrichment in evidence.receipt_digest_enrichments() {
            let original = enrichment.original_receipt();
            let key = ReceiptKey {
                runtime_id: runtime_id.0.clone(),
                run_id: original.run_id.clone(),
                sequence: original.sequence,
            };
            let current = inner.receipts.get(&key).ok_or_else(|| {
                RuntimeStoreError::SessionPersistenceAuthorityConflict {
                    runtime_id: runtime_id.to_string(),
                    detail: format!(
                        "recovery receipt enrichment source {}:{} is absent",
                        original.run_id, original.sequence
                    ),
                }
            })?;
            let current_bytes = serde_json::to_vec(current)
                .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
            let source = super::PreparedRecoveryReceiptSource::from_serialized_row(&current_bytes)?;
            if source.receipt() != original
                || source.exact_row_token() != enrichment.original_exact_row_token()
            {
                return Err(RuntimeStoreError::SessionPersistenceAuthorityConflict {
                    runtime_id: runtime_id.to_string(),
                    detail: format!(
                        "recovery receipt enrichment source {}:{} changed after classification",
                        original.run_id, original.sequence
                    ),
                });
            }
            enriched_receipts.push((key, enrichment.enriched_receipt()));
        }

        let (promoted_bytes, mut catalog_entry, compaction_intents) =
            if let Some(repaired_snapshot) = repaired_snapshot {
                let (session, serialized, blob_sha256) = repaired_snapshot.into_parts();
                if session.id() != expected.session_id() || blob_sha256 != recovered_blob_sha256 {
                    return Err(RuntimeStoreError::SessionPersistenceAuthorityConflict {
                        runtime_id: runtime_id.to_string(),
                        detail: "WholeBlob repaired body differs from recovery evidence"
                            .to_string(),
                    });
                }
                let catalog_entry = super::RuntimeSessionCatalogEntry::from_session(
                    session.as_ref(),
                    RuntimeSessionPersistenceProfile::WholeBlobV1,
                    Some(runtime_state),
                )?;
                let intents = super::validated_compaction_projection_intents(session.as_ref())?;
                (serialized.session_snapshot, catalog_entry, intents)
            } else {
                if recovered_blob_sha256 != expected.candidate_blob_sha256()
                    || receipt.conversation_digest.as_deref()
                        != Some(stored.conversation_digest.as_str())
                    || u64::try_from(receipt.message_count).ok() != Some(stored.message_count)
                {
                    return Err(RuntimeStoreError::SessionPersistenceAuthorityConflict {
                        runtime_id: runtime_id.to_string(),
                        detail:
                            "completed WholeBlob recovery does not bind the stored candidate facts"
                                .to_string(),
                    });
                }
                (
                    Arc::clone(&stored.candidate_bytes),
                    stored.catalog_entry.clone(),
                    stored.compaction_projection_intents.clone(),
                )
            };
        if let Some(existing) = inner.compaction_projection_outbox.get(&runtime_id.0) {
            for intent in &compaction_intents {
                if let Some(entry) = existing.get(&intent.projection)
                    && (entry.finalized || entry.intent != *intent)
                {
                    return Err(RuntimeStoreError::WriteFailed(format!(
                        "WholeBlob recovery conflicts with compaction rewrite {}",
                        intent.projection.revision()
                    )));
                }
            }
        }
        let next = WholeBlobStoreAuthority::issued(
            expected.session_id().clone(),
            current.store_revision().checked_add(1).ok_or_else(|| {
                RuntimeStoreError::WriteFailed(format!(
                    "WholeBlob store revision exhausted for runtime {runtime_id}"
                ))
            })?,
            recovered_blob_sha256.to_string(),
        )?;
        catalog_entry.set_runtime_state(Some(runtime_state));

        inner.sessions.insert(runtime_id.0.clone(), promoted_bytes);
        inner
            .session_authorities
            .insert(runtime_id.0.clone(), next.clone());
        inner
            .session_catalog
            .insert(runtime_id.0.clone(), catalog_entry);
        let outbox = inner
            .compaction_projection_outbox
            .entry(runtime_id.0.clone())
            .or_default();
        for intent in compaction_intents {
            outbox
                .entry(intent.projection.clone())
                .or_insert(CompactionOutboxEntry {
                    intent,
                    finalized: false,
                });
        }
        inner.whole_blob_provisional_tails.remove(&runtime_id.0);
        inner.projection_quarantine.remove(&runtime_id.0);
        inner
            .runtime_lifecycle
            .insert(runtime_id.0.clone(), lifecycle_target);
        for (key, enriched) in enriched_receipts {
            inner.receipts.insert(key, enriched);
        }
        inner.receipts.insert(receipt_key, receipt);
        apply_prepared_memory_input_state_mutations(
            &mut inner,
            &runtime_id.0,
            prepared_input_mutations,
        );
        inner.recovery_boundaries.insert(recovery_key, recovery);
        Ok(PreparedRuntimeSessionCommitResult::recovery(
            super::RuntimeSessionAuthority::WholeBlob(next),
            super::RecoveryCommitStatus::Committed,
        ))
    }
}

impl Default for InMemoryRuntimeStore {
    fn default() -> Self {
        Self::new()
    }
}

fn issue_whole_blob_store_authority(
    current: Option<&WholeBlobStoreAuthority>,
    session_id: &meerkat_core::types::SessionId,
    blob_sha256: &str,
) -> Result<WholeBlobStoreAuthority, RuntimeStoreError> {
    if let Some(current) = current
        && current.session_id() == session_id
        && current.blob_sha256() == blob_sha256
    {
        return Ok(current.clone());
    }
    let next_revision = current
        .map(WholeBlobStoreAuthority::store_revision)
        .unwrap_or(0)
        .checked_add(1)
        .ok_or_else(|| {
            RuntimeStoreError::WriteFailed(format!(
                "WholeBlob store revision exhausted for session {session_id}"
            ))
        })?;
    WholeBlobStoreAuthority::issued(session_id.clone(), next_revision, blob_sha256.to_string())
}

fn whole_blob_body_sha256(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    format!("row-sha256:{:x}", sha2::Sha256::digest(bytes))
}

fn promote_whole_blob_provisional_locked(
    inner: &mut Inner,
    runtime_id: &LogicalRuntimeId,
    expected: &WholeBlobProvisionalTailAuthority,
    receipt: &RunBoundaryReceipt,
    checkpoint_conversation_digest: &str,
    checkpoint_message_count: u64,
    runtime_state: Option<crate::runtime_state::RuntimeState>,
) -> Result<WholeBlobStoreAuthority, RuntimeStoreError> {
    let stored = inner
        .whole_blob_provisional_tails
        .get(&runtime_id.0)
        .ok_or_else(|| RuntimeStoreError::SessionPersistenceAuthorityConflict {
            runtime_id: runtime_id.to_string(),
            detail: "WholeBlob provisional promotion candidate is absent".to_string(),
        })?;
    let current = inner
        .session_authorities
        .get(&runtime_id.0)
        .ok_or_else(|| RuntimeStoreError::SessionPersistenceAuthorityConflict {
            runtime_id: runtime_id.to_string(),
            detail: "WholeBlob provisional promotion has no committed base".to_string(),
        })?;
    if &stored.authority != expected
        || expected.run_id() != &receipt.run_id
        || current.session_id() != expected.session_id()
        || current.store_revision() != expected.base_store_revision()
        || current.blob_sha256() != expected.base_blob_sha256()
    {
        return Err(RuntimeStoreError::SessionPersistenceAuthorityConflict {
            runtime_id: runtime_id.to_string(),
            detail:
                "WholeBlob provisional promotion does not exactly match stored base/run/candidate"
                    .to_string(),
        });
    }
    if stored.conversation_digest != checkpoint_conversation_digest
        || stored.message_count != checkpoint_message_count
        || receipt.conversation_digest.as_deref() != Some(stored.conversation_digest.as_str())
        || u64::try_from(receipt.message_count).ok() != Some(stored.message_count)
    {
        return Err(RuntimeStoreError::SessionPersistenceAuthorityConflict {
            runtime_id: runtime_id.to_string(),
            detail: "WholeBlob final receipt does not bind the stored checkpoint count/digest"
                .to_string(),
        });
    }
    if let Some(existing) = inner.compaction_projection_outbox.get(&runtime_id.0) {
        for intent in &stored.compaction_projection_intents {
            if let Some(entry) = existing.get(&intent.projection)
                && (entry.finalized || entry.intent != *intent)
            {
                return Err(RuntimeStoreError::WriteFailed(format!(
                    "WholeBlob provisional promotion conflicts with compaction rewrite {}",
                    intent.projection.revision()
                )));
            }
        }
    }
    let next = WholeBlobStoreAuthority::issued(
        expected.session_id().clone(),
        current.store_revision().checked_add(1).ok_or_else(|| {
            RuntimeStoreError::WriteFailed(format!(
                "WholeBlob store revision exhausted for runtime {runtime_id}"
            ))
        })?,
        expected.candidate_blob_sha256().to_string(),
    )?;
    // The candidate Arc is the sole body allocation. Promotion only moves the
    // small authority pointer and reuses that exact store-owned allocation.
    let promoted_bytes = Arc::clone(&stored.candidate_bytes);
    let mut catalog_entry = stored.catalog_entry.clone();
    if let Some(runtime_state) = runtime_state {
        catalog_entry.set_runtime_state(Some(runtime_state));
    } else if let Some(existing) = inner.session_catalog.get(&runtime_id.0) {
        catalog_entry.set_runtime_state(existing.runtime_state());
    }
    let compaction_projection_intents = stored.compaction_projection_intents.clone();
    inner.sessions.insert(runtime_id.0.clone(), promoted_bytes);
    inner
        .session_authorities
        .insert(runtime_id.0.clone(), next.clone());
    inner
        .session_catalog
        .insert(runtime_id.0.clone(), catalog_entry);
    let outbox = inner
        .compaction_projection_outbox
        .entry(runtime_id.0.clone())
        .or_default();
    for intent in compaction_projection_intents {
        outbox
            .entry(intent.projection.clone())
            .or_insert(CompactionOutboxEntry {
                intent,
                finalized: false,
            });
    }
    inner.whole_blob_provisional_tails.remove(&runtime_id.0);
    inner.projection_quarantine.remove(&runtime_id.0);
    Ok(next)
}

fn commit_prepared_whole_blob_snapshot_locked(
    inner: &mut Inner,
    runtime_id: &LogicalRuntimeId,
    prepared: PreparedWholeBlobSnapshot,
) -> Result<WholeBlobStoreAuthority, RuntimeStoreError> {
    let (session, serialized, candidate_blob_sha256) = prepared.into_parts();
    if inner
        .whole_blob_provisional_tails
        .contains_key(&runtime_id.0)
    {
        return Err(RuntimeStoreError::SessionPersistenceAuthorityConflict {
            runtime_id: runtime_id.to_string(),
            detail: "ordinary WholeBlob write cannot bypass or re-encode a store-owned provisional candidate; use exact metadata-only promotion".to_string(),
        });
    }
    let next = issue_whole_blob_store_authority(
        inner.session_authorities.get(&runtime_id.0),
        session.id(),
        &candidate_blob_sha256,
    )?;
    let catalog_entry = super::RuntimeSessionCatalogEntry::from_session(
        session.as_ref(),
        super::RuntimeSessionPersistenceProfile::WholeBlobV1,
        inner
            .session_catalog
            .get(&runtime_id.0)
            .and_then(super::RuntimeSessionCatalogEntry::runtime_state),
    )?;
    inner
        .sessions
        .insert(runtime_id.0.clone(), serialized.session_snapshot);
    inner
        .session_authorities
        .insert(runtime_id.0.clone(), next.clone());
    inner
        .session_catalog
        .insert(runtime_id.0.clone(), catalog_entry);
    inner.projection_quarantine.remove(&runtime_id.0);
    Ok(next)
}

/// Exact target-local compare token for one stored input bundle. The memory
/// store's canonical row bytes are the bundle's serialized form (the same
/// encoding the SQLite backend persists), so both backends report and
/// enforce the same digests.
fn memory_input_row_version_digest(bundle: &StoredInputState) -> Result<String, RuntimeStoreError> {
    use sha2::Digest as _;
    let bytes = serde_json::to_vec(bundle)
        .map_err(|err| RuntimeStoreError::Internal(format!("input row encode failed: {err}")))?;
    Ok(format!("sha256:{:x}", sha2::Sha256::digest(&bytes)))
}

fn prepared_memory_recovery_input_snapshot(
    inner: &Inner,
    runtime_id: &LogicalRuntimeId,
) -> Result<PreparedRecoveryInputSnapshot, RuntimeStoreError> {
    let states = inner.input_states.get(&runtime_id.0);
    let revision = RecoveryInputSetRevision::from_store_generation(
        inner
            .recovery_input_set_revisions
            .get(&runtime_id.0)
            .copied()
            .unwrap_or(0),
    );
    let rows = inner
        .recovery_nonterminal_inputs
        .get(&runtime_id.0)
        .into_iter()
        .flatten()
        .map(|input_id| {
            let bundle = states
                .and_then(|states| states.get(&InputId::from_uuid(*input_id)))
                .cloned()
                .ok_or_else(|| {
                    RuntimeStoreError::ReadFailed(format!(
                        "recovery nonterminal index for runtime {runtime_id} names missing input {input_id}"
                    ))
                })?;
            let digest = memory_input_row_version_digest(&bundle)?;
            Ok((bundle, digest))
        })
        .collect::<Result<Vec<_>, _>>()?;
    PreparedRecoveryInputSnapshot::from_exact_nonterminal_rows(runtime_id.clone(), revision, rows)
}

/// Pre-validate every fenced input update against the current rows. Must run
/// BEFORE any mutation so a stale fence leaves the whole boundary untouched
/// (the SQLite backend gets this from its transaction).
fn precheck_fenced_input_updates(
    states: Option<&IndexMap<meerkat_core::lifecycle::InputId, StoredInputState>>,
    input_updates: &[(StoredInputState, Option<String>)],
) -> Result<(), RuntimeStoreError> {
    for (bundle, expected) in input_updates {
        let Some(expected) = expected else { continue };
        let current = states.and_then(|map| map.get(&bundle.state.input_id));
        let matches = match current {
            Some(current) => memory_input_row_version_digest(current)? == *expected,
            None => false,
        };
        if !matches {
            return Err(RuntimeStoreError::InputRowVersionConflict {
                input_id: bundle.state.input_id.0.to_string(),
            });
        }
    }
    Ok(())
}

/// Deserialize a persisted session-snapshot blob through typed serde, matching
/// the SQLite runtime store read path. `Session::deserialize` validates the
/// mandatory envelope version against the generated persistence version
/// authority, so a missing or non-current (v0/v1) row fails closed instead of
/// silently defaulting or upgrading on read.
fn deserialize_persisted_session(bytes: &[u8]) -> Result<meerkat_core::Session, RuntimeStoreError> {
    serde_json::from_slice(bytes).map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))
}

fn ensure_compaction_intents_already_outboxed(
    inner: &Inner,
    runtime_id: &LogicalRuntimeId,
    session: &meerkat_core::Session,
) -> Result<(), RuntimeStoreError> {
    let intents = super::validated_compaction_projection_intents(session)?;
    ensure_compaction_intents_already_outboxed_list(inner, runtime_id, &intents)
}

fn ensure_compaction_intents_already_outboxed_list(
    inner: &Inner,
    runtime_id: &LogicalRuntimeId,
    intents: &[meerkat_core::CompactionProjectionIntent],
) -> Result<(), RuntimeStoreError> {
    let existing = inner.compaction_projection_outbox.get(&runtime_id.0);
    for intent in intents {
        match existing.and_then(|entries| entries.get(&intent.projection)) {
            Some(entry) if entry.finalized => {
                return Err(RuntimeStoreError::WriteFailed(format!(
                    "non-boundary snapshot replays finalized compaction intent {}",
                    intent.projection.revision()
                )));
            }
            Some(entry) if entry.intent == *intent => {}
            Some(_) => {
                return Err(RuntimeStoreError::WriteFailed(format!(
                    "non-boundary snapshot conflicts with compaction outbox rewrite {}",
                    intent.projection.revision()
                )));
            }
            None => {
                return Err(RuntimeStoreError::WriteFailed(format!(
                    "non-boundary snapshot introduces compaction intent {} without atomic outbox authority",
                    intent.projection.revision()
                )));
            }
        }
    }
    Ok(())
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl RuntimeStore for InMemoryRuntimeStore {
    fn session_persistence_profile(&self) -> super::RuntimeSessionPersistenceProfile {
        super::RuntimeSessionPersistenceProfile::WholeBlobV1
    }

    fn session_boundary_authority_read_cost(&self) -> RuntimeSessionAuthorityReadCost {
        RuntimeSessionAuthorityReadCost::Bounded
    }

    async fn commit_prepared_session_boundary(
        &self,
        runtime_id: &LogicalRuntimeId,
        request: super::PreparedRuntimeSessionCommit,
    ) -> Result<super::PreparedRuntimeSessionCommitResult, RuntimeStoreError> {
        use super::{
            PreparedRuntimeSessionCommitPayload, PreparedRuntimeSessionCommitResult,
            RuntimeSessionAuthority, RuntimeSessionPersistenceProfile,
        };

        let authority = match request.into_payload() {
            PreparedRuntimeSessionCommitPayload::SnapshotOnly { session } => {
                let prepared = super::prepared_whole_blob_snapshot(&session)?;
                Some(
                    self.commit_session_snapshot_inner(runtime_id, prepared)
                        .await?,
                )
            }
            PreparedRuntimeSessionCommitPayload::Success {
                session,
                receipt,
                input_updates,
                session_store_key,
            } => {
                let prepared = session
                    .as_ref()
                    .map(super::prepared_whole_blob_snapshot)
                    .transpose()?;
                self.atomic_apply_prepared_whole_blob(
                    runtime_id,
                    prepared,
                    receipt,
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
                self.atomic_promote_whole_blob(
                    runtime_id,
                    promotion,
                    receipt,
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
            } => Some(
                self.atomic_apply_prepared_whole_blob_with_machine_lifecycle(
                    runtime_id,
                    super::prepared_whole_blob_snapshot(&session)?,
                    receipt,
                    machine_lifecycle,
                    Vec::new(),
                    session_store_key,
                )
                .await?,
            ),
            PreparedRuntimeSessionCommitPayload::PromoteWholeBlobServiceTurnTerminal {
                promotion,
                receipt,
                machine_lifecycle,
                session_store_key,
            } => Some(
                self.atomic_promote_whole_blob_with_machine_lifecycle(
                    runtime_id,
                    promotion,
                    receipt,
                    machine_lifecycle,
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
            } => Some(
                self.atomic_apply_prepared_whole_blob_with_machine_lifecycle(
                    runtime_id,
                    super::prepared_whole_blob_snapshot(&session)?,
                    receipt,
                    machine_lifecycle,
                    input_updates,
                    session_store_key,
                )
                .await?,
            ),
            PreparedRuntimeSessionCommitPayload::PromoteWholeBlobMachineTerminal {
                promotion,
                receipt,
                machine_lifecycle,
                input_updates,
                session_store_key,
            } => Some(
                self.atomic_promote_whole_blob_with_machine_lifecycle(
                    runtime_id,
                    promotion,
                    receipt,
                    machine_lifecycle,
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
                    .atomic_recover_whole_blob(
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
            | PreparedRuntimeSessionCommitPayload::PromoteHeadCanonicalMachineTerminal { .. } => {
                return Err(RuntimeStoreError::SessionPersistenceAuthorityConflict {
                    runtime_id: runtime_id.to_string(),
                    detail:
                        "HeadCanonical provisional promotion cannot commit through a WholeBlob store"
                            .to_string(),
                });
            }
            PreparedRuntimeSessionCommitPayload::Recovery { .. } => {
                return Err(
                    RuntimeStoreError::PreparedRecoveryRequiresAtomicPhysicalHeadCas {
                        profile: RuntimeSessionPersistenceProfile::WholeBlobV1,
                    },
                );
            }
        };
        Ok(match authority {
            Some(authority) => PreparedRuntimeSessionCommitResult::committed(
                RuntimeSessionAuthority::WholeBlob(authority),
            ),
            None => PreparedRuntimeSessionCommitResult::receipt_only(
                RuntimeSessionPersistenceProfile::WholeBlobV1,
            ),
        })
    }

    fn supports_compaction_projection_outbox(&self) -> bool {
        true
    }

    fn input_state_batch_cas_implementation_profile(
        &self,
    ) -> InputStateBatchCasImplementationProfile {
        InputStateBatchCasImplementationProfile::MultiWriter
    }

    async fn load_runtime_delivery_authority(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<RuntimeDeliveryAuthorityRecord>, RuntimeStoreError> {
        Ok(self
            .inner
            .lock()
            .await
            .runtime_delivery_authority
            .get(&runtime_id.0)
            .cloned())
    }

    async fn load_runtime_delivery_record(
        &self,
        runtime_id: &LogicalRuntimeId,
        delivery_id: &str,
    ) -> Result<Option<RuntimeDeliveryStoreRecord>, RuntimeStoreError> {
        Ok(self
            .inner
            .lock()
            .await
            .runtime_delivery_records
            .get(&runtime_id.0)
            .and_then(|records| {
                records
                    .values()
                    .find(|record| record.delivery_id() == delivery_id)
            })
            .cloned())
    }

    async fn compare_and_swap_runtime_delivery_authority(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected_revision: Option<u64>,
        replacement: RuntimeDeliveryAuthorityRecord,
        inserted_delivery: Option<RuntimeDeliveryStoreRecord>,
    ) -> Result<RuntimeDeliveryAuthorityCasOutcome, RuntimeStoreError> {
        let mut inner = self.inner.lock().await;
        let current = inner.runtime_delivery_authority.get(&runtime_id.0).cloned();
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
            let records = inner
                .runtime_delivery_records
                .entry(runtime_id.0.clone())
                .or_default();
            if records.contains_key(&record.sequence())
                || records
                    .values()
                    .any(|existing| existing.delivery_id() == record.delivery_id())
            {
                return Err(RuntimeStoreError::WriteFailed(format!(
                    "runtime delivery row {} / sequence {} already exists",
                    record.delivery_id(),
                    record.sequence()
                )));
            }
        }

        inner
            .runtime_delivery_authority
            .insert(runtime_id.0.clone(), replacement.clone());
        if let Some(record) = inserted_delivery {
            inner
                .runtime_delivery_records
                .entry(runtime_id.0.clone())
                .or_default()
                .insert(record.sequence(), record);
        }
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
        Ok(self
            .inner
            .lock()
            .await
            .runtime_delivery_records
            .get(&runtime_id.0)
            .into_iter()
            .flat_map(|records| {
                records
                    .range((
                        std::ops::Bound::Excluded(after_sequence),
                        std::ops::Bound::Unbounded,
                    ))
                    .take(limit)
                    .map(|(_, record)| record.clone())
            })
            .collect())
    }

    fn persist_auth_oauth_flow_snapshot(
        &self,
        snapshot_json: &[u8],
    ) -> Result<(), RuntimeStoreError> {
        *self
            .auth_oauth_flow_snapshot
            .lock()
            .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))? =
            Some(snapshot_json.to_vec());
        Ok(())
    }

    fn load_auth_oauth_flow_snapshot(&self) -> Result<Option<Vec<u8>>, RuntimeStoreError> {
        self.auth_oauth_flow_snapshot
            .lock()
            .map(|snapshot| snapshot.clone())
            .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))
    }

    fn update_auth_oauth_flow_snapshot(
        &self,
        update: &mut AuthOAuthFlowSnapshotUpdate<'_>,
    ) -> Result<(), RuntimeStoreError> {
        let mut snapshot = self
            .auth_oauth_flow_snapshot
            .lock()
            .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
        let next = update(snapshot.as_deref())?;
        *snapshot = Some(next);
        Ok(())
    }

    async fn commit_session_snapshot(
        &self,
        runtime_id: &LogicalRuntimeId,
        session_delta: SerializedSessionSnapshot,
    ) -> Result<(), RuntimeStoreError> {
        let prepared = parsed_whole_blob_snapshot(session_delta)?;
        self.commit_session_snapshot_inner(runtime_id, prepared)
            .await
            .map(|_| ())
    }

    async fn commit_prepared_whole_blob_rewrite_boundary(
        &self,
        runtime_id: &LogicalRuntimeId,
        boundary: PreparedWholeBlobRewriteStoreParts,
    ) -> Result<WholeBlobStoreAuthority, RuntimeStoreError> {
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
            return Err(RuntimeStoreError::SessionPersistenceAuthorityConflict {
                runtime_id: runtime_id.to_string(),
                detail: "prepared WholeBlob rewrite authorities do not bind this runtime/session"
                    .to_string(),
            });
        }
        let mut inner = self.inner.lock().await;
        let current = inner
            .session_authorities
            .get(&runtime_id.0)
            .ok_or_else(|| RuntimeStoreError::SessionPersistenceAuthorityConflict {
                runtime_id: runtime_id.to_string(),
                detail: "prepared WholeBlob predecessor authority is absent".to_string(),
            })?;
        ensure_compaction_intents_already_outboxed_list(
            &inner,
            runtime_id,
            &compaction_projection_intents,
        )?;
        let successor_revision = expected.store_revision().checked_add(1).ok_or_else(|| {
            RuntimeStoreError::WriteFailed(format!(
                "WholeBlob store revision exhausted for runtime {runtime_id}"
            ))
        })?;
        let exact_idempotent_successor = current.session_id() == &successor_session_id
            && current.blob_sha256() == successor_blob_sha256
            && ((current == &expected && expected.blob_sha256() == successor_blob_sha256)
                || current.store_revision() == successor_revision);
        if exact_idempotent_successor {
            return Ok(current.clone());
        }
        if current != &expected {
            return Err(RuntimeStoreError::SessionPersistenceAuthorityConflict {
                runtime_id: runtime_id.to_string(),
                detail:
                    "prepared WholeBlob predecessor revision/token does not match current authority"
                        .to_string(),
            });
        }
        // `PreparedWholeBlobRewriteStoreParts` is non-constructible outside the
        // core preparation module: its successor token was derived from these
        // exact shared bytes once before the CAS. Re-hashing the complete
        // successor here would turn the single prepared final-document hash
        // into a second O(document) pass without adding store-local authority.
        // This lock still owns the only fact a backend must revalidate: the
        // exact current predecessor (or exact already-landed successor).
        successor_catalog_entry.set_runtime_state(
            inner
                .session_catalog
                .get(&runtime_id.0)
                .and_then(super::RuntimeSessionCatalogEntry::runtime_state),
        );
        inner.sessions.insert(runtime_id.0.clone(), successor_bytes);
        let successor = WholeBlobStoreAuthority::issued(
            successor_session_id,
            successor_revision,
            successor_blob_sha256,
        )?;
        inner
            .session_authorities
            .insert(runtime_id.0.clone(), successor.clone());
        inner
            .session_catalog
            .insert(runtime_id.0.clone(), successor_catalog_entry);
        inner.projection_quarantine.remove(&runtime_id.0);
        Ok(successor)
    }

    async fn atomic_apply(
        &self,
        runtime_id: &LogicalRuntimeId,
        session_delta: Option<SerializedSessionSnapshot>,
        receipt: RunBoundaryReceipt,
        input_updates: Vec<InputStatePersistenceRecord>,
        session_store_key: Option<meerkat_core::types::SessionId>,
    ) -> Result<(), RuntimeStoreError> {
        let prepared = session_delta.map(parsed_whole_blob_snapshot).transpose()?;
        self.atomic_apply_prepared_whole_blob(
            runtime_id,
            prepared,
            receipt,
            input_updates,
            session_store_key,
        )
        .await
        .map(|_| ())
    }

    async fn load_pending_compaction_projections(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Vec<meerkat_core::CompactionProjectionIntent>, RuntimeStoreError> {
        let inner = self.inner.lock().await;
        let mut pending = inner
            .compaction_projection_outbox
            .get(&runtime_id.0)
            .into_iter()
            .flat_map(HashMap::values)
            .filter(|entry| !entry.finalized)
            .map(|entry| entry.intent.clone())
            .collect::<Vec<_>>();
        pending.sort_by(|left, right| {
            left.projection
                .session_id()
                .to_string()
                .cmp(&right.projection.session_id().to_string())
                .then_with(|| {
                    left.projection
                        .parent_revision()
                        .cmp(right.projection.parent_revision())
                })
                .then_with(|| left.projection.revision().cmp(right.projection.revision()))
                .then_with(|| {
                    left.projection
                        .commit_fingerprint()
                        .cmp(right.projection.commit_fingerprint())
                })
        });
        Ok(pending)
    }

    async fn mark_compaction_projection_finalized(
        &self,
        runtime_id: &LogicalRuntimeId,
        projection: &meerkat_core::CompactionProjectionId,
    ) -> Result<(), RuntimeStoreError> {
        let mut inner = self.inner.lock().await;
        let outbox_exists = inner
            .compaction_projection_outbox
            .get(&runtime_id.0)
            .is_some_and(|entries| entries.contains_key(projection));
        if !outbox_exists {
            return Err(RuntimeStoreError::NotFound(format!(
                "compaction outbox rewrite {}",
                projection.revision()
            )));
        }
        let cleaned_snapshot = inner
            .sessions
            .get(&runtime_id.0)
            .map(|snapshot| {
                let mut session = deserialize_persisted_session(snapshot)?;
                complete_compaction_projection_intent(&mut session, projection)?;
                let artifact = session
                    .to_persisted_artifact()
                    .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
                let authority = issue_whole_blob_store_authority(
                    inner.session_authorities.get(&runtime_id.0),
                    session.id(),
                    artifact.row_sha256_token(),
                )?;
                Ok((artifact.bytes_arc(), authority))
            })
            .transpose()?;
        let entry = inner
            .compaction_projection_outbox
            .get_mut(&runtime_id.0)
            .and_then(|entries| entries.get_mut(projection))
            .ok_or_else(|| {
                RuntimeStoreError::NotFound(format!(
                    "compaction outbox rewrite {}",
                    projection.revision()
                ))
            })?;
        entry.finalized = true;
        if let Some((cleaned_snapshot, authority)) = cleaned_snapshot {
            inner
                .sessions
                .insert(runtime_id.0.clone(), cleaned_snapshot);
            inner
                .session_authorities
                .insert(runtime_id.0.clone(), authority);
        }
        Ok(())
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
        let prepared = parsed_whole_blob_snapshot(session_delta)?;
        self.atomic_apply_prepared_whole_blob_with_machine_lifecycle(
            runtime_id,
            prepared,
            receipt,
            machine_lifecycle,
            input_updates,
            session_store_key,
        )
        .await
        .map(|_| ())
    }

    async fn load_input_states(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Vec<InputStateRow>, RuntimeStoreError> {
        let inner = self.inner.lock().await;
        let states = inner
            .input_states
            .get(&runtime_id.0)
            .map(|m| {
                m.values()
                    .cloned()
                    .map(|state| InputStateRow::Decoded(Box::new(state)))
                    .collect()
            })
            .unwrap_or_default();
        Ok(states)
    }

    async fn load_boundary_receipt(
        &self,
        runtime_id: &LogicalRuntimeId,
        run_id: &RunId,
        sequence: u64,
    ) -> Result<Option<RunBoundaryReceipt>, RuntimeStoreError> {
        let inner = self.inner.lock().await;
        let key = ReceiptKey {
            runtime_id: runtime_id.0.clone(),
            run_id: run_id.clone(),
            sequence,
        };
        Ok(inner.receipts.get(&key).cloned())
    }

    async fn load_committed_boundary_receipts(
        &self,
        runtime_id: &LogicalRuntimeId,
        run_id: &RunId,
    ) -> Result<Vec<RunBoundaryReceipt>, RuntimeStoreError> {
        let inner = self.inner.lock().await;
        let mut receipts = inner
            .receipts
            .iter()
            .filter(|(key, _)| key.runtime_id == runtime_id.0 && key.run_id == *run_id)
            .map(|(_, receipt)| receipt.clone())
            .collect::<Vec<_>>();
        receipts.sort_by_key(|receipt| receipt.sequence);
        Ok(receipts)
    }

    async fn load_durable_tail_recovery_receipts(
        &self,
        runtime_id: &LogicalRuntimeId,
        run_id: &RunId,
    ) -> Result<Vec<super::PreparedRecoveryReceiptSource>, RuntimeStoreError> {
        let inner = self.inner.lock().await;
        let mut receipts = inner
            .receipts
            .iter()
            .filter(|(key, _)| key.runtime_id == runtime_id.0 && key.run_id == *run_id)
            .map(|(_, receipt)| {
                serde_json::to_vec(receipt)
                    .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))
                    .and_then(|bytes| {
                        super::PreparedRecoveryReceiptSource::from_serialized_row(&bytes)
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        receipts.sort_by_key(|source| source.receipt().sequence);
        Ok(receipts)
    }

    async fn load_committed_recovery_boundary(
        &self,
        runtime_id: &LogicalRuntimeId,
        candidate_id: &str,
    ) -> Result<Option<super::CommittedRecoveryBoundary>, RuntimeStoreError> {
        Ok(self
            .inner
            .lock()
            .await
            .recovery_boundaries
            .get(&(runtime_id.0.clone(), candidate_id.to_string()))
            .cloned())
    }

    async fn load_input_states_with_versions(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<PreparedRecoveryInputSnapshot, RuntimeStoreError> {
        let inner = self.inner.lock().await;
        prepared_memory_recovery_input_snapshot(&inner, runtime_id)
    }

    async fn load_session_snapshot(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<Arc<Vec<u8>>>, RuntimeStoreError> {
        let inner = self.inner.lock().await;
        Ok(inner.sessions.get(&runtime_id.0).cloned())
    }

    async fn load_whole_blob_store_authority(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<WholeBlobStoreAuthority>, RuntimeStoreError> {
        Ok(self
            .inner
            .lock()
            .await
            .session_authorities
            .get(&runtime_id.0)
            .cloned())
    }

    async fn load_session_boundary_authority(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<RuntimeSessionAuthority>, RuntimeStoreError> {
        self.load_whole_blob_store_authority(runtime_id)
            .await
            .map(|authority| authority.map(RuntimeSessionAuthority::WholeBlob))
    }

    async fn delete_runtime_session_catalog_entry(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<(), RuntimeStoreError> {
        self.inner
            .lock()
            .await
            .session_catalog
            .remove(&runtime_id.0);
        Ok(())
    }

    async fn load_runtime_session_catalog_entry(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<super::RuntimeSessionCatalogEntry>, RuntimeStoreError> {
        Ok(self
            .inner
            .lock()
            .await
            .session_catalog
            .get(&runtime_id.0)
            .cloned())
    }

    async fn list_runtime_session_catalog_entries(
        &self,
        filter: meerkat_core::SessionFilter,
    ) -> Result<Vec<super::RuntimeSessionCatalogEntry>, RuntimeStoreError> {
        let inner = self.inner.lock().await;
        let mut entries = inner
            .session_catalog
            .values()
            .filter(|entry| {
                filter
                    .created_after
                    .is_none_or(|after| entry.created_at() >= after)
                    && filter
                        .updated_after
                        .is_none_or(|after| entry.updated_at() >= after)
            })
            .cloned()
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| {
            right.updated_at().cmp(&left.updated_at()).then_with(|| {
                left.session_id()
                    .to_string()
                    .cmp(&right.session_id().to_string())
            })
        });
        let offset = filter.offset.unwrap_or(0).min(entries.len());
        let limit = filter.limit.unwrap_or(usize::MAX);
        Ok(entries.into_iter().skip(offset).take(limit).collect())
    }

    async fn load_committed_whole_blob_snapshot(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<CommittedWholeBlobSnapshot>, RuntimeStoreError> {
        let inner = self.inner.lock().await;
        match (
            inner.sessions.get(&runtime_id.0),
            inner.session_authorities.get(&runtime_id.0),
        ) {
            (None, None) => Ok(None),
            (Some(bytes), Some(authority)) => Ok(Some(CommittedWholeBlobSnapshot::new(
                Arc::clone(bytes),
                authority.clone(),
            )?)),
            _ => Err(RuntimeStoreError::SessionPersistenceAuthorityConflict {
                runtime_id: runtime_id.to_string(),
                detail: "WholeBlob body and store authority ledger disagree on row presence"
                    .to_string(),
            }),
        }
    }

    async fn commit_prepared_whole_blob_snapshot_cas(
        &self,
        runtime_id: &LogicalRuntimeId,
        prepared: PreparedWholeBlobSnapshotCas,
    ) -> Result<WholeBlobSnapshotCasOutcome, RuntimeStoreError> {
        let (expected, candidate_session, candidate_bytes, candidate_blob_sha256) =
            prepared.into_parts();
        if &LogicalRuntimeId::for_session(candidate_session.id()) != runtime_id
            || candidate_session.id() != expected.session_id()
        {
            return Err(RuntimeStoreError::SessionPersistenceAuthorityConflict {
                runtime_id: runtime_id.to_string(),
                detail: "prepared WholeBlob snapshot CAS does not bind this runtime/session"
                    .to_string(),
            });
        }
        let compaction_projection_intents =
            super::validated_compaction_projection_intents(candidate_session.as_ref())?;
        let mut inner = self.inner.lock().await;
        let Some(current) = inner.session_authorities.get(&runtime_id.0) else {
            return Ok(WholeBlobSnapshotCasOutcome::Conflict);
        };
        if current != &expected {
            return Ok(WholeBlobSnapshotCasOutcome::Conflict);
        }
        if current.blob_sha256() == candidate_blob_sha256 {
            return Ok(WholeBlobSnapshotCasOutcome::Committed(current.clone()));
        }
        if inner
            .whole_blob_provisional_tails
            .contains_key(&runtime_id.0)
        {
            return Err(RuntimeStoreError::SessionPersistenceAuthorityConflict {
                runtime_id: runtime_id.to_string(),
                detail: "snapshot CAS cannot bypass a store-owned WholeBlob provisional candidate"
                    .to_string(),
            });
        }
        ensure_compaction_intents_already_outboxed_list(
            &inner,
            runtime_id,
            &compaction_projection_intents,
        )?;
        let runtime_state = inner
            .session_catalog
            .get(&runtime_id.0)
            .and_then(super::RuntimeSessionCatalogEntry::runtime_state);
        let catalog_entry = super::RuntimeSessionCatalogEntry::from_session(
            candidate_session.as_ref(),
            super::RuntimeSessionPersistenceProfile::WholeBlobV1,
            runtime_state,
        )?;
        let authority = issue_whole_blob_store_authority(
            Some(&expected),
            candidate_session.id(),
            &candidate_blob_sha256,
        )?;
        inner.sessions.insert(runtime_id.0.clone(), candidate_bytes);
        inner
            .session_authorities
            .insert(runtime_id.0.clone(), authority.clone());
        inner
            .session_catalog
            .insert(runtime_id.0.clone(), catalog_entry);
        inner.projection_quarantine.remove(&runtime_id.0);
        Ok(WholeBlobSnapshotCasOutcome::Committed(authority))
    }

    async fn write_prepared_whole_blob_provisional_tail(
        &self,
        runtime_id: &LogicalRuntimeId,
        prepared: PreparedWholeBlobProvisionalTail,
    ) -> Result<WholeBlobProvisionalTailAuthority, RuntimeStoreError> {
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
                != super::RuntimeSessionPersistenceProfile::WholeBlobV1
            || u64::try_from(catalog_entry.message_count()).ok() != Some(message_count)
            || candidate_artifact.row_sha256_token() != authority.candidate_blob_sha256()
        {
            return Err(RuntimeStoreError::SessionPersistenceAuthorityConflict {
                runtime_id: runtime_id.to_string(),
                detail: "WholeBlob provisional artifact/catalog does not bind this runtime/session authority"
                    .to_string(),
            });
        }
        let mut inner = self.inner.lock().await;
        let current = inner
            .session_authorities
            .get(&runtime_id.0)
            .ok_or_else(|| RuntimeStoreError::SessionPersistenceAuthorityConflict {
                runtime_id: runtime_id.to_string(),
                detail: "WholeBlob provisional candidate has no committed base".to_string(),
            })?;
        if current.session_id() != authority.session_id()
            || current.store_revision() != authority.base_store_revision()
            || current.blob_sha256() != authority.base_blob_sha256()
        {
            return Err(RuntimeStoreError::SessionPersistenceAuthorityConflict {
                runtime_id: runtime_id.to_string(),
                detail: "WholeBlob provisional candidate base is stale".to_string(),
            });
        }
        if let Some(existing) = inner.whole_blob_provisional_tails.get(&runtime_id.0) {
            if existing.authority == authority {
                if existing.conversation_digest != conversation_digest
                    || existing.message_count != message_count
                {
                    return Err(RuntimeStoreError::SessionPersistenceAuthorityConflict {
                        runtime_id: runtime_id.to_string(),
                        detail: "WholeBlob provisional retry changes bounded candidate facts"
                            .to_string(),
                    });
                }
                return Ok(existing.authority.clone());
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
                || existing.authority.base_store_revision() != authority.base_store_revision()
                || existing.authority.base_blob_sha256() != authority.base_blob_sha256()
                || existing.authority.run_id() != authority.run_id()
                || authority.candidate_sequence() != required_sequence
            {
                return Err(RuntimeStoreError::SessionPersistenceAuthorityConflict {
                    runtime_id: runtime_id.to_string(),
                    detail: "WholeBlob provisional replacement is stale or skips sequence"
                        .to_string(),
                });
            }
        } else if authority.candidate_sequence() != 1 {
            return Err(RuntimeStoreError::SessionPersistenceAuthorityConflict {
                runtime_id: runtime_id.to_string(),
                detail: "first WholeBlob provisional candidate sequence must be one".to_string(),
            });
        }
        inner.whole_blob_provisional_tails.insert(
            runtime_id.0.clone(),
            StoredWholeBlobProvisionalTail {
                authority: authority.clone(),
                candidate_bytes,
                conversation_digest,
                message_count,
                catalog_entry,
                compaction_projection_intents,
            },
        );
        Ok(authority)
    }

    async fn load_whole_blob_provisional_tail(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<CommittedWholeBlobProvisionalTail>, RuntimeStoreError> {
        let inner = self.inner.lock().await;
        let Some(stored) = inner.whole_blob_provisional_tails.get(&runtime_id.0) else {
            return Ok(None);
        };
        if whole_blob_body_sha256(stored.candidate_bytes.as_ref())
            != stored.authority.candidate_blob_sha256()
        {
            return Err(RuntimeStoreError::SessionPersistenceAuthorityConflict {
                runtime_id: runtime_id.to_string(),
                detail: "WholeBlob provisional body digest differs from store authority"
                    .to_string(),
            });
        }
        Ok(Some(CommittedWholeBlobProvisionalTail::new(
            stored.authority.clone(),
            Arc::clone(&stored.candidate_bytes),
        )))
    }

    async fn discard_whole_blob_provisional_tail(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected: &WholeBlobProvisionalTailAuthority,
    ) -> Result<bool, RuntimeStoreError> {
        let mut inner = self.inner.lock().await;
        if inner
            .whole_blob_provisional_tails
            .get(&runtime_id.0)
            .is_some_and(|stored| &stored.authority == expected)
        {
            inner.whole_blob_provisional_tails.remove(&runtime_id.0);
            return Ok(true);
        }
        Ok(false)
    }

    async fn clear_session_snapshot(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<(), RuntimeStoreError> {
        let mut inner = self.inner.lock().await;
        inner.sessions.remove(&runtime_id.0);
        inner.session_authorities.remove(&runtime_id.0);
        inner.session_catalog.remove(&runtime_id.0);
        inner.whole_blob_provisional_tails.remove(&runtime_id.0);
        Ok(())
    }

    async fn replace_session_snapshot_if_current(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected_current: &[u8],
        replacement: Vec<u8>,
    ) -> Result<bool, RuntimeStoreError> {
        let replacement = parsed_whole_blob_snapshot(SerializedSessionSnapshot {
            session_snapshot: Arc::new(replacement),
        })?;
        let (replacement_session, replacement, blob_sha256) = replacement.into_parts();
        let mut inner = self.inner.lock().await;
        let Some(current) = inner.sessions.get(&runtime_id.0) else {
            return Ok(false);
        };
        if current.as_ref() != expected_current {
            return Ok(false);
        }
        ensure_compaction_intents_already_outboxed(
            &inner,
            runtime_id,
            replacement_session.as_ref(),
        )?;
        let authority = issue_whole_blob_store_authority(
            inner.session_authorities.get(&runtime_id.0),
            replacement_session.id(),
            &blob_sha256,
        )?;
        inner
            .sessions
            .insert(runtime_id.0.clone(), replacement.session_snapshot);
        inner
            .session_authorities
            .insert(runtime_id.0.clone(), authority);
        inner.projection_quarantine.remove(&runtime_id.0);
        Ok(true)
    }

    async fn clear_session_snapshot_if_current(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected_current: &[u8],
    ) -> Result<bool, RuntimeStoreError> {
        let mut inner = self.inner.lock().await;
        let Some(current) = inner.sessions.get(&runtime_id.0) else {
            return Ok(false);
        };
        if current.as_ref() != expected_current {
            return Ok(false);
        }
        inner.sessions.remove(&runtime_id.0);
        inner.session_authorities.remove(&runtime_id.0);
        inner.session_catalog.remove(&runtime_id.0);
        inner.whole_blob_provisional_tails.remove(&runtime_id.0);
        // Record the in-memory quarantine marker atomically with the snapshot
        // removal, mirroring the durable SQLite path.
        inner.projection_quarantine.insert(runtime_id.0.clone());
        Ok(true)
    }

    async fn is_runtime_projection_quarantined(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<bool, RuntimeStoreError> {
        let inner = self.inner.lock().await;
        Ok(inner.projection_quarantine.contains(&runtime_id.0))
    }

    async fn persist_input_state(
        &self,
        runtime_id: &LogicalRuntimeId,
        state: &InputStatePersistenceRecord,
    ) -> Result<(), RuntimeStoreError> {
        let mut inner = self.inner.lock().await;
        let update = (
            state.clone_stored(),
            state.expected_row_digest().map(str::to_owned),
        );
        precheck_fenced_input_updates(
            inner.input_states.get(&runtime_id.0),
            std::slice::from_ref(&update),
        )?;
        let (bundle, _expected) = update;
        store_input_state(&mut inner, &runtime_id.0, bundle)
    }

    async fn persist_input_states_atomically(
        &self,
        runtime_id: &LogicalRuntimeId,
        records: &[InputStatePersistenceRecord],
    ) -> Result<(), RuntimeStoreError> {
        let mut inner = self.inner.lock().await;
        let updates = records
            .iter()
            .map(|record| {
                (
                    record.clone_stored(),
                    record.expected_row_digest().map(str::to_owned),
                )
            })
            .collect::<Vec<_>>();
        precheck_fenced_input_updates(inner.input_states.get(&runtime_id.0), &updates)?;
        let prepared = prepare_memory_input_state_mutations(
            &inner,
            &runtime_id.0,
            updates
                .into_iter()
                .map(|(bundle, _expected)| MemoryInputStateMutation::Upsert(bundle))
                .collect(),
        )?;
        apply_prepared_memory_input_state_mutations(&mut inner, &runtime_id.0, prepared);
        Ok(())
    }

    async fn compare_and_swap_input_states_atomically(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected: &[StoredInputState],
        replacements: &[InputStatePersistenceRecord],
    ) -> Result<InputStateBatchCasOutcome, RuntimeStoreError> {
        // Serialize and validate the full request before taking the mutation
        // lock, so no fallible request preparation can occur after writes.
        let prepared = prepare_input_state_batch_cas(expected, replacements)?;
        if prepared.is_empty() {
            return Ok(InputStateBatchCasOutcome::Swapped);
        }

        #[cfg(test)]
        let before_block = {
            self.input_state_batch_cas_before
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
        };
        #[cfg(test)]
        if let Some((entered, release)) = before_block {
            entered.notify_one();
            release.notified().await;
        }

        let mut inner = self.inner.lock().await;
        let Some(states) = inner.input_states.get_mut(&runtime_id.0) else {
            return Ok(InputStateBatchCasOutcome::Stale);
        };
        let mut all_expected = true;
        let mut all_replacements = true;
        for row in &prepared {
            let Some(current) = states.get(&row.input_id) else {
                return Ok(InputStateBatchCasOutcome::Stale);
            };
            let current_json = serde_json::to_vec(current)
                .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
            if current_json != row.expected_json {
                all_expected = false;
            }
            if current_json != row.replacement_json {
                all_replacements = false;
            }
        }
        if all_replacements {
            return Ok(InputStateBatchCasOutcome::Swapped);
        }
        if !all_expected {
            return Ok(InputStateBatchCasOutcome::Stale);
        }
        let _ = states;
        let prepared = prepare_memory_input_state_mutations(
            &inner,
            &runtime_id.0,
            prepared
                .into_iter()
                .map(|row| MemoryInputStateMutation::Upsert(row.replacement))
                .collect(),
        )?;
        apply_prepared_memory_input_state_mutations(&mut inner, &runtime_id.0, prepared);
        drop(inner);

        #[cfg(test)]
        let after_commit_block = {
            self.input_state_batch_cas_after_commit
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
        };
        #[cfg(test)]
        if let Some((entered, release)) = after_commit_block {
            entered.notify_one();
            release.notified().await;
        }
        Ok(InputStateBatchCasOutcome::Swapped)
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

        let mut inner = self.inner.lock().await;
        let Some(states) = inner.input_states.get_mut(&runtime_id.0) else {
            return Ok(FencedInputStateBatchCasOutcome::Stale);
        };
        let mut all_expected = true;
        let mut all_replacements = true;
        for row in &prepared {
            let Some(current) = states.get(&row.input_id) else {
                return Ok(FencedInputStateBatchCasOutcome::Stale);
            };
            let current_json = serde_json::to_vec(current)
                .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
            if current_json != row.expected_json {
                all_expected = false;
            }
            if current_json != row.replacement_json {
                all_replacements = false;
            }
        }
        if !all_replacements && !all_expected {
            return Ok(FencedInputStateBatchCasOutcome::Stale);
        }

        let _ = states;
        let prepared = if all_replacements {
            Vec::new()
        } else {
            prepare_memory_input_state_mutations(
                &inner,
                &runtime_id.0,
                prepared
                    .into_iter()
                    .map(|row| MemoryInputStateMutation::Upsert(row.replacement))
                    .collect(),
            )?
        };
        let fence_outcome = execute_runtime_store_write_fence(write_fence.as_ref(), || {
            apply_prepared_memory_input_state_mutations(&mut inner, &runtime_id.0, prepared);
            Ok(())
        })?;
        match fence_outcome {
            RuntimeStoreWriteFenceOutcome::Applied => Ok(FencedInputStateBatchCasOutcome::Swapped),
            RuntimeStoreWriteFenceOutcome::Conflict { reason } => {
                Ok(FencedInputStateBatchCasOutcome::FenceConflict { reason })
            }
            RuntimeStoreWriteFenceOutcome::Backoff { reason } => {
                Ok(FencedInputStateBatchCasOutcome::FenceBackoff { reason })
            }
        }
    }

    async fn compare_and_swap_recovery_input_states_atomically(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected_revision: RecoveryInputSetRevision,
        mutations: &[RecoveryInputStateMutation],
    ) -> Result<InputStateBatchCasOutcome, RuntimeStoreError> {
        let prepared = prepare_recovery_input_state_mutations(mutations)?;
        let mut inner = self.inner.lock().await;
        let current_revision = inner
            .recovery_input_set_revisions
            .get(&runtime_id.0)
            .copied()
            .unwrap_or(0);
        if current_revision != expected_revision.store_generation() {
            return Ok(InputStateBatchCasOutcome::Stale);
        }

        let states = inner.input_states.get(&runtime_id.0);
        let mut changed = Vec::new();
        for mutation in prepared {
            let Some(current) = states.and_then(|states| states.get(mutation.input_id())) else {
                return Ok(InputStateBatchCasOutcome::Stale);
            };
            if memory_input_row_version_digest(current)? != mutation.expected_row_digest() {
                return Ok(InputStateBatchCasOutcome::Stale);
            }
            match &mutation {
                PreparedRecoveryInputStateMutation::Upsert { replacement, .. } => {
                    let current_bytes = serde_json::to_vec(current)
                        .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
                    let replacement_bytes = serde_json::to_vec(replacement)
                        .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
                    if current_bytes != replacement_bytes {
                        changed.push(mutation);
                    }
                }
                PreparedRecoveryInputStateMutation::Delete { .. } => changed.push(mutation),
            }
        }
        let prepared = prepare_memory_input_state_mutations(
            &inner,
            &runtime_id.0,
            changed
                .into_iter()
                .map(|mutation| match mutation {
                    PreparedRecoveryInputStateMutation::Upsert { replacement, .. } => {
                        MemoryInputStateMutation::Upsert(replacement)
                    }
                    PreparedRecoveryInputStateMutation::Delete { input_id, .. } => {
                        MemoryInputStateMutation::Delete(input_id)
                    }
                })
                .collect(),
        )?;
        apply_prepared_memory_input_state_mutations(&mut inner, &runtime_id.0, prepared);
        Ok(InputStateBatchCasOutcome::Swapped)
    }

    async fn compare_and_swap_recovery_input_states_atomically_with_fence(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected_revision: RecoveryInputSetRevision,
        mutations: &[RecoveryInputStateMutation],
        write_fence: Arc<dyn RuntimeStoreWriteFence>,
    ) -> Result<FencedInputStateBatchCasOutcome, RuntimeStoreError> {
        let prepared = prepare_recovery_input_state_mutations(mutations)?;
        let mut inner = self.inner.lock().await;
        let current_revision = inner
            .recovery_input_set_revisions
            .get(&runtime_id.0)
            .copied()
            .unwrap_or(0);
        if current_revision != expected_revision.store_generation() {
            return Ok(FencedInputStateBatchCasOutcome::Stale);
        }

        let states = inner.input_states.get(&runtime_id.0);
        let mut changed = Vec::new();
        for mutation in prepared {
            let Some(current) = states.and_then(|states| states.get(mutation.input_id())) else {
                return Ok(FencedInputStateBatchCasOutcome::Stale);
            };
            if memory_input_row_version_digest(current)? != mutation.expected_row_digest() {
                return Ok(FencedInputStateBatchCasOutcome::Stale);
            }
            match &mutation {
                PreparedRecoveryInputStateMutation::Upsert { replacement, .. } => {
                    let current_bytes = serde_json::to_vec(current)
                        .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
                    let replacement_bytes = serde_json::to_vec(replacement)
                        .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
                    if current_bytes != replacement_bytes {
                        changed.push(mutation);
                    }
                }
                PreparedRecoveryInputStateMutation::Delete { .. } => changed.push(mutation),
            }
        }
        let prepared = prepare_memory_input_state_mutations(
            &inner,
            &runtime_id.0,
            changed
                .into_iter()
                .map(|mutation| match mutation {
                    PreparedRecoveryInputStateMutation::Upsert { replacement, .. } => {
                        MemoryInputStateMutation::Upsert(replacement)
                    }
                    PreparedRecoveryInputStateMutation::Delete { input_id, .. } => {
                        MemoryInputStateMutation::Delete(input_id)
                    }
                })
                .collect(),
        )?;

        let fence_outcome = execute_runtime_store_write_fence(write_fence.as_ref(), || {
            apply_prepared_memory_input_state_mutations(&mut inner, &runtime_id.0, prepared);
            Ok(())
        })?;
        match fence_outcome {
            RuntimeStoreWriteFenceOutcome::Applied => Ok(FencedInputStateBatchCasOutcome::Swapped),
            RuntimeStoreWriteFenceOutcome::Conflict { reason } => {
                Ok(FencedInputStateBatchCasOutcome::FenceConflict { reason })
            }
            RuntimeStoreWriteFenceOutcome::Backoff { reason } => {
                Ok(FencedInputStateBatchCasOutcome::FenceBackoff { reason })
            }
        }
    }

    async fn load_input_state(
        &self,
        runtime_id: &LogicalRuntimeId,
        input_id: &InputId,
    ) -> Result<Option<StoredInputState>, RuntimeStoreError> {
        let inner = self.inner.lock().await;
        let state = inner
            .input_states
            .get(&runtime_id.0)
            .and_then(|m| m.get(input_id).cloned());
        Ok(state)
    }

    async fn load_input_state_by_idempotency_key(
        &self,
        runtime_id: &LogicalRuntimeId,
        key: &IdempotencyKey,
    ) -> Result<Option<ExactInputStateObservation>, RuntimeStoreError> {
        let inner = self.inner.lock().await;
        let uncertain = |evidence_input_id: String, reason: String| {
            RuntimeStoreError::InputIdempotencyIndexUncertain {
                runtime_id: runtime_id.to_string(),
                key: key.to_string(),
                evidence_input_id,
                reason,
            }
        };
        let Some(input_id) = inner
            .input_idempotency_index
            .get(&runtime_id.0)
            .and_then(|index| index.get(&key.0))
        else {
            return Ok(None);
        };
        let state = inner
            .input_states
            .get(&runtime_id.0)
            .and_then(|states| states.get(input_id))
            .cloned()
            .ok_or_else(|| {
                uncertain(
                    input_id.to_string(),
                    "index names a missing source input row".to_string(),
                )
            })?;
        if &state.state.input_id != input_id || state.state.idempotency_key.as_ref() != Some(key) {
            return Err(uncertain(
                input_id.to_string(),
                format!(
                    "index owner differs from source identity/key (decoded input {}, decoded key \
                     {:?})",
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
                input_id.to_string(),
                format!("indexed source input row has a non-authoritative machine seed: {error}"),
            )
        })?;
        let exact_row_digest = memory_input_row_version_digest(&state).map_err(|error| {
            uncertain(
                input_id.to_string(),
                format!("indexed source input row could not be encoded exactly: {error}"),
            )
        })?;
        ExactInputStateObservation::from_exact_stored_row(state, exact_row_digest)
            .map(Some)
            .map_err(|error| {
                uncertain(
                    input_id.to_string(),
                    format!("indexed source row could not produce an exact observation: {error}"),
                )
            })
    }

    async fn load_input_states_by_ids(
        &self,
        runtime_id: &LogicalRuntimeId,
        input_ids: &[InputId],
    ) -> Result<Vec<Option<StoredInputState>>, RuntimeStoreError> {
        validate_input_state_batch_read_ids(input_ids)?;
        let inner = self.inner.lock().await;
        let states = inner.input_states.get(&runtime_id.0);
        Ok(input_ids
            .iter()
            .map(|input_id| states.and_then(|rows| rows.get(input_id).cloned()))
            .collect())
    }

    async fn load_pending_terminal_owner_ids_page(
        &self,
        runtime_id: &LogicalRuntimeId,
        after: Option<&InputId>,
        limit: usize,
    ) -> Result<Vec<InputId>, RuntimeStoreError> {
        super::validate_pending_terminal_owner_page(after, limit, &[])?;
        let inner = self.inner.lock().await;
        let Some(owners) = inner.pending_terminal_owners.get(&runtime_id.0) else {
            return Ok(Vec::new());
        };
        let lower = after
            .map(|input_id| std::ops::Bound::Excluded(input_id.0))
            .unwrap_or(std::ops::Bound::Unbounded);
        let owner_input_ids = owners
            .range((lower, std::ops::Bound::Unbounded))
            .take(limit)
            .copied()
            .map(InputId::from_uuid)
            .collect::<Vec<_>>();
        super::validate_pending_terminal_owner_page(after, limit, &owner_input_ids)?;
        Ok(owner_input_ids)
    }

    async fn observe_machine_lifecycle(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<MachineLifecycleObservation, RuntimeStoreError> {
        #[cfg(test)]
        if self
            .machine_lifecycle_observe_errors_remaining
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                remaining.checked_sub(1)
            })
            .is_ok()
        {
            return Err(RuntimeStoreError::ReadFailed(
                "synthetic machine lifecycle transport failure".to_string(),
            ));
        }
        let inner = self.inner.lock().await;
        Ok(inner
            .runtime_lifecycle
            .get(&runtime_id.0)
            .map_or(MachineLifecycleObservation::Missing, |bytes| {
                classify_machine_lifecycle_record(bytes)
            }))
    }

    async fn compare_and_swap_machine_lifecycle(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected: MachineLifecycleExpectedVersion,
        replacement: MachineLifecycleCommit,
    ) -> Result<MachineLifecycleCasOutcome, RuntimeStoreError> {
        let replacement = prepare_machine_lifecycle_replacement(replacement)?;
        let mut inner = self.inner.lock().await;
        let current_raw = inner.runtime_lifecycle.get(&runtime_id.0).cloned();
        let current = current_raw.as_deref().map_or(
            MachineLifecycleObservation::Missing,
            classify_machine_lifecycle_record,
        );
        #[cfg(test)]
        if self
            .machine_lifecycle_cas_conflicts_remaining
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                remaining.checked_sub(1)
            })
            .is_ok()
        {
            return Ok(MachineLifecycleCasOutcome::Conflict { current });
        }
        let matches = match (&expected, &current) {
            (MachineLifecycleExpectedVersion::Missing, MachineLifecycleObservation::Missing) => {
                true
            }
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
        let runtime_state = replacement.snapshot.runtime_state();
        inner
            .runtime_lifecycle
            .insert(runtime_id.0.clone(), replacement.bytes);
        sync_runtime_session_catalog_lifecycle(&mut inner, &runtime_id.0, runtime_state);
        Ok(MachineLifecycleCasOutcome::Applied {
            version: replacement.version,
        })
    }

    async fn compare_and_swap_machine_lifecycle_with_fence(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected: MachineLifecycleExpectedVersion,
        replacement: MachineLifecycleCommit,
        write_fence: Arc<dyn RuntimeStoreWriteFence>,
    ) -> Result<FencedMachineLifecycleCasOutcome, RuntimeStoreError> {
        let replacement = prepare_machine_lifecycle_replacement(replacement)?;
        let mut inner = self.inner.lock().await;
        let current_raw = inner.runtime_lifecycle.get(&runtime_id.0).cloned();
        let current = current_raw.as_deref().map_or(
            MachineLifecycleObservation::Missing,
            classify_machine_lifecycle_record,
        );
        let matches = match (&expected, &current) {
            (MachineLifecycleExpectedVersion::Missing, MachineLifecycleObservation::Missing) => {
                true
            }
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
        let runtime_state = replacement.snapshot.runtime_state();
        let fence_outcome = execute_runtime_store_write_fence(write_fence.as_ref(), || {
            if !already_exact {
                inner
                    .runtime_lifecycle
                    .insert(runtime_id.0.clone(), replacement.bytes.clone());
            }
            sync_runtime_session_catalog_lifecycle(&mut inner, &runtime_id.0, runtime_state);
            Ok(())
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
    }

    async fn load_machine_lifecycle_record(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<Vec<u8>>, RuntimeStoreError> {
        let inner = self.inner.lock().await;
        Ok(inner.runtime_lifecycle.get(&runtime_id.0).cloned())
    }

    async fn commit_machine_lifecycle(
        &self,
        runtime_id: &LogicalRuntimeId,
        commit: MachineLifecycleCommit,
        input_states: &[InputStatePersistenceRecord],
    ) -> Result<(), RuntimeStoreError> {
        let runtime_state = commit.runtime_state();
        let record = commit.store_record().encode()?;
        let mut inner = self.inner.lock().await;
        let rid = runtime_id.0.clone();
        let prepared_input_mutations = prepare_memory_input_state_mutations(
            &inner,
            &rid,
            input_states
                .iter()
                .map(|record| MemoryInputStateMutation::Upsert(record.clone_stored()))
                .collect(),
        )?;

        // Single lock acquisition — atomic for in-memory
        inner.runtime_lifecycle.insert(rid.clone(), record);
        sync_runtime_session_catalog_lifecycle(&mut inner, &rid, runtime_state);
        apply_prepared_memory_input_state_mutations(&mut inner, &rid, prepared_input_mutations);

        Ok(())
    }

    async fn commit_unregister_finalization(
        &self,
        runtime_id: &LogicalRuntimeId,
        finalization: crate::store::UnregisterFinalizationCommit,
    ) -> Result<(), RuntimeStoreError> {
        let (snapshot, input_states, retired_ops_epoch) = finalization.into_parts();
        let runtime_state = snapshot.runtime_state();
        let lifecycle_record = MachineLifecycleStoreRecord::from_snapshot(&snapshot).encode()?;
        let mut inner = self.inner.lock().await;
        let rid = runtime_id.0.clone();
        let prepared_input_mutations = prepare_memory_input_state_mutations(
            &inner,
            &rid,
            input_states
                .into_iter()
                .map(|record| MemoryInputStateMutation::Upsert(record.clone_stored()))
                .collect(),
        )?;

        // One lock acquisition is the in-memory transaction boundary. The
        // finalization token prepared every owned value before this method, so
        // no fallible request preparation remains after the first mutation.
        inner
            .runtime_lifecycle
            .insert(rid.clone(), lifecycle_record);
        sync_runtime_session_catalog_lifecycle(&mut inner, &rid, runtime_state);
        apply_prepared_memory_input_state_mutations(&mut inner, &rid, prepared_input_mutations);
        if inner
            .ops_lifecycle_snapshots
            .get(&rid)
            .is_some_and(|snapshot| snapshot.epoch_id == retired_ops_epoch)
        {
            inner.ops_lifecycle_snapshots.remove(&rid);
        }
        inner.retired_ops_epochs.insert((rid, retired_ops_epoch));
        Ok(())
    }

    async fn persist_ops_lifecycle(
        &self,
        runtime_id: &LogicalRuntimeId,
        snapshot: &PersistedOpsSnapshot,
    ) -> Result<(), RuntimeStoreError> {
        let mut inner = self.inner.lock().await;
        if inner
            .retired_ops_epochs
            .contains(&(runtime_id.0.clone(), snapshot.epoch_id.clone()))
        {
            return Err(RuntimeStoreError::OpsLifecycleEpochRetired {
                runtime_id: runtime_id.0.clone(),
                epoch_id: snapshot.epoch_id.clone(),
            });
        }
        inner
            .ops_lifecycle_snapshots
            .insert(runtime_id.0.clone(), snapshot.clone());
        Ok(())
    }

    async fn initialize_ops_lifecycle_if_absent(
        &self,
        runtime_id: &LogicalRuntimeId,
        candidate: &PersistedOpsSnapshot,
    ) -> Result<PersistedOpsSnapshot, RuntimeStoreError> {
        let mut inner = self.inner.lock().await;
        let key = runtime_id.0.clone();
        if inner
            .retired_ops_epochs
            .contains(&(key.clone(), candidate.epoch_id.clone()))
        {
            return Err(RuntimeStoreError::OpsLifecycleEpochRetired {
                runtime_id: key,
                epoch_id: candidate.epoch_id.clone(),
            });
        }
        let canonical = inner
            .ops_lifecycle_snapshots
            .entry(key)
            .or_insert_with(|| candidate.clone())
            .clone();
        if inner
            .retired_ops_epochs
            .contains(&(runtime_id.0.clone(), canonical.epoch_id.clone()))
        {
            return Err(RuntimeStoreError::OpsLifecycleEpochRetired {
                runtime_id: runtime_id.0.clone(),
                epoch_id: canonical.epoch_id,
            });
        }
        Ok(canonical)
    }

    async fn load_ops_lifecycle(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<PersistedOpsSnapshot>, RuntimeStoreError> {
        let inner = self.inner.lock().await;
        Ok(inner.ops_lifecycle_snapshots.get(&runtime_id.0).cloned())
    }

    async fn delete_ops_lifecycle(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<(), RuntimeStoreError> {
        let mut inner = self.inner.lock().await;
        inner.ops_lifecycle_snapshots.remove(&runtime_id.0);
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::RuntimeState;
    use crate::store::MachineLifecycleBindingFacts;
    use meerkat_core::lifecycle::run_primitive::RunApplyBoundary;

    #[tokio::test]
    async fn pending_terminal_owner_index_satisfies_store_contract() {
        crate::store::assert_pending_terminal_owner_index_contract(&InMemoryRuntimeStore::new())
            .await;
    }

    fn make_receipt(run_id: RunId, seq: u64) -> RunBoundaryReceipt {
        RunBoundaryReceipt {
            run_id,
            boundary: RunApplyBoundary::RunStart,
            contributing_input_ids: vec![],
            conversation_digest: None,
            message_count: 0,
            sequence: seq,
        }
    }

    fn lifecycle_commit(
        runtime_id: &LogicalRuntimeId,
        state: RuntimeState,
        fence_token: u64,
        runtime_generation: u64,
    ) -> MachineLifecycleCommit {
        MachineLifecycleCommit::new_with_binding(
            state,
            MachineLifecycleBindingFacts::new(
                Some(runtime_id.0.clone()),
                Some(fence_token),
                Some(runtime_generation),
                Some(format!("epoch-{runtime_generation}")),
            ),
            crate::store::SupervisorAuthoritySnapshot::UnboundNoReceipt,
        )
    }

    struct AppliedWriteFence;

    impl RuntimeStoreWriteFence for AppliedWriteFence {
        fn execute_if_current(
            &self,
            operation: Box<dyn FnOnce() -> Result<(), RuntimeStoreError> + '_>,
        ) -> Result<RuntimeStoreWriteFenceOutcome, RuntimeStoreError> {
            operation()?;
            Ok(RuntimeStoreWriteFenceOutcome::Applied)
        }
    }

    fn persistable(bundle: StoredInputState) -> InputStatePersistenceRecord {
        InputStatePersistenceRecord::from_machine_snapshot(bundle).unwrap()
    }

    fn session_with_user(content: &str) -> meerkat_core::Session {
        let mut session = meerkat_core::Session::new();
        session.push(meerkat_core::types::Message::User(
            meerkat_core::types::UserMessage::text(content.to_string()),
        ));
        session
    }

    fn encode_as_released_0810_compaction_fixture(
        session: &meerkat_core::Session,
    ) -> serde_json::Value {
        let history = session
            .validated_transcript_history_state()
            .unwrap()
            .expect("fixture rewrite graph exists");
        assert_eq!(history.commit_count(), 1, "fixture has one rewrite");
        let commit = history.last_commit().expect("fixture rewrite commit");
        let (start, end) = commit.selection.bounds();
        let mut released_commit = serde_json::to_value(commit).unwrap();
        released_commit
            .as_object_mut()
            .unwrap()
            .remove("rewrite_generation");
        released_commit["selection"] = serde_json::json!({
            "type": "compaction_message_range",
            "range": { "start": start, "end": end }
        });
        let released_graph = serde_json::json!({
            "head": history.head(),
            "commits": [released_commit],
            "revisions": [
                history.materialize_revision(&commit.parent_revision).unwrap(),
                history.materialize_revision(&commit.revision).unwrap(),
            ],
            "digest_format": history.digest_format(),
        });
        let mut encoded = serde_json::to_value(session).unwrap();
        encoded["version"] = serde_json::json!(2);
        encoded["metadata"][meerkat_core::SESSION_TRANSCRIPT_HISTORY_STATE_KEY] = released_graph;
        encoded["metadata"]
            .as_object_mut()
            .unwrap()
            .remove(meerkat_core::SESSION_TRANSCRIPT_REWRITE_PREFIX_AUTHORITY_KEY);
        encoded
    }

    fn compaction_commit_fingerprint(commit: &meerkat_core::TranscriptRewriteCommit) -> String {
        use sha2::{Digest as _, Sha256};

        #[derive(serde::Serialize)]
        struct Fingerprint<'a> {
            selection: &'a meerkat_core::TranscriptRewriteSelection,
            original_span_digest: &'a str,
            replacement_digest: &'a str,
            messages_before: usize,
            messages_after: usize,
            actor: &'a Option<String>,
        }

        let canonical = serde_json::to_vec(&Fingerprint {
            selection: &commit.selection,
            original_span_digest: &commit.original_span_digest,
            replacement_digest: &commit.replacement_digest,
            messages_before: commit.messages_before,
            messages_after: commit.messages_after,
            actor: &commit.actor,
        })
        .unwrap();
        format!("sha256:{:x}", Sha256::digest(canonical))
    }

    fn session_with_compaction_intent() -> (
        meerkat_core::Session,
        meerkat_core::CompactionProjectionIntent,
    ) {
        let mut session = session_with_user("verbose context one");
        session.push(meerkat_core::types::Message::User(
            meerkat_core::types::UserMessage::text("verbose context two"),
        ));
        let parent = session.transcript_revision().unwrap();
        session
            .commit_transcript_rewrite(
                meerkat_core::TranscriptRewriteSelection::MessageRange { start: 0, end: 2 },
                vec![meerkat_core::types::Message::User(
                    meerkat_core::types::UserMessage::compaction_summary("compacted context"),
                )],
                meerkat_core::TranscriptRewriteReason::new("compaction"),
                Some("runtime-store-test".to_string()),
                Some(parent),
            )
            .unwrap();
        let encoded = encode_as_released_0810_compaction_fixture(&session);
        let encoded = serde_json::to_vec(&encoded).unwrap();
        let (mut session, _import_receipt) = meerkat_core::import_released_0810_session(&encoded)
            .unwrap()
            .into_parts();
        let commit = session
            .validated_transcript_history_state()
            .unwrap()
            .unwrap()
            .last_commit()
            .unwrap()
            .clone();
        let commit_fingerprint = compaction_commit_fingerprint(&commit);
        let intent = meerkat_core::CompactionProjectionIntent {
            projection: serde_json::from_value(serde_json::json!({
                "session_id": session.id(),
                "parent_revision": &commit.parent_revision,
                "revision": &commit.revision,
                "commit_fingerprint": commit_fingerprint,
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
        session: &meerkat_core::Session,
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
    async fn atomic_apply_commits_rewrite_and_compaction_outbox_as_one_boundary() {
        let store = InMemoryRuntimeStore::new();
        let (session, intent) = session_with_compaction_intent();
        let rid = LogicalRuntimeId::for_session(session.id());
        let snapshot = serde_json::to_vec(&session).unwrap();
        store
            .atomic_apply(
                &rid,
                Some(SerializedSessionSnapshot {
                    session_snapshot: snapshot.clone().into(),
                }),
                make_receipt(RunId::new(), 1),
                vec![],
                Some(session.id().clone()),
            )
            .await
            .unwrap();
        assert_eq!(
            store.load_session_snapshot(&rid).await.unwrap(),
            Some(Arc::new(snapshot))
        );
        assert_eq!(
            store
                .load_pending_compaction_projections(&rid)
                .await
                .unwrap(),
            vec![intent.clone()]
        );
        store
            .mark_compaction_projection_finalized(&rid, &intent.projection)
            .await
            .unwrap();
        store
            .mark_compaction_projection_finalized(&rid, &intent.projection)
            .await
            .unwrap();
        assert!(
            store
                .load_pending_compaction_projections(&rid)
                .await
                .unwrap()
                .is_empty()
        );
        let persisted: meerkat_core::Session =
            serde_json::from_slice(&store.load_session_snapshot(&rid).await.unwrap().unwrap())
                .unwrap();
        assert!(
            persisted
                .compaction_projection_intents()
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn finalized_outbox_tombstone_rejects_atomic_and_non_boundary_snapshot_replay() {
        let store = InMemoryRuntimeStore::new();
        let (session, intent) = session_with_compaction_intent();
        let rid = LogicalRuntimeId::for_session(session.id());
        let replay_snapshot = serde_json::to_vec(&session).unwrap();
        store
            .atomic_apply(
                &rid,
                Some(SerializedSessionSnapshot {
                    session_snapshot: replay_snapshot.clone().into(),
                }),
                make_receipt(RunId::new(), 1),
                vec![],
                Some(session.id().clone()),
            )
            .await
            .unwrap();
        store
            .mark_compaction_projection_finalized(&rid, &intent.projection)
            .await
            .unwrap();
        let cleaned_snapshot = store.load_session_snapshot(&rid).await.unwrap().unwrap();

        let replay_run_id = RunId::new();
        let error = store
            .atomic_apply(
                &rid,
                Some(SerializedSessionSnapshot {
                    session_snapshot: replay_snapshot.clone().into(),
                }),
                make_receipt(replay_run_id.clone(), 2),
                vec![],
                Some(session.id().clone()),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("finalized compaction intent"));
        assert!(
            store
                .load_boundary_receipt(&rid, &replay_run_id, 2)
                .await
                .unwrap()
                .is_none(),
            "finalized replay rejection must roll back the whole atomic boundary"
        );

        let error = store
            .commit_session_snapshot(
                &rid,
                SerializedSessionSnapshot {
                    session_snapshot: replay_snapshot.clone().into(),
                },
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("finalized compaction intent"));
        let error = store
            .replace_session_snapshot_if_current(&rid, &cleaned_snapshot, replay_snapshot)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("finalized compaction intent"));

        assert_eq!(
            store.load_session_snapshot(&rid).await.unwrap(),
            Some(cleaned_snapshot)
        );
        assert!(
            store
                .load_pending_compaction_projections(&rid)
                .await
                .unwrap()
                .is_empty(),
            "a finalized tombstone must never be silently revived or left untracked"
        );
    }

    #[tokio::test]
    async fn invalid_compaction_intent_leaves_snapshot_and_outbox_unmodified() {
        let store = InMemoryRuntimeStore::new();
        let (session, mut intent) = session_with_compaction_intent();
        let rid = LogicalRuntimeId::for_session(session.id());
        intent.summary_tokens += 1;
        let conflicting = vec![
            session.compaction_projection_intents().unwrap()[0].clone(),
            intent,
        ];
        let error = store
            .atomic_apply(
                &rid,
                Some(SerializedSessionSnapshot {
                    session_snapshot: snapshot_with_raw_intents(&session, &conflicting).into(),
                }),
                make_receipt(RunId::new(), 2),
                vec![],
                Some(session.id().clone()),
            )
            .await
            .unwrap_err();
        assert!(matches!(error, RuntimeStoreError::WriteFailed(_)));
        assert_eq!(store.load_session_snapshot(&rid).await.unwrap(), None);
        assert!(
            store
                .load_pending_compaction_projections(&rid)
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
                    &rid,
                    Some(SerializedSessionSnapshot {
                        session_snapshot: snapshot_with_raw_intents(&session, &[invalid]).into(),
                    }),
                    make_receipt(RunId::new(), 10 + sequence as u64),
                    vec![],
                    Some(session.id().clone()),
                )
                .await
                .unwrap_err();
            assert!(matches!(error, RuntimeStoreError::WriteFailed(_)));
            assert_eq!(store.load_session_snapshot(&rid).await.unwrap(), None);
            assert!(
                store
                    .load_pending_compaction_projections(&rid)
                    .await
                    .unwrap()
                    .is_empty()
            );
        }
    }

    #[tokio::test]
    async fn atomic_apply_commits_compaction_target_state_and_advances_outbox() {
        let store = InMemoryRuntimeStore::new();
        let (incoming, intent) = session_with_compaction_intent();
        let rid = LogicalRuntimeId::for_session(incoming.id());
        let mut current = incoming.clone();
        current
            .complete_compaction_projection_intent(&intent.projection)
            .unwrap();
        current.push(meerkat_core::types::Message::User(
            meerkat_core::types::UserMessage::text("already advanced"),
        ));
        let current_snapshot = serde_json::to_vec(&current).unwrap();
        store
            .commit_session_snapshot(
                &rid,
                SerializedSessionSnapshot {
                    session_snapshot: current_snapshot.clone().into(),
                },
            )
            .await
            .unwrap();
        let incoming_snapshot = serde_json::to_vec(&incoming).unwrap();
        let receipt = make_receipt(RunId::new(), 3);
        store
            .atomic_apply(
                &rid,
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
            store.load_session_snapshot(&rid).await.unwrap(),
            Some(Arc::new(incoming_snapshot))
        );
        assert_eq!(
            store
                .load_pending_compaction_projections(&rid)
                .await
                .unwrap()
                .as_slice(),
            &[intent]
        );
        assert_eq!(
            store
                .load_boundary_receipt(&rid, &receipt.run_id, receipt.sequence)
                .await
                .unwrap(),
            Some(receipt)
        );
    }

    #[tokio::test]
    async fn existing_outbox_rejects_changed_intent_without_advancing_snapshot() {
        let store = InMemoryRuntimeStore::new();
        let (session, intent) = session_with_compaction_intent();
        let rid = LogicalRuntimeId::for_session(session.id());
        let original_snapshot = serde_json::to_vec(&session).unwrap();
        store
            .atomic_apply(
                &rid,
                Some(SerializedSessionSnapshot {
                    session_snapshot: original_snapshot.clone().into(),
                }),
                make_receipt(RunId::new(), 60),
                vec![],
                Some(session.id().clone()),
            )
            .await
            .unwrap();

        let mut advanced = session.clone();
        advanced.push(meerkat_core::types::Message::User(
            meerkat_core::types::UserMessage::text("later turn"),
        ));
        let mut conflicting = intent.clone();
        conflicting.summary_tokens += 1;
        let error = store
            .atomic_apply(
                &rid,
                Some(SerializedSessionSnapshot {
                    session_snapshot: snapshot_with_raw_intents(&advanced, &[conflicting]).into(),
                }),
                make_receipt(RunId::new(), 61),
                vec![],
                Some(session.id().clone()),
            )
            .await
            .unwrap_err();
        assert!(matches!(error, RuntimeStoreError::WriteFailed(_)));
        assert_eq!(
            store.load_session_snapshot(&rid).await.unwrap(),
            Some(Arc::new(original_snapshot))
        );
        assert_eq!(
            store
                .load_pending_compaction_projections(&rid)
                .await
                .unwrap(),
            vec![intent]
        );
    }

    #[tokio::test]
    async fn non_boundary_snapshot_apis_cannot_bypass_compaction_outbox() {
        let store = InMemoryRuntimeStore::new();
        let (session, _intent) = session_with_compaction_intent();
        let rid = LogicalRuntimeId::for_session(session.id());
        let snapshot = serde_json::to_vec(&session).unwrap();
        assert!(
            store
                .commit_session_snapshot(
                    &rid,
                    SerializedSessionSnapshot {
                        session_snapshot: snapshot.clone().into(),
                    },
                )
                .await
                .is_err()
        );
        assert_eq!(store.load_session_snapshot(&rid).await.unwrap(), None);
        let clean = meerkat_core::Session::with_id(session.id().clone());
        let clean_snapshot = serde_json::to_vec(&clean).unwrap();
        store
            .commit_session_snapshot(
                &rid,
                SerializedSessionSnapshot {
                    session_snapshot: clean_snapshot.clone().into(),
                },
            )
            .await
            .unwrap();
        assert!(
            store
                .replace_session_snapshot_if_current(&rid, &clean_snapshot, snapshot)
                .await
                .is_err()
        );
        assert_eq!(
            store.load_session_snapshot(&rid).await.unwrap(),
            Some(Arc::new(clean_snapshot))
        );
        assert!(
            store
                .load_pending_compaction_projections(&rid)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn atomic_apply_roundtrip() {
        let store = InMemoryRuntimeStore::new();
        let run_id = RunId::new();
        let input_id = InputId::new();

        let bundle = StoredInputState::new_accepted(input_id.clone());
        let receipt = make_receipt(run_id.clone(), 0);

        let session = session_with_user("hello");
        let rid = LogicalRuntimeId::for_session(session.id());
        let session_snapshot = serde_json::to_vec(&session).unwrap();

        store
            .atomic_apply(
                &rid,
                Some(SerializedSessionSnapshot {
                    session_snapshot: session_snapshot.into(),
                }),
                receipt.clone(),
                vec![persistable(bundle)],
                None,
            )
            .await
            .unwrap();

        // Load input states
        let states = store.load_input_states_strict(&rid).await.unwrap();
        assert_eq!(states.len(), 1);
        assert_eq!(states[0].state.input_id, input_id);

        // Load receipt
        let loaded = store.load_boundary_receipt(&rid, &run_id, 0).await.unwrap();
        assert!(loaded.is_some());
    }

    #[tokio::test]
    async fn machine_terminal_atomic_apply_rolls_back_all_maps_on_receipt_conflict() {
        let store = InMemoryRuntimeStore::new();
        let session = session_with_user("must roll back");
        let rid = LogicalRuntimeId::for_session(session.id());
        let receipt = make_receipt(RunId::new(), 0);
        let seeded_input = StoredInputState::new_accepted(InputId::new());
        store
            .atomic_apply(
                &rid,
                None,
                receipt.clone(),
                vec![persistable(seeded_input.clone())],
                None,
            )
            .await
            .unwrap();

        let replacement_input = StoredInputState::new_accepted(InputId::new());
        let error = store
            .atomic_apply_with_machine_lifecycle(
                &rid,
                SerializedSessionSnapshot {
                    session_snapshot: serde_json::to_vec(&session).unwrap().into(),
                },
                receipt,
                MachineLifecycleCommit::new_with_binding(
                    crate::RuntimeState::Idle,
                    crate::store::MachineLifecycleBindingFacts::default(),
                    crate::store::SupervisorAuthoritySnapshot::UnboundNoReceipt,
                ),
                vec![persistable(replacement_input)],
                session.id().clone(),
            )
            .await
            .expect_err("duplicate receipt must reject the entire terminal transaction");
        assert!(matches!(error, RuntimeStoreError::WriteFailed(_)));
        assert!(store.load_session_snapshot(&rid).await.unwrap().is_none());
        assert_eq!(
            crate::store::load_runtime_state(&store, &rid)
                .await
                .unwrap(),
            None
        );
        let inputs = store.load_input_states_strict(&rid).await.unwrap();
        assert_eq!(inputs.len(), 1);
        assert_eq!(inputs[0].state.input_id, seeded_input.state.input_id);
    }

    #[tokio::test]
    async fn machine_terminal_atomic_apply_tracks_and_tombstones_compaction_intents() {
        let store = InMemoryRuntimeStore::new();
        let (session, intent) = session_with_compaction_intent();
        let rid = LogicalRuntimeId::for_session(session.id());
        let encoded = serde_json::to_vec(&session).unwrap();

        store
            .atomic_apply_with_machine_lifecycle(
                &rid,
                SerializedSessionSnapshot {
                    session_snapshot: encoded.clone().into(),
                },
                make_receipt(RunId::new(), 0),
                MachineLifecycleCommit::new_with_binding(
                    crate::RuntimeState::Idle,
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
                .load_pending_compaction_projections(&rid)
                .await
                .unwrap(),
            vec![intent.clone()]
        );

        store
            .mark_compaction_projection_finalized(&rid, &intent.projection)
            .await
            .unwrap();
        let error = store
            .atomic_apply_with_machine_lifecycle(
                &rid,
                SerializedSessionSnapshot {
                    session_snapshot: encoded.into(),
                },
                make_receipt(RunId::new(), 1),
                MachineLifecycleCommit::new_with_binding(
                    crate::RuntimeState::Idle,
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
    async fn machine_terminal_atomic_apply_replaces_orphan_body_and_commits_all_effects() {
        let store = InMemoryRuntimeStore::new();
        let session = session_with_user("incoming terminal transcript");
        let rid = LogicalRuntimeId::for_session(session.id());
        let corrupt = b"{not-a-session".to_vec();
        store
            .inner
            .lock()
            .await
            .sessions
            .insert(rid.0.clone(), corrupt.clone().into());
        assert!(matches!(
            store.load_committed_whole_blob_snapshot(&rid).await,
            Err(RuntimeStoreError::SessionPersistenceAuthorityConflict { .. })
        ));
        let receipt = make_receipt(RunId::new(), 0);
        let input_id = InputId::new();
        let session_snapshot = serde_json::to_vec(&session).unwrap();
        store
            .atomic_apply_with_machine_lifecycle(
                &rid,
                SerializedSessionSnapshot {
                    session_snapshot: session_snapshot.clone().into(),
                },
                receipt.clone(),
                MachineLifecycleCommit::new_with_binding(
                    crate::RuntimeState::Idle,
                    crate::store::MachineLifecycleBindingFacts::default(),
                    crate::store::SupervisorAuthoritySnapshot::UnboundNoReceipt,
                ),
                vec![persistable(StoredInputState::new_accepted(
                    input_id.clone(),
                ))],
                session.id().clone(),
            )
            .await
            .unwrap();
        assert_eq!(
            store.load_session_snapshot(&rid).await.unwrap(),
            Some(Arc::new(session_snapshot))
        );
        assert_eq!(
            crate::store::load_runtime_state(&store, &rid)
                .await
                .unwrap(),
            Some(crate::RuntimeState::Idle)
        );
        assert_eq!(
            store.load_input_states_strict(&rid).await.unwrap()[0]
                .state
                .input_id,
            input_id
        );
        assert_eq!(
            store
                .load_boundary_receipt(&rid, &receipt.run_id, receipt.sequence)
                .await
                .unwrap(),
            Some(receipt)
        );
    }

    #[tokio::test]
    async fn machine_terminal_atomic_apply_commits_target_state_and_publication_atomically() {
        let store = InMemoryRuntimeStore::new();
        let incoming = session_with_user("failed turn input");
        let rid = LogicalRuntimeId::for_session(incoming.id());
        let mut durable_head = incoming.clone();
        durable_head.push(meerkat_core::types::Message::User(
            meerkat_core::types::UserMessage::text("already advanced"),
        ));
        let durable_snapshot = serde_json::to_vec(&durable_head).unwrap();
        store
            .commit_session_snapshot(
                &rid,
                SerializedSessionSnapshot {
                    session_snapshot: durable_snapshot.clone().into(),
                },
            )
            .await
            .unwrap();

        let receipt = make_receipt(RunId::new(), 0);
        let input_id = InputId::new();
        let incoming_snapshot = serde_json::to_vec(&incoming).unwrap();
        store
            .atomic_apply_with_machine_lifecycle(
                &rid,
                SerializedSessionSnapshot {
                    session_snapshot: incoming_snapshot.clone().into(),
                },
                receipt.clone(),
                MachineLifecycleCommit::new_with_binding(
                    crate::RuntimeState::Idle,
                    MachineLifecycleBindingFacts::default(),
                    crate::store::SupervisorAuthoritySnapshot::UnboundNoReceipt,
                ),
                vec![persistable(StoredInputState::new_accepted(
                    input_id.clone(),
                ))],
                incoming.id().clone(),
            )
            .await
            .unwrap();
        assert_eq!(
            store.load_session_snapshot(&rid).await.unwrap(),
            Some(Arc::new(incoming_snapshot))
        );
        assert_eq!(
            crate::store::load_runtime_state(&store, &rid)
                .await
                .unwrap(),
            Some(crate::RuntimeState::Idle)
        );
        assert_eq!(
            store.load_input_states_strict(&rid).await.unwrap()[0]
                .state
                .input_id,
            input_id
        );
        assert_eq!(
            store
                .load_boundary_receipt(&rid, &receipt.run_id, receipt.sequence)
                .await
                .unwrap(),
            Some(receipt)
        );
    }

    #[tokio::test]
    async fn atomic_apply_replaces_orphan_body_and_commits_all_effects() {
        let store = InMemoryRuntimeStore::new();
        let session = session_with_user("incoming transcript");
        let rid = LogicalRuntimeId::for_session(session.id());
        let corrupt = b"{not-a-session".to_vec();
        store
            .inner
            .lock()
            .await
            .sessions
            .insert(rid.0.clone(), corrupt.clone().into());
        assert!(matches!(
            store.load_committed_whole_blob_snapshot(&rid).await,
            Err(RuntimeStoreError::SessionPersistenceAuthorityConflict { .. })
        ));
        let receipt = make_receipt(RunId::new(), 0);
        let input_id = InputId::new();
        let session_snapshot = serde_json::to_vec(&session).unwrap();
        store
            .atomic_apply(
                &rid,
                Some(SerializedSessionSnapshot {
                    session_snapshot: session_snapshot.clone().into(),
                }),
                receipt.clone(),
                vec![persistable(StoredInputState::new_accepted(
                    input_id.clone(),
                ))],
                Some(session.id().clone()),
            )
            .await
            .unwrap();
        assert_eq!(
            store.load_session_snapshot(&rid).await.unwrap(),
            Some(Arc::new(session_snapshot))
        );
        assert_eq!(
            store.load_input_states_strict(&rid).await.unwrap()[0]
                .state
                .input_id,
            input_id
        );
        assert_eq!(
            store
                .load_boundary_receipt(&rid, &receipt.run_id, receipt.sequence)
                .await
                .unwrap(),
            Some(receipt)
        );
    }

    #[tokio::test]
    async fn atomic_apply_rejects_non_session_snapshot_without_owner_context() {
        let store = InMemoryRuntimeStore::new();
        let rid = LogicalRuntimeId::new("test-runtime");
        let run_id = RunId::new();
        let input_id = InputId::new();

        let bundle = StoredInputState::new_accepted(input_id);
        let receipt = make_receipt(run_id, 0);

        // Owner-context absence is not a license to store arbitrary bytes as a
        // session snapshot: a non-deserializable snapshot must fail closed.
        let err = store
            .atomic_apply(
                &rid,
                Some(SerializedSessionSnapshot {
                    session_snapshot: b"session-data".to_vec().into(),
                }),
                receipt,
                vec![persistable(bundle)],
                None,
            )
            .await
            .expect_err("non-Session snapshot must be rejected");

        match err {
            RuntimeStoreError::WriteFailed(message) => {
                assert!(
                    message.contains("not a valid Session payload"),
                    "unexpected WriteFailed message: {message}"
                );
            }
            other => panic!("expected WriteFailed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn persist_and_load_single_state() {
        let store = InMemoryRuntimeStore::new();
        let rid = LogicalRuntimeId::new("test");
        let input_id = InputId::new();
        let bundle = StoredInputState::new_accepted(input_id.clone());

        store
            .persist_input_state(&rid, &persistable(bundle))
            .await
            .unwrap();

        let loaded = store.load_input_state(&rid, &input_id).await.unwrap();
        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap().state.input_id, input_id);
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

    #[tokio::test]
    async fn input_idempotency_mutations_use_complete_final_image() {
        let store = InMemoryRuntimeStore::new();
        crate::store::assert_input_idempotency_final_image_contract(&store).await;
    }

    #[tokio::test]
    async fn input_idempotency_corruption_uses_typed_uncertainty() {
        let store = InMemoryRuntimeStore::new();
        let dangling_runtime = LogicalRuntimeId::new("memory-dangling-idempotency");
        let dangling_input_id = InputId::new();
        {
            let mut inner = store.inner.lock().await;
            inner
                .input_idempotency_index
                .entry(dangling_runtime.0.clone())
                .or_default()
                .insert("dangling-key".to_string(), dangling_input_id.clone());
        }
        assert!(matches!(
            store
                .load_input_state_by_idempotency_key(
                    &dangling_runtime,
                    &IdempotencyKey::new("dangling-key"),
                )
                .await,
            Err(RuntimeStoreError::InputIdempotencyIndexUncertain {
                evidence_input_id,
                reason,
                ..
            }) if evidence_input_id == dangling_input_id.to_string()
                && reason.contains("missing source input row")
        ));

        let invalid_seed_runtime = LogicalRuntimeId::new("memory-invalid-idempotency-seed");
        let invalid_seed_input_id = InputId::new();
        let mut invalid_seed = StoredInputState::new_accepted(invalid_seed_input_id.clone());
        invalid_seed.state.idempotency_key = Some(IdempotencyKey::new("invalid-seed-key"));
        invalid_seed.seed.terminal_outcome =
            Some(crate::input_state::InputTerminalOutcome::Consumed);
        {
            let mut inner = store.inner.lock().await;
            inner
                .input_states
                .entry(invalid_seed_runtime.0.clone())
                .or_default()
                .insert(invalid_seed_input_id.clone(), invalid_seed);
            inner
                .input_idempotency_index
                .entry(invalid_seed_runtime.0.clone())
                .or_default()
                .insert(
                    "invalid-seed-key".to_string(),
                    invalid_seed_input_id.clone(),
                );
        }
        assert!(matches!(
            store
                .load_input_state_by_idempotency_key(
                    &invalid_seed_runtime,
                    &IdempotencyKey::new("invalid-seed-key"),
                )
                .await,
            Err(RuntimeStoreError::InputIdempotencyIndexUncertain {
                evidence_input_id,
                reason,
                ..
            }) if evidence_input_id == invalid_seed_input_id.to_string()
                && reason.contains("non-authoritative machine seed")
        ));
    }

    #[tokio::test]
    async fn input_state_batch_cas_memory_swaps_once_and_stale_is_noop() {
        let store = InMemoryRuntimeStore::new();
        let rid = LogicalRuntimeId::new("input-cas-memory");
        let expected: Vec<_> = (0..3)
            .map(|_| StoredInputState::new_accepted(InputId::new()))
            .collect();
        let initial: Vec<_> = expected.iter().cloned().map(persistable).collect();
        store
            .persist_input_states_atomically(&rid, &initial)
            .await
            .unwrap();

        let winner = replacement_records(&expected, 1);
        let stale_candidate = replacement_records(&expected, 2);
        assert_eq!(
            store
                .compare_and_swap_input_states_atomically(&rid, &expected, &winner)
                .await
                .unwrap(),
            InputStateBatchCasOutcome::Swapped
        );
        assert_eq!(
            store
                .compare_and_swap_input_states_atomically(&rid, &expected, &winner)
                .await
                .unwrap(),
            InputStateBatchCasOutcome::Swapped,
            "retry after a lost CAS acknowledgement must observe the exact replacement as success"
        );
        assert_eq!(
            store
                .compare_and_swap_input_states_atomically(&rid, &expected, &stale_candidate)
                .await
                .unwrap(),
            InputStateBatchCasOutcome::Stale
        );
        let rows = store.load_input_states_strict(&rid).await.unwrap();
        assert_eq!(rows.len(), 3);
        assert!(rows.iter().all(|row| row.state.recovery_count == 1));
    }

    #[tokio::test]
    async fn input_state_batch_cas_memory_rejects_missing_extra_and_key_mismatch() {
        let store = InMemoryRuntimeStore::new();
        let rid = LogicalRuntimeId::new("input-cas-shape");
        let expected: Vec<_> = (0..2)
            .map(|_| StoredInputState::new_accepted(InputId::new()))
            .collect();
        store
            .persist_input_state(&rid, &persistable(expected[0].clone()))
            .await
            .unwrap();
        let replacements = replacement_records(&expected, 1);

        assert_eq!(
            store
                .compare_and_swap_input_states_atomically(&rid, &expected, &replacements)
                .await
                .unwrap(),
            InputStateBatchCasOutcome::Stale,
            "one missing durable row must stale the entire batch"
        );
        assert_eq!(
            store
                .load_input_state(&rid, &expected[0].state.input_id)
                .await
                .unwrap()
                .unwrap()
                .state
                .recovery_count,
            0,
            "stale comparison must not update the matching prefix row"
        );

        let extra = vec![
            replacements[0].clone(),
            replacements[1].clone(),
            persistable(StoredInputState::new_accepted(InputId::new())),
        ];
        assert!(matches!(
            store
                .compare_and_swap_input_states_atomically(&rid, &expected, &extra)
                .await,
            Err(RuntimeStoreError::InvalidInputStateBatchCas { .. })
        ));

        let wrong_key = vec![
            replacements[0].clone(),
            persistable(StoredInputState::new_accepted(InputId::new())),
        ];
        assert!(matches!(
            store
                .compare_and_swap_input_states_atomically(&rid, &expected, &wrong_key)
                .await,
            Err(RuntimeStoreError::InvalidInputStateBatchCas { .. })
        ));
    }

    #[tokio::test]
    async fn load_nonexistent_returns_none() {
        let store = InMemoryRuntimeStore::new();
        let rid = LogicalRuntimeId::new("test");

        let states = store.load_input_states_strict(&rid).await.unwrap();
        assert!(states.is_empty());

        let state = store.load_input_state(&rid, &InputId::new()).await.unwrap();
        assert!(state.is_none());

        let receipt = store
            .load_boundary_receipt(&rid, &RunId::new(), 0)
            .await
            .unwrap();
        assert!(receipt.is_none());
    }

    #[tokio::test]
    async fn atomic_apply_updates_existing() {
        let store = InMemoryRuntimeStore::new();
        let rid = LogicalRuntimeId::new("test");
        let input_id = InputId::new();

        // First write
        let bundle1 = StoredInputState::new_accepted(input_id.clone());
        store
            .atomic_apply(
                &rid,
                None,
                make_receipt(RunId::new(), 0),
                vec![persistable(bundle1)],
                None,
            )
            .await
            .unwrap();

        // Second write with updated seed phase
        let mut bundle2 = StoredInputState::new_accepted(input_id.clone());
        bundle2.seed.phase = crate::input_state::InputLifecycleState::Queued;
        store
            .atomic_apply(
                &rid,
                None,
                make_receipt(RunId::new(), 1),
                vec![persistable(bundle2)],
                None,
            )
            .await
            .unwrap();

        let states = store.load_input_states_strict(&rid).await.unwrap();
        assert_eq!(states.len(), 1);
        assert_eq!(
            states[0].seed.phase,
            crate::input_state::InputLifecycleState::Queued
        );
    }

    #[tokio::test]
    async fn atomic_apply_validates_session_store_key_without_aliasing_snapshot() {
        let store = InMemoryRuntimeStore::new();
        let session = meerkat_core::Session::new();
        let rid = LogicalRuntimeId::for_session(session.id());
        let session_id = session.id().clone();
        let snapshot = serde_json::to_vec(&session).unwrap();

        store
            .atomic_apply(
                &rid,
                Some(SerializedSessionSnapshot {
                    session_snapshot: snapshot.clone().into(),
                }),
                make_receipt(RunId::new(), 0),
                vec![],
                Some(session_id.clone()),
            )
            .await
            .unwrap();

        assert_eq!(
            store.load_session_snapshot(&rid).await.unwrap(),
            Some(Arc::new(snapshot))
        );
        assert!(
            store
                .load_session_snapshot(&LogicalRuntimeId::legacy_session_uuid_alias(&session_id))
                .await
                .unwrap()
                .is_none(),
            "session_store_key must validate the snapshot identity, not create a raw UUID runtime alias"
        );
    }

    #[tokio::test]
    async fn atomic_apply_rejects_mismatched_session_store_key() {
        let store = InMemoryRuntimeStore::new();
        let session = meerkat_core::Session::new();
        let rid = LogicalRuntimeId::for_session(session.id());
        let wrong_session_id = meerkat_core::Session::new().id().clone();
        let snapshot = serde_json::to_vec(&session).unwrap();

        let err = store
            .atomic_apply(
                &rid,
                Some(SerializedSessionSnapshot {
                    session_snapshot: snapshot.into(),
                }),
                make_receipt(RunId::new(), 0),
                vec![],
                Some(wrong_session_id),
            )
            .await
            .expect_err("mismatched session_store_key should fail");

        assert!(matches!(err, RuntimeStoreError::SessionKeyMismatch { .. }));
        assert!(store.load_session_snapshot(&rid).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn typed_whole_blob_snapshot_cas_uses_exact_store_authority_and_can_compensate() {
        let store = InMemoryRuntimeStore::new();
        let mut predecessor = meerkat_core::Session::new();
        predecessor.append_system_message("before".to_string());
        let runtime_id = LogicalRuntimeId::for_session(predecessor.id());
        store
            .commit_session_snapshot(
                &runtime_id,
                SerializedSessionSnapshot {
                    session_snapshot: serde_json::to_vec(&predecessor).unwrap().into(),
                },
            )
            .await
            .unwrap();

        let committed = store
            .load_committed_whole_blob_snapshot(&runtime_id)
            .await
            .unwrap()
            .unwrap();
        let base_authority = committed.authority().clone();
        let predecessor = committed.session_arc();
        let predecessor_messages = predecessor.messages().to_vec();
        let mut successor = predecessor.as_ref().clone();
        successor.append_system_message("after".to_string());
        let prepared = PreparedWholeBlobSnapshotCas::prepare(
            base_authority.clone(),
            meerkat_core::lifecycle::core_executor::BoundSessionCommit::sealed(Arc::new(successor))
                .unwrap(),
        )
        .unwrap();
        let target_authority = match store
            .commit_prepared_whole_blob_snapshot_cas(&runtime_id, prepared.clone())
            .await
            .unwrap()
        {
            WholeBlobSnapshotCasOutcome::Committed(authority) => authority,
            WholeBlobSnapshotCasOutcome::Conflict => panic!("exact predecessor must commit"),
        };
        assert!(prepared.accepts_committed_authority(&target_authority));

        let stale = PreparedWholeBlobSnapshotCas::prepare(
            base_authority,
            meerkat_core::lifecycle::core_executor::BoundSessionCommit::sealed(Arc::clone(
                &predecessor,
            ))
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            store
                .commit_prepared_whole_blob_snapshot_cas(&runtime_id, stale)
                .await
                .unwrap(),
            WholeBlobSnapshotCasOutcome::Conflict
        );

        let compensation = PreparedWholeBlobSnapshotCas::prepare(
            target_authority,
            meerkat_core::lifecycle::core_executor::BoundSessionCommit::sealed(predecessor)
                .unwrap(),
        )
        .unwrap();
        let restored = store
            .commit_prepared_whole_blob_snapshot_cas(&runtime_id, compensation.clone())
            .await
            .unwrap();
        let WholeBlobSnapshotCasOutcome::Committed(restored_authority) = restored else {
            panic!("exact target authority must permit compensation");
        };
        assert!(compensation.accepts_committed_authority(&restored_authority));
        let restored = store
            .load_committed_whole_blob_snapshot(&runtime_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            restored.session().messages(),
            predecessor_messages.as_slice()
        );
    }

    #[tokio::test]
    async fn atomic_apply_persists_machine_owned_receipt() {
        let store = InMemoryRuntimeStore::new();
        let run_id = RunId::new();
        let input_id = InputId::new();
        let session = meerkat_core::Session::new();
        let rid = LogicalRuntimeId::for_session(session.id());
        let snapshot = serde_json::to_vec(&session).unwrap();
        let receipt = RunBoundaryReceipt {
            run_id: run_id.clone(),
            boundary: RunApplyBoundary::Immediate,
            contributing_input_ids: vec![input_id.clone()],
            conversation_digest: Some("machine-owned-digest".to_string()),
            message_count: 42,
            sequence: 7,
        };

        store
            .atomic_apply(
                &rid,
                Some(SerializedSessionSnapshot {
                    session_snapshot: snapshot.into(),
                }),
                receipt.clone(),
                vec![persistable(StoredInputState::new_accepted(input_id))],
                None,
            )
            .await
            .unwrap();

        assert_eq!(receipt.run_id, run_id);
        assert!(receipt.conversation_digest.is_some());
        let loaded = store
            .load_boundary_receipt(&rid, &receipt.run_id, receipt.sequence)
            .await
            .unwrap();
        assert!(loaded.is_some(), "receipt should be persisted");
        let Some(loaded) = loaded else {
            unreachable!("asserted above");
        };
        assert_eq!(loaded, receipt);
    }

    #[tokio::test]
    async fn multiple_runtimes_isolated() {
        let store = InMemoryRuntimeStore::new();
        let rid1 = LogicalRuntimeId::new("runtime-1");
        let rid2 = LogicalRuntimeId::new("runtime-2");

        store
            .persist_input_state(
                &rid1,
                &persistable(StoredInputState::new_accepted(InputId::new())),
            )
            .await
            .unwrap();
        store
            .persist_input_state(
                &rid2,
                &persistable(StoredInputState::new_accepted(InputId::new())),
            )
            .await
            .unwrap();
        store
            .persist_input_state(
                &rid2,
                &persistable(StoredInputState::new_accepted(InputId::new())),
            )
            .await
            .unwrap();

        let s1 = store.load_input_states_strict(&rid1).await.unwrap();
        let s2 = store.load_input_states_strict(&rid2).await.unwrap();
        assert_eq!(s1.len(), 1);
        assert_eq!(s2.len(), 2);
    }

    #[tokio::test]
    async fn load_session_snapshot_roundtrip() {
        let store = InMemoryRuntimeStore::new();
        let session = meerkat_core::Session::new();
        let rid = LogicalRuntimeId::for_session(session.id());
        let snapshot = serde_json::to_vec(&session).unwrap();

        store
            .atomic_apply(
                &rid,
                Some(SerializedSessionSnapshot {
                    session_snapshot: snapshot.clone().into(),
                }),
                make_receipt(RunId::new(), 0),
                vec![],
                None,
            )
            .await
            .unwrap();

        let loaded = store.load_session_snapshot(&rid).await.unwrap();
        assert_eq!(loaded, Some(Arc::new(snapshot)));
    }

    #[tokio::test]
    async fn typed_whole_blob_snapshot_cas_rejects_stale_runtime_parent() {
        let store = InMemoryRuntimeStore::new();
        let accepted = session_with_user("accepted runtime turn");
        let rid = LogicalRuntimeId::for_session(accepted.id());
        let mut stale = meerkat_core::Session::with_id(accepted.id().clone());
        stale.push(meerkat_core::types::Message::User(
            meerkat_core::types::UserMessage::text("stale runtime turn".to_string()),
        ));
        let accepted_snapshot = serde_json::to_vec(&accepted).unwrap();

        store
            .commit_session_snapshot(
                &rid,
                SerializedSessionSnapshot {
                    session_snapshot: accepted_snapshot.clone().into(),
                },
            )
            .await
            .unwrap();

        let base = store
            .load_committed_whole_blob_snapshot(&rid)
            .await
            .unwrap()
            .unwrap()
            .authority()
            .clone();
        let mut advanced = accepted.clone();
        advanced.push(meerkat_core::types::Message::User(
            meerkat_core::types::UserMessage::text("accepted continuation"),
        ));
        let advanced_snapshot = serde_json::to_vec(&advanced).unwrap();
        let advance = PreparedWholeBlobSnapshotCas::prepare(
            base.clone(),
            meerkat_core::lifecycle::core_executor::BoundSessionCommit::sealed(Arc::new(advanced))
                .unwrap(),
        )
        .unwrap();
        assert!(matches!(
            store
                .commit_prepared_whole_blob_snapshot_cas(&rid, advance)
                .await
                .unwrap(),
            WholeBlobSnapshotCasOutcome::Committed(_)
        ));

        let stale = PreparedWholeBlobSnapshotCas::prepare(
            base,
            meerkat_core::lifecycle::core_executor::BoundSessionCommit::sealed(Arc::new(stale))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            store
                .commit_prepared_whole_blob_snapshot_cas(&rid, stale)
                .await
                .unwrap(),
            WholeBlobSnapshotCasOutcome::Conflict
        );
        assert_eq!(
            store.load_session_snapshot(&rid).await.unwrap(),
            Some(Arc::new(advanced_snapshot))
        );
    }

    #[tokio::test]
    async fn atomic_apply_commits_target_state_and_receipt_as_one_boundary() {
        let store = InMemoryRuntimeStore::new();
        let incoming = session_with_user("turn input");
        let rid = LogicalRuntimeId::for_session(incoming.id());
        let mut current = incoming.clone();
        current.push(meerkat_core::types::Message::BlockAssistant(
            meerkat_core::types::BlockAssistantMessage {
                blocks: vec![meerkat_core::types::AssistantBlock::Text {
                    text: "peer response already applied".to_string(),
                    meta: None,
                }],
                stop_reason: meerkat_core::types::StopReason::EndTurn,
                identity: meerkat_core::types::TranscriptMessageIdentity::default(),
                created_at: meerkat_core::types::message_timestamp_now(),
            },
        ));
        let current_snapshot = serde_json::to_vec(&current).unwrap();
        let incoming_snapshot = serde_json::to_vec(&incoming).unwrap();
        let receipt = make_receipt(RunId::new(), 11);

        store
            .commit_session_snapshot(
                &rid,
                SerializedSessionSnapshot {
                    session_snapshot: current_snapshot.clone().into(),
                },
            )
            .await
            .unwrap();

        store
            .atomic_apply(
                &rid,
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
            store.load_session_snapshot(&rid).await.unwrap(),
            Some(Arc::new(incoming_snapshot))
        );
        assert_eq!(
            store
                .load_boundary_receipt(&rid, &receipt.run_id, receipt.sequence)
                .await
                .unwrap(),
            Some(receipt)
        );
    }

    #[tokio::test]
    async fn atomic_apply_commits_target_state_receipt_and_inputs_as_one_boundary() {
        let store = InMemoryRuntimeStore::new();
        let incoming = session_with_user("turn input");
        let rid = LogicalRuntimeId::for_session(incoming.id());
        let mut current = incoming.clone();
        current.push(meerkat_core::types::Message::BlockAssistant(
            meerkat_core::types::BlockAssistantMessage {
                blocks: vec![meerkat_core::types::AssistantBlock::Text {
                    text: "peer response already applied".to_string(),
                    meta: None,
                }],
                stop_reason: meerkat_core::types::StopReason::EndTurn,
                identity: meerkat_core::types::TranscriptMessageIdentity::default(),
                created_at: meerkat_core::types::message_timestamp_now(),
            },
        ));
        let current_snapshot = serde_json::to_vec(&current).unwrap();
        let incoming_snapshot = serde_json::to_vec(&incoming).unwrap();
        let receipt = make_receipt(RunId::new(), 21);
        let input_id = InputId::new();
        let bundle = StoredInputState::new_accepted(input_id.clone());

        store
            .commit_session_snapshot(
                &rid,
                SerializedSessionSnapshot {
                    session_snapshot: current_snapshot.clone().into(),
                },
            )
            .await
            .unwrap();

        store
            .atomic_apply(
                &rid,
                Some(SerializedSessionSnapshot {
                    session_snapshot: incoming_snapshot.clone().into(),
                }),
                receipt.clone(),
                vec![persistable(bundle)],
                Some(incoming.id().clone()),
            )
            .await
            .unwrap();

        assert_eq!(
            store.load_session_snapshot(&rid).await.unwrap(),
            Some(Arc::new(incoming_snapshot))
        );
        assert_eq!(
            store
                .load_boundary_receipt(&rid, &receipt.run_id, receipt.sequence)
                .await
                .unwrap(),
            Some(receipt)
        );
        assert_eq!(
            store.load_input_states_strict(&rid).await.unwrap()[0]
                .state
                .input_id,
            input_id
        );
    }

    #[tokio::test]
    async fn atomic_apply_allows_first_generated_snapshot_after_placeholder() {
        let store = InMemoryRuntimeStore::new();
        let mut placeholder = meerkat_core::Session::new();
        let rid = LogicalRuntimeId::for_session(placeholder.id());
        placeholder.append_system_message("base system".to_string());
        let mut incoming = meerkat_core::Session::with_id(placeholder.id().clone());
        incoming.append_system_message("base system".to_string());
        incoming.push(meerkat_core::types::Message::User(
            meerkat_core::types::UserMessage::text("verbose first turn".to_string()),
        ));
        let parent_revision = incoming.transcript_revision().unwrap();
        incoming
            .commit_transcript_rewrite(
                meerkat_core::TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                vec![meerkat_core::types::Message::User(
                    meerkat_core::types::UserMessage::compaction_summary(
                        "[Context compacted] first turn",
                    ),
                )],
                meerkat_core::TranscriptRewriteReason::new("compaction"),
                Some("meerkat-core".to_string()),
                Some(parent_revision),
            )
            .unwrap();
        let incoming_snapshot = serde_json::to_vec(&incoming).unwrap();
        let receipt = make_receipt(RunId::new(), 12);

        store
            .commit_session_snapshot(
                &rid,
                SerializedSessionSnapshot {
                    session_snapshot: serde_json::to_vec(&placeholder).unwrap().into(),
                },
            )
            .await
            .unwrap();

        store
            .atomic_apply(
                &rid,
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
            store.load_session_snapshot(&rid).await.unwrap(),
            Some(Arc::new(incoming_snapshot))
        );
        assert_eq!(
            store
                .load_boundary_receipt(&rid, &receipt.run_id, receipt.sequence)
                .await
                .unwrap(),
            Some(receipt)
        );
    }

    #[tokio::test]
    async fn atomic_apply_allows_generated_compaction_before_retained_tail() {
        let store = InMemoryRuntimeStore::new();
        let mut previous = meerkat_core::Session::new();
        let rid = LogicalRuntimeId::for_session(previous.id());
        previous.append_system_message("runtime system before context refresh".to_string());
        previous.push(meerkat_core::types::Message::User(
            meerkat_core::types::UserMessage::text("Turn 1 request".to_string()),
        ));
        previous.push(meerkat_core::types::Message::BlockAssistant(
            meerkat_core::types::BlockAssistantMessage {
                blocks: vec![meerkat_core::types::AssistantBlock::Text {
                    text: "Turn 1 answer".to_string(),
                    meta: None,
                }],
                stop_reason: meerkat_core::types::StopReason::EndTurn,
                identity: meerkat_core::types::TranscriptMessageIdentity::default(),
                created_at: meerkat_core::types::message_timestamp_now(),
            },
        ));

        let mut incoming = meerkat_core::Session::with_id(previous.id().clone());
        incoming.append_system_message("runtime system after context refresh".to_string());
        incoming.push(meerkat_core::types::Message::User(
            meerkat_core::types::UserMessage::text(
                "Verbose context that will be compacted".to_string(),
            ),
        ));
        for message in previous.messages()[1..].iter().cloned() {
            incoming.push(message);
        }
        incoming.push(meerkat_core::types::Message::BlockAssistant(
            meerkat_core::types::BlockAssistantMessage {
                blocks: vec![meerkat_core::types::AssistantBlock::Text {
                    text: "Turn 2 generated answer".to_string(),
                    meta: None,
                }],
                stop_reason: meerkat_core::types::StopReason::EndTurn,
                identity: meerkat_core::types::TranscriptMessageIdentity::default(),
                created_at: meerkat_core::types::message_timestamp_now(),
            },
        ));
        let parent_revision = incoming.transcript_revision().unwrap();
        incoming
            .commit_transcript_rewrite(
                meerkat_core::TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                vec![meerkat_core::types::Message::User(
                    meerkat_core::types::UserMessage::compaction_summary(
                        "[Context compacted] Earlier runtime context".to_string(),
                    ),
                )],
                meerkat_core::TranscriptRewriteReason::new("compaction"),
                Some("meerkat-core".to_string()),
                Some(parent_revision),
            )
            .unwrap();
        let incoming_snapshot = serde_json::to_vec(&incoming).unwrap();
        let receipt = make_receipt(RunId::new(), 13);

        store
            .commit_session_snapshot(
                &rid,
                SerializedSessionSnapshot {
                    session_snapshot: serde_json::to_vec(&previous).unwrap().into(),
                },
            )
            .await
            .unwrap();

        store
            .atomic_apply(
                &rid,
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
            store.load_session_snapshot(&rid).await.unwrap(),
            Some(Arc::new(incoming_snapshot))
        );
        assert_eq!(
            store
                .load_boundary_receipt(&rid, &receipt.run_id, receipt.sequence)
                .await
                .unwrap(),
            Some(receipt)
        );
    }

    #[tokio::test]
    async fn commit_machine_lifecycle_persists_binding_facts() {
        use crate::runtime_state::RuntimeState;

        let store = InMemoryRuntimeStore::new();
        let rid = LogicalRuntimeId::new("runtime-binding");
        let binding = MachineLifecycleBindingFacts::new(
            Some("rt:session:abc".to_string()),
            Some(7),
            Some(3),
            Some("epoch-1".to_string()),
        );

        store
            .commit_machine_lifecycle(
                &rid,
                MachineLifecycleCommit::new_with_binding(
                    RuntimeState::Retired,
                    binding.clone(),
                    crate::store::SupervisorAuthoritySnapshot::UnboundNoReceipt,
                ),
                &[],
            )
            .await
            .unwrap();

        let lifecycle = crate::store::load_machine_lifecycle(&store, &rid)
            .await
            .unwrap()
            .expect("machine lifecycle snapshot");
        assert_eq!(lifecycle.runtime_state(), RuntimeState::Retired);
        assert_eq!(lifecycle.binding(), &binding);
        assert_eq!(
            crate::store::load_runtime_state(&store, &rid)
                .await
                .unwrap(),
            Some(RuntimeState::Retired)
        );
    }

    #[tokio::test]
    async fn lifecycle_publications_advance_existing_session_catalog() {
        let store = InMemoryRuntimeStore::new();
        let session = session_with_user("catalog lifecycle");
        let runtime_id = LogicalRuntimeId::for_session(session.id());
        store
            .commit_session_snapshot(
                &runtime_id,
                SerializedSessionSnapshot {
                    session_snapshot: session.to_persisted_bytes().unwrap().into(),
                },
            )
            .await
            .unwrap();
        assert_eq!(
            store
                .load_runtime_session_catalog_entry(&runtime_id)
                .await
                .unwrap()
                .unwrap()
                .runtime_state(),
            None
        );

        let MachineLifecycleCasOutcome::Applied { .. } = store
            .compare_and_swap_machine_lifecycle(
                &runtime_id,
                MachineLifecycleExpectedVersion::Missing,
                lifecycle_commit(&runtime_id, RuntimeState::Idle, 7, 3),
            )
            .await
            .unwrap()
        else {
            panic!("missing lifecycle must be installed");
        };
        assert_eq!(
            store
                .load_runtime_session_catalog_entry(&runtime_id)
                .await
                .unwrap()
                .unwrap()
                .runtime_state(),
            Some(RuntimeState::Idle)
        );

        store
            .commit_machine_lifecycle(
                &runtime_id,
                lifecycle_commit(&runtime_id, RuntimeState::Retired, 8, 4),
                &[],
            )
            .await
            .unwrap();
        assert_eq!(
            store
                .load_runtime_session_catalog_entry(&runtime_id)
                .await
                .unwrap()
                .unwrap()
                .runtime_state(),
            Some(RuntimeState::Retired)
        );

        store
            .inner
            .lock()
            .await
            .session_catalog
            .get_mut(&runtime_id.0)
            .expect("catalog entry")
            .set_runtime_state(Some(RuntimeState::Idle));
        let MachineLifecycleObservation::Decoded { version, .. } =
            store.observe_machine_lifecycle(&runtime_id).await.unwrap()
        else {
            panic!("retired lifecycle must decode");
        };
        assert!(matches!(
            store
                .compare_and_swap_machine_lifecycle_with_fence(
                    &runtime_id,
                    MachineLifecycleExpectedVersion::Version(version),
                    lifecycle_commit(&runtime_id, RuntimeState::Retired, 8, 4),
                    Arc::new(AppliedWriteFence),
                )
                .await
                .unwrap(),
            FencedMachineLifecycleCasOutcome::AlreadyExact { .. }
        ));
        assert_eq!(
            store
                .load_runtime_session_catalog_entry(&runtime_id)
                .await
                .unwrap()
                .unwrap()
                .runtime_state(),
            Some(RuntimeState::Retired),
            "applied already-exact fence must heal a stale catalog"
        );

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
        store
            .commit_unregister_finalization(
                &runtime_id,
                crate::store::UnregisterFinalizationCommit::new(
                    lifecycle_commit(&runtime_id, RuntimeState::Destroyed, 9, 5),
                    vec![],
                    ops_snapshot.epoch_id,
                    crate::meerkat_machine::DeleteOpsFinalizationAuthority::for_store_test(),
                ),
            )
            .await
            .unwrap();
        assert_eq!(
            store
                .load_runtime_session_catalog_entry(&runtime_id)
                .await
                .unwrap()
                .unwrap()
                .runtime_state(),
            Some(RuntimeState::Destroyed)
        );
    }

    #[tokio::test]
    async fn concurrent_ops_initializers_return_one_canonical_snapshot() {
        let store = InMemoryRuntimeStore::new();
        let runtime_id = LogicalRuntimeId::new("runtime-concurrent-ops-initialize");
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
            store.initialize_ops_lifecycle_if_absent(&runtime_id, &first_candidate),
            store.initialize_ops_lifecycle_if_absent(&runtime_id, &second_candidate),
        );
        let first = first.unwrap();
        let second = second.unwrap();

        assert_eq!(first.epoch_id, second.epoch_id);
        assert_eq!(
            store
                .load_ops_lifecycle(&runtime_id)
                .await
                .unwrap()
                .expect("canonical snapshot")
                .epoch_id,
            first.epoch_id
        );
    }

    #[tokio::test]
    async fn unregister_finalization_atomically_retires_ops_epoch_and_is_idempotent() {
        let store = InMemoryRuntimeStore::new();
        let reopened = store.clone();
        let runtime_id = LogicalRuntimeId::new("runtime-unregister-finalization");
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

        for _ in 0..2 {
            store
                .commit_unregister_finalization(
                    &runtime_id,
                    crate::store::UnregisterFinalizationCommit::new(
                        MachineLifecycleCommit::new_with_binding(
                            RuntimeState::Stopped,
                            MachineLifecycleBindingFacts::new(None, None, None, None),
                            crate::store::SupervisorAuthoritySnapshot::UnboundNoReceipt,
                        ),
                        vec![],
                        retired_ops_epoch.clone(),
                        crate::meerkat_machine::DeleteOpsFinalizationAuthority::for_store_test(),
                    ),
                )
                .await
                .unwrap();
        }

        assert_eq!(
            crate::store::load_runtime_state(&reopened, &runtime_id)
                .await
                .unwrap(),
            Some(RuntimeState::Stopped)
        );
        assert!(
            reopened
                .load_ops_lifecycle(&runtime_id)
                .await
                .unwrap()
                .is_none(),
            "the same critical section that publishes terminal lifecycle must remove the ops epoch"
        );
        let late_error = reopened
            .persist_ops_lifecycle(&runtime_id, &stale_ops)
            .await
            .expect_err("a detached callback must not resurrect its retired ops epoch");
        assert!(matches!(
            late_error,
            RuntimeStoreError::OpsLifecycleEpochRetired { epoch_id, .. }
                if epoch_id == retired_ops_epoch
        ));
        assert!(matches!(
            reopened
                .initialize_ops_lifecycle_if_absent(&runtime_id, &stale_ops)
                .await
                .expect_err("initialization must honor the same retired-epoch fence"),
            RuntimeStoreError::OpsLifecycleEpochRetired { epoch_id, .. }
                if epoch_id == retired_ops_epoch
        ));
        assert!(
            reopened
                .load_ops_lifecycle(&runtime_id)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn delayed_old_epoch_finalizer_cannot_delete_or_overwrite_new_ops_epoch() {
        let store = InMemoryRuntimeStore::new();
        let runtime_id = LogicalRuntimeId::new("runtime-old-finalizer-new-epoch");
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

        store
            .commit_unregister_finalization(
                &runtime_id,
                crate::store::UnregisterFinalizationCommit::new(
                    MachineLifecycleCommit::new_with_binding(
                        RuntimeState::Stopped,
                        MachineLifecycleBindingFacts::new(None, None, None, None),
                        crate::store::SupervisorAuthoritySnapshot::UnboundNoReceipt,
                    ),
                    vec![],
                    old_ops.epoch_id.clone(),
                    crate::meerkat_machine::DeleteOpsFinalizationAuthority::for_store_test(),
                ),
            )
            .await
            .unwrap();

        assert_eq!(
            store
                .load_ops_lifecycle(&runtime_id)
                .await
                .unwrap()
                .expect("new epoch row must survive delayed old finalization")
                .epoch_id,
            new_ops.epoch_id
        );
        assert!(matches!(
            store
                .persist_ops_lifecycle(&runtime_id, &old_ops)
                .await
                .expect_err("retired old epoch stays fenced"),
            RuntimeStoreError::OpsLifecycleEpochRetired { .. }
        ));
        store
            .persist_ops_lifecycle(&runtime_id, &new_ops)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn clear_session_snapshot_if_current_sets_quarantine_marker_cleared_on_write() {
        let store = InMemoryRuntimeStore::new();
        let rejected_session = session_with_user("rejected");
        let rid = LogicalRuntimeId::for_session(rejected_session.id());
        let rejected = serde_json::to_vec(&rejected_session).unwrap();

        assert!(!store.is_runtime_projection_quarantined(&rid).await.unwrap());
        store
            .commit_session_snapshot(
                &rid,
                SerializedSessionSnapshot {
                    session_snapshot: rejected.clone().into(),
                },
            )
            .await
            .unwrap();
        assert!(
            store
                .clear_session_snapshot_if_current(&rid, &rejected)
                .await
                .unwrap()
        );
        assert!(
            store.is_runtime_projection_quarantined(&rid).await.unwrap(),
            "clearing the rejected snapshot must record the in-memory quarantine marker"
        );

        // A live snapshot write reclaims runtime authority and clears the marker.
        let mut revived = meerkat_core::Session::with_id(rejected_session.id().clone());
        revived.push(meerkat_core::Message::User(
            meerkat_core::types::UserMessage::text("revived"),
        ));
        store
            .commit_session_snapshot(
                &rid,
                SerializedSessionSnapshot {
                    session_snapshot: serde_json::to_vec(&revived).unwrap().into(),
                },
            )
            .await
            .unwrap();
        assert!(
            !store.is_runtime_projection_quarantined(&rid).await.unwrap(),
            "a live snapshot write must clear the in-memory quarantine marker"
        );
    }

    #[tokio::test]
    async fn lifecycle_observation_and_missing_or_version_cas_are_target_local() {
        let store = InMemoryRuntimeStore::new();
        let runtime_id = LogicalRuntimeId::new("runtime-lifecycle-cas");
        let other_runtime_id = LogicalRuntimeId::new("runtime-lifecycle-other");
        assert_eq!(
            store.observe_machine_lifecycle(&runtime_id).await.unwrap(),
            MachineLifecycleObservation::Missing
        );

        let MachineLifecycleCasOutcome::Applied { version } = store
            .compare_and_swap_machine_lifecycle(
                &runtime_id,
                MachineLifecycleExpectedVersion::Missing,
                lifecycle_commit(&runtime_id, RuntimeState::Idle, 7, 3),
            )
            .await
            .unwrap()
        else {
            panic!("missing row must be inserted");
        };
        let observed = store.observe_machine_lifecycle(&runtime_id).await.unwrap();
        let MachineLifecycleObservation::Decoded {
            record,
            version: observed_version,
        } = &observed
        else {
            panic!("committed lifecycle row must decode");
        };
        assert_eq!(observed_version, &version);
        assert_eq!(record.runtime_state(), Some(RuntimeState::Idle));
        assert_eq!(record.binding().fence_token(), Some(7));

        let conflict = store
            .compare_and_swap_machine_lifecycle(
                &runtime_id,
                MachineLifecycleExpectedVersion::Missing,
                lifecycle_commit(&runtime_id, RuntimeState::Stopped, 8, 4),
            )
            .await
            .unwrap();
        assert_eq!(
            conflict,
            MachineLifecycleCasOutcome::Conflict {
                current: observed.clone()
            }
        );
        assert_eq!(
            store
                .observe_machine_lifecycle(&other_runtime_id)
                .await
                .unwrap(),
            MachineLifecycleObservation::Missing
        );

        assert!(matches!(
            store
                .compare_and_swap_machine_lifecycle(
                    &runtime_id,
                    MachineLifecycleExpectedVersion::Version(version),
                    lifecycle_commit(&runtime_id, RuntimeState::Stopped, 8, 4),
                )
                .await
                .unwrap(),
            MachineLifecycleCasOutcome::Applied { .. }
        ));
    }

    #[tokio::test]
    async fn malformed_lifecycle_repair_is_blocked_even_with_apparent_highwater() {
        let store = InMemoryRuntimeStore::new();
        let runtime_id = LogicalRuntimeId::new("runtime-malformed-lifecycle");
        let raw = serde_json::to_vec(&serde_json::json!({
            "record_version": crate::store::MACHINE_LIFECYCLE_STORE_RECORD_VERSION,
            "runtime_state": "idle",
            "binding": {
                "agent_runtime_id": runtime_id.0.clone(),
                "fence_token": 9,
                "runtime_generation": 5,
                "runtime_epoch_id": "epoch-5"
            },
            "current_run_id": null,
            "pre_run_phase": null,
            "unregister_progress": null
        }))
        .unwrap();
        store
            .inner
            .lock()
            .await
            .runtime_lifecycle
            .insert(runtime_id.0.clone(), raw.clone());

        let observed = store.observe_machine_lifecycle(&runtime_id).await.unwrap();
        let MachineLifecycleObservation::Malformed { version, .. } = observed else {
            panic!("structurally incomplete row must remain malformed evidence");
        };
        assert!(matches!(
            store
                .compare_and_swap_machine_lifecycle(
                    &runtime_id,
                    MachineLifecycleExpectedVersion::Version(version.clone()),
                    lifecycle_commit(&runtime_id, RuntimeState::Idle, 8, 5),
                )
                .await
                .expect_err("repair must not lower an independently readable fence"),
            RuntimeStoreError::MachineLifecycleRepairBlocked { .. }
        ));
        assert_eq!(
            store
                .load_machine_lifecycle_record(&runtime_id)
                .await
                .unwrap(),
            Some(raw.clone())
        );

        assert!(matches!(
            store
                .compare_and_swap_machine_lifecycle(
                    &runtime_id,
                    MachineLifecycleExpectedVersion::Version(version),
                    lifecycle_commit(&runtime_id, RuntimeState::Idle, 10, 6),
                )
                .await
                .expect_err("decodable fragments inside malformed bytes are not repair authority"),
            RuntimeStoreError::MachineLifecycleRepairBlocked { .. }
        ));
        assert_eq!(
            store
                .load_machine_lifecycle_record(&runtime_id)
                .await
                .unwrap(),
            Some(raw)
        );
    }

    #[tokio::test]
    async fn malformed_lifecycle_duplicate_highwater_keys_are_repair_blocked() {
        let store = InMemoryRuntimeStore::new();
        let runtime_id = LogicalRuntimeId::new("runtime-duplicate-lifecycle-fence");
        let raw = format!(
            r#"{{"record_version":4,"runtime_state":"idle","binding":{{"agent_runtime_id":"{}","fence_token":99,"fence_token":1,"runtime_generation":3,"runtime_epoch_id":"epoch-3"}},"current_run_id":null,"pre_run_phase":null,"supervisor_authority":{{"kind":"unbound_no_receipt"}},"unregister_progress":null}}"#,
            runtime_id.0
        )
        .into_bytes();
        store
            .inner
            .lock()
            .await
            .runtime_lifecycle
            .insert(runtime_id.0.clone(), raw.clone());
        let MachineLifecycleObservation::Malformed { version, .. } =
            store.observe_machine_lifecycle(&runtime_id).await.unwrap()
        else {
            panic!("duplicate high-water keys must classify as malformed");
        };

        assert!(matches!(
            store
                .compare_and_swap_machine_lifecycle(
                    &runtime_id,
                    MachineLifecycleExpectedVersion::Version(version),
                    lifecycle_commit(&runtime_id, RuntimeState::Idle, 2, 3),
                )
                .await
                .expect_err("ambiguous duplicate high-water must block repair"),
            RuntimeStoreError::MachineLifecycleRepairBlocked { .. }
        ));
        assert_eq!(
            store
                .load_machine_lifecycle_record(&runtime_id)
                .await
                .unwrap(),
            Some(raw)
        );
    }

    /// Replace one occurrence of `needle` so the fixture differs in content
    /// but not in serialized length.
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
    async fn commit_session_snapshot_growth_issues_distinct_store_authority() {
        let store = InMemoryRuntimeStore::new();
        let mut session = meerkat_core::Session::new();
        let rid = LogicalRuntimeId::for_session(session.id());
        session.push(meerkat_core::Message::User(
            meerkat_core::types::UserMessage::text("first turn".to_string()),
        ));
        store
            .commit_session_snapshot(
                &rid,
                SerializedSessionSnapshot {
                    session_snapshot: serde_json::to_vec(&session).unwrap().into(),
                },
            )
            .await
            .unwrap();
        let initial_authority = store
            .load_whole_blob_store_authority(&rid)
            .await
            .unwrap()
            .unwrap();

        session.push(meerkat_core::Message::User(
            meerkat_core::types::UserMessage::text("second turn grows the document".to_string()),
        ));
        let grown = serde_json::to_vec(&session).unwrap();
        store
            .commit_session_snapshot(
                &rid,
                SerializedSessionSnapshot {
                    session_snapshot: grown.clone().into(),
                },
            )
            .await
            .unwrap();

        let grown_authority = store
            .load_whole_blob_store_authority(&rid)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            grown_authority.store_revision(),
            initial_authority.store_revision() + 1
        );
        assert_ne!(
            grown_authority.blob_sha256(),
            initial_authority.blob_sha256()
        );
        assert_eq!(
            store.load_session_snapshot(&rid).await.unwrap(),
            Some(Arc::new(grown))
        );
    }

    #[tokio::test]
    async fn commit_session_snapshot_equal_length_different_bytes_issues_distinct_store_authority()
    {
        let store = InMemoryRuntimeStore::new();
        let mut session = meerkat_core::Session::new();
        let rid = LogicalRuntimeId::for_session(session.id());
        session.push(meerkat_core::Message::User(
            meerkat_core::types::UserMessage::text("probe".to_string()),
        ));
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
                &rid,
                SerializedSessionSnapshot {
                    session_snapshot: first.into(),
                },
            )
            .await
            .unwrap();
        let first_authority = store
            .load_whole_blob_store_authority(&rid)
            .await
            .unwrap()
            .unwrap();
        store
            .commit_session_snapshot(
                &rid,
                SerializedSessionSnapshot {
                    session_snapshot: second.clone().into(),
                },
            )
            .await
            .unwrap();

        let second_authority = store
            .load_whole_blob_store_authority(&rid)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            second_authority.store_revision(),
            first_authority.store_revision() + 1
        );
        assert_ne!(
            second_authority.blob_sha256(),
            first_authority.blob_sha256()
        );
        assert_eq!(
            store.load_session_snapshot(&rid).await.unwrap(),
            Some(Arc::new(second)),
            "length-equal but different content must mint a distinct store-owned authority"
        );
    }
}
