#![cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
//! A mob member's turn over a host store realm whose writer meets a store
//! fence gap. A member's runtime turn is applied through the mob's session
//! service without a reserved admission, the apply path other than the one
//! an RPC turn takes. The member's first model call stops the writer (one
//! renew never answers) and fails retryably, and every acquire fails for
//! longer than the realm's dispatch wait bound: the retry waits at the fence,
//! and when no re-acquire restores the writer the member's run is suspended,
//! not failed. Its bounded turn answers, within one dispatch wait bound of
//! the stop, that the input stays queued
//! (`BoundedTurnFailure::RequeuedAcrossStop`), and once the fence is back the
//! input runs once, in the stage attempt it already had. Unrun until the
//! candidate's validation.

use super::*;
use meerkat_core::host_store::{
    AcquireMode, CommitBatch, FenceGrant, FenceTiming, FenceToken, HeadPage, HostStore,
    HostStoreError, Row, RowIndex, ScopeKind, ScopeName, ScopeRef, VersionedHead,
};
use meerkat_runtime::RuntimeStore as _;
use meerkat_runtime::input_state::InputLifecycleState;
use meerkat_runtime::store::InputStateRow;
use meerkat_runtime::store::spi::{SpiRealm, SpiRealmConfig, SpiRuntimeStore};

/// Bound on one step of the row: it only turns a hang into a failure.
const WAIT: Duration = Duration::from_secs(60);

/// How long every acquire fails from the stop on: longer than the agent's
/// retry backoff plus the realm's 5 s dispatch wait bound (a 2 s self-fence
/// window, two acquires under the 1 s renew timeout and a renew held to the
/// 1 s interval).
const GAP: Duration = Duration::from_secs(12);

/// What the member's answer may take beyond one dispatch wait bound from
/// the stop: the agent's retry backoff before it reaches the fence.
const ANSWER_SLACK: Duration = Duration::from_secs(5);

type Service = meerkat_session::PersistentSessionService<meerkat::FactoryAgentBuilder>;

/// How the row stops the writer, shared by the host and the model.
#[derive(Default)]
struct GapControl {
    /// The next renew never answers.
    hang_next_renew: AtomicBool,
    /// Every acquire answers `Unavailable` until then.
    acquires_fail_until: Mutex<Option<tokio::time::Instant>>,
    /// The realm, once it is open.
    realm: OnceLock<std::sync::Weak<SpiRealm>>,
    /// When the writer's window read closed after the stop.
    stopped_at: Mutex<Option<tokio::time::Instant>>,
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
        let _ = tokio::time::timeout(WAIT, async {
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

/// The reference in-memory host store, on a 1 s renew interval and a 3 s
/// lease, whose renews and acquires follow `control`.
struct GapHost {
    inner: meerkat_store_conformance::InMemoryHostStore,
    control: Arc<GapControl>,
}

#[async_trait]
impl HostStore for GapHost {
    async fn acquire_fence(&self, mode: AcquireMode) -> Result<FenceGrant, HostStoreError> {
        if self.control.acquires_fail() {
            return Err(HostStoreError::Unavailable(
                "the row stops the acquires".to_string(),
            ));
        }
        self.inner.acquire_fence(mode).await
    }

    async fn renew_fence(&self, token: FenceToken) -> Result<(), HostStoreError> {
        if self.control.hang_next_renew.swap(false, Ordering::SeqCst) {
            // The realm's renew timeout gives up on this renew.
            std::future::pending::<()>().await;
        }
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
        self.inner.commit(batch).await
    }
}

/// The member's first request stops the writer and fails retryably (503);
/// every later request answers with text. Counts its requests.
struct GapClient {
    requests: AtomicUsize,
    control: Arc<GapControl>,
}

#[async_trait]
impl meerkat_client::LlmClient for GapClient {
    fn project_replay_messages(
        &self,
        messages: &[Message],
    ) -> Result<Vec<Message>, meerkat_client::LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a meerkat_client::LlmRequest,
    ) -> meerkat_client::types::LlmStream<'a> {
        let first = self.requests.fetch_add(1, Ordering::SeqCst) == 0;
        let control = Arc::clone(&self.control);
        Box::pin(async_stream::try_stream! {
            if first {
                control.stop_the_writer().await;
                Err::<(), _>(meerkat_client::LlmError::ServerOverloaded)?;
            }
            yield meerkat_client::LlmEvent::TextDelta {
                delta: "answered after the gap".to_string(),
                meta: None,
            };
            yield meerkat_client::LlmEvent::UsageUpdate {
                usage: meerkat_core::TurnUsage::host_declared(
                    Provider::OpenAI,
                    &request.model,
                    Usage::default(),
                ),
            };
            yield meerkat_client::LlmEvent::Done {
                outcome: meerkat_client::LlmDoneOutcome::success(meerkat_core::StopReason::EndTurn),
            };
        })
    }

    fn provider(&self) -> Provider {
        Provider::OpenAI
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
async fn a_member_turn_that_meets_a_fence_gap_is_suspended_and_runs_once_the_fence_is_back() {
    let root = tempfile::Builder::new()
        .prefix("meerkat-fence-gap-member-")
        .tempdir()
        .expect("fixture storage in the system temp dir");
    let root_path = root.path().canonicalize().expect("absolute fixture root");
    for directory in ["config", "runtime", "project", "context"] {
        std::fs::create_dir_all(root_path.join(directory)).expect("fixture root");
    }
    let control = Arc::new(GapControl::default());
    let host: Arc<dyn HostStore> = Arc::new(GapHost {
        inner: meerkat_store_conformance::InMemoryHostStore::with_fence_timing(
            FenceTiming::try_new(Duration::from_secs(1), Duration::from_secs(3))
                .expect("a 1 s renew interval on a 3 s lease"),
        ),
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
    let client = Arc::new(GapClient {
        requests: AtomicUsize::new(0),
        control: Arc::clone(&control),
    });
    let factory = meerkat::AgentFactory::new(root_path.join("factory-store"))
        .user_config_root(root_path.join("config"))
        .runtime_root(root_path.join("runtime"))
        .project_root(root_path.join("project"))
        .context_root(root_path.join("context"))
        .builtins(false)
        .comms(false);
    let mut config = meerkat::Config::default();
    config.agent.model = "gpt-5.5".to_string();
    let mut builder = meerkat::FactoryAgentBuilder::new(factory, config);
    builder.default_llm_client = Some(client.clone());
    let service = Arc::new(Service::new(
        builder,
        16,
        Arc::new(MemoryStore::new()),
        Arc::clone(&runtime_store) as Arc<dyn meerkat_runtime::RuntimeStore>,
        Arc::new(meerkat_store::MemoryBlobStore::new()),
    ));
    let mut definition = with_unique_mob_id(sample_definition(), "fence-gap-member");
    let lead = definition
        .profiles
        .get_mut(&ProfileName::from("lead"))
        .and_then(ProfileBinding::as_inline_mut)
        .expect("inline lead");
    lead.model = "gpt-5.5".to_string();
    lead.runtime_mode = crate::MobRuntimeMode::TurnDriven;
    let handle = MobBuilder::new(definition, MobStorage::in_memory())
        .with_session_service(service.clone())
        .with_default_llm_client(client.clone())
        .create()
        .await
        .expect("create the mob");
    let identity = AgentIdentity::from(format!("fence-gap-{}", Uuid::new_v4().simple()));
    handle
        .spawn_spec(SpawnMemberSpec::new("lead", identity.clone()))
        .await
        .expect("spawn the member");
    let session_id = handle
        .resolve_bridge_session_id(&identity)
        .await
        .expect("a session-backed member");

    let spec = BoundedResultSpec::new("fence-gap-member", 4096).expect("bounded result spec");
    let turn = handle
        .start_work_for_identity_bounded(
            identity.clone(),
            WorkSpec::new("probe once", WorkOrigin::Internal),
            HandlingMode::Queue,
            spec.clone(),
        )
        .await
        .expect("admit the member's turn");
    let answer = tokio::time::timeout(WAIT, turn.wait_bounded(spec))
        .await
        .expect("the member's turn answers");
    assert!(
        matches!(
            answer.as_ref().err().map(|error| error.failure()),
            Some(crate::BoundedTurnFailure::RequeuedAcrossStop { .. })
        ),
        "the member's turn answers that its input stays queued: {:?}",
        answer.as_ref().map(|_| ()).map_err(ToString::to_string)
    );
    let answered_after = tokio::time::Instant::now().duration_since(
        control
            .stopped_at()
            .expect("the writer stopped during the member's run"),
    );
    assert!(
        answered_after <= realm.dispatch_wait_bound() + ANSWER_SLACK,
        "the member's turn is answered within one dispatch wait of the stop: {answered_after:?}"
    );

    let started = tokio::time::Instant::now();
    let mut rows = input_rows(&runtime_store, &session_id).await;
    while !rows.as_deref().is_some_and(|rows| {
        !rows.is_empty()
            && rows
                .iter()
                .all(|(phase, _)| *phase == InputLifecycleState::Consumed)
    }) && started.elapsed() < WAIT
    {
        tokio::time::sleep(Duration::from_millis(100)).await;
        rows = input_rows(&runtime_store, &session_id).await;
    }
    let rows = rows.expect("the store answers once the fence is back");
    assert!(
        !rows.is_empty()
            && rows
                .iter()
                .all(|(phase, _)| *phase == InputLifecycleState::Consumed),
        "the member's input runs once the fence is back: {rows:?}"
    );
    assert!(
        rows.iter().all(|(_, attempts)| *attempts <= 1),
        "no stage attempt is spent on the gap: {rows:?}"
    );
    assert_eq!(
        client.requests.load(Ordering::SeqCst),
        2,
        "the member's run asks the model once more, after the gap"
    );

    let mob_id = handle.mob_id().clone();
    tokio::time::timeout(WAIT, handle.shutdown())
        .await
        .expect("mob shutdown finishes")
        .expect("mob shutdown");
    drop(handle);
    await_supervisor_route_release(&mob_id).await;
}
