//! Concurrent live-context bootstrap over a HeadCanonical store.
//!
//! The strict open must hand back its pending channel without materializing
//! the committed transcript body: on a large session that read is the
//! O(document) step, and it is also serialized behind whatever commit is in
//! flight. The summary that later reaches the provider must still cover
//! exactly the rows committed before the open, and a failed source read must
//! surface as the typed bootstrap failure, never as a silent drop.

use super::*;
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::time::{Duration, Instant};

use futures::StreamExt;
use meerkat_client::types::LlmStream;
use meerkat_client::{LlmClient, LlmError, LlmRequest, TestClient};

use crate::session_runtime::live_summary::{
    LiveContextBootstrapMode, LiveContextSummarizer, LiveContextSummaryError,
    LiveContextSummaryPolicy, LiveContextSummarySnapshot,
};
use crate::surface::{
    LiveContextPreparationFailure, LiveContextPreparationStage, LiveContextPreparationStatus,
};
use meerkat_contracts::{
    LiveOpenTransport, WireLiveExecutionIdentityOverrideV1, WireLiveExecutionIdentityVersion,
    WireLiveTransportBootstrap,
};
use meerkat_core::session_store::{
    PreparedHeadCanonicalMutation, PreparedHeadCanonicalRewriteMutation,
};
use meerkat_core::{
    HeadCanonicalAuthorityCrossing, HeadCanonicalStoreActivation, IncrementalSessionStore, Message,
    Session, SessionFilter, SessionHead, SessionHeadCas, SessionId, SessionMeta, SessionStore,
    SessionStoreError, TranscriptRewriteCommit, TranscriptRewriteRecord, TranscriptStrandId,
    VerifiedSessionHeadMaterialization,
};

/// Rows seeded before the open. Roughly `SEEDED_TURNS * SEED_TURN_BYTES` of
/// committed transcript, in the range the console voice open sees on a
/// long-lived household member.
const SEEDED_TURNS: usize = 240;
const SEED_TURN_BYTES: usize = 4 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MaterializeGate {
    Pass,
    Delay(Duration),
    /// Hold every materialization except those issued from the named tokio
    /// task context (`None` is the test's root future). A HeadCanonical
    /// commit materializes the previous committed body itself, so the test
    /// must stay able to commit turns while the summary job's read is held.
    HoldExcept(Option<tokio::task::Id>),
    Fail,
}

/// Delegating HeadCanonical session store whose head materialization (the one
/// O(document) read on the concurrent summary path) is delayed, held, or
/// failed by the test. Every other operation is forwarded unchanged.
struct GatedMaterializationStore {
    inner: Arc<crate::SqliteSessionStore>,
    gate: tokio::sync::watch::Sender<MaterializeGate>,
    materializations: AtomicUsize,
}

impl GatedMaterializationStore {
    fn new(inner: Arc<crate::SqliteSessionStore>) -> Self {
        let (gate, _) = tokio::sync::watch::channel(MaterializeGate::Pass);
        Self {
            inner,
            gate,
            materializations: AtomicUsize::new(0),
        }
    }

    fn set_gate(&self, gate: MaterializeGate) {
        self.gate.send_replace(gate);
    }

    fn materializations(&self) -> usize {
        self.materializations.load(AtomicOrdering::SeqCst)
    }

    async fn pass_gate(&self) -> Result<(), SessionStoreError> {
        self.materializations.fetch_add(1, AtomicOrdering::SeqCst);
        let mut gate = self.gate.subscribe();
        loop {
            let current = *gate.borrow_and_update();
            match current {
                MaterializeGate::Pass => return Ok(()),
                MaterializeGate::Delay(delay) => {
                    tokio::time::sleep(delay).await;
                    return Ok(());
                }
                MaterializeGate::Fail => {
                    return Err(SessionStoreError::Internal(
                        "gated head materialization failed".to_string(),
                    ));
                }
                MaterializeGate::HoldExcept(allowed) => {
                    if tokio::task::try_id() == allowed {
                        return Ok(());
                    }
                    gate.changed().await.map_err(|_| {
                        SessionStoreError::Internal("materialization gate dropped".to_string())
                    })?;
                }
            }
        }
    }
}

#[async_trait]
impl SessionStore for GatedMaterializationStore {
    async fn save(&self, session: &Session) -> Result<(), SessionStoreError> {
        self.inner.save(session).await
    }

    async fn save_transcript_rewrite(
        &self,
        session: &Session,
        commit: &TranscriptRewriteCommit,
    ) -> Result<(), SessionStoreError> {
        self.inner.save_transcript_rewrite(session, commit).await
    }

    async fn save_authoritative_projection(
        &self,
        session: &Session,
    ) -> Result<(), SessionStoreError> {
        self.inner.save_authoritative_projection(session).await
    }

    async fn save_authoritative_projection_if_current_revision(
        &self,
        session: &Session,
        expected_current_revision: Option<String>,
    ) -> Result<(), SessionStoreError> {
        self.inner
            .save_authoritative_projection_if_current_revision(session, expected_current_revision)
            .await
    }

    async fn load(&self, id: &SessionId) -> Result<Option<Session>, SessionStoreError> {
        self.inner.load(id).await
    }

    async fn list(&self, filter: SessionFilter) -> Result<Vec<SessionMeta>, SessionStoreError> {
        self.inner.list(filter).await
    }

    async fn load_meta(&self, id: &SessionId) -> Result<Option<SessionMeta>, SessionStoreError> {
        self.inner.load_meta(id).await
    }

    async fn delete(&self, id: &SessionId) -> Result<(), SessionStoreError> {
        self.inner.delete(id).await
    }

    async fn delete_if_current_revision(
        &self,
        id: &SessionId,
        expected_current_revision: &str,
    ) -> Result<bool, SessionStoreError> {
        self.inner
            .delete_if_current_revision(id, expected_current_revision)
            .await
    }

    async fn exists(&self, id: &SessionId) -> Result<bool, SessionStoreError> {
        self.inner.exists(id).await
    }

    fn as_incremental(self: Arc<Self>) -> Option<Arc<dyn IncrementalSessionStore>> {
        Some(self)
    }
}

#[async_trait]
impl IncrementalSessionStore for GatedMaterializationStore {
    async fn activate_head_canonical_store(
        &self,
    ) -> Result<HeadCanonicalStoreActivation, SessionStoreError> {
        self.inner.activate_head_canonical_store().await
    }

    async fn cross_head_canonical_authority(
        &self,
        id: &SessionId,
    ) -> Result<HeadCanonicalAuthorityCrossing, SessionStoreError> {
        self.inner.cross_head_canonical_authority(id).await
    }

    async fn append_messages(
        &self,
        id: &SessionId,
        strand: &TranscriptStrandId,
        base_seq: u64,
        messages: &[Message],
    ) -> Result<(), SessionStoreError> {
        self.inner
            .append_messages(id, strand, base_seq, messages)
            .await
    }

    async fn commit_rewrite(
        &self,
        id: &SessionId,
        record: &TranscriptRewriteRecord,
        expected: SessionHeadCas,
    ) -> Result<SessionHead, SessionStoreError> {
        self.inner.commit_rewrite(id, record, expected).await
    }

    async fn save_head(
        &self,
        head: &SessionHead,
        expected: SessionHeadCas,
    ) -> Result<(), SessionStoreError> {
        self.inner.save_head(head, expected).await
    }

    async fn load_head(&self, id: &SessionId) -> Result<Option<SessionHead>, SessionStoreError> {
        self.inner.load_head(id).await
    }

    async fn apply_prepared_head_canonical_mutation(
        &self,
        mutation: &PreparedHeadCanonicalMutation,
    ) -> Result<String, SessionStoreError> {
        self.inner
            .apply_prepared_head_canonical_mutation(mutation)
            .await
    }

    async fn apply_prepared_head_canonical_rewrite_mutation(
        &self,
        mutation: &PreparedHeadCanonicalRewriteMutation,
    ) -> Result<String, SessionStoreError> {
        self.inner
            .apply_prepared_head_canonical_rewrite_mutation(mutation)
            .await
    }

    async fn materialize_head(
        &self,
        expected: &SessionHead,
    ) -> Result<VerifiedSessionHeadMaterialization, SessionStoreError> {
        self.pass_gate().await?;
        self.inner.materialize_head(expected).await
    }

    async fn load_messages(
        &self,
        id: &SessionId,
        strand: &TranscriptStrandId,
        range: std::ops::Range<u64>,
    ) -> Result<Vec<Message>, SessionStoreError> {
        self.inner.load_messages(id, strand, range).await
    }

    async fn load_rewrites(
        &self,
        id: &SessionId,
    ) -> Result<Vec<TranscriptRewriteRecord>, SessionStoreError> {
        self.inner.load_rewrites(id).await
    }

    async fn load_canonical_head(
        &self,
        id: &SessionId,
    ) -> Result<Option<SessionHead>, SessionStoreError> {
        self.inner.load_canonical_head(id).await
    }

    async fn load_rewrite_commits(
        &self,
        id: &SessionId,
    ) -> Result<Vec<TranscriptRewriteCommit>, SessionStoreError> {
        self.inner.load_rewrite_commits(id).await
    }
}

/// What the producer was handed: the exact rows the summary covers.
#[derive(Debug, Clone)]
struct ObservedSummarySource {
    cursor: u64,
    rows: usize,
    mentions_amber: bool,
}

struct DeferredSummaryProducer {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
    observed: std::sync::Mutex<Vec<ObservedSummarySource>>,
}

impl DeferredSummaryProducer {
    fn new() -> Self {
        Self {
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
            observed: std::sync::Mutex::new(Vec::new()),
        }
    }

    fn observed(&self) -> Vec<ObservedSummarySource> {
        self.observed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

#[async_trait]
impl LiveContextSummarizer for DeferredSummaryProducer {
    async fn summarize(
        &self,
        snapshot: LiveContextSummarySnapshot<'_>,
    ) -> Result<String, LiveContextSummaryError> {
        assert_eq!(
            snapshot.llm_identity().model,
            "gpt-realtime-2",
            "summary sees the durable identity, never the voice override"
        );
        self.observed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(ObservedSummarySource {
                cursor: snapshot.canonical_message_cursor(),
                rows: snapshot.messages().len(),
                mentions_amber: snapshot.messages().iter().any(|message| {
                    matches!(message, Message::User(user) if user.text_content().contains("Amber"))
                }),
            });
        self.entered.notify_one();
        self.release.notified().await;
        Ok(format!(
            "Factual context summary covering {} canonical rows.",
            snapshot.canonical_message_cursor()
        ))
    }
}

const HELD_TURN_PROMPT: &str = "Mid-flight typed fact: the launch code is amber-713.";
const HELD_TURN_REPLY: &str = "Recorded: the launch code is amber-713.";

/// Provider stream that parks inside the turn so the actor holds an
/// uncommitted prompt ahead of the committed boundary: the busy-member shape
/// the console voice open meets.
struct HeldTurnClient {
    inner: TestClient,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

impl HeldTurnClient {
    fn new() -> Self {
        Self {
            inner: TestClient::new(vec![
                meerkat_client::LlmEvent::TextDelta {
                    delta: HELD_TURN_REPLY.to_string(),
                    meta: None,
                },
                meerkat_client::LlmEvent::Done {
                    outcome: meerkat_client::LlmDoneOutcome::Success {
                        stop_reason: meerkat_core::StopReason::EndTurn,
                    },
                },
            ]),
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
        }
    }
}

#[async_trait]
impl LlmClient for HeldTurnClient {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        self.inner.project_replay_messages(messages)
    }

    fn stream<'a>(&'a self, request: &'a LlmRequest) -> LlmStream<'a> {
        Box::pin(
            futures::stream::once(async move {
                self.entered.notify_one();
                self.release.notified().await;
                self.inner.stream(request)
            })
            .flatten(),
        )
    }

    fn provider(&self) -> meerkat_core::Provider {
        self.inner.provider()
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        self.inner.health_check().await
    }
}

struct DeferredSummaryEnvironment {
    service: Arc<crate::PersistentSessionService<crate::FactoryAgentBuilder>>,
    runtime: Arc<meerkat_runtime::MeerkatMachine>,
    live_adapter_host: Arc<meerkat_live::LiveAdapterHost>,
    member_host: Arc<crate::surface::ServiceMemberLiveHost>,
    mirror_host: Arc<crate::surface::ExperimentalGptLiveContextMirrorHost>,
    authority: Arc<ScriptedStrictOpenAuthority>,
    store: Arc<GatedMaterializationStore>,
    producer: Arc<DeferredSummaryProducer>,
    held_client: Arc<HeldTurnClient>,
    session_id: SessionId,
    /// Committed rows at the moment the open is admitted.
    seeded_rows: usize,
    execution_identity: WireLiveExecutionIdentityOverrideV1,
    _temp: tempfile::TempDir,
}

fn seeded_turn_text(index: usize) -> String {
    let mut text = format!("Historical turn {index}: ");
    let filler = "the household reviewed the day's plans and confirmed the next steps. ";
    while text.len() < SEED_TURN_BYTES {
        text.push_str(filler);
    }
    text
}

/// The environment's producer blocks until released, so every open in these
/// tests misses the pre-open bound; keep the bound short so the open stays
/// fast and the late path (delivery after the first user turn) is exercised.
const TEST_PRE_OPEN_BOUND: Duration = Duration::from_millis(50);

async fn build_environment() -> DeferredSummaryEnvironment {
    build_environment_with_pre_open_bound(TEST_PRE_OPEN_BOUND).await
}

async fn build_environment_with_pre_open_bound(
    pre_open_bound: Duration,
) -> DeferredSummaryEnvironment {
    use meerkat_core::service::{DeferredPromptPolicy, InitialTurnPolicy, SessionBuildOptions};

    let temp = tempfile::tempdir().expect("tempdir");
    let sqlite_path = temp.path().join("sessions.sqlite3");
    let store = Arc::new(GatedMaterializationStore::new(Arc::new(
        crate::SqliteSessionStore::open(sqlite_path.clone()).expect("open sqlite session store"),
    )));
    let session_store: Arc<dyn crate::SessionStore> = store.clone();
    let runtime_store = Arc::new(
        meerkat_runtime::store::SqliteRuntimeStore::new_head_canonical(sqlite_path)
            .expect("open head-canonical runtime store"),
    );
    let persistence = crate::PersistenceBundle::new(
        session_store,
        runtime_store,
        Arc::new(meerkat_store::MemoryBlobStore::new()),
    );
    let factory = crate::AgentFactory::new(temp.path().join("sessions")).builtins(false);
    let mut config = crate::Config::default();
    config.realm.insert(
        "default".to_string(),
        meerkat_core::RealmConfigSection::from_inline_api_keys(&[("openai", "test-openai-key")]),
    );
    let mut builder = crate::FactoryAgentBuilder::new(factory, config);
    let held_client = Arc::new(HeldTurnClient::new());
    builder.default_llm_client = Some(held_client.clone());
    let (service, runtime) = crate::surface::build_runtime_backed_service(builder, 4, persistence);
    let service = Arc::new(service);

    let projection = Arc::new(crate::surface::ServiceLiveProjection::new(
        Arc::clone(&service),
        Arc::clone(&runtime),
    ));
    let projection_sink: Arc<dyn meerkat_live::LiveProjectionSink> = projection.clone();
    let close_feedback: Arc<dyn meerkat_live::LiveChannelCloseFeedback> = projection.clone();
    let status_feedback: Arc<dyn meerkat_live::LiveChannelStatusFeedback> = projection.clone();
    let token_authority: Arc<dyn meerkat_live::LiveWsTokenAuthority> = projection;
    let live_adapter_host = Arc::new(meerkat_live::LiveAdapterHost::new(projection_sink));
    let ws_state = Arc::new(meerkat_live::LiveWsState::new(
        Arc::clone(&live_adapter_host),
        Arc::clone(&close_feedback),
        Arc::clone(&status_feedback),
        token_authority,
    ));
    let webrtc_state = Arc::new(meerkat_live::LiveWebrtcState::new(
        Arc::clone(&live_adapter_host),
        close_feedback,
        status_feedback,
    ));

    let session = crate::Session::new();
    let session_id = session.id().clone();
    let request = crate::CreateSessionRequest {
        injected_context: Vec::new(),
        model: "gpt-realtime-2".to_string(),
        prompt: meerkat_core::ContentInput::Text(String::new()),
        system_prompt: crate::SystemPromptOverride::Disable,
        max_tokens: None,
        event_tx: None,
        initial_turn: InitialTurnPolicy::Defer,
        deferred_prompt_policy: DeferredPromptPolicy::Discard,
        build: Some(SessionBuildOptions::default()),
        labels: None,
    };
    let service_for_executor = Arc::clone(&service);
    let runtime_for_executor = Arc::clone(&runtime);
    Box::pin(crate::surface::materialize_session(
        &service,
        &runtime,
        session,
        request,
        move |materialized_session_id| {
            crate::surface::default_persistent_executor(
                service_for_executor,
                runtime_for_executor,
                materialized_session_id,
            )
        },
    ))
    .await
    .expect("materialize deferred-summary fixture session");

    for index in 0..SEEDED_TURNS {
        service
            .append_external_user_content(
                &session_id,
                meerkat_core::ContentInput::Text(seeded_turn_text(index)),
            )
            .await
            .expect("seed committed historical turn");
    }
    let (committed, _) = service
        .export_live_context_summary_snapshot(&session_id)
        .await
        .expect("committed source after seeding");
    let seeded_rows = committed.messages().len();
    assert!(seeded_rows >= SEEDED_TURNS);
    let live = service
        .export_realtime_refresh_session_snapshot(&session_id)
        .await
        .expect("live actor snapshot after seeding");
    assert_eq!(live.messages().len(), seeded_rows);
    // The body-free boundary the open admits against must state exactly the
    // materialized committed facts.
    let boundary = service
        .observe_live_context_committed_boundary(&session_id)
        .await
        .expect("body-free committed boundary");
    assert_eq!(boundary.message_count(), seeded_rows as u64);
    assert_eq!(
        boundary.transcript_revision(),
        committed
            .transcript_revision()
            .expect("committed transcript revision")
    );
    assert_eq!(
        boundary.rewrite_generation(),
        committed
            .transcript_rewrite_generation()
            .expect("committed rewrite generation")
    );
    let committed_identity = committed
        .session_metadata()
        .expect("committed session metadata")
        .llm_identity();
    assert_eq!(
        service
            .live_session_llm_identity(&session_id)
            .await
            .expect("live identity"),
        committed_identity
    );
    let seeded_bytes = serde_json::to_vec(committed.messages())
        .expect("serialize seeded transcript")
        .len();
    eprintln!(
        "seeded {seeded_rows} committed rows ({seeded_bytes} transcript bytes) before the open"
    );

    #[cfg(feature = "comms")]
    {
        let comms: Arc<dyn meerkat_core::agent::CommsRuntime> = Arc::new(
            meerkat_comms::CommsRuntime::inproc_only(&format!(
                "gpt-live-deferred-summary-{session_id}"
            ))
            .expect("inproc comms runtime"),
        );
        runtime
            .maybe_spawn_mob_comms_drain(
                &session_id,
                comms,
                meerkat_runtime::meerkat_machine::dsl::MobId::from(
                    "mob-gpt-live-deferred-summary-test",
                ),
            )
            .await
            .expect("record mob-owned ingress");
    }

    let fallback_factory: Arc<crate::test_fixtures::realtime::ScriptedRealtimeSessionFactory> =
        Arc::new(crate::test_fixtures::realtime::ScriptedRealtimeSessionFactory::new());
    let producer = Arc::new(DeferredSummaryProducer::new());
    let member_host =
        crate::surface::ServiceMemberLiveHost::new(crate::surface::ServiceMemberLiveHostConfig {
            service: Arc::clone(&service),
            runtime_adapter: Arc::clone(&runtime),
            host: Arc::clone(&live_adapter_host),
            ws_state: Some(ws_state),
            base_url: Some("wss://deferred-summary.test".to_string()),
            session_factory: fallback_factory as Arc<dyn RealtimeSessionFactory>,
            realm_id: None,
            instance_id: None,
            backend: None,
        })
        .with_webrtc_cleanup_state(webrtc_state)
        .with_context_summary_policy(
            LiveContextSummaryPolicy::new(
                Arc::clone(&producer) as Arc<dyn LiveContextSummarizer>,
                8 * 1024 * 1024,
                1024,
                Duration::from_secs(5),
            )
            .expect("bounded host summary policy")
            .with_bootstrap_mode(LiveContextBootstrapMode::Concurrent)
            .with_pre_open_bound(pre_open_bound),
        );
    let member_host = Arc::new(member_host);

    let realm = meerkat_core::RealmId::parse("active-readiness").expect("realm");
    let identity = public_live_identity(public_live_binding(&realm));
    let mut authority = ScriptedStrictOpenAuthority::new(identity).with_client_context();
    authority.snapshot_cuts = true;
    authority.playback_policy = PublicGptLivePlaybackPolicy::ProviderManagedUnmeasured;
    let authority = Arc::new(authority);
    let authority_trait: Arc<dyn ExperimentalLiveOpenAuthorityProvider> = authority.clone();
    let downstream: Arc<dyn ExperimentalLiveBoundChannelActivator> =
        Arc::new(SerializedLifecycleTestActivator {
            runtime: Arc::clone(&runtime),
            rejected_appends: Arc::new(AtomicUsize::new(0)),
            control_release: None,
            preparation_barrier: None,
        });
    let mirror_host = crate::surface::ExperimentalGptLiveContextMirrorHost::new(
        Arc::clone(&runtime),
        Arc::clone(&member_host),
        authority_trait,
        downstream,
    );

    DeferredSummaryEnvironment {
        service,
        runtime,
        live_adapter_host,
        member_host,
        mirror_host,
        authority,
        store,
        producer,
        held_client,
        session_id,
        seeded_rows,
        execution_identity: WireLiveExecutionIdentityOverrideV1 {
            version: WireLiveExecutionIdentityVersion::V1,
            profile_id: GPT_LIVE_PUBLIC_CLIENT_CONTEXT_PROFILE_ID.to_string(),
        },
        _temp: temp,
    }
}

impl DeferredSummaryEnvironment {
    async fn open(
        &self,
    ) -> (
        crate::session_runtime::live_orchestration::ExperimentalLivePendingChannel,
        Duration,
        usize,
    ) {
        let materializations_before = self.store.materializations();
        let started = Instant::now();
        let opened = self
            .member_host
            .open_with_execution_identity(
                self.authority.as_ref(),
                &self.session_id,
                &self.execution_identity,
                None,
                None,
                Some(LiveOpenTransport::Webrtc),
            )
            .await
            .expect("strict concurrent open");
        let elapsed = started.elapsed();
        let materializations = self.store.materializations() - materializations_before;
        eprintln!(
            "concurrent open returned in {elapsed:?} with {materializations} head materialization(s) on the open path"
        );
        (opened, elapsed, materializations)
    }

    /// The user's first utterance on the channel, as the provider reports it:
    /// a user turn start pushed through the sideband, lowered by the
    /// lifecycle activator into the runtime. A held summary is released by
    /// this fact and by nothing else.
    async fn user_speaks(
        &self,
        sideband: &ControlledAmbiguousSideband,
        opened: &crate::session_runtime::live_orchestration::ExperimentalLivePendingChannel,
    ) {
        let binding = self
            .authority
            .transport
            .active_binding(&self.session_id)
            .await
            .expect("active provider binding");
        let turn = LiveSidebandTurnRef::__from_provider_observation(
            opened.channel_id(),
            "first-user-turn".into(),
            "provider-first-user-turn".into(),
        )
        .expect("user turn ref");
        sideband.push(LiveSidebandObservation::new(
            binding,
            LiveSidebandObservationKind::TurnStarted {
                turn,
                role: LiveSidebandTurnRole::User,
            },
        ));
    }

    async fn preparation_status(
        &self,
        opened: &crate::session_runtime::live_orchestration::ExperimentalLivePendingChannel,
    ) -> LiveContextPreparationStatus {
        *self
            .member_host
            .validate_experimental_live_channel_custody(
                opened.channel_id(),
                opened.pending_receipt(),
            )
            .await
            .expect("exact channel custody")
            .context_preparation()
    }

    async fn wait_for_preparation(
        &self,
        opened: &crate::session_runtime::live_orchestration::ExperimentalLivePendingChannel,
        accept: impl Fn(&LiveContextPreparationStatus) -> bool,
    ) -> LiveContextPreparationStatus {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let status = self.preparation_status(opened).await;
                if accept(&status) {
                    return status;
                }
                assert!(
                    !matches!(status, LiveContextPreparationStatus::Failed(_)),
                    "unexpected preparation failure: {status:?}"
                );
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("preparation reaches the awaited status")
    }

    /// Start a runtime turn and park it inside the provider stream, so the
    /// actor carries the uncommitted prompt ahead of the committed boundary.
    async fn start_held_turn(
        &self,
    ) -> tokio::task::JoinHandle<Result<meerkat_core::RunResult, meerkat_core::service::SessionError>>
    {
        let service = Arc::clone(&self.service);
        let runtime = Arc::clone(&self.runtime);
        let session_id = self.session_id.clone();
        let turn = tokio::spawn(async move {
            let admission = service
                .reserve_runtime_turn_admission(&session_id)
                .await
                .expect("turn admission");
            service
                .run_machine_committed_live_turn(
                    crate::MachineServiceTurnCommitProtocol::from_machine(&runtime),
                    &session_id,
                    crate::StartTurnRequest {
                        prompt: meerkat_core::ContentInput::Text(HELD_TURN_PROMPT.to_string()),
                        injected_context: Vec::new(),
                        system_prompt: None,
                        event_tx: None,
                        runtime: meerkat_core::service::StartTurnRuntimeSemantics {
                            turn_metadata: Some(
                                meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                                    execution_kind: Some(
                                        meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn,
                                    ),
                                    ..Default::default()
                                },
                            ),
                            ..Default::default()
                        },
                    },
                    admission,
                )
                .await
                .map_err(|(error, _admission)| error)
        });
        tokio::time::timeout(Duration::from_secs(20), self.held_client.entered.notified())
            .await
            .expect("the turn reaches the provider stream");
        turn
    }

    /// Answer the WebRTC offer so media activates and the provider control
    /// lane is ready to receive the bootstrap append.
    async fn activate_media(
        &self,
        opened: &crate::session_runtime::live_orchestration::ExperimentalLivePendingChannel,
    ) -> Arc<ControlledAmbiguousSideband> {
        let token = match &opened.open().transport {
            WireLiveTransportBootstrap::Webrtc { token, .. } => token.clone(),
            other => panic!("expected WebRTC bootstrap, got {other:?}"),
        };
        let binder = self
            .authority
            .bound_ready_binder_for(
                Arc::clone(&self.mirror_host) as Arc<dyn ExperimentalLiveBoundChannelActivator>,
                Arc::clone(&self.live_adapter_host),
                Arc::new(NoopPublicObservationPublisher),
            )
            .expect("scripted authority supplies atomic binder");
        let readiness = self
            .member_host
            .register_experimental_live_playback_owner(
                opened.channel_id(),
                opened.pending_receipt(),
            )
            .await
            .expect("register playback readiness");
        let sideband = self
            .authority
            .latest_sideband
            .lock()
            .await
            .clone()
            .expect("prepared sideband");
        sideband
            .acknowledge_context
            .store(true, AtomicOrdering::Release);
        let answer = self
            .member_host
            .answer_experimental_live_webrtc_offer(
                Arc::clone(&self.authority.transport) as Arc<dyn LiveWebrtcAnswerTransport>,
                binder,
                opened.channel_id().clone(),
                opened.pending_receipt(),
                readiness.readiness_receipt(),
                token,
                "initial-offer-sdp".to_string(),
            )
            .await
            .expect("answer binds the exact experimental execution");
        answer
            .delivery_custody
            .delivered()
            .await
            .expect("publish answer");
        let custody = self
            .member_host
            .validate_experimental_live_channel_custody(
                opened.channel_id(),
                opened.pending_receipt(),
            )
            .await
            .expect("active custody");
        assert!(matches!(
            custody.phase(),
            crate::surface::ExperimentalLiveChannelPhaseStatus::Active { .. }
        ));
        sideband
    }
}

// The production one-open memory reservation is process wide; nextest isolates
// tests by process, plain `cargo test` does not.
static OPEN_RESERVATION: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Open-path cost with the committed body read slowed by a fixed delay: the
/// open must not pay it. Prints the measured open duration.
#[tokio::test]
async fn concurrent_open_does_not_pay_for_a_slow_committed_body_read() {
    let _reservation = OPEN_RESERVATION.lock().await;
    let env = build_environment().await;
    let delay = Duration::from_millis(1500);
    env.store.set_gate(MaterializeGate::Delay(delay));
    let (opened, elapsed, materializations) = env.open().await;
    assert_eq!(
        materializations, 0,
        "the open path must not materialize the committed body"
    );
    assert!(
        elapsed < delay,
        "open took {elapsed:?}, at least the {delay:?} delayed body read"
    );
    env.store.set_gate(MaterializeGate::Pass);
    env.member_host
        .close_experimental_live_pending_channel(
            env.authority.as_ref(),
            opened.channel_id(),
            opened.pending_receipt(),
        )
        .await
        .expect("close pending channel");
}

/// The open returns while the committed body read is held; a turn committed
/// inside that window is delivered live after the summary and never
/// summarized, and the summary covers exactly the pre-open rows.
#[tokio::test]
async fn concurrent_open_returns_before_the_summary_source_is_read_and_covers_the_pre_open_prefix()
{
    let _reservation = OPEN_RESERVATION.lock().await;
    let env = build_environment().await;
    env.store
        .set_gate(MaterializeGate::HoldExcept(tokio::task::try_id()));
    let (opened, _, materializations) = env.open().await;
    assert_eq!(materializations, 0);
    assert_eq!(
        env.preparation_status(&opened).await,
        LiveContextPreparationStatus::Preparing(LiveContextPreparationStage::Capturing),
        "the source is still being captured when the channel is handed back"
    );
    assert!(env.producer.observed().is_empty());

    // A turn lands while the summary source is still unread.
    tokio::time::timeout(
        Duration::from_secs(20),
        env.service.append_external_user_content(
            &env.session_id,
            meerkat_core::ContentInput::Text("Newer code: Amber.".into()),
        ),
    )
    .await
    .expect("committing a turn never waits on the summary source read")
    .expect("commit a turn during the capture window");
    assert_eq!(
        env.preparation_status(&opened).await,
        LiveContextPreparationStatus::Preparing(LiveContextPreparationStage::Capturing)
    );

    env.store.set_gate(MaterializeGate::Pass);
    env.wait_for_preparation(&opened, |status| {
        *status == LiveContextPreparationStatus::Preparing(LiveContextPreparationStage::Generating)
    })
    .await;
    tokio::time::timeout(Duration::from_secs(10), env.producer.entered.notified())
        .await
        .expect("summary generation starts once the held source is read");
    let observed = env.producer.observed();
    assert_eq!(observed.len(), 1);
    assert_eq!(observed[0].cursor, env.seeded_rows as u64);
    assert_eq!(observed[0].rows, env.seeded_rows);
    assert!(
        !observed[0].mentions_amber,
        "the turn committed after admission is not summarized"
    );

    let sideband = tokio::time::timeout(Duration::from_secs(20), env.activate_media(&opened))
        .await
        .expect("media activation never waits on the summary");
    env.runtime.notify_committed_live_context(&env.session_id);
    tokio::time::timeout(
        Duration::from_secs(20),
        env.runtime.drain_live_context_outbox(&env.session_id),
    )
    .await
    .expect("queueing the later row never waits on the summary")
    .expect("queue the later row behind the bootstrap");
    assert!(sideband.context_commands.lock().await.is_empty());
    env.producer.release.notify_one();
    // Generated, but held: nothing reaches the provider until the user has
    // spoken on the channel; a summary appended into silence is spoken aloud.
    tokio::time::timeout(
        Duration::from_secs(20),
        env.runtime.drain_live_context_outbox(&env.session_id),
    )
    .await
    .expect("drain never waits on the held summary")
    .expect("drain with the summary held");
    assert!(
        sideband.context_commands.lock().await.is_empty(),
        "no append before the user speaks"
    );
    env.user_speaks(&sideband, &opened).await;
    env.wait_for_preparation(&opened, |status| {
        *status == LiveContextPreparationStatus::ProviderAcknowledged
    })
    .await;
    tokio::time::timeout(
        Duration::from_secs(20),
        env.runtime.drain_live_context_outbox(&env.session_id),
    )
    .await
    .expect("tail drain completes after the acknowledged bootstrap")
    .expect("ordered tail drain");
    let commands = sideband.context_commands.lock().await;
    assert!(
        matches!(
            commands.first(),
            Some(LiveSidebandProviderCommand::AppendThinkingContext { text, .. })
                if text.starts_with(LIVE_LATE_SUMMARY_PREFIX)
                    && text.contains(&format!(
                        "Factual context summary covering {} canonical rows.",
                        env.seeded_rows
                    ))
        ),
        "the historical summary travels first, on the quiet lane, after the first user turn"
    );
    assert!(
        commands.iter().skip(1).any(|command| matches!(
            command,
            LiveSidebandProviderCommand::AppendSessionContext { text, .. }
                | LiveSidebandProviderCommand::AppendThinkingContext { text, .. }
                if text.contains("Newer code: Amber.")
        )),
        "the turn committed during the capture window is delivered live after the summary"
    );
    assert_eq!(
        commands
            .iter()
            .filter(|command| matches!(
                command,
                LiveSidebandProviderCommand::AppendSessionContext { text, .. }
                    | LiveSidebandProviderCommand::AppendThinkingContext { text, .. }
                    if text.contains("Newer code: Amber.")
            ))
            .count(),
        1,
        "the later turn is delivered exactly once"
    );
    drop(commands);
    let provenance = env
        .authority
        .transport
        .bound_context_summary(opened.channel_id(), &env.session_id)
        .await
        .expect("summary provenance follows the exact channel");
    assert_eq!(
        provenance.canonical_message_cursor(),
        env.seeded_rows as u64
    );
    assert_eq!(env.producer.observed().len(), 1);
    env.member_host
        .close_experimental_live_pending_channel(
            env.authority.as_ref(),
            opened.channel_id(),
            opened.pending_receipt(),
        )
        .await
        .expect("close active channel");
}

/// A failed committed-body read after the open surfaces as the typed
/// bootstrap failure on the channel; media still activates and no raw history
/// is replayed in its place.
#[tokio::test]
async fn held_summary_source_failure_surfaces_as_typed_bootstrap_failure() {
    let _reservation = OPEN_RESERVATION.lock().await;
    let env = build_environment().await;
    env.store
        .set_gate(MaterializeGate::HoldExcept(tokio::task::try_id()));
    let (opened, _, materializations) = env.open().await;
    assert_eq!(materializations, 0);
    assert_eq!(
        env.preparation_status(&opened).await,
        LiveContextPreparationStatus::Preparing(LiveContextPreparationStage::Capturing)
    );
    env.store.set_gate(MaterializeGate::Fail);
    let failed = env
        .wait_for_preparation(&opened, |status| {
            matches!(status, LiveContextPreparationStatus::Failed(_))
        })
        .await;
    assert_eq!(
        failed,
        LiveContextPreparationStatus::Failed(LiveContextPreparationFailure::SourceRead)
    );
    assert!(env.producer.observed().is_empty());
    env.store.set_gate(MaterializeGate::Pass);
    let sideband = env.activate_media(&opened).await;
    assert_eq!(
        env.preparation_status(&opened).await,
        LiveContextPreparationStatus::Failed(LiveContextPreparationFailure::SourceRead)
    );
    assert!(
        sideband.context_commands.lock().await.is_empty(),
        "failed preparation cannot fall back to raw history"
    );
    assert!(
        env.authority
            .transport
            .bound_context_summary(opened.channel_id(), &env.session_id)
            .await
            .is_none()
    );
    env.member_host
        .close_experimental_live_pending_channel(
            env.authority.as_ref(),
            opened.channel_id(),
            opened.pending_receipt(),
        )
        .await
        .expect("close active channel");
}

/// The busy-member shape: a turn is parked inside the provider stream, so the
/// actor is one uncommitted prompt ahead of the committed boundary and answers
/// no actor command until the turn ends. The open must still hand back its
/// channel without an actor command or a body materialization, and the
/// summary must cover exactly the committed pre-open rows; the turn's rows
/// commit later and are delivered live behind the summary.
#[tokio::test]
async fn concurrent_open_with_a_turn_mid_flight_stays_body_free_and_summarizes_the_committed_prefix()
 {
    const HELD_TURN_RELEASE: Duration = Duration::from_secs(3);
    let _reservation = OPEN_RESERVATION.lock().await;
    let env = build_environment().await;
    // The prompt is appended to the actor's transcript before the provider
    // stream is entered, so once `entered` fires the actor is ahead of the
    // committed boundary and stays parked there.
    let turn = env.start_held_turn().await;
    assert_eq!(
        tokio::time::timeout(
            Duration::from_secs(20),
            env.service
                .observe_live_context_committed_boundary(&env.session_id),
        )
        .await
        .expect("boundary observation never waits on the parked turn")
        .expect("committed boundary")
        .message_count(),
        env.seeded_rows as u64
    );
    let releaser = {
        let client = Arc::clone(&env.held_client);
        tokio::spawn(async move {
            tokio::time::sleep(HELD_TURN_RELEASE).await;
            client.release.notify_one();
        })
    };

    env.store
        .set_gate(MaterializeGate::HoldExcept(tokio::task::try_id()));
    let (opened, elapsed, materializations) =
        tokio::time::timeout(Duration::from_secs(60), env.open())
            .await
            .expect("the open never waits on the parked turn");
    assert_eq!(
        materializations, 0,
        "a mid-flight turn must not make the open materialize the committed body"
    );
    assert!(
        elapsed < HELD_TURN_RELEASE,
        "open took {elapsed:?}: it waited for the parked turn"
    );
    assert_eq!(
        env.preparation_status(&opened).await,
        LiveContextPreparationStatus::Preparing(LiveContextPreparationStage::Capturing)
    );
    assert!(env.producer.observed().is_empty());

    // The turn finishes while the summary source is still unread: its prompt
    // and reply commit after admission.
    releaser.await.expect("releaser");
    tokio::time::timeout(Duration::from_secs(30), turn)
        .await
        .expect("held turn completes once released")
        .expect("turn task")
        .expect("turn commits");
    let committed_after = env
        .service
        .observe_live_context_committed_boundary(&env.session_id)
        .await
        .expect("committed boundary after the turn")
        .message_count();
    assert!(committed_after > env.seeded_rows as u64);
    assert_eq!(
        env.preparation_status(&opened).await,
        LiveContextPreparationStatus::Preparing(LiveContextPreparationStage::Capturing)
    );

    env.store.set_gate(MaterializeGate::Pass);
    env.wait_for_preparation(&opened, |status| {
        *status == LiveContextPreparationStatus::Preparing(LiveContextPreparationStage::Generating)
    })
    .await;
    tokio::time::timeout(Duration::from_secs(10), env.producer.entered.notified())
        .await
        .expect("summary generation starts once the source is read");
    let observed = env.producer.observed();
    assert_eq!(observed.len(), 1);
    assert_eq!(observed[0].cursor, env.seeded_rows as u64);
    assert_eq!(
        observed[0].rows, env.seeded_rows,
        "the turn that was mid-flight at admission is not summarized"
    );

    let sideband = tokio::time::timeout(Duration::from_secs(20), env.activate_media(&opened))
        .await
        .expect("media activation never waits on the summary");
    env.runtime.notify_committed_live_context(&env.session_id);
    tokio::time::timeout(
        Duration::from_secs(20),
        env.runtime.drain_live_context_outbox(&env.session_id),
    )
    .await
    .expect("queueing the turn rows never waits on the summary")
    .expect("queue the turn rows behind the bootstrap");
    assert!(sideband.context_commands.lock().await.is_empty());
    env.producer.release.notify_one();
    // Generated, but held: nothing reaches the provider until the user has
    // spoken on the channel; a summary appended into silence is spoken aloud.
    tokio::time::timeout(
        Duration::from_secs(20),
        env.runtime.drain_live_context_outbox(&env.session_id),
    )
    .await
    .expect("drain never waits on the held summary")
    .expect("drain with the summary held");
    assert!(
        sideband.context_commands.lock().await.is_empty(),
        "no append before the user speaks"
    );
    env.user_speaks(&sideband, &opened).await;
    env.wait_for_preparation(&opened, |status| {
        *status == LiveContextPreparationStatus::ProviderAcknowledged
    })
    .await;
    tokio::time::timeout(
        Duration::from_secs(20),
        env.runtime.drain_live_context_outbox(&env.session_id),
    )
    .await
    .expect("tail drain completes after the acknowledged bootstrap")
    .expect("ordered tail drain");
    let commands = sideband.context_commands.lock().await;
    assert!(
        matches!(
            commands.first(),
            Some(LiveSidebandProviderCommand::AppendThinkingContext { text, .. })
                if text.starts_with(LIVE_LATE_SUMMARY_PREFIX)
                    && text.contains(&format!(
                        "Factual context summary covering {} canonical rows.",
                        env.seeded_rows
                    ))
        ),
        "the historical summary travels first, on the quiet lane, after the first user turn"
    );
    assert!(
        commands.iter().skip(1).any(|command| matches!(
            command,
            LiveSidebandProviderCommand::AppendSessionContext { text, .. }
                | LiveSidebandProviderCommand::AppendThinkingContext { text, .. }
                if text.contains(HELD_TURN_PROMPT)
        )),
        "the turn that was mid-flight at admission is delivered live after the summary"
    );
    drop(commands);
    assert_eq!(env.producer.observed().len(), 1);
    env.member_host
        .close_experimental_live_pending_channel(
            env.authority.as_ref(),
            opened.channel_id(),
            opened.pending_receipt(),
        )
        .await
        .expect("close active channel");
}

/// A summary that is ready within the pre-open bound rides the startup
/// `session.input` as a developer item: no preparation lease, no append on
/// any lane at open, and the bound summary provenance names the boundary.
#[tokio::test]
async fn concurrent_open_seeds_a_ready_summary_as_startup_input_and_appends_nothing() {
    let _reservation = OPEN_RESERVATION.lock().await;
    let env = build_environment_with_pre_open_bound(Duration::from_secs(10)).await;
    // Release the producer before the open: the summary is ready immediately.
    env.producer.release.notify_one();
    env.store.set_gate(MaterializeGate::Pass);
    let (opened, _, _) = env.open().await;
    assert_eq!(
        env.preparation_status(&opened).await,
        LiveContextPreparationStatus::NotRequested,
        "a seeded summary needs no preparation lease"
    );
    let observed = env.producer.observed();
    assert_eq!(observed.len(), 1);
    assert_eq!(observed[0].cursor, env.seeded_rows as u64);
    let seed = env
        .authority
        .latest_initial_seed
        .lock()
        .await
        .clone()
        .and_then(|seed| seed.upgrade())
        .expect("seed custody");
    let seeded_text = match &seed.lock().await.as_ref().expect("initial seed").context {
        GptLiveSeedContext::SeededSummary(summary) => summary.text().to_string(),
        other => panic!("the summary must be seeded at creation, got {other:?}"),
    };
    assert_eq!(
        seeded_text,
        format!(
            "Factual context summary covering {} canonical rows.",
            env.seeded_rows
        )
    );
    let provenance = env
        .authority
        .transport
        .bound_context_summary(opened.channel_id(), &env.session_id)
        .await
        .expect("seeded summary provenance");
    assert_eq!(
        provenance.canonical_message_cursor(),
        env.seeded_rows as u64
    );
    let sideband = tokio::time::timeout(Duration::from_secs(20), env.activate_media(&opened))
        .await
        .expect("media activation");
    env.runtime.notify_committed_live_context(&env.session_id);
    tokio::time::timeout(
        Duration::from_secs(20),
        env.runtime.drain_live_context_outbox(&env.session_id),
    )
    .await
    .expect("drain")
    .expect("drain");
    assert!(
        sideband.context_commands.lock().await.is_empty(),
        "nothing is appended on any lane at open when the summary was seeded"
    );
    env.member_host
        .close_experimental_live_pending_channel(
            env.authority.as_ref(),
            opened.channel_id(),
            opened.pending_receipt(),
        )
        .await
        .expect("close active channel");
}
