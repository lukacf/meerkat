//! A host store writer whose self-fence window closes dispatches nothing more:
//! a real native agent runs over the store checkpointer of a host store realm,
//! and the window closes while one of its round commits lands. A writer that
//! one timed-out renew stopped waits for its re-acquire instead: the turn goes
//! on once the re-acquire succeeds. When no re-acquire succeeds within the
//! wait's bound and no other writer is found (a store fence gap), a runtime
//! run is suspended: its input stays pending, its waiter is told so, and the
//! run resumes once the fence is back. A round commit that meets the gap waits
//! the same way and suspends the run the same way; a closing save waits the
//! same way, and its refusal stands when the gap outlasts the wait. A cancel
//! accepted before the gap, an interrupt or a stop of a run that waits at the
//! fence, and an interrupt of the suspended run, end the run once the fence
//! is back: it never resumes, and its waiter is told within one wait that its
//! input stays queued. Another writer, met at the re-acquire, at a dispatch
//! whose window still reads open, at a commit or while a suspended run waits,
//! still refuses and tears the live session down, and the waiters are told
//! the store keeps their inputs for the new owner. A direct turn's run is
//! stopped and suspended at the gap: the turn fails with a retryable store
//! error after its one wait, another direct turn is refused busy while the
//! run stays marked, and the runtime settles the run once the fence is back.
//! Unrun until the candidate's validation.

use super::*;
use futures::stream;
use meerkat_client::{LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
use meerkat_core::host_store::{
    AcquireMode, CommitBatch, CommitId, FenceGrant, FenceTiming, FenceToken, HeadPage, HostStore,
    HostStoreError, Row, RowIndex, ScopeHead, ScopeKind, ScopeName, ScopeRef, ScopeVersion,
    VersionedHead,
};
use meerkat_core::service::{DeferredPromptPolicy, InitialTurnPolicy};
use meerkat_core::{
    AgentError, AgentToolDispatcher, Provider, ToolCallView, ToolDef, ToolDispatchOutcome,
    ToolError, ToolResult,
};
use meerkat_runtime::MeerkatMachine;
use meerkat_runtime::completion::{CompletionOutcome, CompletionWaitError};
use meerkat_runtime::identifiers::LogicalRuntimeId;
use meerkat_runtime::input_state::{InputAbandonReason, InputLifecycleState, InputTerminalOutcome};
use meerkat_runtime::store::spi::{SpiRealm, SpiRealmConfig, SpiRuntimeStore};
use meerkat_runtime::store::{InputStateRow, RuntimeStore};
use meerkat_runtime::{Input, PromptInput, RunStopReceipt};
use meerkat_session::PersistentSessionService;
use meerkat_store::{MemoryBlobStore, MemoryStore};
use serde_json::json;
use std::collections::{BTreeMap, HashSet};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex as StdMutex, OnceLock, Weak};
use std::time::Duration;
use tempfile::TempDir;

/// A turn's deadline. The rows run on real time: the machine works on its own
/// runtimes, which a paused test clock does not cover. Four store fence gaps
/// are about twice what the slowest row needs (a resumed run's gap and its
/// resume), and short of a hung-test kill, so a row that hangs fails here.
const TURN_DEADLINE: Duration = Duration::from_secs(FENCE_GAP.as_secs() * 4);

/// How long a row waits for the writer to stop.
const STOP_DEADLINE: Duration = Duration::from_secs(30);

// The rows' timings are scaled together to the floors the host store
// contract allows, so the module's real-time run stays short, and keep the
// relations each row relies on: a listing within the 2 s window
// finishes inside it; the dispatch wait bound outlasts the window; a slow
// listing outlasts the bound; a short gap ends within one wait, after the
// first write that meets it; a fence gap outlasts three waits; a cancel
// lands inside a run's wait; and the answer slack is shorter than a wait, so
// an answer after a second wait fails the one-wait check. The window cannot
// shrink: a renew interval is at least 1 s and the window at least two.

/// The host's renew interval on its 3 s lease leaves a self-fence window of
/// 2 s, the contract's floor; the realm holds a renew to the interval (1 s).
const RENEW_INTERVAL: Duration = Duration::from_secs(1);

/// The rows' realm renew timeout: its renews are held to the 1 s interval
/// anyway, and its two acquires, which this host answers at once, are held
/// to it too instead of the default 5 s.
const RENEW_TIMEOUT: Duration = Duration::from_secs(1);

/// A re-acquire listing that answers within the 2 s window.
const LISTING_WITHIN_THE_WINDOW: Duration = Duration::from_millis(1500);

/// A re-acquire listing that answers long after the 2 s window and the
/// 5 s dispatch wait bound.
const LISTING_PAST_THE_WINDOW: Duration = Duration::from_secs(12);

/// How long a dispatch waits for a stopped writer's re-acquire on this host:
/// its 2 s self-fence window plus the re-acquire's own timeouts, two acquires
/// and a renew under the rows' 1 s renew timeout. Each row's realm reports
/// it (`SpiRealm::dispatch_wait_bound`), checked when the row opens.
const DISPATCH_WAIT_BOUND: Duration = Duration::from_secs(5);

/// A store fence gap longer than three dispatch wait bounds: acquires fail
/// for this long from the stop on.
const FENCE_GAP: Duration = Duration::from_secs(18);

/// How long a suspended run's waiter waits for its answer: the stop, one
/// dispatch wait bound and the run's retry, with room to spare.
const ANSWER_DEADLINE: Duration = Duration::from_secs(30);

/// What a waiter's answer may take beyond one dispatch wait bound from the
/// stop: the agent's retry backoff before it reaches the fence. Shorter than
/// a bound, so an answer that came after a second wait fails the check.
const ANSWER_SLACK: Duration = Duration::from_secs(3);

/// How far into the gap a row's cancel comes: the run's retry then waits at
/// the fence of the stopped writer (it reaches the fence half a second after
/// the stop and waits up to the 5 s bound).
const CANCEL_INTO_THE_WAIT: Duration = Duration::from_secs(2);

/// A store fence gap shorter than one dispatch wait bound: acquires fail for
/// this long from the stop on, so the first write after the stop (at most
/// two renew intervals in, once the stop is seen) is refused and a later
/// re-acquire within the wait (they are tried once per renew interval)
/// restores the writer.
const SHORT_GAP: Duration = Duration::from_secs(3);

/// The scope another writer's batch creates in the foreign-batch rows, in a
/// family Core never uses (Core leaves `x` families to the layers above it),
/// so the new owner's runtime store reads past it. Its name encodes the key
/// `foreign` and an incarnation, as every journaled scope's name does.
const FOREIGN_SCOPE: &str = "x:rforeign~another-writer";

/// [`FOREIGN_SCOPE`]'s head: the smallest valid V1 head of an empty scope,
/// which a realm's listing decodes and checks against the scope's name.
const FOREIGN_HEAD: &[u8] = br#"{"f":1,"fam":"x","key":"foreign","w":{"s":0,"e":0},"lb":0,"sb":0}"#;

/// Which step of the turn the writer stops during.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClosesDuring {
    /// The round's first commit: the window is closed before any call of
    /// the round is dispatched.
    FirstCommit,
    /// The round's second commit: the window is closed before the next
    /// model call.
    SecondCommit,
    /// The first of the round's two calls, which run one at a time: the
    /// window is closed before the second call starts.
    FirstCall,
    /// The run's first model call, which fails retryably once the writer
    /// stopped: the retry waits at the fence before the run's first round
    /// commit.
    FirstModelCall,
    /// The run's first model call, which stops the writer and answers with
    /// the round's call: the round's first commit meets the stopped writer.
    RoundModelCall,
    /// The round's one call: the writer stops, or another writer takes the
    /// fence over, while it runs, so the round's next commit meets it first.
    OnlyCall,
    /// The run's one model call, which stops the writer and answers with
    /// text: the run's closing save is its first write after the stop.
    TextModelCall,
    /// Never: the writer is not stopped.
    Never,
}

/// How the row stops the writer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Stop {
    /// Every renew from then on fails, the renew of each re-acquire
    /// included.
    #[default]
    FailingRenews,
    /// One renew never answers, and the realm's renew timeout gives up on
    /// it; later renews answer. What the re-acquire after it meets:
    TimedOutRenew(Reacquire),
    /// Another writer acquires the fence: this writer's next renew is fenced
    /// out, which poisons the realm and leaves its own window open, so a
    /// commit is the first to meet it.
    TakenOver,
}

/// What the re-acquire after a timed-out renew meets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reacquire {
    /// Its listing answers after this long; its acquire and its renew
    /// answer at once.
    ListsIn(Duration),
    /// Every acquire answers `Unavailable`.
    Fails,
    /// Every acquire answers `Unavailable` for this long from the stop on,
    /// and succeeds after.
    FailsFor(Duration),
    /// Another writer committed while this one was stopped: the listing
    /// shows that writer's batch.
    FindsForeignBatch,
    /// Every acquire answers `Unavailable` for this long from the stop on;
    /// the first that succeeds after lists a batch another writer committed
    /// meanwhile.
    FailsForThenForeign(Duration),
}

/// Shared between the host, the model and the tools of one row.
#[derive(Default)]
struct FenceControl {
    /// How the row stops the writer.
    stop: Stop,
    /// Every renew from now on fails.
    fail_renews: AtomicBool,
    /// A renew failed since `fail_renews` was set.
    renew_failed: AtomicBool,
    /// The next host commit waits until the writer stopped, then lands.
    hold_next_commit: AtomicBool,
    /// The next renew never answers.
    hang_next_renew: AtomicBool,
    /// Every acquire from now on answers `Unavailable`.
    fail_acquires: AtomicBool,
    /// Every acquire answers `Unavailable` until then.
    acquires_fail_until: StdMutex<Option<tokio::time::Instant>>,
    /// The next listing call answers after this long.
    list_delay: StdMutex<Option<Duration>>,
    /// The next listing finds a batch another writer committed.
    foreign_batch: AtomicBool,
    /// The next renew finds that another writer acquired the fence.
    take_over_at_next_renew: AtomicBool,
    /// When the writer last stopped, as a row's stop leaves it.
    stopped_at: StdMutex<Option<tokio::time::Instant>>,
    /// The realm, once it is open: a row waits on its own window.
    realm: OnceLock<Weak<SpiRealm>>,
}

impl FenceControl {
    /// Close the writer's window while the next host commit is in flight:
    /// the commit has passed the write path's own fence check when the host
    /// sees it, and the writer stops only from then on, so the commit lands
    /// and the window is closed by the time it answers.
    fn close_during_next_commit(&self) {
        self.hold_next_commit.store(true, Ordering::SeqCst);
    }

    /// Stop the writer with one renew that never answers, set up what the
    /// re-acquire after it meets, and wait until the realm's own window
    /// reads closed. A row that does not stop the writer this way returns at
    /// once.
    async fn stop_with_a_timed_out_renew(&self) {
        let Stop::TimedOutRenew(reacquire) = self.stop else {
            return;
        };
        match reacquire {
            Reacquire::ListsIn(delay) => {
                *self
                    .list_delay
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(delay);
            }
            Reacquire::Fails => self.fail_acquires.store(true, Ordering::SeqCst),
            Reacquire::FailsFor(gap) => {
                *self
                    .acquires_fail_until
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) =
                    Some(tokio::time::Instant::now() + gap);
            }
            Reacquire::FindsForeignBatch => self.foreign_batch.store(true, Ordering::SeqCst),
            Reacquire::FailsForThenForeign(gap) => {
                *self
                    .acquires_fail_until
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) =
                    Some(tokio::time::Instant::now() + gap);
                self.foreign_batch.store(true, Ordering::SeqCst);
            }
        }
        self.hang_next_renew.store(true, Ordering::SeqCst);
        self.wait_until_stopped().await;
    }

    /// Wait, at most [`STOP_DEADLINE`], until the realm's own window reads
    /// closed, and record when it did.
    async fn wait_until_stopped(&self) {
        let _ = tokio::time::timeout(STOP_DEADLINE, async {
            while !self.writer_stopped() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        if self.writer_stopped() {
            let mut stopped_at = self
                .stopped_at
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if stopped_at.is_none() {
                *stopped_at = Some(tokio::time::Instant::now());
            }
        }
    }

    /// Stop the writer the way the row's stop names, from inside a step of
    /// the turn.
    async fn stop_the_writer(&self) {
        match self.stop {
            Stop::FailingRenews => {
                self.fail_renews.store(true, Ordering::SeqCst);
                let _ = tokio::time::timeout(STOP_DEADLINE, async {
                    while !self.renew_failed.load(Ordering::SeqCst) {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                })
                .await;
            }
            Stop::TimedOutRenew(_) => self.stop_with_a_timed_out_renew().await,
            Stop::TakenOver => self.take_over().await,
        }
    }

    /// Another writer acquires the fence: wait, at most [`STOP_DEADLINE`],
    /// until this writer's next renew is fenced out and its realm poisoned.
    async fn take_over(&self) {
        self.take_over_at_next_renew.store(true, Ordering::SeqCst);
        let _ = tokio::time::timeout(STOP_DEADLINE, async {
            while !self.realm_lost() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
    }

    /// When the writer stopped, once it has.
    fn stopped_at(&self) -> Option<tokio::time::Instant> {
        *self
            .stopped_at
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Whether the realm is poisoned: another writer holds its fence.
    fn realm_lost(&self) -> bool {
        self.realm
            .get()
            .and_then(|realm| realm.upgrade())
            .is_some_and(|realm| realm.realm_poisoned().is_some())
    }

    /// Whether acquires answer `Unavailable` now.
    fn acquires_fail(&self) -> bool {
        self.fail_acquires.load(Ordering::SeqCst)
            || self
                .acquires_fail_until
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_some_and(|until| tokio::time::Instant::now() < until)
    }

    /// Whether the realm's own window reads closed.
    fn writer_stopped(&self) -> bool {
        self.realm
            .get()
            .and_then(|realm| realm.upgrade())
            .is_some_and(|realm| !realm.lease_is_live())
    }
}

#[derive(Debug, Clone)]
struct StoredScope {
    version: ScopeVersion,
    commit_id: CommitId,
    head: Vec<u8>,
    start: u64,
    rows: BTreeMap<u64, Vec<u8>>,
    end: u64,
}

#[derive(Default)]
struct HostState {
    token: u64,
    scopes: BTreeMap<(ScopeKind, String), StoredScope>,
    landed: HashSet<String>,
}

/// An in-memory host store that meets its obligations at commit, with a
/// 1 s renew interval and a 3 s lease, whose renews, acquires, listings and
/// commits follow `control`.
struct FenceHost {
    state: StdMutex<HostState>,
    control: Arc<FenceControl>,
}

impl FenceHost {
    fn new(control: Arc<FenceControl>) -> Self {
        Self {
            state: StdMutex::new(HostState::default()),
            control,
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, HostState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[async_trait]
impl HostStore for FenceHost {
    async fn acquire_fence(&self, _mode: AcquireMode) -> Result<FenceGrant, HostStoreError> {
        if self.control.acquires_fail() {
            return Err(HostStoreError::Unavailable(
                "the row stops the acquires".to_string(),
            ));
        }
        let mut state = self.state();
        state.token += 1;
        Ok(FenceGrant {
            token: FenceToken::new(state.token),
            superseded_lease: Duration::ZERO,
        })
    }

    async fn renew_fence(&self, token: FenceToken) -> Result<(), HostStoreError> {
        if self.control.hang_next_renew.swap(false, Ordering::SeqCst) {
            // The realm's renew timeout gives up on this renew.
            std::future::pending::<()>().await;
        }
        if self.control.fail_renews.load(Ordering::SeqCst) {
            self.control.renew_failed.store(true, Ordering::SeqCst);
            return Err(HostStoreError::Unavailable(
                "the row stops the renews".to_string(),
            ));
        }
        let mut state = self.state();
        if self
            .control
            .take_over_at_next_renew
            .swap(false, Ordering::SeqCst)
        {
            // Another writer acquired the fence since this writer's last
            // renew.
            state.token += 1;
        }
        if token.get() != state.token {
            return Err(HostStoreError::FencedOut {
                current: FenceToken::new(state.token),
            });
        }
        Ok(())
    }

    fn fence_timing(&self) -> FenceTiming {
        FenceTiming::try_new(RENEW_INTERVAL, Duration::from_secs(3))
            .expect("a 1 s renew interval on a 3 s lease")
    }

    async fn load_head(&self, scope: &ScopeRef) -> Result<Option<VersionedHead>, HostStoreError> {
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
        let delay = self
            .control
            .list_delay
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(delay) = delay {
            tokio::time::sleep(delay).await;
        }
        if self.control.foreign_batch.swap(false, Ordering::SeqCst) {
            // Another writer's batch landed while this writer was stopped.
            self.state().scopes.insert(
                (ScopeKind::Session, FOREIGN_SCOPE.to_string()),
                StoredScope {
                    version: ScopeVersion::FIRST,
                    commit_id: CommitId::try_new("another-writers-batch")?,
                    head: FOREIGN_HEAD.to_vec(),
                    start: 0,
                    rows: BTreeMap::new(),
                    end: 0,
                },
            );
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
        stored
            .rows
            .range(from.get().max(stored.start)..)
            .take(limit)
            .map(|(_, row)| Row::try_new(row.clone()))
            .collect()
    }

    async fn commit(&self, batch: &CommitBatch) -> Result<(), HostStoreError> {
        if self.control.hold_next_commit.swap(false, Ordering::SeqCst) {
            // The writer passed its own fence check before this commit; it
            // stops while the commit is in flight.
            match self.control.stop {
                Stop::FailingRenews => {
                    self.control.fail_renews.store(true, Ordering::SeqCst);
                    let _ = tokio::time::timeout(Duration::from_secs(5), async {
                        while !self.control.renew_failed.load(Ordering::SeqCst) {
                            tokio::time::sleep(Duration::from_millis(10)).await;
                        }
                    })
                    .await;
                    // The commit lands once the window reads closed, and the
                    // stop is recorded then, as a timed-out renew's is.
                    self.control.wait_until_stopped().await;
                }
                Stop::TimedOutRenew(_) => self.control.stop_with_a_timed_out_renew().await,
                Stop::TakenOver => self.control.take_over().await,
            }
        }
        let mut state = self.state();
        if state.landed.contains(batch.commit_id().as_str()) {
            return Ok(());
        }
        if batch.fence().get() < state.token {
            return Err(HostStoreError::FencedOut {
                current: FenceToken::new(state.token),
            });
        }
        for update in batch.updates() {
            let key = (update.scope.kind, update.scope.name.as_str().to_string());
            let current = state.scopes.get(&key).map(|stored| stored.version);
            if current != update.previous {
                return Err(HostStoreError::Conflict {
                    scope: update.scope.clone(),
                    current,
                });
            }
        }
        for update in batch.updates() {
            let key = (update.scope.kind, update.scope.name.as_str().to_string());
            if update.delete {
                state.scopes.remove(&key);
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
        Ok(())
    }
}

/// One answer of the model, by request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModelStep {
    /// Ask for the row's calls of `probe`: one, or two when the window
    /// closes during the first call.
    Tools,
    /// Stop the writer, the way the row's stop names, then fail retryably
    /// (503): the agent's retry asks the fence of the stopped writer (or of
    /// the realm another writer took over).
    StopThenOverloaded,
    /// Stop the writer, the way the row's stop names, then answer with one
    /// call of `probe`: the round's first commit meets the stopped writer.
    StopThenTools,
    /// Stop the writer, the way the row's stop names, then answer with text:
    /// the run's closing save meets the stopped writer.
    StopThenText,
    /// Never answer: the request waits until its run is dropped.
    Hold,
    /// Answer with text.
    Text,
}

/// Answers each request with the next step of its script, and with text past
/// it. Counts its requests.
struct FenceModel {
    requests: AtomicUsize,
    control: Arc<FenceControl>,
    closes_during: ClosesDuring,
    script: Vec<ModelStep>,
}

#[async_trait]
impl LlmClient for FenceModel {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        let index = self.requests.fetch_add(1, Ordering::SeqCst);
        let step = self.script.get(index).copied().unwrap_or(ModelStep::Text);
        let usage = meerkat_core::TurnUsage::host_declared(
            Provider::Anthropic,
            &request.model,
            Default::default(),
        );
        if step == ModelStep::StopThenOverloaded {
            let control = Arc::clone(&self.control);
            return Box::pin(stream::once(async move {
                control.stop_the_writer().await;
                Err(LlmError::ServerOverloaded)
            }));
        }
        if step == ModelStep::Hold {
            return Box::pin(stream::once(std::future::pending::<
                Result<LlmEvent, LlmError>,
            >()));
        }
        if step == ModelStep::StopThenText {
            let control = Arc::clone(&self.control);
            let events: Vec<Result<LlmEvent, LlmError>> = vec![
                Ok(LlmEvent::TextDelta {
                    delta: "probed".into(),
                    meta: None,
                }),
                Ok(LlmEvent::UsageUpdate { usage }),
                Ok(LlmEvent::Done {
                    outcome: LlmDoneOutcome::success(meerkat_core::StopReason::EndTurn),
                }),
            ];
            return Box::pin(futures::StreamExt::flat_map(
                stream::once(async move {
                    control.stop_the_writer().await;
                    events
                }),
                stream::iter,
            ));
        }
        if step == ModelStep::StopThenTools {
            let control = Arc::clone(&self.control);
            let events: Vec<Result<LlmEvent, LlmError>> = vec![
                Ok(LlmEvent::ToolCallComplete {
                    id: "probe-call".into(),
                    name: "probe".into(),
                    args: json!({}),
                    meta: None,
                }),
                Ok(LlmEvent::UsageUpdate { usage }),
                Ok(LlmEvent::Done {
                    outcome: LlmDoneOutcome::success(meerkat_core::StopReason::ToolUse),
                }),
            ];
            return Box::pin(futures::StreamExt::flat_map(
                stream::once(async move {
                    control.stop_with_a_timed_out_renew().await;
                    events
                }),
                stream::iter,
            ));
        }
        if step == ModelStep::Tools {
            if self.closes_during == ClosesDuring::FirstCommit {
                self.control.close_during_next_commit();
            }
            let mut events: Vec<Result<LlmEvent, LlmError>> =
                vec![Ok(LlmEvent::ToolCallComplete {
                    id: "probe-call".into(),
                    name: "probe".into(),
                    args: json!({}),
                    meta: None,
                })];
            if self.closes_during == ClosesDuring::FirstCall {
                events.push(Ok(LlmEvent::ToolCallComplete {
                    id: "probe-call-2".into(),
                    name: "probe".into(),
                    args: json!({}),
                    meta: None,
                }));
            }
            events.push(Ok(LlmEvent::UsageUpdate { usage }));
            events.push(Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::success(meerkat_core::StopReason::ToolUse),
            }));
            return Box::pin(stream::iter(events));
        }
        Box::pin(stream::iter(vec![
            Ok(LlmEvent::TextDelta {
                delta: "probed".into(),
                meta: None,
            }),
            Ok(LlmEvent::UsageUpdate { usage }),
            Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::success(meerkat_core::StopReason::EndTurn),
            }),
        ]))
    }

    fn provider(&self) -> Provider {
        Provider::Anthropic
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

/// One tool, `probe`. Counts its dispatches.
struct FenceTools {
    dispatched: AtomicUsize,
    control: Arc<FenceControl>,
    closes_during: ClosesDuring,
}

#[async_trait]
impl AgentToolDispatcher for FenceTools {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        vec![Arc::new(ToolDef::new(
            "probe",
            "a probe",
            json!({"type":"object"}),
        ))]
        .into()
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        let first = self.dispatched.fetch_add(1, Ordering::SeqCst) == 0;
        match self.closes_during {
            ClosesDuring::SecondCommit => self.control.close_during_next_commit(),
            ClosesDuring::FirstCall if first => self.control.stop_with_a_timed_out_renew().await,
            ClosesDuring::OnlyCall if first => self.control.stop_the_writer().await,
            ClosesDuring::FirstCommit
            | ClosesDuring::FirstCall
            | ClosesDuring::FirstModelCall
            | ClosesDuring::RoundModelCall
            | ClosesDuring::OnlyCall
            | ClosesDuring::TextModelCall
            | ClosesDuring::Never => {}
        }
        Ok(ToolDispatchOutcome::sync_result(ToolResult::new(
            call.id.into(),
            "probed".into(),
            false,
        )))
    }
}

/// A real native agent through the factory, with the row's model, tools
/// and configuration.
struct FenceBuilder {
    factory: AgentFactory,
    client: Arc<FenceModel>,
    tools: Arc<FenceTools>,
    config: Config,
}

#[async_trait]
impl SessionAgentBuilder for FenceBuilder {
    type Agent = FactoryAgent;

    fn session_comms_identity_root(&self) -> meerkat_core::SessionCommsIdentityRoot {
        self.factory.session_comms_identity_root()
    }

    async fn build_agent(
        &self,
        request: &CreateSessionRequest,
        event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<FactoryAgent, SessionError> {
        let mut config = AgentBuildConfig::from_create_session_request(request, event_tx);
        config.llm_client_override = Some(self.client.clone());
        config.tool_dispatcher_override = Some(self.tools.clone());
        let session_context = match request
            .build
            .as_ref()
            .map(|build| &build.runtime_build_mode)
        {
            Some(meerkat_core::RuntimeBuildMode::SessionOwned(bindings)) => {
                Some(Arc::clone(bindings.session_context()))
            }
            _ => None,
        };
        let factory = self.factory.clone();
        let agent_config = self.config.clone();
        let agent = meerkat_runtime::stack_relief::relieve_caller_stack(move || async move {
            factory.build_agent(config, &agent_config).await
        })
        .await
        .map_err(|error| SessionError::Agent(AgentError::InternalError(error.to_string())))?;
        Ok(FactoryAgent {
            agent,
            session_context,
            pending_head_canonical_boundary: None,
            acknowledged_head_canonical_boundary: None,
        })
    }
}

/// How a row's turn reaches the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Turn {
    /// The session's initial turn, which the surface runs as a direct
    /// service turn: no runtime input row backs it.
    Direct,
    /// A prompt queued on the session's runtime, which the runtime loop
    /// stages and dispatches as a run of one input row.
    Queued,
}

/// One session over a host store realm whose writer stops, the way `stop`
/// names, during the step `closes_during` names: the session's executor is
/// attached and its turn not yet run.
struct FenceRow {
    host: Arc<dyn HostStore>,
    control: Arc<FenceControl>,
    realm: Arc<SpiRealm>,
    runtime_store: Arc<SpiRuntimeStore>,
    client: Arc<FenceModel>,
    tools: Arc<FenceTools>,
    machine: Arc<MeerkatMachine>,
    session_id: meerkat_core::SessionId,
    service: Arc<PersistentSessionService<FenceBuilder>>,
    /// A direct row's initial turn, until it runs.
    initial_turn: Option<crate::surface::RuntimeBackedInitialTurn>,
    _temp: TempDir,
}

impl FenceRow {
    async fn open(closes_during: ClosesDuring, stop: Stop, turn: Turn) -> Self {
        let script = match closes_during {
            ClosesDuring::FirstModelCall => vec![ModelStep::StopThenOverloaded],
            ClosesDuring::RoundModelCall => vec![ModelStep::StopThenTools, ModelStep::Tools],
            ClosesDuring::TextModelCall => vec![ModelStep::StopThenText],
            ClosesDuring::FirstCommit
            | ClosesDuring::SecondCommit
            | ClosesDuring::FirstCall
            | ClosesDuring::OnlyCall
            | ClosesDuring::Never => vec![ModelStep::Tools],
        };
        Self::open_scripted(closes_during, stop, turn, script).await
    }

    async fn open_scripted(
        closes_during: ClosesDuring,
        stop: Stop,
        turn: Turn,
        script: Vec<ModelStep>,
    ) -> Self {
        let temp = TempDir::new().expect("temp");
        let control = Arc::new(FenceControl {
            stop,
            ..FenceControl::default()
        });
        let host: Arc<dyn HostStore> = Arc::new(FenceHost::new(Arc::clone(&control)));
        let realm = SpiRealm::open(
            Arc::clone(&host),
            SpiRealmConfig {
                renew_timeout: RENEW_TIMEOUT,
                ..SpiRealmConfig::default()
            },
        )
        .await
        .expect("the realm opens");
        assert_eq!(
            realm.dispatch_wait_bound(),
            DISPATCH_WAIT_BOUND,
            "the rows' dispatch wait bound is the realm's"
        );
        let _ = control.realm.set(Arc::downgrade(&realm));
        let runtime_store = Arc::new(
            SpiRuntimeStore::open(Arc::clone(&realm))
                .await
                .expect("the runtime store opens"),
        );
        let store = Arc::new(MemoryStore::new());
        let blob_store = Arc::new(MemoryBlobStore::new());
        let machine = Arc::new(
            MeerkatMachine::persistent(runtime_store.clone(), blob_store.clone())
                .expect("canonical machine"),
        );
        let client = Arc::new(FenceModel {
            requests: AtomicUsize::new(0),
            control: Arc::clone(&control),
            closes_during,
            script,
        });
        let tools = Arc::new(FenceTools {
            dispatched: AtomicUsize::new(0),
            control: Arc::clone(&control),
            closes_during,
        });
        let mut config = Config::default();
        if closes_during == ClosesDuring::FirstCall {
            // One call at a time: the round's second call asks the fence
            // once the first one has run.
            config.tools.max_concurrent = 1;
        }
        let builder = FenceBuilder {
            factory: AgentFactory::new(temp.path().join("sessions")).session_store(store.clone()),
            client: client.clone(),
            tools: tools.clone(),
            config,
        };
        let service = Arc::new(
            PersistentSessionService::new(
                builder,
                1,
                store.clone(),
                runtime_store.clone(),
                blob_store,
            )
            .with_canonical_runtime_adapter(machine.clone()),
        );
        // A queued row's session has no first turn of its own.
        let request = CreateSessionRequest {
            model: "claude-sonnet-4-5".into(),
            prompt: match turn {
                Turn::Direct => "probe once",
                Turn::Queued => "unused",
            }
            .into(),
            injected_context: Vec::new(),
            system_prompt: meerkat_core::SystemPromptOverride::Inherit,
            max_tokens: None,
            event_tx: None,
            initial_turn: match turn {
                Turn::Direct => InitialTurnPolicy::RunImmediately,
                Turn::Queued => InitialTurnPolicy::Defer,
            },
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
            build: None,
            labels: None,
        };
        let (request, initial_turn) =
            crate::surface::split_runtime_backed_eager_create_request(request);
        let created = Box::pin(crate::surface::materialize_session(
            &service,
            &machine,
            meerkat_core::Session::new(),
            request,
            {
                let service = service.clone();
                let machine = machine.clone();
                move |id| crate::surface::default_persistent_executor(service, machine, id)
            },
        ))
        .await
        .expect("materialize the session and its executor");
        Self {
            host,
            control,
            realm,
            runtime_store,
            client,
            tools,
            machine,
            session_id: created.session_id,
            service,
            initial_turn,
            _temp: temp,
        }
    }

    /// Run the row's turn, waiting at most `deadline` for its answer: a
    /// direct row's initial turn, or a queued row's prompt, whose answer is
    /// its input's waiter's.
    async fn run_turn(&mut self, deadline: Duration) -> TurnAnswer {
        match self.initial_turn.take() {
            Some(initial_turn) => {
                let turn = tokio::time::timeout(
                    deadline,
                    crate::surface::run_runtime_backed_initial_turn_with_machine(
                        &self.service,
                        &self.machine,
                        &self.session_id,
                        initial_turn,
                    ),
                )
                .await
                .expect("the turn ends");
                TurnAnswer::Direct(turn.map(|_| ()).map_err(|error| format!("{error:?}")))
            }
            None => {
                let (_, completion) = self
                    .machine
                    .accept_input_with_completion(
                        &self.session_id,
                        Input::Prompt(PromptInput::new("probe once", None)),
                    )
                    .await
                    .expect("the runtime accepts the prompt");
                let completion = completion.expect("a completion waiter");
                TurnAnswer::Queued(
                    tokio::time::timeout(deadline, completion.wait())
                        .await
                        .expect("the input's waiter is answered"),
                )
            }
        }
    }

    /// Queue the row's prompt on the session's runtime, without waiting for
    /// its answer: its waiter.
    async fn accept_prompt(&self) -> meerkat_runtime::completion::CompletionHandle {
        let (_, completion) = self
            .machine
            .accept_input_with_completion(
                &self.session_id,
                Input::Prompt(PromptInput::new("probe once", None)),
            )
            .await
            .expect("the runtime accepts the prompt");
        completion.expect("a completion waiter")
    }

    /// Run one more direct turn of the session, as a surface runs a turn
    /// the runtime loop does not dispatch, waiting at most
    /// [`TURN_DEADLINE`] for its answer.
    async fn run_direct_turn(
        &self,
        prompt: &str,
    ) -> Result<RunResult, crate::surface::SurfaceRuntimeMaterializeError> {
        let (_, turn) =
            crate::surface::split_runtime_backed_eager_create_request(CreateSessionRequest {
                model: "claude-sonnet-4-5".into(),
                prompt: prompt.into(),
                injected_context: Vec::new(),
                system_prompt: meerkat_core::SystemPromptOverride::Inherit,
                max_tokens: None,
                event_tx: None,
                initial_turn: InitialTurnPolicy::RunImmediately,
                deferred_prompt_policy: DeferredPromptPolicy::Discard,
                build: None,
                labels: None,
            });
        let turn = turn.expect("a request that runs at once carries its turn");
        tokio::time::timeout(
            TURN_DEADLINE,
            crate::surface::run_runtime_backed_initial_turn_with_machine(
                &self.service,
                &self.machine,
                &self.session_id,
                turn,
            ),
        )
        .await
        .expect("the direct turn ends")
    }

    /// Wait, at most `deadline`, until the session's runtime marks no run
    /// resumable: it resumed or settled the run it marked. Whether it did.
    async fn wait_until_no_run_is_marked(&self, deadline: Duration) -> bool {
        let started = tokio::time::Instant::now();
        while self
            .machine
            .resumable_run_id(&self.session_id)
            .await
            .is_some()
        {
            if started.elapsed() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        true
    }

    /// Wait, at most `deadline`, until the session's runtime loop has
    /// ended: its attachment no longer has open command channels.
    async fn wait_for_the_runtime_loop_to_end(&self, deadline: Duration) -> bool {
        let started = tokio::time::Instant::now();
        while self
            .machine
            .session_has_live_executor_attachment(&self.session_id)
            .await
        {
            if started.elapsed() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        true
    }

    fn model_requests(&self) -> usize {
        self.client.requests.load(Ordering::SeqCst)
    }

    fn dispatched(&self) -> usize {
        self.tools.dispatched.load(Ordering::SeqCst)
    }

    /// The phase of each of the session's input rows (`None` for a row that
    /// does not decode), when the store answers.
    async fn input_phases(&self) -> Option<Vec<Option<InputLifecycleState>>> {
        input_phases_in(self.runtime_store.as_ref(), &self.session_id).await
    }

    /// Wait, at most `deadline`, until the session's input rows read
    /// `expected`; the last reading either way.
    async fn wait_for_input_phases(
        &self,
        expected: &[InputLifecycleState],
        deadline: Duration,
    ) -> Option<Vec<Option<InputLifecycleState>>> {
        let expected: Vec<Option<InputLifecycleState>> =
            expected.iter().copied().map(Some).collect();
        let started = tokio::time::Instant::now();
        loop {
            let phases = self.input_phases().await;
            if phases.as_deref() == Some(expected.as_slice()) || started.elapsed() >= deadline {
                return phases;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// The terminal outcome of each of the session's input rows (`None` for
    /// a row with none yet, or one that does not decode), when the store
    /// answers.
    async fn input_terminals(&self) -> Option<Vec<Option<InputTerminalOutcome>>> {
        self.runtime_store
            .load_input_states(&LogicalRuntimeId::for_session(&self.session_id))
            .await
            .ok()
            .map(|rows| {
                rows.iter()
                    .map(|row| match row {
                        InputStateRow::Decoded(stored) => stored.seed.terminal_outcome.clone(),
                        _ => None,
                    })
                    .collect()
            })
    }

    /// Check that a stopped run's one input ends once the fence is back,
    /// abandoned as cancelled (the run was settled, not resumed), and that
    /// the run never asked the model again: `requests` is what it asked
    /// before.
    async fn assert_settled_as_cancelled(&self, site: &str, requests: usize) {
        let phases = self
            .wait_for_input_phases(&[InputLifecycleState::Abandoned], TURN_DEADLINE)
            .await;
        assert_eq!(
            phases,
            Some(vec![Some(InputLifecycleState::Abandoned)]),
            "{site}: the stopped run's input ends abandoned once the fence is back"
        );
        assert_eq!(
            self.input_terminals().await,
            Some(vec![Some(InputTerminalOutcome::Abandoned {
                reason: InputAbandonReason::Cancelled,
            })]),
            "{site}: the stopped run's input ends as cancelled"
        );
        assert_eq!(
            self.model_requests(),
            requests,
            "{site}: the stopped run never asks the model again"
        );
    }

    /// Check that a waiter answered just now was answered within one
    /// dispatch wait bound (and the agent's retry backoff) of the writer's
    /// stop.
    fn assert_answered_within_one_wait(&self, site: &str) {
        let stopped_at = self
            .control
            .stopped_at()
            .expect("the writer stopped during the run");
        let answered_after = tokio::time::Instant::now().duration_since(stopped_at);
        assert!(
            answered_after <= DISPATCH_WAIT_BOUND + ANSWER_SLACK,
            "{site}: the waiter is answered within one dispatch wait of the stop: {answered_after:?}"
        );
    }

    /// What another process that takes the session over reads: the phase of
    /// each of the session's input rows, through a realm of its own over the
    /// same host.
    async fn new_owner_input_phases(&self) -> Option<Vec<Option<InputLifecycleState>>> {
        let realm = SpiRealm::open(Arc::clone(&self.host), SpiRealmConfig::default())
            .await
            .expect("the new owner's realm opens");
        let store = SpiRuntimeStore::open(Arc::clone(&realm))
            .await
            .expect("the new owner's runtime store opens");
        input_phases_in(&store, &self.session_id).await
    }

    /// The session's committed transcript, one label per message after the
    /// system prompt.
    async fn committed_rows(&self) -> Vec<String> {
        let session = self
            .service
            .load_authoritative_session(&self.session_id)
            .await
            .expect("read the committed session")
            .expect("the session is committed");
        session
            .messages()
            .iter()
            .filter_map(|message| match message {
                meerkat_core::Message::System(_) => None,
                meerkat_core::Message::User(user) => Some(format!("user {}", user.text_content())),
                meerkat_core::Message::BlockAssistant(assistant) => {
                    Some(
                        if assistant.blocks.iter().any(|block| {
                            matches!(block, meerkat_core::AssistantBlock::ToolUse { .. })
                        }) {
                            "assistant tool use".to_string()
                        } else {
                            format!("assistant {}", assistant.text_blocks().collect::<String>())
                        },
                    )
                }
                meerkat_core::Message::ToolResults { .. } => Some("tool results".to_string()),
                meerkat_core::Message::SystemNotice(notice) => {
                    Some(format!("notice {:?}", notice.kind))
                }
            })
            .collect()
    }
}

/// Whether every input row reads a phase that is not terminal: the input is
/// still pending, to run later.
fn all_pending(phases: &Option<Vec<Option<InputLifecycleState>>>) -> bool {
    phases.as_ref().is_some_and(|phases| {
        !phases.is_empty()
            && phases.iter().all(|phase| {
                matches!(
                    phase,
                    Some(
                        InputLifecycleState::Accepted
                            | InputLifecycleState::Queued
                            | InputLifecycleState::Staged
                            | InputLifecycleState::Applied
                            | InputLifecycleState::AppliedPendingConsumption
                    )
                )
            })
    })
}

/// The phase of each input row of `session_id`'s runtime in `store`.
async fn input_phases_in(
    store: &dyn RuntimeStore,
    session_id: &meerkat_core::SessionId,
) -> Option<Vec<Option<InputLifecycleState>>> {
    store
        .load_input_states(&LogicalRuntimeId::for_session(session_id))
        .await
        .ok()
        .map(|rows| {
            rows.iter()
                .map(|row| match row {
                    InputStateRow::Decoded(stored) => Some(stored.seed.phase),
                    _ => None,
                })
                .collect()
        })
}

/// What a row's turn was answered.
#[derive(Debug)]
enum TurnAnswer {
    /// A direct turn's result, its error in `Debug` form.
    Direct(Result<(), String>),
    /// A queued input's waiter's answer.
    Queued(Result<CompletionOutcome, CompletionWaitError>),
}

/// What a turn whose writer stopped during one of its steps did.
struct FencedRun {
    /// The turn's answer.
    answer: TurnAnswer,
    /// When the turn was answered.
    answered_at: tokio::time::Instant,
    model_requests: usize,
    dispatched: usize,
    lease_live_after: bool,
    poisoned_after: bool,
    /// The phase of each of the session's input rows once the waiter was
    /// answered, when the store answered.
    input_phases: Option<Vec<Option<InputLifecycleState>>>,
    /// The row, kept open for one that goes on observing it.
    row: FenceRow,
}

impl FencedRun {
    /// Whether the turn completed: a direct turn's result, or a queued
    /// input's completed result.
    fn completed(&self) -> bool {
        matches!(
            self.answer,
            TurnAnswer::Direct(Ok(())) | TurnAnswer::Queued(Ok(CompletionOutcome::Completed(_)))
        )
    }

    /// The turn's answer, for a row's messages and text checks.
    fn answer_text(&self) -> String {
        format!("{:?}", self.answer)
    }

    /// Check that the turn was answered within one dispatch wait bound (and
    /// the agent's retry backoff) of the writer's stop: no second wait, such
    /// as a closing save's, came after the first.
    fn assert_answered_within_one_wait(&self, site: &str) {
        let stopped_at = self
            .row
            .control
            .stopped_at()
            .expect("the writer stopped during the run");
        let answered_after = self.answered_at.duration_since(stopped_at);
        assert!(
            answered_after <= DISPATCH_WAIT_BOUND + ANSWER_SLACK,
            "{site}: the turn is answered within one dispatch wait of the stop: {answered_after:?}"
        );
    }
}

/// Run the initial turn of a fresh session over a host store realm whose
/// writer's window closes during the round commit `closes_during` names,
/// through renews that fail from then on.
async fn run_with_window_closing_during(closes_during: ClosesDuring) -> FencedRun {
    run_with_stop(closes_during, Stop::FailingRenews, Turn::Direct).await
}

/// Run one turn, the way `turn` names, of a fresh session over a host
/// store realm whose writer stops, the way `stop` names, during the step
/// `closes_during` names.
async fn run_with_stop(closes_during: ClosesDuring, stop: Stop, turn: Turn) -> FencedRun {
    let mut row = FenceRow::open(closes_during, stop, turn).await;
    let answer = row.run_turn(TURN_DEADLINE).await;
    let answered_at = tokio::time::Instant::now();
    let input_phases = row.input_phases().await;
    FencedRun {
        answer,
        answered_at,
        model_requests: row.model_requests(),
        dispatched: row.dispatched(),
        lease_live_after: row.realm.lease_is_live(),
        poisoned_after: row.realm.realm_poisoned().is_some(),
        input_phases,
        row,
    }
}

/// A turn whose writer one timed-out renew stopped at `site`, and whose
/// re-acquire then succeeded, ran to its end: the dispatch at `site` waited
/// for the re-acquire, and nothing ran twice.
fn assert_completed_once(run: &FencedRun, site: &str, calls: usize) {
    assert!(
        run.completed(),
        "{site}: the dispatch waits for the re-acquire, and the turn completes: {}",
        run.answer_text()
    );
    assert_eq!(
        run.model_requests, 2,
        "{site}: the tool round and the answer, each asked once"
    );
    assert_eq!(
        run.dispatched, calls,
        "{site}: each call of the round ran once"
    );
    assert!(
        run.lease_live_after,
        "{site}: the re-acquire resumed the writer"
    );
    assert!(!run.poisoned_after, "{site}: no other writer showed up");
    assert_eq!(
        run.input_phases,
        Some(vec![Some(InputLifecycleState::Consumed)]),
        "{site}: the turn's one input is consumed once, not stopped or queued again"
    );
}

/// A turn whose writer stopped while the round's second commit was in
/// flight, and whose re-acquire did not restore its window, made no model
/// call after the stop: the dispatch fence refused it, as before.
fn assert_refused_after_the_round(run: &FencedRun, row: &str) {
    assert!(
        !run.completed(),
        "{row}: the run stops at the closed window"
    );
    assert!(
        run.answer_text().contains("self-fence window closed"),
        "{row}: the dispatch fence refused it, not the write path: {}",
        run.answer_text()
    );
    assert_eq!(
        run.dispatched, 1,
        "{row}: the call ran once, before the writer stopped"
    );
    assert_eq!(run.model_requests, 1, "{row}: no model call after the stop");
}

/// A direct turn whose writer stayed stopped through its whole dispatch
/// wait, with no other writer found, failed with a retryable store error
/// after that one wait: its run is stopped and suspended there, with no
/// terminal and no closing save, and nothing proves another owner, so it is
/// not the teardown class. Before, the run ended failed at the wait, its
/// closing save waited a second bound, and its terminal's write met the gap,
/// which left the session repair-blocked.
fn assert_direct_turn_failed_retryably(run: &FencedRun, row: &str) {
    assert!(
        !run.completed(),
        "{row}: the turn does not complete while the fence stays closed"
    );
    let answer = run.answer_text();
    assert!(
        answer.contains("stayed closed through its dispatch wait"),
        "{row}: the dispatch fence ended the turn after its wait: {answer}"
    );
    assert!(
        !answer.contains("DurableProjectionAuthorityUnknown"),
        "{row}: no other writer was found, so not the teardown class: {answer}"
    );
    run.assert_answered_within_one_wait(row);
}

/// Run `row`'s queued turn over a writer whose fence gap outlasts three
/// dispatch wait bounds, and check what its run did while the gap lasted:
/// the run is suspended, its waiter is told within one dispatch wait that
/// its input stays queued (`RequeuedAcrossStop`), and the input stays
/// pending through the gap, with no model call and no tool call after the
/// stop. `requests` and `calls` are the model requests and tool calls the
/// run made before the stop.
async fn assert_suspended_through_the_gap(
    row: &mut FenceRow,
    site: &str,
    requests: usize,
    calls: usize,
) {
    let started = tokio::time::Instant::now();
    let answer = row.run_turn(ANSWER_DEADLINE).await;
    assert!(
        matches!(
            answer,
            TurnAnswer::Queued(Err(CompletionWaitError::RequeuedAcrossStop { .. }))
        ),
        "{site}: the suspended run's waiter is told that its input stays queued: {answer:?}"
    );
    row.assert_answered_within_one_wait(site);
    let phases = row.input_phases().await;
    assert!(
        all_pending(&phases),
        "{site}: the input stays pending: {phases:?}"
    );
    assert_eq!(
        row.model_requests(),
        requests,
        "{site}: no model call after the stop"
    );
    assert_eq!(
        row.dispatched(),
        calls,
        "{site}: no tool call after the stop"
    );
    tokio::time::sleep_until(started + DISPATCH_WAIT_BOUND * 3 + Duration::from_secs(1)).await;
    let phases = row.input_phases().await;
    assert!(
        all_pending(&phases),
        "{site}: three dispatch wait bounds into the gap, the input is still pending: {phases:?}"
    );
    assert_eq!(
        row.model_requests(),
        requests,
        "{site}: no model call while the gap lasts"
    );
    assert_eq!(
        row.dispatched(),
        calls,
        "{site}: no tool call while the gap lasts"
    );
}

/// A host store writer whose self-fence window has closed makes no model
/// call and dispatches no tool, through the real store checkpointer. Its
/// round commit passes the write path's own fence check, and the window
/// closes while that commit is in flight, so only the dispatch fence stands
/// between the commit and the next step:
/// - closed during the round's first commit, the round's call is never
///   dispatched, and no model call follows;
/// - closed during the round's second commit, the call ran once, while the
///   window was open, and the next model call is never made.
///
/// Every renew fails from then on, the renew of each re-acquire included,
/// so the writer never gets its window back. No other writer is found, so
/// the direct turn fails with a retryable store error once its one dispatch
/// wait passes (its run is stopped and suspended there), not with the
/// teardown class.
#[tokio::test]
async fn a_writer_past_its_fence_window_makes_no_model_call_and_dispatches_no_tool() {
    let first = run_with_window_closing_during(ClosesDuring::FirstCommit).await;
    assert!(!first.lease_live_after, "the window closed");
    assert_direct_turn_failed_retryably(&first, "closed during the first commit");
    assert_eq!(first.dispatched, 0, "the round's call is never dispatched");
    assert_eq!(first.model_requests, 1, "no model call follows");
    assert!(!first.poisoned_after, "no other writer showed up");

    let second = run_with_window_closing_during(ClosesDuring::SecondCommit).await;
    assert!(!second.lease_live_after, "the window closed");
    assert_direct_turn_failed_retryably(&second, "closed during the second commit");
    assert_eq!(
        second.dispatched, 1,
        "the call ran once, before the window closed"
    );
    assert_eq!(
        second.model_requests, 1,
        "the next model call is never made"
    );
    assert!(!second.poisoned_after, "no other writer showed up");
}

/// A writer that one timed-out renew stopped while the round's second
/// commit was in flight waits, at the next model call, for the re-acquire
/// that its next write would make. The re-acquire's listing answers 1.5 s
/// later, within the 2 s window, so the model call goes ahead and the turn
/// completes with its input consumed once. Before the fix the model call was
/// refused at once and the turn failed with the teardown class
/// (`DurableProjectionAuthorityUnknown`, "self-fence window closed ...
/// renew failed: true").
#[tokio::test]
async fn a_model_call_after_a_timed_out_renew_waits_for_the_reacquire() {
    let run = run_with_stop(
        ClosesDuring::SecondCommit,
        Stop::TimedOutRenew(Reacquire::ListsIn(LISTING_WITHIN_THE_WINDOW)),
        Turn::Queued,
    )
    .await;
    assert_completed_once(&run, "the model call after the round", 1);
}

/// A writer that one timed-out renew stopped while the round's first commit
/// was in flight waits, before it admits the round's call, for the
/// re-acquire; the call then runs once and the turn completes. Before the
/// fix the round was refused at once and the turn failed.
#[tokio::test]
async fn a_tool_round_after_a_timed_out_renew_waits_for_the_reacquire() {
    let run = run_with_stop(
        ClosesDuring::FirstCommit,
        Stop::TimedOutRenew(Reacquire::ListsIn(LISTING_WITHIN_THE_WINDOW)),
        Turn::Queued,
    )
    .await;
    assert_completed_once(&run, "the round after its first commit", 1);
}

/// A writer that one timed-out renew stopped while the round's first call
/// ran (two calls, one at a time) waits, right before the second call
/// starts, for the re-acquire; the second call then runs once and the turn
/// completes. Before the fix the second call was refused ("this writer's
/// self-fence window closed before the call started") and the turn failed.
#[tokio::test]
async fn a_tool_call_after_a_timed_out_renew_waits_for_the_reacquire() {
    let run = run_with_stop(
        ClosesDuring::FirstCall,
        Stop::TimedOutRenew(Reacquire::ListsIn(LISTING_WITHIN_THE_WINDOW)),
        Turn::Queued,
    )
    .await;
    assert_completed_once(&run, "the second call of the round", 2);
}

/// A re-acquire that keeps failing (every acquire answers a store error)
/// restores nothing: the waiting model call of a direct turn waits its whole
/// bound, retrying, and the turn then fails with a retryable store error,
/// its run stopped and suspended there. No other writer was found: the
/// writer stays stopped, the realm is not poisoned, and the turn is not the
/// teardown class. Before, the turn was refused at once with the teardown
/// class.
#[tokio::test]
async fn a_direct_turn_whose_reacquire_keeps_failing_fails_retryably() {
    let run = run_with_stop(
        ClosesDuring::SecondCommit,
        Stop::TimedOutRenew(Reacquire::Fails),
        Turn::Direct,
    )
    .await;
    assert_direct_turn_failed_retryably(&run, "a failing re-acquire");
    assert_eq!(run.dispatched, 1, "the call ran once, before the stop");
    assert_eq!(run.model_requests, 1, "no model call after the stop");
    assert!(!run.lease_live_after, "the writer stays stopped");
    assert!(!run.poisoned_after, "a failed acquire poisons nothing");
}

/// A re-acquire whose listing shows a batch another writer committed while
/// this one was stopped poisons the realm: the waiting model call is
/// refused with the teardown class, and nothing more is dispatched.
#[tokio::test]
async fn a_foreign_batch_at_the_reacquire_refuses_the_dispatch() {
    let run = run_with_stop(
        ClosesDuring::SecondCommit,
        Stop::TimedOutRenew(Reacquire::FindsForeignBatch),
        Turn::Direct,
    )
    .await;
    assert_refused_after_the_round(&run, "another writer's batch");
    assert!(run.poisoned_after, "the foreign batch poisons the realm");
    assert!(!run.lease_live_after, "the writer never resumes");
}

/// The wait is bounded: a re-acquire whose listing takes 12 s outlasts the
/// 5 s dispatch wait bound, so the direct turn fails with a retryable store
/// error after that one wait, its run stopped and suspended there. The
/// session keeps its live session, as a suspended runtime run does. Another
/// direct turn asked while the fence stays closed is refused busy
/// (`SESSION_BUSY`): no run starts beside the suspended one while it is
/// marked resumable. The re-acquire itself goes on and resumes the writer
/// once its listing answers; the runtime then settles the stopped run,
/// which never asks the model again, and the next direct turn completes on
/// the live session, brought to the settled store. Before, the turn failed
/// only after a second wait, its closing save's, and its terminal's write
/// met the gap, which left the session repair-blocked; and a failed direct
/// turn discarded its live session, so the next direct turn, which recovers
/// no stored session itself, could not run.
#[tokio::test]
async fn a_direct_turn_fails_retryably_when_its_reacquire_outlasts_the_bound() {
    let run = run_with_stop(
        ClosesDuring::SecondCommit,
        Stop::TimedOutRenew(Reacquire::ListsIn(LISTING_PAST_THE_WINDOW)),
        Turn::Direct,
    )
    .await;
    assert_direct_turn_failed_retryably(&run, "a re-acquire slower than the bound");
    assert_eq!(run.dispatched, 1, "the call ran once, before the stop");
    assert_eq!(run.model_requests, 1, "no model call after the stop");
    assert!(!run.poisoned_after, "a slow listing poisons nothing");
    assert!(
        run.row
            .service
            .has_live_session_under_runtime_turn_boundary(&run.row.session_id)
            .await
            .expect("read the session's live authority"),
        "the suspended direct run's live session stays, for its settlement and the next turn"
    );
    let during_the_gap = run.row.run_direct_turn("probe again").await;
    assert!(
        matches!(
            during_the_gap,
            Err(crate::surface::SurfaceRuntimeMaterializeError::Session(
                SessionError::Busy { .. }
            ))
        ),
        "a direct turn while the stopped run waits for the fence is refused busy: \
         {during_the_gap:?}"
    );
    tokio::time::sleep(LISTING_PAST_THE_WINDOW + Duration::from_secs(2)).await;
    assert!(
        run.row.realm.lease_is_live(),
        "the slow re-acquire resumes the writer once its listing answers"
    );
    assert!(
        run.row.wait_until_no_run_is_marked(TURN_DEADLINE).await,
        "the runtime settles the stopped run once the fence is back"
    );
    let next = run.row.run_direct_turn("probe again").await;
    assert!(next.is_ok(), "the next direct turn completes: {next:?}");
    assert_eq!(
        run.row.model_requests(),
        2,
        "the stopped run never asked the model again: only the next turn did"
    );
    assert_eq!(run.row.dispatched(), 1, "nothing ran twice");
}

/// A direct turn whose round commit meets a store fence gap is stopped and
/// suspended at the commit's wait, as at a dispatch: the run's first model
/// call stops the writer, whose re-acquires keep failing, and answers with
/// the round's call, so the round's first commit is the first write that
/// meets the stopped writer. The turn fails with a retryable store error
/// after that one wait, not with the teardown class, and the round's call
/// never runs. Before, the refused commit failed the run, its closing save
/// waited a second bound, and its terminal's write met the gap, which left
/// the session repair-blocked.
#[tokio::test]
async fn a_direct_turns_round_commit_at_a_fence_gap_fails_retryably_after_one_wait() {
    let run = run_with_stop(
        ClosesDuring::RoundModelCall,
        Stop::TimedOutRenew(Reacquire::Fails),
        Turn::Direct,
    )
    .await;
    let row = "a direct turn's round commit";
    assert!(
        !run.completed(),
        "{row}: the turn does not complete while the fence stays closed"
    );
    let answer = run.answer_text();
    assert!(
        answer.contains("stayed closed through its round commit's wait"),
        "{row}: the commit's fence wait ended the turn: {answer}"
    );
    assert!(
        !answer.contains("DurableProjectionAuthorityUnknown"),
        "{row}: no other writer was found, so not the teardown class: {answer}"
    );
    run.assert_answered_within_one_wait(row);
    assert_eq!(run.dispatched, 0, "{row}: the round's call never runs");
    assert_eq!(run.model_requests, 1, "{row}: no model call after the stop");
    assert!(
        !run.poisoned_after,
        "{row}: a failed acquire poisons nothing"
    );
}

/// An ordinary run whose writer stopped before its first round commit (its
/// first model call failed retryably once the writer stopped, so the retry
/// waits at the fence), with acquires failing for 18 s, longer than three
/// dispatch wait bounds: the run is suspended at the end of its first wait,
/// its waiter is told the input stays queued, and the input stays pending
/// through the gap with no stage attempt spent on it. Once the fence is back
/// the run starts again from its input, under the same run, and completes:
/// the input is consumed once and the prompt is in the transcript once.
/// Before, the retry was refused with the teardown class and the teardown
/// ended the input stopped.
#[tokio::test]
async fn an_ordinary_runs_fence_gap_before_its_first_commit_suspends_it_and_it_runs_once_the_fence_is_back()
 {
    let mut row = FenceRow::open(
        ClosesDuring::FirstModelCall,
        Stop::TimedOutRenew(Reacquire::FailsFor(FENCE_GAP)),
        Turn::Queued,
    )
    .await;
    assert_suspended_through_the_gap(&mut row, "before the first commit", 1, 0).await;
    let phases = row
        .wait_for_input_phases(&[InputLifecycleState::Consumed], TURN_DEADLINE)
        .await;
    assert_eq!(
        phases,
        Some(vec![Some(InputLifecycleState::Consumed)]),
        "the input runs once the fence is back, and is consumed once"
    );
    assert_eq!(row.model_requests(), 2, "the run asked the model once more");
    assert_eq!(row.dispatched(), 0, "the answer called no tool");
    assert!(row.realm.lease_is_live(), "the writer resumed");
    assert_eq!(
        row.committed_rows().await,
        vec![
            "user probe once".to_string(),
            "assistant probed".to_string()
        ],
        "the prompt is in the transcript once"
    );
}

/// An ordinary run whose writer stopped while its round's second commit was
/// in flight, with acquires failing for 18 s: the run is suspended at its
/// next model call, its waiter is told the input stays queued, and the input
/// stays pending through the gap. Once the fence is back the run resumes from
/// that commit: its tool round is kept, the call does not run again, and the
/// next model call completes the run with its input consumed once.
#[tokio::test]
async fn an_ordinary_runs_fence_gap_after_a_round_suspends_it_and_it_resumes_once_the_fence_is_back()
 {
    let mut row = FenceRow::open(
        ClosesDuring::SecondCommit,
        Stop::TimedOutRenew(Reacquire::FailsFor(FENCE_GAP)),
        Turn::Queued,
    )
    .await;
    assert_suspended_through_the_gap(&mut row, "after the round", 1, 1).await;
    let phases = row
        .wait_for_input_phases(&[InputLifecycleState::Consumed], TURN_DEADLINE)
        .await;
    assert_eq!(
        phases,
        Some(vec![Some(InputLifecycleState::Consumed)]),
        "the input runs once the fence is back, and is consumed once"
    );
    assert_eq!(
        row.model_requests(),
        2,
        "the resumed run asked the model once"
    );
    assert_eq!(row.dispatched(), 1, "the round's call did not run again");
    assert_eq!(
        row.committed_rows().await,
        vec![
            "user probe once".to_string(),
            "assistant tool use".to_string(),
            "tool results".to_string(),
            "assistant probed".to_string(),
        ],
        "the kept round is in the transcript once, then the answer"
    );
}

/// A resumed run is suspended for a fence gap the same way. The run of the
/// row above resumes once the first gap ends, and its first model call stops
/// the writer again, with acquires failing for another 18 s: the resumed run
/// is suspended in its turn, keeps its round, and resumes again once the
/// fence is back, without running its call again. Before, a resumed run's
/// failure stopped the runtime and ended its input stopped.
#[tokio::test]
async fn a_resumed_run_suspended_for_a_fence_gap_keeps_its_round_and_resumes_again() {
    let mut row = FenceRow::open_scripted(
        ClosesDuring::SecondCommit,
        Stop::TimedOutRenew(Reacquire::FailsFor(FENCE_GAP)),
        Turn::Queued,
        vec![ModelStep::Tools, ModelStep::StopThenOverloaded],
    )
    .await;
    assert_suspended_through_the_gap(&mut row, "the first gap", 1, 1).await;
    // The run resumes once the first gap ends; its model call stops the
    // writer again.
    let resumed = tokio::time::Instant::now();
    while row.model_requests() < 2 && resumed.elapsed() < TURN_DEADLINE {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(row.model_requests(), 2, "the resumed run asked the model");
    tokio::time::sleep(DISPATCH_WAIT_BOUND * 3).await;
    let phases = row.input_phases().await;
    assert!(
        all_pending(&phases),
        "the resumed run is suspended again, its input pending through the second gap: {phases:?}"
    );
    assert_eq!(
        row.model_requests(),
        2,
        "no model call while the second gap lasts"
    );
    let phases = row
        .wait_for_input_phases(&[InputLifecycleState::Consumed], TURN_DEADLINE)
        .await;
    assert_eq!(
        phases,
        Some(vec![Some(InputLifecycleState::Consumed)]),
        "the resumed run completes once the fence is back, its input consumed once"
    );
    assert_eq!(
        row.model_requests(),
        3,
        "the run resumed again and asked the model"
    );
    assert_eq!(row.dispatched(), 1, "the round's call never ran again");
    assert_eq!(
        row.committed_rows().await,
        vec![
            "user probe once".to_string(),
            "assistant tool use".to_string(),
            "tool results".to_string(),
            "assistant probed".to_string(),
        ],
        "the kept round is in the transcript once, then the answer"
    );
}

/// Another writer's batch at the re-acquire poisons the realm: the waiting
/// model call is refused and the live session torn down, as before, but the
/// input's waiter is told the input stays queued (`RequeuedAcrossStop`)
/// instead of abandoned, because the store, which refuses every write of
/// this writer from then on, keeps the input pending for the new owner, as
/// the new owner reads it.
#[tokio::test]
async fn a_foreign_batch_tells_the_waiter_its_input_stays_queued_for_the_new_owner() {
    let mut row = FenceRow::open(
        ClosesDuring::SecondCommit,
        Stop::TimedOutRenew(Reacquire::FindsForeignBatch),
        Turn::Queued,
    )
    .await;
    let answer = row.run_turn(ANSWER_DEADLINE).await;
    assert!(
        matches!(
            answer,
            TurnAnswer::Queued(Err(CompletionWaitError::RequeuedAcrossStop { .. }))
        ),
        "the waiter is told the input stays queued, not that it ended: {answer:?}"
    );
    assert!(
        row.realm.realm_poisoned().is_some(),
        "the foreign batch poisons the realm"
    );
    assert_eq!(row.model_requests(), 1, "no model call after the stop");
    assert_eq!(row.dispatched(), 1, "the call ran once, before the stop");
    let phases = row.new_owner_input_phases().await;
    assert!(
        all_pending(&phases),
        "the new owner finds the input pending, to run it: {phases:?}"
    );
}

/// A round commit that meets a fence gap waits for the re-acquire as a
/// dispatch does, and suspends the run when none restores the writer: the
/// run's first model call stops the writer, with acquires failing for 18 s,
/// and answers with the round's call, so the round's first commit is the
/// first write that meets the stopped writer. The waiter is told the input
/// stays queued, the input stays pending through the gap with no stage
/// attempt spent on it, and once the fence is back the run starts again from
/// its input (the refused round was never committed) and completes once.
/// Before, the refused commit failed the run, which spent a stage attempt on
/// each retry through the gap.
#[tokio::test]
async fn a_round_commit_that_meets_a_fence_gap_suspends_the_run_and_it_runs_once_the_fence_is_back()
{
    let mut row = FenceRow::open(
        ClosesDuring::RoundModelCall,
        Stop::TimedOutRenew(Reacquire::FailsFor(FENCE_GAP)),
        Turn::Queued,
    )
    .await;
    assert_suspended_through_the_gap(&mut row, "the round's first commit", 1, 0).await;
    let phases = row
        .wait_for_input_phases(&[InputLifecycleState::Consumed], TURN_DEADLINE)
        .await;
    assert_eq!(
        phases,
        Some(vec![Some(InputLifecycleState::Consumed)]),
        "the input runs once the fence is back, and is consumed once"
    );
    assert_eq!(
        row.model_requests(),
        3,
        "the run asks for its round again, then for the answer"
    );
    assert_eq!(row.dispatched(), 1, "the round's call runs once");
    assert_eq!(
        row.committed_rows().await,
        vec![
            "user probe once".to_string(),
            "assistant tool use".to_string(),
            "tool results".to_string(),
            "assistant probed".to_string(),
        ],
        "the prompt and the round are in the transcript once"
    );
}

/// Another writer that takes the fence over while the round's call runs is
/// first met at the round's next commit: this writer's renew is fenced out,
/// which poisons its realm but leaves its own window open, so no dispatch
/// check stood in between. The commit fails, the live session is torn down,
/// and the input's waiter is told the input stays queued
/// (`RequeuedAcrossStop`), since the store keeps it pending for the new owner,
/// as the new owner reads it. Before, the failed commit was an ordinary run
/// failure: the run was retried in this process and the waiter got another
/// answer.
#[tokio::test]
async fn a_takeover_first_met_at_a_commit_tells_the_waiter_its_input_stays_queued_for_the_new_owner()
 {
    let mut row = FenceRow::open(ClosesDuring::OnlyCall, Stop::TakenOver, Turn::Queued).await;
    let answer = row.run_turn(ANSWER_DEADLINE).await;
    assert!(
        matches!(
            answer,
            TurnAnswer::Queued(Err(CompletionWaitError::RequeuedAcrossStop { .. }))
        ),
        "the waiter is told the input stays queued, not that it failed: {answer:?}"
    );
    assert!(
        row.realm.realm_poisoned().is_some(),
        "the fenced-out renew poisons the realm"
    );
    assert_eq!(row.model_requests(), 1, "no model call after the takeover");
    assert_eq!(
        row.dispatched(),
        1,
        "the call ran once, before the takeover"
    );
    let phases = row.new_owner_input_phases().await;
    assert!(
        all_pending(&phases),
        "the new owner finds the input pending, to run it: {phases:?}"
    );
}

/// A cancel accepted while the run waits at the fence of its stopped writer
/// survives the fence gap that then suspends the run: the suspension keeps
/// the run's stop intent, the run never resumes, and once the fence is back
/// it is settled, its input abandoned as cancelled. Its waiter is answered
/// at the suspension, within one dispatch wait of the stop, that the input
/// stays queued (`RequeuedAcrossStop`): it does not wait out the gap. Before
/// round 3 the suspension dropped the cancel (the resumed turn's start
/// cleared it), and the run resumed and completed; before round 4 the
/// waiter waited for the fence to come back, or for the whole outage.
#[tokio::test]
async fn a_cancel_accepted_before_a_fence_gap_survives_it_and_the_run_does_not_come_back() {
    let row = FenceRow::open(
        ClosesDuring::FirstModelCall,
        Stop::TimedOutRenew(Reacquire::FailsFor(FENCE_GAP)),
        Turn::Queued,
    )
    .await;
    let completion = row.accept_prompt().await;
    row.control.wait_until_stopped().await;
    assert!(
        row.control.writer_stopped(),
        "the run's model call stopped the writer"
    );
    // The run's retry now waits at the fence of the stopped writer.
    tokio::time::sleep(CANCEL_INTO_THE_WAIT).await;
    row.machine
        .cancel_after_boundary(&row.session_id)
        .await
        .expect("the cancel is accepted while the run waits at the fence");
    let answer = tokio::time::timeout(TURN_DEADLINE, completion.wait())
        .await
        .expect("the input's waiter is answered");
    assert!(
        matches!(answer, Err(CompletionWaitError::RequeuedAcrossStop { .. })),
        "the suspended run's waiter is told that its input stays queued: {answer:?}"
    );
    row.assert_answered_within_one_wait("a cancel before the gap");
    tokio::time::sleep(DISPATCH_WAIT_BOUND * 2).await;
    assert_eq!(
        row.model_requests(),
        1,
        "the cancelled run never asks the model again"
    );
    row.assert_settled_as_cancelled("a cancel before the gap", 1)
        .await;
}

/// An interrupt of a session whose run a fence gap suspended settles that
/// run: the interrupt is accepted, the run never resumes, and once the fence
/// is back its input ends abandoned. Before, the interrupt found no current
/// run and was refused, and the run resumed and completed once the fence was
/// back.
#[tokio::test]
async fn an_interrupt_settles_a_run_a_fence_gap_suspended_and_it_does_not_come_back() {
    let mut row = FenceRow::open(
        ClosesDuring::FirstModelCall,
        Stop::TimedOutRenew(Reacquire::FailsFor(FENCE_GAP)),
        Turn::Queued,
    )
    .await;
    let answer = row.run_turn(ANSWER_DEADLINE).await;
    assert!(
        matches!(
            answer,
            TurnAnswer::Queued(Err(CompletionWaitError::RequeuedAcrossStop { .. }))
        ),
        "the gap suspends the run: {answer:?}"
    );
    row.assert_answered_within_one_wait("an interrupt of the suspended run");
    row.machine
        .hard_cancel_current_run(&row.session_id, "the user stops the suspended run")
        .await
        .expect("the interrupt is accepted for the run the gap suspended");
    row.assert_settled_as_cancelled("an interrupt of the suspended run", 1)
        .await;
}

/// A run a fence gap suspended waits in the runtime loop for the fence to
/// come back; when it comes back to another writer's batch, the realm is
/// lost, and the loop tears the session down as a dispatch-time takeover
/// does: the runtime loop ends, the run never resumes, and the new owner
/// finds the input pending. Before, the loop stayed attached and idle.
#[tokio::test]
async fn a_takeover_met_while_a_suspended_run_waits_tears_the_session_down() {
    let mut row = FenceRow::open(
        ClosesDuring::FirstModelCall,
        Stop::TimedOutRenew(Reacquire::FailsForThenForeign(FENCE_GAP)),
        Turn::Queued,
    )
    .await;
    let answer = row.run_turn(ANSWER_DEADLINE).await;
    assert!(
        matches!(
            answer,
            TurnAnswer::Queued(Err(CompletionWaitError::RequeuedAcrossStop { .. }))
        ),
        "the gap suspends the run: {answer:?}"
    );
    row.assert_answered_within_one_wait("a takeover while suspended");
    let started = tokio::time::Instant::now();
    while row.realm.realm_poisoned().is_none() && started.elapsed() < TURN_DEADLINE {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        row.realm.realm_poisoned().is_some(),
        "the re-acquire after the gap finds another writer's batch"
    );
    assert!(
        row.wait_for_the_runtime_loop_to_end(TURN_DEADLINE).await,
        "the session's runtime loop ends, as for a takeover met at a dispatch"
    );
    assert_eq!(row.model_requests(), 1, "the run never resumes");
    let phases = row.new_owner_input_phases().await;
    assert!(
        all_pending(&phases),
        "the new owner finds the input pending, to run it: {phases:?}"
    );
}

/// A closing save that meets a store fence gap waits for the re-acquire as
/// a round commit does, and is written again, once, when a re-acquire within
/// the wait restored the writer: the run's one model call stops the writer
/// (acquires fail for 3 s) and answers with text, so the run's closing save
/// is its first write after the stop. The turn completes once, its input
/// consumed. Before, the refused closing save failed the turn at once, after
/// its final answer, with the teardown class.
#[tokio::test]
async fn a_closing_save_refused_in_a_short_gap_is_written_again_and_the_turn_completes_once() {
    let run = run_with_stop(
        ClosesDuring::TextModelCall,
        Stop::TimedOutRenew(Reacquire::FailsFor(SHORT_GAP)),
        Turn::Queued,
    )
    .await;
    assert!(
        run.completed(),
        "the closing save waits for the re-acquire and the turn completes: {}",
        run.answer_text()
    );
    assert_eq!(run.model_requests, 1, "the model was asked once");
    assert!(run.lease_live_after, "the re-acquire resumed the writer");
    assert!(!run.poisoned_after, "no other writer showed up");
    assert_eq!(
        run.input_phases,
        Some(vec![Some(InputLifecycleState::Consumed)]),
        "the turn's one input is consumed once"
    );
    assert_eq!(
        run.row.committed_rows().await,
        vec![
            "user probe once".to_string(),
            "assistant probed".to_string()
        ],
        "the prompt and the answer are in the transcript once"
    );
}

/// The documented residual of a closing save: a closing save still refused
/// after its one wait keeps the teardown class. The turn's terminal was
/// recorded before its closing save, so the run cannot be suspended there:
/// the turn fails, its waiter is not told that its input stays queued, and
/// the model is not asked again in this process. The wait and its one retry
/// are the protection (the short-gap row above).
#[tokio::test]
async fn a_closing_save_whose_gap_outlasts_its_wait_still_fails_the_turn() {
    let run = run_with_stop(
        ClosesDuring::TextModelCall,
        Stop::TimedOutRenew(Reacquire::FailsFor(FENCE_GAP)),
        Turn::Queued,
    )
    .await;
    assert!(
        !run.completed(),
        "the turn fails after its final answer: {}",
        run.answer_text()
    );
    assert!(
        !matches!(
            run.answer,
            TurnAnswer::Queued(Err(CompletionWaitError::RequeuedAcrossStop { .. }))
        ),
        "the run is not suspended, so its input is not answered as queued: {}",
        run.answer_text()
    );
    assert_eq!(run.model_requests, 1, "the model is not asked again");
    assert!(!run.poisoned_after, "no other writer showed up");
}

/// An interrupt that reaches a run waiting at the fence of its stopped
/// writer stops it through a suspension, not a hard interrupt: the stop is
/// recorded on the run and the run is suspended at once, its waiter told
/// within one dispatch wait of the stop that its input stays queued. The run
/// never comes back, and once the fence is back it is settled, its input
/// abandoned as cancelled. Before, the hard interrupt cancelled the run only
/// in memory: the gap refused the cancel's writes, the session had to reload,
/// and the reload resumed the stopped run from the store.
#[tokio::test]
async fn an_interrupt_of_a_run_waiting_at_the_fence_suspends_it_and_it_does_not_come_back() {
    let row = FenceRow::open(
        ClosesDuring::FirstModelCall,
        Stop::TimedOutRenew(Reacquire::FailsFor(FENCE_GAP)),
        Turn::Queued,
    )
    .await;
    let completion = row.accept_prompt().await;
    row.control.wait_until_stopped().await;
    // The run's retry now waits at the fence of the stopped writer.
    tokio::time::sleep(CANCEL_INTO_THE_WAIT).await;
    row.machine
        .hard_cancel_current_run(&row.session_id, "the user stops the run")
        .await
        .expect("the interrupt is accepted for the run waiting at the fence");
    let answer = tokio::time::timeout(ANSWER_DEADLINE, completion.wait())
        .await
        .expect("the input's waiter is answered");
    assert!(
        matches!(answer, Err(CompletionWaitError::RequeuedAcrossStop { .. })),
        "the stopped run is suspended and its waiter told the input stays queued: {answer:?}"
    );
    row.assert_answered_within_one_wait("an interrupt at the fence");
    row.assert_settled_as_cancelled("an interrupt at the fence", 1)
        .await;
}

/// A run stop that reaches a run waiting at the fence of its stopped writer
/// is recorded and the run suspended, as an interrupt there is: the call
/// returns at once, its receipt listing no contributor (the inputs end only
/// once the fence is back), the waiter is told within one dispatch wait of
/// the stop that its input stays queued, and once the fence is back the run
/// is settled, its input abandoned as cancelled. Before, the stop's hard
/// interrupt cancelled the run only in memory, the contributor waits failed
/// with the reload that followed, and the reload resumed the stopped run.
#[tokio::test]
async fn a_stop_of_a_run_waiting_at_the_fence_returns_at_once_and_the_run_does_not_come_back() {
    let row = FenceRow::open(
        ClosesDuring::FirstModelCall,
        Stop::TimedOutRenew(Reacquire::FailsFor(FENCE_GAP)),
        Turn::Queued,
    )
    .await;
    let completion = row.accept_prompt().await;
    row.control.wait_until_stopped().await;
    tokio::time::sleep(CANCEL_INTO_THE_WAIT).await;
    let run_id = row
        .machine
        .current_run(&row.session_id)
        .await
        .expect("the run waits at the fence");
    let receipt = tokio::time::timeout(
        ANSWER_DEADLINE,
        row.machine
            .stop_run(&row.session_id, &run_id, "the user stops the run"),
    )
    .await
    .expect("the stop returns at once")
    .expect("the stop is accepted");
    assert!(
        matches!(
            &receipt,
            RunStopReceipt::Stopped { contributors, .. } if contributors.is_empty()
        ),
        "the stop is recorded and its receipt lists no contributor: {receipt:?}"
    );
    let answer = tokio::time::timeout(ANSWER_DEADLINE, completion.wait())
        .await
        .expect("the input's waiter is answered");
    assert!(
        matches!(answer, Err(CompletionWaitError::RequeuedAcrossStop { .. })),
        "the stopped run is suspended and its waiter told the input stays queued: {answer:?}"
    );
    row.assert_answered_within_one_wait("a stop at the fence");
    row.assert_settled_as_cancelled("a stop at the fence", 1)
        .await;
}

/// Another writer that takes the fence over during the run's first model
/// call poisons the realm while this writer's own window still reads open
/// (the fenced-out renew came a renew interval after the last one, within
/// the 2 s window). The model call fails retryably, and the retry, half a
/// second later and still within the window, is refused at once on the lost
/// fence by its dispatch check, before the open-window fast path: no model
/// call follows, the live session is torn down, and the waiter is told the
/// input stays queued for the new owner, which reads it pending. Without
/// that check the retry would ask the model on a realm another writer holds.
#[tokio::test]
async fn a_takeover_met_by_a_dispatch_whose_window_reads_open_refuses_it() {
    let mut row = FenceRow::open(ClosesDuring::FirstModelCall, Stop::TakenOver, Turn::Queued).await;
    let answer = row.run_turn(ANSWER_DEADLINE).await;
    assert!(
        matches!(
            answer,
            TurnAnswer::Queued(Err(CompletionWaitError::RequeuedAcrossStop { .. }))
        ),
        "the waiter is told the input stays queued for the new owner: {answer:?}"
    );
    assert!(
        row.realm.realm_poisoned().is_some(),
        "the fenced-out renew poisons the realm"
    );
    assert_eq!(
        row.model_requests(),
        1,
        "the retry's model call never starts on the lost fence"
    );
    let phases = row.new_owner_input_phases().await;
    assert!(
        all_pending(&phases),
        "the new owner finds the input pending, to run it: {phases:?}"
    );
}

/// A round commit refused in a gap shorter than its wait is written again,
/// once, when a re-acquire within the wait restored the writer: the run's
/// first model call stops the writer (acquires fail for 3 s) and answers
/// with the round's call, the round's first commit is refused, waits, and is
/// written again. The round's call runs once and the run completes once.
#[tokio::test]
async fn a_round_commit_refused_in_a_short_gap_is_written_again_and_the_run_completes_once() {
    // The round's call, then the answer: no second round.
    let mut row = FenceRow::open_scripted(
        ClosesDuring::RoundModelCall,
        Stop::TimedOutRenew(Reacquire::FailsFor(SHORT_GAP)),
        Turn::Queued,
        vec![ModelStep::StopThenTools],
    )
    .await;
    let answer = row.run_turn(TURN_DEADLINE).await;
    assert!(
        matches!(
            answer,
            TurnAnswer::Queued(Ok(CompletionOutcome::Completed(_)))
        ),
        "the round commit waits for the re-acquire and the run completes: {answer:?}"
    );
    assert_eq!(
        row.model_requests(),
        2,
        "the round and the answer, each asked once"
    );
    assert_eq!(row.dispatched(), 1, "the round's call runs once");
    assert!(
        row.realm.lease_is_live(),
        "the re-acquire resumed the writer"
    );
    assert_eq!(
        row.input_phases().await,
        Some(vec![Some(InputLifecycleState::Consumed)]),
        "the run's one input is consumed once"
    );
    assert_eq!(
        row.committed_rows().await,
        vec![
            "user probe once".to_string(),
            "assistant tool use".to_string(),
            "tool results".to_string(),
            "assistant probed".to_string(),
        ],
        "the prompt and the round are in the transcript once"
    );
}

/// A graceful requeue stop of a facade session whose durable run is in
/// flight suspends the run instead of cancelling it, as on the other runtime
/// surfaces: the facade's default executor suspends a run. The run's second
/// model call is in flight after its committed tool round; the stop keeps
/// the run's input staged and bound to the run, its waiter told the input
/// stays queued, for the next start to resume the run from that round. No
/// fence gap here: the writer is never stopped. Before round 4 the facade's
/// executor could not suspend, so the stop cancelled the run and ended its
/// input stopped.
#[tokio::test]
async fn a_requeue_stop_suspends_a_facade_run_in_flight_for_the_next_start() {
    let row = FenceRow::open_scripted(
        ClosesDuring::Never,
        Stop::default(),
        Turn::Queued,
        vec![ModelStep::Tools, ModelStep::Hold],
    )
    .await;
    let completion = row.accept_prompt().await;
    let started = tokio::time::Instant::now();
    while row.model_requests() < 2 && started.elapsed() < TURN_DEADLINE {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(
        row.model_requests(),
        2,
        "the run's second model call is in flight"
    );
    assert_eq!(row.dispatched(), 1, "its tool round ran");
    let registration = row
        .machine
        .current_session_registration_witness(&row.session_id)
        .await
        .expect("the session is registered");
    assert!(
        row.machine
            .unregister_session_registration_until_terminal_with_queued_inputs_if_current(
                &registration,
                meerkat_runtime::UnregisterQueuedInputs::Requeue {
                    max_age: Duration::from_mins(10),
                },
            )
            .await
            .expect("the requeue stop completes"),
        "the exact registration is torn down"
    );
    let answer = tokio::time::timeout(ANSWER_DEADLINE, completion.wait())
        .await
        .expect("the input's waiter is answered");
    assert!(
        matches!(answer, Err(CompletionWaitError::RequeuedAcrossStop { .. })),
        "the suspended run's waiter is told its input stays queued: {answer:?}"
    );
    assert_eq!(
        row.input_phases().await,
        Some(vec![Some(InputLifecycleState::Staged)]),
        "the run's input stays staged, bound to the suspended run, for the next start"
    );
    assert_eq!(
        row.model_requests(),
        2,
        "the suspended run asks the model no more in this process"
    );
}
