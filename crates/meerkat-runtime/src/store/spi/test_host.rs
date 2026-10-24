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
    /// Acquires that answer `Unavailable`.
    pub(crate) acquire_unavailable: u32,
    /// Head loads that answer `Unavailable`.
    pub(crate) load_head_unavailable: u32,
    /// Retries of a batch that already landed that answer `Unavailable`
    /// instead of `Ok`.
    pub(crate) landed_retry_unavailable: u32,
    /// How long every renew takes before it answers.
    pub(crate) renew_delay: Option<Duration>,
    /// How long the next listing call takes before it answers (once).
    pub(crate) list_delay: Option<Duration>,
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
}

#[async_trait]
impl HostStore for TestHost {
    async fn acquire_fence(&self, mode: AcquireMode) -> Result<FenceGrant, HostStoreError> {
        {
            let mut faults = self.faults();
            if faults.acquire_unavailable > 0 {
                faults.acquire_unavailable -= 1;
                return Err(HostStoreError::Unavailable("injected".into()));
            }
        }
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

    async fn renew_fence(&self, token: FenceToken) -> Result<(), HostStoreError> {
        let delay = {
            let mut faults = self.faults();
            if faults.renew_unavailable > 0 {
                faults.renew_unavailable -= 1;
                return Err(HostStoreError::Unavailable("injected".into()));
            }
            faults.renew_delay
        };
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
        let delay = self.faults().list_delay.take();
        if let Some(delay) = delay {
            tokio::time::sleep(delay).await;
        }
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
