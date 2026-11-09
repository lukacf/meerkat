//! Tests of the host store harness, unrun until the validation candidate:
//! the reference store passes the host store chapter, stores that break an
//! obligation fail it at the row that names the obligation, and the long
//! run chapters accept a Core that resumes as the contract says and refuse
//! one that calls the model again for a committed round.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};
use std::time::Duration;

use async_trait::async_trait;
use meerkat_core::host_store::{
    AcquireMode, CommitBatch, CommitId, FenceGrant, FenceTiming, FenceToken, HeadPage, HostStore,
    HostStoreError, Row, RowIndex, ScopeConflict, ScopeHead, ScopeKind, ScopeName, ScopeRef,
    ScopeUpdate, ScopeVersion, VersionedHead,
};
use meerkat_core::{RunId, ToolCallDedupeKey};
use serde_json::value::RawValue;

use crate::long_run::{
    BeforeResume, DurableWorkerHarness, KillPoint, KilledRun, KilledWorker, LongRunHarness,
    LongRunPlan, OtherWriterOutcome, RestartReport, RestartedWorker, ResumeGate, ResumePointKind,
    ResumedRun, RoundSet, call_id, resumes, takes_first_commit,
};
use crate::{ConformanceFailure, HostStoreFactory, InMemoryHostStore, chapters};

/// A store that meets obligation 4 at load: the reference store refuses the
/// second batch for one version at commit, and this store keeps it instead,
/// so the scope loads as a typed conflict at that version from then on. With
/// `per_scope` its listings report such a scope in `conflicts`; without, a
/// page that would show it fails whole, as a store that does not report a
/// conflict per scope does. With `rows_serve`, a row read of such a scope
/// answers the rows of the batch the reference store kept instead of the
/// conflict, as a store that checks its versions at head loads only does.
struct LoadConflicting {
    inner: Arc<InMemoryHostStore>,
    per_scope: bool,
    rows_serve: bool,
    newest: AtomicU64,
    conflicted: Mutex<BTreeMap<ScopeRef, ScopeVersion>>,
}

impl LoadConflicting {
    fn factory(per_scope: bool) -> Shared {
        Self::canary(per_scope, false)
    }

    /// Lists its conflicts per scope and fails their head loads, but serves
    /// their rows.
    fn serving_rows() -> Shared {
        Self::canary(true, true)
    }

    fn canary(per_scope: bool, rows_serve: bool) -> Shared {
        let inner = Arc::new(InMemoryHostStore::new());
        let store = Arc::new(Self {
            inner: Arc::clone(&inner),
            per_scope,
            rows_serve,
            newest: AtomicU64::new(0),
            conflicted: Mutex::new(BTreeMap::new()),
        });
        Shared::canary(inner, store)
    }

    fn conflict_of(&self, scope: &ScopeRef) -> Option<HostStoreError> {
        self.conflicted
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(scope)
            .map(|version| HostStoreError::Conflict {
                scope: scope.clone(),
                current: Some(*version),
            })
    }
}

#[async_trait]
impl HostStore for LoadConflicting {
    async fn acquire_fence(&self, mode: AcquireMode) -> Result<FenceGrant, HostStoreError> {
        let grant = self.inner.acquire_fence(mode).await?;
        self.newest.fetch_max(grant.token.get(), Ordering::SeqCst);
        Ok(grant)
    }

    async fn renew_fence(&self, token: FenceToken) -> Result<(), HostStoreError> {
        self.inner.renew_fence(token).await
    }

    fn fence_timing(&self) -> FenceTiming {
        self.inner.fence_timing()
    }

    async fn load_head(&self, scope: &ScopeRef) -> Result<Option<VersionedHead>, HostStoreError> {
        match self.conflict_of(scope) {
            Some(conflict) => Err(conflict),
            None => self.inner.load_head(scope).await,
        }
    }

    async fn list_heads(
        &self,
        kind: ScopeKind,
        after: Option<&ScopeName>,
        limit: usize,
    ) -> Result<HeadPage, HostStoreError> {
        let every = self.inner.list_heads(kind, after, usize::MAX).await?;
        let conflicted = self
            .conflicted
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let mut heads = Vec::new();
        let mut conflicts = Vec::new();
        let mut last = None;
        let mut more = false;
        for entry in every.heads {
            if heads.len() + conflicts.len() == limit {
                more = true;
                break;
            }
            let scope = ScopeRef::new(kind, entry.name.clone());
            last = Some(entry.name.clone());
            match conflicted.get(&scope) {
                Some(version) if self.per_scope => {
                    conflicts.push(ScopeConflict::new(entry.name, *version, Vec::new()));
                }
                Some(version) => {
                    return Err(HostStoreError::Conflict {
                        scope,
                        current: Some(*version),
                    });
                }
                None => heads.push(entry),
            }
        }
        let next = if more { last } else { None };
        Ok(HeadPage::new(heads, next).with_conflicts(conflicts))
    }

    async fn read_rows(
        &self,
        scope: &ScopeRef,
        from: RowIndex,
        limit: usize,
    ) -> Result<Vec<Row>, HostStoreError> {
        match self.conflict_of(scope) {
            Some(conflict) if !self.rows_serve => Err(conflict),
            _ => self.inner.read_rows(scope, from, limit).await,
        }
    }

    async fn commit(&self, batch: &CommitBatch) -> Result<(), HostStoreError> {
        match self.inner.commit(batch).await {
            // A second batch for a version under the current token is kept:
            // its first scope loads as a typed conflict at that version.
            Err(HostStoreError::Conflict {
                scope,
                current: Some(version),
            }) if batch.fence().get() == self.newest.load(Ordering::SeqCst) => {
                self.conflicted
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(scope, version);
                Ok(())
            }
            answer => answer,
        }
    }
}

/// Hands out handles over one store: one realm that outlives its processes.
struct Shared {
    store: Arc<dyn HostStore>,
    memory: Arc<InMemoryHostStore>,
}

impl Shared {
    fn reference(memory: InMemoryHostStore) -> Self {
        let memory = Arc::new(memory);
        Self {
            store: Arc::clone(&memory) as Arc<dyn HostStore>,
            memory,
        }
    }

    fn canary(memory: Arc<InMemoryHostStore>, store: Arc<dyn HostStore>) -> Self {
        Self { store, memory }
    }
}

#[async_trait]
impl HostStoreFactory for Shared {
    async fn open(&self) -> Result<Arc<dyn HostStore>, ConformanceFailure> {
        Ok(Arc::clone(&self.store))
    }

    async fn lapse_fence_leases(&self) -> Result<(), ConformanceFailure> {
        self.memory.lapse_fence_leases();
        Ok(())
    }
}

/// Returns `Pending` once, waking itself, so a canary's commit stops between
/// two scopes the way a remote store's commit stops between two requests.
#[derive(Default)]
struct YieldOnce(bool);

impl Future for YieldOnce {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.0 {
            Poll::Ready(())
        } else {
            self.0 = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }
}

/// The canary stores: each forwards to the reference store and breaks one
/// obligation in its commit.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Breaks {
    /// Commits every batch under the newest token: no fencing.
    Fencing,
    /// Applies a batch one scope at a time, stopping between scopes: not
    /// all or nothing.
    Atomicity,
    /// Moves each scope from its current version, whatever the batch says:
    /// two batches for one version both land.
    VersionChain,
    /// Answers a takeover with the newest token granted before it: a grant
    /// no higher than an earlier one.
    TokenOrder,
    /// Takes a commit's place in the store's order when it is first polled,
    /// and applies the batch only when the commit completes, even after a
    /// higher acquire: a batch ordered before an acquire that a load right
    /// after the acquire does not show yet.
    LateVisibility,
    /// Skips the fence check at commit: a batch under a superseded token is
    /// acknowledged and never applied, as a store that fences only at load
    /// answers it.
    CommitFence,
}

struct Canary {
    inner: Arc<InMemoryHostStore>,
    breaks: Breaks,
    newest: AtomicU64,
}

impl Canary {
    fn factory(breaks: Breaks) -> Shared {
        let inner = Arc::new(InMemoryHostStore::new());
        let canary = Arc::new(Self {
            inner: Arc::clone(&inner),
            breaks,
            newest: AtomicU64::new(0),
        });
        Shared::canary(inner, canary)
    }
}

#[async_trait]
impl HostStore for Canary {
    async fn acquire_fence(&self, mode: AcquireMode) -> Result<FenceGrant, HostStoreError> {
        let before = self.newest.load(Ordering::SeqCst);
        let grant = self.inner.acquire_fence(mode).await?;
        self.newest.fetch_max(grant.token.get(), Ordering::SeqCst);
        if self.breaks == Breaks::TokenOrder && mode == AcquireMode::Takeover && before > 0 {
            return Ok(FenceGrant {
                token: FenceToken::new(before),
                ..grant
            });
        }
        Ok(grant)
    }

    async fn renew_fence(&self, token: FenceToken) -> Result<(), HostStoreError> {
        self.inner.renew_fence(token).await
    }

    fn fence_timing(&self) -> FenceTiming {
        self.inner.fence_timing()
    }

    async fn load_head(&self, scope: &ScopeRef) -> Result<Option<VersionedHead>, HostStoreError> {
        self.inner.load_head(scope).await
    }

    async fn list_heads(
        &self,
        kind: ScopeKind,
        after: Option<&ScopeName>,
        limit: usize,
    ) -> Result<HeadPage, HostStoreError> {
        self.inner.list_heads(kind, after, limit).await
    }

    async fn read_rows(
        &self,
        scope: &ScopeRef,
        from: RowIndex,
        limit: usize,
    ) -> Result<Vec<Row>, HostStoreError> {
        self.inner.read_rows(scope, from, limit).await
    }

    async fn commit(&self, batch: &CommitBatch) -> Result<(), HostStoreError> {
        match self.breaks {
            Breaks::Fencing => {
                let newest = FenceToken::new(self.newest.load(Ordering::SeqCst));
                let rebatched = CommitBatch::try_new(
                    batch.commit_id().clone(),
                    newest,
                    batch.updates().to_vec(),
                )?;
                self.inner.commit(&rebatched).await
            }
            Breaks::Atomicity => {
                for index in 0..batch.updates().len() {
                    if index > 0 {
                        YieldOnce::default().await;
                    }
                    self.inner.apply_one_update(batch, index)?;
                }
                Ok(())
            }
            Breaks::VersionChain => {
                let mut updates = Vec::with_capacity(batch.updates().len());
                for update in batch.updates() {
                    let current = self
                        .inner
                        .load_head(&update.scope)
                        .await?
                        .map(|head| head.version);
                    let next = current
                        .map_or(Some(ScopeVersion::FIRST), ScopeVersion::next)
                        .ok_or_else(|| HostStoreError::Invalid("version overflow".to_string()))?;
                    updates.push(ScopeUpdate {
                        previous: current,
                        next,
                        ..update.clone()
                    });
                }
                let rebatched =
                    CommitBatch::try_new(batch.commit_id().clone(), batch.fence(), updates)?;
                self.inner.commit(&rebatched).await
            }
            Breaks::TokenOrder => self.inner.commit(batch).await,
            Breaks::LateVisibility => {
                if self.inner.commit_point(batch)? {
                    return Ok(());
                }
                YieldOnce::default().await;
                self.inner.apply_unfenced(batch)
            }
            Breaks::CommitFence => {
                if batch.fence().get() < self.newest.load(Ordering::SeqCst) {
                    return Ok(());
                }
                self.inner.commit(batch).await
            }
        }
    }
}

/// A handle that serves each kind's listing from its first full listing of
/// that kind, forever, applying only its own commits to it: a listing kept
/// from one granted acquire for the next.
struct StaleRelist {
    inner: Arc<InMemoryHostStore>,
    listed: Mutex<BTreeMap<ScopeKind, BTreeMap<ScopeName, VersionedHead>>>,
}

impl StaleRelist {
    fn listed(
        &self,
    ) -> std::sync::MutexGuard<'_, BTreeMap<ScopeKind, BTreeMap<ScopeName, VersionedHead>>> {
        self.listed.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Hands out a new [`StaleRelist`] handle per open, over one store: each
/// process keeps its own listing.
struct StaleRelistFactory {
    memory: Arc<InMemoryHostStore>,
}

#[async_trait]
impl HostStoreFactory for StaleRelistFactory {
    async fn open(&self) -> Result<Arc<dyn HostStore>, ConformanceFailure> {
        Ok(Arc::new(StaleRelist {
            inner: Arc::clone(&self.memory),
            listed: Mutex::new(BTreeMap::new()),
        }))
    }

    async fn lapse_fence_leases(&self) -> Result<(), ConformanceFailure> {
        self.memory.lapse_fence_leases();
        Ok(())
    }
}

#[async_trait]
impl HostStore for StaleRelist {
    async fn acquire_fence(&self, mode: AcquireMode) -> Result<FenceGrant, HostStoreError> {
        self.inner.acquire_fence(mode).await
    }

    async fn renew_fence(&self, token: FenceToken) -> Result<(), HostStoreError> {
        self.inner.renew_fence(token).await
    }

    fn fence_timing(&self) -> FenceTiming {
        self.inner.fence_timing()
    }

    async fn load_head(&self, scope: &ScopeRef) -> Result<Option<VersionedHead>, HostStoreError> {
        self.inner.load_head(scope).await
    }

    async fn list_heads(
        &self,
        kind: ScopeKind,
        after: Option<&ScopeName>,
        limit: usize,
    ) -> Result<HeadPage, HostStoreError> {
        let cached = self.listed().get(&kind).cloned();
        let heads = match cached {
            Some(heads) => heads,
            None => {
                let page = self.inner.list_heads(kind, None, usize::MAX).await?;
                let heads: BTreeMap<ScopeName, VersionedHead> = page
                    .heads
                    .into_iter()
                    .map(|entry| (entry.name, entry.head))
                    .collect();
                self.listed().insert(kind, heads.clone());
                heads
            }
        };
        let mut matching = heads
            .into_iter()
            .filter(|(name, _)| after.is_none_or(|after| name > after));
        let page: Vec<ScopeHead> = matching
            .by_ref()
            .take(limit)
            .map(|(name, head)| ScopeHead { name, head })
            .collect();
        let next = if matching.next().is_some() {
            page.last().map(|last| last.name.clone())
        } else {
            None
        };
        Ok(HeadPage::new(page, next))
    }

    async fn read_rows(
        &self,
        scope: &ScopeRef,
        from: RowIndex,
        limit: usize,
    ) -> Result<Vec<Row>, HostStoreError> {
        self.inner.read_rows(scope, from, limit).await
    }

    async fn commit(&self, batch: &CommitBatch) -> Result<(), HostStoreError> {
        self.inner.commit(batch).await?;
        let mut listed = self.listed();
        for update in batch.updates() {
            let Some(heads) = listed.get_mut(&update.scope.kind) else {
                continue;
            };
            if update.delete {
                heads.remove(&update.scope.name);
                continue;
            }
            let head = match &update.head {
                Some(head) => head.clone(),
                None => heads
                    .get(&update.scope.name)
                    .map(|known| known.head.clone())
                    .unwrap_or_default(),
            };
            heads.insert(
                update.scope.name.clone(),
                VersionedHead {
                    version: update.next,
                    commit_id: batch.commit_id().clone(),
                    head,
                },
            );
        }
        Ok(())
    }
}

#[tokio::test]
async fn reference_host_store_passes_the_host_store_chapter() {
    chapters::host_store(&Shared::reference(InMemoryHostStore::new()))
        .await
        .expect("the reference host store must satisfy the host store chapter");
}

#[tokio::test]
async fn reference_host_store_passes_with_a_host_sized_lease() {
    let timing = FenceTiming::try_new(Duration::from_secs(2), Duration::from_secs(6))
        .expect("a two second interval with a six second lease is valid");
    chapters::host_store(&Shared::reference(InMemoryHostStore::with_fence_timing(
        timing,
    )))
    .await
    .expect("the reference host store must satisfy the chapter with a host sized lease");
}

#[tokio::test]
async fn fence_blind_store_fails_the_host_store_chapter() {
    let failure = chapters::host_store(&Canary::factory(Breaks::Fencing))
        .await
        .expect_err("a store that ignores fence tokens must fail");
    assert_eq!(failure.chapter(), "host_store");
    // The kill row retries an unlanded batch under the superseded token
    // first, before the stale fence row.
    assert_eq!(failure.step(), "kill");
}

#[tokio::test]
async fn store_splitting_a_batch_fails_the_host_store_chapter() {
    let failure = chapters::host_store(&Canary::factory(Breaks::Atomicity))
        .await
        .expect_err("a store that applies a batch one scope at a time must fail");
    assert_eq!(failure.chapter(), "host_store");
    assert_eq!(failure.step(), "kill");
}

#[tokio::test]
async fn store_landing_two_batches_for_one_version_fails_the_host_store_chapter() {
    let failure = chapters::host_store(&Canary::factory(Breaks::VersionChain))
        .await
        .expect_err("a store that lands two batches for one version must fail");
    assert_eq!(failure.chapter(), "host_store");
    assert_eq!(failure.step(), "same_version_twice");
}

#[tokio::test]
async fn store_answering_a_takeover_with_an_earlier_token_fails_the_host_store_chapter() {
    let failure = chapters::host_store(&Canary::factory(Breaks::TokenOrder))
        .await
        .expect_err(
            "a store whose takeover answers no higher token than an earlier grant must fail",
        );
    assert_eq!(failure.chapter(), "host_store");
    assert_eq!(failure.step(), "token_order");
}

#[tokio::test]
async fn store_showing_an_earlier_batch_only_after_an_acquire_fails_the_host_store_chapter() {
    let failure = chapters::host_store(&Canary::factory(Breaks::LateVisibility))
        .await
        .expect_err(
            "a store that shows a batch it ordered before an acquire only after the acquire \
             returned must fail",
        );
    assert_eq!(failure.chapter(), "host_store");
    assert_eq!(failure.step(), "an_acquire_decides_the_cut");
}

#[tokio::test]
async fn store_keeping_a_listing_across_acquires_fails_the_host_store_chapter() {
    let factory = StaleRelistFactory {
        memory: Arc::new(InMemoryHostStore::new()),
    };
    let failure = chapters::host_store(&factory).await.expect_err(
        "a store whose handle serves the listing it made before another writer committed must \
         fail",
    );
    assert_eq!(failure.chapter(), "host_store");
    assert_eq!(failure.step(), "reacquire_lists_another_writers_commits");
}

// The canary passes every other row: the chapter runs its rows in order and
// this one last.
#[tokio::test]
async fn store_acknowledging_a_superseded_token_fails_only_the_commit_fence_row() {
    let failure = chapters::host_store(&Canary::factory(Breaks::CommitFence))
        .await
        .expect_err("a store that answers Ok to a commit under a superseded token must fail");
    assert_eq!(failure.chapter(), "host_store");
    assert_eq!(failure.step(), "a_superseded_token_is_refused_at_commit");
}

#[tokio::test]
async fn store_listing_its_load_conflicts_per_scope_passes_the_host_store_chapter() {
    chapters::host_store(&LoadConflicting::factory(true))
        .await
        .expect(
            "a store that loads two batches for one version as a typed conflict and lists the \
             scope per scope must satisfy the host store chapter",
        );
}

#[tokio::test]
async fn store_failing_the_page_of_a_conflicted_scope_fails_the_host_store_chapter() {
    let failure = chapters::host_store(&LoadConflicting::factory(false))
        .await
        .expect_err(
            "a store that fails a whole listing page for a scope that loads as a typed conflict \
             must fail",
        );
    assert_eq!(failure.chapter(), "host_store");
    assert_eq!(failure.step(), "a_conflicted_scope_is_listed_typed");
}

// A store that lists a conflicted scope typed and fails its head load, but
// serves one batch's rows of it, would let Core fold the rows of a scope
// whose conflict arose after the open's listing instead of quarantining it:
// the conflict row reads the scope's rows too.
#[tokio::test]
async fn store_serving_the_rows_of_a_conflicted_scope_fails_the_host_store_chapter() {
    let failure = chapters::host_store(&LoadConflicting::serving_rows())
        .await
        .expect_err(
            "a store that serves the rows of a scope whose head loads as a typed conflict must \
             fail",
        );
    assert_eq!(failure.chapter(), "host_store");
    assert_eq!(failure.step(), "a_conflicted_scope_is_listed_typed");
}

fn scope(name: &str) -> ScopeRef {
    ScopeRef::new(
        ScopeKind::Session,
        ScopeName::try_new(name).expect("valid name"),
    )
}

fn create(scope: &ScopeRef, rows: &[&str]) -> ScopeUpdate {
    ScopeUpdate {
        scope: scope.clone(),
        previous: None,
        next: ScopeVersion::FIRST,
        head: Some(b"head".to_vec()),
        append: rows
            .iter()
            .map(|row| Row::try_new(row.as_bytes().to_vec()).expect("small row"))
            .collect(),
        truncate_after: None,
        drop_before: None,
        delete: false,
    }
}

fn batch(id: &str, token: FenceToken, updates: Vec<ScopeUpdate>) -> CommitBatch {
    CommitBatch::try_new(CommitId::try_new(id).expect("valid id"), token, updates)
        .expect("valid batch")
}

#[tokio::test]
async fn reference_host_store_leases() {
    let store = InMemoryHostStore::new();
    let first = store
        .acquire_fence(AcquireMode::Exclusive)
        .await
        .expect("an empty realm grants");
    assert_eq!(first.token, FenceToken::new(1));
    assert_eq!(first.superseded_lease, Duration::ZERO);

    let lease = FenceTiming::default().lease();
    assert_eq!(
        store.acquire_fence(AcquireMode::Exclusive).await,
        Err(HostStoreError::Held {
            holder: first.token,
            remaining: lease,
        })
    );
    store.advance_clock(
        lease
            .checked_sub(Duration::from_secs(1))
            .expect("the default lease is longer than one second"),
    );
    assert_eq!(
        store.acquire_fence(AcquireMode::Exclusive).await,
        Err(HostStoreError::Held {
            holder: first.token,
            remaining: Duration::from_secs(1),
        })
    );
    store
        .renew_fence(first.token)
        .await
        .expect("the holder renews");
    let second = store
        .acquire_fence(AcquireMode::Takeover)
        .await
        .expect("a takeover always grants");
    assert_eq!(second.token, FenceToken::new(2));
    assert_eq!(second.superseded_lease, lease);
    assert_eq!(
        store.renew_fence(first.token).await,
        Err(HostStoreError::FencedOut {
            current: second.token
        })
    );
    store.lapse_fence_leases();
    let third = store
        .acquire_fence(AcquireMode::Exclusive)
        .await
        .expect("a lapsed lease does not hold the fence");
    assert_eq!(third.token, FenceToken::new(3));
    assert_eq!(third.superseded_lease, Duration::ZERO);
    assert!(matches!(
        store.renew_fence(FenceToken::new(9)).await,
        Err(HostStoreError::Invalid(_))
    ));
}

// Unrun until the candidate's validation.
// A renew of the current token after its lease lapsed: nothing superseded
// the token, so the store accepts the renew and the lease is live again.
// Stopping such a writer is Core's rule, not the store's: its self-fence
// window (`SelfFenceWindow`) closed before the lease lapsed, and a renew
// does not reopen it; only a fresh fenced acquire does.
#[tokio::test]
async fn reference_host_store_accepts_a_lapsed_current_token_renew() {
    let store = InMemoryHostStore::new();
    let grant = store
        .acquire_fence(AcquireMode::Exclusive)
        .await
        .expect("an empty realm grants");
    store.lapse_fence_leases();
    assert_eq!(
        store.renew_fence(grant.token).await,
        Ok(()),
        "the store accepts a renew of the current token after its lease lapsed"
    );
    assert_eq!(
        store.acquire_fence(AcquireMode::Exclusive).await,
        Err(HostStoreError::Held {
            holder: grant.token,
            remaining: FenceTiming::default().lease(),
        }),
        "the accepted renew made the holder's lease live again"
    );
}

#[tokio::test]
async fn reference_host_store_leases_follow_the_host_timing() {
    let timing =
        FenceTiming::try_new(Duration::from_secs(2), Duration::from_secs(6)).expect("valid timing");
    let store = InMemoryHostStore::with_fence_timing(timing);
    assert_eq!(store.fence_timing(), timing);
    let grant = store
        .acquire_fence(AcquireMode::Exclusive)
        .await
        .expect("an empty realm grants");
    assert_eq!(
        store.acquire_fence(AcquireMode::Exclusive).await,
        Err(HostStoreError::Held {
            holder: grant.token,
            remaining: Duration::from_secs(6),
        })
    );
}

#[tokio::test]
async fn reference_host_store_reads_an_outside_clock() {
    let now = Arc::new(Mutex::new(Duration::ZERO));
    let read = Arc::clone(&now);
    let store = InMemoryHostStore::with_clock(FenceTiming::default(), move || {
        *read.lock().unwrap_or_else(PoisonError::into_inner)
    });
    store
        .acquire_fence(AcquireMode::Exclusive)
        .await
        .expect("an empty realm grants");
    *now.lock().unwrap_or_else(PoisonError::into_inner) = FenceTiming::default().lease();
    let grant = store
        .acquire_fence(AcquireMode::Exclusive)
        .await
        .expect("the lease ran out on the outside clock");
    assert_eq!(grant.superseded_lease, Duration::ZERO);
}

#[tokio::test]
async fn reference_host_store_refuses_writer_rule_breaks() {
    let store = InMemoryHostStore::new();
    let token = store
        .acquire_fence(AcquireMode::Exclusive)
        .await
        .expect("an empty realm grants")
        .token;
    let s = scope("s");
    store
        .commit(&batch("c1", token, vec![create(&s, &["r0", "r1"])]))
        .await
        .expect("a create commits");

    // A landed commit id answers Ok again, and is refused with other updates.
    store
        .commit(&batch("c1", token, vec![create(&s, &["r0", "r1"])]))
        .await
        .expect("a retry of a landed batch answers Ok");
    assert!(matches!(
        store
            .commit(&batch("c1", token, vec![create(&s, &["other"])]))
            .await,
        Err(HostStoreError::Invalid(_))
    ));
    assert_eq!(
        store
            .read_rows(&s, RowIndex::new(0), 10)
            .await
            .expect("rows read")
            .len(),
        2
    );

    // A stale previous version is a typed conflict.
    let stale = ScopeUpdate {
        previous: None,
        ..create(&s, &[])
    };
    assert_eq!(
        store.commit(&batch("c2", token, vec![stale])).await,
        Err(HostStoreError::Conflict {
            scope: s.clone(),
            current: Some(ScopeVersion::FIRST),
        })
    );

    // A truncate below the first visible row is refused, and nothing of
    // its batch applies.
    let drop_first_row = ScopeUpdate {
        scope: s.clone(),
        previous: Some(ScopeVersion::FIRST),
        next: ScopeVersion::new(2),
        head: None,
        append: Vec::new(),
        truncate_after: None,
        drop_before: Some(RowIndex::new(1)),
        delete: false,
    };
    store
        .commit(&batch("c3", token, vec![drop_first_row]))
        .await
        .expect("a drop commits");
    let below = ScopeUpdate {
        scope: s.clone(),
        previous: Some(ScopeVersion::new(2)),
        next: ScopeVersion::new(3),
        head: Some(b"moved".to_vec()),
        append: Vec::new(),
        truncate_after: Some(RowIndex::new(0)),
        drop_before: None,
        delete: false,
    };
    assert!(matches!(
        store.commit(&batch("c4", token, vec![below])).await,
        Err(HostStoreError::Invalid(_))
    ));
    let head = store
        .load_head(&s)
        .await
        .expect("head loads")
        .expect("scope exists");
    assert_eq!(head.version, ScopeVersion::new(2));
    assert_eq!(head.head, b"head".to_vec());

    // A deleted name is never created again.
    let delete = ScopeUpdate {
        scope: s.clone(),
        previous: Some(ScopeVersion::new(2)),
        next: ScopeVersion::new(3),
        head: None,
        append: Vec::new(),
        truncate_after: None,
        drop_before: None,
        delete: true,
    };
    store
        .commit(&batch("c5", token, vec![delete]))
        .await
        .expect("a delete commits");
    assert_eq!(store.load_head(&s).await, Ok(None));
    assert!(matches!(
        store
            .commit(&batch("c6", token, vec![create(&s, &[])]))
            .await,
        Err(HostStoreError::Invalid(_))
    ));
}

#[test]
fn fence_timing_bounds() {
    let timing = FenceTiming::default();
    assert_eq!(timing.renew_interval(), Duration::from_secs(10));
    assert_eq!(timing.lease(), Duration::from_secs(30));
    assert_eq!(timing.self_fence_window(), Duration::from_secs(20));
    let secs = Duration::from_secs;
    assert!(FenceTiming::try_new(Duration::from_millis(500), secs(3)).is_err());
    assert!(FenceTiming::try_new(secs(2), secs(5)).is_err());
    assert!(FenceTiming::try_new(secs(10), secs(2 * 60 * 60)).is_err());
    let host = FenceTiming::try_new(secs(2), secs(6)).expect("three intervals is enough");
    assert_eq!(host.self_fence_window(), secs(4));
}

/// A Core that resumes exactly as the contract says, or (`recalls_model`)
/// one that calls the model again for a round whose calls were committed.
/// Over a store with fence leases (`fence_leases`) or one whose sessions
/// are hosted under cross-process claims; `race_requeues` plays a
/// compaction race the way the chapter's first draft expected (the input
/// runs again as a new run), which no store meets now; `race_abandons`
/// settles a fence takeover race's run with its input abandoned instead of
/// consumed.
struct ScriptedCore {
    recalls_model: bool,
    fence_leases: bool,
    race_requeues: bool,
    race_abandons: bool,
    run_id: Mutex<Option<RunId>>,
}

impl ScriptedCore {
    fn new(recalls_model: bool) -> Self {
        Self {
            recalls_model,
            fence_leases: true,
            race_requeues: false,
            race_abandons: false,
            run_id: Mutex::new(None),
        }
    }

    /// The contract Core over a store whose sessions are hosted under
    /// cross-process claims.
    fn over_hosted_sessions() -> Self {
        Self {
            fence_leases: false,
            ..Self::new(false)
        }
    }

    /// A Core that requeues a raced run's input.
    fn requeuing_the_raced_input(fence_leases: bool) -> Self {
        Self {
            fence_leases,
            race_requeues: true,
            ..Self::new(false)
        }
    }

    /// A Core whose fence takeover settles the raced run with its input
    /// abandoned.
    fn abandoning_the_raced_input() -> Self {
        Self {
            race_abandons: true,
            ..Self::new(false)
        }
    }

    fn calls(plan: &LongRunPlan, round: u64) -> Vec<String> {
        if round + 1 >= plan.rounds {
            return Vec::new();
        }
        (0..plan.calls_per_round)
            .map(|index| call_id(round, index))
            .collect()
    }

    /// The arguments the scripted model gives a call, the same each time
    /// the model is asked for its round.
    fn arguments(call: &str) -> Box<RawValue> {
        RawValue::from_string(format!("{{\"call\":\"{call}\"}}")).expect("valid json")
    }

    fn key(run_id: &RunId, round: u64, call: String) -> ToolCallDedupeKey {
        let arguments = Self::arguments(&call);
        ToolCallDedupeKey::new(run_id.clone(), round, call, "scripted_tool", &arguments)
            .expect("valid arguments")
    }

    fn kill(&self, plan: &LongRunPlan) -> KilledRun {
        let run_id = RunId::new();
        *self.run_id.lock().unwrap_or_else(PoisonError::into_inner) = Some(run_id.clone());
        // Over hosted sessions the race's writer is refused and the run
        // finishes in its first incarnation.
        let finished_first =
            plan.kill_point == KillPoint::RebaseRace && !self.fence_leases && !self.race_requeues;
        let last_round = if finished_first {
            plan.rounds
        } else {
            plan.kill_round
        };
        let mut dispatched: Vec<ToolCallDedupeKey> = (0..last_round)
            .flat_map(|round| {
                Self::calls(plan, round)
                    .into_iter()
                    .map(move |call| (round, call))
            })
            .map(|(round, call)| Self::key(&run_id, round, call))
            .collect();
        if matches!(
            plan.kill_point,
            KillPoint::DuringTools
                | KillPoint::DuringUnrepeatableTool(_)
                | KillPoint::DuringCallbackTools
        ) {
            dispatched.push(Self::key(
                &run_id,
                plan.kill_round,
                call_id(plan.kill_round, 0),
            ));
        }
        KilledRun { run_id, dispatched }
    }

    fn settled_report(&self) -> RestartReport {
        RestartReport {
            resumed: None,
            waited_out_lease: self.fence_leases,
            runs: Vec::new(),
            model_call_rounds: Vec::new(),
            dispatched: Vec::new(),
            interrupted: Vec::new(),
            transcript_rounds: Vec::new(),
            transcript_results: Vec::new(),
            receipts: 0,
            other_writer_messages: 0,
            other_writer: None,
            interrupted_notices: 0,
            other_writer_follows_interrupted_notice: false,
            inputs_settled: true,
            inputs_consumed: true,
        }
    }

    /// The restart after a compaction race the contract Core played.
    fn race_report(&self, plan: &LongRunPlan) -> RestartReport {
        if self.fence_leases {
            // The takeover settled the committed rounds as interrupted.
            let committed: Vec<u64> = (0..plan.kill_round).collect();
            RestartReport {
                transcript_results: committed
                    .iter()
                    .flat_map(|round| Self::calls(plan, *round))
                    .collect(),
                transcript_rounds: committed,
                receipts: 1,
                other_writer_messages: 1,
                other_writer: Some(OtherWriterOutcome::Committed),
                interrupted_notices: 1,
                other_writer_follows_interrupted_notice: true,
                inputs_consumed: !self.race_abandons,
                ..self.settled_report()
            }
        } else {
            // The refused writer left the run to finish.
            let first_transcript_round = plan.compactions.last().unwrap_or(0);
            let rounds: Vec<u64> = (first_transcript_round..plan.rounds).collect();
            RestartReport {
                transcript_results: rounds
                    .iter()
                    .flat_map(|round| Self::calls(plan, *round))
                    .collect(),
                transcript_rounds: rounds,
                receipts: 1,
                other_writer: Some(OtherWriterOutcome::ServedElsewhere),
                ..self.settled_report()
            }
        }
    }

    fn restart_report(&self, plan: &LongRunPlan) -> RestartReport {
        if matches!(
            plan.before_resume,
            BeforeResume::Stop | BeforeResume::Retire
        ) {
            return self.settled_report();
        }
        let race = plan.kill_point == KillPoint::RebaseRace;
        if race && !self.race_requeues {
            return self.race_report(plan);
        }
        let killed_run = self
            .run_id
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .unwrap_or_default();
        let kill_round = plan.kill_round;
        let resumed_run = resumes(plan);
        let tools_pending = resumed_run
            && !matches!(
                plan.kill_point,
                KillPoint::ModelCall | KillPoint::RebaseRace
            )
            && takes_first_commit(plan, kill_round);
        let input_run = if resumed_run {
            killed_run
        } else {
            RunId::new()
        };
        let mut runs = vec![input_run.clone()];
        if plan.before_resume == BeforeResume::QueueInput {
            runs.push(RunId::new());
        }
        let first_round = if resumed_run { kill_round } else { 0 };
        let first_model_round = if tools_pending && !self.recalls_model {
            kill_round + 1
        } else {
            first_round
        };
        let interrupted = (tools_pending
            && matches!(plan.kill_point, KillPoint::DuringUnrepeatableTool(_)))
        .then(|| call_id(kill_round, 0));
        let dispatched = (first_round..plan.rounds)
            .flat_map(|round| {
                Self::calls(plan, round)
                    .into_iter()
                    .map(move |call| (round, call))
            })
            .filter(|(_, call)| interrupted.as_ref() != Some(call))
            .map(|(round, call)| Self::key(&input_run, round, call))
            .collect();
        let first_transcript_round = plan.compactions.last().unwrap_or(0);
        let transcript_rounds: Vec<u64> = (first_transcript_round..plan.rounds).collect();
        let transcript_results = transcript_rounds
            .iter()
            .flat_map(|round| Self::calls(plan, *round))
            .collect();
        let resumed = resumed_run.then(|| {
            if tools_pending {
                ResumedRun {
                    run_id: input_run.clone(),
                    round: kill_round,
                    resume_point: ResumePointKind::ToolsPending,
                }
            } else {
                ResumedRun {
                    run_id: input_run.clone(),
                    round: kill_round.saturating_sub(1),
                    resume_point: ResumePointKind::AwaitingModel,
                }
            }
        });
        RestartReport {
            resumed,
            waited_out_lease: self.fence_leases,
            runs,
            model_call_rounds: (first_model_round..plan.rounds).collect(),
            dispatched,
            interrupted: interrupted.into_iter().collect(),
            transcript_rounds,
            transcript_results,
            receipts: 1,
            other_writer_messages: usize::from(race),
            other_writer: race.then_some(OtherWriterOutcome::Committed),
            interrupted_notices: 0,
            other_writer_follows_interrupted_notice: false,
            inputs_settled: true,
            inputs_consumed: true,
        }
    }
}

#[async_trait]
impl LongRunHarness for ScriptedCore {
    fn fence_leases(&self) -> bool {
        self.fence_leases
    }

    async fn run_and_kill(&self, plan: &LongRunPlan) -> Result<KilledRun, ConformanceFailure> {
        Ok(self.kill(plan))
    }

    async fn restart(&self, plan: &LongRunPlan) -> Result<RestartReport, ConformanceFailure> {
        Ok(self.restart_report(plan))
    }
}

#[async_trait]
impl DurableWorkerHarness for ScriptedCore {
    fn fence_leases(&self) -> bool {
        self.fence_leases
    }

    async fn spawn_and_kill(&self, plan: &LongRunPlan) -> Result<KilledWorker, ConformanceFailure> {
        Ok(KilledWorker {
            identity: "worker".to_string(),
            session: "worker-session".to_string(),
            run: self.kill(plan),
        })
    }

    async fn restart(
        &self,
        plan: &LongRunPlan,
        gate: ResumeGate,
    ) -> Result<RestartedWorker, ConformanceFailure> {
        let refused = gate == ResumeGate::Superseded;
        let report = match gate {
            ResumeGate::Superseded => self.settled_report(),
            // The gate could not decide: the run stays open for the next
            // start, nothing of it runs and nothing of it ends.
            ResumeGate::Unavailable => RestartReport {
                inputs_settled: false,
                inputs_consumed: false,
                ..self.settled_report()
            },
            _ => self.restart_report(plan),
        };
        Ok(RestartedWorker {
            identity: "worker".to_string(),
            session: "worker-session".to_string(),
            refused,
            report,
        })
    }
}

#[tokio::test]
async fn long_run_chapter_accepts_a_core_that_resumes_as_the_contract_says() {
    chapters::long_run(&ScriptedCore::new(false))
        .await
        .expect("a contract Core passes the long run chapter");
    chapters::compacted_long_run(&ScriptedCore::new(false))
        .await
        .expect("a contract Core passes the compacted long run chapter");
    chapters::durable_worker(&ScriptedCore::new(false))
        .await
        .expect("a contract Core passes the durable worker chapter");
}

/// The compaction race over a store whose sessions are hosted under
/// cross-process claims: the writer is refused and the run finishes.
#[tokio::test]
async fn compacted_long_run_chapter_accepts_a_core_over_hosted_sessions() {
    chapters::compacted_long_run(&ScriptedCore::over_hosted_sessions())
        .await
        .expect("a contract Core over hosted sessions passes the compacted long run chapter");
}

/// Requeuing a raced run's input runs its committed rounds' tool calls
/// again: the race rows of both store kinds refuse it.
#[tokio::test]
async fn compacted_long_run_chapter_refuses_a_core_that_requeues_the_raced_input() {
    for (fence_leases, step) in [
        (
            true,
            "compaction_racing_a_fence_takeover_settles_the_run_as_interrupted",
        ),
        (
            false,
            "compaction_racing_the_sessions_host_refuses_the_writer",
        ),
    ] {
        let failure =
            chapters::compacted_long_run(&ScriptedCore::requeuing_the_raced_input(fence_leases))
                .await
                .expect_err("requeuing the raced input must fail the race row");
        assert_eq!(failure.chapter(), "compacted_long_run");
        assert_eq!(failure.step(), step);
    }
}

/// A fence takeover that settles the raced run with its input abandoned
/// leaves the input settled but not consumed: the fence race row refuses it.
#[tokio::test]
async fn compacted_long_run_chapter_refuses_a_core_that_abandons_the_raced_input() {
    let failure = chapters::compacted_long_run(&ScriptedCore::abandoning_the_raced_input())
        .await
        .expect_err("abandoning the raced input must fail the fence race row");
    assert_eq!(failure.chapter(), "compacted_long_run");
    assert_eq!(
        failure.step(),
        "compaction_racing_a_fence_takeover_settles_the_run_as_interrupted"
    );
}

#[tokio::test]
async fn long_run_chapter_refuses_a_core_that_calls_the_model_again() {
    let failure = chapters::long_run(&ScriptedCore::new(true))
        .await
        .expect_err("calling the model again for a committed round must fail");
    assert_eq!(failure.chapter(), "long_run");
    assert_eq!(failure.step(), "first_round_tools");
}

#[test]
fn first_commit_rule() {
    let plain = LongRunPlan {
        rounds: 5,
        calls_per_round: 2,
        kill_round: 3,
        kill_point: KillPoint::DuringTools,
        compactions: RoundSet::EMPTY.with(3),
        image_rounds: RoundSet::EMPTY,
        before_resume: BeforeResume::Nothing,
    };
    // Every tool round takes its first commit, round 0, the round after a
    // compaction and a callback batch included.
    assert!(takes_first_commit(&plain, 0));
    assert!(takes_first_commit(&plain, 3));
    assert!(takes_first_commit(
        &LongRunPlan {
            kill_point: KillPoint::DuringCallbackTools,
            ..plain
        },
        3
    ));
    // A round whose assistant message carries an image takes none.
    let image = LongRunPlan {
        image_rounds: RoundSet::EMPTY.with(3),
        ..plain
    };
    assert!(!takes_first_commit(&image, 3));
    assert!(takes_first_commit(&image, 2));
    // Round 0 resumes after its first commit; a kill during its model call,
    // a round 0 image round and a compaction race requeue.
    assert!(resumes(&plain));
    let first_round = LongRunPlan {
        kill_round: 0,
        ..plain
    };
    assert!(resumes(&first_round));
    assert!(!resumes(&LongRunPlan {
        kill_point: KillPoint::ModelCall,
        ..first_round
    }));
    assert!(!resumes(&LongRunPlan {
        image_rounds: RoundSet::EMPTY.with(0),
        ..first_round
    }));
    assert!(!resumes(&LongRunPlan {
        kill_point: KillPoint::RebaseRace,
        ..plain
    }));
}

#[test]
fn round_sets() {
    let set = RoundSet::EMPTY.with(2).with(6).with(4);
    assert!(set.contains(2) && set.contains(4) && set.contains(6));
    assert!(!set.contains(3) && !set.contains(64));
    assert_eq!(set.last(), Some(6));
    assert_eq!(set.len(), 3);
    assert!(RoundSet::EMPTY.is_empty() && RoundSet::EMPTY.last().is_none());
    assert_eq!(RoundSet::EMPTY.with(64), RoundSet::EMPTY);
}
