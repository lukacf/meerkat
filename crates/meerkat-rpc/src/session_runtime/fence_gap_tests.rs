//! A runtime turn over a host store realm whose writer meets a store fence
//! gap, on the JSON-RPC session runtime. Its executor applies runtime turns
//! with a reserved admission, one of the two apply paths of the session
//! service. The run's first model call stops the writer (one renew never
//! answers) and fails retryably, and every acquire fails for longer than the
//! realm's dispatch wait bound: the retry waits at the fence, and when no
//! re-acquire restores the writer the run is suspended, not failed. The turn
//! answers, within one dispatch wait bound of the stop, that its input stays
//! queued (`requeued_across_stop`), and once the fence is back the input runs
//! once, in the stage attempt it already had.
//! Unrun until the candidate's validation.

use super::*;
use meerkat_client::{LlmDoneOutcome, LlmEvent, LlmRequest};
use meerkat_core::host_store::{
    AcquireMode, CommitBatch, CommitId, FenceGrant, FenceTiming, FenceToken, HeadPage, HostStore,
    HostStoreError, Row, RowIndex, ScopeHead, ScopeKind, ScopeName, ScopeRef, ScopeVersion,
    VersionedHead,
};
use meerkat_runtime::RuntimeStore as _;
use meerkat_runtime::input_state::InputLifecycleState;
use meerkat_runtime::store::InputStateRow;
use meerkat_runtime::store::spi::{SpiRealm, SpiRealmConfig, SpiRuntimeStore};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Bound on one step of the row: it only turns a hang into a failure.
const STEP: Duration = Duration::from_secs(60);
/// The model id the row's session runs.
const MODEL: &str = "claude-sonnet-4-5";
/// How long every acquire fails from the stop on: longer than the agent's
/// retry backoff plus the realm's 5 s dispatch wait bound (a 2 s self-fence
/// window, two acquires under the 1 s renew timeout and a renew held to the
/// 1 s interval).
const GAP: Duration = Duration::from_secs(12);
/// What the turn's answer may take beyond one dispatch wait bound from the
/// stop: the agent's retry backoff before it reaches the fence.
const ANSWER_SLACK: Duration = Duration::from_secs(5);

/// How the row stops the writer, shared by the host and the model.
#[derive(Default)]
struct GapControl {
    /// The next renew never answers.
    hang_next_renew: AtomicBool,
    /// Every acquire answers `Unavailable` until then.
    acquires_fail_until: StdMutex<Option<tokio::time::Instant>>,
    /// The realm, once it is open.
    realm: std::sync::OnceLock<std::sync::Weak<SpiRealm>>,
    /// When the writer's window read closed after the stop.
    stopped_at: StdMutex<Option<tokio::time::Instant>>,
}

impl GapControl {
    fn acquires_fail(&self) -> bool {
        self.acquires_fail_until
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some_and(|until| tokio::time::Instant::now() < until)
    }

    fn writer_stopped(&self) -> bool {
        self.realm
            .get()
            .and_then(std::sync::Weak::upgrade)
            .is_some_and(|realm| !realm.lease_is_live())
    }

    /// Stop the writer with one renew that never answers, fail every
    /// acquire for [`GAP`], and wait until the realm's window reads closed.
    async fn stop_the_writer(&self) {
        *self
            .acquires_fail_until
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some(tokio::time::Instant::now() + GAP);
        self.hang_next_renew.store(true, Ordering::SeqCst);
        let _ = tokio::time::timeout(STEP, async {
            while !self.writer_stopped() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        if self.writer_stopped() {
            *self
                .stopped_at
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                Some(tokio::time::Instant::now());
        }
    }

    /// When the writer's window read closed after the stop.
    fn stopped_at(&self) -> Option<tokio::time::Instant> {
        *self
            .stopped_at
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
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

/// An in-memory host store that meets its obligations at commit, with a 1 s
/// renew interval on a 3 s lease, whose renews and acquires follow
/// `control`.
struct GapHost {
    state: StdMutex<HostState>,
    control: Arc<GapControl>,
}

impl GapHost {
    fn state(&self) -> std::sync::MutexGuard<'_, HostState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[async_trait::async_trait]
impl HostStore for GapHost {
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
        let state = self.state();
        if token.get() != state.token {
            return Err(HostStoreError::FencedOut {
                current: FenceToken::new(state.token),
            });
        }
        Ok(())
    }

    fn fence_timing(&self) -> FenceTiming {
        FenceTiming::try_new(Duration::from_secs(1), Duration::from_secs(3))
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

/// The run's first request stops the writer and fails retryably (503); every
/// later request answers with text. Counts its requests.
struct GapModel {
    requests: AtomicUsize,
    control: Arc<GapControl>,
}

#[async_trait::async_trait]
impl LlmClient for GapModel {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> std::pin::Pin<
        Box<dyn futures::Stream<Item = Result<LlmEvent, meerkat_client::LlmError>> + Send + 'a>,
    > {
        if self.requests.fetch_add(1, Ordering::SeqCst) == 0 {
            let control = Arc::clone(&self.control);
            return Box::pin(futures::stream::once(async move {
                control.stop_the_writer().await;
                Err(meerkat_client::LlmError::ServerOverloaded)
            }));
        }
        let usage = meerkat_core::TurnUsage::host_declared(
            meerkat_core::Provider::Anthropic,
            &request.model,
            Default::default(),
        );
        Box::pin(futures::stream::iter(vec![
            Ok(LlmEvent::TextDelta {
                delta: "answered after the gap".into(),
                meta: None,
            }),
            Ok(LlmEvent::UsageUpdate { usage }),
            Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::success(meerkat_core::StopReason::EndTurn),
            }),
        ]))
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::Anthropic
    }

    async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
        Ok(())
    }
}

/// The phase and stage attempts of each of the session's input rows, when
/// the store answers.
async fn input_rows(
    store: &SpiRuntimeStore,
    session_id: &SessionId,
) -> Option<Vec<(InputLifecycleState, u32)>> {
    store
        .load_input_states(&meerkat_runtime::LogicalRuntimeId::for_session(session_id))
        .await
        .ok()
        .map(|rows| {
            rows.iter()
                .filter_map(|row| match row {
                    InputStateRow::Decoded(stored) => {
                        Some((stored.seed.phase, stored.seed.attempt_count))
                    }
                    _ => None,
                })
                .collect()
        })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_rpc_turn_whose_run_meets_a_fence_gap_is_suspended_and_runs_once_the_fence_is_back() {
    let root = tempfile::tempdir().expect("tempdir");
    let control = Arc::new(GapControl::default());
    let host: Arc<dyn HostStore> = Arc::new(GapHost {
        state: StdMutex::new(HostState::default()),
        control: Arc::clone(&control),
    });
    let realm = SpiRealm::open(
        Arc::clone(&host),
        SpiRealmConfig {
            renew_timeout: Duration::from_secs(1),
            ..SpiRealmConfig::default()
        },
    )
    .await
    .expect("the realm opens");
    let _ = control.realm.set(Arc::downgrade(&realm));
    assert_eq!(
        realm.dispatch_wait_bound(),
        Duration::from_secs(5),
        "a 2 s window, two 1 s acquires and a 1 s renew"
    );
    let runtime_store = Arc::new(
        SpiRuntimeStore::open(Arc::clone(&realm))
            .await
            .expect("the runtime store opens"),
    );
    let model = Arc::new(GapModel {
        requests: AtomicUsize::new(0),
        control: Arc::clone(&control),
    });
    let runtime = Arc::new(
        SessionRuntime::new(
            AgentFactory::new(root.path().join("agent-sessions")),
            Config::default(),
            4,
            meerkat::PersistenceBundle::new(
                Arc::new(meerkat::MemoryStore::new()),
                Arc::clone(&runtime_store) as Arc<dyn meerkat_runtime::RuntimeStore>,
                Arc::new(meerkat_store::MemoryBlobStore::new()),
            )
            .expect("construct runtime authority"),
            crate::router::NotificationSink::noop(),
        )
        .expect("compose the RPC session runtime"),
    );
    runtime.set_default_llm_client(Some(model.clone()));
    let session_id = runtime
        .create_session(
            AgentBuildConfig {
                llm_client_override: Some(model.clone()),
                ..AgentBuildConfig::new(MODEL)
            },
            None,
            None,
            Vec::new(),
        )
        .await
        .expect("create the session");

    let (event_tx, mut event_rx) = mpsc::channel(256);
    let drain = tokio::spawn(async move { while event_rx.recv().await.is_some() {} });
    let answer = tokio::time::timeout(
        STEP,
        runtime.start_turn_via_runtime(
            &session_id,
            "probe once".into(),
            Vec::new(),
            event_tx,
            None,
            None,
            None,
            None,
        ),
    )
    .await
    .expect("the turn answers");
    let _ = drain.await;
    let error = answer.expect_err("the suspended run's turn does not complete");
    assert_eq!(
        error
            .data
            .as_ref()
            .and_then(|data| data.get("kind"))
            .and_then(serde_json::Value::as_str),
        Some("requeued_across_stop"),
        "the turn answers that its input stays queued: {error:?}"
    );
    let answered_after = tokio::time::Instant::now().duration_since(
        control
            .stopped_at()
            .expect("the writer stopped during the run"),
    );
    assert!(
        answered_after <= realm.dispatch_wait_bound() + ANSWER_SLACK,
        "the turn is answered within one dispatch wait of the stop: {answered_after:?}"
    );

    let started = tokio::time::Instant::now();
    let mut rows = input_rows(&runtime_store, &session_id).await;
    while !rows.as_deref().is_some_and(|rows| {
        !rows.is_empty()
            && rows
                .iter()
                .all(|(phase, _)| *phase == InputLifecycleState::Consumed)
    }) && started.elapsed() < STEP
    {
        tokio::time::sleep(Duration::from_millis(100)).await;
        rows = input_rows(&runtime_store, &session_id).await;
    }
    assert_eq!(
        rows,
        Some(vec![(InputLifecycleState::Consumed, 1)]),
        "the input runs once the fence is back, in its one stage attempt"
    );
    assert_eq!(
        model.requests.load(Ordering::SeqCst),
        2,
        "the run asks the model once more, after the gap"
    );
}
