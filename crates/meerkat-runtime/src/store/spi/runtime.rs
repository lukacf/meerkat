//! [`SpiRuntimeStore`]: the in-memory runtime store's semantics, written
//! through to a host store before every answer.
//!
//! Every method runs the in-memory store's own implementation
//! ([`InMemoryRuntimeStore`], WholeBlobV1) under the locks of the scopes
//! its arguments name, inside a change capture ([`journal::capture`]):
//!
//! 1. take the realm gate shared (cross-scope reads take it exclusive);
//! 2. a mutation checks the self-fence first ([`SpiRealm::ensure_lease`]),
//!    so a writer past its window changes nothing;
//! 3. lock the named scopes in order, refuse poisoned ones, and load a body
//!    that is not resident yet; a mutation checks the self-fence again,
//!    since the lock waits may have outlasted the window;
//! 4. run the in-memory method, recording every map entry it touches; from
//!    here until the hand-off, a dropped call poisons the locked scopes
//!    ([`ArmedScopes`]);
//! 5. refuse and poison when it touched a scope it did not lock;
//! 6. export the touched entries and hand them, with the scopes and the
//!    gate, to a task that commits them to the host as one batch
//!    ([`SpiRealm::commit`]) and adopts it, or poisons, whether or not the
//!    caller still waits; the call answers once it settled.
//!
//! Which scopes each method names is written beside it; the capture, not the
//! list, decides what is committed, and a touch outside the list is a typed
//! refusal.
//!
//! **One call, one batch.** Every change one in-memory call makes to a
//! runtime lands in the same host batch. In particular a run boundary commit
//! removes the run's provisional tail and consumes its inputs in ONE batch,
//! as the SQLite store does in one transaction: a resumable run's boundary
//! has therefore never committed while its tail survives, and the input
//! requeue on a release relies on it. Mob host bindings and their revocations are this adapter's own
//! record scopes (the in-memory store does not support them); they follow
//! the SQLite store's semantics exactly (`SqliteRuntimeStore`'s
//! `*_mob_host_*` methods).

use std::collections::{BTreeMap, HashSet};
use std::sync::{Arc, Mutex as StdMutex};

use meerkat_core::host_store::ScopeKind;
use meerkat_core::lifecycle::{InputId, RunBoundaryReceipt, RunId};
use tokio::sync::RwLock;

use super::SpiError;
use super::realm::{
    ArmedScopes, ScopeDelta, ScopeFamily, ScopeGuard, ScopeId, ScopeState, SpiRealm,
};
use crate::identifiers::{IdempotencyKey, LogicalRuntimeId};
use crate::input_state::{InputStatePersistenceRecord, StoredInputState};
use crate::ops_lifecycle::PersistedOpsSnapshot;
use crate::store::memory::journal::{self, TouchScope, Touches};
use crate::store::memory::journal_image::{self, BOOT_FACTS_KEY, BootFactsV1, ScopeExport};
use crate::store::{
    AuthOAuthFlowSnapshotUpdate, CommittedWholeBlobProvisionalTail, CommittedWholeBlobSnapshot,
    ContinuationAdmission, ContinuationAdmissionOutcome, ContinuationAdmissionTransition,
    ContinuationKeyBinding, ExactInputStateObservation, FencedInputStateBatchCasOutcome,
    FencedMachineLifecycleCasOutcome, InMemoryRuntimeStore,
    InputStateBatchCasImplementationProfile, InputStateBatchCasOutcome, InputStateRow,
    KeyedRuntimeDeliveryCasOutcome, MachineLifecycleCasOutcome, MachineLifecycleCommit,
    MachineLifecycleExpectedVersion, MachineLifecycleObservation, PreparedRecoveryInputSnapshot,
    PreparedWholeBlobProvisionalTail, PreparedWholeBlobRewriteStoreParts,
    PreparedWholeBlobSnapshotCas, RecoveryInputSetRevision, RecoveryInputStateMutation,
    RuntimeDeliveryAuthorityCasOutcome, RuntimeDeliveryAuthorityRecord, RuntimeDeliveryStoreRecord,
    RuntimeSessionAuthority, RuntimeSessionAuthorityOps, RuntimeSessionAuthorityReadCost,
    RuntimeSessionCatalogEntry, RuntimeSessionResumeObservation, RuntimeStore, RuntimeStoreError,
    RuntimeStoreWriteFence, SerializedSessionSnapshot, WholeBlobProvisionalTailAuthority,
    WholeBlobSnapshotCasOutcome, WholeBlobStoreAuthority,
};

/// One runtime: a session scope. Its head pins the catalog entry, the
/// machine lifecycle, the delivery authority and the boot facts; its rows
/// carry the rest (see `journal_image`).
pub const RUNTIME_FAMILY: ScopeFamily = ScopeFamily {
    tag: "rt",
    kind: ScopeKind::Session,
    pinned: &["cat", "lc", "rda", BOOT_FACTS_KEY],
};

/// One continuation key binding: a record scope keyed by owner and key.
pub(crate) const KEY_BINDING_FAMILY: ScopeFamily = ScopeFamily {
    tag: "ck",
    kind: ScopeKind::Record,
    pinned: &["v"],
};

/// One direct member's incarnation high water: a record scope keyed by the
/// member session id.
pub(crate) const MEMBER_HIGH_WATER_FAMILY: ScopeFamily = ScopeFamily {
    tag: "dmh",
    kind: ScopeKind::Record,
    pinned: &["v"],
};

/// One mob's host binding record: a record scope keyed by the mob id.
pub(crate) const MOB_HOST_BINDING_FAMILY: ScopeFamily = ScopeFamily {
    tag: "mhb",
    kind: ScopeKind::Record,
    pinned: &["v"],
};

/// One mob's host revocation receipt: a record scope keyed by the mob id.
pub(crate) const MOB_HOST_REVOCATION_FAMILY: ScopeFamily = ScopeFamily {
    tag: "mhr",
    kind: ScopeKind::Record,
    pinned: &["v"],
};

/// The single entry of a record scope.
const RECORD_ENTRY: &str = "v";

fn runtime_id_scope(runtime_id: &LogicalRuntimeId) -> ScopeId {
    ScopeId::new(RUNTIME_FAMILY, runtime_id.0.clone())
}

/// A key binding's scope key: the owner's length, `:`, the owner, then the
/// key (injective over owner and key pairs).
fn binding_key(owner: &str, key: &str) -> String {
    format!("{}:{owner}{key}", owner.len())
}

fn split_binding_key(scope_key: &str) -> Result<(String, String), SpiError> {
    let (len, rest) = scope_key
        .split_once(':')
        .ok_or_else(|| SpiError::Corrupt(format!("key binding scope key {scope_key:?}")))?;
    let len: usize = len
        .parse()
        .map_err(|_| SpiError::Corrupt(format!("key binding scope key {scope_key:?}")))?;
    if !rest.is_char_boundary(len.min(rest.len())) || len > rest.len() {
        return Err(SpiError::Corrupt(format!(
            "key binding scope key {scope_key:?}"
        )));
    }
    let (owner, key) = rest.split_at(len);
    Ok((owner.to_string(), key.to_string()))
}

fn key_binding_scope(owner: &str, key: &str) -> ScopeId {
    ScopeId::new(KEY_BINDING_FAMILY, binding_key(owner, key))
}

fn member_high_water_scope(member_session_id: &str) -> ScopeId {
    ScopeId::new(MEMBER_HIGH_WATER_FAMILY, member_session_id)
}

fn mob_host_binding_scope(mob_id: &str) -> ScopeId {
    ScopeId::new(MOB_HOST_BINDING_FAMILY, mob_id)
}

fn mob_host_revocation_scope(mob_id: &str) -> ScopeId {
    ScopeId::new(MOB_HOST_REVOCATION_FAMILY, mob_id)
}

fn scope_id_of(scope: &TouchScope) -> ScopeId {
    match scope {
        TouchScope::Runtime(runtime_id) => ScopeId::new(RUNTIME_FAMILY, runtime_id.clone()),
        TouchScope::KeyBinding { owner, key } => key_binding_scope(owner, key),
        TouchScope::MemberHighWater(member) => member_high_water_scope(member),
    }
}

fn touch_scope_of(id: &ScopeId) -> Result<Option<TouchScope>, SpiError> {
    Ok(match id.family.tag {
        tag if tag == RUNTIME_FAMILY.tag => Some(TouchScope::Runtime(id.key.clone())),
        tag if tag == KEY_BINDING_FAMILY.tag => {
            let (owner, key) = split_binding_key(&id.key)?;
            Some(TouchScope::KeyBinding { owner, key })
        }
        tag if tag == MEMBER_HIGH_WATER_FAMILY.tag => {
            Some(TouchScope::MemberHighWater(id.key.clone()))
        }
        _ => None,
    })
}

fn memory_error(error: RuntimeStoreError) -> SpiError {
    SpiError::Corrupt(error.to_string())
}

/// Whether a runtime's body loads at boot: recovery must act on it.
/// An archived runtime never loads at boot.
fn needs_body_at_boot(pinned: &BTreeMap<String, Arc<Vec<u8>>>) -> Result<bool, SpiError> {
    if let Some(catalog) = pinned.get("cat") {
        let entry: RuntimeSessionCatalogEntry = serde_json::from_slice(catalog)
            .map_err(|error| SpiError::Corrupt(format!("catalog entry: {error}")))?;
        if entry
            .lifecycle_terminal()
            .is_some_and(meerkat_core::SessionLifecycleTerminal::is_archived)
        {
            return Ok(false);
        }
    }
    let facts = match pinned.get(BOOT_FACTS_KEY) {
        Some(bytes) => BootFactsV1::decode(bytes).map_err(memory_error)?,
        None => BootFactsV1::default(),
    };
    let lifecycle = pinned.get("lc").map(|bytes| bytes.as_slice());
    Ok(facts.nonterminal
        || facts.pending_owners
        || facts.tail
        || journal_image::lifecycle_has_current_run(lifecycle)
        || (facts.inputs && lifecycle.is_none()))
}

/// The changes one scope's export makes: every whole group's entries, the
/// group's committed entries it no longer has as removals, and the single
/// entries. A later change of one key replaces an earlier one.
fn changes_of(state: &ScopeState, export: &ScopeExport) -> Vec<(String, Option<Arc<Vec<u8>>>)> {
    let mut changes: BTreeMap<String, Option<Arc<Vec<u8>>>> = BTreeMap::new();
    for (group, entries) in &export.whole {
        let present: HashSet<&str> = entries.iter().map(|(key, _)| key.as_str()).collect();
        for key in state.entry_keys_in_group(group) {
            if !present.contains(key.as_str()) {
                changes.insert(key, None);
            }
        }
        for (key, value) in entries {
            changes.insert(key.clone(), Some(Arc::clone(value)));
        }
    }
    for (key, value) in &export.entries {
        changes.insert(key.clone(), value.clone());
    }
    changes.into_iter().collect()
}

/// How a call treats its scopes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Reads the scope's body: loads it first when it is not resident.
    Read,
    /// Reads only pinned entries (always resident): no body load.
    ReadPinned,
    /// Changes the scope: self-fence first, body loaded, changes committed.
    Mutate,
}

#[derive(Debug, Default)]
struct MobHostRecords {
    bindings: BTreeMap<String, Vec<u8>>,
    revocations: BTreeMap<String, Vec<u8>>,
}

/// The [`RuntimeStore`] over a host store. See the module documentation.
pub struct SpiRuntimeStore {
    realm: Arc<SpiRealm>,
    memory: InMemoryRuntimeStore,
    /// Shared by single-scope calls, exclusive for cross-scope reads and the
    /// controller custody.
    gate: Arc<RwLock<()>>,
    /// The mob host records; a change lands here only after its batch did,
    /// on the task that committed it.
    mob_host: Arc<StdMutex<MobHostRecords>>,
}

impl std::fmt::Debug for SpiRuntimeStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpiRuntimeStore")
            .field("realm", &self.realm)
            .finish_non_exhaustive()
    }
}

fn lock_records(records: &StdMutex<MobHostRecords>) -> std::sync::MutexGuard<'_, MobHostRecords> {
    records
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn find<'a>(guards: &'a mut [ScopeGuard], id: &ScopeId) -> Result<&'a mut ScopeState, SpiError> {
    guards
        .iter_mut()
        .find(|guard| guard.id() == id)
        .map(|guard| &mut **guard)
        .ok_or_else(|| SpiError::Misuse(format!("{id} is not locked")))
}

impl SpiRuntimeStore {
    /// Open the runtime store over a realm: claim its families, import every
    /// realm record and every runtime's pinned entries, and load the bodies
    /// recovery must act on. Every other body loads on first use.
    ///
    /// # Errors
    /// Host failures and data that does not decode.
    pub async fn open(realm: Arc<SpiRealm>) -> Result<Self, SpiError> {
        realm.check_realm()?;
        let memory = InMemoryRuntimeStore::new_journaled();
        for cell in realm.claim(KEY_BINDING_FAMILY)? {
            let state = cell.lock().await;
            let (owner, key) = split_binding_key(&state.id().key)?;
            let pinned = realm.pinned_values(&state).await?;
            memory
                .journal_import(&TouchScope::KeyBinding { owner, key }, &pinned)
                .await
                .map_err(memory_error)?;
        }
        for cell in realm.claim(MEMBER_HIGH_WATER_FAMILY)? {
            let state = cell.lock().await;
            let pinned = realm.pinned_values(&state).await?;
            memory
                .journal_import(
                    &TouchScope::MemberHighWater(state.id().key.clone()),
                    &pinned,
                )
                .await
                .map_err(memory_error)?;
        }
        let mut records = MobHostRecords::default();
        for (family, map) in [
            (MOB_HOST_BINDING_FAMILY, &mut records.bindings),
            (MOB_HOST_REVOCATION_FAMILY, &mut records.revocations),
        ] {
            for cell in realm.claim(family)? {
                let state = cell.lock().await;
                let pinned = realm.pinned_values(&state).await?;
                if let Some(value) = pinned.get(RECORD_ENTRY) {
                    map.insert(state.id().key.clone(), Vec::clone(value));
                }
            }
        }
        for cell in realm.claim(RUNTIME_FAMILY)? {
            let mut state = cell.lock().await;
            let scope = TouchScope::Runtime(state.id().key.clone());
            let pinned = realm.pinned_values(&state).await?;
            memory
                .journal_import(&scope, &pinned)
                .await
                .map_err(memory_error)?;
            if needs_body_at_boot(&pinned)? {
                let body = realm.load_body(&mut state).await?;
                memory
                    .journal_import(&scope, &body)
                    .await
                    .map_err(memory_error)?;
                state.mark_loaded();
            }
        }
        Ok(Self {
            realm,
            memory,
            gate: Arc::new(RwLock::new(())),
            mob_host: Arc::new(StdMutex::new(records)),
        })
    }

    /// The realm this store commits through.
    #[must_use]
    pub fn realm(&self) -> &Arc<SpiRealm> {
        &self.realm
    }

    /// Run one in-memory method under its scopes and commit what it changed
    /// before answering. See the module documentation.
    ///
    /// Cancel safety: from the start of a mutation until its commit
    /// is handed to a task that owns the scopes, dropping this future
    /// poisons them ([`ArmedScopes`]); after the hand-off the task lands the
    /// batch and adopts it (or poisons) whether or not anyone still waits.
    async fn call<T, F>(&self, mode: Mode, ids: Vec<ScopeId>, op: F) -> Result<T, RuntimeStoreError>
    where
        F: std::future::Future<Output = Result<T, RuntimeStoreError>> + Send,
        T: Send,
    {
        let refuse = |error: SpiError| match mode {
            Mode::Mutate => error.into_write(),
            Mode::Read | Mode::ReadPinned => error.into_read(),
        };
        let gate = Arc::clone(&self.gate).read_owned().await;
        match mode {
            Mode::Mutate => self.realm.ensure_lease().await.map_err(refuse)?,
            Mode::Read | Mode::ReadPinned => self.realm.check_realm().map_err(refuse)?,
        }
        let guards = self.realm.lock_scopes(&ids).await;
        let mut scopes = ArmedScopes::new(Arc::clone(&self.realm), guards).hold(gate);
        for guard in scopes.guards_mut() {
            self.realm.check_scope(guard).map_err(refuse)?;
            if mode != Mode::ReadPinned {
                self.ensure_loaded(guard).await.map_err(refuse)?;
            }
        }
        if mode == Mode::Mutate {
            // The lock waits may have outlasted the window: check it again
            // before memory changes, so a lapse refuses cleanly.
            self.realm.ensure_lease().await.map_err(refuse)?;
            scopes.arm();
        }
        let (result, touches) = journal::capture(op).await;
        if touches.whole.is_empty() && touches.entries.is_empty() {
            scopes.disarm();
            return result;
        }
        // A read is not expected to change memory; one that did is
        // committed like a mutation, and a drop from here on poisons.
        scopes.arm();
        let (deltas, runtimes) = match self.prepare_commit(scopes.guards(), &touches).await {
            Ok(prepared) => prepared,
            Err(error) => {
                // Memory changed and cannot be exported: poison what it touched.
                let cause = error.to_string();
                let touched: HashSet<ScopeId> = touches.scopes().iter().map(scope_id_of).collect();
                for guard in scopes.guards_mut() {
                    if touched.contains(guard.id()) {
                        self.realm.poison(guard, &cause);
                    }
                }
                scopes.disarm();
                return Err(error.into_write());
            }
        };
        let mut scopes = scopes
            .commit_detached(deltas)
            .await
            .map_err(SpiError::into_write)?;
        // An archived runtime with nothing left for recovery leaves
        // memory once its archive is durable.
        for runtime_id in runtimes {
            let id = ScopeId::new(RUNTIME_FAMILY, runtime_id.clone());
            if let Ok(state) = find(scopes.guards_mut(), &id)
                && state.is_loaded()
                && self.memory.journal_archived_idle(&runtime_id).await
            {
                self.memory.journal_evict_body(&runtime_id).await;
                state.unload();
            }
        }
        result
    }

    /// A cross-scope read: exclusive gate (no commit in flight), and
    /// refused while any scope is poisoned.
    async fn cross_scope_read<T, F>(&self, op: F) -> Result<T, RuntimeStoreError>
    where
        F: std::future::Future<Output = Result<T, RuntimeStoreError>> + Send,
        T: Send,
    {
        let _gate = self.gate.write().await;
        self.realm
            .check_cross_scope_read()
            .map_err(SpiError::into_read)?;
        let (result, touches) = journal::capture(op).await;
        if !(touches.whole.is_empty() && touches.entries.is_empty()) {
            return Err(
                SpiError::Misuse("a cross-scope read changed the store".into()).into_read(),
            );
        }
        result
    }

    /// Load a scope's body into memory when it is not resident yet. A body
    /// that does not import poisons the scope: memory may hold part of it.
    async fn ensure_loaded(&self, state: &mut ScopeState) -> Result<(), SpiError> {
        if state.is_loaded() {
            return Ok(());
        }
        let Some(scope) = touch_scope_of(state.id())? else {
            return Ok(());
        };
        let body = self.realm.load_body(state).await?;
        // The import holds the in-memory lock without awaiting: it lands
        // whole or not at all, so a dropped call leaves the scope unloaded.
        if let Err(error) = self.memory.journal_import(&scope, &body).await {
            let cause = format!("its body did not import: {error}");
            self.realm.poison(state, &cause);
            return Err(memory_error(error));
        }
        state.mark_loaded();
        Ok(())
    }

    /// Turn what a call touched into one delta per changed locked scope
    /// (paired with its index among `guards`), and name the runtimes whose
    /// archive may let their body leave memory. A touch outside the locked
    /// scopes is refused and poisons that scope, or the realm when the
    /// scope is busy.
    async fn prepare_commit(
        &self,
        guards: &[ScopeGuard],
        touches: &Touches,
    ) -> Result<(Vec<(usize, ScopeDelta)>, Vec<String>), SpiError> {
        let locked: HashSet<ScopeId> = guards.iter().map(|guard| guard.id().clone()).collect();
        let stray: Vec<ScopeId> = touches
            .scopes()
            .iter()
            .map(scope_id_of)
            .filter(|id| !locked.contains(id))
            .collect();
        if !stray.is_empty() {
            let cause = format!(
                "a call changed {} outside the scopes it locked",
                stray
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            for id in &stray {
                match self.realm.cell(id).try_lock() {
                    Ok(mut state) => self.realm.poison(&mut state, &cause),
                    Err(_) => return Err(self.realm.poison_all(cause)),
                }
            }
            return Err(SpiError::Misuse(cause));
        }
        let exports = self
            .memory
            .journal_export(touches)
            .await
            .map_err(memory_error)?;
        let mut deltas = Vec::new();
        let mut runtimes = Vec::new();
        for (index, guard) in guards.iter().enumerate() {
            let Some(scope) = touch_scope_of(guard.id())? else {
                continue;
            };
            let Some(export) = exports.get(&scope) else {
                continue;
            };
            let delta = if guard.wants_full_image() {
                ScopeDelta::Full(
                    self.memory
                        .journal_export_full(&scope)
                        .await
                        .map_err(memory_error)?,
                )
            } else {
                ScopeDelta::Changes(changes_of(guard, export))
            };
            if let TouchScope::Runtime(runtime_id) = &scope {
                runtimes.push(runtime_id.clone());
            }
            deltas.push((index, delta));
        }
        Ok((deltas, runtimes))
    }

    /// Lock one mob's host record scopes: gate shared, self-fence (again
    /// after the locks for a change), locks, poison checks. The gate
    /// travels with the scopes.
    async fn mob_host_scopes(
        &self,
        ids: &[ScopeId],
        mutate: bool,
    ) -> Result<ArmedScopes, RuntimeStoreError> {
        let refuse = |error: SpiError| {
            if mutate {
                error.into_write()
            } else {
                error.into_read()
            }
        };
        let gate = Arc::clone(&self.gate).read_owned().await;
        if mutate {
            self.realm.ensure_lease().await.map_err(refuse)?;
        } else {
            self.realm.check_realm().map_err(refuse)?;
        }
        let guards = self.realm.lock_scopes(ids).await;
        let scopes = ArmedScopes::new(Arc::clone(&self.realm), guards).hold(gate);
        for guard in scopes.guards() {
            self.realm.check_scope(guard).map_err(refuse)?;
        }
        if mutate {
            self.realm.ensure_lease().await.map_err(refuse)?;
        }
        Ok(scopes)
    }

    /// Commit record scope changes, each scope's value setting its record
    /// entry or removing it (and so the scope) when `None`, then `apply`
    /// them to the records in memory. Both run on a task that owns the
    /// scopes and the gate, so a caller dropped meanwhile still leaves
    /// memory and host agreeing.
    async fn commit_records(
        &self,
        scopes: ArmedScopes,
        changes: BTreeMap<ScopeId, Option<Vec<u8>>>,
        apply: impl FnOnce(&mut MobHostRecords) + Send + 'static,
    ) -> Result<(), RuntimeStoreError> {
        let mut deltas = Vec::with_capacity(changes.len());
        for (index, guard) in scopes.guards().iter().enumerate() {
            if let Some(value) = changes.get(guard.id()) {
                deltas.push((
                    index,
                    ScopeDelta::Changes(vec![(
                        RECORD_ENTRY.to_string(),
                        value.clone().map(Arc::new),
                    )]),
                ));
            }
        }
        if deltas.len() != changes.len() {
            return Err(SpiError::Misuse("a record scope is not locked".into()).into_write());
        }
        let records = Arc::clone(&self.mob_host);
        let task = tokio::spawn(async move {
            let mut scopes = scopes;
            // A task dropped mid-commit (a runtime shutdown) poisons: the
            // batch may land without memory following it.
            scopes.arm();
            let outcome = scopes.commit_in_place(deltas).await;
            if outcome.is_ok() {
                apply(&mut lock_records(&records));
            }
            outcome
        });
        match task.await {
            Ok(outcome) => outcome.map_err(SpiError::into_write),
            Err(error) => {
                Err(SpiError::Misuse(format!("the host commit task failed: {error}")).into_write())
            }
        }
    }
}

#[cfg(test)]
impl SpiRuntimeStore {
    /// The full image memory holds for a runtime.
    pub(crate) async fn export_runtime_for_test(
        &self,
        runtime_id: &str,
    ) -> BTreeMap<String, Arc<Vec<u8>>> {
        self.memory
            .journal_export_full(&TouchScope::Runtime(runtime_id.to_string()))
            .await
            .unwrap_or_default()
    }

    /// Whether a runtime's body is resident.
    pub(crate) async fn runtime_loaded_for_test(&self, runtime_id: &str) -> bool {
        self.realm
            .cell(&runtime_id_scope(&LogicalRuntimeId::new(runtime_id)))
            .lock()
            .await
            .is_loaded()
    }
}

/// The controller custody of an [`SpiRuntimeStore`]: the in-memory custody
/// plus the realm gate held exclusive, so no host commit is in flight while
/// a controller visits the runtimes.
struct SpiControllerCustody<'a> {
    inner: Box<dyn crate::store::RuntimeStoreControllerCustody + 'a>,
    _gate: tokio::sync::RwLockWriteGuard<'a, ()>,
}

impl crate::store::RuntimeStoreControllerCustody for SpiControllerCustody<'_> {
    fn visit_runtimes(
        &self,
        visit: &mut dyn FnMut(&crate::store::RuntimeStoreControllerRuntime) -> bool,
    ) -> Result<bool, RuntimeStoreError> {
        self.inner.visit_runtimes(visit)
    }
}

#[async_trait::async_trait]
impl RuntimeSessionAuthorityOps for SpiRuntimeStore {
    fn try_controller_mutation_custody<'a>(
        &'a self,
        claim: &'a crate::store::RuntimeStoreExecutionClaim,
    ) -> Result<Box<dyn crate::store::RuntimeStoreControllerCustody + 'a>, RuntimeStoreError> {
        self.realm
            .check_cross_scope_read()
            .map_err(SpiError::into_write)?;
        let gate = self.gate.try_write().map_err(|_| {
            RuntimeStoreError::WriteFailed("controller administration memory writer is busy".into())
        })?;
        let inner =
            <InMemoryRuntimeStore as RuntimeSessionAuthorityOps>::try_controller_mutation_custody(
                &self.memory,
                claim,
            )?;
        Ok(Box::new(SpiControllerCustody { inner, _gate: gate }))
    }

    fn commit_outcome_deadline(&self) -> Option<std::time::Duration> {
        <InMemoryRuntimeStore as RuntimeSessionAuthorityOps>::commit_outcome_deadline(&self.memory)
    }

    fn execution_custody(&self) -> Option<&crate::store::RuntimeStoreExecutionCustody> {
        <InMemoryRuntimeStore as RuntimeSessionAuthorityOps>::execution_custody(&self.memory)
    }

    fn session_persistence_profile(&self) -> crate::store::RuntimeSessionPersistenceProfile {
        <InMemoryRuntimeStore as RuntimeSessionAuthorityOps>::session_persistence_profile(
            &self.memory,
        )
    }

    fn session_boundary_authority_read_cost(&self) -> RuntimeSessionAuthorityReadCost {
        <InMemoryRuntimeStore as RuntimeSessionAuthorityOps>::session_boundary_authority_read_cost(
            &self.memory,
        )
    }

    async fn activate_head_canonical_runtime_authority(
        &self,
        authority: meerkat_core::VerifiedHeadCanonicalAuthority,
    ) -> Result<crate::store::HeadCanonicalRuntimeAuthorityActivation, RuntimeStoreError> {
        self.call(Mode::Mutate, Vec::new(), <InMemoryRuntimeStore as RuntimeSessionAuthorityOps>::activate_head_canonical_runtime_authority(&self.memory, authority)).await
    }

    async fn write_prepared_head_canonical_provisional_tail(
        &self,
        runtime_id: &LogicalRuntimeId,
        prepared: crate::store::PreparedHeadCanonicalProvisionalTail,
    ) -> Result<meerkat_core::HeadCanonicalProvisionalTailAuthority, RuntimeStoreError> {
        self.call(Mode::Mutate, vec![runtime_id_scope(runtime_id)], <InMemoryRuntimeStore as RuntimeSessionAuthorityOps>::write_prepared_head_canonical_provisional_tail(&self.memory, runtime_id, prepared)).await
    }

    async fn load_head_canonical_provisional_tail(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<meerkat_core::HeadCanonicalProvisionalTailAuthority>, RuntimeStoreError>
    {
        self.call(Mode::Read, vec![runtime_id_scope(runtime_id)], <InMemoryRuntimeStore as RuntimeSessionAuthorityOps>::load_head_canonical_provisional_tail(&self.memory, runtime_id)).await
    }

    async fn discard_head_canonical_provisional_tail(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected: &meerkat_core::HeadCanonicalProvisionalTailAuthority,
    ) -> Result<bool, RuntimeStoreError> {
        self.call(Mode::Mutate, vec![runtime_id_scope(runtime_id)], <InMemoryRuntimeStore as RuntimeSessionAuthorityOps>::discard_head_canonical_provisional_tail(&self.memory, runtime_id, expected)).await
    }

    async fn load_durable_tail_recovery_source(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<crate::store::PreparedDurableTailRecoverySource>, RuntimeStoreError> {
        self.call(
            Mode::Read,
            vec![runtime_id_scope(runtime_id)],
            <InMemoryRuntimeStore as RuntimeSessionAuthorityOps>::load_durable_tail_recovery_source(
                &self.memory,
                runtime_id,
            ),
        )
        .await
    }

    async fn commit_prepared_session_boundary_execution(
        &self,
        runtime_id: &LogicalRuntimeId,
        execution: crate::store::PreparedRuntimeSessionCommitExecution,
    ) -> Result<crate::store::PreparedRuntimeSessionCommitResult, RuntimeStoreError> {
        self.call(Mode::Mutate, vec![runtime_id_scope(runtime_id)], <InMemoryRuntimeStore as RuntimeSessionAuthorityOps>::commit_prepared_session_boundary_execution(&self.memory, runtime_id, execution)).await
    }

    async fn load_durable_tail_recovery_receipts(
        &self,
        runtime_id: &LogicalRuntimeId,
        run_id: &RunId,
    ) -> Result<Vec<crate::store::PreparedRecoveryReceiptSource>, RuntimeStoreError> {
        self.call(Mode::Read, vec![runtime_id_scope(runtime_id)], <InMemoryRuntimeStore as RuntimeSessionAuthorityOps>::load_durable_tail_recovery_receipts(&self.memory, runtime_id, run_id)).await
    }

    async fn load_committed_recovery_boundary(
        &self,
        runtime_id: &LogicalRuntimeId,
        candidate_id: &str,
    ) -> Result<Option<crate::store::CommittedRecoveryBoundary>, RuntimeStoreError> {
        self.call(
            Mode::Read,
            vec![runtime_id_scope(runtime_id)],
            <InMemoryRuntimeStore as RuntimeSessionAuthorityOps>::load_committed_recovery_boundary(
                &self.memory,
                runtime_id,
                candidate_id,
            ),
        )
        .await
    }

    async fn load_whole_blob_store_authority(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<WholeBlobStoreAuthority>, RuntimeStoreError> {
        self.call(
            Mode::Read,
            vec![runtime_id_scope(runtime_id)],
            <InMemoryRuntimeStore as RuntimeSessionAuthorityOps>::load_whole_blob_store_authority(
                &self.memory,
                runtime_id,
            ),
        )
        .await
    }

    async fn load_session_boundary_authority(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<RuntimeSessionAuthority>, RuntimeStoreError> {
        self.call(
            Mode::Read,
            vec![runtime_id_scope(runtime_id)],
            <InMemoryRuntimeStore as RuntimeSessionAuthorityOps>::load_session_boundary_authority(
                &self.memory,
                runtime_id,
            ),
        )
        .await
    }

    async fn load_session_resume_observation(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<RuntimeSessionResumeObservation, RuntimeStoreError> {
        self.call(
            Mode::Read,
            vec![runtime_id_scope(runtime_id)],
            <InMemoryRuntimeStore as RuntimeSessionAuthorityOps>::load_session_resume_observation(
                &self.memory,
                runtime_id,
            ),
        )
        .await
    }

    async fn delete_runtime_session_catalog_entry(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<(), RuntimeStoreError> {
        self.call(Mode::Mutate, vec![runtime_id_scope(runtime_id)], <InMemoryRuntimeStore as RuntimeSessionAuthorityOps>::delete_runtime_session_catalog_entry(&self.memory, runtime_id)).await
    }

    async fn load_runtime_session_catalog_entry(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<crate::store::RuntimeSessionCatalogEntry>, RuntimeStoreError> {
        self.call(Mode::ReadPinned, vec![runtime_id_scope(runtime_id)], <InMemoryRuntimeStore as RuntimeSessionAuthorityOps>::load_runtime_session_catalog_entry(&self.memory, runtime_id)).await
    }

    async fn list_runtime_session_catalog_entries(
        &self,
        filter: meerkat_core::SessionFilter,
    ) -> Result<Vec<crate::store::RuntimeSessionCatalogEntry>, RuntimeStoreError> {
        self.cross_scope_read(<InMemoryRuntimeStore as RuntimeSessionAuthorityOps>::list_runtime_session_catalog_entries(&self.memory, filter)).await
    }

    async fn load_committed_whole_blob_bytes(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<(Arc<Vec<u8>>, WholeBlobStoreAuthority)>, RuntimeStoreError> {
        self.call(
            Mode::Read,
            vec![runtime_id_scope(runtime_id)],
            <InMemoryRuntimeStore as RuntimeSessionAuthorityOps>::load_committed_whole_blob_bytes(
                &self.memory,
                runtime_id,
            ),
        )
        .await
    }

    async fn load_committed_whole_blob_snapshot(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<CommittedWholeBlobSnapshot>, RuntimeStoreError> {
        self.call(Mode::Read, vec![runtime_id_scope(runtime_id)], <InMemoryRuntimeStore as RuntimeSessionAuthorityOps>::load_committed_whole_blob_snapshot(&self.memory, runtime_id)).await
    }

    async fn commit_prepared_whole_blob_snapshot_cas(
        &self,
        runtime_id: &LogicalRuntimeId,
        prepared: PreparedWholeBlobSnapshotCas,
    ) -> Result<WholeBlobSnapshotCasOutcome, RuntimeStoreError> {
        self.call(Mode::Mutate, vec![runtime_id_scope(runtime_id)], <InMemoryRuntimeStore as RuntimeSessionAuthorityOps>::commit_prepared_whole_blob_snapshot_cas(&self.memory, runtime_id, prepared)).await
    }

    async fn write_prepared_whole_blob_provisional_tail(
        &self,
        runtime_id: &LogicalRuntimeId,
        prepared: PreparedWholeBlobProvisionalTail,
    ) -> Result<WholeBlobProvisionalTailAuthority, RuntimeStoreError> {
        self.call(Mode::Mutate, vec![runtime_id_scope(runtime_id)], <InMemoryRuntimeStore as RuntimeSessionAuthorityOps>::write_prepared_whole_blob_provisional_tail(&self.memory, runtime_id, prepared)).await
    }

    async fn load_whole_blob_provisional_tail(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<CommittedWholeBlobProvisionalTail>, RuntimeStoreError> {
        self.call(
            Mode::Read,
            vec![runtime_id_scope(runtime_id)],
            <InMemoryRuntimeStore as RuntimeSessionAuthorityOps>::load_whole_blob_provisional_tail(
                &self.memory,
                runtime_id,
            ),
        )
        .await
    }

    async fn discard_whole_blob_provisional_tail(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected: &WholeBlobProvisionalTailAuthority,
    ) -> Result<bool, RuntimeStoreError> {
        self.call(Mode::Mutate, vec![runtime_id_scope(runtime_id)], <InMemoryRuntimeStore as RuntimeSessionAuthorityOps>::discard_whole_blob_provisional_tail(&self.memory, runtime_id, expected)).await
    }
}

#[async_trait::async_trait]
impl RuntimeStore for SpiRuntimeStore {
    fn session_authority_ops(&self) -> &dyn RuntimeSessionAuthorityOps {
        self
    }

    /// One writer process holds the realm fence: no other process can host
    /// a session of this store while this one serves it.
    fn hosting_capability(&self) -> crate::session_hosting::HostingCapability {
        crate::session_hosting::HostingCapability::ProcessLocal
    }

    /// The realm's self-fence: its window closes when a renew fails or the
    /// last successful acquire or renew started more than the lease minus
    /// the host-declared margin ago.
    fn self_fence(&self) -> Option<Arc<dyn meerkat_core::host_store::SelfFence>> {
        Some(Arc::clone(&self.realm) as Arc<dyn meerkat_core::host_store::SelfFence>)
    }

    async fn commit_prepared_session_boundary_with_fence(
        &self,
        runtime_id: &LogicalRuntimeId,
        request: crate::store::PreparedRuntimeSessionCommit,
        write_fence: Arc<dyn crate::store::RuntimeStoreWriteFence>,
    ) -> Result<crate::store::FencedPreparedRuntimeSessionCommitOutcome, RuntimeStoreError> {
        self.call(
            Mode::Mutate,
            vec![runtime_id_scope(runtime_id)],
            <InMemoryRuntimeStore as RuntimeStore>::commit_prepared_session_boundary_with_fence(
                &self.memory,
                runtime_id,
                request,
                write_fence,
            ),
        )
        .await
    }

    fn supports_compaction_projection_outbox(&self) -> bool {
        <InMemoryRuntimeStore as RuntimeStore>::supports_compaction_projection_outbox(&self.memory)
    }

    fn input_state_batch_cas_implementation_profile(
        &self,
    ) -> InputStateBatchCasImplementationProfile {
        <InMemoryRuntimeStore as RuntimeStore>::input_state_batch_cas_implementation_profile(
            &self.memory,
        )
    }

    async fn load_runtime_delivery_authority(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<RuntimeDeliveryAuthorityRecord>, RuntimeStoreError> {
        self.call(
            Mode::ReadPinned,
            vec![runtime_id_scope(runtime_id)],
            <InMemoryRuntimeStore as RuntimeStore>::load_runtime_delivery_authority(
                &self.memory,
                runtime_id,
            ),
        )
        .await
    }

    async fn load_runtime_delivery_record(
        &self,
        runtime_id: &LogicalRuntimeId,
        delivery_id: &str,
    ) -> Result<Option<RuntimeDeliveryStoreRecord>, RuntimeStoreError> {
        self.call(
            Mode::Read,
            vec![runtime_id_scope(runtime_id)],
            <InMemoryRuntimeStore as RuntimeStore>::load_runtime_delivery_record(
                &self.memory,
                runtime_id,
                delivery_id,
            ),
        )
        .await
    }

    async fn compare_and_swap_runtime_delivery_authority(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected_revision: Option<u64>,
        replacement: RuntimeDeliveryAuthorityRecord,
        inserted_delivery: Option<RuntimeDeliveryStoreRecord>,
    ) -> Result<RuntimeDeliveryAuthorityCasOutcome, RuntimeStoreError> {
        self.call(
            Mode::Mutate,
            vec![runtime_id_scope(runtime_id)],
            <InMemoryRuntimeStore as RuntimeStore>::compare_and_swap_runtime_delivery_authority(
                &self.memory,
                runtime_id,
                expected_revision,
                replacement,
                inserted_delivery,
            ),
        )
        .await
    }

    async fn load_continuation_key_binding(
        &self,
        owner: &str,
        key: &str,
    ) -> Result<Option<ContinuationKeyBinding>, RuntimeStoreError> {
        self.call(
            Mode::ReadPinned,
            vec![key_binding_scope(owner, key)],
            <InMemoryRuntimeStore as RuntimeStore>::load_continuation_key_binding(
                &self.memory,
                owner,
                key,
            ),
        )
        .await
    }

    async fn load_continuation_admission(
        &self,
        address: &LogicalRuntimeId,
        delivery_id: &str,
    ) -> Result<Option<ContinuationAdmission>, RuntimeStoreError> {
        self.call(
            Mode::Read,
            vec![runtime_id_scope(address)],
            <InMemoryRuntimeStore as RuntimeStore>::load_continuation_admission(
                &self.memory,
                address,
                delivery_id,
            ),
        )
        .await
    }

    async fn transition_continuation_admission(
        &self,
        address: &LogicalRuntimeId,
        delivery_id: &str,
        transition: ContinuationAdmissionTransition,
    ) -> Result<ContinuationAdmissionOutcome, RuntimeStoreError> {
        self.call(
            Mode::Mutate,
            vec![runtime_id_scope(address)],
            <InMemoryRuntimeStore as RuntimeStore>::transition_continuation_admission(
                &self.memory,
                address,
                delivery_id,
                transition,
            ),
        )
        .await
    }

    async fn compare_and_swap_runtime_delivery_authority_with_key_binding(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected_revision: Option<u64>,
        replacement: RuntimeDeliveryAuthorityRecord,
        inserted_delivery: RuntimeDeliveryStoreRecord,
        binding: ContinuationKeyBinding,
    ) -> Result<KeyedRuntimeDeliveryCasOutcome, RuntimeStoreError> {
        self.call(Mode::Mutate, vec![runtime_id_scope(runtime_id), key_binding_scope(&binding.owner, &binding.key)], <InMemoryRuntimeStore as RuntimeStore>::compare_and_swap_runtime_delivery_authority_with_key_binding(&self.memory, runtime_id, expected_revision, replacement, inserted_delivery, binding)).await
    }

    async fn list_runtime_delivery_authorities(
        &self,
    ) -> Result<Vec<(LogicalRuntimeId, RuntimeDeliveryAuthorityRecord)>, RuntimeStoreError> {
        self.cross_scope_read(
            <InMemoryRuntimeStore as RuntimeStore>::list_runtime_delivery_authorities(&self.memory),
        )
        .await
    }

    async fn list_runtime_delivery_records(
        &self,
        runtime_id: &LogicalRuntimeId,
        after_sequence: u64,
        limit: usize,
    ) -> Result<Vec<RuntimeDeliveryStoreRecord>, RuntimeStoreError> {
        self.call(
            Mode::Read,
            vec![runtime_id_scope(runtime_id)],
            <InMemoryRuntimeStore as RuntimeStore>::list_runtime_delivery_records(
                &self.memory,
                runtime_id,
                after_sequence,
                limit,
            ),
        )
        .await
    }

    fn persist_auth_oauth_flow_snapshot(
        &self,
        snapshot_json: &[u8],
    ) -> Result<(), RuntimeStoreError> {
        <InMemoryRuntimeStore as RuntimeStore>::persist_auth_oauth_flow_snapshot(
            &self.memory,
            snapshot_json,
        )
    }

    fn load_auth_oauth_flow_snapshot(&self) -> Result<Option<Vec<u8>>, RuntimeStoreError> {
        <InMemoryRuntimeStore as RuntimeStore>::load_auth_oauth_flow_snapshot(&self.memory)
    }

    fn update_auth_oauth_flow_snapshot(
        &self,
        update: &mut AuthOAuthFlowSnapshotUpdate<'_>,
    ) -> Result<(), RuntimeStoreError> {
        <InMemoryRuntimeStore as RuntimeStore>::update_auth_oauth_flow_snapshot(
            &self.memory,
            update,
        )
    }

    async fn commit_session_snapshot(
        &self,
        runtime_id: &LogicalRuntimeId,
        session_delta: SerializedSessionSnapshot,
    ) -> Result<(), RuntimeStoreError> {
        self.call(
            Mode::Mutate,
            vec![runtime_id_scope(runtime_id)],
            <InMemoryRuntimeStore as RuntimeStore>::commit_session_snapshot(
                &self.memory,
                runtime_id,
                session_delta,
            ),
        )
        .await
    }

    async fn commit_prepared_whole_blob_rewrite_boundary(
        &self,
        runtime_id: &LogicalRuntimeId,
        boundary: PreparedWholeBlobRewriteStoreParts,
    ) -> Result<WholeBlobStoreAuthority, RuntimeStoreError> {
        self.call(
            Mode::Mutate,
            vec![runtime_id_scope(runtime_id)],
            <InMemoryRuntimeStore as RuntimeStore>::commit_prepared_whole_blob_rewrite_boundary(
                &self.memory,
                runtime_id,
                boundary,
            ),
        )
        .await
    }

    /// Forwarded with the re-base itself: the in-memory store implements it,
    /// so the session service re-bases this store's WholeBlob runs instead
    /// of taking the fallback.
    fn supports_whole_blob_run_rebase(&self) -> bool {
        <InMemoryRuntimeStore as RuntimeStore>::supports_whole_blob_run_rebase(&self.memory)
    }

    /// A run re-base is one in-memory call, so it lands as one host batch
    /// (plus the chunk pre-batches of its documents): the committed
    /// successor, its outbox rows, the old tail's removal and the new tail.
    async fn commit_prepared_whole_blob_run_rebase(
        &self,
        runtime_id: &LogicalRuntimeId,
        rebase: crate::store::PreparedWholeBlobRunRebaseStoreParts,
    ) -> Result<crate::store::WholeBlobRunRebaseOutcome, RuntimeStoreError> {
        self.call(
            Mode::Mutate,
            vec![runtime_id_scope(runtime_id)],
            <InMemoryRuntimeStore as RuntimeStore>::commit_prepared_whole_blob_run_rebase(
                &self.memory,
                runtime_id,
                rebase,
            ),
        )
        .await
    }

    async fn atomic_apply(
        &self,
        runtime_id: &LogicalRuntimeId,
        session_delta: Option<SerializedSessionSnapshot>,
        receipt: RunBoundaryReceipt,
        input_updates: Vec<InputStatePersistenceRecord>,
        session_store_key: Option<meerkat_core::types::SessionId>,
    ) -> Result<(), RuntimeStoreError> {
        self.call(
            Mode::Mutate,
            vec![runtime_id_scope(runtime_id)],
            <InMemoryRuntimeStore as RuntimeStore>::atomic_apply(
                &self.memory,
                runtime_id,
                session_delta,
                receipt,
                input_updates,
                session_store_key,
            ),
        )
        .await
    }

    async fn load_pending_compaction_projections(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Vec<meerkat_core::CompactionProjectionIntent>, RuntimeStoreError> {
        self.call(
            Mode::Read,
            vec![runtime_id_scope(runtime_id)],
            <InMemoryRuntimeStore as RuntimeStore>::load_pending_compaction_projections(
                &self.memory,
                runtime_id,
            ),
        )
        .await
    }

    async fn mark_compaction_projection_finalized(
        &self,
        runtime_id: &LogicalRuntimeId,
        projection: &meerkat_core::CompactionProjectionId,
    ) -> Result<(), RuntimeStoreError> {
        self.call(
            Mode::Mutate,
            vec![runtime_id_scope(runtime_id)],
            <InMemoryRuntimeStore as RuntimeStore>::mark_compaction_projection_finalized(
                &self.memory,
                runtime_id,
                projection,
            ),
        )
        .await
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
        self.call(
            Mode::Mutate,
            vec![runtime_id_scope(runtime_id)],
            <InMemoryRuntimeStore as RuntimeStore>::atomic_apply_with_machine_lifecycle(
                &self.memory,
                runtime_id,
                session_delta,
                receipt,
                machine_lifecycle,
                input_updates,
                session_store_key,
            ),
        )
        .await
    }

    async fn load_input_states(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Vec<InputStateRow>, RuntimeStoreError> {
        self.call(
            Mode::Read,
            vec![runtime_id_scope(runtime_id)],
            <InMemoryRuntimeStore as RuntimeStore>::load_input_states(&self.memory, runtime_id),
        )
        .await
    }

    async fn load_boundary_receipt(
        &self,
        runtime_id: &LogicalRuntimeId,
        run_id: &RunId,
        sequence: u64,
    ) -> Result<Option<RunBoundaryReceipt>, RuntimeStoreError> {
        self.call(
            Mode::Read,
            vec![runtime_id_scope(runtime_id)],
            <InMemoryRuntimeStore as RuntimeStore>::load_boundary_receipt(
                &self.memory,
                runtime_id,
                run_id,
                sequence,
            ),
        )
        .await
    }

    async fn load_committed_boundary_receipts(
        &self,
        runtime_id: &LogicalRuntimeId,
        run_id: &RunId,
    ) -> Result<Vec<RunBoundaryReceipt>, RuntimeStoreError> {
        self.call(
            Mode::Read,
            vec![runtime_id_scope(runtime_id)],
            <InMemoryRuntimeStore as RuntimeStore>::load_committed_boundary_receipts(
                &self.memory,
                runtime_id,
                run_id,
            ),
        )
        .await
    }

    async fn load_input_states_with_versions(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<PreparedRecoveryInputSnapshot, RuntimeStoreError> {
        self.call(
            Mode::Read,
            vec![runtime_id_scope(runtime_id)],
            <InMemoryRuntimeStore as RuntimeStore>::load_input_states_with_versions(
                &self.memory,
                runtime_id,
            ),
        )
        .await
    }

    async fn load_session_snapshot(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<Arc<Vec<u8>>>, RuntimeStoreError> {
        self.call(
            Mode::Read,
            vec![runtime_id_scope(runtime_id)],
            <InMemoryRuntimeStore as RuntimeStore>::load_session_snapshot(&self.memory, runtime_id),
        )
        .await
    }

    async fn clear_session_snapshot(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<(), RuntimeStoreError> {
        self.call(
            Mode::Mutate,
            vec![runtime_id_scope(runtime_id)],
            <InMemoryRuntimeStore as RuntimeStore>::clear_session_snapshot(
                &self.memory,
                runtime_id,
            ),
        )
        .await
    }

    async fn replace_session_snapshot_if_current(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected_current: &[u8],
        replacement: Vec<u8>,
    ) -> Result<bool, RuntimeStoreError> {
        self.call(
            Mode::Mutate,
            vec![runtime_id_scope(runtime_id)],
            <InMemoryRuntimeStore as RuntimeStore>::replace_session_snapshot_if_current(
                &self.memory,
                runtime_id,
                expected_current,
                replacement,
            ),
        )
        .await
    }

    async fn clear_session_snapshot_if_current(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected_current: &[u8],
    ) -> Result<bool, RuntimeStoreError> {
        self.call(
            Mode::Mutate,
            vec![runtime_id_scope(runtime_id)],
            <InMemoryRuntimeStore as RuntimeStore>::clear_session_snapshot_if_current(
                &self.memory,
                runtime_id,
                expected_current,
            ),
        )
        .await
    }

    async fn is_runtime_projection_quarantined(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<bool, RuntimeStoreError> {
        self.call(
            Mode::Read,
            vec![runtime_id_scope(runtime_id)],
            <InMemoryRuntimeStore as RuntimeStore>::is_runtime_projection_quarantined(
                &self.memory,
                runtime_id,
            ),
        )
        .await
    }

    async fn persist_input_state(
        &self,
        runtime_id: &LogicalRuntimeId,
        state: &InputStatePersistenceRecord,
    ) -> Result<(), RuntimeStoreError> {
        self.call(
            Mode::Mutate,
            vec![runtime_id_scope(runtime_id)],
            <InMemoryRuntimeStore as RuntimeStore>::persist_input_state(
                &self.memory,
                runtime_id,
                state,
            ),
        )
        .await
    }

    async fn persist_input_states_atomically(
        &self,
        runtime_id: &LogicalRuntimeId,
        records: &[InputStatePersistenceRecord],
    ) -> Result<(), RuntimeStoreError> {
        self.call(
            Mode::Mutate,
            vec![runtime_id_scope(runtime_id)],
            <InMemoryRuntimeStore as RuntimeStore>::persist_input_states_atomically(
                &self.memory,
                runtime_id,
                records,
            ),
        )
        .await
    }

    async fn compare_and_swap_input_states_atomically(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected: &[StoredInputState],
        replacements: &[InputStatePersistenceRecord],
    ) -> Result<InputStateBatchCasOutcome, RuntimeStoreError> {
        self.call(
            Mode::Mutate,
            vec![runtime_id_scope(runtime_id)],
            <InMemoryRuntimeStore as RuntimeStore>::compare_and_swap_input_states_atomically(
                &self.memory,
                runtime_id,
                expected,
                replacements,
            ),
        )
        .await
    }

    async fn compare_and_swap_input_states_atomically_with_fence(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected: &[StoredInputState],
        replacements: &[InputStatePersistenceRecord],
        write_fence: Arc<dyn RuntimeStoreWriteFence>,
    ) -> Result<FencedInputStateBatchCasOutcome, RuntimeStoreError> {
        self.call(Mode::Mutate, vec![runtime_id_scope(runtime_id)], <InMemoryRuntimeStore as RuntimeStore>::compare_and_swap_input_states_atomically_with_fence(&self.memory, runtime_id, expected, replacements, write_fence)).await
    }

    async fn compare_and_swap_recovery_input_states_atomically(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected_revision: RecoveryInputSetRevision,
        mutations: &[RecoveryInputStateMutation],
    ) -> Result<InputStateBatchCasOutcome, RuntimeStoreError> {
        self.call(Mode::Mutate, vec![runtime_id_scope(runtime_id)], <InMemoryRuntimeStore as RuntimeStore>::compare_and_swap_recovery_input_states_atomically(&self.memory, runtime_id, expected_revision, mutations)).await
    }

    async fn compare_and_swap_recovery_input_states_atomically_with_fence(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected_revision: RecoveryInputSetRevision,
        mutations: &[RecoveryInputStateMutation],
        write_fence: Arc<dyn RuntimeStoreWriteFence>,
    ) -> Result<FencedInputStateBatchCasOutcome, RuntimeStoreError> {
        self.call(Mode::Mutate, vec![runtime_id_scope(runtime_id)], <InMemoryRuntimeStore as RuntimeStore>::compare_and_swap_recovery_input_states_atomically_with_fence(&self.memory, runtime_id, expected_revision, mutations, write_fence)).await
    }

    async fn load_input_state(
        &self,
        runtime_id: &LogicalRuntimeId,
        input_id: &InputId,
    ) -> Result<Option<StoredInputState>, RuntimeStoreError> {
        self.call(
            Mode::Read,
            vec![runtime_id_scope(runtime_id)],
            <InMemoryRuntimeStore as RuntimeStore>::load_input_state(
                &self.memory,
                runtime_id,
                input_id,
            ),
        )
        .await
    }

    async fn load_input_state_by_idempotency_key(
        &self,
        runtime_id: &LogicalRuntimeId,
        key: &IdempotencyKey,
    ) -> Result<Option<ExactInputStateObservation>, RuntimeStoreError> {
        self.call(
            Mode::Read,
            vec![runtime_id_scope(runtime_id)],
            <InMemoryRuntimeStore as RuntimeStore>::load_input_state_by_idempotency_key(
                &self.memory,
                runtime_id,
                key,
            ),
        )
        .await
    }

    async fn load_input_states_by_ids(
        &self,
        runtime_id: &LogicalRuntimeId,
        input_ids: &[InputId],
    ) -> Result<Vec<Option<StoredInputState>>, RuntimeStoreError> {
        self.call(
            Mode::Read,
            vec![runtime_id_scope(runtime_id)],
            <InMemoryRuntimeStore as RuntimeStore>::load_input_states_by_ids(
                &self.memory,
                runtime_id,
                input_ids,
            ),
        )
        .await
    }

    async fn load_pending_terminal_owner_ids_page(
        &self,
        runtime_id: &LogicalRuntimeId,
        after: Option<&InputId>,
        limit: usize,
    ) -> Result<Vec<InputId>, RuntimeStoreError> {
        self.call(
            Mode::Read,
            vec![runtime_id_scope(runtime_id)],
            <InMemoryRuntimeStore as RuntimeStore>::load_pending_terminal_owner_ids_page(
                &self.memory,
                runtime_id,
                after,
                limit,
            ),
        )
        .await
    }

    async fn observe_machine_lifecycle(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<MachineLifecycleObservation, RuntimeStoreError> {
        self.call(
            Mode::ReadPinned,
            vec![runtime_id_scope(runtime_id)],
            <InMemoryRuntimeStore as RuntimeStore>::observe_machine_lifecycle(
                &self.memory,
                runtime_id,
            ),
        )
        .await
    }

    async fn compare_and_swap_machine_lifecycle(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected: MachineLifecycleExpectedVersion,
        replacement: MachineLifecycleCommit,
    ) -> Result<MachineLifecycleCasOutcome, RuntimeStoreError> {
        self.call(
            Mode::Mutate,
            vec![runtime_id_scope(runtime_id)],
            <InMemoryRuntimeStore as RuntimeStore>::compare_and_swap_machine_lifecycle(
                &self.memory,
                runtime_id,
                expected,
                replacement,
            ),
        )
        .await
    }

    async fn compare_and_swap_machine_lifecycle_with_fence(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected: MachineLifecycleExpectedVersion,
        replacement: MachineLifecycleCommit,
        write_fence: Arc<dyn RuntimeStoreWriteFence>,
    ) -> Result<FencedMachineLifecycleCasOutcome, RuntimeStoreError> {
        self.call(
            Mode::Mutate,
            vec![runtime_id_scope(runtime_id)],
            <InMemoryRuntimeStore as RuntimeStore>::compare_and_swap_machine_lifecycle_with_fence(
                &self.memory,
                runtime_id,
                expected,
                replacement,
                write_fence,
            ),
        )
        .await
    }

    async fn load_machine_lifecycle_record(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<Vec<u8>>, RuntimeStoreError> {
        self.call(
            Mode::ReadPinned,
            vec![runtime_id_scope(runtime_id)],
            <InMemoryRuntimeStore as RuntimeStore>::load_machine_lifecycle_record(
                &self.memory,
                runtime_id,
            ),
        )
        .await
    }

    async fn commit_machine_lifecycle(
        &self,
        runtime_id: &LogicalRuntimeId,
        commit: MachineLifecycleCommit,
        input_states: &[InputStatePersistenceRecord],
    ) -> Result<(), RuntimeStoreError> {
        self.call(
            Mode::Mutate,
            vec![runtime_id_scope(runtime_id)],
            <InMemoryRuntimeStore as RuntimeStore>::commit_machine_lifecycle(
                &self.memory,
                runtime_id,
                commit,
                input_states,
            ),
        )
        .await
    }

    async fn commit_unregister_finalization(
        &self,
        runtime_id: &LogicalRuntimeId,
        finalization: crate::store::UnregisterFinalizationCommit,
    ) -> Result<(), RuntimeStoreError> {
        self.call(
            Mode::Mutate,
            vec![runtime_id_scope(runtime_id)],
            <InMemoryRuntimeStore as RuntimeStore>::commit_unregister_finalization(
                &self.memory,
                runtime_id,
                finalization,
            ),
        )
        .await
    }

    async fn persist_ops_lifecycle(
        &self,
        runtime_id: &LogicalRuntimeId,
        snapshot: &PersistedOpsSnapshot,
    ) -> Result<(), RuntimeStoreError> {
        self.call(
            Mode::Mutate,
            vec![runtime_id_scope(runtime_id)],
            <InMemoryRuntimeStore as RuntimeStore>::persist_ops_lifecycle(
                &self.memory,
                runtime_id,
                snapshot,
            ),
        )
        .await
    }

    async fn initialize_ops_lifecycle_if_absent(
        &self,
        runtime_id: &LogicalRuntimeId,
        candidate: &PersistedOpsSnapshot,
    ) -> Result<PersistedOpsSnapshot, RuntimeStoreError> {
        self.call(
            Mode::Mutate,
            vec![runtime_id_scope(runtime_id)],
            <InMemoryRuntimeStore as RuntimeStore>::initialize_ops_lifecycle_if_absent(
                &self.memory,
                runtime_id,
                candidate,
            ),
        )
        .await
    }

    async fn load_ops_lifecycle(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<PersistedOpsSnapshot>, RuntimeStoreError> {
        self.call(
            Mode::Read,
            vec![runtime_id_scope(runtime_id)],
            <InMemoryRuntimeStore as RuntimeStore>::load_ops_lifecycle(&self.memory, runtime_id),
        )
        .await
    }

    async fn delete_ops_lifecycle(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<(), RuntimeStoreError> {
        self.call(
            Mode::Mutate,
            vec![runtime_id_scope(runtime_id)],
            <InMemoryRuntimeStore as RuntimeStore>::delete_ops_lifecycle(&self.memory, runtime_id),
        )
        .await
    }

    async fn admit_direct_member_incarnation_high_water(
        &self,
        member_session_id: &str,
        candidate: &meerkat_contracts::wire::supervisor_bridge::BridgeDirectMemberIncarnation,
    ) -> Result<
        meerkat_contracts::wire::supervisor_bridge::BridgeDirectMemberIncarnation,
        RuntimeStoreError,
    > {
        self.call(
            Mode::Mutate,
            vec![member_high_water_scope(member_session_id)],
            <InMemoryRuntimeStore as RuntimeStore>::admit_direct_member_incarnation_high_water(
                &self.memory,
                member_session_id,
                candidate,
            ),
        )
        .await
    }

    // Mob host bindings and revocations: the adapter's own record
    // scopes, one per mob for each, with the SQLite store's semantics
    // (`runtime_mob_host_bindings` and `runtime_mob_host_revocations`): CAS
    // compares the full record bytes, an insert supersedes the mob's revoke
    // receipt in the same batch, and a revoke deletes the binding and
    // publishes its receipt together or writes nothing.

    async fn load_mob_host_binding(
        &self,
        mob_id: &str,
    ) -> Result<Option<Vec<u8>>, RuntimeStoreError> {
        let _scopes = self
            .mob_host_scopes(&[mob_host_binding_scope(mob_id)], false)
            .await?;
        Ok(lock_records(&self.mob_host).bindings.get(mob_id).cloned())
    }

    async fn list_mob_host_bindings(&self) -> Result<Vec<(String, Vec<u8>)>, RuntimeStoreError> {
        let _gate = self.gate.write().await;
        self.realm
            .check_cross_scope_read()
            .map_err(SpiError::into_read)?;
        Ok(lock_records(&self.mob_host)
            .bindings
            .iter()
            .map(|(mob_id, record)| (mob_id.clone(), record.clone()))
            .collect())
    }

    async fn put_mob_host_binding_if_absent(
        &self,
        mob_id: &str,
        record_json: &[u8],
    ) -> Result<bool, RuntimeStoreError> {
        let binding = mob_host_binding_scope(mob_id);
        let revocation = mob_host_revocation_scope(mob_id);
        let scopes = self
            .mob_host_scopes(&[binding.clone(), revocation.clone()], true)
            .await?;
        if lock_records(&self.mob_host).bindings.contains_key(mob_id) {
            return Ok(false);
        }
        let mut changes = BTreeMap::new();
        changes.insert(binding, Some(record_json.to_vec()));
        changes.insert(revocation, None);
        let (mob_id, record) = (mob_id.to_string(), record_json.to_vec());
        self.commit_records(scopes, changes, move |records| {
            records.revocations.remove(&mob_id);
            records.bindings.insert(mob_id, record);
        })
        .await?;
        Ok(true)
    }

    async fn compare_and_put_mob_host_binding(
        &self,
        mob_id: &str,
        expected_json: &[u8],
        next_json: &[u8],
    ) -> Result<bool, RuntimeStoreError> {
        let binding = mob_host_binding_scope(mob_id);
        let scopes = self
            .mob_host_scopes(std::slice::from_ref(&binding), true)
            .await?;
        let matches = lock_records(&self.mob_host)
            .bindings
            .get(mob_id)
            .is_some_and(|current| current.as_slice() == expected_json);
        if !matches {
            return Ok(false);
        }
        let mut changes = BTreeMap::new();
        changes.insert(binding, Some(next_json.to_vec()));
        let (mob_id, record) = (mob_id.to_string(), next_json.to_vec());
        self.commit_records(scopes, changes, move |records| {
            records.bindings.insert(mob_id, record);
        })
        .await?;
        Ok(true)
    }

    async fn delete_mob_host_binding(
        &self,
        mob_id: &str,
        expected_json: &[u8],
    ) -> Result<bool, RuntimeStoreError> {
        let binding = mob_host_binding_scope(mob_id);
        let scopes = self
            .mob_host_scopes(std::slice::from_ref(&binding), true)
            .await?;
        let matches = lock_records(&self.mob_host)
            .bindings
            .get(mob_id)
            .is_some_and(|current| current.as_slice() == expected_json);
        if !matches {
            return Ok(false);
        }
        let mut changes = BTreeMap::new();
        changes.insert(binding, None);
        let mob_id = mob_id.to_string();
        self.commit_records(scopes, changes, move |records| {
            records.bindings.remove(&mob_id);
        })
        .await?;
        Ok(true)
    }

    async fn load_mob_host_revocation(
        &self,
        mob_id: &str,
    ) -> Result<Option<Vec<u8>>, RuntimeStoreError> {
        let _scopes = self
            .mob_host_scopes(&[mob_host_revocation_scope(mob_id)], false)
            .await?;
        Ok(lock_records(&self.mob_host)
            .revocations
            .get(mob_id)
            .cloned())
    }

    async fn list_mob_host_revocations(&self) -> Result<Vec<(String, Vec<u8>)>, RuntimeStoreError> {
        let _gate = self.gate.write().await;
        self.realm
            .check_cross_scope_read()
            .map_err(SpiError::into_read)?;
        Ok(lock_records(&self.mob_host)
            .revocations
            .iter()
            .map(|(mob_id, receipt)| (mob_id.clone(), receipt.clone()))
            .collect())
    }

    async fn revoke_mob_host_binding(
        &self,
        mob_id: &str,
        expected_binding_json: &[u8],
        receipt_json: &[u8],
    ) -> Result<bool, RuntimeStoreError> {
        let binding = mob_host_binding_scope(mob_id);
        let revocation = mob_host_revocation_scope(mob_id);
        let scopes = self
            .mob_host_scopes(&[binding.clone(), revocation.clone()], true)
            .await?;
        let matches = lock_records(&self.mob_host)
            .bindings
            .get(mob_id)
            .is_some_and(|current| current.as_slice() == expected_binding_json);
        if !matches {
            return Ok(false);
        }
        let mut changes = BTreeMap::new();
        changes.insert(binding, None);
        changes.insert(revocation, Some(receipt_json.to_vec()));
        let (mob_id, receipt) = (mob_id.to_string(), receipt_json.to_vec());
        self.commit_records(scopes, changes, move |records| {
            records.bindings.remove(&mob_id);
            records.revocations.insert(mob_id, receipt);
        })
        .await?;
        Ok(true)
    }
}
