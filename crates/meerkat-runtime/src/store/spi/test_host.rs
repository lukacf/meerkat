//! An in-memory host store for the adapters' tests. It meets every
//! obligation at commit: a stale previous version answers `Conflict`, a
//! lower fence token `FencedOut`, a repeated commit id lands once, and a
//! deleted name is refused if created again. Faults are injected per call.

use std::collections::{BTreeMap, HashSet};
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use meerkat_core::host_store::{
    AcquireMode, CommitBatch, CommitId, FenceGrant, FenceTiming, FenceToken, HeadPage, HostStore,
    HostStoreError, Row, RowIndex, ScopeHead, ScopeKind, ScopeName, ScopeRef, ScopeVersion,
    VersionedHead,
};
use tokio::time::Instant;

#[derive(Debug, Clone)]
pub(crate) struct StoredScope {
    pub(crate) version: ScopeVersion,
    pub(crate) commit_id: CommitId,
    pub(crate) head: Vec<u8>,
    pub(crate) start: u64,
    pub(crate) rows: BTreeMap<u64, Vec<u8>>,
    pub(crate) end: u64,
}

/// One call the host answered or refused, with the scope it named (a
/// listing names its kind, a commit names none).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HostCall {
    AcquireFence,
    RenewFence,
    LoadHead { kind: ScopeKind, name: String },
    ListHeads { kind: ScopeKind },
    ReadRows { kind: ScopeKind, name: String },
    Commit,
}

/// One visible scope as a row reads it, without copying its rows: its
/// name, its head, its first visible row, the index its next row takes, and
/// the length of each visible row by index.
#[derive(Debug, Clone)]
pub(crate) struct ScopeWindow {
    pub(crate) name: String,
    pub(crate) head: Vec<u8>,
    pub(crate) start: u64,
    pub(crate) end: u64,
    pub(crate) row_lens: BTreeMap<u64, usize>,
}

#[derive(Debug, Default)]
struct HostState {
    token: u64,
    lease_until: Option<Instant>,
    scopes: BTreeMap<(ScopeKind, String), StoredScope>,
    deleted: HashSet<(ScopeKind, String)>,
    landed: HashSet<String>,
    commits: u64,
}

/// Faults the next calls meet.
#[derive(Debug, Default)]
pub(crate) struct Faults {
    /// Commits that apply and then answer `Indeterminate`.
    pub(crate) indeterminate_after_apply: u32,
    /// Commits that answer `Unavailable` without applying.
    pub(crate) unavailable: u32,
    /// Commits that answer `Conflict` without applying.
    pub(crate) conflict: u32,
    /// Renews that answer `Unavailable`.
    pub(crate) renew_unavailable: u32,
    /// Renews that never answer: the realm's renew timeout gives up on
    /// them.
    pub(crate) renew_hang: u32,
    /// Acquires that answer `Unavailable`.
    pub(crate) acquire_unavailable: u32,
    /// Acquires that never answer and grant nothing: the realm's acquire
    /// timeout gives up on them.
    pub(crate) acquire_hang: u32,
    /// How long every acquire takes to answer: the host decides when the
    /// call arrives, and the answer reaches the caller this much later, as
    /// over a slow network.
    pub(crate) acquire_delay: Option<Duration>,
    /// Head loads that answer `Unavailable`.
    pub(crate) load_head_unavailable: u32,
    /// Retries of a batch that already landed that answer `Unavailable`
    /// instead of `Ok`.
    pub(crate) landed_retry_unavailable: u32,
    /// How long every renew takes before it answers.
    pub(crate) renew_delay: Option<Duration>,
    /// How long the next listing call takes before it answers (once).
    pub(crate) list_delay: Option<Duration>,
    /// The most heads every listing call answers, whatever limit it was
    /// asked for: a store whose pages are shorter than Core's.
    pub(crate) list_page_cap: Option<usize>,
    /// How long every row read of a session scope takes before it answers.
    /// The reads waiting meanwhile are counted ([`TestHost::session_read_peak`]).
    pub(crate) session_read_delay: Option<Duration>,
    /// Row reads of the session scopes whose host name starts with this
    /// prefix wait this long (instead of `session_read_delay`) and then answer
    /// `Unavailable`.
    pub(crate) failing_session_reads: Option<(String, Duration)>,
}

/// The test host store.
#[derive(Debug)]
pub(crate) struct TestHost {
    state: Mutex<HostState>,
    timing: FenceTiming,
    pub(crate) faults: Mutex<Faults>,
    /// When set, every commit waits here until notified.
    pub(crate) commit_gate: Mutex<Option<std::sync::Arc<tokio::sync::Notify>>>,
    /// Commits that started, counted before they wait at the gate.
    pub(crate) commits_started: std::sync::atomic::AtomicU64,
    /// Reads of the host's scopes: head loads, head listings and row reads.
    reads: std::sync::atomic::AtomicU64,
    /// Row reads of session scopes waiting at `Faults::session_read_delay`
    /// now, and the most that waited at once.
    session_reads_waiting: std::sync::atomic::AtomicUsize,
    session_reads_peak: std::sync::atomic::AtomicUsize,
    /// Every call, in order, until a row takes them.
    calls: Mutex<Vec<HostCall>>,
}

/// One row read waiting at its delay, counted until it is dropped (answered
/// or abandoned).
struct WaitingRead<'a>(&'a std::sync::atomic::AtomicUsize);

impl<'a> WaitingRead<'a> {
    fn enter(
        waiting: &'a std::sync::atomic::AtomicUsize,
        peak: &std::sync::atomic::AtomicUsize,
    ) -> Self {
        let now = waiting.fetch_add(1, std::sync::atomic::Ordering::AcqRel) + 1;
        peak.fetch_max(now, std::sync::atomic::Ordering::AcqRel);
        Self(waiting)
    }
}

impl Drop for WaitingRead<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
    }
}

impl TestHost {
    pub(crate) fn new() -> Self {
        Self::with_timing(FenceTiming::default())
    }

    pub(crate) fn with_timing(timing: FenceTiming) -> Self {
        Self {
            state: Mutex::new(HostState::default()),
            timing,
            faults: Mutex::new(Faults::default()),
            commit_gate: Mutex::new(None),
            commits_started: std::sync::atomic::AtomicU64::new(0),
            reads: std::sync::atomic::AtomicU64::new(0),
            session_reads_waiting: std::sync::atomic::AtomicUsize::new(0),
            session_reads_peak: std::sync::atomic::AtomicUsize::new(0),
            calls: Mutex::new(Vec::new()),
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, HostState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn faults(&self) -> std::sync::MutexGuard<'_, Faults> {
        self.faults
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn count_read(&self) {
        self.reads.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
    }

    fn record(&self, call: HostCall) {
        self.calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(call);
    }

    /// Every call recorded since the last take, in order.
    pub(crate) fn take_calls(&self) -> Vec<HostCall> {
        std::mem::take(
            &mut *self
                .calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }

    /// How many visible scopes of `kind` the host holds.
    pub(crate) fn scope_count(&self, kind: ScopeKind) -> usize {
        self.state()
            .scopes
            .keys()
            .filter(|(scope_kind, _)| *scope_kind == kind)
            .count()
    }

    /// The first visible scope of `kind`, by name, whose name starts with
    /// `prefix`.
    pub(crate) fn window_of(&self, kind: ScopeKind, prefix: &str) -> Option<ScopeWindow> {
        let state = self.state();
        state
            .scopes
            .iter()
            .find(|((scope_kind, name), _)| *scope_kind == kind && name.starts_with(prefix))
            .map(|((_, name), stored)| ScopeWindow {
                name: name.clone(),
                head: stored.head.clone(),
                start: stored.start,
                end: stored.end,
                row_lens: stored
                    .rows
                    .iter()
                    .map(|(index, row)| (*index, row.len()))
                    .collect(),
            })
    }

    /// The visible row `index` of the scope of `kind` named `name`.
    pub(crate) fn row_at(&self, kind: ScopeKind, name: &str, index: u64) -> Option<Vec<u8>> {
        self.state()
            .scopes
            .get(&(kind, name.to_string()))
            .and_then(|stored| stored.rows.get(&index).cloned())
    }

    /// How many reads of the host's scopes (head loads, head listings and
    /// row reads) the host answered or refused.
    pub(crate) fn read_count(&self) -> u64 {
        self.reads.load(std::sync::atomic::Ordering::Acquire)
    }

    /// The most row reads of session scopes that waited at
    /// `Faults::session_read_delay` at once.
    pub(crate) fn session_read_peak(&self) -> usize {
        self.session_reads_peak
            .load(std::sync::atomic::Ordering::Acquire)
    }

    /// End the current holder's lease now (a crashed writer's lease ran
    /// out), so an exclusive acquire succeeds.
    pub(crate) fn expire_lease(&self) {
        self.state().lease_until = None;
    }

    /// Every visible scope of a kind, by name.
    pub(crate) fn scopes_of(&self, kind: ScopeKind) -> BTreeMap<String, StoredScope> {
        self.state()
            .scopes
            .iter()
            .filter(|((scope_kind, _), _)| *scope_kind == kind)
            .map(|((_, name), scope)| (name.clone(), scope.clone()))
            .collect()
    }

    /// How many batches landed.
    pub(crate) fn commit_count(&self) -> u64 {
        self.state().commits
    }

    /// An acquire's answer, decided when the call arrives.
    fn decide_acquire(&self, mode: AcquireMode) -> Result<FenceGrant, HostStoreError> {
        let now = Instant::now();
        let mut state = self.state();
        let remaining = state
            .lease_until
            .filter(|until| *until > now)
            .map(|until| until - now);
        if let Some(remaining) = remaining
            && mode == AcquireMode::Exclusive
        {
            return Err(HostStoreError::Held {
                holder: FenceToken::new(state.token),
                remaining,
            });
        }
        state.token += 1;
        state.lease_until = Some(now + self.timing.lease());
        Ok(FenceGrant {
            token: FenceToken::new(state.token),
            superseded_lease: remaining.unwrap_or(Duration::ZERO),
        })
    }
}

#[async_trait]
impl HostStore for TestHost {
    async fn acquire_fence(&self, mode: AcquireMode) -> Result<FenceGrant, HostStoreError> {
        self.record(HostCall::AcquireFence);
        let (delay, hang) = {
            let mut faults = self.faults();
            if faults.acquire_unavailable > 0 {
                faults.acquire_unavailable -= 1;
                return Err(HostStoreError::Unavailable("injected".into()));
            }
            let hang = faults.acquire_hang > 0;
            if hang {
                faults.acquire_hang -= 1;
            }
            (faults.acquire_delay, hang)
        };
        if hang {
            std::future::pending::<()>().await;
        }
        let answer = self.decide_acquire(mode);
        if let Some(delay) = delay {
            tokio::time::sleep(delay).await;
        }
        answer
    }

    async fn renew_fence(&self, token: FenceToken) -> Result<(), HostStoreError> {
        self.record(HostCall::RenewFence);
        let (delay, hang) = {
            let mut faults = self.faults();
            if faults.renew_unavailable > 0 {
                faults.renew_unavailable -= 1;
                return Err(HostStoreError::Unavailable("injected".into()));
            }
            let hang = faults.renew_hang > 0;
            if hang {
                faults.renew_hang -= 1;
            }
            (faults.renew_delay, hang)
        };
        if hang {
            std::future::pending::<()>().await;
        }
        if let Some(delay) = delay {
            tokio::time::sleep(delay).await;
        }
        let mut state = self.state();
        if token.get() != state.token {
            return Err(HostStoreError::FencedOut {
                current: FenceToken::new(state.token),
            });
        }
        state.lease_until = Some(Instant::now() + self.timing.lease());
        Ok(())
    }

    fn fence_timing(&self) -> FenceTiming {
        self.timing
    }

    async fn load_head(&self, scope: &ScopeRef) -> Result<Option<VersionedHead>, HostStoreError> {
        self.count_read();
        self.record(HostCall::LoadHead {
            kind: scope.kind,
            name: scope.name.as_str().to_string(),
        });
        {
            let mut faults = self.faults();
            if faults.load_head_unavailable > 0 {
                faults.load_head_unavailable -= 1;
                return Err(HostStoreError::Unavailable("injected".into()));
            }
        }
        Ok(self
            .state()
            .scopes
            .get(&(scope.kind, scope.name.as_str().to_string()))
            .map(|stored| VersionedHead {
                version: stored.version,
                commit_id: stored.commit_id.clone(),
                head: stored.head.clone(),
            }))
    }

    async fn list_heads(
        &self,
        kind: ScopeKind,
        after: Option<&ScopeName>,
        limit: usize,
    ) -> Result<HeadPage, HostStoreError> {
        self.count_read();
        self.record(HostCall::ListHeads { kind });
        let (delay, cap) = {
            let mut faults = self.faults();
            (faults.list_delay.take(), faults.list_page_cap)
        };
        if let Some(delay) = delay {
            tokio::time::sleep(delay).await;
        }
        let limit = cap.map_or(limit, |cap| limit.min(cap));
        let state = self.state();
        let mut heads = Vec::new();
        let mut more = false;
        for ((scope_kind, name), stored) in &state.scopes {
            if *scope_kind != kind || after.is_some_and(|after| name.as_str() <= after.as_str()) {
                continue;
            }
            if heads.len() == limit {
                more = true;
                break;
            }
            heads.push(ScopeHead {
                name: ScopeName::try_new(name.clone())?,
                head: VersionedHead {
                    version: stored.version,
                    commit_id: stored.commit_id.clone(),
                    head: stored.head.clone(),
                },
            });
        }
        let next = if more {
            heads.last().map(|head| head.name.clone())
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
        self.count_read();
        self.record(HostCall::ReadRows {
            kind: scope.kind,
            name: scope.name.as_str().to_string(),
        });
        if scope.kind == ScopeKind::Session {
            let (delay, failing) = {
                let faults = self.faults();
                let failing = faults
                    .failing_session_reads
                    .as_ref()
                    .filter(|(prefix, _)| scope.name.as_str().starts_with(prefix.as_str()))
                    .map(|(_, delay)| *delay);
                (faults.session_read_delay, failing)
            };
            if let Some(delay) = failing {
                tokio::time::sleep(delay).await;
                return Err(HostStoreError::Unavailable("injected".into()));
            }
            if let Some(delay) = delay {
                let _waiting =
                    WaitingRead::enter(&self.session_reads_waiting, &self.session_reads_peak);
                tokio::time::sleep(delay).await;
            }
        }
        let state = self.state();
        let Some(stored) = state
            .scopes
            .get(&(scope.kind, scope.name.as_str().to_string()))
        else {
            return Ok(Vec::new());
        };
        if from.get() < stored.start {
            return Err(HostStoreError::Invalid(format!(
                "{scope}: row {from} was dropped"
            )));
        }
        stored
            .rows
            .range(from.get()..)
            .take(limit)
            .map(|(_, row)| Row::try_new(row.clone()))
            .collect()
    }

    async fn commit(&self, batch: &CommitBatch) -> Result<(), HostStoreError> {
        self.record(HostCall::Commit);
        self.commits_started
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        let gate = self
            .commit_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Some(gate) = gate {
            gate.notified().await;
        }
        let indeterminate = {
            let mut faults = self.faults();
            if faults.unavailable > 0 {
                faults.unavailable -= 1;
                return Err(HostStoreError::Unavailable("injected".into()));
            }
            if faults.conflict > 0 {
                faults.conflict -= 1;
                let scope = batch
                    .updates()
                    .first()
                    .map(|update| update.scope.clone())
                    .ok_or_else(|| HostStoreError::Invalid("empty".into()))?;
                return Err(HostStoreError::Conflict {
                    scope,
                    current: None,
                });
            }
            if faults.indeterminate_after_apply > 0 {
                faults.indeterminate_after_apply -= 1;
                true
            } else {
                false
            }
        };
        let mut state = self.state();
        if state.landed.contains(batch.commit_id().as_str()) {
            let mut faults = self.faults();
            if faults.landed_retry_unavailable > 0 {
                faults.landed_retry_unavailable -= 1;
                return Err(HostStoreError::Unavailable("injected".into()));
            }
            return Ok(());
        }
        if batch.fence().get() < state.token {
            return Err(HostStoreError::FencedOut {
                current: FenceToken::new(state.token),
            });
        }
        // Check every update first: all or nothing.
        for update in batch.updates() {
            let key = (update.scope.kind, update.scope.name.as_str().to_string());
            let current = state.scopes.get(&key).map(|stored| stored.version);
            if current != update.previous {
                return Err(HostStoreError::Conflict {
                    scope: update.scope.clone(),
                    current,
                });
            }
            if update.previous.is_none() && state.deleted.contains(&key) {
                return Err(HostStoreError::Invalid(format!(
                    "{} was deleted and is created again",
                    update.scope
                )));
            }
        }
        for update in batch.updates() {
            let key = (update.scope.kind, update.scope.name.as_str().to_string());
            if update.delete {
                state.scopes.remove(&key);
                state.deleted.insert(key);
                continue;
            }
            let stored = state.scopes.entry(key).or_insert_with(|| StoredScope {
                version: update.next,
                commit_id: batch.commit_id().clone(),
                head: Vec::new(),
                start: 0,
                rows: BTreeMap::new(),
                end: 0,
            });
            if let Some(truncate) = update.truncate_after {
                stored.rows.retain(|index, _| *index < truncate.get());
                stored.end = stored.end.min(truncate.get());
            }
            if let Some(drop_before) = update.drop_before {
                stored.rows.retain(|index, _| *index >= drop_before.get());
                stored.start = stored.start.max(drop_before.get());
            }
            for row in &update.append {
                stored.rows.insert(stored.end, row.as_bytes().to_vec());
                stored.end += 1;
            }
            if let Some(head) = &update.head {
                stored.head = head.clone();
            }
            stored.version = update.next;
            stored.commit_id = batch.commit_id().clone();
        }
        state.landed.insert(batch.commit_id().as_str().to_string());
        state.commits += 1;
        if indeterminate {
            return Err(HostStoreError::Indeterminate("injected after apply".into()));
        }
        Ok(())
    }
}
