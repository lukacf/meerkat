//! The reference host store: one realm held in memory behind one lock.
//!
//! It meets every obligation of the host store contract at commit: a batch
//! whose previous version is not current is refused
//! ([`HostStoreError::Conflict`]), a batch carrying a token below the highest
//! acquired is refused ([`HostStoreError::FencedOut`]), a commit id that
//! already landed answers `Ok` without acting again, and each batch applies
//! completely under the lock or not at all. The lock is never held across an
//! await.
//!
//! It is stricter than the contract requires where that catches a writer
//! breaking its own rules: it refuses, as [`HostStoreError::Invalid`], a
//! commit id that landed being sent again with different updates, and a
//! deleted scope name being created again.
//!
//! Fence leases run on the store's own clock. By default it is a manual
//! clock that only [`InMemoryHostStore::advance_clock`] and
//! [`InMemoryHostStore::lapse_fence_leases`] move, so lease rows are
//! deterministic; [`InMemoryHostStore::with_clock`] reads another time
//! source instead (for example the elapsed time of a paused test runtime),
//! plus whatever those two added.
//!
//! It is not durable across processes. A conformance factory hands out
//! handles over one shared instance, which is how a test kills every Core
//! object and reopens over the same store: the store outlives the "process".

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use meerkat_core::host_store::{
    AcquireMode, CommitBatch, CommitId, FenceGrant, FenceTiming, FenceToken, HeadPage, HostStore,
    HostStoreError, Row, RowIndex, ScopeHead, ScopeKind, ScopeName, ScopeRef, ScopeUpdate,
    ScopeVersion, VersionedHead,
};

/// One visible scope.
#[derive(Debug, Clone)]
struct ScopeState {
    version: ScopeVersion,
    commit_id: CommitId,
    head: Vec<u8>,
    /// The index of the first visible row.
    first: u64,
    /// The visible rows, from `first` on.
    rows: Vec<Row>,
}

impl ScopeState {
    /// The index the next appended row takes.
    fn end(&self) -> u64 {
        self.first.saturating_add(self.rows.len() as u64)
    }

    fn versioned_head(&self) -> VersionedHead {
        VersionedHead {
            version: self.version,
            commit_id: self.commit_id.clone(),
            head: self.head.clone(),
        }
    }
}

/// Where the store's clock reads its time: nowhere (a manual clock) or an
/// outside time source.
#[derive(Clone, Default)]
struct StoreClock(Option<Arc<dyn Fn() -> Duration + Send + Sync>>);

impl StoreClock {
    fn read(&self) -> Duration {
        self.0.as_ref().map_or(Duration::ZERO, |read| read())
    }
}

impl fmt::Debug for StoreClock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(if self.0.is_some() {
            "StoreClock(outside)"
        } else {
            "StoreClock(manual)"
        })
    }
}

/// The realm's state.
#[derive(Debug, Default)]
struct Realm {
    /// How far tests moved the store's clock past its time source.
    advanced: Duration,
    /// The highest fence token acquired; 0 before the first acquire.
    fence: u64,
    /// When the lease of the holder of `fence` ends, on the store's clock.
    lease_end: Duration,
    scopes: BTreeMap<ScopeRef, ScopeState>,
    /// Every scope deleted so far: its name is never created again.
    deleted: BTreeSet<ScopeRef>,
    /// Every commit id that landed, with a fingerprint of its updates, so
    /// that a retry acts once and a reused id is caught.
    landed: BTreeMap<CommitId, u64>,
}

impl Realm {
    /// What remains at `now` of the current holder's lease; zero when none
    /// is live.
    fn live_lease(&self, now: Duration) -> Duration {
        if self.fence == 0 {
            Duration::ZERO
        } else {
            self.lease_end.saturating_sub(now)
        }
    }

    /// Refuse a token this realm never acquired.
    fn check_acquired(&self, token: FenceToken) -> Result<(), HostStoreError> {
        if token.get() == 0 || token.get() > self.fence {
            return Err(HostStoreError::Invalid(format!(
                "token {token} was never acquired"
            )));
        }
        Ok(())
    }
}

/// The reference [`HostStore`]: see the module documentation.
#[derive(Debug, Default)]
pub struct InMemoryHostStore {
    timing: FenceTiming,
    clock: StoreClock,
    realm: Mutex<Realm>,
}

impl InMemoryHostStore {
    /// An empty realm with the default fence timing and a manual clock.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// An empty realm whose leases follow `timing`, on a manual clock.
    #[must_use]
    pub fn with_fence_timing(timing: FenceTiming) -> Self {
        Self {
            timing,
            ..Self::default()
        }
    }

    /// An empty realm whose leases follow `timing`, on a clock that reads
    /// `now` (time since any fixed start).
    #[must_use]
    pub fn with_clock(
        timing: FenceTiming,
        now: impl Fn() -> Duration + Send + Sync + 'static,
    ) -> Self {
        Self {
            timing,
            clock: StoreClock(Some(Arc::new(now))),
            realm: Mutex::default(),
        }
    }

    /// Move the store's clock forward by `by`.
    pub fn advance_clock(&self, by: Duration) {
        let mut realm = self.realm();
        realm.advanced = realm.advanced.saturating_add(by);
    }

    /// Move the store's clock to the end of the current holder's lease, so
    /// that no holder is live.
    pub fn lapse_fence_leases(&self) {
        let mut realm = self.realm();
        let left = realm.lease_end.saturating_sub(self.now(&realm));
        realm.advanced = realm.advanced.saturating_add(left);
    }

    fn now(&self, realm: &Realm) -> Duration {
        self.clock.read().saturating_add(realm.advanced)
    }

    fn realm(&self) -> MutexGuard<'_, Realm> {
        self.realm.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A fingerprint of what a batch does, without its fence token: a retry
/// after a restart carries the same updates.
fn fingerprint(batch: &CommitBatch) -> u64 {
    let mut hasher = DefaultHasher::new();
    for update in batch.updates() {
        update.scope.hash(&mut hasher);
        update.previous.hash(&mut hasher);
        update.next.hash(&mut hasher);
        update.head.hash(&mut hasher);
        update.append.len().hash(&mut hasher);
        for row in &update.append {
            row.as_bytes().hash(&mut hasher);
        }
        update.truncate_after.hash(&mut hasher);
        update.drop_before.hash(&mut hasher);
        update.delete.hash(&mut hasher);
    }
    hasher.finish()
}

/// Refuse what an update would do before anything of its batch applies, so
/// that a batch applies completely or not at all.
fn check_update(realm: &Realm, update: &ScopeUpdate) -> Result<(), HostStoreError> {
    let state = realm.scopes.get(&update.scope);
    let current = state.map(|state| state.version);
    if current != update.previous {
        return Err(HostStoreError::Conflict {
            scope: update.scope.clone(),
            current,
        });
    }
    if update.previous.is_none() && realm.deleted.contains(&update.scope) {
        return Err(HostStoreError::Invalid(format!(
            "{} was deleted, and a deleted scope name is never created again",
            update.scope
        )));
    }
    if let (Some(truncate), Some(state)) = (update.truncate_after, state)
        && truncate.get() < state.first
    {
        return Err(HostStoreError::Invalid(format!(
            "{}: a truncate after {truncate} reaches below the first visible row {}",
            update.scope, state.first
        )));
    }
    Ok(())
}

/// Apply one checked update.
fn apply(realm: &mut Realm, update: &ScopeUpdate, commit_id: &CommitId) {
    if update.delete {
        realm.scopes.remove(&update.scope);
        realm.deleted.insert(update.scope.clone());
        return;
    }
    let state = realm
        .scopes
        .entry(update.scope.clone())
        .or_insert_with(|| ScopeState {
            version: update.next,
            commit_id: commit_id.clone(),
            head: Vec::new(),
            first: 0,
            rows: Vec::new(),
        });
    if let Some(truncate) = update.truncate_after
        && truncate.get() < state.end()
    {
        let keep =
            usize::try_from(truncate.get().saturating_sub(state.first)).unwrap_or(usize::MAX);
        state.rows.truncate(keep);
    }
    if let Some(drop) = update.drop_before
        && drop.get() > state.first
    {
        let new_first = drop.get().min(state.end());
        let gone = usize::try_from(new_first.saturating_sub(state.first)).unwrap_or(usize::MAX);
        state.rows.drain(..gone.min(state.rows.len()));
        state.first = new_first;
    }
    state.rows.extend(update.append.iter().cloned());
    if let Some(head) = &update.head {
        state.head.clone_from(head);
    }
    state.version = update.next;
    state.commit_id.clone_from(commit_id);
}

/// Hooks for the canary stores of this crate's tests, which break the
/// contract on purpose.
#[cfg(all(test, not(target_arch = "wasm32")))]
impl InMemoryHostStore {
    /// Apply update `index` of `batch` on its own, checked as a commit
    /// would check it, without marking the commit id as landed.
    pub(crate) fn apply_one_update(
        &self,
        batch: &CommitBatch,
        index: usize,
    ) -> Result<(), HostStoreError> {
        let mut realm = self.realm();
        realm.check_acquired(batch.fence())?;
        if batch.fence().get() < realm.fence {
            return Err(HostStoreError::FencedOut {
                current: FenceToken::new(realm.fence),
            });
        }
        let update = batch
            .updates()
            .get(index)
            .ok_or_else(|| HostStoreError::Invalid(format!("no update {index}")))?;
        check_update(&realm, update)?;
        apply(&mut realm, update, batch.commit_id());
        Ok(())
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl HostStore for InMemoryHostStore {
    async fn acquire_fence(&self, mode: AcquireMode) -> Result<FenceGrant, HostStoreError> {
        let mut realm = self.realm();
        let now = self.now(&realm);
        let live = realm.live_lease(now);
        if mode == AcquireMode::Exclusive && !live.is_zero() {
            return Err(HostStoreError::Held {
                holder: FenceToken::new(realm.fence),
                remaining: live,
            });
        }
        realm.fence = realm
            .fence
            .checked_add(1)
            .ok_or_else(|| HostStoreError::Unavailable("fence tokens exhausted".to_string()))?;
        realm.lease_end = now.saturating_add(self.timing.lease());
        Ok(FenceGrant {
            token: FenceToken::new(realm.fence),
            superseded_lease: live,
        })
    }

    async fn renew_fence(&self, token: FenceToken) -> Result<(), HostStoreError> {
        let mut realm = self.realm();
        realm.check_acquired(token)?;
        if token.get() < realm.fence {
            return Err(HostStoreError::FencedOut {
                current: FenceToken::new(realm.fence),
            });
        }
        realm.lease_end = self.now(&realm).saturating_add(self.timing.lease());
        Ok(())
    }

    fn fence_timing(&self) -> FenceTiming {
        self.timing
    }

    async fn load_head(&self, scope: &ScopeRef) -> Result<Option<VersionedHead>, HostStoreError> {
        Ok(self
            .realm()
            .scopes
            .get(scope)
            .map(ScopeState::versioned_head))
    }

    async fn list_heads(
        &self,
        kind: ScopeKind,
        after: Option<&ScopeName>,
        limit: usize,
    ) -> Result<HeadPage, HostStoreError> {
        if limit == 0 {
            return Err(HostStoreError::Invalid(
                "a listing limit of zero".to_string(),
            ));
        }
        let realm = self.realm();
        let mut matching = realm
            .scopes
            .iter()
            .filter(|(scope, _)| scope.kind == kind)
            .filter(|(scope, _)| after.is_none_or(|after| scope.name > *after));
        let heads: Vec<ScopeHead> = matching
            .by_ref()
            .take(limit)
            .map(|(scope, state)| ScopeHead {
                name: scope.name.clone(),
                head: state.versioned_head(),
            })
            .collect();
        let next = if matching.next().is_some() {
            heads.last().map(|last| last.name.clone())
        } else {
            None
        };
        Ok(HeadPage { heads, next })
    }

    async fn read_rows(
        &self,
        scope: &ScopeRef,
        from: RowIndex,
        limit: usize,
    ) -> Result<Vec<Row>, HostStoreError> {
        let realm = self.realm();
        let Some(state) = realm.scopes.get(scope) else {
            return Ok(Vec::new());
        };
        if from.get() < state.first {
            return Err(HostStoreError::Invalid(format!(
                "{scope}: rows before {} were dropped",
                state.first
            )));
        }
        let skip = usize::try_from(from.get() - state.first).unwrap_or(usize::MAX);
        Ok(state.rows.iter().skip(skip).take(limit).cloned().collect())
    }

    async fn commit(&self, batch: &CommitBatch) -> Result<(), HostStoreError> {
        let print = fingerprint(batch);
        let mut realm = self.realm();
        // A retry of a batch that landed acts once: it answers what the
        // first attempt did, whatever was acquired or written since.
        if let Some(landed) = realm.landed.get(batch.commit_id()) {
            if *landed == print {
                return Ok(());
            }
            return Err(HostStoreError::Invalid(format!(
                "commit id {} landed with different updates",
                batch.commit_id()
            )));
        }
        realm.check_acquired(batch.fence())?;
        if batch.fence().get() < realm.fence {
            return Err(HostStoreError::FencedOut {
                current: FenceToken::new(realm.fence),
            });
        }
        for update in batch.updates() {
            check_update(&realm, update)?;
        }
        for update in batch.updates() {
            apply(&mut realm, update, batch.commit_id());
        }
        realm.landed.insert(batch.commit_id().clone(), print);
        Ok(())
    }
}
